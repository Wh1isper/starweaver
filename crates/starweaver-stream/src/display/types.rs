//! Display protocol types.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::{AgentId, Metadata, RunId, SessionId, TraceContext};
use starweaver_runtime::AgentStreamRecord;

/// AGUI-compatible display event type consumed by product renderers and clients.
///
/// Starweaver keeps one wire event shape for CLI JSONL, service transports,
/// replay archives, and terminal restore. The serialized event type follows AGUI lifecycle names
/// where a standard AGUI concept exists, and uses Starweaver extension names for
/// durable runtime-specific events.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum DisplayMessageKind {
    /// Run accepted and waiting for execution.
    #[serde(rename = "RUN_QUEUED", alias = "run_queued")]
    RunQueued,
    /// Execution started.
    #[serde(rename = "RUN_STARTED", alias = "run_started")]
    RunStarted,
    /// Assistant text block started.
    #[serde(rename = "TEXT_MESSAGE_START", alias = "assistant_text_start")]
    AssistantTextStart,
    /// Assistant streaming text delta.
    #[serde(rename = "TEXT_MESSAGE_CONTENT", alias = "assistant_text_delta")]
    AssistantTextDelta,
    /// Assistant text block completed.
    #[serde(rename = "TEXT_MESSAGE_END", alias = "assistant_text_end")]
    AssistantTextEnd,
    /// Tool call started.
    #[serde(rename = "TOOL_CALL_START", alias = "tool_call_start")]
    ToolCallStart,
    /// Tool call streaming arguments delta.
    #[serde(rename = "TOOL_CALL_ARGS", alias = "tool_call_delta")]
    ToolCallDelta,
    /// Tool call completed.
    #[serde(rename = "TOOL_CALL_END", alias = "tool_call_end")]
    ToolCallEnd,
    /// Tool result or error preview.
    #[serde(rename = "TOOL_CALL_RESULT", alias = "tool_result")]
    ToolResult,
    /// Approval requested.
    #[serde(rename = "APPROVAL_REQUESTED", alias = "approval_requested")]
    ApprovalRequested,
    /// Approval decision recorded.
    #[serde(rename = "APPROVAL_RESOLVED", alias = "approval_resolved")]
    ApprovalResolved,
    /// Runtime checkpoint emitted.
    #[serde(rename = "CHECKPOINT", alias = "checkpoint")]
    Checkpoint,
    /// Subagent started.
    #[serde(rename = "SUBAGENT_STARTED", alias = "subagent_started")]
    SubagentStarted,
    /// Subagent completed.
    #[serde(rename = "SUBAGENT_COMPLETED", alias = "subagent_completed")]
    SubagentCompleted,
    /// History or context compaction started.
    #[serde(rename = "COMPACTION_STARTED", alias = "compaction_started")]
    CompactionStarted,
    /// History or context compaction completed.
    #[serde(rename = "COMPACTION_COMPLETED", alias = "compaction_completed")]
    CompactionCompleted,
    /// History or context compaction failed.
    #[serde(rename = "COMPACTION_FAILED", alias = "compaction_failed")]
    CompactionFailed,
    /// Progress handoff summary started.
    #[serde(rename = "HANDOFF_STARTED", alias = "handoff_started")]
    HandoffStarted,
    /// Progress handoff summary completed.
    #[serde(rename = "HANDOFF_COMPLETED", alias = "handoff_completed")]
    HandoffCompleted,
    /// Progress handoff summary failed.
    #[serde(rename = "HANDOFF_FAILED", alias = "handoff_failed")]
    HandoffFailed,
    /// Steering message was submitted to a running agent.
    #[serde(rename = "STEERING_SUBMITTED", alias = "steering_submitted")]
    SteeringSubmitted,
    /// Steering message was received by a running agent.
    #[serde(rename = "STEERING_RECEIVED", alias = "steering_received")]
    SteeringReceived,
    /// Full task board snapshot.
    #[serde(
        rename = "TASK_SNAPSHOT",
        alias = "task_snapshot",
        alias = "TASK_PANEL",
        alias = "task_panel"
    )]
    TaskSnapshot,
    /// Run completed successfully.
    #[serde(rename = "RUN_FINISHED", alias = "run_completed")]
    RunCompleted,
    /// Run failed.
    #[serde(rename = "RUN_ERROR", alias = "run_failed")]
    RunFailed,
    /// Run cancelled or interrupted.
    #[serde(rename = "RUN_CANCELLED", alias = "run_cancelled")]
    RunCancelled,
}

impl DisplayMessageKind {
    /// Returns true when this event terminates a run stream.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::RunCompleted | Self::RunFailed | Self::RunCancelled
        )
    }
}

/// Visibility for display messages.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayVisibility {
    /// Show to normal users.
    #[default]
    Public,
    /// Show in diagnostics and trace views.
    Diagnostic,
    /// Hide unless internal debugging is requested.
    Internal,
}

/// AGUI-compatible Starweaver display event.
///
/// This is the durable and transport-level display protocol. CLI headless mode
/// writes one `DisplayMessage` JSON object per line. Service transports can wrap
/// the same object in SSE frames. Product renderers can replay the same records into TUI
/// or web view state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DisplayMessage {
    /// Display protocol schema id.
    #[serde(default = "default_display_schema")]
    pub schema: String,
    /// Monotonic sequence within the replay scope.
    pub sequence: usize,
    /// Session id.
    pub session_id: SessionId,
    /// Run id.
    pub run_id: RunId,
    /// Agent id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<AgentId>,
    /// Agent display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// Event timestamp.
    pub timestamp: DateTime<Utc>,
    /// Trace context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
    /// AGUI-compatible event type.
    #[serde(rename = "type", alias = "kind")]
    pub kind: DisplayMessageKind,
    /// Canonical structured event data.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub payload: Value,
    /// Compact renderer-friendly summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// Visibility class.
    #[serde(default)]
    pub visibility: DisplayVisibility,
    /// Application metadata and Starweaver extensions.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl DisplayMessage {
    /// Current display protocol schema id.
    pub const SCHEMA: &'static str = "starweaver.display.v1";

    /// Build a display message.
    #[must_use]
    pub fn new(
        sequence: usize,
        session_id: SessionId,
        run_id: RunId,
        kind: DisplayMessageKind,
    ) -> Self {
        Self {
            schema: Self::SCHEMA.to_string(),
            sequence,
            session_id,
            run_id,
            agent_id: None,
            agent_name: None,
            timestamp: Utc::now(),
            trace_context: TraceContext::default(),
            kind,
            payload: Value::Null,
            preview: None,
            visibility: DisplayVisibility::Public,
            metadata: Metadata::default(),
        }
    }

    /// Attach payload.
    #[must_use]
    pub fn with_payload(mut self, payload: Value) -> Self {
        self.payload = payload;
        self
    }

    /// Attach preview text.
    #[must_use]
    pub fn with_preview(mut self, preview: impl Into<String>) -> Self {
        self.preview = Some(preview.into());
        self
    }

    /// Attach trace context.
    #[must_use]
    pub fn with_trace_context(mut self, trace_context: TraceContext) -> Self {
        self.trace_context = trace_context;
        self
    }

    /// Returns true when this event terminates a run stream.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        self.kind.is_terminal()
    }

    /// Render one display JSONL line.
    ///
    /// # Errors
    ///
    /// Returns a serialization error when the event cannot be encoded as JSON.
    pub fn to_jsonl_line(&self) -> serde_json::Result<String> {
        serde_json::to_string(self).map(|line| format!("{line}\n"))
    }
}

/// Context used while projecting runtime stream records into display messages.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DisplayProjectionContext {
    /// Session id.
    pub session_id: SessionId,
    /// Run id.
    pub run_id: RunId,
    /// Agent id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<AgentId>,
    /// Agent display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// Trace context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
}

impl DisplayProjectionContext {
    /// Build projection context for one session and run.
    #[must_use]
    pub fn new(session_id: SessionId, run_id: RunId) -> Self {
        Self {
            session_id,
            run_id,
            agent_id: None,
            agent_name: None,
            trace_context: TraceContext::default(),
        }
    }
}

/// Runtime stream to display message projector.
#[async_trait]
pub trait DisplayMessageProjector: Send + Sync {
    /// Project one runtime stream record into zero or more display messages.
    async fn project(
        &self,
        context: &DisplayProjectionContext,
        record: &AgentStreamRecord,
    ) -> Vec<DisplayMessage>;
}

pub(super) fn default_display_schema() -> String {
    DisplayMessage::SCHEMA.to_string()
}
