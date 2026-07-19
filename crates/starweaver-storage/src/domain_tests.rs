use chrono::Utc;
use rusqlite::{params, types::ValueRef};
use starweaver_context::{AgentCheckpoint, AgentRunState, ResumableState};
use starweaver_core::{AgentExecutionNode, AgentId, CheckpointId, ConversationId, RunId};
use starweaver_session::{
    AcquireRunAdmission, ApprovalDecision, ApprovalRecord, ApprovalStatus, DeferredToolRecord,
    ExecutionStatus, HitlResumeClaim, InMemorySessionStore, LOCAL_SESSION_NAMESPACE,
    RelatedRunUpdate, RunRecord, RunStatus, SessionRecord, SessionStore, SessionStoreError,
    StreamCursorRef, StreamPublicationTarget, StreamPublicationTargets,
};
use starweaver_stream::{AgentStreamEvent, AgentStreamRecord};
use starweaver_stream::{
    DisplayMessage, DisplayMessageKind, ReplayCursor, ReplayEvent, ReplayEventKind, ReplayEventLog,
    ReplayScope, ReplaySnapshot, StreamArchive,
};

use crate::{RunEvidenceCommit, SqliteStorage, domain::EvidenceWritePoint};

fn evidence_state(run: &RunRecord) -> ResumableState {
    ResumableState {
        agent_id: AgentId::from_string("storage-test-agent"),
        session_id: Some(run.session_id.clone()),
        run_id: Some(run.run_id.clone()),
        conversation_id: Some(run.conversation_id.clone()),
        ..ResumableState::default()
    }
}

fn begun_run(storage: &SqliteStorage, suffix: &str) -> RunRecord {
    let session = storage
        .create_session(
            Some("general".to_string()),
            Some(format!("session {suffix}")),
        )
        .expect("create session");
    let run = RunRecord::new(
        session.session_id,
        RunId::from_string(format!("run-{suffix}")),
        ConversationId::new(),
    );
    storage.begin_run(run).expect("begin run")
}

#[tokio::test]
async fn unified_storage_adapters_share_database_but_isolate_stream_families() {
    let storage = SqliteStorage::in_memory().expect("storage");
    let run = begun_run(&storage, "shared");
    let archive = storage.stream_archive();
    let replay = storage.replay_event_log();
    let scope = ReplayScope::run(run.run_id.as_str());
    let message = DisplayMessage::new(
        1,
        run.session_id.clone(),
        run.run_id.clone(),
        DisplayMessageKind::RunCompleted,
    )
    .with_preview("done");
    archive
        .append_display_messages(scope.clone(), vec![message.clone()])
        .await
        .expect("append display");
    assert_eq!(
        archive
            .replay_display_after(&scope, None)
            .await
            .expect("persisted display"),
        vec![message]
    );
    assert!(
        replay
            .replay_after(&scope, None, None)
            .await
            .expect("separate replay family")
            .is_empty()
    );

    let display_snapshot = ReplaySnapshot {
        scope: Some(scope.clone()),
        revision: 1,
        cursor: Some(ReplayCursor::display(scope.clone(), 1)),
        ..ReplaySnapshot::default()
    };
    let event_snapshot = ReplaySnapshot {
        scope: Some(scope.clone()),
        revision: 2,
        cursor: Some(ReplayCursor::replay_event(scope.clone(), 2)),
        ..ReplaySnapshot::default()
    };
    archive
        .append_snapshot(scope.clone(), display_snapshot.clone())
        .await
        .expect("save display snapshot");
    replay
        .save_snapshot(scope.clone(), event_snapshot.clone())
        .expect("save event snapshot");
    assert_eq!(
        archive
            .latest_snapshot(&scope)
            .await
            .expect("load display snapshot"),
        Some(display_snapshot)
    );
    assert_eq!(
        replay
            .compact_snapshot(&scope)
            .await
            .expect("load event snapshot"),
        event_snapshot
    );
}

#[test]
fn begin_run_assigns_unique_sequences_and_rejects_conflicting_retries() {
    let storage = SqliteStorage::in_memory().expect("storage");
    let session = storage.create_session(None, None).expect("create session");
    let first = storage
        .begin_run(RunRecord::new(
            session.session_id.clone(),
            RunId::from_string("run-first"),
            ConversationId::new(),
        ))
        .expect("first run");
    let second_input = RunRecord::new(
        session.session_id,
        RunId::from_string("run-second"),
        ConversationId::new(),
    );
    let second = storage.begin_run(second_input.clone()).expect("second run");
    assert_eq!((first.sequence_no, second.sequence_no), (1, 2));
    assert_eq!(
        storage.begin_run(second_input).expect("idempotent retry"),
        second
    );
    let mut conflicting = second.clone();
    conflicting.output_preview = Some("different".to_string());
    assert!(
        storage
            .begin_run(conflicting)
            .expect_err("conflicting retry")
            .to_string()
            .contains("run conflict")
    );
}

#[test]
fn session_store_allocates_sequences_atomically_and_keeps_them_immutable() {
    use std::sync::{Arc, Barrier};

    let tempdir = tempfile::tempdir().expect("tempdir");
    let database_path = tempdir.path().join("run-sequences.sqlite3");
    let storage = SqliteStorage::open(&database_path).expect("first storage connection");
    let session = storage.create_session(None, None).expect("create session");
    let store = storage.session_store();
    let independent_store = SqliteStorage::open(&database_path)
        .expect("second storage connection")
        .session_store();
    let barrier = Arc::new(Barrier::new(2));
    let mut workers = Vec::new();
    for (suffix, worker_store) in ["first", "second"]
        .into_iter()
        .zip([store.clone(), independent_store])
    {
        let store = worker_store;
        let barrier = Arc::clone(&barrier);
        let session_id = session.session_id.clone();
        workers.push(std::thread::spawn(move || {
            let run = RunRecord::new(
                session_id,
                RunId::from_string(format!("run-{suffix}")),
                ConversationId::new(),
            );
            barrier.wait();
            store
                .append_run_allocated(run)
                .expect("append allocated run")
        }));
    }
    let mut runs = workers
        .into_iter()
        .map(|worker| worker.join().expect("join run allocator"))
        .collect::<Vec<_>>();
    runs.sort_by_key(|run| run.sequence_no);
    assert_eq!(
        runs.iter().map(|run| run.sequence_no).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(
        storage
            .list_runs(&session.session_id)
            .expect("list persisted runs")
            .into_iter()
            .map(|run| run.sequence_no)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );

    let mut updated = runs[0].clone();
    updated.sequence_no = 0;
    updated.status = RunStatus::Waiting;
    let updated = store
        .append_run_allocated(updated)
        .expect("reuse allocated sequence");
    assert_eq!(updated.sequence_no, runs[0].sequence_no);

    let mut conflicting = updated;
    conflicting.sequence_no += 1;
    assert!(
        store
            .append_run_allocated(conflicting)
            .expect_err("sequence mutation must fail")
            .to_string()
            .contains("run sequence is immutable")
    );
}

#[test]
fn sqlite_store_clears_active_run_for_every_terminal_status() {
    let storage = SqliteStorage::in_memory().expect("storage");
    let store = storage.session_store();
    for (suffix, status) in [
        ("completed", RunStatus::Completed),
        ("failed", RunStatus::Failed),
        ("cancelled", RunStatus::Cancelled),
    ] {
        let session = storage
            .create_session(None, Some(format!("session {suffix}")))
            .expect("create session");
        let run_id = RunId::from_string(format!("run-{suffix}"));
        let run = store
            .append_run_allocated(RunRecord::new(
                session.session_id.clone(),
                run_id.clone(),
                ConversationId::new(),
            ))
            .expect("append active run");
        assert_eq!(
            storage
                .load_session(&session.session_id)
                .expect("load active session")
                .active_run_id,
            Some(run_id.clone())
        );

        let mut terminal = run;
        terminal.status = status;
        store
            .append_run_allocated(terminal)
            .expect("append terminal run");
        let persisted_session = storage
            .load_session(&session.session_id)
            .expect("load terminal session");
        assert_eq!(persisted_session.active_run_id, None);
        assert_eq!(
            persisted_session.head_success_run_id,
            (status == RunStatus::Completed).then_some(run_id)
        );
    }
}

#[tokio::test]
async fn commit_run_evidence_is_atomic_and_resume_uses_run_context() {
    let storage = SqliteStorage::in_memory().expect("storage");
    let mut run = begun_run(&storage, "commit");
    let mut state = evidence_state(&run);
    state.started_at = Utc::now();
    state
        .notes
        .insert("source".to_string(), "selected-run".to_string());
    let mut runtime_state = AgentRunState::new(run.run_id.clone(), run.conversation_id.clone());
    runtime_state.run_step = 2;
    let checkpoint = AgentCheckpoint::new(AgentExecutionNode::RunStart, &runtime_state);
    let raw = AgentStreamRecord::new(
        3,
        AgentStreamEvent::RunComplete {
            run_id: run.run_id.clone(),
            output: "done".to_string(),
        },
    );
    let approval = ApprovalRecord::new(
        "approval-commit",
        run.session_id.clone(),
        run.run_id.clone(),
        "call-1",
        "shell",
    );
    let deferred = DeferredToolRecord::new(
        "deferred-commit",
        run.session_id.clone(),
        run.run_id.clone(),
        "call-2",
        "remote",
    );
    run.status = RunStatus::Completed;
    run.output_preview = Some("done".to_string());
    let mut commit = RunEvidenceCommit::new(run.clone(), state.clone());
    commit.environment_state = Some(serde_json::json!({
        "schema": "starweaver.environment.state",
        "version": 1,
        "payload": {"provider_id": "test"}
    }));
    commit.stream_records = vec![raw.clone()];
    commit.checkpoints = vec![checkpoint.clone()];
    commit.approvals = vec![approval.clone()];
    commit.deferred_tools = vec![deferred.clone()];
    let display = DisplayMessage::new(
        0,
        run.session_id.clone(),
        run.run_id.clone(),
        DisplayMessageKind::RunCompleted,
    )
    .with_preview("done");
    let display_snapshot = ReplaySnapshot {
        scope: Some(ReplayScope::run(run.run_id.as_str())),
        revision: 1,
        display_messages: vec![display.clone()],
        ..ReplaySnapshot::default()
    };
    commit.display_messages = vec![display.clone()];
    commit.display_snapshot = Some(display_snapshot.clone());
    let committed = storage
        .commit_run_evidence(commit.clone())
        .expect("commit evidence");
    assert_eq!(
        committed
            .latest_checkpoint
            .as_ref()
            .map(|value| &value.checkpoint_id),
        Some(&checkpoint.checkpoint_id)
    );
    storage
        .commit_run_evidence(commit)
        .expect("idempotent evidence retry");

    let snapshot = storage
        .session_store()
        .resume_snapshot(&run.session_id, &run.run_id)
        .await
        .expect("resume snapshot");
    assert_eq!(snapshot.state, state);
    assert_eq!(snapshot.stream_records, vec![raw]);
    assert_eq!(snapshot.approvals, vec![approval]);
    assert_eq!(snapshot.deferred_tools, vec![deferred]);
    let archive = storage.stream_archive();
    assert_eq!(
        archive
            .replay_display_after(&ReplayScope::run(run.run_id.as_str()), None)
            .await
            .expect("display replay"),
        vec![display]
    );
    assert_eq!(
        archive
            .latest_snapshot(&ReplayScope::run(run.run_id.as_str()))
            .await
            .expect("display snapshot"),
        Some(display_snapshot)
    );
    assert_eq!(
        storage
            .load_run_environment(&run.session_id, &run.run_id)
            .expect("environment state"),
        Some(serde_json::json!({
            "schema": "starweaver.environment.state",
            "version": 1,
            "payload": {"provider_id": "test"}
        }))
    );
}

#[test]
fn commit_run_evidence_rolls_back_every_child_on_late_failure() {
    let storage = SqliteStorage::in_memory().expect("storage");
    let mut run = begun_run(&storage, "rollback");
    let approval = ApprovalRecord::new(
        "approval-rollback",
        run.session_id.clone(),
        run.run_id.clone(),
        "call",
        "shell",
    );
    {
        let connection = storage.lock().expect("connection");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_evidence_approval
                 BEFORE INSERT ON approval_records
                 BEGIN
                   SELECT RAISE(ABORT, 'injected evidence failure');
                 END;",
            )
            .expect("trigger");
    }
    run.status = RunStatus::Completed;
    let raw = AgentStreamRecord::new(
        1,
        AgentStreamEvent::RunComplete {
            run_id: run.run_id.clone(),
            output: "done".to_string(),
        },
    );
    let mut commit = RunEvidenceCommit::new(run.clone(), evidence_state(&run));
    commit.stream_records = vec![raw];
    commit.approvals = vec![approval];
    let error = storage
        .commit_run_evidence(commit)
        .expect_err("commit must fail");
    assert!(error.to_string().contains("injected evidence failure"));
    let connection = storage.lock().expect("connection");
    for table in ["run_context_records", "stream_records", "approval_records"] {
        let count: i64 = connection
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE session_id = ?1 AND run_id = ?2"),
                params![run.session_id.as_str(), run.run_id.as_str()],
                |row| row.get(0),
            )
            .expect("count rows");
        assert_eq!(count, 0, "{table} must roll back");
    }
}

#[tokio::test]
async fn latest_checkpoint_uses_persistence_order_within_one_run_step() {
    let storage = SqliteStorage::in_memory().expect("storage");
    let mut run = begun_run(&storage, "checkpoint-order");
    let mut runtime_state = AgentRunState::new(run.run_id.clone(), run.conversation_id.clone());
    runtime_state.run_step = 1;

    let mut earlier = AgentCheckpoint::new(AgentExecutionNode::ModelResponse, &runtime_state);
    earlier.checkpoint_id = CheckpointId::from_string("checkpoint-zzzz-earlier");
    storage
        .session_store()
        .commit_checkpoint(&run.session_id, earlier)
        .await
        .expect("persist earlier checkpoint");

    let mut latest = AgentCheckpoint::new(AgentExecutionNode::ToolReturn, &runtime_state);
    latest.checkpoint_id = CheckpointId::from_string("checkpoint-aaaa-latest");
    storage
        .session_store()
        .commit_checkpoint(&run.session_id, latest.clone())
        .await
        .expect("persist latest checkpoint");

    run = storage
        .session_store()
        .load_run(&run.session_id, &run.run_id)
        .await
        .expect("load run checkpoint reference");
    run.status = RunStatus::Waiting;
    let mut checkpoints = storage
        .session_store()
        .load_checkpoints(&run.session_id, &run.run_id)
        .await
        .expect("load checkpoints");
    checkpoints.reverse();
    let mut commit = RunEvidenceCommit::new(run.clone(), evidence_state(&run));
    commit.checkpoints = checkpoints;
    let committed = storage
        .commit_run_evidence(commit)
        .expect("commit complete evidence");
    assert_eq!(
        committed
            .latest_checkpoint
            .as_ref()
            .map(|reference| &reference.checkpoint_id),
        Some(&latest.checkpoint_id)
    );

    let snapshot = storage
        .session_store()
        .resume_snapshot(&run.session_id, &run.run_id)
        .await
        .expect("resume snapshot");
    assert_eq!(snapshot.latest_checkpoint, Some(latest));
}

#[tokio::test]
async fn memory_and_sqlite_evidence_commits_preserve_the_same_caller_timestamp() {
    let memory = InMemorySessionStore::new();
    let sqlite = SqliteStorage::in_memory().expect("storage").session_store();
    let session_id = starweaver_core::SessionId::from_string("timestamp-parity-session");
    let session = SessionRecord::new(session_id.clone());
    memory
        .save_session(session.clone())
        .await
        .expect("memory session");
    sqlite.save_session(session).await.expect("sqlite session");

    let mut run = RunRecord::new(
        session_id,
        RunId::from_string("timestamp-parity-run"),
        ConversationId::from_string("timestamp-parity-conversation"),
    );
    memory.append_run(run.clone()).await.expect("memory run");
    sqlite.append_run(run.clone()).await.expect("sqlite run");
    run.status = RunStatus::Completed;
    run.updated_at = Utc::now();
    let expected = run.updated_at;
    let commit = RunEvidenceCommit::new(run.clone(), evidence_state(&run));

    let memory_run = memory
        .commit_run_evidence(commit.clone())
        .await
        .expect("memory evidence");
    let sqlite_run = sqlite
        .commit_run_evidence(commit)
        .await
        .expect("sqlite evidence");
    assert_eq!(memory_run.updated_at, expected);
    assert_eq!(sqlite_run.updated_at, expected);
    assert_eq!(memory_run.updated_at, sqlite_run.updated_at);
}

#[tokio::test]
async fn stream_publication_outbox_is_atomic_acknowledged_and_not_recreated_by_retry() {
    let storage = SqliteStorage::in_memory().expect("storage");
    let mut run = begun_run(&storage, "publication-outbox");
    run.status = RunStatus::Completed;
    let mut commit = RunEvidenceCommit::new(run.clone(), evidence_state(&run));
    commit.publication_targets = StreamPublicationTargets::new(true, false);

    storage
        .commit_run_evidence(commit.clone())
        .expect("commit evidence and outbox");
    let store = storage.session_store();
    let pending = store
        .pending_stream_publications(&run.session_id)
        .await
        .expect("pending publication");
    assert_eq!(pending.len(), 1);
    store
        .acknowledge_stream_publication(
            &pending[0].publication_id,
            StreamPublicationTarget::Archive,
        )
        .await
        .expect("ack archive");
    assert!(
        store
            .pending_stream_publications(&run.session_id)
            .await
            .expect("drained publication")
            .is_empty()
    );

    storage
        .commit_run_evidence(commit)
        .expect("exact evidence retry");
    assert!(
        store
            .pending_stream_publications(&run.session_id)
            .await
            .expect("retry must not recreate publication")
            .is_empty()
    );
}

#[test]
fn legacy_unsealed_run_marker_rejects_post_migration_evidence_overwrite() {
    let storage = SqliteStorage::in_memory().expect("storage");
    let mut run = begun_run(&storage, "legacy-unsealed");
    run.status = RunStatus::Completed;
    {
        let connection = storage.lock().expect("connection");
        connection
            .execute(
                "INSERT INTO run_evidence_commits
                 (session_id, run_id, digest, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![
                    run.session_id.as_str(),
                    run.run_id.as_str(),
                    "legacy-unsealed:v1",
                    run.updated_at.to_rfc3339()
                ],
            )
            .expect("legacy seal");
    }

    let error = storage
        .commit_run_evidence(RunEvidenceCommit::new(run.clone(), evidence_state(&run)))
        .expect_err("legacy run must never accept a first modern evidence bundle");
    assert!(error.to_string().contains("run evidence conflict"));
    assert_ne!(
        storage
            .load_run(&run.session_id, &run.run_id)
            .expect("unchanged legacy run")
            .status,
        RunStatus::Completed
    );
}

#[test]
fn stream_publication_outbox_failure_rolls_back_complete_evidence() {
    let storage = SqliteStorage::in_memory().expect("storage");
    let mut run = begun_run(&storage, "publication-outbox-failure");
    run.status = RunStatus::Completed;
    {
        let connection = storage.lock().expect("connection");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_publication_outbox
                 BEFORE INSERT ON stream_publication_outbox
                 BEGIN
                   SELECT RAISE(ABORT, 'injected outbox failure');
                 END;",
            )
            .expect("trigger");
    }
    let mut commit = RunEvidenceCommit::new(run.clone(), evidence_state(&run));
    commit.publication_targets = StreamPublicationTargets::new(true, false);
    let error = storage
        .commit_run_evidence(commit)
        .expect_err("outbox write must roll back evidence");
    assert!(error.to_string().contains("injected outbox failure"));
    assert_ne!(
        storage
            .load_run(&run.session_id, &run.run_id)
            .expect("original run")
            .status,
        RunStatus::Completed
    );
    let connection = storage.lock().expect("connection");
    let context_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM run_context_records WHERE session_id = ?1 AND run_id = ?2",
            params![run.session_id.as_str(), run.run_id.as_str()],
            |row| row.get(0),
        )
        .expect("count context");
    assert_eq!(context_count, 0);
}

#[tokio::test]
async fn continuation_related_run_update_rolls_back_on_late_replay_failure() {
    let storage = SqliteStorage::in_memory().expect("storage");
    let mut source = begun_run(&storage, "continuation-source");
    source.status = RunStatus::Waiting;
    storage
        .commit_run_evidence(RunEvidenceCommit::new(
            source.clone(),
            evidence_state(&source),
        ))
        .expect("commit waiting source");

    let claim_id = "claim-continuation-source".to_string();
    storage
        .session_store()
        .claim_hitl_resume(HitlResumeClaim::new(
            claim_id.clone(),
            source.session_id.clone(),
            source.run_id.clone(),
            Utc::now(),
        ))
        .await
        .expect("claim waiting source");
    storage
        .session_store()
        .mark_hitl_resume_started(&source.session_id, &source.run_id, &claim_id)
        .await
        .expect("mark continuation started");

    let continuation_id = RunId::from_string("run-continuation-target");
    let mut continuation = RunRecord::new(
        source.session_id.clone(),
        continuation_id.clone(),
        source.conversation_id.clone(),
    );
    continuation.status = RunStatus::Completed;
    continuation.restore_from_run_id = Some(source.run_id.clone());
    let continuation_state = evidence_state(&continuation);
    let mut commit = RunEvidenceCommit::new(continuation, continuation_state);
    let mut source_update = RelatedRunUpdate::new(
        source.run_id.clone(),
        RunStatus::Waiting,
        RunStatus::Completed,
    );
    source_update.resume_claim_id = Some(claim_id);
    commit.related_run_updates.push(source_update);
    commit.replay_events.push(ReplayEvent::new(
        ReplayScope::run(continuation_id.as_str()),
        0,
        ReplayEventKind::Heartbeat,
    ));
    {
        let connection = storage.lock().expect("connection");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_continuation_replay
                 BEFORE INSERT ON replay_events
                 BEGIN
                   SELECT RAISE(ABORT, 'injected continuation failure');
                 END;",
            )
            .expect("trigger");
    }

    let error = storage
        .commit_run_evidence(commit)
        .expect_err("late continuation write must fail");
    assert!(error.to_string().contains("injected continuation failure"));
    assert_eq!(
        storage
            .load_run(&source.session_id, &source.run_id)
            .expect("source remains visible")
            .status,
        RunStatus::Waiting
    );
    assert!(matches!(
        storage.load_run(&source.session_id, &continuation_id),
        Err(SessionStoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn expired_hitl_replacement_terminalizes_source_and_consumes_started_claim() {
    let storage = SqliteStorage::in_memory().expect("storage");
    let mut source = begun_run(&storage, "expired-hitl-replacement");
    source.status = RunStatus::Waiting;
    source.updated_at = Utc::now();
    storage
        .commit_run_evidence(RunEvidenceCommit::new(
            source.clone(),
            evidence_state(&source),
        ))
        .expect("commit waiting source");

    let claim_id = "expired-hitl-replacement-claim";
    let store = storage.session_store();
    store
        .claim_hitl_resume(HitlResumeClaim::new(
            claim_id.to_string(),
            source.session_id.clone(),
            source.run_id.clone(),
            Utc::now(),
        ))
        .await
        .expect("claim waiting source");
    let replacement_run_id = RunId::from_string("run-expired-hitl-replacement-continuation");
    let mut replacement = RunRecord::new(
        source.session_id.clone(),
        replacement_run_id.clone(),
        source.conversation_id.clone(),
    );
    replacement.restore_from_run_id = Some(source.run_id.clone());
    let expires_at = Utc::now() + chrono::Duration::seconds(1);
    let reconciliation_at = expires_at + chrono::Duration::seconds(1);
    let request = AcquireRunAdmission {
        run: replacement,
        namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
        host_instance_id: "expired-hitl-replacement-host".to_string(),
        admission_id: "expired-hitl-replacement-admission".to_string(),
        lease_expires_at: expires_at,
        idempotency_key: "expired-hitl-replacement-key".to_string(),
        command_fingerprint: "expired-hitl-replacement-fingerprint".to_string(),
        replaces_waiting_run_id: Some(source.run_id.clone()),
        hitl_resume_claim_id: Some(claim_id.to_string()),
    };
    let receipt = store
        .acquire_run_admission(request.clone())
        .await
        .expect("admit already-expired waiting replacement");
    store
        .start_hitl_resume_effect(&receipt.lease, &source.run_id, claim_id)
        .await
        .expect("start the admitted continuation before simulating process loss");
    store
        .release_hitl_resume_claim(&source.session_id, &source.run_id, claim_id)
        .await
        .expect_err("admission must have started the source claim");

    storage
        .lock()
        .expect("connection")
        .execute_batch(
            "CREATE TRIGGER fail_started_claim_consumption
             BEFORE DELETE ON hitl_resume_claims
             BEGIN
               SELECT RAISE(ABORT, 'injected started claim consumption failure');
             END;",
        )
        .expect("create claim-consumption failure trigger");
    store
        .reconcile_expired_run_admissions(LOCAL_SESSION_NAMESPACE, reconciliation_at)
        .await
        .expect_err("claim-consumption failure must roll back orphan terminalization");
    assert_eq!(
        store
            .load_run(&source.session_id, &replacement_run_id)
            .await
            .expect("load replacement after failed reconciliation")
            .status,
        RunStatus::Queued
    );
    assert_eq!(
        store
            .load_run(&source.session_id, &source.run_id)
            .await
            .expect("load source after failed reconciliation")
            .status,
        RunStatus::Waiting
    );
    assert!(
        store
            .load_run_admission(&receipt.lease.target)
            .await
            .expect("load admission after failed reconciliation")
            .is_some()
    );
    storage
        .lock()
        .expect("connection")
        .execute_batch("DROP TRIGGER fail_started_claim_consumption;")
        .expect("drop claim-consumption failure trigger");

    assert_eq!(
        store
            .reconcile_expired_run_admissions(LOCAL_SESSION_NAMESPACE, reconciliation_at)
            .await
            .expect("reconcile replacement immediately after admission"),
        vec![receipt.lease.target.clone()]
    );
    assert_eq!(
        store
            .load_run(&source.session_id, &replacement_run_id)
            .await
            .expect("load terminal replacement")
            .status,
        RunStatus::Cancelled
    );
    assert_eq!(
        store
            .load_run(&source.session_id, &source.run_id)
            .await
            .expect("load terminal waiting source")
            .status,
        RunStatus::Cancelled
    );
    let claim_count: i64 = storage
        .lock()
        .expect("connection")
        .query_row(
            "SELECT COUNT(*) FROM hitl_resume_claims WHERE session_id = ?1 AND run_id = ?2",
            params![source.session_id.as_str(), source.run_id.as_str()],
            |row| row.get(0),
        )
        .expect("count consumed source claim");
    assert_eq!(claim_count, 0, "started source claim must be consumed");
    assert!(
        store
            .load_run_admission(&receipt.lease.target)
            .await
            .expect("load reconciled admission")
            .is_none()
    );
    assert!(
        store
            .reconcile_expired_run_admissions(LOCAL_SESSION_NAMESPACE, reconciliation_at)
            .await
            .expect("repeat reconciliation")
            .is_empty(),
        "terminalization and claim consumption must be at most once"
    );
    let replay = store
        .acquire_run_admission(request)
        .await
        .expect("exact admission replay remains durable");
    assert!(replay.idempotent_replay);
    store
        .claim_hitl_resume(HitlResumeClaim::new(
            "duplicate-after-expiry".to_string(),
            source.session_id.clone(),
            source.run_id.clone(),
            Utc::now(),
        ))
        .await
        .expect_err("terminal source must not admit another continuation claim");
}

#[tokio::test]
async fn cross_connection_release_cannot_delete_a_started_resume_claim() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let database_path = tempdir.path().join("resume-claim.sqlite3");
    let first = SqliteStorage::open(&database_path).expect("first storage");
    let mut run = begun_run(&first, "cross-connection-claim");
    run.status = RunStatus::Waiting;
    first
        .commit_run_evidence(RunEvidenceCommit::new(run.clone(), evidence_state(&run)))
        .expect("waiting evidence");
    let second = SqliteStorage::open(&database_path).expect("second storage");
    let claim_id = "cross-connection-claim";
    first
        .session_store()
        .claim_hitl_resume(HitlResumeClaim::new(
            claim_id.to_string(),
            run.session_id.clone(),
            run.run_id.clone(),
            Utc::now(),
        ))
        .await
        .expect("claim");
    second
        .session_store()
        .mark_hitl_resume_started(&run.session_id, &run.run_id, claim_id)
        .await
        .expect("mark from second connection");

    let error = first
        .session_store()
        .release_hitl_resume_claim(&run.session_id, &run.run_id, claim_id)
        .await
        .expect_err("started claim must not be released from a stale connection");
    assert!(error.to_string().contains("cannot be released"));
    let payload: String = first
        .lock()
        .expect("connection")
        .query_row(
            "SELECT record FROM hitl_resume_claims WHERE session_id = ?1 AND run_id = ?2",
            params![run.session_id.as_str(), run.run_id.as_str()],
            |row| row.get(0),
        )
        .expect("started claim remains");
    assert!(payload.contains("started"));
}

#[test]
fn hitl_transitions_are_idempotent_and_conflict_safe() {
    let storage = SqliteStorage::in_memory().expect("storage");
    let mut run = begun_run(&storage, "hitl");
    let approval = ApprovalRecord::new(
        "approval-hitl",
        run.session_id.clone(),
        run.run_id.clone(),
        "call-1",
        "shell",
    );
    let deferred = DeferredToolRecord::new(
        "deferred-hitl",
        run.session_id.clone(),
        run.run_id.clone(),
        "call-2",
        "remote",
    );
    run.status = RunStatus::Waiting;
    let state = evidence_state(&run);
    let mut commit = RunEvidenceCommit::new(run, state);
    commit.approvals = vec![approval];
    commit.deferred_tools = vec![deferred];
    storage
        .commit_run_evidence(commit)
        .expect("commit waiting evidence");

    let approved = storage
        .decide_approval(
            "approval-hitl",
            ApprovalStatus::Approved,
            Some("tester".to_string()),
            Some("ok".to_string()),
        )
        .expect("approve");
    assert_eq!(approved.status, ApprovalStatus::Approved);
    storage
        .decide_approval(
            "approval-hitl",
            ApprovalStatus::Approved,
            Some("tester".to_string()),
            Some("ok".to_string()),
        )
        .expect("approval retry");
    assert!(
        storage
            .decide_approval(
                "approval-hitl",
                ApprovalStatus::Denied,
                Some("tester".to_string()),
                None,
            )
            .expect_err("approval conflict")
            .to_string()
            .contains("approval conflict")
    );

    storage
        .resolve_deferred_tool(
            "deferred-hitl",
            ExecutionStatus::Completed,
            serde_json::json!({"ok": true}),
        )
        .expect("complete deferred");
    storage
        .resolve_deferred_tool(
            "deferred-hitl",
            ExecutionStatus::Completed,
            serde_json::json!({"ok": true}),
        )
        .expect("deferred retry");
    assert!(
        storage
            .resolve_deferred_tool(
                "deferred-hitl",
                ExecutionStatus::Failed,
                serde_json::json!({"error": "different"}),
            )
            .expect_err("deferred conflict")
            .to_string()
            .contains("deferred tool conflict")
    );
}

const EVIDENCE_TABLES: &[&str] = &[
    "session_records",
    "run_records",
    "run_context_records",
    "run_environment_records",
    "stream_records",
    "checkpoint_records",
    "approval_records",
    "deferred_tool_records",
    "display_message_records",
    "replay_events",
    "display_snapshot_records",
    "run_evidence_commits",
    "stream_publication_outbox",
    "hitl_resume_claims",
];

fn durable_evidence_snapshot(storage: &SqliteStorage) -> Vec<(String, Vec<Vec<String>>)> {
    let connection = storage.lock().expect("snapshot connection");
    EVIDENCE_TABLES
        .iter()
        .map(|table| {
            let mut statement = connection
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                .expect("prepare snapshot query");
            let column_count = statement.column_count();
            let rows = statement
                .query_map([], |row| {
                    (0..column_count)
                        .map(|index| match row.get_ref(index)? {
                            ValueRef::Null => Ok("null".to_string()),
                            ValueRef::Integer(value) => Ok(format!("i:{value}")),
                            ValueRef::Real(value) => Ok(format!("f:{:016x}", value.to_bits())),
                            ValueRef::Text(value) => {
                                Ok(format!("t:{}", String::from_utf8_lossy(value)))
                            }
                            ValueRef::Blob(value) => Ok(format!("b:{value:02x?}")),
                        })
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .expect("query snapshot")
                .collect::<rusqlite::Result<Vec<_>>>()
                .expect("collect snapshot");
            ((*table).to_string(), rows)
        })
        .collect()
}

fn maximal_primary_commit(storage: &SqliteStorage, suffix: &str) -> (RunRecord, RunEvidenceCommit) {
    let mut run = begun_run(storage, suffix);
    run.status = RunStatus::Completed;
    run.output_preview = Some("complete".to_string());
    let mut state = evidence_state(&run);
    state
        .notes
        .insert("fault-matrix".to_string(), suffix.to_string());
    let mut commit = RunEvidenceCommit::new(run.clone(), state);
    commit.environment_state = Some(serde_json::json!({
        "schema": "starweaver.environment.state",
        "version": 1,
        "payload": {"provider_id": "fault-matrix"}
    }));
    commit.stream_records = ["first", "second"]
        .into_iter()
        .enumerate()
        .map(|(sequence, output)| {
            AgentStreamRecord::new(
                sequence,
                AgentStreamEvent::RunComplete {
                    run_id: run.run_id.clone(),
                    output: output.to_string(),
                },
            )
        })
        .collect();
    commit.checkpoints = (1..=2)
        .map(|run_step| {
            let mut runtime_state =
                AgentRunState::new(run.run_id.clone(), run.conversation_id.clone());
            runtime_state.run_step = run_step;
            AgentCheckpoint::new(AgentExecutionNode::RunStart, &runtime_state)
        })
        .collect();
    commit.approvals = (0..2)
        .map(|index| {
            ApprovalRecord::new(
                format!("approval-{suffix}-{index}"),
                run.session_id.clone(),
                run.run_id.clone(),
                format!("approval-call-{index}"),
                "shell",
            )
        })
        .collect();
    commit.deferred_tools = (0..2)
        .map(|index| {
            DeferredToolRecord::new(
                format!("deferred-{suffix}-{index}"),
                run.session_id.clone(),
                run.run_id.clone(),
                format!("deferred-call-{index}"),
                "remote",
            )
        })
        .collect();
    let scope = ReplayScope::run(run.run_id.as_str());
    commit.display_messages = (0..2)
        .map(|sequence| {
            DisplayMessage::new(
                sequence,
                run.session_id.clone(),
                run.run_id.clone(),
                DisplayMessageKind::RunCompleted,
            )
            .with_preview(format!("display-{sequence}"))
        })
        .collect();
    commit.replay_events = (0..2)
        .map(|sequence| ReplayEvent::new(scope.clone(), sequence, ReplayEventKind::Heartbeat))
        .collect();
    commit.display_snapshot = Some(ReplaySnapshot {
        scope: Some(scope.clone()),
        revision: 1,
        cursor: Some(ReplayCursor::display(scope.clone(), 1)),
        display_messages: commit.display_messages.clone(),
        ..ReplaySnapshot::default()
    });
    commit.stream_cursors = vec![
        StreamCursorRef::new(ReplayCursor::raw_runtime(scope.clone(), 1)),
        StreamCursorRef::new(ReplayCursor::display(scope.clone(), 1)),
        StreamCursorRef::new(ReplayCursor::replay_event(scope, 1)),
    ];
    commit.publication_targets = StreamPublicationTargets::new(true, true);
    (run, commit)
}

#[test]
#[allow(clippy::too_many_lines)]
fn every_primary_evidence_write_boundary_is_atomic_after_database_reopen() {
    let points = [
        EvidenceWritePoint::PrimaryRunInitial,
        EvidenceWritePoint::RunContext,
        EvidenceWritePoint::RunEnvironment,
        EvidenceWritePoint::StreamRecord(0),
        EvidenceWritePoint::StreamRecord(1),
        EvidenceWritePoint::Checkpoint(0),
        EvidenceWritePoint::Checkpoint(1),
        EvidenceWritePoint::Approval(0),
        EvidenceWritePoint::Approval(1),
        EvidenceWritePoint::DeferredTool(0),
        EvidenceWritePoint::DeferredTool(1),
        EvidenceWritePoint::DisplayMessage(0),
        EvidenceWritePoint::DisplayMessage(1),
        EvidenceWritePoint::ReplayEvent(0),
        EvidenceWritePoint::ReplayEvent(1),
        EvidenceWritePoint::DisplaySnapshot,
        EvidenceWritePoint::PrimaryRunFinal,
        EvidenceWritePoint::Session,
        EvidenceWritePoint::EvidenceDigest,
        EvidenceWritePoint::PublicationOutbox,
        EvidenceWritePoint::TransactionCommitted,
    ];
    for (case, point) in points.into_iter().enumerate() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let database_path = tempdir.path().join(format!("primary-{case}.sqlite3"));
        let storage = SqliteStorage::open(&database_path).expect("storage");
        let (run, commit) = maximal_primary_commit(&storage, &format!("primary-{case}"));
        let old = durable_evidence_snapshot(&storage);
        let error = storage
            .commit_run_evidence_with_fault(commit.clone(), point)
            .expect_err("injected write boundary must surface");
        assert!(error.to_string().contains("injected run-evidence fault"));
        drop(storage);

        let reopened = SqliteStorage::open(&database_path).expect("reopen after fault");
        let observed = durable_evidence_snapshot(&reopened);
        if point == EvidenceWritePoint::TransactionCommitted {
            assert_eq!(
                reopened
                    .load_run(&run.session_id, &run.run_id)
                    .expect("committed run")
                    .status,
                RunStatus::Completed
            );
        } else {
            assert_eq!(observed, old, "partial state after {point:?}");
        }
        reopened
            .commit_run_evidence(commit.clone())
            .expect("exact retry after injected fault");
        let complete = durable_evidence_snapshot(&reopened);
        if point == EvidenceWritePoint::TransactionCommitted {
            assert_eq!(observed, complete, "post-commit retry changed evidence");
        }
        let persisted_run = reopened
            .load_run(&run.session_id, &run.run_id)
            .expect("complete run");
        let persisted_session = reopened
            .load_session(&run.session_id)
            .expect("complete session");
        assert_eq!(persisted_run.status, RunStatus::Completed);
        assert_eq!(persisted_run.stream_cursors, commit.stream_cursors);
        assert_eq!(persisted_session.stream_cursors, commit.stream_cursors);
        assert_eq!(persisted_session.head_run_id, Some(run.run_id.clone()));
        drop(reopened);
        let reopened_again = SqliteStorage::open(&database_path).expect("second reopen");
        assert_eq!(durable_evidence_snapshot(&reopened_again), complete);
    }
}

async fn related_fault_fixture(
    storage: &SqliteStorage,
    suffix: &str,
) -> (RunRecord, RunRecord, RunEvidenceCommit) {
    let mut source = begun_run(storage, &format!("source-{suffix}"));
    source.status = RunStatus::Waiting;
    let pending_approvals = (0..2)
        .map(|index| {
            ApprovalRecord::new(
                format!("source-approval-{suffix}-{index}"),
                source.session_id.clone(),
                source.run_id.clone(),
                format!("source-approval-call-{index}"),
                "shell",
            )
        })
        .collect::<Vec<_>>();
    let pending_deferred = (0..2)
        .map(|index| {
            DeferredToolRecord::new(
                format!("source-deferred-{suffix}-{index}"),
                source.session_id.clone(),
                source.run_id.clone(),
                format!("source-deferred-call-{index}"),
                "remote",
            )
        })
        .collect::<Vec<_>>();
    let mut source_commit = RunEvidenceCommit::new(source.clone(), evidence_state(&source));
    source_commit.approvals.clone_from(&pending_approvals);
    source_commit.deferred_tools.clone_from(&pending_deferred);
    storage
        .commit_run_evidence(source_commit)
        .expect("commit waiting source");

    let claim_id = format!("claim-{suffix}");
    let store = storage.session_store();
    store
        .claim_hitl_resume(HitlResumeClaim::new(
            claim_id.clone(),
            source.session_id.clone(),
            source.run_id.clone(),
            Utc::now(),
        ))
        .await
        .expect("claim source");
    store
        .mark_hitl_resume_started(&source.session_id, &source.run_id, &claim_id)
        .await
        .expect("start source claim");

    let mut continuation = RunRecord::new(
        source.session_id.clone(),
        RunId::from_string(format!("continuation-{suffix}")),
        source.conversation_id.clone(),
    );
    continuation.status = RunStatus::Completed;
    continuation.restore_from_run_id = Some(source.run_id.clone());
    let mut commit = RunEvidenceCommit::new(continuation.clone(), evidence_state(&continuation));
    let now = Utc::now();
    let approvals = pending_approvals
        .into_iter()
        .map(|mut approval| {
            approval.status = ApprovalStatus::Approved;
            approval.updated_at = now;
            approval.decision = Some(ApprovalDecision {
                status: ApprovalStatus::Approved,
                decided_by: Some("fault-matrix".to_string()),
                decided_at: now,
                reason: None,
                metadata: serde_json::Map::default(),
            });
            approval
        })
        .collect();
    let deferred_tools = pending_deferred
        .into_iter()
        .map(|mut deferred| {
            deferred.status = ExecutionStatus::Completed;
            deferred.response = serde_json::json!({"ok": true});
            deferred.updated_at = now;
            deferred
        })
        .collect();
    let mut update = RelatedRunUpdate::new(
        source.run_id.clone(),
        RunStatus::Waiting,
        RunStatus::Completed,
    );
    update.resume_claim_id = Some(claim_id);
    update.approvals = approvals;
    update.deferred_tools = deferred_tools;
    commit.related_run_updates.push(update);
    (source, continuation, commit)
}

#[tokio::test]
async fn every_related_run_write_boundary_is_atomic_after_database_reopen() {
    let points = [
        EvidenceWritePoint::RelatedRun(0),
        EvidenceWritePoint::RelatedApproval {
            update: 0,
            record: 0,
        },
        EvidenceWritePoint::RelatedApproval {
            update: 0,
            record: 1,
        },
        EvidenceWritePoint::RelatedDeferredTool {
            update: 0,
            record: 0,
        },
        EvidenceWritePoint::RelatedDeferredTool {
            update: 0,
            record: 1,
        },
        EvidenceWritePoint::ResumeClaimDelete(0),
    ];
    for (case, point) in points.into_iter().enumerate() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let database_path = tempdir.path().join(format!("related-{case}.sqlite3"));
        let storage = SqliteStorage::open(&database_path).expect("storage");
        let (source, continuation, commit) =
            related_fault_fixture(&storage, &format!("related-{case}")).await;
        let old = durable_evidence_snapshot(&storage);
        storage
            .commit_run_evidence_with_fault(commit.clone(), point)
            .expect_err("related write boundary must fail");
        drop(storage);

        let reopened = SqliteStorage::open(&database_path).expect("reopen after related fault");
        assert_eq!(
            durable_evidence_snapshot(&reopened),
            old,
            "partial related state after {point:?}"
        );
        assert_eq!(
            reopened
                .load_run(&source.session_id, &source.run_id)
                .expect("source run")
                .status,
            RunStatus::Waiting
        );
        assert!(matches!(
            reopened.load_run(&continuation.session_id, &continuation.run_id),
            Err(SessionStoreError::NotFound(_))
        ));
        reopened
            .commit_run_evidence(commit)
            .expect("retry complete related evidence");
        assert_eq!(
            reopened
                .load_run(&source.session_id, &source.run_id)
                .expect("completed source")
                .status,
            RunStatus::Completed
        );
        assert_eq!(
            reopened
                .load_run(&continuation.session_id, &continuation.run_id)
                .expect("completed continuation")
                .status,
            RunStatus::Completed
        );
        let completed_approvals = reopened
            .session_store()
            .load_approvals(&source.session_id, &source.run_id)
            .await
            .expect("resolved approvals");
        assert!(completed_approvals.iter().all(|approval| {
            approval.status == ApprovalStatus::Approved && approval.decision.is_some()
        }));
        let completed_deferred_tools = reopened
            .session_store()
            .load_deferred_tools(&source.session_id, &source.run_id)
            .await
            .expect("resolved deferred tools");
        assert!(completed_deferred_tools.iter().all(|deferred| {
            deferred.status == ExecutionStatus::Completed
                && deferred.response == serde_json::json!({"ok": true})
        }));
        drop(reopened);
        let reopened_again = SqliteStorage::open(&database_path).expect("second related reopen");
        assert_eq!(
            reopened_again
                .load_run(&source.session_id, &source.run_id)
                .expect("durable source")
                .status,
            RunStatus::Completed
        );
    }
}
