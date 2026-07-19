//! Host-neutral continuation preparation over durable session evidence.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_context::AgentRunState;
use starweaver_model::TOOL_RETURN_APPROVAL_ARGUMENTS_METADATA_KEY;
use thiserror::Error;

use crate::{ApprovalStatus, ExecutionStatus, SessionResumeSnapshot};

/// Continuation preparation mode selected by the host.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationPreparationMode {
    /// Restore durable context without consuming HITL decisions.
    Ordinary,
    /// Validate a waiting run and prepare its terminal approval/deferred decisions.
    WaitingHitl,
}

/// Side-effect-free continuation package shared by every product host.
///
/// This package deliberately contains durable evidence only. Approved tools are executed later by
/// the agent layer, after the host has atomically admitted a waiting-run replacement and moved its
/// claim to `Started`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreparedContinuation {
    /// Preparation mode used to validate the evidence.
    pub mode: ContinuationPreparationMode,
    /// Canonical durable snapshot. Waiting checkpoint state is normalized to `Waiting` in-memory.
    pub snapshot: SessionResumeSnapshot,
}

impl PreparedContinuation {
    /// Prepare an ordinary context continuation.
    ///
    /// # Errors
    ///
    /// Returns an error when the snapshot contains inconsistent session/run identities.
    pub fn ordinary(snapshot: SessionResumeSnapshot) -> Result<Self, ContinuationPreparationError> {
        validate_snapshot_identity(&snapshot)?;
        Ok(Self {
            mode: ContinuationPreparationMode::Ordinary,
            snapshot,
        })
    }

    /// Prepare a waiting HITL continuation without executing hooks or tools.
    ///
    /// # Errors
    ///
    /// Returns an error when the source is not waiting, has no checkpoint, or its pending HITL
    /// items do not have one matching terminal durable decision/result.
    pub fn waiting_hitl(
        mut snapshot: SessionResumeSnapshot,
    ) -> Result<Self, ContinuationPreparationError> {
        validate_snapshot_identity(&snapshot)?;
        if snapshot.run.status != crate::RunStatus::Waiting {
            return Err(ContinuationPreparationError::SourceNotWaiting(
                snapshot.run.run_id.as_str().to_string(),
            ));
        }
        let checkpoint_state = {
            let checkpoint = snapshot.latest_checkpoint.as_mut().ok_or_else(|| {
                ContinuationPreparationError::MissingCheckpoint(
                    snapshot.run.run_id.as_str().to_string(),
                )
            })?;
            // Checkpoints may precede the terminal Waiting projection. The durable run record is
            // authoritative; normalization is process-local and never rewrites immutable evidence.
            checkpoint.state.status = starweaver_core::RunLifecycle::Waiting;
            checkpoint.state.clone()
        };
        validate_waiting_evidence(&snapshot, &checkpoint_state)?;
        Ok(Self {
            mode: ContinuationPreparationMode::WaitingHitl,
            snapshot,
        })
    }

    /// Return the normalized checkpoint state for a waiting HITL continuation.
    #[must_use]
    pub fn waiting_state(&self) -> Option<&AgentRunState> {
        matches!(self.mode, ContinuationPreparationMode::WaitingHitl)
            .then(|| {
                self.snapshot
                    .latest_checkpoint
                    .as_ref()
                    .map(|checkpoint| &checkpoint.state)
            })
            .flatten()
    }

    /// Consume the package and return its canonical snapshot.
    #[must_use]
    pub fn into_snapshot(self) -> SessionResumeSnapshot {
        self.snapshot
    }
}

/// Stable continuation preparation failures shared by CLI, RPC, and future hosts.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ContinuationPreparationError {
    /// Snapshot session and run ownership disagree.
    #[error("continuation snapshot identity mismatch: {0}")]
    IdentityMismatch(String),
    /// Waiting preparation was requested for another source status.
    #[error("run {0} is not waiting")]
    SourceNotWaiting(String),
    /// A waiting source has no resumable checkpoint.
    #[error("waiting run {0} has no resumable checkpoint")]
    MissingCheckpoint(String),
    /// The checkpoint has no pending HITL work.
    #[error("waiting run {0} has no pending HITL work")]
    NoPendingHitl(String),
    /// Durable decision evidence contains duplicate identities.
    #[error("duplicate durable {kind} decision for {id}")]
    DuplicateDecision {
        /// Durable decision family (`approval` or `deferred`).
        kind: &'static str,
        /// Duplicate durable identity.
        id: String,
    },
    /// One pending item has no terminal durable decision.
    #[error("missing terminal durable {kind} decision for {id}")]
    MissingDecision {
        /// Durable decision family (`approval` or `deferred`).
        kind: &'static str,
        /// Pending identity without a terminal decision.
        id: String,
    },
    /// Durable evidence references an item that is not pending in the checkpoint.
    #[error("unknown durable {kind} decision for {id}")]
    UnknownDecision {
        /// Durable decision family (`approval` or `deferred`).
        kind: &'static str,
        /// Durable identity absent from the waiting checkpoint.
        id: String,
    },
    /// Durable evidence uses the expected durable id but references another tool call.
    #[error("durable {kind} decision {id} references {actual}; expected {expected}")]
    MismatchedDecisionIdentity {
        /// Durable decision family (`approval` or `deferred`).
        kind: &'static str,
        /// Durable decision identity.
        id: String,
        /// Referenced tool call identity.
        actual: String,
        /// Expected tool call identity.
        expected: String,
    },
    /// Durable decision evidence does not match the pending checkpoint evidence.
    #[error("durable {kind} decision {id} has mismatched {field}")]
    MismatchedDecisionEvidence {
        /// Durable decision family (`approval` or `deferred`).
        kind: &'static str,
        /// Durable decision identity.
        id: String,
        /// Evidence field that did not match.
        field: &'static str,
    },
    /// A durable approval status and decision record disagree.
    #[error("inconsistent durable approval decision for {0}")]
    InconsistentApproval(String),
}

fn validate_snapshot_identity(
    snapshot: &SessionResumeSnapshot,
) -> Result<(), ContinuationPreparationError> {
    if snapshot.session.session_id != snapshot.run.session_id {
        return Err(ContinuationPreparationError::IdentityMismatch(format!(
            "session {} does not own run {}",
            snapshot.session.session_id.as_str(),
            snapshot.run.run_id.as_str()
        )));
    }
    if snapshot
        .state
        .run_id
        .as_ref()
        .is_some_and(|run_id| run_id != &snapshot.run.run_id)
    {
        return Err(ContinuationPreparationError::IdentityMismatch(format!(
            "context run does not match {}",
            snapshot.run.run_id.as_str()
        )));
    }
    if snapshot
        .state
        .conversation_id
        .as_ref()
        .is_some_and(|conversation_id| conversation_id != &snapshot.run.conversation_id)
    {
        return Err(ContinuationPreparationError::IdentityMismatch(format!(
            "context conversation does not match {}",
            snapshot.run.conversation_id.as_str()
        )));
    }
    validate_optional_durable_id(
        &snapshot.state.metadata,
        "starweaver.durable_session_id",
        snapshot.session.session_id.as_str(),
    )?;
    validate_optional_durable_id(
        &snapshot.state.metadata,
        "starweaver.durable_run_id",
        snapshot.run.run_id.as_str(),
    )?;
    if let Some(checkpoint) = &snapshot.latest_checkpoint {
        if checkpoint.run_id != snapshot.run.run_id {
            return Err(ContinuationPreparationError::IdentityMismatch(format!(
                "checkpoint run does not match {}",
                snapshot.run.run_id.as_str()
            )));
        }
        if checkpoint.state.run_id != snapshot.run.run_id {
            return Err(ContinuationPreparationError::IdentityMismatch(format!(
                "checkpoint state run does not match {}",
                snapshot.run.run_id.as_str()
            )));
        }
        if checkpoint.conversation_id != snapshot.run.conversation_id {
            return Err(ContinuationPreparationError::IdentityMismatch(format!(
                "checkpoint conversation does not match {}",
                snapshot.run.conversation_id.as_str()
            )));
        }
        if checkpoint.state.conversation_id != snapshot.run.conversation_id {
            return Err(ContinuationPreparationError::IdentityMismatch(format!(
                "checkpoint state conversation does not match {}",
                snapshot.run.conversation_id.as_str()
            )));
        }
        if checkpoint.node != checkpoint.resume.node
            || checkpoint.run_step != checkpoint.resume.run_step
            || checkpoint.run_step != checkpoint.state.run_step
        {
            return Err(ContinuationPreparationError::IdentityMismatch(
                "checkpoint boundary metadata does not match checkpoint state".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_optional_durable_id(
    metadata: &serde_json::Map<String, Value>,
    key: &'static str,
    expected: &str,
) -> Result<(), ContinuationPreparationError> {
    if metadata
        .get(key)
        .is_some_and(|value| value.as_str() != Some(expected))
    {
        return Err(ContinuationPreparationError::IdentityMismatch(format!(
            "context metadata {key} does not match {expected}"
        )));
    }
    Ok(())
}

fn validate_waiting_evidence(
    snapshot: &SessionResumeSnapshot,
    state: &AgentRunState,
) -> Result<(), ContinuationPreparationError> {
    if !state.has_pending_hitl() {
        return Err(ContinuationPreparationError::NoPendingHitl(
            snapshot.run.run_id.as_str().to_string(),
        ));
    }
    validate_approval_evidence(snapshot, state)?;

    let pending_deferred = state
        .deferred_tool_returns
        .iter()
        .map(|item| {
            (
                format!("deferred_{}_{}", state.run_id.as_str(), item.tool_call_id),
                item.tool_call_id.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut deferred = BTreeMap::new();
    for record in &snapshot.deferred_tools {
        if record.session_id != snapshot.run.session_id || record.run_id != snapshot.run.run_id {
            return Err(ContinuationPreparationError::IdentityMismatch(format!(
                "deferred record {} does not belong to the source run",
                record.deferred_id
            )));
        }
        if deferred
            .insert(record.deferred_id.clone(), record)
            .is_some()
        {
            return Err(ContinuationPreparationError::DuplicateDecision {
                kind: "deferred",
                id: record.deferred_id.clone(),
            });
        }
    }
    validate_deferred_set(&pending_deferred, &deferred)
}

fn validate_approval_evidence(
    snapshot: &SessionResumeSnapshot,
    state: &AgentRunState,
) -> Result<(), ContinuationPreparationError> {
    let pending_tool_calls = state
        .latest_response
        .iter()
        .flat_map(starweaver_model::ModelResponse::tool_calls)
        .chain(state.pending_tool_calls.iter().cloned())
        .map(|call| (call.id.clone(), call))
        .collect::<BTreeMap<_, _>>();
    let mut pending_approvals = BTreeMap::new();
    for item in &state.pending_approval_tool_returns {
        let action_id = item.tool_call_id.clone();
        let approval_id = format!("approval_{}_{}", state.run_id.as_str(), action_id);
        let tool_call = pending_tool_calls.get(&action_id).ok_or_else(|| {
            ContinuationPreparationError::IdentityMismatch(format!(
                "pending approval {approval_id} has no matching checkpoint tool call"
            ))
        })?;
        if tool_call.name != item.name {
            return Err(ContinuationPreparationError::MismatchedDecisionEvidence {
                kind: "approval",
                id: approval_id,
                field: "checkpoint tool name",
            });
        }
        let request = item
            .metadata
            .get("approval")
            .cloned()
            .unwrap_or(Value::Null);
        let reviewed_arguments = item
            .metadata
            .get(TOOL_RETURN_APPROVAL_ARGUMENTS_METADATA_KEY)
            .ok_or_else(
                || ContinuationPreparationError::MismatchedDecisionEvidence {
                    kind: "approval",
                    id: approval_id.clone(),
                    field: "reviewed tool arguments",
                },
            )?;
        if reviewed_arguments != &tool_call.arguments.execution_value() {
            return Err(ContinuationPreparationError::MismatchedDecisionEvidence {
                kind: "approval",
                id: approval_id,
                field: "checkpoint tool arguments",
            });
        }
        if pending_approvals
            .insert(
                action_id.clone(),
                PendingApprovalEvidence {
                    approval_id,
                    action_name: item.name.clone(),
                    request,
                    reviewed_arguments: reviewed_arguments.clone(),
                },
            )
            .is_some()
        {
            return Err(ContinuationPreparationError::DuplicateDecision {
                kind: "pending approval",
                id: action_id,
            });
        }
    }
    let mut approvals = BTreeMap::new();
    for record in &snapshot.approvals {
        if record.session_id != snapshot.run.session_id || record.run_id != snapshot.run.run_id {
            return Err(ContinuationPreparationError::IdentityMismatch(format!(
                "approval {} does not belong to the source run",
                record.approval_id
            )));
        }
        let Some(expected) = pending_approvals.get(&record.action_id) else {
            return Err(ContinuationPreparationError::UnknownDecision {
                kind: "approval",
                id: record.action_id.clone(),
            });
        };
        if record.approval_id != expected.approval_id {
            return Err(ContinuationPreparationError::MismatchedDecisionEvidence {
                kind: "approval",
                id: record.action_id.clone(),
                field: "approval id",
            });
        }
        if approvals.insert(record.action_id.clone(), record).is_some() {
            return Err(ContinuationPreparationError::DuplicateDecision {
                kind: "approval",
                id: record.action_id.clone(),
            });
        }
    }
    validate_approval_set(&pending_approvals, &approvals)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingApprovalEvidence {
    approval_id: String,
    action_name: String,
    request: Value,
    reviewed_arguments: Value,
}

fn validate_approval_set(
    pending: &BTreeMap<String, PendingApprovalEvidence>,
    records: &BTreeMap<String, &crate::ApprovalRecord>,
) -> Result<(), ContinuationPreparationError> {
    for (id, expected) in pending {
        let record =
            records
                .get(id)
                .ok_or_else(|| ContinuationPreparationError::MissingDecision {
                    kind: "approval",
                    id: id.clone(),
                })?;
        if record.action_name != expected.action_name {
            return Err(ContinuationPreparationError::MismatchedDecisionEvidence {
                kind: "approval",
                id: record.approval_id.clone(),
                field: "action name",
            });
        }
        if record.request != expected.request {
            return Err(ContinuationPreparationError::MismatchedDecisionEvidence {
                kind: "approval",
                id: record.approval_id.clone(),
                field: "request",
            });
        }
        if record.reviewed_arguments.as_ref() != Some(&expected.reviewed_arguments) {
            return Err(ContinuationPreparationError::MismatchedDecisionEvidence {
                kind: "approval",
                id: record.approval_id.clone(),
                field: "durable reviewed tool arguments",
            });
        }
        if record.status == ApprovalStatus::Pending {
            return Err(ContinuationPreparationError::MissingDecision {
                kind: "approval",
                id: id.clone(),
            });
        }
        let Some(decision) = record.decision.as_ref() else {
            return Err(ContinuationPreparationError::MissingDecision {
                kind: "approval",
                id: id.clone(),
            });
        };
        let consistent = decision.status == record.status
            || (matches!(
                record.status,
                ApprovalStatus::Expired | ApprovalStatus::Cancelled
            ) && matches!(decision.status, ApprovalStatus::Denied));
        if !consistent {
            return Err(ContinuationPreparationError::InconsistentApproval(
                id.clone(),
            ));
        }
    }
    if let Some(id) = records.keys().find(|id| !pending.contains_key(*id)) {
        return Err(ContinuationPreparationError::UnknownDecision {
            kind: "approval",
            id: id.clone(),
        });
    }
    Ok(())
}

fn validate_deferred_set(
    pending: &BTreeMap<String, String>,
    records: &BTreeMap<String, &crate::DeferredToolRecord>,
) -> Result<(), ContinuationPreparationError> {
    for (id, expected_tool_call_id) in pending {
        let record =
            records
                .get(id)
                .ok_or_else(|| ContinuationPreparationError::MissingDecision {
                    kind: "deferred",
                    id: id.clone(),
                })?;
        if record.tool_call_id != *expected_tool_call_id {
            return Err(ContinuationPreparationError::MismatchedDecisionIdentity {
                kind: "deferred",
                id: id.clone(),
                actual: record.tool_call_id.clone(),
                expected: expected_tool_call_id.clone(),
            });
        }
        if matches!(
            record.status,
            ExecutionStatus::Pending | ExecutionStatus::Running | ExecutionStatus::Waiting
        ) {
            return Err(ContinuationPreparationError::MissingDecision {
                kind: "deferred",
                id: id.clone(),
            });
        }
    }
    if let Some(id) = records.keys().find(|id| !pending.contains_key(*id)) {
        return Err(ContinuationPreparationError::UnknownDecision {
            kind: "deferred",
            id: id.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;
    use starweaver_context::{AgentCheckpoint, AgentRunState, ResumableState};
    use starweaver_core::{AgentExecutionNode, ConversationId, RunId, SessionId};
    use starweaver_model::{
        TOOL_RETURN_APPROVAL_ARGUMENTS_METADATA_KEY, ToolArguments, ToolCallPart, ToolReturnPart,
    };

    use super::{ContinuationPreparationError, PreparedContinuation, validate_deferred_set};
    use crate::{
        ApprovalRecord, ApprovalStatus, DeferredToolRecord, ExecutionStatus, RunRecord, RunStatus,
        SessionRecord, SessionResumeSnapshot, ToolApprovalDecision, ToolReturnRecordInput,
    };

    fn waiting_approval_snapshot() -> SessionResumeSnapshot {
        let session_id = SessionId::from_string("session_waiting");
        let run_id = RunId::from_string("run_waiting");
        let conversation_id = ConversationId::from_string("conversation_waiting");
        let arguments = json!({"command": "rm -rf target/tmp"});
        let approval_request = json!({
            "reviewer": "shell_command_reviewer",
            "reason": "destructive command needs review",
            "risk_level": "high",
            "command": "rm -rf target/tmp",
        });
        let mut metadata = serde_json::Map::new();
        metadata.insert("control_flow".to_string(), json!("approval_required"));
        metadata.insert("approval".to_string(), approval_request);
        metadata.insert(
            TOOL_RETURN_APPROVAL_ARGUMENTS_METADATA_KEY.to_string(),
            arguments.clone(),
        );
        let pending_return =
            ToolReturnPart::new("call_approval", "shell", json!("approval required"))
                .with_error(true)
                .with_metadata(metadata);
        let mut run_state = AgentRunState::new(run_id.clone(), conversation_id.clone());
        run_state.pending_tool_calls.push(ToolCallPart {
            id: "call_approval".to_string(),
            name: "shell".to_string(),
            arguments: ToolArguments::parsed(arguments),
        });
        run_state
            .pending_approval_tool_returns
            .push(pending_return.clone());
        let checkpoint = AgentCheckpoint::new(AgentExecutionNode::ToolReturn, &run_state);

        let input = ToolReturnRecordInput::new(
            &session_id,
            &run_id,
            &pending_return.tool_call_id,
            &pending_return.name,
            &pending_return.metadata,
        );
        let Some(mut approval) = ApprovalRecord::from_tool_return(&input) else {
            panic!("approval-required tool return must produce durable evidence");
        };
        approval.status = ApprovalStatus::Approved;
        approval.decision = Some(ToolApprovalDecision::approved().into_approval_decision());

        let mut state = ResumableState {
            run_id: Some(run_id.clone()),
            // Provider-routing affinity deliberately differs from durable session identity.
            session_id: Some(SessionId::from_string("provider_routing_affinity")),
            conversation_id: Some(conversation_id.clone()),
            ..ResumableState::default()
        };
        state.metadata.insert(
            "starweaver.durable_session_id".to_string(),
            json!(session_id.as_str()),
        );
        state.metadata.insert(
            "starweaver.durable_run_id".to_string(),
            json!(run_id.as_str()),
        );
        let mut run = RunRecord::new(session_id.clone(), run_id, conversation_id);
        run.status = RunStatus::Waiting;
        SessionResumeSnapshot {
            session: SessionRecord::new(session_id),
            run,
            state,
            environment_state: None,
            latest_checkpoint: Some(checkpoint),
            stream_records: Vec::new(),
            approvals: vec![approval],
            deferred_tools: Vec::new(),
            stream_cursors: Vec::new(),
        }
    }

    #[test]
    fn waiting_approval_binds_durable_identity_name_request_and_tool_arguments() {
        let snapshot = waiting_approval_snapshot();
        assert!(PreparedContinuation::waiting_hitl(snapshot.clone()).is_ok());

        let mut missing_arguments_binding = snapshot.clone();
        let Some(checkpoint) = missing_arguments_binding.latest_checkpoint.as_mut() else {
            panic!("test snapshot must include a checkpoint");
        };
        checkpoint.state.pending_approval_tool_returns[0]
            .metadata
            .remove(TOOL_RETURN_APPROVAL_ARGUMENTS_METADATA_KEY);
        assert!(matches!(
            PreparedContinuation::waiting_hitl(missing_arguments_binding),
            Err(ContinuationPreparationError::MismatchedDecisionEvidence {
                field: "reviewed tool arguments",
                ..
            })
        ));

        let mut jointly_tampered_arguments = snapshot.clone();
        let Some(checkpoint) = jointly_tampered_arguments.latest_checkpoint.as_mut() else {
            panic!("test snapshot must include a checkpoint");
        };
        let tampered = json!({"command": "other", "environment": {"TOKEN": "changed"}});
        checkpoint.state.pending_tool_calls[0].arguments = ToolArguments::parsed(tampered.clone());
        checkpoint.state.pending_approval_tool_returns[0]
            .metadata
            .insert(
                TOOL_RETURN_APPROVAL_ARGUMENTS_METADATA_KEY.to_string(),
                tampered,
            );
        assert!(matches!(
            PreparedContinuation::waiting_hitl(jointly_tampered_arguments),
            Err(ContinuationPreparationError::MismatchedDecisionEvidence {
                field: "durable reviewed tool arguments",
                ..
            })
        ));

        let mut wrong_id = snapshot.clone();
        wrong_id.approvals[0].approval_id = "approval_other".to_string();
        assert!(matches!(
            PreparedContinuation::waiting_hitl(wrong_id),
            Err(ContinuationPreparationError::MismatchedDecisionEvidence {
                field: "approval id",
                ..
            })
        ));

        let mut wrong_name = snapshot.clone();
        wrong_name.approvals[0].action_name = "other_tool".to_string();
        assert!(matches!(
            PreparedContinuation::waiting_hitl(wrong_name),
            Err(ContinuationPreparationError::MismatchedDecisionEvidence {
                field: "action name",
                ..
            })
        ));

        let mut wrong_request = snapshot.clone();
        wrong_request.approvals[0].request = json!({"arguments": {"command": "other"}});
        assert!(matches!(
            PreparedContinuation::waiting_hitl(wrong_request),
            Err(ContinuationPreparationError::MismatchedDecisionEvidence {
                field: "request",
                ..
            })
        ));

        let mut wrong_arguments = snapshot;
        let Some(checkpoint) = wrong_arguments.latest_checkpoint.as_mut() else {
            panic!("test snapshot must include a checkpoint");
        };
        checkpoint.state.pending_tool_calls[0].arguments =
            ToolArguments::parsed(json!({"command": "other"}));
        assert!(matches!(
            PreparedContinuation::waiting_hitl(wrong_arguments),
            Err(ContinuationPreparationError::MismatchedDecisionEvidence {
                field: "checkpoint tool arguments",
                ..
            })
        ));
    }

    fn assert_identity_rejected(snapshot: SessionResumeSnapshot) {
        assert!(matches!(
            PreparedContinuation::ordinary(snapshot.clone()),
            Err(ContinuationPreparationError::IdentityMismatch(_))
        ));
        assert!(matches!(
            PreparedContinuation::waiting_hitl(snapshot),
            Err(ContinuationPreparationError::IdentityMismatch(_))
        ));
    }

    #[test]
    fn continuation_rejects_context_and_checkpoint_identity_tampering() {
        let snapshot = waiting_approval_snapshot();

        let mut context_conversation = snapshot.clone();
        context_conversation.state.conversation_id =
            Some(ConversationId::from_string("conversation_other"));
        assert_identity_rejected(context_conversation);

        let mut durable_session = snapshot.clone();
        durable_session.state.metadata.insert(
            "starweaver.durable_session_id".to_string(),
            json!("session_other"),
        );
        assert_identity_rejected(durable_session);

        let mut checkpoint_run = snapshot.clone();
        let Some(checkpoint) = checkpoint_run.latest_checkpoint.as_mut() else {
            panic!("test snapshot must include a checkpoint");
        };
        checkpoint.run_id = RunId::from_string("run_other");
        assert_identity_rejected(checkpoint_run);

        let mut checkpoint_conversation = snapshot.clone();
        let Some(checkpoint) = checkpoint_conversation.latest_checkpoint.as_mut() else {
            panic!("test snapshot must include a checkpoint");
        };
        checkpoint.conversation_id = ConversationId::from_string("conversation_other");
        assert_identity_rejected(checkpoint_conversation);

        let mut checkpoint_state_conversation = snapshot;
        let Some(checkpoint) = checkpoint_state_conversation.latest_checkpoint.as_mut() else {
            panic!("test snapshot must include a checkpoint");
        };
        checkpoint.state.conversation_id = ConversationId::from_string("conversation_other");
        assert_identity_rejected(checkpoint_state_conversation);
    }

    #[test]
    fn deferred_decision_rejects_mismatched_tool_call_identity() {
        let deferred_id = "deferred_run_waiting_call_expected".to_string();
        let pending = BTreeMap::from([(deferred_id.clone(), "call_expected".to_string())]);
        let mut record = DeferredToolRecord::new(
            deferred_id.clone(),
            SessionId::from_string("session_waiting"),
            RunId::from_string("run_waiting"),
            "call_other",
            "shell",
        );
        record.status = ExecutionStatus::Completed;
        let records = BTreeMap::from([(deferred_id.clone(), &record)]);

        assert_eq!(
            validate_deferred_set(&pending, &records),
            Err(ContinuationPreparationError::MismatchedDecisionIdentity {
                kind: "deferred",
                id: deferred_id,
                actual: "call_other".to_string(),
                expected: "call_expected".to_string(),
            })
        );
    }

    #[test]
    fn deferred_decision_accepts_matching_terminal_identity() {
        let deferred_id = "deferred_run_waiting_call_expected".to_string();
        let pending = BTreeMap::from([(deferred_id.clone(), "call_expected".to_string())]);
        let mut record = DeferredToolRecord::new(
            deferred_id.clone(),
            SessionId::from_string("session_waiting"),
            RunId::from_string("run_waiting"),
            "call_expected",
            "shell",
        );
        record.status = ExecutionStatus::Completed;
        let records = BTreeMap::from([(deferred_id, &record)]);

        assert_eq!(validate_deferred_set(&pending, &records), Ok(()));
    }
}
