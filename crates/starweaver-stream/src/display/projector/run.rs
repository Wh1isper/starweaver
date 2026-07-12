//! Run lifecycle display projection.

use serde_json::json;
use starweaver_core::{AgentExecutionNode, RunId};

use crate::AgentStreamRecord;

use super::super::{DisplayMessage, DisplayMessageKind, DisplayProjectionContext};

pub(super) fn project_run_started(
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

pub(super) fn project_run_failed(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    error_kind: &str,
    message: &str,
) -> DisplayMessage {
    DisplayMessage::new(
        sequence,
        context.session_id.clone(),
        run_id,
        DisplayMessageKind::RunFailed,
    )
    .with_payload(json!({"error_kind": error_kind, "error": message}))
    .with_preview(message.to_string())
}

pub(super) fn project_checkpoint(
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

pub(super) fn project_steering_guard(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    step: usize,
    prompt: &str,
) -> DisplayMessage {
    DisplayMessage::new(
        sequence,
        context.session_id.clone(),
        run_id,
        DisplayMessageKind::Checkpoint,
    )
    .with_payload(json!({"step": step, "kind": "steering_guard", "prompt": prompt}))
    .with_preview("steering update pending")
}

pub(super) fn project_run_completed(
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

pub(super) fn project_run_terminal_cancelled(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    reason: &str,
) -> DisplayMessage {
    DisplayMessage::new(
        sequence,
        context.session_id.clone(),
        run_id,
        DisplayMessageKind::RunCancelled,
    )
    .with_payload(json!({"reason": reason}))
    .with_preview(reason.to_string())
}

pub(super) fn project_run_cancelled(
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
