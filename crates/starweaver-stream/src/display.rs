//! Display message protocol and projection helpers.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use starweaver_core::{AgentId, Metadata, RunId, SessionId, TraceContext};
use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord, ModelResponseStreamEvent};

/// Display message kind consumed by product renderers.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayMessageKind {
    /// Run accepted and waiting for execution.
    RunQueued,
    /// Execution started.
    RunStarted,
    /// Assistant text block started.
    AssistantTextStart,
    /// Assistant streaming text delta.
    AssistantTextDelta,
    /// Assistant text block completed.
    AssistantTextEnd,
    /// Tool call started.
    ToolCallStart,
    /// Tool call streaming delta.
    ToolCallDelta,
    /// Tool call completed.
    ToolCallEnd,
    /// Tool result or error preview.
    ToolResult,
    /// Approval requested.
    ApprovalRequested,
    /// Approval decision recorded.
    ApprovalResolved,
    /// Runtime checkpoint emitted.
    Checkpoint,
    /// Subagent started.
    SubagentStarted,
    /// Subagent completed.
    SubagentCompleted,
    /// History or context compaction started.
    CompactionStarted,
    /// History or context compaction completed.
    CompactionCompleted,
    /// Run completed successfully.
    RunCompleted,
    /// Run failed.
    RunFailed,
    /// Run cancelled or interrupted.
    RunCancelled,
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

/// Renderer-neutral semantic execution event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DisplayMessage {
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
    /// Display message kind.
    pub kind: DisplayMessageKind,
    /// Canonical structured projection.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub payload: Value,
    /// Compact renderer-friendly summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// Visibility class.
    #[serde(default)]
    pub visibility: DisplayVisibility,
    /// Application metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl DisplayMessage {
    /// Build a display message.
    #[must_use]
    pub fn new(
        sequence: usize,
        session_id: SessionId,
        run_id: RunId,
        kind: DisplayMessageKind,
    ) -> Self {
        Self {
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
            AgentStreamEvent::RunStart { conversation_id, .. } => DisplayMessage::new(
                record.sequence,
                context.session_id.clone(),
                run_id,
                DisplayMessageKind::RunStarted,
            )
            .with_payload(json!({"conversation_id": conversation_id.as_str()}))
            .with_preview("run started"),
            AgentStreamEvent::ModelStream { event, .. } => match event {
                ModelResponseStreamEvent::PartStart(part) => DisplayMessage::new(
                    record.sequence,
                    context.session_id.clone(),
                    run_id,
                    DisplayMessageKind::AssistantTextStart,
                )
                .with_payload(json!({"part_index": part.index, "part_kind": part.part_kind})),
                ModelResponseStreamEvent::PartDelta(delta) => DisplayMessage::new(
                    record.sequence,
                    context.session_id.clone(),
                    run_id,
                    DisplayMessageKind::AssistantTextDelta,
                )
                .with_payload(json!({"part_index": delta.index, "delta": delta.delta}))
                .with_preview(delta.delta.clone()),
                ModelResponseStreamEvent::PartEnd(part) => DisplayMessage::new(
                    record.sequence,
                    context.session_id.clone(),
                    run_id,
                    DisplayMessageKind::AssistantTextEnd,
                )
                .with_payload(json!({"part_index": part.index})),
                ModelResponseStreamEvent::FinalResult(response) => DisplayMessage::new(
                    record.sequence,
                    context.session_id.clone(),
                    run_id,
                    DisplayMessageKind::AssistantTextEnd,
                )
                .with_payload(json!({"text": response.text_output()}))
                .with_preview(response.text_output()),
            },
            AgentStreamEvent::ToolCall { call, .. } => DisplayMessage::new(
                record.sequence,
                context.session_id.clone(),
                run_id,
                DisplayMessageKind::ToolCallStart,
            )
            .with_payload(json!({"tool_call_id": call.id, "tool_name": call.name, "arguments": call.arguments}))
            .with_preview(format!("tool call {}", call.name)),
            AgentStreamEvent::ToolReturn { tool_return, .. } => DisplayMessage::new(
                record.sequence,
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
            .with_preview(format!("tool result {}", tool_return.name)),
            AgentStreamEvent::Checkpoint { node, step } => DisplayMessage::new(
                record.sequence,
                context.session_id.clone(),
                run_id,
                DisplayMessageKind::Checkpoint,
            )
            .with_payload(json!({"node": node, "step": step}))
            .with_preview(format!("checkpoint {node:?}")),
            AgentStreamEvent::RunComplete { output, .. } => DisplayMessage::new(
                record.sequence,
                context.session_id.clone(),
                run_id,
                DisplayMessageKind::RunCompleted,
            )
            .with_payload(json!({"output": output}))
            .with_preview(output.clone()),
            AgentStreamEvent::Suspended { reason, node } => DisplayMessage::new(
                record.sequence,
                context.session_id.clone(),
                run_id,
                DisplayMessageKind::RunCancelled,
            )
            .with_payload(json!({"node": node, "reason": reason}))
            .with_preview(reason.clone()),
            _ => return Vec::new(),
        };
        vec![message]
    }
}

fn run_id_for_event(event: &AgentStreamEvent) -> Option<RunId> {
    match event {
        AgentStreamEvent::RunStart { run_id, .. }
        | AgentStreamEvent::RunComplete { run_id, .. } => Some(run_id.clone()),
        _ => None,
    }
}
