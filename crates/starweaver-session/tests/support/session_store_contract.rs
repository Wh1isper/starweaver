#![allow(clippy::expect_used, clippy::too_many_lines)]

use std::sync::Arc;

use chrono::Utc;
use starweaver_context::{AgentCheckpoint, AgentRunState, ResumableState};
use starweaver_core::{
    AgentExecutionNode, AgentId, ConversationId, RunId, RunLifecycle, SessionId, SubagentAttemptId,
};
use starweaver_session::{
    AcquireBackgroundSubagentContinuation, AcquireRunAdmission, ApprovalRecord, ApprovalStatus,
    BACKGROUND_SUBAGENT_RECORD_VERSION, BackgroundSubagentArtifact,
    BackgroundSubagentArtifactLimits, BackgroundSubagentContinuationCause,
    BackgroundSubagentRecord, BackgroundSubagentTerminalCommit,
    DurableBackgroundSubagentDeliveryClaim, DurableBackgroundSubagentDeliveryRelease,
    DurableBackgroundSubagentDeliveryStatus, DurableBackgroundSubagentExecutionStatus,
    DurableBackgroundSubagentOwnerLease, DurableBackgroundSubagentResultRef,
    DurableBackgroundSubagentRetentionStatus, HitlResumeAbortOutcome, HitlResumeClaim, InputPart,
    LOCAL_SESSION_NAMESPACE, RelatedRunUpdate, RunEvidenceCommit, RunRecord, RunStatus,
    SessionDeletionFence, SessionRecord, SessionStatus, SessionStore, SessionStoreError,
    StreamPublicationTarget, StreamPublicationTargets, ToolApprovalDecision,
};
use starweaver_stream::{ReplayEvent, ReplayEventKind, ReplayScope};

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
        .claim_hitl_resume(HitlResumeClaim::new(
            claim_id.clone(),
            session_id.clone(),
            source_run_id.clone(),
            Utc::now() + chrono::Duration::seconds(1),
        ))
        .await
        .expect("same deterministic preflight claim must survive process restart");
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

pub async fn assert_approval_reviewed_arguments_immutable_contract(
    store: Arc<dyn SessionStore>,
    suffix: &str,
) {
    let session_id = SessionId::from_string(format!("approval-binding-session-{suffix}"));
    let source_run_id = RunId::from_string(format!("approval-binding-source-{suffix}"));
    let continuation_run_id = RunId::from_string(format!("approval-binding-continuation-{suffix}"));
    let conversation_id =
        ConversationId::from_string(format!("approval-binding-conversation-{suffix}"));
    store
        .save_session(SessionRecord::new(session_id.clone()))
        .await
        .expect("save approval binding session");
    let mut source = RunRecord::new(
        session_id.clone(),
        source_run_id.clone(),
        conversation_id.clone(),
    );
    source.status = RunStatus::Waiting;
    store
        .append_run(source)
        .await
        .expect("park approval binding source");

    let mut pending = ApprovalRecord::new(
        format!("approval-binding-{suffix}"),
        session_id.clone(),
        source_run_id.clone(),
        format!("approval-binding-call-{suffix}"),
        "shell",
    );
    pending.reviewed_arguments = Some(serde_json::json!({
        "command": "echo safe",
        "environment": {"MODE": "safe"},
        "timeout_seconds": 10,
    }));
    store
        .append_approval(pending.clone())
        .await
        .expect("append pending approval binding");

    let claim_id = format!("approval-binding-claim-{suffix}");
    store
        .claim_hitl_resume(HitlResumeClaim::new(
            claim_id.clone(),
            session_id.clone(),
            source_run_id.clone(),
            Utc::now(),
        ))
        .await
        .expect("claim approval binding source");
    store
        .mark_hitl_resume_started(&session_id, &source_run_id, &claim_id)
        .await
        .expect("start approval binding claim");

    let mut resolved = pending.clone();
    resolved.status = ApprovalStatus::Approved;
    resolved.decision = Some(ToolApprovalDecision::approved().into_approval_decision());
    resolved.updated_at = Utc::now();
    let mut tampered = resolved.clone();
    tampered.reviewed_arguments = Some(serde_json::json!({
        "command": "echo unsafe",
        "environment": {"MODE": "unsafe"},
        "timeout_seconds": 600,
    }));

    let mut continuation = RunRecord::new(
        session_id.clone(),
        continuation_run_id.clone(),
        conversation_id.clone(),
    );
    continuation.status = RunStatus::Completed;
    continuation.restore_from_run_id = Some(source_run_id.clone());
    let context = resumable_state(&session_id, &continuation_run_id, &conversation_id);
    let mut rejected = RunEvidenceCommit::new(continuation.clone(), context.clone());
    let mut rejected_update = RelatedRunUpdate::new(
        source_run_id.clone(),
        RunStatus::Waiting,
        RunStatus::Completed,
    );
    rejected_update.resume_claim_id = Some(claim_id.clone());
    rejected_update.approvals.push(tampered);
    rejected.related_run_updates.push(rejected_update);
    store
        .commit_run_evidence(rejected)
        .await
        .expect_err("approval resolution must not rewrite reviewed arguments");
    assert!(matches!(
        store.load_run(&session_id, &continuation_run_id).await,
        Err(SessionStoreError::NotFound(_))
    ));
    assert_eq!(
        store
            .load_approvals(&session_id, &source_run_id)
            .await
            .expect("load approval after rejected rewrite"),
        vec![pending]
    );

    let mut accepted = RunEvidenceCommit::new(continuation, context);
    let mut accepted_update = RelatedRunUpdate::new(
        source_run_id.clone(),
        RunStatus::Waiting,
        RunStatus::Completed,
    );
    accepted_update.resume_claim_id = Some(claim_id);
    accepted_update.approvals.push(resolved.clone());
    accepted.related_run_updates.push(accepted_update);
    store
        .commit_run_evidence(accepted)
        .await
        .expect("resolve approval without changing reviewed arguments");
    assert_eq!(
        store
            .load_approvals(&session_id, &source_run_id)
            .await
            .expect("load resolved immutable approval"),
        vec![resolved]
    );
}

pub async fn assert_atomic_hitl_replacement_admission_contract(
    store: Arc<dyn SessionStore>,
    suffix: &str,
) {
    let session_id = SessionId::from_string(format!("hitl-admission-session-{suffix}"));
    let source_run_id = RunId::from_string(format!("hitl-admission-source-{suffix}"));
    let continuation_run_id = RunId::from_string(format!("hitl-admission-continuation-{suffix}"));
    let conversation_id =
        ConversationId::from_string(format!("hitl-admission-conversation-{suffix}"));
    store
        .save_session(SessionRecord::new(session_id.clone()))
        .await
        .expect("save HITL admission session");
    let mut source = RunRecord::new(
        session_id.clone(),
        source_run_id.clone(),
        conversation_id.clone(),
    );
    source.status = RunStatus::Waiting;
    source.sequence_no = 1;
    store
        .append_run(source)
        .await
        .expect("park HITL source run");

    let claim_id = format!("hitl-admission-claim-{suffix}");
    let mut continuation = RunRecord::new(
        session_id.clone(),
        continuation_run_id.clone(),
        conversation_id,
    );
    continuation.restore_from_run_id = Some(source_run_id.clone());
    let request = AcquireRunAdmission {
        run: continuation,
        namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
        host_instance_id: format!("hitl-admission-host-{suffix}"),
        admission_id: format!("hitl-admission-lease-{suffix}"),
        lease_expires_at: Utc::now() + chrono::Duration::minutes(1),
        idempotency_key: format!("hitl-admission-key-{suffix}"),
        command_fingerprint: format!("hitl-admission-fingerprint-{suffix}"),
        replaces_waiting_run_id: Some(source_run_id.clone()),
        hitl_resume_claim_id: Some(claim_id.clone()),
    };

    assert!(
        store
            .load_run_admission_receipt(
                &request.namespace_id,
                &request.idempotency_key,
                &request.command_fingerprint,
            )
            .await
            .expect("read-only receipt miss")
            .is_none()
    );
    let mut missing_claim = request.clone();
    missing_claim.hitl_resume_claim_id = None;
    store
        .acquire_run_admission(missing_claim)
        .await
        .expect_err("waiting replacement without a claim must fail");
    assert_waiting_source_is_still_active(
        store.as_ref(),
        &session_id,
        &source_run_id,
        &continuation_run_id,
    )
    .await;

    store
        .claim_hitl_resume(HitlResumeClaim::new(
            claim_id.clone(),
            session_id.clone(),
            source_run_id.clone(),
            Utc::now(),
        ))
        .await
        .expect("create replacement preflight claim");
    let mut wrong_claim = request.clone();
    wrong_claim.hitl_resume_claim_id = Some(format!("wrong-{suffix}"));
    store
        .acquire_run_admission(wrong_claim)
        .await
        .expect_err("waiting replacement with a foreign claim must fail");
    assert_waiting_source_is_still_active(
        store.as_ref(),
        &session_id,
        &source_run_id,
        &continuation_run_id,
    )
    .await;
    store
        .release_hitl_resume_claim(&session_id, &source_run_id, &claim_id)
        .await
        .expect("failed admission must leave the claim releasable in preflight");
    store
        .claim_hitl_resume(HitlResumeClaim::new(
            claim_id.clone(),
            session_id.clone(),
            source_run_id.clone(),
            Utc::now(),
        ))
        .await
        .expect("recreate replacement preflight claim");

    let receipt = store
        .acquire_run_admission(request.clone())
        .await
        .expect("atomically admit waiting replacement and start claim");
    assert_eq!(receipt.run.run_id, continuation_run_id);
    assert!(!receipt.idempotent_replay);
    let replay = store
        .acquire_run_admission(request.clone())
        .await
        .expect("exact admission retry must survive the started claim");
    assert!(replay.idempotent_replay);
    assert_eq!(replay.lease, receipt.lease);
    let loaded = store
        .load_run_admission_receipt(
            &request.namespace_id,
            &request.idempotency_key,
            &request.command_fingerprint,
        )
        .await
        .expect("load exact admission receipt")
        .expect("admission receipt exists");
    assert!(loaded.idempotent_replay);
    assert_eq!(loaded.lease, receipt.lease);
    store
        .load_run_admission_receipt(
            &request.namespace_id,
            &request.idempotency_key,
            &format!("different-{}", request.command_fingerprint),
        )
        .await
        .expect_err("same admission key with another fingerprint must conflict");
    store
        .release_hitl_resume_claim(&session_id, &source_run_id, &claim_id)
        .await
        .expect_err("admitted replacement must atomically make the claim non-releasable");
    assert_eq!(
        store
            .load_run(&session_id, &source_run_id)
            .await
            .expect("load waiting source after replacement")
            .status,
        RunStatus::Waiting,
        "admission must preserve source evidence until continuation finalization"
    );
    assert_eq!(
        store
            .load_session(&session_id)
            .await
            .expect("load session after replacement")
            .active_run_id,
        Some(continuation_run_id.clone())
    );
    assert_eq!(
        store
            .abort_admitted_hitl_resume(
                &receipt.lease,
                &source_run_id,
                &claim_id,
                "pre-effect launch failed",
            )
            .await
            .expect("abort admitted replacement before the effect fence"),
        HitlResumeAbortOutcome::AbortedBeforeEffect
    );
    assert_eq!(
        store
            .load_run(&session_id, &continuation_run_id)
            .await
            .expect("load aborted replacement")
            .status,
        RunStatus::Failed
    );
    assert_eq!(
        store
            .load_run(&session_id, &source_run_id)
            .await
            .expect("load retryable waiting source")
            .status,
        RunStatus::Waiting
    );
    assert_eq!(
        store
            .load_session(&session_id)
            .await
            .expect("aborted replacement must clear the active session pointer")
            .active_run_id,
        None
    );
    store
        .finalize_run_admission(&receipt.lease, RunStatus::Failed, None)
        .await
        .expect("release aborted replacement admission");
    assert!(
        store
            .load_run_admission_receipt(
                &request.namespace_id,
                &request.idempotency_key,
                &request.command_fingerprint,
            )
            .await
            .expect("load receipt after lease finalization")
            .is_some(),
        "idempotency truth must outlive the active lease"
    );
    let retry_claim_id = format!("hitl-admission-retry-claim-{suffix}");
    let retry_run_id = RunId::from_string(format!("hitl-admission-retry-run-{suffix}"));
    store
        .claim_hitl_resume(HitlResumeClaim::new(
            retry_claim_id.clone(),
            session_id.clone(),
            source_run_id.clone(),
            Utc::now(),
        ))
        .await
        .expect("aborted source must accept a new preflight claim");
    let mut retry_run = RunRecord::new(
        session_id.clone(),
        retry_run_id.clone(),
        ConversationId::from_string(format!("hitl-admission-retry-conversation-{suffix}")),
    );
    retry_run.restore_from_run_id = Some(source_run_id.clone());
    let retry = store
        .acquire_run_admission(AcquireRunAdmission {
            run: retry_run,
            namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
            host_instance_id: format!("hitl-admission-retry-host-{suffix}"),
            admission_id: format!("hitl-admission-retry-lease-{suffix}"),
            lease_expires_at: Utc::now() + chrono::Duration::minutes(1),
            idempotency_key: format!("hitl-admission-retry-key-{suffix}"),
            command_fingerprint: format!("hitl-admission-retry-fingerprint-{suffix}"),
            replaces_waiting_run_id: Some(source_run_id),
            hitl_resume_claim_id: Some(retry_claim_id),
        })
        .await
        .expect("aborted source must be retryable through a new fenced admission");
    assert_eq!(retry.run.run_id, retry_run_id);
}

pub async fn assert_started_hitl_orphan_reconciliation_contract(
    store: Arc<dyn SessionStore>,
    suffix: &str,
) {
    let session_id = SessionId::from_string(format!("hitl-orphan-session-{suffix}"));
    let source_run_id = RunId::from_string(format!("hitl-orphan-source-{suffix}"));
    let replacement_run_id = RunId::from_string(format!("hitl-orphan-replacement-{suffix}"));
    let conversation_id = ConversationId::from_string(format!("hitl-orphan-conversation-{suffix}"));
    store
        .save_session(SessionRecord::new(session_id.clone()))
        .await
        .expect("save HITL orphan session");
    let mut source = RunRecord::new(
        session_id.clone(),
        source_run_id.clone(),
        conversation_id.clone(),
    );
    source.status = RunStatus::Waiting;
    store
        .append_run(source)
        .await
        .expect("park HITL orphan source");

    let claim_id = format!("hitl-orphan-claim-{suffix}");
    store
        .claim_hitl_resume(HitlResumeClaim::new(
            claim_id.clone(),
            session_id.clone(),
            source_run_id.clone(),
            Utc::now(),
        ))
        .await
        .expect("claim HITL orphan source");
    let mut replacement = RunRecord::new(
        session_id.clone(),
        replacement_run_id.clone(),
        conversation_id,
    );
    replacement.restore_from_run_id = Some(source_run_id.clone());
    let expires_at = Utc::now() + chrono::Duration::seconds(1);
    let reconciliation_at = expires_at + chrono::Duration::seconds(1);
    let request = AcquireRunAdmission {
        run: replacement,
        namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
        host_instance_id: format!("hitl-orphan-host-{suffix}"),
        admission_id: format!("hitl-orphan-admission-{suffix}"),
        lease_expires_at: expires_at,
        idempotency_key: format!("hitl-orphan-key-{suffix}"),
        command_fingerprint: format!("hitl-orphan-fingerprint-{suffix}"),
        replaces_waiting_run_id: Some(source_run_id.clone()),
        hitl_resume_claim_id: Some(claim_id.clone()),
    };
    let receipt = store
        .acquire_run_admission(request.clone())
        .await
        .expect("admit already-expired HITL replacement");
    store
        .start_hitl_resume_effect(&receipt.lease, &source_run_id, &claim_id)
        .await
        .expect("cross the effect fence before simulating host loss");

    assert_eq!(
        store
            .reconcile_expired_run_admissions(LOCAL_SESSION_NAMESPACE, reconciliation_at)
            .await
            .expect("atomically reconcile started HITL orphan"),
        vec![receipt.lease.target.clone()]
    );
    for run_id in [&replacement_run_id, &source_run_id] {
        let run = store
            .load_run(&session_id, run_id)
            .await
            .expect("load reconciled HITL run");
        assert_eq!(run.status, RunStatus::Cancelled);
        assert_eq!(
            run.output_preview.as_deref(),
            Some("interrupted after host lease expired")
        );
        assert_eq!(
            starweaver_session::ContinuationEffectState::from_metadata(&run.metadata).unwrap(),
            Some(starweaver_session::ContinuationEffectState::indeterminate())
        );
    }
    assert_eq!(
        store
            .load_session(&session_id)
            .await
            .expect("load reconciled HITL session")
            .active_run_id,
        None
    );
    assert!(
        store
            .load_run_admission(&receipt.lease.target)
            .await
            .expect("load reconciled HITL admission")
            .is_none()
    );
    store
        .mark_hitl_resume_started(&session_id, &source_run_id, &claim_id)
        .await
        .expect_err("reconciliation must consume the exact started claim");

    let replay = store
        .acquire_run_admission(request)
        .await
        .expect("exact admission retry returns durable receipt without replaying the effect");
    assert!(replay.idempotent_replay);
    assert_eq!(replay.lease, receipt.lease);
    assert!(
        store
            .load_run_admission(&receipt.lease.target)
            .await
            .expect("idempotency retry must not restore admission")
            .is_none()
    );
    assert!(
        store
            .reconcile_expired_run_admissions(LOCAL_SESSION_NAMESPACE, reconciliation_at)
            .await
            .expect("repeat HITL orphan reconciliation")
            .is_empty(),
        "claim consumption and terminalization must be at most once"
    );
    assert_eq!(
        store
            .load_run(&session_id, &source_run_id)
            .await
            .expect("load source after retry")
            .status,
        RunStatus::Cancelled
    );
}

pub async fn assert_implicit_started_hitl_orphan_reconciliation_contract(
    store: Arc<dyn SessionStore>,
    suffix: &str,
) {
    let session_id = SessionId::from_string(format!("implicit-hitl-orphan-session-{suffix}"));
    let source_run_id = RunId::from_string(format!("implicit-hitl-orphan-source-{suffix}"));
    let replacement_run_id =
        RunId::from_string(format!("implicit-hitl-orphan-replacement-{suffix}"));
    let next_run_id = RunId::from_string(format!("implicit-hitl-orphan-next-{suffix}"));
    let conversation_id =
        ConversationId::from_string(format!("implicit-hitl-orphan-conversation-{suffix}"));
    store
        .save_session(SessionRecord::new(session_id.clone()))
        .await
        .expect("save implicit HITL orphan session");
    let mut source = RunRecord::new(
        session_id.clone(),
        source_run_id.clone(),
        conversation_id.clone(),
    );
    source.status = RunStatus::Waiting;
    store
        .append_run(source)
        .await
        .expect("park implicit HITL orphan source");

    let claim_id = format!("implicit-hitl-orphan-claim-{suffix}");
    store
        .claim_hitl_resume(HitlResumeClaim::new(
            claim_id.clone(),
            session_id.clone(),
            source_run_id.clone(),
            Utc::now(),
        ))
        .await
        .expect("claim implicit HITL orphan source");
    let mut replacement = RunRecord::new(
        session_id.clone(),
        replacement_run_id.clone(),
        conversation_id.clone(),
    );
    replacement.restore_from_run_id = Some(source_run_id.clone());
    let expired_receipt = store
        .acquire_run_admission(AcquireRunAdmission {
            run: replacement,
            namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
            host_instance_id: format!("implicit-hitl-orphan-host-{suffix}"),
            admission_id: format!("implicit-hitl-orphan-admission-{suffix}"),
            lease_expires_at: Utc::now() - chrono::Duration::seconds(1),
            idempotency_key: format!("implicit-hitl-orphan-key-{suffix}"),
            command_fingerprint: format!("implicit-hitl-orphan-fingerprint-{suffix}"),
            replaces_waiting_run_id: Some(source_run_id.clone()),
            hitl_resume_claim_id: Some(claim_id.clone()),
        })
        .await
        .expect("admit expired HITL replacement");
    // The stale lease is already expired, so use the explicit pre-effect transition only in the
    // controlled contract fixture by first moving its expiry into the future is not possible.
    // This scenario validates admitted recovery below; started recovery is covered above.

    let next_receipt = store
        .acquire_run_admission(AcquireRunAdmission {
            run: RunRecord::new(session_id.clone(), next_run_id.clone(), conversation_id),
            namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
            host_instance_id: format!("implicit-hitl-next-host-{suffix}"),
            admission_id: format!("implicit-hitl-next-admission-{suffix}"),
            lease_expires_at: Utc::now() + chrono::Duration::minutes(1),
            idempotency_key: format!("implicit-hitl-next-key-{suffix}"),
            command_fingerprint: format!("implicit-hitl-next-fingerprint-{suffix}"),
            replaces_waiting_run_id: None,
            hitl_resume_claim_id: None,
        })
        .await
        .expect("new admission atomically reconciles expired HITL replacement");

    let replacement = store
        .load_run(&session_id, &replacement_run_id)
        .await
        .expect("load implicitly reconciled replacement");
    assert_eq!(replacement.status, RunStatus::Cancelled);
    assert_eq!(
        replacement.output_preview.as_deref(),
        Some("interrupted after host lease expired")
    );
    assert_eq!(
        store
            .load_run(&session_id, &source_run_id)
            .await
            .expect("load source preserved before effect")
            .status,
        RunStatus::Waiting
    );
    assert_eq!(
        store
            .load_session(&session_id)
            .await
            .expect("load session after implicit reconciliation")
            .active_run_id,
        Some(next_run_id)
    );
    assert!(
        store
            .load_run_admission(&expired_receipt.lease.target)
            .await
            .expect("load expired admission after implicit reconciliation")
            .is_none()
    );
    assert_eq!(
        store
            .load_run_admission(&next_receipt.lease.target)
            .await
            .expect("load replacement admission after implicit reconciliation"),
        Some(next_receipt.lease)
    );
    store
        .mark_hitl_resume_started(&session_id, &source_run_id, &claim_id)
        .await
        .expect_err("implicit reconciliation must consume the exact started claim");
}

pub async fn assert_terminal_evidence_admission_cleanup_contract(
    store: Arc<dyn SessionStore>,
    suffix: &str,
) {
    let session_id = SessionId::from_string(format!("terminal-cleanup-session-{suffix}"));
    let run_id = RunId::from_string(format!("terminal-cleanup-run-{suffix}"));
    let conversation_id =
        ConversationId::from_string(format!("terminal-cleanup-conversation-{suffix}"));
    store
        .save_session(SessionRecord::new(session_id.clone()))
        .await
        .expect("save terminal cleanup session");
    let receipt = store
        .acquire_run_admission(AcquireRunAdmission {
            run: RunRecord::new(session_id.clone(), run_id.clone(), conversation_id.clone()),
            namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
            host_instance_id: format!("terminal-cleanup-host-{suffix}"),
            admission_id: format!("terminal-cleanup-admission-{suffix}"),
            lease_expires_at: Utc::now() + chrono::Duration::minutes(1),
            idempotency_key: format!("terminal-cleanup-key-{suffix}"),
            command_fingerprint: format!("terminal-cleanup-fingerprint-{suffix}"),
            replaces_waiting_run_id: None,
            hitl_resume_claim_id: None,
        })
        .await
        .expect("acquire terminal cleanup admission");

    let mut terminal = receipt.run.clone();
    terminal.status = RunStatus::Completed;
    terminal.output_preview = Some("authoritative result".to_string());
    terminal.updated_at = Utc::now();
    let committed = store
        .commit_run_evidence_fenced(
            &receipt.lease,
            RunEvidenceCommit::new(
                terminal,
                resumable_state(&session_id, &run_id, &conversation_id),
            ),
        )
        .await
        .expect("commit authoritative terminal evidence before lease release");
    assert_eq!(committed.status, RunStatus::Completed);
    assert!(
        store
            .load_run_admission(&receipt.lease.target)
            .await
            .expect("load active admission after evidence commit")
            .is_some(),
        "evidence commit and admission release are separate durability steps"
    );

    let finalized = store
        .finalize_run_admission(
            &receipt.lease,
            RunStatus::Failed,
            Some("process-local cleanup failure".to_string()),
        )
        .await
        .expect("cleanup must release the matching lease without replacing terminal evidence");
    assert_eq!(finalized.status, RunStatus::Completed);
    assert_eq!(
        finalized.output_preview.as_deref(),
        Some("authoritative result")
    );
    assert!(
        store
            .load_run_admission(&receipt.lease.target)
            .await
            .expect("load admission after cleanup")
            .is_none()
    );
    let exact_retry = store
        .finalize_run_admission(
            &receipt.lease,
            RunStatus::Completed,
            Some("authoritative result".to_string()),
        )
        .await
        .expect("exact terminal cleanup retry is idempotent");
    assert_eq!(exact_retry, finalized);
    store
        .finalize_run_admission(
            &receipt.lease,
            RunStatus::Failed,
            Some("process-local cleanup failure".to_string()),
        )
        .await
        .expect_err("released admission cannot rewrite authoritative terminal evidence");
}

async fn assert_waiting_source_is_still_active(
    store: &dyn SessionStore,
    session_id: &SessionId,
    source_run_id: &RunId,
    continuation_run_id: &RunId,
) {
    assert_eq!(
        store
            .load_run(session_id, source_run_id)
            .await
            .expect("load waiting source after rejected admission")
            .status,
        RunStatus::Waiting
    );
    assert_eq!(
        store
            .load_session(session_id)
            .await
            .expect("load session after rejected admission")
            .active_run_id
            .as_ref(),
        Some(source_run_id)
    );
    assert!(matches!(
        store.load_run(session_id, continuation_run_id).await,
        Err(SessionStoreError::NotFound(_))
    ));
}

pub async fn assert_fenced_replay_batch_contract(store: Arc<dyn SessionStore>, suffix: &str) {
    let session_id = SessionId::from_string(format!("replay-batch-session-{suffix}"));
    let run_id = RunId::from_string(format!("replay-batch-run-{suffix}"));
    store
        .save_session(SessionRecord::new(session_id.clone()))
        .await
        .expect("save replay batch session");
    let receipt = store
        .acquire_run_admission(AcquireRunAdmission {
            run: RunRecord::new(session_id.clone(), run_id.clone(), ConversationId::new()),
            namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
            host_instance_id: format!("replay-batch-host-{suffix}"),
            admission_id: format!("replay-batch-admission-{suffix}"),
            lease_expires_at: Utc::now() + chrono::Duration::minutes(1),
            idempotency_key: format!("replay-batch-key-{suffix}"),
            command_fingerprint: format!("replay-batch-fingerprint-{suffix}"),
            replaces_waiting_run_id: None,
            hitl_resume_claim_id: None,
        })
        .await
        .expect("acquire replay batch admission");
    let scope = ReplayScope::run(run_id.as_str());
    let first = ReplayEvent::new(scope.clone(), 10, ReplayEventKind::Heartbeat);
    let second = ReplayEvent::new(
        scope.clone(),
        11,
        ReplayEventKind::Raw(serde_json::json!({"batch": 1})),
    );
    let initial_batch = vec![first.clone(), second.clone()];
    store
        .append_replay_events_fenced(&receipt.lease, initial_batch.clone())
        .await
        .expect("append replay batch");
    store
        .append_replay_events_fenced(&receipt.lease, initial_batch)
        .await
        .expect("exact replay batch retry");

    let rolled_back = ReplayEvent::new(scope.clone(), 12, ReplayEventKind::Heartbeat);
    let conflicting_second = ReplayEvent::new(
        scope.clone(),
        second.sequence,
        ReplayEventKind::Raw(serde_json::json!({"batch": "conflict"})),
    );
    store
        .append_replay_events_fenced(
            &receipt.lease,
            vec![rolled_back.clone(), conflicting_second],
        )
        .await
        .expect_err("a second-event conflict must reject the whole replay batch");
    let replacement = ReplayEvent::new(
        scope.clone(),
        rolled_back.sequence,
        ReplayEventKind::Raw(serde_json::json!({"after": "rollback"})),
    );
    store
        .append_replay_events_fenced(&receipt.lease, vec![replacement.clone()])
        .await
        .expect("the first event from a rejected batch must have rolled back");
    store
        .append_replay_events_fenced(&receipt.lease, vec![replacement])
        .await
        .expect("replacement exact retry");

    let foreign_scope = ReplayEvent::new(
        ReplayScope::run("another-run"),
        13,
        ReplayEventKind::Heartbeat,
    );
    store
        .append_replay_events_fenced(&receipt.lease, vec![foreign_scope])
        .await
        .expect_err("replay event scope must match the admitted run");
    store
        .append_replay_events_fenced(
            &receipt.lease,
            vec![ReplayEvent::new(
                scope.clone(),
                13,
                ReplayEventKind::Heartbeat,
            )],
        )
        .await
        .expect("scope rejection must not reserve the sequence");

    let mut stale_admission = receipt.lease.clone();
    stale_admission.admission_id.push_str("-stale");
    store
        .append_replay_events_fenced(&stale_admission, Vec::new())
        .await
        .expect_err("stale admission id must be fenced");
    let mut stale_host = receipt.lease.clone();
    stale_host.host_instance_id.push_str("-stale");
    store
        .append_replay_events_fenced(&stale_host, Vec::new())
        .await
        .expect_err("stale host must be fenced");
    let mut stale_target = receipt.lease.clone();
    stale_target.target.run_id = RunId::from_string(format!("replay-batch-foreign-{suffix}"));
    store
        .append_replay_events_fenced(&stale_target, Vec::new())
        .await
        .expect_err("stale target must be fenced");
    let mut stale_generation = receipt.lease.clone();
    stale_generation.fencing_generation = stale_generation.fencing_generation.saturating_add(1);
    store
        .append_replay_events_fenced(&stale_generation, Vec::new())
        .await
        .expect_err("stale generation must be fenced");

    let expired_session_id =
        SessionId::from_string(format!("replay-batch-expired-session-{suffix}"));
    let expired_run_id = RunId::from_string(format!("replay-batch-expired-run-{suffix}"));
    store
        .save_session(SessionRecord::new(expired_session_id.clone()))
        .await
        .expect("save expired replay batch session");
    let expired = store
        .acquire_run_admission(AcquireRunAdmission {
            run: RunRecord::new(expired_session_id, expired_run_id, ConversationId::new()),
            namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
            host_instance_id: format!("replay-batch-expired-host-{suffix}"),
            admission_id: format!("replay-batch-expired-admission-{suffix}"),
            lease_expires_at: Utc::now() - chrono::Duration::seconds(1),
            idempotency_key: format!("replay-batch-expired-key-{suffix}"),
            command_fingerprint: format!("replay-batch-expired-fingerprint-{suffix}"),
            replaces_waiting_run_id: None,
            hitl_resume_claim_id: None,
        })
        .await
        .expect("acquire already-expired replay batch admission");
    store
        .append_replay_events_fenced(&expired.lease, Vec::new())
        .await
        .expect_err("expired admission must be fenced");
}

#[allow(dead_code)]
pub async fn assert_background_subagent_contract(store: Arc<dyn SessionStore>, suffix: &str) {
    store
        .drain_background_subagent_operations()
        .await
        .expect("background operation drain capability");
    let session_id = SessionId::from_string(format!("background-session-{suffix}"));
    let parent_run_id = RunId::from_string(format!("background-parent-{suffix}"));
    let conversation_id = ConversationId::from_string(format!("background-conversation-{suffix}"));
    store
        .save_session(SessionRecord::new(session_id.clone()))
        .await
        .expect("save background parent session");
    let mut parent = RunRecord::new(
        session_id.clone(),
        parent_run_id.clone(),
        conversation_id.clone(),
    );
    parent.status = RunStatus::Completed;
    parent.profile = Some("default".to_string());
    store
        .append_run(parent)
        .await
        .expect("save background parent run");

    let now = Utc::now();
    let attempt_id = SubagentAttemptId::from_string(format!("background-attempt-{suffix}"));
    let mut record = BackgroundSubagentRecord {
        schema_version: BACKGROUND_SUBAGENT_RECORD_VERSION,
        attempt_id: attempt_id.clone(),
        agent_id: format!("background-agent-{suffix}"),
        linked_task_id: None,
        subagent_name: "researcher".to_string(),
        namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
        parent_session_id: session_id.clone(),
        parent_run_id: parent_run_id.clone(),
        child_run_id: None,
        continuation_run_id: None,
        profile: "default".to_string(),
        owner_lease: DurableBackgroundSubagentOwnerLease {
            host_instance_id: format!("background-owner-{suffix}"),
            fencing_generation: 1,
            heartbeat_at: now,
            lease_expires_at: now + chrono::Duration::minutes(1),
        },
        execution_status: DurableBackgroundSubagentExecutionStatus::Accepted,
        result_ref: None,
        failure_category: None,
        cancellation_reason: None,
        delivery_status: DurableBackgroundSubagentDeliveryStatus::Undelivered,
        delivery_claim: None,
        delivered_claim_id: None,
        automatic_continuation_suppressed_by_run_id: None,
        retention_status: DurableBackgroundSubagentRetentionStatus::Inline,
        retention_expires_at: None,
        trace_context: None,
        accepted_at: now,
        updated_at: now,
        terminal_at: None,
    };
    store
        .record_background_subagent_acceptance(record.clone())
        .await
        .expect("record acceptance");
    let parent_before_delete = store
        .load_session(&session_id)
        .await
        .expect("load parent before deletion fence");
    store
        .acquire_session_deletion_fence(
            &session_id,
            parent_before_delete.revision,
            &format!("background-active-fence-{suffix}"),
            "contract",
            &format!("background-active-delete-{suffix}"),
            &format!("background-active-delete-v1-{suffix}"),
        )
        .await
        .expect_err("active background ownership must atomically reject deletion fencing");
    assert_eq!(
        store
            .load_session(&session_id)
            .await
            .expect("load unfenced parent")
            .deletion_fence,
        SessionDeletionFence::Stable,
        "rejected deletion must not leave a partial fence"
    );
    store
        .record_background_subagent_acceptance(record.clone())
        .await
        .expect("acceptance retry is idempotent");
    let mut competing = record.clone();
    competing.attempt_id =
        SubagentAttemptId::from_string(format!("background-competing-attempt-{suffix}"));
    competing.owner_lease.host_instance_id = format!("background-competing-owner-{suffix}");
    assert!(matches!(
        store.record_background_subagent_acceptance(competing).await,
        Err(SessionStoreError::Conflict(_))
    ));
    record.execution_status = DurableBackgroundSubagentExecutionStatus::Starting;
    record.updated_at = now + chrono::Duration::milliseconds(1);
    store
        .update_background_subagent_execution(record.clone())
        .await
        .expect("record starting");
    record.execution_status = DurableBackgroundSubagentExecutionStatus::Running;
    record.child_run_id = Some(RunId::from_string(format!("background-child-{suffix}")));
    record.updated_at = now + chrono::Duration::milliseconds(2);
    store
        .update_background_subagent_execution(record.clone())
        .await
        .expect("record running");
    record.execution_status = DurableBackgroundSubagentExecutionStatus::Completed;
    record.result_ref = Some(DurableBackgroundSubagentResultRef {
        content: Some("durable result".to_string()),
        size_bytes: 14,
        ..DurableBackgroundSubagentResultRef::default()
    });
    record.terminal_at = Some(now + chrono::Duration::milliseconds(3));
    record.updated_at = record.terminal_at.expect("terminal timestamp");
    record.retention_expires_at = Some(record.updated_at + chrono::Duration::hours(1));
    store
        .record_background_subagent_terminal(record.clone())
        .await
        .expect("record terminal outcome");
    store
        .record_background_subagent_terminal(record.clone())
        .await
        .expect("exact inline terminal retry is idempotent");
    let mut forged_inline = record.clone();
    forged_inline
        .result_ref
        .as_mut()
        .expect("inline terminal evidence")
        .content = Some("forged-result!".to_string());
    assert!(matches!(
        store
            .record_background_subagent_terminal(forged_inline)
            .await,
        Err(SessionStoreError::Conflict(_))
    ));
    let mut forged_owner_generation = record.clone();
    forged_owner_generation.owner_lease.fencing_generation = forged_owner_generation
        .owner_lease
        .fencing_generation
        .saturating_add(1);
    assert!(matches!(
        store
            .record_background_subagent_terminal(forged_owner_generation)
            .await,
        Err(SessionStoreError::Conflict(_))
    ));

    let claim = DurableBackgroundSubagentDeliveryClaim {
        claim_id: format!("background-claim-{suffix}"),
        continuation_run_id: None,
        deadline: Utc::now() + chrono::Duration::minutes(1),
    };
    store
        .claim_background_subagent_delivery(&attempt_id, claim.clone())
        .await
        .expect("claim terminal delivery");
    store
        .claim_background_subagent_delivery(
            &attempt_id,
            DurableBackgroundSubagentDeliveryClaim {
                claim_id: format!("background-conflict-{suffix}"),
                ..claim.clone()
            },
        )
        .await
        .expect_err("another unexpired claim must conflict");
    store
        .release_background_subagent_delivery(
            &attempt_id,
            &claim.claim_id,
            DurableBackgroundSubagentDeliveryRelease::Retryable,
        )
        .await
        .expect("release failed pre-admission claim");
    store
        .release_background_subagent_delivery(
            &attempt_id,
            &claim.claim_id,
            DurableBackgroundSubagentDeliveryRelease::ConsumerTerminated {
                run_id: parent_run_id.clone(),
            },
        )
        .await
        .expect_err("an undelivered result cannot fake a terminated-consumer release");

    let continuation_run_id = RunId::from_string(format!("background-continuation-{suffix}"));
    let mut continuation = RunRecord::new(
        session_id.clone(),
        continuation_run_id.clone(),
        conversation_id,
    );
    continuation.input = record.continuation_input(None);
    continuation.profile = Some("default".to_string());
    continuation.restore_from_run_id = Some(parent_run_id.clone());
    continuation.parent_run_id = Some(parent_run_id.clone());
    continuation.trigger_type = Some("async_subagent_result".to_string());
    continuation.metadata.insert(
        "starweaver.async_subagent.attempt_id".to_string(),
        serde_json::json!(attempt_id.as_str()),
    );
    continuation.metadata.insert(
        "starweaver.async_subagent.agent_id".to_string(),
        serde_json::json!(record.agent_id.as_str()),
    );
    continuation.metadata.insert(
        "starweaver.async_subagent.parent_run_id".to_string(),
        serde_json::json!(parent_run_id.as_str()),
    );
    continuation.metadata.insert(
        "starweaver.async_subagent.child_run_id".to_string(),
        serde_json::json!(record.child_run_id.as_ref().expect("child run").as_str()),
    );
    let continuation_claim_id = format!("background-continuation-claim-{suffix}");
    let cause = BackgroundSubagentContinuationCause::new(&record, &continuation.input)
        .expect("canonical continuation cause");
    let request = AcquireBackgroundSubagentContinuation {
        attempt_id: attempt_id.clone(),
        claim_id: continuation_claim_id.clone(),
        claim_deadline: Utc::now() + chrono::Duration::minutes(1),
        cause,
        admission: AcquireRunAdmission {
            run: continuation,
            namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
            host_instance_id: format!("background-host-{suffix}"),
            admission_id: format!("background-admission-{suffix}"),
            lease_expires_at: Utc::now() + chrono::Duration::minutes(1),
            idempotency_key: format!("background-idempotency-{suffix}"),
            command_fingerprint: format!("background-fingerprint-{suffix}"),
            replaces_waiting_run_id: None,
            hitl_resume_claim_id: None,
        },
    };
    let mut stale_source = request.clone();
    stale_source.admission.run.restore_from_run_id = Some(RunId::from_string(format!(
        "stale-background-head-{suffix}"
    )));
    assert!(matches!(
        store
            .acquire_background_subagent_continuation(stale_source)
            .await,
        Err(SessionStoreError::Conflict(_))
    ));
    let mut forged_cause = request.clone();
    forged_cause.cause.agent_id = format!("forged-agent-{suffix}");
    assert!(matches!(
        store
            .acquire_background_subagent_continuation(forged_cause)
            .await,
        Err(SessionStoreError::Conflict(_))
    ));
    let mut forged_input = request.clone();
    forged_input.admission.run.input = vec![InputPart::text("unrelated input")];
    forged_input.cause =
        BackgroundSubagentContinuationCause::new(&record, &forged_input.admission.run.input)
            .expect("forged input digest");
    assert!(matches!(
        store
            .acquire_background_subagent_continuation(forged_input)
            .await,
        Err(SessionStoreError::Conflict(_))
    ));
    let mut missing_input = request.clone();
    missing_input.admission.run.input.clear();
    missing_input.cause =
        BackgroundSubagentContinuationCause::new(&record, &missing_input.admission.run.input)
            .expect("missing input digest");
    assert!(matches!(
        store
            .acquire_background_subagent_continuation(missing_input)
            .await,
        Err(SessionStoreError::Conflict(_))
    ));
    let admitted = store
        .acquire_background_subagent_continuation(request.clone())
        .await
        .expect("atomically admit background continuation");
    assert_eq!(admitted.admission.run.run_id, continuation_run_id);
    assert_eq!(admitted.cause, request.cause);
    assert_eq!(
        admitted.background.delivery_status,
        DurableBackgroundSubagentDeliveryStatus::Delivered
    );
    let replay = store
        .acquire_background_subagent_continuation(request.clone())
        .await
        .expect("exact continuation replay");
    assert!(replay.admission.idempotent_replay);
    assert_eq!(replay.cause, request.cause);
    let mut forged_replay = request;
    forged_replay.admission.run.input = vec![InputPart::text("forged replay input")];
    forged_replay.cause =
        BackgroundSubagentContinuationCause::new(&record, &forged_replay.admission.run.input)
            .expect("forged replay digest");
    assert!(matches!(
        store
            .acquire_background_subagent_continuation(forged_replay)
            .await,
        Err(SessionStoreError::Conflict(_))
    ));
    let delivered = store
        .acknowledge_background_subagent_delivery(&attempt_id, &continuation_claim_id)
        .await
        .expect("acknowledge started continuation");
    assert_eq!(
        delivered.delivery_status,
        DurableBackgroundSubagentDeliveryStatus::Delivered
    );
    assert_eq!(
        delivered.continuation_run_id.as_ref(),
        Some(&continuation_run_id)
    );
    let replay_after_delivery = store
        .record_background_subagent_terminal(record.clone())
        .await
        .expect("exact terminal retry tolerates later delivery projection");
    assert_eq!(
        replay_after_delivery.delivery_status,
        DurableBackgroundSubagentDeliveryStatus::Delivered
    );

    let claim_now = Utc::now();
    let run_owned_attempt =
        SubagentAttemptId::from_string(format!("background-run-owned-claim-{suffix}"));
    let mut run_owned = record.clone();
    run_owned.attempt_id = run_owned_attempt.clone();
    run_owned.agent_id = format!("background-run-owned-agent-{suffix}");
    run_owned.child_run_id = None;
    run_owned.continuation_run_id = None;
    run_owned.execution_status = DurableBackgroundSubagentExecutionStatus::Accepted;
    run_owned.result_ref = None;
    run_owned.failure_category = None;
    run_owned.cancellation_reason = None;
    run_owned.delivery_status = DurableBackgroundSubagentDeliveryStatus::Undelivered;
    run_owned.delivery_claim = None;
    run_owned.delivered_claim_id = None;
    run_owned.automatic_continuation_suppressed_by_run_id = None;
    run_owned.retention_status = DurableBackgroundSubagentRetentionStatus::Inline;
    run_owned.retention_expires_at = None;
    run_owned.owner_lease.host_instance_id = format!("run-owned-owner-{suffix}");
    run_owned.owner_lease.heartbeat_at = claim_now;
    run_owned.owner_lease.lease_expires_at = claim_now + chrono::Duration::minutes(5);
    run_owned.accepted_at = claim_now;
    run_owned.updated_at = claim_now;
    run_owned.terminal_at = None;
    store
        .record_background_subagent_acceptance(run_owned.clone())
        .await
        .expect("accept run-owned claim attempt");
    run_owned.execution_status = DurableBackgroundSubagentExecutionStatus::Starting;
    run_owned.updated_at = claim_now + chrono::Duration::milliseconds(1);
    store
        .update_background_subagent_execution(run_owned.clone())
        .await
        .expect("start run-owned claim attempt");
    run_owned.execution_status = DurableBackgroundSubagentExecutionStatus::Running;
    run_owned.updated_at = claim_now + chrono::Duration::milliseconds(2);
    store
        .update_background_subagent_execution(run_owned.clone())
        .await
        .expect("run run-owned claim attempt");
    run_owned.execution_status = DurableBackgroundSubagentExecutionStatus::Completed;
    run_owned.result_ref = Some(DurableBackgroundSubagentResultRef {
        content: Some("run-owned result".to_string()),
        size_bytes: 16,
        ..DurableBackgroundSubagentResultRef::default()
    });
    run_owned.updated_at = claim_now + chrono::Duration::milliseconds(3);
    run_owned.terminal_at = Some(run_owned.updated_at);
    run_owned.retention_expires_at = Some(claim_now + chrono::Duration::hours(1));
    store
        .record_background_subagent_terminal(run_owned)
        .await
        .expect("commit run-owned result");
    let run_owned_claim_id = format!("run-owned-claim-{suffix}");
    store
        .claim_background_subagent_delivery(
            &run_owned_attempt,
            DurableBackgroundSubagentDeliveryClaim {
                claim_id: run_owned_claim_id.clone(),
                continuation_run_id: Some(continuation_run_id.clone()),
                deadline: claim_now - chrono::Duration::milliseconds(1),
            },
        )
        .await
        .expect("claim result for active continuation run");
    store
        .claim_background_subagent_delivery(
            &run_owned_attempt,
            DurableBackgroundSubagentDeliveryClaim {
                claim_id: format!("run-owned-steal-{suffix}"),
                continuation_run_id: None,
                deadline: claim_now + chrono::Duration::minutes(1),
            },
        )
        .await
        .expect_err("an expired deadline cannot steal from a live admitted consumer");
    store
        .reconcile_background_subagents(LOCAL_SESSION_NAMESPACE, claim_now)
        .await
        .expect("reconcile expired run-owned claim");
    assert_eq!(
        store
            .load_background_subagent(&run_owned_attempt)
            .await
            .expect("load retained run-owned claim")
            .delivery_status,
        DurableBackgroundSubagentDeliveryStatus::Claimed,
        "a live admitted consumer retains ownership past the delivery deadline"
    );
    store
        .update_run_status(
            &session_id,
            &continuation_run_id,
            RunStatus::Cancelled,
            Some("cancelled consumer".to_string()),
        )
        .await
        .expect("terminalize run-owned consumer");
    store
        .claim_background_subagent_delivery(
            &run_owned_attempt,
            DurableBackgroundSubagentDeliveryClaim {
                claim_id: format!("run-owned-after-cancel-{suffix}"),
                continuation_run_id: None,
                deadline: claim_now + chrono::Duration::minutes(1),
            },
        )
        .await
        .expect_err("claim CAS atomically releases and suppresses a cancelled consumer");
    let suppressed = store
        .load_background_subagent(&run_owned_attempt)
        .await
        .expect("load suppressed pending result");
    assert_eq!(
        suppressed.delivery_status,
        DurableBackgroundSubagentDeliveryStatus::Undelivered
    );
    assert_eq!(
        suppressed
            .automatic_continuation_suppressed_by_run_id
            .as_ref(),
        Some(&continuation_run_id)
    );
    let explicit_claim_id = format!("explicit-after-cancel-{suffix}");
    store
        .claim_background_subagent_delivery(
            &run_owned_attempt,
            DurableBackgroundSubagentDeliveryClaim {
                claim_id: explicit_claim_id.clone(),
                continuation_run_id: Some(RunId::from_string(format!(
                    "explicit-consumer-{suffix}"
                ))),
                deadline: Utc::now() + chrono::Duration::minutes(1),
            },
        )
        .await
        .expect("explicit later run may claim a suppressed result");
    let explicitly_delivered = store
        .acknowledge_background_subagent_delivery(&run_owned_attempt, &explicit_claim_id)
        .await
        .expect("explicit later run consumes suppressed result");
    assert_eq!(
        explicitly_delivered.delivery_status,
        DurableBackgroundSubagentDeliveryStatus::Delivered
    );
    assert!(
        explicitly_delivered
            .automatic_continuation_suppressed_by_run_id
            .is_none()
    );

    let artifact_now = Utc::now();
    let artifact_attempt = SubagentAttemptId::from_string(format!("background-artifact-{suffix}"));
    let mut artifact_record = record.clone();
    artifact_record.attempt_id = artifact_attempt.clone();
    artifact_record.agent_id = format!("background-artifact-agent-{suffix}");
    artifact_record.child_run_id = None;
    artifact_record.execution_status = DurableBackgroundSubagentExecutionStatus::Accepted;
    artifact_record.result_ref = None;
    artifact_record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Undelivered;
    artifact_record.delivery_claim = None;
    artifact_record.delivered_claim_id = None;
    artifact_record.continuation_run_id = None;
    artifact_record.retention_status = DurableBackgroundSubagentRetentionStatus::Inline;
    artifact_record.retention_expires_at = None;
    artifact_record.owner_lease.host_instance_id = format!("artifact-owner-{suffix}");
    artifact_record.owner_lease.heartbeat_at = artifact_now;
    artifact_record.owner_lease.lease_expires_at = artifact_now + chrono::Duration::minutes(5);
    artifact_record.accepted_at = artifact_now;
    artifact_record.updated_at = artifact_now;
    artifact_record.terminal_at = None;
    store
        .record_background_subagent_acceptance(artifact_record.clone())
        .await
        .expect("accept artifact-backed attempt");
    artifact_record.execution_status = DurableBackgroundSubagentExecutionStatus::Starting;
    artifact_record.updated_at = artifact_now + chrono::Duration::milliseconds(1);
    store
        .update_background_subagent_execution(artifact_record.clone())
        .await
        .expect("start artifact-backed attempt");
    artifact_record.execution_status = DurableBackgroundSubagentExecutionStatus::Running;
    artifact_record.child_run_id = Some(RunId::from_string(format!("artifact-child-{suffix}")));
    artifact_record.updated_at = artifact_now + chrono::Duration::milliseconds(2);
    store
        .update_background_subagent_execution(artifact_record.clone())
        .await
        .expect("run artifact-backed attempt");
    let full_content = "oversized-result-".repeat(64);
    let artifact_ref = format!("starweaver:background-subagent-result:{suffix}");
    let digest = BackgroundSubagentArtifact::content_digest(&full_content);
    let expires_at = artifact_now + chrono::Duration::minutes(1);
    artifact_record.execution_status = DurableBackgroundSubagentExecutionStatus::Completed;
    artifact_record.result_ref = Some(DurableBackgroundSubagentResultRef {
        content: Some("oversized-result-preview".to_string()),
        artifact_ref: Some(artifact_ref.clone()),
        digest: Some(digest.clone()),
        size_bytes: u64::try_from(full_content.len()).expect("artifact size"),
        ..DurableBackgroundSubagentResultRef::default()
    });
    artifact_record.retention_status = DurableBackgroundSubagentRetentionStatus::Artifact;
    artifact_record.retention_expires_at = Some(expires_at);
    artifact_record.updated_at = artifact_now + chrono::Duration::milliseconds(3);
    artifact_record.terminal_at = Some(artifact_record.updated_at);
    let artifact = BackgroundSubagentArtifact {
        artifact_ref: artifact_ref.clone(),
        namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
        attempt_id: artifact_attempt,
        content: full_content.clone(),
        digest,
        size_bytes: u64::try_from(full_content.len()).expect("artifact size"),
        created_at: artifact_record.updated_at,
        expires_at,
    };
    let artifact_commit = BackgroundSubagentTerminalCommit {
        record: artifact_record.clone(),
        artifact: Some(artifact.clone()),
        artifact_limits: Some(BackgroundSubagentArtifactLimits {
            max_single_bytes: 1_000_000,
            max_retained_bytes: 10_000_000,
        }),
    };
    let mut missing_artifact_limits = artifact_commit.clone();
    missing_artifact_limits.artifact_limits = None;
    assert!(matches!(
        store
            .commit_background_subagent_terminal(missing_artifact_limits)
            .await,
        Err(SessionStoreError::Conflict(_))
    ));
    let mut insufficient_artifact_limits = artifact_commit.clone();
    insufficient_artifact_limits.artifact_limits = Some(BackgroundSubagentArtifactLimits {
        max_single_bytes: artifact.size_bytes.saturating_sub(1),
        max_retained_bytes: artifact.size_bytes,
    });
    assert!(matches!(
        store
            .commit_background_subagent_terminal(insufficient_artifact_limits)
            .await,
        Err(SessionStoreError::QuotaExceeded(_))
    ));
    store
        .commit_background_subagent_terminal(artifact_commit.clone())
        .await
        .expect("atomically commit artifact-backed terminal result");
    assert_eq!(
        store
            .load_background_subagent_artifact(&artifact_ref)
            .await
            .expect("load complete artifact"),
        artifact
    );
    store
        .commit_background_subagent_terminal(artifact_commit.clone())
        .await
        .expect("exact artifact commit retry is idempotent");
    let completed_consumer_claim_id = format!("artifact-completed-consumer-{suffix}");
    store
        .claim_background_subagent_delivery(
            &artifact_record.attempt_id,
            DurableBackgroundSubagentDeliveryClaim {
                claim_id: completed_consumer_claim_id.clone(),
                continuation_run_id: Some(parent_run_id.clone()),
                deadline: Utc::now() - chrono::Duration::milliseconds(1),
            },
        )
        .await
        .expect("claim artifact result for completed consumer reconciliation");
    store
        .claim_background_subagent_delivery(
            &artifact_record.attempt_id,
            DurableBackgroundSubagentDeliveryClaim {
                claim_id: format!("artifact-completed-steal-{suffix}"),
                continuation_run_id: None,
                deadline: Utc::now() + chrono::Duration::minutes(1),
            },
        )
        .await
        .expect_err("claim CAS atomically acknowledges an already-completed consumer");
    let completed_consumer_delivery = store
        .load_background_subagent(&artifact_record.attempt_id)
        .await
        .expect("load completed consumer delivery");
    assert_eq!(
        completed_consumer_delivery.delivery_status,
        DurableBackgroundSubagentDeliveryStatus::Delivered
    );
    assert_eq!(
        completed_consumer_delivery.continuation_run_id.as_ref(),
        Some(&parent_run_id)
    );
    let aggregate_now = Utc::now();
    let aggregate_attempt =
        SubagentAttemptId::from_string(format!("background-artifact-aggregate-{suffix}"));
    let aggregate_ref = format!("{artifact_ref}:aggregate-overflow");
    let aggregate_expires_at = aggregate_now + chrono::Duration::minutes(1);
    let mut aggregate_record = artifact_record.clone();
    aggregate_record.attempt_id = aggregate_attempt.clone();
    aggregate_record.agent_id = format!("background-artifact-aggregate-agent-{suffix}");
    aggregate_record.child_run_id = None;
    aggregate_record.execution_status = DurableBackgroundSubagentExecutionStatus::Accepted;
    aggregate_record.result_ref = None;
    aggregate_record.retention_status = DurableBackgroundSubagentRetentionStatus::Inline;
    aggregate_record.retention_expires_at = None;
    aggregate_record.owner_lease.host_instance_id = format!("aggregate-owner-{suffix}");
    aggregate_record.owner_lease.heartbeat_at = aggregate_now;
    aggregate_record.owner_lease.lease_expires_at = aggregate_now + chrono::Duration::minutes(5);
    aggregate_record.accepted_at = aggregate_now;
    aggregate_record.updated_at = aggregate_now;
    aggregate_record.terminal_at = None;
    store
        .record_background_subagent_acceptance(aggregate_record.clone())
        .await
        .expect("accept aggregate quota attempt");
    aggregate_record.execution_status = DurableBackgroundSubagentExecutionStatus::Starting;
    aggregate_record.updated_at = aggregate_now + chrono::Duration::milliseconds(1);
    store
        .update_background_subagent_execution(aggregate_record.clone())
        .await
        .expect("start aggregate quota attempt");
    aggregate_record.execution_status = DurableBackgroundSubagentExecutionStatus::Running;
    aggregate_record.updated_at = aggregate_now + chrono::Duration::milliseconds(2);
    store
        .update_background_subagent_execution(aggregate_record.clone())
        .await
        .expect("run aggregate quota attempt");
    aggregate_record.execution_status = DurableBackgroundSubagentExecutionStatus::Completed;
    aggregate_record.result_ref = Some(DurableBackgroundSubagentResultRef {
        content: Some("oversized-result-preview".to_string()),
        artifact_ref: Some(aggregate_ref.clone()),
        digest: Some(artifact.digest.clone()),
        size_bytes: artifact.size_bytes,
        ..DurableBackgroundSubagentResultRef::default()
    });
    aggregate_record.retention_status = DurableBackgroundSubagentRetentionStatus::Artifact;
    aggregate_record.retention_expires_at = Some(aggregate_expires_at);
    aggregate_record.updated_at = aggregate_now + chrono::Duration::milliseconds(3);
    aggregate_record.terminal_at = Some(aggregate_record.updated_at);
    let aggregate_overflow = BackgroundSubagentTerminalCommit {
        record: aggregate_record,
        artifact: Some(BackgroundSubagentArtifact {
            artifact_ref: aggregate_ref,
            namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
            attempt_id: aggregate_attempt,
            content: full_content.clone(),
            digest: artifact.digest.clone(),
            size_bytes: artifact.size_bytes,
            created_at: aggregate_now + chrono::Duration::milliseconds(3),
            expires_at: aggregate_expires_at,
        }),
        artifact_limits: Some(BackgroundSubagentArtifactLimits {
            max_single_bytes: artifact.size_bytes,
            max_retained_bytes: artifact.size_bytes.saturating_mul(2).saturating_sub(1),
        }),
    };
    assert!(matches!(
        store
            .commit_background_subagent_terminal(aggregate_overflow)
            .await,
        Err(SessionStoreError::QuotaExceeded(_))
    ));
    let mut conflicting = artifact_commit.clone();
    conflicting.artifact.as_mut().expect("artifact").content = "x".repeat(full_content.len());
    assert!(matches!(
        store.commit_background_subagent_terminal(conflicting).await,
        Err(SessionStoreError::Conflict(_))
    ));
    assert!(
        store
            .expire_background_subagent_retention(
                LOCAL_SESSION_NAMESPACE,
                expires_at - chrono::Duration::milliseconds(1),
                32,
            )
            .await
            .expect("retention before deadline")
            .is_empty()
    );
    let expired = store
        .expire_background_subagent_retention(
            LOCAL_SESSION_NAMESPACE,
            expires_at + chrono::Duration::milliseconds(1),
            32,
        )
        .await
        .expect("expire artifact");
    assert_eq!(expired.len(), 1);
    assert_eq!(
        expired[0].retention_status,
        DurableBackgroundSubagentRetentionStatus::Expired
    );
    assert_eq!(
        expired[0].execution_status,
        DurableBackgroundSubagentExecutionStatus::Completed
    );
    assert_eq!(
        expired[0].delivery_status,
        DurableBackgroundSubagentDeliveryStatus::Delivered
    );
    assert!(matches!(
        store.load_background_subagent_artifact(&artifact_ref).await,
        Err(SessionStoreError::NotFound(_))
    ));
    let replay = store
        .commit_background_subagent_terminal(artifact_commit.clone())
        .await
        .expect("terminal retry after expiry");
    assert_eq!(
        replay.retention_status,
        DurableBackgroundSubagentRetentionStatus::Expired
    );
    let unrelated_content = "unrelated-artifact-content".repeat(32);
    let unrelated_digest = BackgroundSubagentArtifact::content_digest(&unrelated_content);
    let unrelated_ref = format!("{artifact_ref}:unrelated-after-expiry");
    let mut unrelated_after_expiry = artifact_commit;
    let unrelated_artifact = unrelated_after_expiry
        .artifact
        .as_mut()
        .expect("unrelated replay artifact");
    unrelated_artifact.artifact_ref.clone_from(&unrelated_ref);
    unrelated_artifact.content.clone_from(&unrelated_content);
    unrelated_artifact.digest.clone_from(&unrelated_digest);
    unrelated_artifact.size_bytes =
        u64::try_from(unrelated_content.len()).expect("unrelated artifact size");
    let unrelated_result = unrelated_after_expiry
        .record
        .result_ref
        .as_mut()
        .expect("unrelated replay terminal evidence");
    unrelated_result.artifact_ref = Some(unrelated_ref);
    unrelated_result.digest = Some(unrelated_digest);
    unrelated_result.size_bytes = unrelated_artifact.size_bytes;
    assert!(matches!(
        store
            .commit_background_subagent_terminal(unrelated_after_expiry)
            .await,
        Err(SessionStoreError::Conflict(_))
    ));
    assert!(
        store
            .expire_background_subagent_retention(
                LOCAL_SESSION_NAMESPACE,
                expires_at + chrono::Duration::seconds(1),
                32,
            )
            .await
            .expect("repeat expiry")
            .is_empty()
    );

    let lost_attempt = SubagentAttemptId::from_string(format!("background-lost-{suffix}"));
    record.attempt_id = lost_attempt.clone();
    record.agent_id = format!("background-lost-agent-{suffix}");
    record.execution_status = DurableBackgroundSubagentExecutionStatus::Accepted;
    record.child_run_id = None;
    record.continuation_run_id = None;
    record.result_ref = None;
    record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Undelivered;
    record.delivery_claim = None;
    record.delivered_claim_id = None;
    record.retention_status = DurableBackgroundSubagentRetentionStatus::Inline;
    record.retention_expires_at = None;
    record.accepted_at = Utc::now();
    record.updated_at = record.accepted_at;
    record.terminal_at = None;
    record.owner_lease.host_instance_id = format!("background-foreign-owner-{suffix}");
    record.owner_lease.fencing_generation = 7;
    record.owner_lease.heartbeat_at = record.accepted_at;
    record.owner_lease.lease_expires_at = record.accepted_at + chrono::Duration::minutes(1);
    store
        .record_background_subagent_acceptance(record)
        .await
        .expect("record process-local attempt");
    store
        .reconcile_background_subagents(LOCAL_SESSION_NAMESPACE, Utc::now())
        .await
        .expect("preserve unexpired foreign owner");
    assert_eq!(
        store
            .load_background_subagent(&lost_attempt)
            .await
            .expect("load live foreign attempt")
            .execution_status,
        DurableBackgroundSubagentExecutionStatus::Accepted
    );
    store
        .heartbeat_background_subagent(
            &lost_attempt,
            "stale-owner",
            7,
            Utc::now() + chrono::Duration::minutes(2),
        )
        .await
        .expect_err("foreign owner heartbeat must be fenced");
    let heartbeat = store
        .heartbeat_background_subagent(
            &lost_attempt,
            &format!("background-foreign-owner-{suffix}"),
            7,
            Utc::now() + chrono::Duration::minutes(2),
        )
        .await
        .expect("current owner heartbeat");
    store
        .reconcile_background_subagents(
            LOCAL_SESSION_NAMESPACE,
            heartbeat.owner_lease.lease_expires_at + chrono::Duration::milliseconds(1),
        )
        .await
        .expect("reconcile expired process owner");
    let lost = store
        .load_background_subagent(&lost_attempt)
        .await
        .expect("load interrupted attempt");
    assert_eq!(
        lost.execution_status,
        DurableBackgroundSubagentExecutionStatus::Failed
    );
    assert_eq!(lost.failure_category.as_deref(), Some("host_process_lost"));
    let lost_retention_deadline = lost
        .retention_expires_at
        .expect("reconciliation retention deadline");
    let reconciled_expiry = store
        .expire_background_subagent_retention(
            LOCAL_SESSION_NAMESPACE,
            lost_retention_deadline + chrono::Duration::milliseconds(1),
            32,
        )
        .await
        .expect("expire reconciled terminal retention");
    let expired_lost = reconciled_expiry
        .iter()
        .find(|expired| expired.attempt_id == lost_attempt)
        .expect("reconciled terminal was included in retention expiry");
    assert_eq!(
        expired_lost.retention_status,
        DurableBackgroundSubagentRetentionStatus::Expired
    );
    assert!(
        expired_lost
            .result_ref
            .as_ref()
            .expect("expired reconciled result reference")
            .error
            .is_none()
    );
    let reconciled_replay = store
        .record_background_subagent_terminal(lost.clone())
        .await
        .expect("exact reconciled terminal replay after retention expiry");
    assert_eq!(
        reconciled_replay.retention_status,
        DurableBackgroundSubagentRetentionStatus::Expired
    );
    let mut forged_reconciled_terminal = lost.clone();
    forged_reconciled_terminal
        .result_ref
        .as_mut()
        .expect("reconciled terminal result")
        .error = Some("forged host restart failure".to_string());
    assert!(matches!(
        store
            .record_background_subagent_terminal(forged_reconciled_terminal)
            .await,
        Err(SessionStoreError::Conflict(_))
    ));

    let expired_now = Utc::now();
    let expired_attempt =
        SubagentAttemptId::from_string(format!("background-expired-owner-{suffix}"));
    let mut expired_owner = lost.clone();
    expired_owner.attempt_id = expired_attempt.clone();
    expired_owner.agent_id = format!("background-expired-owner-agent-{suffix}");
    expired_owner.execution_status = DurableBackgroundSubagentExecutionStatus::Accepted;
    expired_owner.child_run_id = None;
    expired_owner.continuation_run_id = None;
    expired_owner.result_ref = None;
    expired_owner.failure_category = None;
    expired_owner.delivery_status = DurableBackgroundSubagentDeliveryStatus::Undelivered;
    expired_owner.delivery_claim = None;
    expired_owner.delivered_claim_id = None;
    expired_owner.automatic_continuation_suppressed_by_run_id = None;
    expired_owner.retention_status = DurableBackgroundSubagentRetentionStatus::Inline;
    expired_owner.retention_expires_at = None;
    expired_owner.owner_lease.host_instance_id = format!("expired-owner-{suffix}");
    expired_owner.owner_lease.heartbeat_at = expired_now;
    expired_owner.owner_lease.lease_expires_at = expired_now + chrono::Duration::milliseconds(500);
    expired_owner.accepted_at = expired_owner.owner_lease.heartbeat_at;
    expired_owner.updated_at = expired_owner.accepted_at;
    expired_owner.terminal_at = None;
    let mut already_expired = expired_owner.clone();
    already_expired.attempt_id =
        SubagentAttemptId::from_string(format!("background-already-expired-{suffix}"));
    already_expired.agent_id = format!("background-already-expired-agent-{suffix}");
    already_expired.owner_lease.heartbeat_at = expired_now - chrono::Duration::minutes(2);
    already_expired.owner_lease.lease_expires_at = expired_now - chrono::Duration::minutes(1);
    already_expired.accepted_at = already_expired.owner_lease.heartbeat_at;
    already_expired.updated_at = already_expired.accepted_at;
    store
        .record_background_subagent_acceptance(already_expired)
        .await
        .expect_err("store-owned now rejects an already-expired acceptance lease");
    store
        .record_background_subagent_acceptance(expired_owner.clone())
        .await
        .expect("record short-lived owner for fencing contract");
    tokio::time::sleep(std::time::Duration::from_millis(550)).await;
    let mut expired_update = expired_owner.clone();
    expired_update.execution_status = DurableBackgroundSubagentExecutionStatus::Starting;
    expired_update.updated_at = expired_now;
    store
        .update_background_subagent_execution(expired_update)
        .await
        .expect_err("expired owner cannot advance execution");
    store
        .heartbeat_background_subagent(
            &expired_attempt,
            &expired_owner.owner_lease.host_instance_id,
            expired_owner.owner_lease.fencing_generation,
            expired_now + chrono::Duration::minutes(1),
        )
        .await
        .expect_err("expired owner cannot revive its lease");
    let mut expired_terminal = expired_owner;
    expired_terminal.execution_status = DurableBackgroundSubagentExecutionStatus::Failed;
    expired_terminal.failure_category = Some("execution_error".to_string());
    expired_terminal.result_ref = Some(DurableBackgroundSubagentResultRef {
        error: Some("stale terminal write".to_string()),
        size_bytes: 20,
        ..DurableBackgroundSubagentResultRef::default()
    });
    expired_terminal.updated_at = expired_now;
    expired_terminal.terminal_at = Some(expired_now);
    expired_terminal.retention_expires_at = Some(expired_now + chrono::Duration::hours(1));
    store
        .record_background_subagent_terminal(expired_terminal)
        .await
        .expect_err("expired owner cannot commit first terminal evidence");

    let replay_now = Utc::now();
    let replay_attempt =
        SubagentAttemptId::from_string(format!("background-terminal-replay-{suffix}"));
    let mut replay_record = lost;
    replay_record.attempt_id = replay_attempt;
    replay_record.agent_id = format!("background-terminal-replay-agent-{suffix}");
    replay_record.execution_status = DurableBackgroundSubagentExecutionStatus::Accepted;
    replay_record.child_run_id = None;
    replay_record.continuation_run_id = None;
    replay_record.result_ref = None;
    replay_record.failure_category = None;
    replay_record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Undelivered;
    replay_record.delivery_claim = None;
    replay_record.delivered_claim_id = None;
    replay_record.automatic_continuation_suppressed_by_run_id = None;
    replay_record.retention_status = DurableBackgroundSubagentRetentionStatus::Inline;
    replay_record.retention_expires_at = None;
    replay_record.owner_lease.host_instance_id = format!("terminal-replay-owner-{suffix}");
    replay_record.owner_lease.heartbeat_at = replay_now;
    replay_record.owner_lease.lease_expires_at = replay_now + chrono::Duration::milliseconds(100);
    replay_record.accepted_at = replay_now;
    replay_record.updated_at = replay_now;
    replay_record.terminal_at = None;
    store
        .record_background_subagent_acceptance(replay_record.clone())
        .await
        .expect("record short-lived terminal replay owner");
    replay_record.execution_status = DurableBackgroundSubagentExecutionStatus::Failed;
    replay_record.failure_category = Some("execution_error".to_string());
    replay_record.result_ref = Some(DurableBackgroundSubagentResultRef {
        error: Some("terminal before expiry".to_string()),
        size_bytes: 22,
        ..DurableBackgroundSubagentResultRef::default()
    });
    replay_record.updated_at = replay_now + chrono::Duration::milliseconds(1);
    replay_record.terminal_at = Some(replay_record.updated_at);
    replay_record.retention_expires_at = Some(replay_now + chrono::Duration::hours(1));
    store
        .record_background_subagent_terminal(replay_record.clone())
        .await
        .expect("commit terminal before owner expiry");
    tokio::time::sleep(std::time::Duration::from_millis(125)).await;
    store
        .record_background_subagent_terminal(replay_record)
        .await
        .expect("exact terminal replay remains valid after owner expiry");

    let fenced_session_id =
        SessionId::from_string(format!("background-fail-closed-session-{suffix}"));
    let fenced_run_id = RunId::from_string(format!("background-fail-closed-run-{suffix}"));
    store
        .save_session(SessionRecord::new(fenced_session_id.clone()))
        .await
        .expect("save fail-closed session");
    let mut fenced_parent = RunRecord::new(
        fenced_session_id.clone(),
        fenced_run_id.clone(),
        ConversationId::from_string(format!("background-fail-closed-conversation-{suffix}")),
    );
    fenced_parent.status = RunStatus::Completed;
    store
        .append_run(fenced_parent)
        .await
        .expect("save fail-closed parent run");
    let fenced_now = Utc::now();
    let fenced_attempt =
        SubagentAttemptId::from_string(format!("background-fail-closed-attempt-{suffix}"));
    let mut fenced_record = BackgroundSubagentRecord {
        schema_version: BACKGROUND_SUBAGENT_RECORD_VERSION,
        attempt_id: fenced_attempt.clone(),
        agent_id: format!("background-fail-closed-agent-{suffix}"),
        linked_task_id: None,
        subagent_name: "researcher".to_string(),
        namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
        parent_session_id: fenced_session_id.clone(),
        parent_run_id: fenced_run_id,
        child_run_id: None,
        continuation_run_id: None,
        profile: "default".to_string(),
        owner_lease: DurableBackgroundSubagentOwnerLease {
            host_instance_id: format!("background-fail-closed-owner-{suffix}"),
            fencing_generation: 1,
            heartbeat_at: fenced_now,
            lease_expires_at: fenced_now + chrono::Duration::minutes(5),
        },
        execution_status: DurableBackgroundSubagentExecutionStatus::Accepted,
        result_ref: None,
        failure_category: None,
        cancellation_reason: None,
        delivery_status: DurableBackgroundSubagentDeliveryStatus::Undelivered,
        delivery_claim: None,
        delivered_claim_id: None,
        automatic_continuation_suppressed_by_run_id: None,
        retention_status: DurableBackgroundSubagentRetentionStatus::Inline,
        retention_expires_at: None,
        trace_context: None,
        accepted_at: fenced_now,
        updated_at: fenced_now,
        terminal_at: None,
    };
    store
        .record_background_subagent_acceptance(fenced_record.clone())
        .await
        .expect("save fail-closed acceptance");
    let mut deleting = store
        .load_session(&fenced_session_id)
        .await
        .expect("load fail-closed session");
    deleting.deletion_fence = SessionDeletionFence::Deleting {
        fence_id: format!("forced-background-fence-{suffix}"),
        expected_revision: deleting.revision,
        requested_by: "contract-fixture".to_string(),
        started_at: Utc::now(),
    };
    store
        .save_session(deleting)
        .await
        .expect("force deletion fixture through low-level save");
    store
        .heartbeat_background_subagent(
            &fenced_attempt,
            &fenced_record.owner_lease.host_instance_id,
            fenced_record.owner_lease.fencing_generation,
            Utc::now() + chrono::Duration::minutes(5),
        )
        .await
        .expect_err("background heartbeat must fail closed once deletion starts");
    fenced_record.execution_status = DurableBackgroundSubagentExecutionStatus::Starting;
    fenced_record.updated_at = Utc::now();
    store
        .update_background_subagent_execution(fenced_record.clone())
        .await
        .expect_err("background execution update must fail closed once deletion starts");
    fenced_record.execution_status = DurableBackgroundSubagentExecutionStatus::Failed;
    fenced_record.failure_category = Some("execution_error".to_string());
    fenced_record.result_ref = Some(DurableBackgroundSubagentResultRef {
        error: Some("must not be committed after deletion starts".to_string()),
        size_bytes: 42,
        ..DurableBackgroundSubagentResultRef::default()
    });
    fenced_record.terminal_at = Some(Utc::now());
    fenced_record.updated_at = fenced_record.terminal_at.expect("terminal timestamp");
    fenced_record.retention_expires_at = Some(Utc::now() + chrono::Duration::hours(1));
    store
        .record_background_subagent_terminal(fenced_record)
        .await
        .expect_err("background terminal update must fail closed once deletion starts");
    let mut deleted = store
        .load_session(&fenced_session_id)
        .await
        .expect("load deleting fixture");
    deleted.status = SessionStatus::Deleted;
    store
        .save_session(deleted)
        .await
        .expect("save deleted fixture");
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
