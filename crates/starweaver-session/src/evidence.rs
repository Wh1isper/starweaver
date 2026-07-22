//! Atomic durable evidence committed for one agent run.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use starweaver_context::{AgentCheckpoint, ResumableState};
use starweaver_core::RunId;
use starweaver_stream::{
    AgentStreamRecord, DisplayMessage, ReplayCursorFamily, ReplayEvent, ReplayScope, ReplaySnapshot,
};

use crate::{
    ApprovalRecord, DeferredToolRecord, PendingHostEventPublication, RunRecord, RunStatus,
    RunTerminalError, RunTerminalProjection, SessionStoreError, SessionStoreResult,
    StreamCursorRef, StreamPublicationTargets,
};

/// Atomic transition applied to an existing run together with a new run evidence commit.
///
/// This supports durable continuations: resolved HITL records and the source run's terminal state
/// become visible in the same transaction as the continuation run. `expected_status` provides an
/// optimistic concurrency guard and prevents two continuations from consuming one waiting run.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RelatedRunUpdate {
    /// Existing run to update. It must belong to the evidence commit's session.
    pub run_id: RunId,
    /// Status that must be present before the transition.
    pub expected_status: RunStatus,
    /// Terminal status written by the transition.
    pub status: RunStatus,
    /// Exclusive resume claim that authorizes this transition.
    pub resume_claim_id: Option<String>,
    /// Optional source-run output preview.
    pub output_preview: Option<String>,
    /// Safe source-run terminal diagnostic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_error: Option<RunTerminalError>,
    /// Resolved approval records owned by the source run.
    pub approvals: Vec<ApprovalRecord>,
    /// Resolved deferred-tool records owned by the source run.
    pub deferred_tools: Vec<DeferredToolRecord>,
}

impl RelatedRunUpdate {
    /// Build a guarded source-run transition with no HITL records.
    #[must_use]
    pub const fn new(run_id: RunId, expected_status: RunStatus, status: RunStatus) -> Self {
        Self {
            run_id,
            expected_status,
            status,
            resume_claim_id: None,
            output_preview: None,
            terminal_error: None,
            approvals: Vec::new(),
            deferred_tools: Vec::new(),
        }
    }
}

/// Complete product-neutral durable evidence for one run.
///
/// A [`crate::SessionStore`] implementation must either persist all fields in this value or leave
/// its previous state unchanged. Repeating an identical commit is idempotent; reusing an evidence
/// identity with a different payload is an error.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RunEvidenceCommit {
    /// Final or checkpointed run record.
    pub run: RunRecord,
    /// Per-run resumable context state.
    pub context_state: ResumableState,
    /// Optional product-neutral serialized environment state.
    pub environment_state: Option<Value>,
    /// Raw runtime stream records.
    pub stream_records: Vec<AgentStreamRecord>,
    /// Full runtime checkpoints. Legacy checkpoint references are not accepted here.
    pub checkpoints: Vec<AgentCheckpoint>,
    /// Approval evidence created by the run.
    pub approvals: Vec<ApprovalRecord>,
    /// Deferred tool evidence created by the run.
    pub deferred_tools: Vec<DeferredToolRecord>,
    /// Durable stream cursors selected by the caller.
    pub stream_cursors: Vec<StreamCursorRef>,
    /// Display messages owned by this run's replay scope.
    pub display_messages: Vec<DisplayMessage>,
    /// Replay events, including terminal markers, owned by this run's replay scope.
    pub replay_events: Vec<ReplayEvent>,
    /// Optional compact display snapshot for this run's replay scope.
    pub display_snapshot: Option<ReplaySnapshot>,
    /// External sink families transactionally enqueued with this evidence.
    pub publication_targets: StreamPublicationTargets,
    /// View-independent host events enqueued atomically with this evidence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_event_publications: Vec<PendingHostEventPublication>,
    /// Existing runs transitioned atomically with this commit.
    pub related_run_updates: Vec<RelatedRunUpdate>,
}

impl RunEvidenceCommit {
    /// Reserved legacy metadata key rejected to prevent caller-forged evidence digests.
    ///
    /// Stores persist trusted digests independently from caller-controlled run metadata.
    pub const DIGEST_METADATA_KEY: &'static str = "starweaver.run_evidence_sha256";

    /// Build a run-evidence commit with no optional evidence.
    #[must_use]
    pub const fn new(run: RunRecord, context_state: ResumableState) -> Self {
        Self {
            run,
            context_state,
            environment_state: None,
            stream_records: Vec::new(),
            checkpoints: Vec::new(),
            approvals: Vec::new(),
            deferred_tools: Vec::new(),
            stream_cursors: Vec::new(),
            display_messages: Vec::new(),
            replay_events: Vec::new(),
            display_snapshot: None,
            publication_targets: StreamPublicationTargets::new(false, false),
            host_event_publications: Vec::new(),
            related_run_updates: Vec::new(),
        }
    }

    /// Validate identities, terminal projections, cursor scopes, sequence uniqueness, snapshots,
    /// and environment shape for a new evidence write.
    ///
    /// # Errors
    ///
    /// Returns a store error when evidence members cannot belong to one atomic run commit.
    pub fn validate(&self) -> SessionStoreResult<()> {
        self.validate_structure()?;
        self.validate_terminal_projections()
    }

    /// Validate the legacy-compatible structure needed before exact-retry lookup.
    ///
    /// This method does not authorize a new write: stores must call
    /// [`Self::validate_terminal_projections`] after an exact-retry miss.
    ///
    /// # Errors
    ///
    /// Returns a store error when evidence members cannot safely identify one atomic commit.
    pub fn validate_structure(&self) -> SessionStoreResult<()> {
        validate_primary_identity(self)?;
        validate_stream_evidence(self)?;
        validate_host_event_publications(self)?;
        validate_related_run_evidence(self)
    }

    /// Validate terminal projections before inserting new evidence.
    ///
    /// Kept separate from structural validation so an exact retry of legacy evidence committed
    /// before terminal diagnostics existed remains idempotent across upgrades.
    ///
    /// # Errors
    ///
    /// Returns a store error when primary or related terminal evidence is incomplete.
    pub fn validate_terminal_projections(&self) -> SessionStoreResult<()> {
        validate_primary_terminal_projection(self)?;
        validate_related_terminal_projections(self)
    }

    /// Return the canonical SHA-256 digest for exact-retry conflict detection.
    ///
    /// # Errors
    ///
    /// Returns a store error when the bundle cannot be serialized.
    pub fn digest(&self) -> SessionStoreResult<String> {
        let payload = serde_json::to_vec(self)
            .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        Ok(format!("{:x}", Sha256::digest(payload)))
    }
}

fn validate_primary_terminal_projection(commit: &RunEvidenceCommit) -> SessionStoreResult<()> {
    commit.run.validate_new_write().map_err(|error| {
        SessionStoreError::Failed(format!(
            "invalid terminal evidence for run {}: {error}",
            commit.run.run_id.as_str()
        ))
    })
}

fn validate_primary_identity(commit: &RunEvidenceCommit) -> SessionStoreResult<()> {
    let session_id = &commit.run.session_id;
    let run_id = &commit.run.run_id;
    if commit
        .run
        .metadata
        .contains_key(RunEvidenceCommit::DIGEST_METADATA_KEY)
    {
        return Err(SessionStoreError::Failed(format!(
            "reserved run metadata key {} cannot be supplied by callers",
            RunEvidenceCommit::DIGEST_METADATA_KEY
        )));
    }
    let scope = ReplayScope::run(run_id.as_str());
    // AgentContext.session_id is provider-routing affinity, and its run_id is
    // runtime execution state. Durable store identity belongs to RunRecord and
    // the canonical metadata keys, so those context fields must not be equated
    // with product persistence IDs.
    let context_binding_matches = commit
        .context_state
        .conversation_id
        .as_ref()
        .is_none_or(|conversation_id| conversation_id == &commit.run.conversation_id)
        && optional_metadata_id_matches(
            &commit.context_state.metadata,
            "starweaver.durable_session_id",
            session_id.as_str(),
        )
        && optional_metadata_id_matches(
            &commit.context_state.metadata,
            "starweaver.durable_run_id",
            run_id.as_str(),
        );
    let identity_mismatch = !context_binding_matches
        || commit.checkpoints.iter().any(|checkpoint| {
            checkpoint.run_id != *run_id
                || checkpoint.conversation_id != commit.run.conversation_id
                || checkpoint.state.run_id != *run_id
                || checkpoint.state.conversation_id != commit.run.conversation_id
                || checkpoint.node != checkpoint.resume.node
                || checkpoint.run_step != checkpoint.resume.run_step
                || checkpoint.run_step != checkpoint.state.run_step
        })
        || commit
            .approvals
            .iter()
            .any(|record| record.session_id != *session_id || record.run_id != *run_id)
        || commit
            .deferred_tools
            .iter()
            .any(|record| record.session_id != *session_id || record.run_id != *run_id)
        || commit
            .display_messages
            .iter()
            // Display run_id identifies the originating source run. A parent
            // commit may archive source-attributed child messages under its
            // own replay scope, so only the durable session must match here.
            .any(|message| message.session_id != *session_id)
        || commit
            .replay_events
            .iter()
            .any(|event| event.scope != scope);
    if identity_mismatch {
        return Err(SessionStoreError::Failed(format!(
            "run evidence identity mismatch for session {} and run {}",
            session_id.as_str(),
            run_id.as_str()
        )));
    }
    Ok(())
}

fn optional_metadata_id_matches(
    metadata: &serde_json::Map<String, Value>,
    key: &str,
    expected: &str,
) -> bool {
    metadata
        .get(key)
        .is_none_or(|value| value.as_str() == Some(expected))
}

fn validate_stream_evidence(commit: &RunEvidenceCommit) -> SessionStoreResult<()> {
    let session_id = &commit.run.session_id;
    let run_id = &commit.run.run_id;
    let scope = ReplayScope::run(run_id.as_str());
    ensure_unique_sequences(
        commit.stream_records.iter().map(|record| record.sequence),
        "raw stream",
    )?;
    ensure_unique_sequences(
        commit
            .display_messages
            .iter()
            .map(|message| message.sequence),
        "display stream",
    )?;
    ensure_unique_sequences(
        commit.replay_events.iter().map(|event| event.sequence),
        "replay event",
    )?;
    let mut cursor_streams = BTreeSet::new();
    for cursor in &commit.stream_cursors {
        cursor
            .validate_for_run(run_id)
            .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        let identity = (
            cursor.family().as_str().to_string(),
            cursor.scope().as_str().to_string(),
        );
        if !cursor_streams.insert(identity) {
            return Err(SessionStoreError::Failed(format!(
                "duplicate stream cursor family/scope for run {}",
                run_id.as_str()
            )));
        }
    }
    if let Some(snapshot) = commit.display_snapshot.as_ref() {
        snapshot
            .validate(ReplayCursorFamily::Display, &scope)
            .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        if snapshot
            .display_messages
            .iter()
            .any(|message| message.session_id != *session_id || message.run_id != *run_id)
        {
            return Err(SessionStoreError::Failed(format!(
                "display snapshot ownership mismatch for session {} and run {}",
                session_id.as_str(),
                run_id.as_str()
            )));
        }
    }
    if let Some(environment) = commit.environment_state.as_ref() {
        validate_environment_envelope(environment)?;
    }
    Ok(())
}

fn validate_host_event_publications(commit: &RunEvidenceCommit) -> SessionStoreResult<()> {
    let mut publication_keys = BTreeSet::new();
    let mut event_ids = BTreeSet::new();
    for publication in &commit.host_event_publications {
        publication.validate()?;
        if !publication_keys.insert(publication.publication_key.as_str())
            || !event_ids.insert(publication.event_id.as_str())
        {
            return Err(SessionStoreError::Failed(format!(
                "duplicate host event publication for run {}",
                commit.run.run_id.as_str()
            )));
        }
        match &publication.scope {
            crate::DurableHostEventScope::Global => {
                return Err(SessionStoreError::Failed(format!(
                    "run evidence {} cannot publish a global host event",
                    commit.run.run_id.as_str()
                )));
            }
            crate::DurableHostEventScope::Session { session_id }
                if session_id != &commit.run.session_id =>
            {
                return Err(SessionStoreError::Failed(format!(
                    "host event session scope mismatch for run {}",
                    commit.run.run_id.as_str()
                )));
            }
            crate::DurableHostEventScope::Run { session_id, run_id }
                if session_id != &commit.run.session_id
                    || (run_id != &commit.run.run_id
                        && !commit
                            .related_run_updates
                            .iter()
                            .any(|update| &update.run_id == run_id)) =>
            {
                return Err(SessionStoreError::Failed(format!(
                    "host event run scope mismatch for run {}",
                    commit.run.run_id.as_str()
                )));
            }
            crate::DurableHostEventScope::Session { .. }
            | crate::DurableHostEventScope::Run { .. } => {}
        }
    }
    Ok(())
}

fn validate_related_run_evidence(commit: &RunEvidenceCommit) -> SessionStoreResult<()> {
    let session_id = &commit.run.session_id;
    let run_id = &commit.run.run_id;
    if !commit.related_run_updates.is_empty()
        && (commit.related_run_updates.len() != 1
            || commit.run.restore_from_run_id.as_ref()
                != commit
                    .related_run_updates
                    .first()
                    .map(|update| &update.run_id))
    {
        return Err(SessionStoreError::Failed(format!(
            "related run update must match restore_from_run_id for run {}",
            run_id.as_str()
        )));
    }
    let mut related_runs = BTreeSet::new();
    for update in &commit.related_run_updates {
        if update.run_id == *run_id || !related_runs.insert(update.run_id.clone()) {
            return Err(SessionStoreError::Failed(format!(
                "invalid or duplicate related run update {} for run {}",
                update.run_id.as_str(),
                run_id.as_str()
            )));
        }
        if update.resume_claim_id.as_deref().is_none_or(str::is_empty) {
            return Err(SessionStoreError::Failed(format!(
                "related run update {} requires an exclusive resume claim",
                update.run_id.as_str()
            )));
        }
        let identity_mismatch = update
            .approvals
            .iter()
            .any(|record| record.session_id != *session_id || record.run_id != update.run_id)
            || update
                .deferred_tools
                .iter()
                .any(|record| record.session_id != *session_id || record.run_id != update.run_id);
        if identity_mismatch {
            return Err(SessionStoreError::Failed(format!(
                "related run evidence identity mismatch for session {} and run {}",
                session_id.as_str(),
                update.run_id.as_str()
            )));
        }
        ensure_unique_strings(
            update
                .approvals
                .iter()
                .map(|record| record.approval_id.as_str()),
            "approval",
        )?;
        ensure_unique_strings(
            update
                .deferred_tools
                .iter()
                .map(|record| record.deferred_id.as_str()),
            "deferred tool",
        )?;
    }
    Ok(())
}

fn validate_related_terminal_projections(commit: &RunEvidenceCommit) -> SessionStoreResult<()> {
    for update in &commit.related_run_updates {
        RunTerminalProjection {
            status: update.status,
            output_preview: update.output_preview.clone(),
            error: update.terminal_error.clone(),
        }
        .validate()
        .map_err(|error| {
            SessionStoreError::Failed(format!(
                "invalid related run update {}: {error}",
                update.run_id.as_str()
            ))
        })?;
    }
    Ok(())
}

fn ensure_unique_sequences(
    sequences: impl IntoIterator<Item = usize>,
    family: &str,
) -> SessionStoreResult<()> {
    let mut seen = BTreeSet::new();
    for sequence in sequences {
        if !seen.insert(sequence) {
            return Err(SessionStoreError::Failed(format!(
                "duplicate {family} sequence {sequence}"
            )));
        }
    }
    Ok(())
}

fn ensure_unique_strings<'a>(
    values: impl IntoIterator<Item = &'a str>,
    kind: &str,
) -> SessionStoreResult<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(SessionStoreError::Failed(format!(
                "duplicate {kind} id {value} in related run update"
            )));
        }
    }
    Ok(())
}

fn validate_environment_envelope(value: &Value) -> SessionStoreResult<()> {
    let Some(object) = value.as_object() else {
        return Err(SessionStoreError::Failed(
            "environment state must use a versioned envelope".to_string(),
        ));
    };
    let schema = object.get("schema").and_then(Value::as_str);
    let version = object.get("version").and_then(Value::as_u64);
    if schema != Some("starweaver.environment.state")
        || version != Some(1)
        || !object.contains_key("payload")
    {
        return Err(SessionStoreError::Failed(
            "environment state must use starweaver.environment.state version 1".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use starweaver_context::{AgentCheckpoint, AgentRunState, ResumableState};
    use starweaver_core::{AgentExecutionNode, ConversationId, RunId, SessionId};
    use starweaver_stream::{DisplayMessage, DisplayMessageKind};

    use super::RunEvidenceCommit;
    use crate::RunRecord;

    fn evidence() -> RunEvidenceCommit {
        let session_id = SessionId::from_string("session-evidence-validation");
        let run_id = RunId::from_string("run-evidence-validation");
        let conversation_id = ConversationId::from_string("conversation-evidence-validation");
        let run = RunRecord::new(session_id.clone(), run_id.clone(), conversation_id.clone());
        let state = ResumableState {
            session_id: Some(session_id),
            run_id: Some(run_id),
            conversation_id: Some(conversation_id),
            ..ResumableState::default()
        };
        RunEvidenceCommit::new(run, state)
    }

    #[test]
    fn validate_rejects_context_checkpoint_and_display_identity_mismatches() {
        let mut provider_affinity = evidence();
        provider_affinity.context_state.session_id =
            Some(SessionId::from_string("provider-affinity"));
        provider_affinity.context_state.run_id = Some(RunId::from_string("runtime-run"));
        assert!(provider_affinity.validate().is_ok());

        let mut context_mismatch = evidence();
        context_mismatch.context_state.metadata.insert(
            "starweaver.durable_session_id".to_string(),
            serde_json::json!("other-session"),
        );
        assert!(context_mismatch.validate().is_err());

        let mut checkpoint_mismatch = evidence();
        let state = AgentRunState::new(
            checkpoint_mismatch.run.run_id.clone(),
            checkpoint_mismatch.run.conversation_id.clone(),
        );
        let mut checkpoint = AgentCheckpoint::new(AgentExecutionNode::PrepareModelRequest, &state);
        checkpoint.run_id = RunId::from_string("other-run");
        checkpoint_mismatch.checkpoints.push(checkpoint);
        assert!(checkpoint_mismatch.validate().is_err());

        let mut display_mismatch = evidence();
        display_mismatch.display_messages.push(DisplayMessage::new(
            0,
            SessionId::from_string("other-session"),
            display_mismatch.run.run_id.clone(),
            DisplayMessageKind::RunStarted,
        ));
        assert!(display_mismatch.validate().is_err());
    }

    #[test]
    fn validate_rejects_caller_control_of_reserved_evidence_digest() {
        let mut commit = evidence();
        commit.run.metadata.insert(
            RunEvidenceCommit::DIGEST_METADATA_KEY.to_string(),
            serde_json::json!("caller-controlled"),
        );
        let error = commit.validate().expect_err("reserved key must fail");
        assert!(error.to_string().contains("reserved run metadata key"));
    }
}
