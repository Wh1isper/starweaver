//! Approval and deferred tool records.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::{Metadata, RunId, SessionId, TraceContext};

use crate::records::ExecutionStatus;

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
            session_id,
            run_id,
            action_id: action_id.into(),
            action_name: action_name.into(),
            request: Value::Null,
            status: ApprovalStatus::Pending,
            decision: None,
            created_at: now,
            updated_at: now,
            trace_context: TraceContext::default(),
            metadata: Metadata::default(),
        }
    }
}

/// Durable deferred tool record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeferredToolRecord {
    /// Deferred record id.
    pub deferred_id: String,
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
}
