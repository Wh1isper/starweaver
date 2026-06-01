//! AGUI-compatible display message protocol and projection helpers.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use starweaver_core::{AgentId, Metadata, RunId, SessionId, TraceContext};
use starweaver_model::{ToolCallPart, ToolReturnPart};
use starweaver_runtime::{
    AgentExecutionNode, AgentStreamEvent, AgentStreamRecord, ModelResponseStreamEvent,
};

/// AGUI-compatible display event type consumed by product renderers and clients.
///
/// Starweaver keeps one wire event shape for CLI JSONL, Claw SSE, replay archives,
/// and terminal restore. The serialized event type follows AGUI lifecycle names
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
/// writes one `DisplayMessage` JSON object per line. Claw can wrap the same
/// object in SSE frames. Product renderers can replay the same records into TUI
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

/// Default display message projector for runtime stream records.
#[derive(Clone, Debug, Default)]
pub struct DefaultDisplayMessageProjector;

#[async_trait]
impl DisplayMessageProjector for DefaultDisplayMessageProjector {
    async fn project(
        &self,
        context: &DisplayProjectionContext,
        record: &AgentStreamRecord,
    ) -> Vec<DisplayMessage> {
        let run_id = run_id_for_event(&record.event).unwrap_or_else(|| context.run_id.clone());
        let message = match &record.event {
            AgentStreamEvent::RunStart {
                conversation_id, ..
            } => project_run_started(context, record, run_id, conversation_id.as_str()),
            AgentStreamEvent::ModelStream { event, .. } => {
                project_model_stream(context, record.sequence, run_id, event)
            }
            AgentStreamEvent::ToolCall { call, .. } => {
                project_tool_call(context, record.sequence, run_id, call)
            }
            AgentStreamEvent::ToolReturn { tool_return, .. } => {
                project_tool_return(context, record.sequence, run_id, tool_return)
            }
            AgentStreamEvent::Checkpoint { node, step } => {
                project_checkpoint(context, record.sequence, run_id, *node, *step)
            }
            AgentStreamEvent::RunComplete { output, .. } => {
                project_run_completed(context, record.sequence, run_id, output)
            }
            AgentStreamEvent::Suspended { reason, node } => {
                project_run_cancelled(context, record.sequence, run_id, *node, reason)
            }
            _ => return Vec::new(),
        };
        vec![message]
    }
}

fn project_run_started(
    context: &DisplayProjectionContext,
    sequence: &AgentStreamRecord,
    run_id: RunId,
    conversation_id: &str,
) -> DisplayMessage {
    DisplayMessage::new(
        sequence.sequence,
        context.session_id.clone(),
        run_id,
        DisplayMessageKind::RunStarted,
    )
    .with_payload(json!({"conversation_id": conversation_id}))
    .with_preview("run started")
}

fn project_model_stream(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    event: &ModelResponseStreamEvent,
) -> DisplayMessage {
    match event {
        ModelResponseStreamEvent::PartStart(part) => DisplayMessage::new(
            sequence,
            context.session_id.clone(),
            run_id,
            DisplayMessageKind::AssistantTextStart,
        )
        .with_payload(json!({
            "message_id": format!("message-{}", part.index),
            "role": "assistant",
            "part_index": part.index,
            "part_kind": part.part_kind,
        })),
        ModelResponseStreamEvent::PartDelta(delta) => DisplayMessage::new(
            sequence,
            context.session_id.clone(),
            run_id,
            DisplayMessageKind::AssistantTextDelta,
        )
        .with_payload(json!({
            "message_id": format!("message-{}", delta.index),
            "part_index": delta.index,
            "delta": delta.delta,
        }))
        .with_preview(delta.delta.clone()),
        ModelResponseStreamEvent::PartEnd(part) => DisplayMessage::new(
            sequence,
            context.session_id.clone(),
            run_id,
            DisplayMessageKind::AssistantTextEnd,
        )
        .with_payload(json!({
            "message_id": format!("message-{}", part.index),
            "part_index": part.index,
        })),
        ModelResponseStreamEvent::FinalResult(response) => DisplayMessage::new(
            sequence,
            context.session_id.clone(),
            run_id,
            DisplayMessageKind::AssistantTextEnd,
        )
        .with_payload(json!({
            "message_id": "final",
            "text": response.text_output(),
        }))
        .with_preview(response.text_output()),
    }
}

fn project_tool_call(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    call: &ToolCallPart,
) -> DisplayMessage {
    DisplayMessage::new(
        sequence,
        context.session_id.clone(),
        run_id,
        DisplayMessageKind::ToolCallStart,
    )
    .with_payload(
        json!({"tool_call_id": call.id, "tool_name": call.name, "arguments": call.arguments}),
    )
    .with_preview(format!("tool call {}", call.name))
}

fn project_tool_return(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    tool_return: &ToolReturnPart,
) -> DisplayMessage {
    DisplayMessage::new(
        sequence,
        context.session_id.clone(),
        run_id,
        DisplayMessageKind::ToolResult,
    )
    .with_payload(json!({
        "tool_call_id": tool_return.tool_call_id,
        "tool_name": tool_return.name,
        "content": tool_return.content,
        "is_error": tool_return.is_error,
    }))
    .with_preview(format!("tool result {}", tool_return.name))
}

fn project_checkpoint(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    node: AgentExecutionNode,
    step: usize,
) -> DisplayMessage {
    DisplayMessage::new(
        sequence,
        context.session_id.clone(),
        run_id,
        DisplayMessageKind::Checkpoint,
    )
    .with_payload(json!({"node": node, "step": step}))
    .with_preview(format!("checkpoint {node:?}"))
}

fn project_run_completed(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    output: &str,
) -> DisplayMessage {
    DisplayMessage::new(
        sequence,
        context.session_id.clone(),
        run_id,
        DisplayMessageKind::RunCompleted,
    )
    .with_payload(json!({"output": output}))
    .with_preview(output.to_string())
}

fn project_run_cancelled(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    node: AgentExecutionNode,
    reason: &str,
) -> DisplayMessage {
    DisplayMessage::new(
        sequence,
        context.session_id.clone(),
        run_id,
        DisplayMessageKind::RunCancelled,
    )
    .with_payload(json!({"node": node, "reason": reason}))
    .with_preview(reason.to_string())
}

fn run_id_for_event(event: &AgentStreamEvent) -> Option<RunId> {
    match event {
        AgentStreamEvent::RunStart { run_id, .. }
        | AgentStreamEvent::RunComplete { run_id, .. } => Some(run_id.clone()),
        _ => None,
    }
}

fn default_display_schema() -> String {
    DisplayMessage::SCHEMA.to_string()
}
