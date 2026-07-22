//! Approval and deferred tool records.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::{Metadata, RunId, SessionId, TraceContext};
use starweaver_model::TOOL_RETURN_APPROVAL_ARGUMENTS_METADATA_KEY;

use crate::records::ExecutionStatus;

const fn initial_interaction_revision() -> u64 {
    1
}

/// Tool-return evidence used to derive durable HITL records without depending on model crates.
pub struct ToolReturnRecordInput<'a> {
    /// Session id that owns the tool return.
    pub session_id: &'a SessionId,
    /// Run id that owns the tool return.
    pub run_id: &'a RunId,
    /// Tool call id.
    pub tool_call_id: &'a str,
    /// Tool name.
    pub tool_name: &'a str,
    /// Tool return metadata.
    pub metadata: &'a Metadata,
    /// Trace context propagated by the run, when available.
    pub trace_context: Option<&'a TraceContext>,
    /// Optional host policy metadata to copy into created records.
    pub policy: Option<Value>,
}

impl<'a> ToolReturnRecordInput<'a> {
    /// Create record input from durable ids and tool-return fields.
    #[must_use]
    pub const fn new(
        session_id: &'a SessionId,
        run_id: &'a RunId,
        tool_call_id: &'a str,
        tool_name: &'a str,
        metadata: &'a Metadata,
    ) -> Self {
        Self {
            session_id,
            run_id,
            tool_call_id,
            tool_name,
            metadata,
            trace_context: None,
            policy: None,
        }
    }

    /// Attach trace context to generated records.
    #[must_use]
    pub const fn with_trace_context(mut self, trace_context: &'a TraceContext) -> Self {
        self.trace_context = Some(trace_context);
        self
    }

    /// Attach host policy metadata to generated records.
    #[must_use]
    pub fn with_policy(mut self, policy: Value) -> Self {
        self.policy = Some(policy);
        self
    }

    fn control_flow(&self) -> Option<&str> {
        self.metadata.get("control_flow").and_then(Value::as_str)
    }

    fn approval_id(&self) -> String {
        format!("approval_{}_{}", self.run_id.as_str(), self.tool_call_id)
    }

    fn deferred_id(&self) -> String {
        format!("deferred_{}_{}", self.run_id.as_str(), self.tool_call_id)
    }
}

/// Durable approval status.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    /// Approval is waiting for a decision.
    #[default]
    Pending,
    /// Approval was granted.
    Approved,
    /// Approval was denied.
    Denied,
    /// Approval expired.
    Expired,
    /// Approval was cancelled.
    Cancelled,
}

/// Recorded approval decision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApprovalDecision {
    /// Decision status.
    pub status: ApprovalStatus,
    /// Actor that made the decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    /// Decision time.
    pub decided_at: DateTime<Utc>,
    /// Decision reason or note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Decision metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Durable approval prompt record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApprovalRecord {
    /// Approval id.
    pub approval_id: String,
    /// Monotonic optimistic-concurrency revision.
    #[serde(default = "initial_interaction_revision")]
    pub revision: u64,
    /// Session id.
    pub session_id: SessionId,
    /// Run id.
    pub run_id: RunId,
    /// Tool call id or action id.
    pub action_id: String,
    /// Tool or action name.
    pub action_name: String,
    /// Requested action payload.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub request: Value,
    /// Canonical tool arguments independently persisted as the reviewed effect boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_arguments: Option<Value>,
    /// Current approval status.
    #[serde(default)]
    pub status: ApprovalStatus,
    /// Recorded decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<ApprovalDecision>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Update time.
    pub updated_at: DateTime<Utc>,
    /// Trace context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
    /// Approval metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl starweaver_core::VersionedRecord for ApprovalRecord {
    const SCHEMA: &'static str = "starweaver.session.approval_record";
    const ALLOW_BARE_V0: bool = true;
}

impl ApprovalRecord {
    /// Build a pending approval record.
    #[must_use]
    pub fn new(
        approval_id: impl Into<String>,
        session_id: SessionId,
        run_id: RunId,
        action_id: impl Into<String>,
        action_name: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            approval_id: approval_id.into(),
            revision: initial_interaction_revision(),
            session_id,
            run_id,
            action_id: action_id.into(),
            action_name: action_name.into(),
            request: Value::Null,
            reviewed_arguments: None,
            status: ApprovalStatus::Pending,
            decision: None,
            created_at: now,
            updated_at: now,
            trace_context: TraceContext::default(),
            metadata: Metadata::default(),
        }
    }

    /// Build a durable approval record from an approval-required tool return.
    #[must_use]
    pub fn from_tool_return(input: &ToolReturnRecordInput<'_>) -> Option<Self> {
        if input.control_flow() != Some("approval_required") {
            return None;
        }
        let mut record = Self::new(
            input.approval_id(),
            input.session_id.clone(),
            input.run_id.clone(),
            input.tool_call_id,
            input.tool_name,
        );
        record.request = input
            .metadata
            .get("approval")
            .cloned()
            .unwrap_or(Value::Null);
        record.reviewed_arguments = input
            .metadata
            .get(TOOL_RETURN_APPROVAL_ARGUMENTS_METADATA_KEY)
            .cloned();
        if let Some(trace_context) = input.trace_context {
            record.trace_context = trace_context.clone();
        }
        if let Some(policy) = &input.policy {
            record.metadata.insert("policy".to_string(), policy.clone());
        }
        Some(record)
    }
}

/// Tool approval decision supplied by a host or user.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ToolApprovalDecision {
    /// Approve tool execution.
    Approved {
        /// Actor that approved the call.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decided_by: Option<String>,
        /// Optional reason.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// Optional replacement arguments for the approved call.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        override_arguments: Option<Value>,
        /// Decision metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
    /// Deny tool execution.
    Denied {
        /// Actor that denied the call.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decided_by: Option<String>,
        /// Optional reason.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// Decision metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
}

impl ToolApprovalDecision {
    /// Build an approved decision.
    #[must_use]
    pub fn approved() -> Self {
        Self::Approved {
            decided_by: None,
            reason: None,
            override_arguments: None,
            metadata: Metadata::default(),
        }
    }

    /// Build a denied decision with a reason.
    #[must_use]
    pub fn denied(reason: impl Into<String>) -> Self {
        Self::Denied {
            decided_by: None,
            reason: Some(reason.into()),
            metadata: Metadata::default(),
        }
    }

    /// Attach replacement arguments to an approved decision.
    #[must_use]
    pub fn with_override_arguments(self, arguments: Value) -> Self {
        match self {
            Self::Approved {
                decided_by,
                reason,
                metadata,
                ..
            } => Self::Approved {
                decided_by,
                reason,
                override_arguments: Some(arguments),
                metadata,
            },
            denied @ Self::Denied { .. } => denied,
        }
    }

    /// Convert this SDK decision into a durable approval decision.
    #[must_use]
    pub fn into_approval_decision(self) -> ApprovalDecision {
        let decided_at = Utc::now();
        match self {
            Self::Approved {
                decided_by,
                reason,
                override_arguments,
                mut metadata,
            } => {
                if let Some(arguments) = override_arguments {
                    metadata.insert("override_arguments".to_string(), arguments);
                }
                ApprovalDecision {
                    status: ApprovalStatus::Approved,
                    decided_by,
                    decided_at,
                    reason,
                    metadata,
                }
            }
            Self::Denied {
                decided_by,
                reason,
                metadata,
            } => ApprovalDecision {
                status: ApprovalStatus::Denied,
                decided_by,
                decided_at,
                reason,
                metadata,
            },
        }
    }
}

/// Durable deferred tool record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeferredToolRecord {
    /// Deferred record id.
    pub deferred_id: String,
    /// Monotonic optimistic-concurrency revision.
    #[serde(default = "initial_interaction_revision")]
    pub revision: u64,
    /// Session id.
    pub session_id: SessionId,
    /// Run id.
    pub run_id: RunId,
    /// Tool call id.
    pub tool_call_id: String,
    /// Tool name.
    pub tool_name: String,
    /// Arguments or request payload.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub request: Value,
    /// Deferred status.
    pub status: ExecutionStatus,
    /// Optional response payload.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub response: Value,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Update time.
    pub updated_at: DateTime<Utc>,
    /// Trace context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
    /// Deferred metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl starweaver_core::VersionedRecord for DeferredToolRecord {
    const SCHEMA: &'static str = "starweaver.session.deferred_tool_record";
    const ALLOW_BARE_V0: bool = true;
}

impl DeferredToolRecord {
    /// Build a deferred tool record.
    #[must_use]
    pub fn new(
        deferred_id: impl Into<String>,
        session_id: SessionId,
        run_id: RunId,
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            deferred_id: deferred_id.into(),
            revision: initial_interaction_revision(),
            session_id,
            run_id,
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            request: Value::Null,
            status: ExecutionStatus::Pending,
            response: Value::Null,
            created_at: now,
            updated_at: now,
            trace_context: TraceContext::default(),
            metadata: Metadata::default(),
        }
    }

    /// Build a durable deferred-tool record from a deferred tool return.
    #[must_use]
    pub fn from_tool_return(input: &ToolReturnRecordInput<'_>) -> Option<Self> {
        if input.control_flow() != Some("call_deferred") {
            return None;
        }
        let mut record = Self::new(
            input.deferred_id(),
            input.session_id.clone(),
            input.run_id.clone(),
            input.tool_call_id,
            input.tool_name,
        );
        record.request = input
            .metadata
            .get("deferred")
            .cloned()
            .unwrap_or(Value::Null);
        record.status = ExecutionStatus::Waiting;
        if let Some(trace_context) = input.trace_context {
            record.trace_context = trace_context.clone();
        }
        if let Some(policy) = &input.policy {
            record.metadata.insert("policy".to_string(), policy.clone());
        }
        Some(record)
    }
}

/// SDK-facing deferred tool request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeferredToolRequest {
    /// Deferred request id.
    pub deferred_id: String,
    /// Session id.
    pub session_id: SessionId,
    /// Run id.
    pub run_id: RunId,
    /// Tool call id.
    pub tool_call_id: String,
    /// Tool name.
    pub tool_name: String,
    /// Arguments requested by the model or host.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub arguments: Value,
    /// Trace context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
    /// Request metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl DeferredToolRequest {
    /// Convert a durable record into an SDK request facade.
    #[must_use]
    pub fn from_record(record: &DeferredToolRecord) -> Self {
        Self {
            deferred_id: record.deferred_id.clone(),
            session_id: record.session_id.clone(),
            run_id: record.run_id.clone(),
            tool_call_id: record.tool_call_id.clone(),
            tool_name: record.tool_name.clone(),
            arguments: record.request.clone(),
            trace_context: record.trace_context.clone(),
            metadata: record.metadata.clone(),
        }
    }

    /// Convert this request into a durable record.
    #[must_use]
    pub fn into_record(self) -> DeferredToolRecord {
        let mut record = DeferredToolRecord::new(
            self.deferred_id,
            self.session_id,
            self.run_id,
            self.tool_call_id,
            self.tool_name,
        );
        record.request = self.arguments;
        record.trace_context = self.trace_context;
        record.metadata = self.metadata;
        record
    }
}

/// Collection of deferred tool requests.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeferredToolRequests {
    /// Deferred requests.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requests: Vec<DeferredToolRequest>,
}

impl DeferredToolRequests {
    /// Build a request collection from durable records.
    #[must_use]
    pub fn from_records(records: &[DeferredToolRecord]) -> Self {
        Self {
            requests: records
                .iter()
                .map(DeferredToolRequest::from_record)
                .collect(),
        }
    }

    /// Return whether the collection is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }
}

/// SDK-facing deferred tool result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeferredToolResult {
    /// Deferred request id.
    pub deferred_id: String,
    /// Result status.
    pub status: ExecutionStatus,
    /// Response payload.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub response: Value,
    /// Result metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl DeferredToolResult {
    /// Build a completed deferred result.
    #[must_use]
    pub fn completed(deferred_id: impl Into<String>, response: Value) -> Self {
        Self {
            deferred_id: deferred_id.into(),
            status: ExecutionStatus::Completed,
            response,
            metadata: Metadata::default(),
        }
    }

    /// Build a failed deferred result.
    #[must_use]
    pub fn failed(deferred_id: impl Into<String>, response: Value) -> Self {
        Self {
            deferred_id: deferred_id.into(),
            status: ExecutionStatus::Failed,
            response,
            metadata: Metadata::default(),
        }
    }

    /// Build a cancelled deferred result.
    #[must_use]
    pub fn cancelled(deferred_id: impl Into<String>, response: Value) -> Self {
        Self {
            deferred_id: deferred_id.into(),
            status: ExecutionStatus::Cancelled,
            response,
            metadata: Metadata::default(),
        }
    }

    /// Attach result metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Apply this result to a durable deferred record.
    pub fn apply_to_record(self, record: &mut DeferredToolRecord) {
        record.status = self.status;
        record.response = self.response;
        record.updated_at = Utc::now();
        record.metadata.extend(self.metadata);
    }
}

/// Collection of deferred tool results.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeferredToolResults {
    /// Deferred results.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<DeferredToolResult>,
}

impl DeferredToolResults {
    /// Build a result collection.
    #[must_use]
    pub fn new(results: impl IntoIterator<Item = DeferredToolResult>) -> Self {
        Self {
            results: results.into_iter().collect(),
        }
    }

    /// Return whether the collection is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.results.is_empty()
    }
}
