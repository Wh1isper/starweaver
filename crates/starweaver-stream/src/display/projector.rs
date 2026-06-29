//! Runtime stream to display message projection.

mod model;
mod run;
mod tool;

use async_trait::async_trait;
use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord, AgentStreamSource};

use super::custom::project_custom_event;
use super::{DisplayMessage, DisplayMessageProjector, DisplayProjectionContext};

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
        let source_context = record
            .source
            .as_ref()
            .map(|source| source_projection_context(context, source));
        let context = source_context.as_ref().unwrap_or(context);
        let run_id = context.run_id.clone();
        let mut messages = project_record_event(context, record, run_id);
        if let Some(source) = record.source.as_ref() {
            apply_source_attribution(&mut messages, source);
        }
        messages
    }
}

fn source_projection_context(
    context: &DisplayProjectionContext,
    source: &AgentStreamSource,
) -> DisplayProjectionContext {
    DisplayProjectionContext {
        session_id: context.session_id.clone(),
        run_id: source
            .run_id
            .clone()
            .unwrap_or_else(|| context.run_id.clone()),
        agent_id: Some(source.agent_id.clone()),
        agent_name: Some(source.agent_name.clone()),
        trace_context: context.trace_context.clone(),
    }
}

fn project_record_event(
    context: &DisplayProjectionContext,
    record: &AgentStreamRecord,
    run_id: starweaver_core::RunId,
) -> Vec<DisplayMessage> {
    match &record.event {
        AgentStreamEvent::RunStart {
            conversation_id, ..
        } => vec![run::project_run_started(
            context,
            record,
            run_id,
            conversation_id.as_str(),
        )],
        AgentStreamEvent::ModelStream { event, .. } => {
            model::project_model_stream(context, record.sequence, run_id, event)
        }
        AgentStreamEvent::ModelResponse { response, .. } => {
            model::project_model_response(context, record.sequence, &run_id, response)
        }
        AgentStreamEvent::ToolCall { call, .. } => {
            tool::project_tool_call_messages(context, record.sequence, run_id, call, false)
        }
        AgentStreamEvent::ToolReturn { tool_return, .. } => {
            tool::project_tool_return_messages(context, record.sequence, run_id, tool_return)
        }
        AgentStreamEvent::Checkpoint { node, step } => vec![run::project_checkpoint(
            context,
            record.sequence,
            run_id,
            *node,
            *step,
        )],
        AgentStreamEvent::Custom { event } => project_custom_event(
            context,
            record.sequence,
            run_id,
            &event.kind,
            &event.payload,
            &event.metadata,
        ),
        AgentStreamEvent::SteeringGuard { step, prompt } => vec![run::project_steering_guard(
            context,
            record.sequence,
            run_id,
            *step,
            prompt,
        )],
        AgentStreamEvent::RunComplete { output, .. } => vec![run::project_run_completed(
            context,
            record.sequence,
            run_id,
            output,
        )],
        AgentStreamEvent::RunFailed {
            error_kind,
            message,
            ..
        } => vec![run::project_run_failed(
            context,
            record.sequence,
            run_id,
            error_kind,
            message,
        )],
        AgentStreamEvent::Suspended { reason, node } => vec![run::project_run_cancelled(
            context,
            record.sequence,
            run_id,
            *node,
            reason,
        )],
        _ => Vec::new(),
    }
}

fn apply_source_attribution(messages: &mut [DisplayMessage], source: &AgentStreamSource) {
    for message in messages {
        message.agent_id = Some(source.agent_id.clone());
        message.agent_name = Some(source.agent_name.clone());
        message
            .metadata
            .insert("source_kind".to_string(), serde_json::json!(source.kind));
        message.metadata.insert(
            "source_agent_id".to_string(),
            serde_json::json!(source.agent_id.as_str()),
        );
        message.metadata.insert(
            "source_agent_name".to_string(),
            serde_json::json!(source.agent_name),
        );
        if let Some(task_id) = source.task_id.as_ref() {
            message.metadata.insert(
                "source_task_id".to_string(),
                serde_json::json!(task_id.as_str()),
            );
        }
        message.metadata.insert(
            "source_sequence".to_string(),
            serde_json::json!(source.source_sequence),
        );
    }
}
