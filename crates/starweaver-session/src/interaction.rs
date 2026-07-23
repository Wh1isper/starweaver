//! Product-neutral commands and results for durable human-interaction mutations.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::{Metadata, RunId, SessionId};

use crate::{
    ApprovalDecision, ApprovalRecord, ApprovalStatus, DeferredToolRecord, ExecutionStatus,
    MutationReceipt, PendingHostEventPublication, SessionStoreError, SessionStoreResult,
};

/// Stable operation identifier for approval decisions.
pub const APPROVAL_DECIDE_OPERATION: &str = "approval.decide";
/// Stable operation identifier for successful deferred-tool completion.
pub const DEFERRED_COMPLETE_OPERATION: &str = "deferred.complete";
/// Stable operation identifier for failed deferred-tool completion.
pub const DEFERRED_FAIL_OPERATION: &str = "deferred.fail";
/// Stable operation identifier for clarification resolution.
pub const CLARIFICATION_RESOLVE_OPERATION: &str = "clarification.resolve";
/// Tool name whose approval records represent clarification requests.
pub const ASK_USER_QUESTION_ACTION: &str = "ask_user_question";
/// Approval-decision metadata key containing validated clarification answers.
pub const CLARIFICATION_ANSWERS_METADATA_KEY: &str = "clarification_answers";
/// Approval-decision metadata key containing an optional aggregate response.
pub const CLARIFICATION_RESPONSE_METADATA_KEY: &str = "clarification_response";

/// Shared authority, concurrency, idempotency, and publication evidence for an interaction command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractionMutationContext {
    /// Trusted authority binding owning the idempotency namespace.
    pub authority_binding: String,
    /// Expected current revision of the authoritative interaction record.
    pub expected_revision: u64,
    /// Idempotency key scoped to `authority_binding`.
    pub idempotency_key: String,
    /// Canonical fingerprint of the normalized authorized command.
    pub command_fingerprint: String,
    /// Authoritative mutation timestamp.
    pub occurred_at: DateTime<Utc>,
    /// Optional product-neutral host event committed with state and receipt.
    pub host_event_publication: Option<PendingHostEventPublication>,
}

impl InteractionMutationContext {
    /// Validate shared mutation evidence.
    ///
    /// # Errors
    ///
    /// Returns an error for empty authority/idempotency evidence, revision zero, or an invalid
    /// caller-supplied host-event publication.
    pub fn validate(&self) -> SessionStoreResult<()> {
        require_non_empty("interaction authority binding", &self.authority_binding)?;
        require_non_empty("interaction idempotency key", &self.idempotency_key)?;
        require_non_empty("interaction command fingerprint", &self.command_fingerprint)?;
        if self.expected_revision == 0 {
            return Err(SessionStoreError::Failed(
                "interaction expected revision must be greater than zero".to_string(),
            ));
        }
        if let Some(publication) = &self.host_event_publication {
            publication.validate()?;
        }
        Ok(())
    }
}

/// Atomic approval-decision command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecideApproval {
    /// Shared mutation evidence.
    pub context: InteractionMutationContext,
    /// Owning session.
    pub session_id: SessionId,
    /// Owning run.
    pub run_id: RunId,
    /// Approval identity.
    pub approval_id: String,
    /// Terminal decision to persist. Only approved and denied are accepted.
    pub decision: ApprovalDecision,
}

impl DecideApproval {
    /// Validate command-local invariants.
    ///
    /// # Errors
    ///
    /// Returns an error when shared evidence is invalid, the id is empty, the decision is not
    /// approved/denied, or the decision timestamp differs from the mutation timestamp.
    pub fn validate(&self) -> SessionStoreResult<()> {
        self.context.validate()?;
        require_non_empty("approval id", &self.approval_id)?;
        if !matches!(
            self.decision.status,
            ApprovalStatus::Approved | ApprovalStatus::Denied
        ) {
            return Err(SessionStoreError::Failed(
                "approval decision must be approved or denied".to_string(),
            ));
        }
        if self.decision.decided_at != self.context.occurred_at {
            return Err(SessionStoreError::Failed(
                "approval decision time must match mutation time".to_string(),
            ));
        }
        Ok(())
    }
}

/// Durable result of an atomic approval decision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApprovalMutationResult {
    /// Authoritative post-mutation approval record.
    pub approval: ApprovalRecord,
    /// Durable mutation receipt.
    pub receipt: MutationReceipt,
}

impl starweaver_core::VersionedRecord for ApprovalMutationResult {
    const SCHEMA: &'static str = "starweaver.session.approval_mutation_result";
}

impl ApprovalMutationResult {
    /// Return an exact-replay projection without changing durable evidence.
    #[must_use]
    pub fn replayed_projection(&self) -> Self {
        Self {
            approval: self.approval.clone(),
            receipt: self.receipt.replayed_projection(),
        }
    }
}

/// Terminal outcome supplied for a deferred tool.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DeferredMutationOutcome {
    /// Tool completed successfully.
    Completed {
        /// Tool response.
        response: Value,
        /// Product-neutral result metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
    /// Tool failed.
    Failed {
        /// Safe failure response.
        response: Value,
        /// Product-neutral result metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
}

impl DeferredMutationOutcome {
    /// Stable operation selected by this terminal outcome.
    #[must_use]
    pub const fn operation(&self) -> &'static str {
        match self {
            Self::Completed { .. } => DEFERRED_COMPLETE_OPERATION,
            Self::Failed { .. } => DEFERRED_FAIL_OPERATION,
        }
    }

    /// Durable execution status selected by this terminal outcome.
    #[must_use]
    pub const fn status(&self) -> ExecutionStatus {
        match self {
            Self::Completed { .. } => ExecutionStatus::Completed,
            Self::Failed { .. } => ExecutionStatus::Failed,
        }
    }

    /// Borrow the response and metadata.
    #[must_use]
    pub const fn parts(&self) -> (&Value, &Metadata) {
        match self {
            Self::Completed { response, metadata } | Self::Failed { response, metadata } => {
                (response, metadata)
            }
        }
    }
}

/// Atomic deferred-tool completion/failure command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolveDeferredTool {
    /// Shared mutation evidence.
    pub context: InteractionMutationContext,
    /// Owning session.
    pub session_id: SessionId,
    /// Owning run.
    pub run_id: RunId,
    /// Deferred request identity.
    pub deferred_id: String,
    /// Terminal outcome.
    pub outcome: DeferredMutationOutcome,
}

impl ResolveDeferredTool {
    /// Validate command-local invariants.
    ///
    /// # Errors
    ///
    /// Returns an error when shared mutation evidence is invalid or the deferred id is empty.
    pub fn validate(&self) -> SessionStoreResult<()> {
        self.context.validate()?;
        require_non_empty("deferred id", &self.deferred_id)
    }
}

/// Durable result of an atomic deferred-tool mutation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeferredMutationResult {
    /// Authoritative post-mutation deferred record.
    pub deferred: DeferredToolRecord,
    /// Durable mutation receipt.
    pub receipt: MutationReceipt,
}

impl starweaver_core::VersionedRecord for DeferredMutationResult {
    const SCHEMA: &'static str = "starweaver.session.deferred_mutation_result";
}

impl DeferredMutationResult {
    /// Return an exact-replay projection without changing durable evidence.
    #[must_use]
    pub fn replayed_projection(&self) -> Self {
        Self {
            deferred: self.deferred.clone(),
            receipt: self.receipt.replayed_projection(),
        }
    }
}

/// One option in a durable clarification question.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClarificationOption {
    /// Stable option label returned in answers.
    pub label: String,
    /// Human-readable option description.
    #[serde(default)]
    pub description: String,
    /// Optional preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// One question parsed from durable `ask_user_question` request evidence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClarificationQuestion {
    /// Human-readable short header.
    #[serde(default)]
    pub header: String,
    /// Stable full question text and answer binding.
    pub question: String,
    /// Whether more than one listed option may be selected.
    #[serde(default, alias = "multiSelect")]
    pub multi_select: bool,
    /// Listed options. An empty list means a free-text-only question.
    #[serde(default)]
    pub options: Vec<ClarificationOption>,
}

/// Validated answer to one clarification question.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClarificationAnswer {
    /// Exact question text identifying the durable request question.
    pub question: String,
    /// Selected durable option labels.
    #[serde(default, alias = "selectedOptions")]
    pub selected_options: Vec<String>,
    /// Optional non-empty free-text answer.
    #[serde(default, alias = "freeText", skip_serializing_if = "Option::is_none")]
    pub free_text: Option<String>,
}

/// Atomic clarification-resolution command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolveClarification {
    /// Shared mutation evidence.
    pub context: InteractionMutationContext,
    /// Owning session.
    pub session_id: SessionId,
    /// Owning run.
    pub run_id: RunId,
    /// Approval identity representing the clarification.
    pub clarification_id: String,
    /// Answers, exactly one per durable request question.
    pub answers: Vec<ClarificationAnswer>,
    /// Optional aggregate response retained with answer metadata.
    pub response: Option<String>,
    /// Actor resolving the clarification.
    pub resolved_by: Option<String>,
}

impl ResolveClarification {
    /// Validate command-local invariants that do not require the durable request.
    ///
    /// # Errors
    ///
    /// Returns an error when shared mutation evidence is invalid, an identity is empty, or an
    /// optional response/resolver is present but empty.
    pub fn validate(&self) -> SessionStoreResult<()> {
        self.context.validate()?;
        require_non_empty("clarification id", &self.clarification_id)?;
        if self.response.as_deref().is_some_and(str::is_empty) {
            return Err(SessionStoreError::Failed(
                "clarification response cannot be empty".to_string(),
            ));
        }
        if self.resolved_by.as_deref().is_some_and(str::is_empty) {
            return Err(SessionStoreError::Failed(
                "clarification resolver cannot be empty".to_string(),
            ));
        }
        Ok(())
    }
}

/// Durable typed clarification resolution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClarificationResolution {
    /// Clarification/approval identity.
    pub clarification_id: String,
    /// Owning session.
    pub session_id: SessionId,
    /// Owning run.
    pub run_id: RunId,
    /// Questions as read from the authoritative durable request.
    pub questions: Vec<ClarificationQuestion>,
    /// Validated answers in durable question order.
    pub answers: Vec<ClarificationAnswer>,
    /// Optional aggregate response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<String>,
    /// Post-mutation revision of the backing approval record.
    pub revision: u64,
    /// Resolution timestamp.
    pub resolved_at: DateTime<Utc>,
}

/// Durable result of an atomic clarification resolution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClarificationMutationResult {
    /// Typed, validated resolution projection.
    pub clarification: ClarificationResolution,
    /// Authoritative post-mutation approval record.
    pub approval: ApprovalRecord,
    /// Durable mutation receipt.
    pub receipt: MutationReceipt,
}

impl starweaver_core::VersionedRecord for ClarificationMutationResult {
    const SCHEMA: &'static str = "starweaver.session.clarification_mutation_result";
}

impl ClarificationMutationResult {
    /// Return an exact-replay projection without changing durable evidence.
    #[must_use]
    pub fn replayed_projection(&self) -> Self {
        Self {
            clarification: self.clarification.clone(),
            approval: self.approval.clone(),
            receipt: self.receipt.replayed_projection(),
        }
    }
}

/// Parse and strictly validate a durable clarification request and caller answers.
///
/// The request must be an object containing a non-empty `questions` array. Questions and option
/// labels must be unique. Answers must bind one-to-one by exact question text; selected labels
/// must exist, single-select questions accept at most one label, and every answer must contain a
/// selection or non-empty free text.
///
/// # Errors
///
/// Returns an error when the durable request is malformed or answers do not bind one-to-one to
/// its questions and option constraints.
#[allow(clippy::too_many_lines)]
pub fn validate_clarification_answers(
    request: &Value,
    answers: &[ClarificationAnswer],
) -> SessionStoreResult<(Vec<ClarificationQuestion>, Vec<ClarificationAnswer>)> {
    let questions_value = request
        .as_object()
        .and_then(|object| object.get("questions"))
        .ok_or_else(|| {
            SessionStoreError::Failed(
                "clarification request must contain a questions array".to_string(),
            )
        })?;
    let questions = serde_json::from_value::<Vec<ClarificationQuestion>>(questions_value.clone())
        .map_err(|error| {
        SessionStoreError::Failed(format!("malformed clarification questions: {error}"))
    })?;
    if questions.is_empty() {
        return Err(SessionStoreError::Failed(
            "clarification request must contain at least one question".to_string(),
        ));
    }

    let mut by_question = BTreeMap::new();
    for question in &questions {
        require_non_empty("clarification question", &question.question)?;
        let mut labels = BTreeSet::new();
        for option in &question.options {
            require_non_empty("clarification option label", &option.label)?;
            if !labels.insert(option.label.as_str()) {
                return Err(SessionStoreError::Failed(format!(
                    "duplicate clarification option {} for question {}",
                    option.label, question.question
                )));
            }
        }
        if by_question
            .insert(question.question.as_str(), question)
            .is_some()
        {
            return Err(SessionStoreError::Failed(format!(
                "duplicate clarification question {}",
                question.question
            )));
        }
    }

    if answers.len() != questions.len() {
        return Err(SessionStoreError::Failed(
            "clarification answers must match every durable question exactly once".to_string(),
        ));
    }
    let mut answer_by_question = BTreeMap::new();
    for answer in answers {
        let question = by_question.get(answer.question.as_str()).ok_or_else(|| {
            SessionStoreError::Failed(format!(
                "clarification answer does not match durable question {}",
                answer.question
            ))
        })?;
        if answer_by_question
            .insert(answer.question.as_str(), answer)
            .is_some()
        {
            return Err(SessionStoreError::Failed(format!(
                "duplicate clarification answer for {}",
                answer.question
            )));
        }
        if answer.free_text.as_deref().is_some_and(str::is_empty) {
            return Err(SessionStoreError::Failed(format!(
                "clarification free text cannot be empty for {}",
                answer.question
            )));
        }
        if answer.selected_options.is_empty() && answer.free_text.is_none() {
            return Err(SessionStoreError::Failed(format!(
                "clarification answer is empty for {}",
                answer.question
            )));
        }
        if !question.multi_select && answer.selected_options.len() > 1 {
            return Err(SessionStoreError::Failed(format!(
                "clarification question {} does not allow multiple selections",
                answer.question
            )));
        }
        let allowed = question
            .options
            .iter()
            .map(|option| option.label.as_str())
            .collect::<BTreeSet<_>>();
        let mut selected = BTreeSet::new();
        for label in &answer.selected_options {
            if !selected.insert(label.as_str()) {
                return Err(SessionStoreError::Failed(format!(
                    "duplicate clarification selection {label} for {}",
                    answer.question
                )));
            }
            if !allowed.contains(label.as_str()) {
                return Err(SessionStoreError::Failed(format!(
                    "clarification selection {label} is not an option for {}",
                    answer.question
                )));
            }
        }
    }

    let ordered_answers = questions
        .iter()
        .map(|question| {
            answer_by_question
                .get(question.question.as_str())
                .map(|answer| (*answer).clone())
                .ok_or_else(|| {
                    SessionStoreError::Failed(format!(
                        "missing clarification answer for {}",
                        question.question
                    ))
                })
        })
        .collect::<SessionStoreResult<Vec<_>>>()?;
    Ok((questions, ordered_answers))
}

fn require_non_empty(label: &str, value: &str) -> SessionStoreResult<()> {
    if value.is_empty() {
        return Err(SessionStoreError::Failed(format!(
            "{label} cannot be empty"
        )));
    }
    Ok(())
}
