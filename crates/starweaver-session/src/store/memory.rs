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
use starweaver_stream::AgentStreamRecord;

use crate::{
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
    approvals: BTreeMap<(SessionId, RunId), Vec<ApprovalRecord>>,
    deferred_tools: BTreeMap<(SessionId, RunId), Vec<DeferredToolRecord>>,
    evidence_commits: BTreeMap<(SessionId, RunId), RunEvidenceCommit>,
    evidence_digests: BTreeMap<(SessionId, RunId), String>,
    hitl_resume_claims: BTreeMap<(SessionId, RunId), HitlResumeClaim>,
    stream_publication_outbox: BTreeMap<String, PendingStreamPublication>,
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
