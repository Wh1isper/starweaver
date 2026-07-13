#![allow(clippy::expect_used, clippy::too_many_lines)]

use std::sync::Arc;

use chrono::Utc;
use starweaver_context::{AgentCheckpoint, AgentRunState, ResumableState};
use starweaver_core::{
    AgentExecutionNode, AgentId, ConversationId, RunId, RunLifecycle, SessionId,
};
use starweaver_session::{
    HitlResumeClaim, RelatedRunUpdate, RunEvidenceCommit, RunRecord, RunStatus, SessionStore,
    SessionStoreError, StreamPublicationTarget, StreamPublicationTargets,
};

pub async fn assert_session_store_contract(store: Arc<dyn SessionStore>, suffix: &str) {
    let session_id = SessionId::from_string(format!("contract-session-{suffix}"));
    let source_run_id = RunId::from_string(format!("contract-source-{suffix}"));
    let conversation_id = ConversationId::from_string(format!("contract-conversation-{suffix}"));
    let mut source_state = AgentRunState::new(source_run_id.clone(), conversation_id.clone());
    source_state.status = RunLifecycle::Waiting;
    let checkpoint = AgentCheckpoint::new(AgentExecutionNode::ToolReturn, &source_state);

    store
        .commit_checkpoint(&session_id, checkpoint.clone())
        .await
        .expect("atomic checkpoint bootstrap");
    store
        .commit_checkpoint(&session_id, checkpoint.clone())
        .await
        .expect("exact checkpoint retry");
    let mut conflicting_checkpoint = checkpoint.clone();
    conflicting_checkpoint.run_step = conflicting_checkpoint.run_step.saturating_add(1);
    store
        .commit_checkpoint(&session_id, conflicting_checkpoint)
        .await
        .expect_err("checkpoint identity with a changed run step must conflict");
    let source_checkpoints = store
        .load_checkpoints(&session_id, &source_run_id)
        .await
        .expect("load checkpoint bootstrap");
    assert_eq!(source_checkpoints, vec![checkpoint.clone()]);

    let mut source_run = store
        .load_run(&session_id, &source_run_id)
        .await
        .expect("load bootstrapped run");
    assert_eq!(source_run.status, RunStatus::Waiting);
    source_run.status = RunStatus::Waiting;
    let source_context = resumable_state(&session_id, &source_run_id, &conversation_id);
    let mut source_commit = RunEvidenceCommit::new(source_run, source_context);
    source_commit.checkpoints.push(checkpoint);
    source_commit.publication_targets = StreamPublicationTargets::new(true, true);
    let committed_source = store
        .commit_run_evidence(source_commit.clone())
        .await
        .expect("commit source evidence");
    assert_eq!(committed_source.status, RunStatus::Waiting);
    store
        .commit_run_evidence(source_commit)
        .await
        .expect("exact source evidence retry");

    let pending = store
        .pending_stream_publications(&session_id)
        .await
        .expect("load publication outbox");
    assert_eq!(pending.len(), 1);
    let publication_id = pending[0].publication_id.clone();
    store
        .acknowledge_stream_publication(&publication_id, StreamPublicationTarget::Archive)
        .await
        .expect("ack archive target");
    store
        .acknowledge_stream_publication(&publication_id, StreamPublicationTarget::Archive)
        .await
        .expect("repeat archive acknowledgement");
    let pending = store
        .pending_stream_publications(&session_id)
        .await
        .expect("load replay-only publication");
    assert_eq!(pending.len(), 1);
    assert!(!pending[0].archive_pending);
    assert!(pending[0].replay_pending);
    store
        .acknowledge_stream_publication(&publication_id, StreamPublicationTarget::Replay)
        .await
        .expect("ack replay target");
    store
        .acknowledge_stream_publication(&publication_id, StreamPublicationTarget::Replay)
        .await
        .expect("repeat acknowledgement after completion");
    assert!(
        store
            .pending_stream_publications(&session_id)
            .await
            .expect("outbox drained")
            .is_empty()
    );

    let claim_id = format!("contract-resume-claim-{suffix}");
    let mut invalid_started = HitlResumeClaim::new(
        format!("invalid-started-{suffix}"),
        session_id.clone(),
        source_run_id.clone(),
        Utc::now(),
    );
    invalid_started.state = starweaver_session::HitlResumeClaimState::Started;
    store
        .claim_hitl_resume(invalid_started)
        .await
        .expect_err("claim acquisition must start in preflight");
    store
        .claim_hitl_resume(HitlResumeClaim::new(
            String::new(),
            session_id.clone(),
            source_run_id.clone(),
            Utc::now(),
        ))
        .await
        .expect_err("claim id must not be empty");
    store
        .claim_hitl_resume(HitlResumeClaim::new(
            claim_id.clone(),
            session_id.clone(),
            source_run_id.clone(),
            Utc::now(),
        ))
        .await
        .expect("claim waiting source");
    store
        .mark_hitl_resume_started(&session_id, &source_run_id, &claim_id)
        .await
        .expect("mark resume started");
    store
        .mark_hitl_resume_started(&session_id, &source_run_id, &claim_id)
        .await
        .expect("exact started retry is idempotent");

    let continuation_run_id = RunId::from_string(format!("contract-continuation-{suffix}"));
    let mut continuation_run = RunRecord::new(
        session_id.clone(),
        continuation_run_id.clone(),
        conversation_id.clone(),
    );
    continuation_run.status = RunStatus::Completed;
    continuation_run.restore_from_run_id = Some(source_run_id.clone());
    continuation_run.output_preview = Some("continued".to_string());
    let continuation_context = resumable_state(&session_id, &continuation_run_id, &conversation_id);
    let mut rejected =
        RunEvidenceCommit::new(continuation_run.clone(), continuation_context.clone());
    let mut rejected_update = RelatedRunUpdate::new(
        source_run_id.clone(),
        RunStatus::Running,
        RunStatus::Completed,
    );
    rejected_update.resume_claim_id = Some(claim_id.clone());
    rejected.related_run_updates.push(rejected_update);
    store
        .commit_run_evidence(rejected)
        .await
        .expect_err("source status compare-and-set must reject stale continuation");
    assert!(matches!(
        store.load_run(&session_id, &continuation_run_id).await,
        Err(SessionStoreError::NotFound(_))
    ));
    assert_eq!(
        store
            .load_run(&session_id, &source_run_id)
            .await
            .expect("source survives rejected continuation")
            .status,
        RunStatus::Waiting
    );

    let mut continuation_commit = RunEvidenceCommit::new(continuation_run, continuation_context);
    let mut continuation_update = RelatedRunUpdate::new(
        source_run_id.clone(),
        RunStatus::Waiting,
        RunStatus::Completed,
    );
    continuation_update.resume_claim_id = Some(claim_id);
    continuation_commit
        .related_run_updates
        .push(continuation_update);
    let committed_continuation = store
        .commit_run_evidence(continuation_commit.clone())
        .await
        .expect("atomic continuation commit");
    assert_eq!(committed_continuation.status, RunStatus::Completed);
    assert_eq!(
        store
            .load_run(&session_id, &source_run_id)
            .await
            .expect("load transitioned source")
            .status,
        RunStatus::Completed
    );
    store
        .commit_run_evidence(continuation_commit.clone())
        .await
        .expect("exact continuation retry bypasses consumed source CAS");

    let mut conflicting = continuation_commit;
    conflicting.run.output_preview = Some("different".to_string());
    store
        .commit_run_evidence(conflicting)
        .await
        .expect_err("conflicting evidence retry must fail");
}

fn resumable_state(
    session_id: &SessionId,
    run_id: &RunId,
    conversation_id: &ConversationId,
) -> ResumableState {
    ResumableState {
        agent_id: AgentId::from_string("contract-agent"),
        session_id: Some(session_id.clone()),
        run_id: Some(run_id.clone()),
        conversation_id: Some(conversation_id.clone()),
        ..ResumableState::default()
    }
}
