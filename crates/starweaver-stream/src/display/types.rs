//! Display protocol types.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::{AgentId, Metadata, RunId, SessionId, TraceContext};
use starweaver_runtime::AgentStreamRecord;

/// Display event type consumed by product renderers and AGUI adapters.
///
/// Starweaver keeps one wire event shape for CLI JSONL, service transports,
/// replay archives, and terminal restore. The serialized event type follows AGUI lifecycle names
/// where a standard AGUI concept exists, and uses Starweaver extension names for
/// durable runtime-specific events.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum DisplayMessageKind {
    /// Run accepted and waiting for execution.
    #[serde(rename = "RUN_QUEUED")]
    RunQueued,
    /// Execution started.
    #[serde(rename = "RUN_STARTED")]
    RunStarted,
    /// Assistant text block started.
    #[serde(rename = "TEXT_MESSAGE_START")]
    AssistantTextStart,
    /// Assistant streaming text delta.
    #[serde(rename = "TEXT_MESSAGE_CONTENT")]
    AssistantTextDelta,
    /// Assistant text block completed.
    #[serde(rename = "TEXT_MESSAGE_END")]
    AssistantTextEnd,
    /// Tool call started.
    #[serde(rename = "TOOL_CALL_START")]
    ToolCallStart,
    /// Tool call streaming arguments delta.
    #[serde(rename = "TOOL_CALL_ARGS")]
    ToolCallDelta,
    /// Tool call completed.
    #[serde(rename = "TOOL_CALL_END")]
    ToolCallEnd,
    /// Tool result or error preview.
    #[serde(rename = "TOOL_CALL_RESULT")]
    ToolResult,
    /// Tool availability filtering skipped one or more tools.
    #[serde(rename = "TOOLS_UNAVAILABLE")]
    ToolsUnavailable,
    /// Dynamic tool-search loaded tools or namespaces for the next model turn.
    #[serde(rename = "TOOL_SEARCH_LOADED")]
    ToolSearchLoaded,
    /// Dynamic tool-search initialization report.
    #[serde(rename = "TOOL_SEARCH_INITIALIZED")]
    ToolSearchInitialized,
    /// Dynamic tool-search refresh report.
    #[serde(rename = "TOOL_SEARCH_REFRESHED")]
    ToolSearchRefreshed,
    /// Dynamic tool-search loaded state was invalidated by host code.
    #[serde(rename = "TOOL_SEARCH_INVALIDATED")]
    ToolSearchInvalidated,
    /// Dynamic tool-search query failed validation.
    #[serde(rename = "TOOL_SEARCH_FAILED")]
    ToolSearchFailed,
    /// Dynamic tool-search query returned no matches.
    #[serde(rename = "TOOL_SEARCH_NO_MATCH")]
    ToolSearchNoMatch,
    /// Toolset initialized for a runtime context.
    #[serde(rename = "TOOLSET_INITIALIZED")]
    ToolsetInitialized,
    /// Toolset unavailable for a runtime context.
    #[serde(rename = "TOOLSET_UNAVAILABLE")]
    ToolsetUnavailable,
    /// Toolset failed while preparing for a runtime context.
    #[serde(rename = "TOOLSET_FAILED")]
    ToolsetFailed,
    /// Toolset refreshed its runtime-context inventory.
    #[serde(rename = "TOOLSET_REFRESHED")]
    ToolsetRefreshed,
    /// Toolset exited a runtime context.
    #[serde(rename = "TOOLSET_CLOSED")]
    ToolsetClosed,
    /// Approval requested.
    #[serde(rename = "APPROVAL_REQUESTED")]
    ApprovalRequested,
    /// Approval decision recorded.
    #[serde(rename = "APPROVAL_RESOLVED")]
    ApprovalResolved,
    /// HITL decisions were resolved into tool returns.
    #[serde(rename = "HITL_RESOLVED")]
    HitlResolved,
    /// HITL decision diagnostic emitted when supplied decisions cannot be applied.
    #[serde(rename = "HITL_DIAGNOSTIC")]
    HitlDiagnostic,
    /// Runtime checkpoint emitted.
    #[serde(rename = "CHECKPOINT")]
    Checkpoint,
    /// Skill scan report emitted.
    #[serde(rename = "SKILLS_SCANNED")]
    SkillsScanned,
    /// Skill package activated.
    #[serde(rename = "SKILL_ACTIVATED")]
    SkillActivated,
    /// Skill registry reload report emitted.
    #[serde(rename = "SKILLS_RELOADED")]
    SkillsReloaded,
    /// Subagent started.
    #[serde(rename = "SUBAGENT_STARTED")]
    SubagentStarted,
    /// Subagent completed.
    #[serde(rename = "SUBAGENT_COMPLETED")]
    SubagentCompleted,
    /// Subagent failed.
    #[serde(rename = "SUBAGENT_FAILED")]
    SubagentFailed,
    /// History or context compaction started.
    #[serde(rename = "COMPACTION_STARTED")]
    CompactionStarted,
    /// History or context compaction completed.
    #[serde(rename = "COMPACTION_COMPLETED")]
    CompactionCompleted,
    /// History or context compaction failed.
    #[serde(rename = "COMPACTION_FAILED")]
    CompactionFailed,
    /// Progress handoff summary started.
    #[serde(rename = "HANDOFF_STARTED")]
    HandoffStarted,
    /// Progress handoff summary completed.
    #[serde(rename = "HANDOFF_COMPLETED")]
    HandoffCompleted,
    /// Progress handoff summary failed.
    #[serde(rename = "HANDOFF_FAILED")]
    HandoffFailed,
    /// Steering message was submitted to a running agent.
    #[serde(rename = "STEERING_SUBMITTED")]
    SteeringSubmitted,
    /// Steering message was received by a running agent.
    #[serde(rename = "STEERING_RECEIVED")]
    SteeringReceived,
    /// Runtime goal-mode iteration requested another model attempt.
    #[serde(rename = "GOAL_ITERATION")]
    GoalIteration,
    /// Runtime goal-mode stopped.
    #[serde(rename = "GOAL_COMPLETED")]
    GoalCompleted,
    /// Full task board snapshot.
    #[serde(rename = "TASK_SNAPSHOT")]
    TaskSnapshot,
    /// Task workflow event other than a full snapshot.
    #[serde(rename = "TASK_EVENT")]
    TaskEvent,
    /// Note workflow event.
    #[serde(rename = "NOTE_EVENT")]
    NoteEvent,
    /// File workflow event.
    #[serde(rename = "FILE_EVENT")]
    FileEvent,
    /// Media workflow event.
    #[serde(rename = "MEDIA_EVENT")]
    MediaEvent,
    /// Host operation workflow event.
    #[serde(rename = "HOST_OPERATION")]
    HostOperation,
    /// Run completed successfully.
    #[serde(rename = "RUN_FINISHED")]
    RunCompleted,
    /// Run failed.
    #[serde(rename = "RUN_ERROR")]
    RunFailed,
    /// Run cancelled or interrupted.
    #[serde(rename = "RUN_CANCELLED")]
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

/// Starweaver display event with AGUI lifecycle event naming where applicable.
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
    /// Display event type.
    #[serde(rename = "type")]
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
