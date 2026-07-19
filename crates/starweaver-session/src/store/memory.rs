//! In-memory session store implementation.

mod approvals;
mod checkpoints;
mod runs;
mod sessions;
mod streams;
mod traces;

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use starweaver_context::{AgentCheckpoint, ResumableState};
use starweaver_core::{RunId, RunLifecycle, SessionId};
use starweaver_stream::{AgentStreamRecord, ReplayEvent, ReplayScope};

use crate::{
    AcquireBackgroundSubagentContinuation, AcquireRunAdmission, BackgroundSubagentArtifact,
    BackgroundSubagentArtifactLimits, BackgroundSubagentContinuationReceipt,
    BackgroundSubagentRecord, BackgroundSubagentTerminalCommit,
    DurableBackgroundSubagentDeliveryClaim, DurableBackgroundSubagentDeliveryRelease,
    DurableBackgroundSubagentDeliveryStatus, DurableBackgroundSubagentExecutionStatus,
    DurableBackgroundSubagentResultRef, DurableControlReceipt, ManagedRunTarget,
    ManagedSessionTarget, RunAdmissionLease, RunAdmissionReceipt, SessionContinuationFence,
    SessionDeletionFence, UpdateManagedSession,
    approval::{ApprovalRecord, ApprovalStatus, DeferredToolRecord},
    claim::{HitlResumeClaim, HitlResumeClaimState},
    error::{SessionStoreError, SessionStoreResult},
    evidence::RunEvidenceCommit,
    publication::{PendingStreamPublication, StreamPublicationTarget},
    records::{
        EnvironmentStateRef, ExecutionStatus, RunRecord, RunStatus, SessionRecord, SessionStatus,
        StreamCursorRef,
    },
    resume::SessionResumeSnapshot,
    trace::{CompactRunTrace, CompactSessionTrace},
};

use super::{SessionFilter, SessionStore};

/// In-memory session store for deterministic tests and single-process hosts.
#[derive(Clone, Debug, Default)]
pub struct InMemorySessionStore {
    inner: Arc<Mutex<StoreInner>>,
}

#[derive(Clone, Debug, Default)]
struct StoreInner {
    sessions: BTreeMap<SessionId, SessionRecord>,
    runs: BTreeMap<(SessionId, RunId), RunRecord>,
    checkpoints: BTreeMap<(SessionId, RunId), Vec<AgentCheckpoint>>,
    streams: BTreeMap<(SessionId, RunId), Vec<AgentStreamRecord>>,
    replay_events: BTreeMap<(ReplayScope, usize), ReplayEvent>,
    approvals: BTreeMap<(SessionId, RunId), Vec<ApprovalRecord>>,
    deferred_tools: BTreeMap<(SessionId, RunId), Vec<DeferredToolRecord>>,
    evidence_commits: BTreeMap<(SessionId, RunId), RunEvidenceCommit>,
    evidence_digests: BTreeMap<(SessionId, RunId), String>,
    hitl_resume_claims: BTreeMap<(SessionId, RunId), HitlResumeClaim>,
    stream_publication_outbox: BTreeMap<String, PendingStreamPublication>,
    session_idempotency: BTreeMap<(String, String), (String, SessionRecord)>,
    run_admission_idempotency: BTreeMap<(String, String), (String, RunAdmissionReceipt)>,
    run_admissions: BTreeMap<ManagedSessionTarget, RunAdmissionLease>,
    admission_generations: BTreeMap<ManagedSessionTarget, u64>,
    control_receipts: BTreeMap<String, DurableControlReceipt>,
    control_idempotency: BTreeMap<(ManagedRunTarget, String), String>,
    background_subagents: BTreeMap<starweaver_core::SubagentAttemptId, BackgroundSubagentRecord>,
    background_artifacts: BTreeMap<String, BackgroundSubagentArtifact>,
    background_terminal_fingerprints: BTreeMap<starweaver_core::SubagentAttemptId, String>,
}

impl InMemorySessionStore {
    /// Create an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

fn run_key(session_id: &SessionId, run_id: &RunId) -> (SessionId, RunId) {
    (session_id.clone(), run_id.clone())
}

fn run_key_label(session_id: &SessionId, run_id: &RunId) -> String {
    format!("{}:{}", session_id.as_str(), run_id.as_str())
}

fn validate_approval_transition(
    existing: &ApprovalRecord,
    resolved: &ApprovalRecord,
) -> SessionStoreResult<()> {
    if existing == resolved {
        return Ok(());
    }
    let same_request = existing.approval_id == resolved.approval_id
        && existing.session_id == resolved.session_id
        && existing.run_id == resolved.run_id
        && existing.action_id == resolved.action_id
        && existing.action_name == resolved.action_name
        && existing.request == resolved.request
        && existing.created_at == resolved.created_at
        && existing.trace_context == resolved.trace_context
        && existing.metadata == resolved.metadata;
    if existing.status != ApprovalStatus::Pending
        || resolved.status == ApprovalStatus::Pending
        || resolved.decision.is_none()
        || !same_request
    {
        return Err(SessionStoreError::Failed(format!(
            "approval transition conflict for {}",
            resolved.approval_id
        )));
    }
    Ok(())
}

const fn checkpoint_run_status(status: RunLifecycle) -> RunStatus {
    match status {
        RunLifecycle::Starting | RunLifecycle::Running => RunStatus::Running,
        RunLifecycle::Waiting => RunStatus::Waiting,
        RunLifecycle::Completed => RunStatus::Completed,
        RunLifecycle::Failed => RunStatus::Failed,
        RunLifecycle::Cancelled => RunStatus::Cancelled,
    }
}

fn validate_deferred_transition(
    existing: &DeferredToolRecord,
    resolved: &DeferredToolRecord,
) -> SessionStoreResult<()> {
    if existing == resolved {
        return Ok(());
    }
    let same_request = existing.deferred_id == resolved.deferred_id
        && existing.session_id == resolved.session_id
        && existing.run_id == resolved.run_id
        && existing.tool_call_id == resolved.tool_call_id
        && existing.tool_name == resolved.tool_name
        && existing.request == resolved.request
        && existing.created_at == resolved.created_at
        && existing.trace_context == resolved.trace_context;
    if !matches!(
        existing.status,
        ExecutionStatus::Pending | ExecutionStatus::Waiting
    ) || matches!(
        resolved.status,
        ExecutionStatus::Pending | ExecutionStatus::Running | ExecutionStatus::Waiting
    ) || !same_request
    {
        return Err(SessionStoreError::Failed(format!(
            "deferred tool transition conflict for {}",
            resolved.deferred_id
        )));
    }
    Ok(())
}

#[allow(clippy::needless_pass_by_value)]
fn store_failed(
    error: std::sync::PoisonError<std::sync::MutexGuard<'_, StoreInner>>,
) -> SessionStoreError {
    SessionStoreError::Failed(error.to_string())
}

fn ensure_active_admission_locked(
    inner: &StoreInner,
    lease: &RunAdmissionLease,
    now: chrono::DateTime<chrono::Utc>,
) -> SessionStoreResult<()> {
    let key = ManagedSessionTarget::new(
        lease.target.namespace_id.clone(),
        lease.target.session_id.clone(),
    );
    let current = inner.run_admissions.get(&key).ok_or_else(|| {
        SessionStoreError::StaleFence("run has no active owner lease".to_string())
    })?;
    if current.target != lease.target
        || current.admission_id != lease.admission_id
        || current.host_instance_id != lease.host_instance_id
        || current.fencing_generation != lease.fencing_generation
    {
        return Err(SessionStoreError::StaleFence(
            "stale admission owner".to_string(),
        ));
    }
    if current.expired_at(now) {
        return Err(SessionStoreError::StaleFence(
            "run admission lease expired".to_string(),
        ));
    }
    Ok(())
}

fn apply_run_status_locked(
    inner: &mut StoreInner,
    session_id: &SessionId,
    run_id: &RunId,
    status: RunStatus,
    output_preview: Option<String>,
    updated_at: chrono::DateTime<chrono::Utc>,
) -> SessionStoreResult<RunRecord> {
    let key = run_key(session_id, run_id);
    let run = inner
        .runs
        .get_mut(&key)
        .ok_or_else(|| SessionStoreError::NotFound(run_key_label(session_id, run_id)))?;
    run.status = status;
    run.output_preview = output_preview;
    run.updated_at = updated_at;
    let result = run.clone();
    if let Some(session) = inner.sessions.get_mut(session_id) {
        session.head_run_id = Some(run_id.clone());
        if status.is_active() {
            session.active_run_id = Some(run_id.clone());
        } else {
            if status == RunStatus::Completed {
                session.head_success_run_id = Some(run_id.clone());
            }
            if session.active_run_id.as_ref() == Some(run_id) {
                session.active_run_id = None;
            }
        }
        session.revision = session.revision.saturating_add(1);
        session.updated_at = updated_at;
    }
    Ok(result)
}

fn resolve_evidence_retry(
    inner: &StoreInner,
    key: &(SessionId, RunId),
    commit: &RunEvidenceCommit,
    digest: &str,
) -> SessionStoreResult<Option<RunRecord>> {
    let Some(existing_digest) = inner.evidence_digests.get(key) else {
        return Ok(None);
    };
    if existing_digest == digest {
        return inner.runs.get(key).cloned().map(Some).ok_or_else(|| {
            SessionStoreError::NotFound(run_key_label(&commit.run.session_id, &commit.run.run_id))
        });
    }
    Err(SessionStoreError::Failed(format!(
        "run evidence conflict for session {} and run {}",
        commit.run.session_id.as_str(),
        commit.run.run_id.as_str()
    )))
}

fn validate_related_evidence(
    inner: &StoreInner,
    commit: &RunEvidenceCommit,
) -> SessionStoreResult<()> {
    for update in &commit.related_run_updates {
        let related_key = run_key(&commit.run.session_id, &update.run_id);
        let claim_id = update.resume_claim_id.as_deref().ok_or_else(|| {
            SessionStoreError::Failed(format!(
                "related run {} requires an exclusive resume claim",
                update.run_id.as_str()
            ))
        })?;
        let claim = inner.hitl_resume_claims.get(&related_key).ok_or_else(|| {
            SessionStoreError::Failed(format!(
                "related run {} has no active resume claim",
                update.run_id.as_str()
            ))
        })?;
        if claim.claim_id != claim_id || claim.state != HitlResumeClaimState::Started {
            return Err(SessionStoreError::Failed(format!(
                "started resume claim conflict for related run {}",
                update.run_id.as_str()
            )));
        }
        for approval in &update.approvals {
            let existing = inner
                .approvals
                .get(&related_key)
                .into_iter()
                .flatten()
                .find(|existing| existing.approval_id == approval.approval_id)
                .ok_or_else(|| SessionStoreError::NotFound(approval.approval_id.clone()))?;
            validate_approval_transition(existing, approval)?;
        }
        for deferred in &update.deferred_tools {
            let existing = inner
                .deferred_tools
                .get(&related_key)
                .into_iter()
                .flatten()
                .find(|existing| existing.deferred_id == deferred.deferred_id)
                .ok_or_else(|| SessionStoreError::NotFound(deferred.deferred_id.clone()))?;
            validate_deferred_transition(existing, deferred)?;
        }
    }
    Ok(())
}

fn validate_existing_evidence(
    inner: &StoreInner,
    key: &(SessionId, RunId),
    commit: &RunEvidenceCommit,
) -> SessionStoreResult<()> {
    if let Some(existing_run) = inner.runs.get(key) {
        for cursor in &commit.stream_cursors {
            for existing in existing_run.stream_cursors.iter().chain(
                inner
                    .sessions
                    .get(&commit.run.session_id)
                    .into_iter()
                    .flat_map(|session| session.stream_cursors.iter()),
            ) {
                cursor
                    .validate_progression(existing)
                    .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
            }
        }
    }
    for approval in &commit.approvals {
        if let Some(existing) = inner
            .approvals
            .get(key)
            .into_iter()
            .flatten()
            .find(|existing| existing.approval_id == approval.approval_id)
            && existing != approval
        {
            return Err(SessionStoreError::Failed(format!(
                "approval conflict for id {}",
                approval.approval_id
            )));
        }
    }
    for deferred in &commit.deferred_tools {
        if let Some(existing) = inner
            .deferred_tools
            .get(key)
            .into_iter()
            .flatten()
            .find(|existing| existing.deferred_id == deferred.deferred_id)
            && existing != deferred
        {
            return Err(SessionStoreError::Failed(format!(
                "deferred tool conflict for id {}",
                deferred.deferred_id
            )));
        }
    }
    validate_related_evidence(inner, commit)
}

fn apply_related_evidence(
    staged: &InMemorySessionStore,
    commit: &RunEvidenceCommit,
) -> SessionStoreResult<()> {
    for update in &commit.related_run_updates {
        let source = staged.load_run_record(&commit.run.session_id, &update.run_id)?;
        if source.status != update.expected_status {
            return Err(SessionStoreError::Failed(format!(
                "related run {} status conflict: expected {}, found {}",
                update.run_id.as_str(),
                update.expected_status.as_str(),
                source.status.as_str()
            )));
        }
        staged.set_run_status(
            &commit.run.session_id,
            &update.run_id,
            update.status,
            update.output_preview.clone(),
        )?;
        for approval in update.approvals.clone() {
            staged.append_approval_record(approval)?;
        }
        for deferred in update.deferred_tools.clone() {
            staged.append_deferred_tool_record(deferred)?;
        }
        staged
            .inner
            .lock()
            .map_err(store_failed)?
            .hitl_resume_claims
            .remove(&run_key(&commit.run.session_id, &update.run_id));
    }
    Ok(())
}

fn apply_primary_evidence(
    staged: &InMemorySessionStore,
    commit: &RunEvidenceCommit,
) -> SessionStoreResult<()> {
    staged.append_run_record(commit.run.clone())?;
    staged.save_context_state_snapshot(&commit.run.session_id, commit.context_state.clone())?;
    for checkpoint in commit.checkpoints.clone() {
        staged.append_checkpoint_record(&commit.run.session_id, checkpoint)?;
    }
    staged.append_stream_record_batch(
        &commit.run.session_id,
        &commit.run.run_id,
        commit.stream_records.clone(),
    )?;
    for cursor in commit.stream_cursors.clone() {
        staged.save_stream_cursor_ref(&commit.run.session_id, &commit.run.run_id, cursor)?;
    }
    for approval in commit.approvals.clone() {
        staged.append_approval_record(approval)?;
    }
    for deferred in commit.deferred_tools.clone() {
        staged.append_deferred_tool_record(deferred)?;
    }
    Ok(())
}

fn finalize_staged_evidence(
    staged: &InMemorySessionStore,
    commit: RunEvidenceCommit,
    key: (SessionId, RunId),
    digest: String,
) -> SessionStoreResult<(StoreInner, RunRecord)> {
    let mut inner = staged.inner.lock().map_err(store_failed)?;
    let commit_timestamp = commit.run.updated_at;
    let target_key = run_key(&commit.run.session_id, &commit.run.run_id);
    inner
        .runs
        .get_mut(&target_key)
        .ok_or_else(|| {
            SessionStoreError::NotFound(run_key_label(&commit.run.session_id, &commit.run.run_id))
        })?
        .updated_at = commit_timestamp;
    for update in &commit.related_run_updates {
        if let Some(source) = inner
            .runs
            .get_mut(&run_key(&commit.run.session_id, &update.run_id))
        {
            source.updated_at = commit_timestamp;
        }
    }
    if let Some(session) = inner.sessions.get_mut(&commit.run.session_id) {
        session.updated_at = commit_timestamp;
    }
    let committed = inner.runs.get(&target_key).cloned().ok_or_else(|| {
        SessionStoreError::NotFound(run_key_label(&commit.run.session_id, &commit.run.run_id))
    })?;
    if !commit.publication_targets.is_empty() {
        let mut publication = PendingStreamPublication::new(
            commit.run.session_id.clone(),
            commit.run.run_id.clone(),
            commit.publication_targets,
            commit.run.updated_at,
        );
        publication
            .stream_records
            .clone_from(&commit.stream_records);
        publication
            .display_messages
            .clone_from(&commit.display_messages);
        publication.replay_events.clone_from(&commit.replay_events);
        publication
            .display_snapshot
            .clone_from(&commit.display_snapshot);
        inner
            .stream_publication_outbox
            .insert(publication.publication_id.clone(), publication);
    }
    inner.evidence_commits.insert(key.clone(), commit);
    inner.evidence_digests.insert(key, digest);
    Ok((inner.clone(), committed))
}

#[allow(clippy::too_many_lines)]
fn acquire_run_admission_locked(
    inner: &mut StoreInner,
    request: AcquireRunAdmission,
) -> SessionStoreResult<RunAdmissionReceipt> {
    let idempotency_key = (
        request.namespace_id.clone(),
        request.idempotency_key.clone(),
    );
    if let Some((fingerprint, receipt)) = inner.run_admission_idempotency.get(&idempotency_key) {
        if fingerprint == &request.command_fingerprint {
            let mut receipt = receipt.clone();
            receipt.idempotent_replay = true;
            return Ok(receipt);
        }
        return Err(SessionStoreError::IdempotencyConflict(
            request.idempotency_key,
        ));
    }
    if request.replaces_waiting_run_id.is_some() != request.hitl_resume_claim_id.is_some() {
        return Err(SessionStoreError::Conflict(
            "waiting-run replacement requires exactly one preflight HITL claim".to_string(),
        ));
    }
    let session_target =
        ManagedSessionTarget::new(request.namespace_id.clone(), request.run.session_id.clone());
    let now = chrono::Utc::now();
    if let Some(active) = inner.run_admissions.get(&session_target).cloned() {
        if !active.expired_at(now) {
            return Err(SessionStoreError::RunConflict(format!(
                "session {} already has active run {}",
                request.run.session_id.as_str(),
                active.target.run_id.as_str()
            )));
        }
        if let Some(run) = inner
            .runs
            .get_mut(&run_key(&active.target.session_id, &active.target.run_id))
        {
            run.status = RunStatus::Cancelled;
            run.updated_at = now;
        }
        inner.run_admissions.remove(&session_target);
        if let Some(session) = inner.sessions.get_mut(&request.run.session_id)
            && session.active_run_id.as_ref() == Some(&active.target.run_id)
        {
            session.active_run_id = None;
        }
    }
    {
        let session = inner.sessions.get(&request.run.session_id).ok_or_else(|| {
            SessionStoreError::NotFound(request.run.session_id.as_str().to_string())
        })?;
        if session.namespace_id != request.namespace_id {
            return Err(SessionStoreError::NotFound(
                request.run.session_id.as_str().to_string(),
            ));
        }
        if session.status != SessionStatus::Active || session.deletion_fence.blocks_continuation() {
            return Err(SessionStoreError::Conflict(
                "session cannot admit new work".to_string(),
            ));
        }
        if let Some(active_run_id) = session.active_run_id.as_ref() {
            let valid_waiting_replacement = request.replaces_waiting_run_id.as_ref()
                == Some(active_run_id)
                && request.run.restore_from_run_id.as_ref() == Some(active_run_id)
                && inner
                    .runs
                    .get(&run_key(&request.run.session_id, active_run_id))
                    .is_some_and(|source| source.status == RunStatus::Waiting);
            if !valid_waiting_replacement {
                return Err(SessionStoreError::RunConflict(format!(
                    "session {} already has active run {}",
                    request.run.session_id.as_str(),
                    active_run_id.as_str()
                )));
            }
        } else if request.replaces_waiting_run_id.is_some() {
            return Err(SessionStoreError::Conflict(
                "waiting-run replacement has no parked active run".to_string(),
            ));
        }
    }
    let hitl_claim_key = match (
        request.replaces_waiting_run_id.as_ref(),
        request.hitl_resume_claim_id.as_deref(),
    ) {
        (Some(source_run_id), Some(claim_id)) => {
            let key = run_key(&request.run.session_id, source_run_id);
            let claim = inner.hitl_resume_claims.get(&key).ok_or_else(|| {
                SessionStoreError::NotFound(format!("resume claim for {}", source_run_id.as_str()))
            })?;
            if claim.claim_id != claim_id
                || claim.session_id != request.run.session_id
                || claim.run_id != *source_run_id
                || claim.state != HitlResumeClaimState::Preflight
            {
                return Err(SessionStoreError::Conflict(format!(
                    "invalid preflight resume claim for run {}",
                    source_run_id.as_str()
                )));
            }
            Some(key)
        }
        (None, None) => None,
        _ => unreachable!("replacement and claim presence checked above"),
    };
    if let Some(key) = hitl_claim_key.as_ref() {
        let claim = inner.hitl_resume_claims.get_mut(key).ok_or_else(|| {
            SessionStoreError::Conflict("validated preflight resume claim disappeared".to_string())
        })?;
        claim.state = HitlResumeClaimState::Started;
    }
    let mut run = request.run;
    run.status = RunStatus::Queued;
    if run.sequence_no == 0 {
        run.sequence_no = inner
            .runs
            .values()
            .filter(|current| current.session_id == run.session_id)
            .map(|current| current.sequence_no)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
    }
    run.updated_at = now;
    let generation = inner
        .admission_generations
        .entry(session_target.clone())
        .and_modify(|generation| *generation = generation.saturating_add(1))
        .or_insert(1);
    let lease = RunAdmissionLease {
        target: ManagedRunTarget::new(
            request.namespace_id,
            run.session_id.clone(),
            run.run_id.clone(),
        ),
        admission_id: request.admission_id,
        host_instance_id: request.host_instance_id,
        fencing_generation: *generation,
        lease_expires_at: request.lease_expires_at,
        heartbeat_at: now,
        command_fingerprint: request.command_fingerprint.clone(),
        idempotency_key: request.idempotency_key,
    };
    inner
        .runs
        .insert(run_key(&run.session_id, &run.run_id), run.clone());
    let session = inner
        .sessions
        .get_mut(&run.session_id)
        .ok_or_else(|| SessionStoreError::NotFound(run.session_id.as_str().to_string()))?;
    session.head_run_id = Some(run.run_id.clone());
    session.active_run_id = Some(run.run_id.clone());
    session.revision = session.revision.saturating_add(1);
    session.updated_at = now;
    inner.run_admissions.insert(session_target, lease.clone());
    let receipt = RunAdmissionReceipt {
        run,
        lease,
        idempotent_replay: false,
    };
    inner.run_admission_idempotency.insert(
        idempotency_key,
        (request.command_fingerprint, receipt.clone()),
    );
    Ok(receipt)
}

fn same_background_identity(
    current: &BackgroundSubagentRecord,
    next: &BackgroundSubagentRecord,
) -> bool {
    current.schema_version == next.schema_version
        && current.attempt_id == next.attempt_id
        && current.agent_id == next.agent_id
        && current.linked_task_id == next.linked_task_id
        && current.subagent_name == next.subagent_name
        && current.namespace_id == next.namespace_id
        && current.parent_session_id == next.parent_session_id
        && current.parent_run_id == next.parent_run_id
        && current.profile == next.profile
        && current.owner_lease.host_instance_id == next.owner_lease.host_instance_id
        && current.owner_lease.fencing_generation == next.owner_lease.fencing_generation
        && current.accepted_at == next.accepted_at
}

fn same_background_owner(
    current: &BackgroundSubagentRecord,
    next: &BackgroundSubagentRecord,
) -> bool {
    current.owner_lease.host_instance_id == next.owner_lease.host_instance_id
        && current.owner_lease.fencing_generation == next.owner_lease.fencing_generation
}

fn valid_background_terminal_base(
    current: &BackgroundSubagentRecord,
    next: &BackgroundSubagentRecord,
) -> bool {
    current.delivery_status == DurableBackgroundSubagentDeliveryStatus::Undelivered
        && current.delivery_claim.is_none()
        && current.delivered_claim_id.is_none()
        && current.continuation_run_id.is_none()
        && current
            .automatic_continuation_suppressed_by_run_id
            .is_none()
        && next.automatic_continuation_suppressed_by_run_id.is_none()
        && current.retention_status == crate::DurableBackgroundSubagentRetentionStatus::Inline
        && current.retention_expires_at.is_none()
        && matches!(
            next.retention_status,
            crate::DurableBackgroundSubagentRetentionStatus::Inline
                | crate::DurableBackgroundSubagentRetentionStatus::Artifact
        )
        && current.trace_context == next.trace_context
        && next.updated_at >= current.updated_at
}

fn same_background_non_execution_state(
    current: &BackgroundSubagentRecord,
    next: &BackgroundSubagentRecord,
) -> bool {
    current.continuation_run_id == next.continuation_run_id
        && current.result_ref == next.result_ref
        && current.failure_category == next.failure_category
        && current.cancellation_reason == next.cancellation_reason
        && current.delivery_status == next.delivery_status
        && current.delivery_claim == next.delivery_claim
        && current.delivered_claim_id == next.delivered_claim_id
        && current.automatic_continuation_suppressed_by_run_id
            == next.automatic_continuation_suppressed_by_run_id
        && current.retention_status == next.retention_status
        && current.retention_expires_at == next.retention_expires_at
        && current.trace_context == next.trace_context
        && current.terminal_at == next.terminal_at
        && next.updated_at >= current.updated_at
}

fn continuation_artifact_content(
    inner: &StoreInner,
    background: &BackgroundSubagentRecord,
    now: chrono::DateTime<chrono::Utc>,
) -> SessionStoreResult<Option<String>> {
    if background.retention_status != crate::DurableBackgroundSubagentRetentionStatus::Artifact {
        return Ok(None);
    }
    let result_ref = background.result_ref.as_ref().ok_or_else(|| {
        SessionStoreError::Conflict("artifact result is missing terminal evidence".to_string())
    })?;
    let artifact_ref = result_ref.artifact_ref.as_deref().ok_or_else(|| {
        SessionStoreError::Conflict("artifact result is missing its reference".to_string())
    })?;
    let artifact = inner
        .background_artifacts
        .get(artifact_ref)
        .ok_or_else(|| SessionStoreError::NotFound(artifact_ref.to_string()))?;
    if !artifact.is_available_at(now)
        || artifact.namespace_id != background.namespace_id
        || artifact.attempt_id != background.attempt_id
        || artifact.digest != result_ref.digest.clone().unwrap_or_default()
        || artifact.size_bytes != result_ref.size_bytes
        || background.retention_expires_at != Some(artifact.expires_at)
    {
        return Err(SessionStoreError::Conflict(
            "background-subagent artifact failed integrity or retention validation".to_string(),
        ));
    }
    Ok(Some(artifact.content.clone()))
}

fn continuation_receipt_matches_request(
    background: &BackgroundSubagentRecord,
    request: &AcquireBackgroundSubagentContinuation,
    admission: &RunAdmissionReceipt,
) -> bool {
    let proposed = &request.admission.run;
    let admitted = &admission.run;
    background.validates_continuation_cause_envelope(&request.cause, admitted)
        && admitted.session_id == proposed.session_id
        && admitted.run_id == proposed.run_id
        && admitted.conversation_id == proposed.conversation_id
        && admitted.input == proposed.input
        && admitted.parent_run_id == proposed.parent_run_id
        && admitted.parent_task_id == proposed.parent_task_id
        && admitted.trigger_type == proposed.trigger_type
        && admitted.profile == proposed.profile
        && admitted.trace_context == proposed.trace_context
        && admitted.metadata == proposed.metadata
        && admission.lease.target.session_id == proposed.session_id
        && admission.lease.target.run_id == proposed.run_id
        && admission.lease.target.namespace_id == request.admission.namespace_id
        && admission.lease.command_fingerprint == request.admission.command_fingerprint
        && admission.lease.idempotency_key == request.admission.idempotency_key
}

fn background_terminal_fingerprint(
    record: &BackgroundSubagentRecord,
    artifact: Option<&BackgroundSubagentArtifact>,
) -> SessionStoreResult<String> {
    BackgroundSubagentTerminalCommit {
        record: record.clone(),
        artifact: artifact.cloned(),
        artifact_limits: None,
    }
    .canonical_fingerprint()
    .map_err(|error| SessionStoreError::Failed(error.to_string()))
}

fn persisted_background_terminal_fingerprint(
    inner: &StoreInner,
    current: &BackgroundSubagentRecord,
) -> SessionStoreResult<Option<String>> {
    if let Some(fingerprint) = inner
        .background_terminal_fingerprints
        .get(&current.attempt_id)
    {
        return Ok(Some(fingerprint.clone()));
    }
    reconstruct_background_terminal_fingerprint(inner, current)
}

fn reconstruct_background_terminal_fingerprint(
    inner: &StoreInner,
    current: &BackgroundSubagentRecord,
) -> SessionStoreResult<Option<String>> {
    if !current.execution_status.is_terminal()
        || current.retention_status == crate::DurableBackgroundSubagentRetentionStatus::Expired
        || current.result_ref.is_none()
    {
        return Ok(None);
    }
    let Some(terminal_at) = current.terminal_at else {
        return Ok(None);
    };
    if current
        .retention_expires_at
        .is_none_or(|deadline| deadline <= terminal_at)
    {
        return Ok(None);
    }
    let artifact = match current.retention_status {
        crate::DurableBackgroundSubagentRetentionStatus::Inline => None,
        crate::DurableBackgroundSubagentRetentionStatus::Artifact => {
            let Some(result_ref) = current.result_ref.as_ref() else {
                return Ok(None);
            };
            let Some(artifact_ref) = result_ref.artifact_ref.as_deref() else {
                return Ok(None);
            };
            let Some(artifact) = inner.background_artifacts.get(artifact_ref) else {
                return Ok(None);
            };
            if !artifact.is_valid()
                || artifact.namespace_id != current.namespace_id
                || artifact.attempt_id != current.attempt_id
                || artifact.digest != result_ref.digest.clone().unwrap_or_default()
                || artifact.size_bytes != result_ref.size_bytes
                || current.retention_expires_at != Some(artifact.expires_at)
            {
                return Ok(None);
            }
            Some(artifact)
        }
        crate::DurableBackgroundSubagentRetentionStatus::Expired => return Ok(None),
    };
    let mut terminal = current.clone();
    terminal.updated_at = terminal_at;
    background_terminal_fingerprint(&terminal, artifact).map(Some)
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum BackgroundClaimConsumerState {
    Reclaimable,
    Live,
    Completed(RunId),
    Terminated(RunId),
}

fn background_claim_consumer_state(
    inner: &StoreInner,
    record: &BackgroundSubagentRecord,
    claim: &DurableBackgroundSubagentDeliveryClaim,
    now: chrono::DateTime<chrono::Utc>,
) -> BackgroundClaimConsumerState {
    let Some(run_id) = claim.continuation_run_id.as_ref() else {
        return BackgroundClaimConsumerState::Reclaimable;
    };
    let Some(run) = inner.runs.get(&run_key(&record.parent_session_id, run_id)) else {
        return BackgroundClaimConsumerState::Reclaimable;
    };
    match run.status {
        RunStatus::Completed => BackgroundClaimConsumerState::Completed(run_id.clone()),
        RunStatus::Failed | RunStatus::Cancelled => {
            BackgroundClaimConsumerState::Terminated(run_id.clone())
        }
        status
            if status.is_active()
                && inner
                    .run_admissions
                    .get(&ManagedSessionTarget::new(
                        record.namespace_id.clone(),
                        record.parent_session_id.clone(),
                    ))
                    .is_some_and(|lease| {
                        lease.target.run_id == *run_id && !lease.expired_at(now)
                    }) =>
        {
            BackgroundClaimConsumerState::Live
        }
        _ => BackgroundClaimConsumerState::Reclaimable,
    }
}

fn validate_background_artifact_quota(
    inner: &StoreInner,
    artifact: &BackgroundSubagentArtifact,
    limits: BackgroundSubagentArtifactLimits,
    now: chrono::DateTime<chrono::Utc>,
) -> SessionStoreResult<()> {
    if !artifact.is_available_at(now) {
        return Err(SessionStoreError::Conflict(
            "background-subagent artifact is invalid or already expired".to_string(),
        ));
    }
    let retained_bytes = inner
        .background_artifacts
        .values()
        .filter(|current| {
            current.namespace_id == artifact.namespace_id
                && current.artifact_ref != artifact.artifact_ref
                && current.expires_at > now
        })
        .map(|current| current.size_bytes)
        .fold(0u64, u64::saturating_add);
    if artifact.size_bytes > limits.max_single_bytes
        || retained_bytes.saturating_add(artifact.size_bytes) > limits.max_retained_bytes
    {
        return Err(SessionStoreError::QuotaExceeded(
            "background-subagent artifact exceeds host retention quota".to_string(),
        ));
    }
    Ok(())
}

fn ensure_background_parent_writable(
    inner: &StoreInner,
    record: &BackgroundSubagentRecord,
) -> SessionStoreResult<()> {
    let session = inner
        .sessions
        .get(&record.parent_session_id)
        .filter(|session| session.namespace_id == record.namespace_id)
        .ok_or_else(|| {
            SessionStoreError::NotFound(record.parent_session_id.as_str().to_string())
        })?;
    if session.status != SessionStatus::Active || session.deletion_fence.blocks_continuation() {
        return Err(SessionStoreError::Conflict(
            "background owner write rejected because parent session is deleting or deleted"
                .to_string(),
        ));
    }
    Ok(())
}

fn commit_background_terminal(
    inner: &mut StoreInner,
    mut record: BackgroundSubagentRecord,
    artifact: Option<BackgroundSubagentArtifact>,
    artifact_limits: Option<BackgroundSubagentArtifactLimits>,
) -> SessionStoreResult<BackgroundSubagentRecord> {
    if !record.is_valid_terminal() {
        return Err(SessionStoreError::Conflict(
            "invalid background-subagent terminal record".to_string(),
        ));
    }
    if artifact.is_some() != artifact_limits.is_some()
        || artifact_limits.is_some_and(|limits| !limits.is_valid())
    {
        return Err(SessionStoreError::Conflict(
            "background-subagent artifact requires valid host quota limits".to_string(),
        ));
    }
    let current = inner
        .background_subagents
        .get(&record.attempt_id)
        .cloned()
        .ok_or_else(|| SessionStoreError::NotFound(record.attempt_id.as_str().to_string()))?;
    ensure_background_parent_writable(inner, &current)?;
    let terminal_fingerprint = background_terminal_fingerprint(&record, artifact.as_ref())?;
    if current.execution_status.is_terminal() {
        let persisted_fingerprint = persisted_background_terminal_fingerprint(inner, &current)?;
        return if persisted_fingerprint.as_deref() == Some(terminal_fingerprint.as_str()) {
            inner
                .background_terminal_fingerprints
                .insert(current.attempt_id.clone(), terminal_fingerprint);
            Ok(current)
        } else {
            Err(SessionStoreError::Conflict(format!(
                "terminal background attempt {} is immutable",
                record.attempt_id.as_str()
            )))
        };
    }
    let now = chrono::Utc::now();
    if current.owner_lease.expired_at(now) {
        return Err(SessionStoreError::Conflict(format!(
            "background owner lease expired for {}",
            record.attempt_id.as_str()
        )));
    }
    if let (Some(artifact), Some(limits)) = (artifact.as_ref(), artifact_limits) {
        validate_background_artifact_quota(inner, artifact, limits, now)?;
    }
    if !same_background_identity(&current, &record)
        || !same_background_owner(&current, &record)
        || !valid_background_terminal_base(&current, &record)
        || !valid_background_transition(current.execution_status, record.execution_status)
    {
        return Err(SessionStoreError::Conflict(format!(
            "invalid terminal background transition for {}",
            record.attempt_id.as_str()
        )));
    }
    if let Some(artifact) = artifact {
        let artifact_matches = artifact.is_available_at(now)
            && artifact.attempt_id == record.attempt_id
            && artifact.namespace_id == record.namespace_id
            && record.retention_status == crate::DurableBackgroundSubagentRetentionStatus::Artifact
            && record.retention_expires_at == Some(artifact.expires_at)
            && record.result_ref.as_ref().is_some_and(|result| {
                result.artifact_ref.as_deref() == Some(artifact.artifact_ref.as_str())
                    && result.digest.as_deref() == Some(artifact.digest.as_str())
                    && result.size_bytes == artifact.size_bytes
            });
        if !artifact_matches {
            return Err(SessionStoreError::Conflict(
                "background-subagent artifact does not match terminal evidence".to_string(),
            ));
        }
        if let Some(existing) = inner.background_artifacts.get(&artifact.artifact_ref) {
            if existing != &artifact {
                return Err(SessionStoreError::Conflict(
                    "background-subagent artifact identity conflict".to_string(),
                ));
            }
        } else {
            inner
                .background_artifacts
                .insert(artifact.artifact_ref.clone(), artifact);
        }
    } else if record.retention_status == crate::DurableBackgroundSubagentRetentionStatus::Artifact {
        return Err(SessionStoreError::Conflict(
            "artifact retention requires an atomic artifact payload".to_string(),
        ));
    }
    record.owner_lease = current.owner_lease;
    inner
        .background_terminal_fingerprints
        .insert(record.attempt_id.clone(), terminal_fingerprint);
    inner
        .background_subagents
        .insert(record.attempt_id.clone(), record.clone());
    Ok(record)
}

fn valid_background_transition(
    current: DurableBackgroundSubagentExecutionStatus,
    next: DurableBackgroundSubagentExecutionStatus,
) -> bool {
    current == next
        || matches!(
            (current, next),
            (
                DurableBackgroundSubagentExecutionStatus::Accepted,
                DurableBackgroundSubagentExecutionStatus::Starting
                    | DurableBackgroundSubagentExecutionStatus::Failed
                    | DurableBackgroundSubagentExecutionStatus::Cancelled
            ) | (
                DurableBackgroundSubagentExecutionStatus::Starting,
                DurableBackgroundSubagentExecutionStatus::Running
                    | DurableBackgroundSubagentExecutionStatus::Failed
                    | DurableBackgroundSubagentExecutionStatus::Cancelled
            ) | (
                DurableBackgroundSubagentExecutionStatus::Running,
                DurableBackgroundSubagentExecutionStatus::Waiting
                    | DurableBackgroundSubagentExecutionStatus::Completed
                    | DurableBackgroundSubagentExecutionStatus::Failed
                    | DurableBackgroundSubagentExecutionStatus::Cancelled
            ) | (
                DurableBackgroundSubagentExecutionStatus::Waiting,
                DurableBackgroundSubagentExecutionStatus::Running
                    | DurableBackgroundSubagentExecutionStatus::Completed
                    | DurableBackgroundSubagentExecutionStatus::Failed
                    | DurableBackgroundSubagentExecutionStatus::Cancelled
            )
        )
}

#[allow(clippy::too_many_lines)]
#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn commit_run_evidence(
        &self,
        mut commit: RunEvidenceCommit,
    ) -> SessionStoreResult<RunRecord> {
        commit.run.stream_cursors.clone_from(&commit.stream_cursors);
        commit.validate()?;
        let digest = commit.digest()?;
        let key = run_key(&commit.run.session_id, &commit.run.run_id);
        let mut original = self.inner.lock().map_err(store_failed)?;
        if let Some(existing) = resolve_evidence_retry(&original, &key, &commit, &digest)? {
            return Ok(existing);
        }
        validate_existing_evidence(&original, &key, &commit)?;
        let staged = Self {
            inner: Arc::new(Mutex::new(original.clone())),
        };
        apply_related_evidence(&staged, &commit)?;
        apply_primary_evidence(&staged, &commit)?;
        let (committed_inner, committed_run) =
            finalize_staged_evidence(&staged, commit, key, digest)?;
        *original = committed_inner;
        Ok(committed_run)
    }

    async fn commit_run_evidence_fenced(
        &self,
        lease: &RunAdmissionLease,
        mut commit: RunEvidenceCommit,
    ) -> SessionStoreResult<RunRecord> {
        if lease.target.session_id != commit.run.session_id
            || lease.target.run_id != commit.run.run_id
        {
            return Err(SessionStoreError::Conflict(
                "run evidence does not match admission target".to_string(),
            ));
        }
        commit.run.stream_cursors.clone_from(&commit.stream_cursors);
        commit.validate()?;
        let digest = commit.digest()?;
        let key = run_key(&commit.run.session_id, &commit.run.run_id);
        let mut original = self.inner.lock().map_err(store_failed)?;
        if let Some(existing) = resolve_evidence_retry(&original, &key, &commit, &digest)? {
            return Ok(existing);
        }
        ensure_active_admission_locked(&original, lease, chrono::Utc::now())?;
        validate_existing_evidence(&original, &key, &commit)?;
        let staged = Self {
            inner: Arc::new(Mutex::new(original.clone())),
        };
        apply_related_evidence(&staged, &commit)?;
        apply_primary_evidence(&staged, &commit)?;
        let (committed_inner, committed_run) =
            finalize_staged_evidence(&staged, commit, key, digest)?;
        *original = committed_inner;
        Ok(committed_run)
    }

    async fn append_replay_events_fenced(
        &self,
        lease: &RunAdmissionLease,
        events: Vec<ReplayEvent>,
    ) -> SessionStoreResult<()> {
        let expected_scope = ReplayScope::run(lease.target.run_id.as_str());
        for event in &events {
            if event.scope != expected_scope {
                return Err(SessionStoreError::Conflict(format!(
                    "replay event scope {} does not match admission run {}",
                    event.scope.as_str(),
                    lease.target.run_id.as_str()
                )));
            }
            i64::try_from(event.sequence).map_err(|error| {
                SessionStoreError::Failed(format!("invalid replay event sequence: {error}"))
            })?;
        }

        let mut inner = self.inner.lock().map_err(store_failed)?;
        ensure_active_admission_locked(&inner, lease, chrono::Utc::now())?;
        let mut staged = inner.replay_events.clone();
        for event in events {
            let key = (expected_scope.clone(), event.sequence);
            if let Some(persisted) = staged.get(&key) {
                if persisted != &event {
                    return Err(SessionStoreError::Failed(format!(
                        "replay event conflict for scope {} at sequence {}",
                        expected_scope.as_str(),
                        event.sequence
                    )));
                }
            } else {
                staged.insert(key, event);
            }
        }
        inner.replay_events = staged;
        Ok(())
    }

    async fn commit_checkpoint(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()> {
        let mut original = self.inner.lock().map_err(store_failed)?;
        let staged = Self {
            inner: Arc::new(Mutex::new(original.clone())),
        };
        if staged.load_session_record(session_id).is_err() {
            staged.save_session_record(SessionRecord::new(session_id.clone()))?;
        }
        if staged
            .load_run_record(session_id, &checkpoint.run_id)
            .is_err()
        {
            let mut run = RunRecord::new(
                session_id.clone(),
                checkpoint.run_id.clone(),
                checkpoint.conversation_id.clone(),
            );
            run.status = checkpoint_run_status(checkpoint.resume.status);
            run.trace_context = checkpoint.resume.trace_context.clone();
            run.parent_run_id
                .clone_from(&checkpoint.state.parent_run_id);
            run.parent_task_id
                .clone_from(&checkpoint.state.parent_task_id);
            staged.append_run_record(run)?;
        }
        staged.append_checkpoint_record(session_id, checkpoint)?;
        let staged_inner = staged.inner.lock().map_err(store_failed)?;
        *original = staged_inner.clone();
        Ok(())
    }

    async fn commit_checkpoint_fenced(
        &self,
        lease: &RunAdmissionLease,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()> {
        if lease.target.run_id != checkpoint.run_id {
            return Err(SessionStoreError::Conflict(
                "checkpoint does not match admission target".to_string(),
            ));
        }
        let key = run_key(&lease.target.session_id, &checkpoint.run_id);
        let mut original = self.inner.lock().map_err(store_failed)?;
        if let Some(existing) = original
            .checkpoints
            .get(&key)
            .into_iter()
            .flatten()
            .find(|existing| existing.checkpoint_id == checkpoint.checkpoint_id)
        {
            if existing == &checkpoint {
                return Ok(());
            }
            return Err(SessionStoreError::Failed(format!(
                "checkpoint conflict for session {} run {} checkpoint {}",
                lease.target.session_id.as_str(),
                checkpoint.run_id.as_str(),
                checkpoint.checkpoint_id.as_str()
            )));
        }
        ensure_active_admission_locked(&original, lease, chrono::Utc::now())?;
        let staged = Self {
            inner: Arc::new(Mutex::new(original.clone())),
        };
        if staged
            .load_session_record(&lease.target.session_id)
            .is_err()
        {
            staged.save_session_record(SessionRecord::new(lease.target.session_id.clone()))?;
        }
        if staged
            .load_run_record(&lease.target.session_id, &checkpoint.run_id)
            .is_err()
        {
            let mut run = RunRecord::new(
                lease.target.session_id.clone(),
                checkpoint.run_id.clone(),
                checkpoint.conversation_id.clone(),
            );
            run.status = checkpoint_run_status(checkpoint.resume.status);
            run.trace_context = checkpoint.resume.trace_context.clone();
            run.parent_run_id
                .clone_from(&checkpoint.state.parent_run_id);
            run.parent_task_id
                .clone_from(&checkpoint.state.parent_task_id);
            staged.append_run_record(run)?;
        }
        staged.append_checkpoint_record(&lease.target.session_id, checkpoint)?;
        let staged_inner = staged.inner.lock().map_err(store_failed)?;
        *original = staged_inner.clone();
        Ok(())
    }

    async fn claim_hitl_resume(&self, claim: HitlResumeClaim) -> SessionStoreResult<()> {
        if !claim.is_valid_preflight() {
            return Err(SessionStoreError::Failed(
                "invalid HITL preflight claim".to_string(),
            ));
        }
        let key = run_key(&claim.session_id, &claim.run_id);
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let run = inner.runs.get(&key).ok_or_else(|| {
            SessionStoreError::NotFound(run_key_label(&claim.session_id, &claim.run_id))
        })?;
        if run.status != RunStatus::Waiting {
            return Err(SessionStoreError::Failed(format!(
                "run {} is not waiting",
                claim.run_id.as_str()
            )));
        }
        if let Some(existing) = inner.hitl_resume_claims.get(&key) {
            if existing == &claim {
                return Ok(());
            }
            return Err(SessionStoreError::Failed(format!(
                "run {} already has an active resume claim",
                claim.run_id.as_str()
            )));
        }
        inner.hitl_resume_claims.insert(key, claim);
        Ok(())
    }

    async fn mark_hitl_resume_started(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        claim_id: &str,
    ) -> SessionStoreResult<()> {
        let key = run_key(session_id, run_id);
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let claim = inner.hitl_resume_claims.get_mut(&key).ok_or_else(|| {
            SessionStoreError::NotFound(format!("resume claim for {}", run_id.as_str()))
        })?;
        if claim.claim_id != claim_id {
            return Err(SessionStoreError::Failed(format!(
                "resume claim conflict for run {}",
                run_id.as_str()
            )));
        }
        if claim.state == HitlResumeClaimState::Preflight {
            claim.state = HitlResumeClaimState::Started;
        }
        Ok(())
    }

    async fn release_hitl_resume_claim(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        claim_id: &str,
    ) -> SessionStoreResult<()> {
        let key = run_key(session_id, run_id);
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let Some(existing) = inner.hitl_resume_claims.get(&key) else {
            return Ok(());
        };
        if existing.claim_id != claim_id {
            return Err(SessionStoreError::Failed(format!(
                "resume claim conflict for run {}",
                run_id.as_str()
            )));
        }
        if existing.state != HitlResumeClaimState::Preflight {
            return Err(SessionStoreError::Failed(format!(
                "started resume claim for run {} cannot be released",
                run_id.as_str()
            )));
        }
        inner.hitl_resume_claims.remove(&key);
        Ok(())
    }

    async fn pending_stream_publications(
        &self,
        session_id: &SessionId,
    ) -> SessionStoreResult<Vec<PendingStreamPublication>> {
        let inner = self.inner.lock().map_err(store_failed)?;
        Ok(inner
            .stream_publication_outbox
            .values()
            .filter(|publication| &publication.session_id == session_id)
            .cloned()
            .collect())
    }

    async fn acknowledge_stream_publication(
        &self,
        publication_id: &str,
        target: StreamPublicationTarget,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let Some(publication) = inner.stream_publication_outbox.get_mut(publication_id) else {
            return Ok(());
        };
        match target {
            StreamPublicationTarget::Archive => publication.archive_pending = false,
            StreamPublicationTarget::Replay => publication.replay_pending = false,
        }
        if publication.is_complete() {
            inner.stream_publication_outbox.remove(publication_id);
        }
        Ok(())
    }

    async fn create_session_idempotent(
        &self,
        mut session: SessionRecord,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> SessionStoreResult<SessionRecord> {
        let key = (session.namespace_id.clone(), idempotency_key.to_string());
        let mut inner = self.inner.lock().map_err(store_failed)?;
        if let Some((fingerprint, existing)) = inner.session_idempotency.get(&key) {
            if fingerprint == command_fingerprint {
                return Ok(existing.clone());
            }
            return Err(SessionStoreError::IdempotencyConflict(
                idempotency_key.to_string(),
            ));
        }
        if inner.sessions.contains_key(&session.session_id) {
            return Err(SessionStoreError::AlreadyExists(
                session.session_id.as_str().to_string(),
            ));
        }
        session.revision = session.revision.max(1);
        session.updated_at = chrono::Utc::now();
        inner
            .sessions
            .insert(session.session_id.clone(), session.clone());
        inner
            .session_idempotency
            .insert(key, (command_fingerprint.to_string(), session.clone()));
        Ok(session)
    }

    async fn update_managed_session(
        &self,
        command: UpdateManagedSession,
        command_fingerprint: &str,
    ) -> SessionStoreResult<SessionRecord> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let namespace = inner
            .sessions
            .get(&command.session_id)
            .ok_or_else(|| SessionStoreError::NotFound(command.session_id.as_str().to_string()))?
            .namespace_id
            .clone();
        let idempotency_key = (namespace, command.idempotency_key.clone());
        if let Some((fingerprint, existing)) = inner.session_idempotency.get(&idempotency_key) {
            if fingerprint == command_fingerprint {
                return Ok(existing.clone());
            }
            return Err(SessionStoreError::IdempotencyConflict(
                command.idempotency_key,
            ));
        }
        let session = inner
            .sessions
            .get_mut(&command.session_id)
            .ok_or_else(|| SessionStoreError::NotFound(command.session_id.as_str().to_string()))?;
        if session.revision != command.expected_revision {
            return Err(SessionStoreError::Conflict(format!(
                "expected revision {}, current {}",
                command.expected_revision, session.revision
            )));
        }
        if session.deletion_fence.blocks_continuation() || session.status == SessionStatus::Deleted
        {
            return Err(SessionStoreError::Conflict(
                "session is deleting or deleted".to_string(),
            ));
        }
        if let Some(title) = command.patch.title {
            session.title = title.map(|value| value.chars().take(256).collect());
        }
        if let Some(profile) = command.patch.profile {
            session.profile = profile;
        }
        if let Some(archived) = command.patch.archived {
            session.status = if archived {
                SessionStatus::Archived
            } else {
                SessionStatus::Active
            };
        }
        for (key, value) in command.patch.metadata {
            if value.is_null() {
                session.metadata.remove(&key);
            } else {
                session.metadata.insert(key, value);
            }
        }
        session.revision = session.revision.saturating_add(1);
        session.updated_at = chrono::Utc::now();
        let result = session.clone();
        inner.session_idempotency.insert(
            idempotency_key,
            (command_fingerprint.to_string(), result.clone()),
        );
        Ok(result)
    }

    async fn acquire_session_deletion_fence(
        &self,
        session_id: &SessionId,
        expected_revision: u64,
        fence_id: &str,
        requested_by: &str,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> SessionStoreResult<SessionRecord> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let namespace = inner
            .sessions
            .get(session_id)
            .ok_or_else(|| SessionStoreError::NotFound(session_id.as_str().to_string()))?
            .namespace_id
            .clone();
        let key = (namespace, idempotency_key.to_string());
        if let Some((fingerprint, existing)) = inner.session_idempotency.get(&key) {
            if fingerprint == command_fingerprint {
                return Ok(existing.clone());
            }
            return Err(SessionStoreError::IdempotencyConflict(
                idempotency_key.to_string(),
            ));
        }
        if inner
            .sessions
            .get(session_id)
            .is_some_and(|session| session.active_run_id.is_some())
            || inner
                .run_admissions
                .contains_key(&ManagedSessionTarget::new(&key.0, session_id.clone()))
        {
            return Err(SessionStoreError::RunConflict(
                "session still has an admitted active run".to_string(),
            ));
        }
        let now = chrono::Utc::now();
        if inner.background_subagents.values().any(|record| {
            record.namespace_id == key.0
                && &record.parent_session_id == session_id
                && !record.execution_status.is_terminal()
                && !record.owner_lease.expired_at(now)
        }) {
            return Err(SessionStoreError::RunConflict(
                "session still has active background-subagent ownership".to_string(),
            ));
        }
        let session = inner
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| SessionStoreError::NotFound(session_id.as_str().to_string()))?;
        if session.revision != expected_revision {
            return Err(SessionStoreError::Conflict(format!(
                "expected revision {expected_revision}, current {}",
                session.revision
            )));
        }
        if !matches!(session.deletion_fence, SessionDeletionFence::Stable) {
            return Err(SessionStoreError::Conflict(
                "session already has a deletion fence".to_string(),
            ));
        }
        session.deletion_fence = SessionDeletionFence::Deleting {
            fence_id: fence_id.to_string(),
            expected_revision,
            requested_by: requested_by.to_string(),
            started_at: chrono::Utc::now(),
        };
        session.revision = session.revision.saturating_add(1);
        session.updated_at = chrono::Utc::now();
        let result = session.clone();
        inner
            .session_idempotency
            .insert(key, (command_fingerprint.to_string(), result.clone()));
        Ok(result)
    }

    async fn tombstone_session(
        &self,
        session_id: &SessionId,
        fence_id: &str,
    ) -> SessionStoreResult<SessionRecord> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        if inner
            .runs
            .values()
            .any(|run| &run.session_id == session_id && run.status.is_active())
        {
            return Err(SessionStoreError::RunConflict(
                "session still has an active run".to_string(),
            ));
        }
        let now = chrono::Utc::now();
        if inner.background_subagents.values().any(|record| {
            &record.parent_session_id == session_id
                && !record.execution_status.is_terminal()
                && !record.owner_lease.expired_at(now)
        }) {
            return Err(SessionStoreError::RunConflict(
                "session still has active background-subagent ownership".to_string(),
            ));
        }
        let session = inner
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| SessionStoreError::NotFound(session_id.as_str().to_string()))?;
        match &session.deletion_fence {
            SessionDeletionFence::Deleted {
                fence_id: current, ..
            } if current == fence_id => {
                return Ok(session.clone());
            }
            SessionDeletionFence::Deleting {
                fence_id: current, ..
            } if current == fence_id => {}
            _ => {
                return Err(SessionStoreError::Conflict(
                    "deletion fence mismatch".to_string(),
                ));
            }
        }
        session.status = SessionStatus::Deleted;
        session.active_run_id = None;
        session.deletion_fence = SessionDeletionFence::Deleted {
            fence_id: fence_id.to_string(),
            deleted_at: chrono::Utc::now(),
        };
        session.revision = session.revision.saturating_add(1);
        session.updated_at = chrono::Utc::now();
        Ok(session.clone())
    }

    async fn session_continuation_fence(
        &self,
        namespace_id: &str,
        session_id: &SessionId,
    ) -> SessionStoreResult<SessionContinuationFence> {
        let inner = self.inner.lock().map_err(store_failed)?;
        let session = inner
            .sessions
            .get(session_id)
            .filter(|session| session.namespace_id == namespace_id)
            .ok_or_else(|| SessionStoreError::NotFound(session_id.as_str().to_string()))?;
        let fence_id = match &session.deletion_fence {
            SessionDeletionFence::Stable => None,
            SessionDeletionFence::Deleting { fence_id, .. }
            | SessionDeletionFence::Deleted { fence_id, .. } => Some(fence_id.clone()),
        };
        Ok(SessionContinuationFence {
            target: ManagedSessionTarget::new(namespace_id, session_id.clone()),
            revision: session.revision,
            continuation_allowed: !session.deletion_fence.blocks_continuation()
                && session.status != SessionStatus::Deleted,
            fence_id,
        })
    }

    async fn acquire_run_admission(
        &self,
        request: AcquireRunAdmission,
    ) -> SessionStoreResult<RunAdmissionReceipt> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let mut staged = inner.clone();
        let receipt = acquire_run_admission_locked(&mut staged, request)?;
        *inner = staged;
        Ok(receipt)
    }

    async fn load_run_admission_receipt(
        &self,
        namespace_id: &str,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> SessionStoreResult<Option<RunAdmissionReceipt>> {
        let inner = self.inner.lock().map_err(store_failed)?;
        let Some((fingerprint, receipt)) = inner
            .run_admission_idempotency
            .get(&(namespace_id.to_string(), idempotency_key.to_string()))
        else {
            return Ok(None);
        };
        if fingerprint != command_fingerprint {
            return Err(SessionStoreError::IdempotencyConflict(
                idempotency_key.to_string(),
            ));
        }
        let mut receipt = receipt.clone();
        receipt.idempotent_replay = true;
        Ok(Some(receipt))
    }

    async fn heartbeat_run_admission(
        &self,
        lease: &RunAdmissionLease,
        lease_expires_at: chrono::DateTime<chrono::Utc>,
    ) -> SessionStoreResult<RunAdmissionLease> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let key = ManagedSessionTarget::new(
            lease.target.namespace_id.clone(),
            lease.target.session_id.clone(),
        );
        let current = inner
            .run_admissions
            .get_mut(&key)
            .ok_or_else(|| SessionStoreError::NotFound(lease.admission_id.clone()))?;
        if current.admission_id != lease.admission_id
            || current.host_instance_id != lease.host_instance_id
            || current.fencing_generation != lease.fencing_generation
            || current.target != lease.target
            || current.expired_at(chrono::Utc::now())
        {
            return Err(SessionStoreError::Conflict(
                "stale admission owner".to_string(),
            ));
        }
        current.heartbeat_at = chrono::Utc::now();
        current.lease_expires_at = lease_expires_at;
        Ok(current.clone())
    }

    async fn release_run_admission(&self, lease: &RunAdmissionLease) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let key = ManagedSessionTarget::new(
            lease.target.namespace_id.clone(),
            lease.target.session_id.clone(),
        );
        if let Some(current) = inner.run_admissions.get(&key) {
            if current.admission_id != lease.admission_id
                || current.host_instance_id != lease.host_instance_id
                || current.fencing_generation != lease.fencing_generation
                || current.target != lease.target
                || current.expired_at(chrono::Utc::now())
            {
                return Err(SessionStoreError::Conflict(
                    "stale admission owner".to_string(),
                ));
            }
            inner.run_admissions.remove(&key);
        }
        Ok(())
    }

    async fn update_run_status_fenced(
        &self,
        lease: &RunAdmissionLease,
        status: RunStatus,
        output_preview: Option<String>,
    ) -> SessionStoreResult<RunRecord> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        ensure_active_admission_locked(&inner, lease, chrono::Utc::now())?;
        apply_run_status_locked(
            &mut inner,
            &lease.target.session_id,
            &lease.target.run_id,
            status,
            output_preview,
            chrono::Utc::now(),
        )
    }

    async fn finalize_run_admission(
        &self,
        lease: &RunAdmissionLease,
        status: RunStatus,
        output_preview: Option<String>,
    ) -> SessionStoreResult<RunRecord> {
        if status.is_active() {
            return Err(SessionStoreError::Conflict(
                "run admission can only finalize to a non-active status".to_string(),
            ));
        }
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let session_key = ManagedSessionTarget::new(
            lease.target.namespace_id.clone(),
            lease.target.session_id.clone(),
        );
        if !inner.run_admissions.contains_key(&session_key) {
            let run = inner
                .runs
                .get(&run_key(&lease.target.session_id, &lease.target.run_id))
                .cloned()
                .ok_or_else(|| {
                    SessionStoreError::NotFound(run_key_label(
                        &lease.target.session_id,
                        &lease.target.run_id,
                    ))
                })?;
            if run.status == status && run.output_preview == output_preview {
                return Ok(run);
            }
            return Err(SessionStoreError::Conflict(
                "stale admission owner".to_string(),
            ));
        }
        ensure_active_admission_locked(&inner, lease, chrono::Utc::now())?;
        let target_key = run_key(&lease.target.session_id, &lease.target.run_id);
        let committed = inner.runs.get(&target_key).cloned().ok_or_else(|| {
            SessionStoreError::NotFound(run_key_label(
                &lease.target.session_id,
                &lease.target.run_id,
            ))
        })?;
        let run = if committed.status.is_terminal() {
            // Complete run evidence may be committed before its admission lease is released.
            // Cleanup owns only the matching lease and must never replace that evidence with a
            // process-local fallback outcome.
            committed
        } else {
            apply_run_status_locked(
                &mut inner,
                &lease.target.session_id,
                &lease.target.run_id,
                status,
                output_preview,
                chrono::Utc::now(),
            )?
        };
        inner.run_admissions.remove(&session_key);
        Ok(run)
    }

    async fn load_run_admission(
        &self,
        target: &ManagedRunTarget,
    ) -> SessionStoreResult<Option<RunAdmissionLease>> {
        let inner = self.inner.lock().map_err(store_failed)?;
        Ok(inner
            .run_admissions
            .get(&ManagedSessionTarget::new(
                target.namespace_id.clone(),
                target.session_id.clone(),
            ))
            .filter(|lease| &lease.target == target)
            .cloned())
    }

    async fn reconcile_expired_run_admissions(
        &self,
        namespace_id: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> SessionStoreResult<Vec<ManagedRunTarget>> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let expired = inner
            .run_admissions
            .iter()
            .filter(|(session, lease)| {
                session.namespace_id == namespace_id && lease.expired_at(now)
            })
            .map(|(session, lease)| (session.clone(), lease.target.clone()))
            .collect::<Vec<_>>();
        for (session_target, target) in &expired {
            if let Some(run) = inner
                .runs
                .get_mut(&run_key(&target.session_id, &target.run_id))
                && run.status.is_active()
            {
                run.status = RunStatus::Cancelled;
                run.output_preview = Some("interrupted after host lease expired".to_string());
                run.updated_at = now;
            }
            if let Some(session) = inner.sessions.get_mut(&target.session_id) {
                if session.active_run_id.as_ref() == Some(&target.run_id) {
                    session.active_run_id = None;
                }
                session.revision = session.revision.saturating_add(1);
                session.updated_at = now;
            }
            inner.run_admissions.remove(session_target);
        }
        Ok(expired.into_iter().map(|(_, target)| target).collect())
    }

    async fn load_control_receipt(
        &self,
        target: &ManagedRunTarget,
        idempotency_key: &str,
    ) -> SessionStoreResult<Option<DurableControlReceipt>> {
        let inner = self.inner.lock().map_err(store_failed)?;
        let key = (target.clone(), idempotency_key.to_string());
        Ok(inner
            .control_idempotency
            .get(&key)
            .and_then(|receipt_id| inner.control_receipts.get(receipt_id))
            .cloned())
    }

    async fn reserve_control_receipt(
        &self,
        receipt: DurableControlReceipt,
    ) -> SessionStoreResult<DurableControlReceipt> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let key = (receipt.target.clone(), receipt.idempotency_key.clone());
        if let Some(receipt_id) = inner.control_idempotency.get(&key)
            && let Some(existing) = inner.control_receipts.get(receipt_id)
        {
            if existing.command_fingerprint == receipt.command_fingerprint {
                return Ok(existing.clone());
            }
            return Err(SessionStoreError::IdempotencyConflict(
                receipt.idempotency_key,
            ));
        }
        let session_key = ManagedSessionTarget::new(
            receipt.target.namespace_id.clone(),
            receipt.target.session_id.clone(),
        );
        let lease = inner.run_admissions.get(&session_key).ok_or_else(|| {
            SessionStoreError::Conflict("run has no active owner lease".to_string())
        })?;
        if lease.target != receipt.target || lease.fencing_generation != receipt.fencing_generation
        {
            return Err(SessionStoreError::Conflict(
                "stale control generation".to_string(),
            ));
        }
        inner
            .control_idempotency
            .insert(key, receipt.receipt_id.clone());
        inner
            .control_receipts
            .insert(receipt.receipt_id.clone(), receipt.clone());
        Ok(receipt)
    }

    async fn update_control_receipt_state(
        &self,
        receipt_id: &str,
        state: &str,
    ) -> SessionStoreResult<DurableControlReceipt> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let receipt = inner
            .control_receipts
            .get_mut(receipt_id)
            .ok_or_else(|| SessionStoreError::NotFound(receipt_id.to_string()))?;
        receipt.state = state.to_string();
        Ok(receipt.clone())
    }

    async fn drain_background_subagent_operations(&self) -> SessionStoreResult<()> {
        Ok(())
    }

    async fn record_background_subagent_acceptance(
        &self,
        record: BackgroundSubagentRecord,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        if !record.is_valid_acceptance() {
            return Err(SessionStoreError::Conflict(
                "invalid background-subagent acceptance record".to_string(),
            ));
        }
        let mut inner = self.inner.lock().map_err(store_failed)?;
        if let Some(existing) = inner.background_subagents.get(&record.attempt_id) {
            return if same_background_identity(existing, &record) {
                Ok(existing.clone())
            } else {
                Err(SessionStoreError::Conflict(format!(
                    "background attempt {} already exists with different identity",
                    record.attempt_id.as_str()
                )))
            };
        }
        if record.owner_lease.expired_at(chrono::Utc::now()) {
            return Err(SessionStoreError::Conflict(
                "background-subagent acceptance owner lease is already expired".to_string(),
            ));
        }
        let session = inner
            .sessions
            .get(&record.parent_session_id)
            .filter(|session| session.namespace_id == record.namespace_id)
            .ok_or_else(|| {
                SessionStoreError::NotFound(record.parent_session_id.as_str().to_string())
            })?;
        if session.status != SessionStatus::Active || session.deletion_fence.blocks_continuation() {
            return Err(SessionStoreError::Conflict(
                "session cannot admit background delegation".to_string(),
            ));
        }
        if !inner
            .runs
            .contains_key(&run_key(&record.parent_session_id, &record.parent_run_id))
        {
            return Err(SessionStoreError::NotFound(format!(
                "{}:{}",
                record.parent_session_id.as_str(),
                record.parent_run_id.as_str()
            )));
        }
        if inner.background_subagents.values().any(|existing| {
            existing.namespace_id == record.namespace_id
                && existing.parent_session_id == record.parent_session_id
                && existing.agent_id == record.agent_id
                && !existing.execution_status.is_terminal()
        }) {
            return Err(SessionStoreError::Conflict(format!(
                "background agent {} already has an active durable attempt",
                record.agent_id
            )));
        }
        inner
            .background_subagents
            .insert(record.attempt_id.clone(), record.clone());
        Ok(record)
    }

    async fn update_background_subagent_execution(
        &self,
        mut record: BackgroundSubagentRecord,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let current = inner
            .background_subagents
            .get(&record.attempt_id)
            .ok_or_else(|| SessionStoreError::NotFound(record.attempt_id.as_str().to_string()))?;
        ensure_background_parent_writable(&inner, current)?;
        if current.owner_lease.expired_at(chrono::Utc::now())
            || !same_background_identity(current, &record)
            || !same_background_owner(current, &record)
            || current.execution_status.is_terminal()
            || record.execution_status.is_terminal()
            || !valid_background_transition(current.execution_status, record.execution_status)
            || !same_background_non_execution_state(current, &record)
        {
            return Err(SessionStoreError::Conflict(format!(
                "invalid background execution transition for {}",
                record.attempt_id.as_str()
            )));
        }
        record.owner_lease = current.owner_lease.clone();
        inner
            .background_subagents
            .insert(record.attempt_id.clone(), record.clone());
        Ok(record)
    }

    async fn heartbeat_background_subagent(
        &self,
        attempt_id: &starweaver_core::SubagentAttemptId,
        host_instance_id: &str,
        fencing_generation: u64,
        lease_expires_at: chrono::DateTime<chrono::Utc>,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let now = chrono::Utc::now();
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let identity = inner
            .background_subagents
            .get(attempt_id)
            .cloned()
            .ok_or_else(|| SessionStoreError::NotFound(attempt_id.as_str().to_string()))?;
        ensure_background_parent_writable(&inner, &identity)?;
        let record = inner
            .background_subagents
            .get_mut(attempt_id)
            .ok_or_else(|| SessionStoreError::NotFound(attempt_id.as_str().to_string()))?;
        if record.execution_status.is_terminal()
            || record.owner_lease.expired_at(now)
            || record.owner_lease.host_instance_id != host_instance_id
            || record.owner_lease.fencing_generation != fencing_generation
            || lease_expires_at <= now
        {
            return Err(SessionStoreError::Conflict(
                "stale or invalid background-subagent owner heartbeat".to_string(),
            ));
        }
        record.owner_lease.heartbeat_at = now;
        record.owner_lease.lease_expires_at = lease_expires_at;
        Ok(record.clone())
    }

    async fn commit_background_subagent_terminal(
        &self,
        commit: BackgroundSubagentTerminalCommit,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        commit_background_terminal(
            &mut inner,
            commit.record,
            commit.artifact,
            commit.artifact_limits,
        )
    }

    async fn load_background_subagent_artifact(
        &self,
        artifact_ref: &str,
    ) -> SessionStoreResult<BackgroundSubagentArtifact> {
        let artifact = self
            .inner
            .lock()
            .map_err(store_failed)?
            .background_artifacts
            .get(artifact_ref)
            .cloned()
            .ok_or_else(|| SessionStoreError::NotFound(artifact_ref.to_string()))?;
        if artifact.expires_at <= chrono::Utc::now() {
            return Err(SessionStoreError::NotFound(artifact_ref.to_string()));
        }
        if !artifact.is_valid() {
            return Err(SessionStoreError::Conflict(
                "background-subagent artifact failed integrity validation".to_string(),
            ));
        }
        Ok(artifact)
    }

    async fn expire_background_subagent_retention(
        &self,
        namespace_id: &str,
        now: chrono::DateTime<chrono::Utc>,
        limit: usize,
    ) -> SessionStoreResult<Vec<BackgroundSubagentRecord>> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let mut attempt_ids = inner
            .background_subagents
            .values()
            .filter(|record| {
                record.namespace_id == namespace_id
                    && record.execution_status.is_terminal()
                    && record.retention_status
                        != crate::DurableBackgroundSubagentRetentionStatus::Expired
                    && record
                        .retention_expires_at
                        .is_some_and(|deadline| deadline <= now)
            })
            .map(|record| (record.retention_expires_at, record.attempt_id.clone()))
            .collect::<Vec<_>>();
        attempt_ids.sort();
        attempt_ids.truncate(limit);
        let mut expired = Vec::with_capacity(attempt_ids.len());
        let mut artifact_refs = Vec::new();
        for (_, attempt_id) in attempt_ids {
            if !inner
                .background_terminal_fingerprints
                .contains_key(&attempt_id)
                && let Some(record) = inner.background_subagents.get(&attempt_id).cloned()
                && let Some(fingerprint) =
                    reconstruct_background_terminal_fingerprint(&inner, &record)?
            {
                inner
                    .background_terminal_fingerprints
                    .insert(attempt_id.clone(), fingerprint);
            }
            let Some(record) = inner.background_subagents.get_mut(&attempt_id) else {
                continue;
            };
            if let Some(result_ref) = record.result_ref.as_mut() {
                if let Some(artifact_ref) = result_ref.artifact_ref.take() {
                    artifact_refs.push(artifact_ref);
                }
                result_ref.content = None;
                result_ref.error = None;
            }
            record.retention_status = crate::DurableBackgroundSubagentRetentionStatus::Expired;
            record.retention_expires_at = None;
            expired.push(record.clone());
        }
        for artifact_ref in artifact_refs {
            inner.background_artifacts.remove(&artifact_ref);
        }
        Ok(expired)
    }

    async fn record_background_subagent_terminal(
        &self,
        record: BackgroundSubagentRecord,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        commit_background_terminal(&mut inner, record, None, None)
    }

    async fn load_background_subagent(
        &self,
        attempt_id: &starweaver_core::SubagentAttemptId,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        self.inner
            .lock()
            .map_err(store_failed)?
            .background_subagents
            .get(attempt_id)
            .cloned()
            .ok_or_else(|| SessionStoreError::NotFound(attempt_id.as_str().to_string()))
    }

    async fn list_background_subagents(
        &self,
        namespace_id: &str,
        session_id: Option<&SessionId>,
        limit: usize,
    ) -> SessionStoreResult<Vec<BackgroundSubagentRecord>> {
        let inner = self.inner.lock().map_err(store_failed)?;
        let mut records = inner
            .background_subagents
            .values()
            .filter(|record| {
                record.namespace_id == namespace_id
                    && session_id.is_none_or(|session_id| &record.parent_session_id == session_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.attempt_id.cmp(&right.attempt_id))
        });
        records.truncate(limit);
        Ok(records)
    }

    async fn list_pending_background_subagents(
        &self,
        namespace_id: &str,
        session_id: Option<&SessionId>,
        limit: usize,
    ) -> SessionStoreResult<Vec<BackgroundSubagentRecord>> {
        let inner = self.inner.lock().map_err(store_failed)?;
        let mut records = inner
            .background_subagents
            .values()
            .filter(|record| {
                record.namespace_id == namespace_id
                    && session_id.is_none_or(|session_id| &record.parent_session_id == session_id)
                    && record.execution_status.is_terminal()
                    && record.delivery_status != DurableBackgroundSubagentDeliveryStatus::Delivered
            })
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            left.updated_at
                .cmp(&right.updated_at)
                .then_with(|| left.attempt_id.cmp(&right.attempt_id))
        });
        records.truncate(limit);
        Ok(records)
    }

    async fn claim_background_subagent_delivery(
        &self,
        attempt_id: &starweaver_core::SubagentAttemptId,
        claim: DurableBackgroundSubagentDeliveryClaim,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let now = chrono::Utc::now();
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let mut record = inner
            .background_subagents
            .get(attempt_id)
            .cloned()
            .ok_or_else(|| SessionStoreError::NotFound(attempt_id.as_str().to_string()))?;
        if record.delivery_status == DurableBackgroundSubagentDeliveryStatus::Claimed
            && record.delivery_claim.as_ref() == Some(&claim)
        {
            return Ok(record);
        }
        if let Some(current_claim) = record.delivery_claim.clone()
            && record.delivery_status == DurableBackgroundSubagentDeliveryStatus::Claimed
            && current_claim.deadline <= now
        {
            match background_claim_consumer_state(&inner, &record, &current_claim, now) {
                BackgroundClaimConsumerState::Live => {
                    return Err(SessionStoreError::Conflict(
                        "live admitted consumer still owns the background delivery claim"
                            .to_string(),
                    ));
                }
                BackgroundClaimConsumerState::Completed(run_id) => {
                    record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Delivered;
                    record.delivery_claim = None;
                    record.delivered_claim_id = Some(current_claim.claim_id);
                    record.continuation_run_id = Some(run_id);
                    record.automatic_continuation_suppressed_by_run_id = None;
                    record.updated_at = now;
                    inner
                        .background_subagents
                        .insert(attempt_id.clone(), record);
                    return Err(SessionStoreError::Conflict(
                        "completed consumer already delivered the background result".to_string(),
                    ));
                }
                BackgroundClaimConsumerState::Terminated(run_id) => {
                    record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Undelivered;
                    record.delivery_claim = None;
                    record.automatic_continuation_suppressed_by_run_id = Some(run_id);
                    record.updated_at = now;
                    inner
                        .background_subagents
                        .insert(attempt_id.clone(), record);
                    return Err(SessionStoreError::Conflict(
                        "terminated consumer released the background delivery claim".to_string(),
                    ));
                }
                BackgroundClaimConsumerState::Reclaimable => {}
            }
        }
        if !record.delivery_claimable_at(now) {
            return Err(SessionStoreError::Conflict(
                "background result delivery is not claimable".to_string(),
            ));
        }
        record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Claimed;
        record.delivery_claim = Some(claim);
        record.updated_at = now;
        inner
            .background_subagents
            .insert(attempt_id.clone(), record.clone());
        Ok(record)
    }

    async fn acknowledge_background_subagent_delivery(
        &self,
        attempt_id: &starweaver_core::SubagentAttemptId,
        claim_id: &str,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let record = inner
            .background_subagents
            .get_mut(attempt_id)
            .ok_or_else(|| SessionStoreError::NotFound(attempt_id.as_str().to_string()))?;
        if record.delivery_status == DurableBackgroundSubagentDeliveryStatus::Delivered {
            return if record.delivered_claim_id.as_deref() == Some(claim_id) {
                Ok(record.clone())
            } else {
                Err(SessionStoreError::Conflict(
                    "background result was delivered by another claim".to_string(),
                ))
            };
        }
        if record.delivery_status != DurableBackgroundSubagentDeliveryStatus::Claimed
            || record
                .delivery_claim
                .as_ref()
                .is_none_or(|claim| claim.claim_id != claim_id)
        {
            return Err(SessionStoreError::Conflict(
                "background delivery claim mismatch".to_string(),
            ));
        }
        record.continuation_run_id = record
            .delivery_claim
            .as_ref()
            .and_then(|claim| claim.continuation_run_id.clone());
        record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Delivered;
        record.delivery_claim = None;
        record.delivered_claim_id = Some(claim_id.to_string());
        record.automatic_continuation_suppressed_by_run_id = None;
        record.updated_at = chrono::Utc::now();
        Ok(record.clone())
    }

    async fn release_background_subagent_delivery(
        &self,
        attempt_id: &starweaver_core::SubagentAttemptId,
        claim_id: &str,
        release: DurableBackgroundSubagentDeliveryRelease,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let mut record = inner
            .background_subagents
            .get(attempt_id)
            .cloned()
            .ok_or_else(|| SessionStoreError::NotFound(attempt_id.as_str().to_string()))?;
        if record.delivery_status == DurableBackgroundSubagentDeliveryStatus::Undelivered {
            return match release {
                DurableBackgroundSubagentDeliveryRelease::Retryable => Ok(record),
                DurableBackgroundSubagentDeliveryRelease::ConsumerTerminated { .. } => {
                    Err(SessionStoreError::Conflict(
                        "terminated consumer has no matching background delivery claim".to_string(),
                    ))
                }
            };
        }
        if record.delivery_status != DurableBackgroundSubagentDeliveryStatus::Claimed
            || record
                .delivery_claim
                .as_ref()
                .is_none_or(|claim| claim.claim_id != claim_id)
        {
            return Err(SessionStoreError::Conflict(
                "background delivery claim mismatch".to_string(),
            ));
        }
        if let DurableBackgroundSubagentDeliveryRelease::ConsumerTerminated { run_id } = release {
            if record
                .delivery_claim
                .as_ref()
                .and_then(|claim| claim.continuation_run_id.as_ref())
                != Some(&run_id)
                || inner
                    .runs
                    .get(&run_key(&record.parent_session_id, &run_id))
                    .is_none_or(|run| {
                        !matches!(run.status, RunStatus::Failed | RunStatus::Cancelled)
                    })
            {
                return Err(SessionStoreError::Conflict(
                    "terminated consumer does not own the claim or is not terminal".to_string(),
                ));
            }
            record.automatic_continuation_suppressed_by_run_id = Some(run_id);
        }
        record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Undelivered;
        record.delivery_claim = None;
        record.updated_at = chrono::Utc::now();
        inner
            .background_subagents
            .insert(attempt_id.clone(), record.clone());
        Ok(record)
    }

    async fn acquire_background_subagent_continuation(
        &self,
        mut request: AcquireBackgroundSubagentContinuation,
    ) -> SessionStoreResult<BackgroundSubagentContinuationReceipt> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let mut background = inner
            .background_subagents
            .get(&request.attempt_id)
            .cloned()
            .ok_or_else(|| SessionStoreError::NotFound(request.attempt_id.as_str().to_string()))?;
        let continuation_run_id = request.admission.run.run_id.clone();
        if request.attempt_id != request.cause.attempt_id
            || background
                .automatic_continuation_suppressed_by_run_id
                .is_some()
            || !background
                .validates_continuation_cause_envelope(&request.cause, &request.admission.run)
        {
            return Err(SessionStoreError::Conflict(
                "background continuation cause does not match its durable result".to_string(),
            ));
        }
        if background.delivery_status == DurableBackgroundSubagentDeliveryStatus::Delivered
            || background.delivery_status == DurableBackgroundSubagentDeliveryStatus::Claimed
        {
            let same_claim = background.continuation_run_id.as_ref() == Some(&continuation_run_id)
                && (background.delivered_claim_id.as_deref() == Some(&request.claim_id)
                    || background
                        .delivery_claim
                        .as_ref()
                        .is_some_and(|claim| claim.claim_id == request.claim_id));
            if !same_claim {
                return Err(SessionStoreError::Conflict(
                    "background result was already claimed or delivered".to_string(),
                ));
            }
            let key = (
                request.admission.namespace_id.clone(),
                request.admission.idempotency_key.clone(),
            );
            let (fingerprint, admission) = inner
                .run_admission_idempotency
                .get(&key)
                .ok_or_else(|| SessionStoreError::NotFound(request.claim_id.clone()))?;
            if fingerprint != &request.admission.command_fingerprint {
                return Err(SessionStoreError::IdempotencyConflict(
                    request.admission.idempotency_key.clone(),
                ));
            }
            let mut admission = admission.clone();
            admission.idempotent_replay = true;
            if !continuation_receipt_matches_request(&background, &request, &admission) {
                return Err(SessionStoreError::Conflict(
                    "stored continuation receipt does not match the causal request".to_string(),
                ));
            }
            let cause =
                crate::BackgroundSubagentContinuationCause::new(&background, &admission.run.input)
                    .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
            return Ok(BackgroundSubagentContinuationReceipt {
                cause,
                background,
                admission,
            });
        }
        let now = chrono::Utc::now();
        let artifact_content = continuation_artifact_content(&inner, &background, now)?;
        if background.parent_session_id != request.admission.run.session_id
            || background.namespace_id != request.admission.namespace_id
            || !background.validates_continuation_cause(
                &request.cause,
                &request.admission.run,
                artifact_content.as_deref(),
            )
            || request.claim_id.is_empty()
            || request.claim_deadline <= now
            || !background.delivery_claimable_at(now)
        {
            return Err(SessionStoreError::Conflict(
                "background result cannot admit this continuation".to_string(),
            ));
        }
        let session = inner
            .sessions
            .get(&background.parent_session_id)
            .ok_or_else(|| {
                SessionStoreError::NotFound(background.parent_session_id.as_str().to_string())
            })?;
        request
            .admission
            .run
            .restore_from_run_id
            .clone_from(&session.head_run_id);
        let admission_request = request.clone();
        let mut staged = inner.clone();
        let admission = acquire_run_admission_locked(&mut staged, request.admission.clone())?;
        if !continuation_receipt_matches_request(&background, &admission_request, &admission) {
            return Err(SessionStoreError::Conflict(
                "admitted continuation receipt lost causal input binding".to_string(),
            ));
        }
        background.delivery_status = DurableBackgroundSubagentDeliveryStatus::Delivered;
        background.delivery_claim = None;
        background.delivered_claim_id = Some(request.claim_id);
        background.continuation_run_id = Some(admission.run.run_id.clone());
        background.updated_at = chrono::Utc::now();
        staged
            .background_subagents
            .insert(background.attempt_id.clone(), background.clone());
        let cause =
            crate::BackgroundSubagentContinuationCause::new(&background, &admission.run.input)
                .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        *inner = staged;
        Ok(BackgroundSubagentContinuationReceipt {
            cause,
            background,
            admission,
        })
    }

    async fn reconcile_background_subagents(
        &self,
        namespace_id: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> SessionStoreResult<Vec<BackgroundSubagentRecord>> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let attempt_ids = inner
            .background_subagents
            .values()
            .filter(|record| record.namespace_id == namespace_id)
            .map(|record| record.attempt_id.clone())
            .collect::<Vec<_>>();
        let mut changed = Vec::new();
        for attempt_id in attempt_ids {
            let Some(mut record) = inner.background_subagents.get(&attempt_id).cloned() else {
                continue;
            };
            let mut terminal_fingerprint = None;
            if !record.execution_status.is_terminal() && record.owner_lease.expired_at(now) {
                let error = "in-process background execution was interrupted by host restart";
                record.execution_status = DurableBackgroundSubagentExecutionStatus::Failed;
                record.failure_category = Some("host_process_lost".to_string());
                record.result_ref = Some(DurableBackgroundSubagentResultRef {
                    error: Some(error.to_string()),
                    size_bytes: u64::try_from(error.len()).unwrap_or(u64::MAX),
                    ..DurableBackgroundSubagentResultRef::default()
                });
                record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Undelivered;
                record.delivery_claim = None;
                record.retention_expires_at = Some(
                    now + chrono::Duration::seconds(
                        crate::DEFAULT_BACKGROUND_RESULT_RETENTION_SECS,
                    ),
                );
                record.terminal_at = Some(now);
                record.updated_at = now;
                terminal_fingerprint = Some(background_terminal_fingerprint(&record, None)?);
            } else if record.delivery_status == DurableBackgroundSubagentDeliveryStatus::Claimed
                && record
                    .delivery_claim
                    .as_ref()
                    .is_some_and(|claim| claim.deadline <= now)
            {
                let claim = record.delivery_claim.clone().ok_or_else(|| {
                    SessionStoreError::Conflict(
                        "claimed background result is missing its claim".to_string(),
                    )
                })?;
                let consumer = claim.continuation_run_id.as_ref().and_then(|run_id| {
                    inner
                        .runs
                        .get(&run_key(&record.parent_session_id, run_id))
                        .map(|run| (run_id, run.status))
                });
                let consumer_has_live_admission = consumer.is_some_and(|(run_id, status)| {
                    status.is_active()
                        && inner
                            .run_admissions
                            .get(&ManagedSessionTarget::new(
                                record.namespace_id.clone(),
                                record.parent_session_id.clone(),
                            ))
                            .is_some_and(|lease| {
                                lease.target.run_id == *run_id && !lease.expired_at(now)
                            })
                });
                if consumer_has_live_admission {
                    continue;
                }
                if consumer.is_some_and(|(_, status)| status == RunStatus::Completed) {
                    record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Delivered;
                    record
                        .continuation_run_id
                        .clone_from(&claim.continuation_run_id);
                    record.delivered_claim_id = Some(claim.claim_id);
                    record.automatic_continuation_suppressed_by_run_id = None;
                } else {
                    record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Undelivered;
                    if let Some((run_id, status)) = consumer
                        && matches!(status, RunStatus::Failed | RunStatus::Cancelled)
                    {
                        record.automatic_continuation_suppressed_by_run_id = Some(run_id.clone());
                    }
                }
                record.delivery_claim = None;
                record.updated_at = now;
            } else {
                continue;
            }
            if let Some(fingerprint) = terminal_fingerprint {
                inner
                    .background_terminal_fingerprints
                    .insert(attempt_id.clone(), fingerprint);
            }
            inner
                .background_subagents
                .insert(attempt_id, record.clone());
            changed.push(record);
        }
        Ok(changed)
    }

    async fn save_session(&self, session: SessionRecord) -> SessionStoreResult<()> {
        self.save_session_record(session)
    }

    async fn load_session(&self, session_id: &SessionId) -> SessionStoreResult<SessionRecord> {
        self.load_session_record(session_id)
    }

    async fn list_sessions(&self, filter: SessionFilter) -> SessionStoreResult<Vec<SessionRecord>> {
        self.list_session_records(filter)
    }

    async fn update_session_status(
        &self,
        session_id: &SessionId,
        status: SessionStatus,
    ) -> SessionStoreResult<()> {
        self.set_session_status(session_id, status)
    }

    async fn save_context_state(
        &self,
        session_id: &SessionId,
        state: ResumableState,
    ) -> SessionStoreResult<()> {
        self.save_context_state_snapshot(session_id, state)
    }

    async fn save_environment_state(
        &self,
        session_id: &SessionId,
        environment_state: EnvironmentStateRef,
    ) -> SessionStoreResult<()> {
        self.save_environment_state_ref(session_id, environment_state)
    }

    async fn append_run(&self, run: RunRecord) -> SessionStoreResult<()> {
        self.append_run_record(run)
    }

    async fn load_run(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<RunRecord> {
        self.load_run_record(session_id, run_id)
    }

    async fn list_runs(&self, session_id: &SessionId) -> SessionStoreResult<Vec<RunRecord>> {
        self.list_run_records(session_id)
    }

    async fn update_run_status(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        status: RunStatus,
        output_preview: Option<String>,
    ) -> SessionStoreResult<()> {
        self.set_run_status(session_id, run_id, status, output_preview)
    }

    async fn append_checkpoint(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()> {
        self.append_checkpoint_record(session_id, checkpoint)
    }

    async fn load_checkpoints(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentCheckpoint>> {
        self.load_checkpoint_records(session_id, run_id)
    }

    async fn append_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        records: Vec<AgentStreamRecord>,
    ) -> SessionStoreResult<()> {
        self.append_stream_record_batch(session_id, run_id, records)
    }

    async fn replay_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentStreamRecord>> {
        self.replay_stream_record_batch(session_id, run_id)
    }

    async fn save_stream_cursor(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        cursor: StreamCursorRef,
    ) -> SessionStoreResult<()> {
        self.save_stream_cursor_ref(session_id, run_id, cursor)
    }

    async fn append_approval(&self, approval: ApprovalRecord) -> SessionStoreResult<()> {
        self.append_approval_record(approval)
    }

    async fn load_approvals(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<ApprovalRecord>> {
        self.load_approval_records(session_id, run_id)
    }

    async fn append_deferred_tool(&self, record: DeferredToolRecord) -> SessionStoreResult<()> {
        self.append_deferred_tool_record(record)
    }

    async fn load_deferred_tools(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<DeferredToolRecord>> {
        self.load_deferred_tool_records(session_id, run_id)
    }

    async fn resume_snapshot(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<SessionResumeSnapshot> {
        let session = self.load_session(session_id).await?;
        let run = self.load_run(session_id, run_id).await?;
        let state = {
            let inner = self.inner.lock().map_err(store_failed)?;
            inner
                .evidence_commits
                .get(&run_key(session_id, run_id))
                .map_or_else(
                    || session.state.clone(),
                    |commit| commit.context_state.clone(),
                )
        };
        let latest_checkpoint = self.latest_checkpoint(session_id, run_id).await?;
        let after_sequence = latest_checkpoint
            .as_ref()
            .and_then(|checkpoint| checkpoint.resume.cursor.stream_cursor);
        let stream_records = self
            .replay_stream_records_after(session_id, run_id, after_sequence)
            .await?;
        let approvals = self.load_approvals(session_id, run_id).await?;
        let deferred_tools = self.load_deferred_tools(session_id, run_id).await?;
        let environment_state = run
            .environment_state
            .clone()
            .or_else(|| session.environment_state.clone());
        let mut stream_cursors = session.stream_cursors.clone();
        stream_cursors.extend(run.stream_cursors.clone());
        Ok(SessionResumeSnapshot {
            session,
            run,
            state,
            environment_state,
            latest_checkpoint,
            stream_records,
            approvals,
            deferred_tools,
            stream_cursors,
        })
    }

    async fn compact_run_trace(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<CompactRunTrace> {
        self.compact_run_trace_projection(session_id, run_id)
    }

    async fn compact_session_trace(
        &self,
        session_id: &SessionId,
    ) -> SessionStoreResult<CompactSessionTrace> {
        self.compact_session_trace_projection(session_id)
    }
}
