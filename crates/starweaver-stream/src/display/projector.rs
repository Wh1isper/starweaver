//! Runtime stream to display message projection.

mod model;
mod run;
mod tool;

use async_trait::async_trait;
use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord};

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
        let run_id = context.run_id.clone();
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
            AgentStreamEvent::Checkpoint { node, step } => {
                vec![run::project_checkpoint(
                    context,
                    record.sequence,
                    run_id,
                    *node,
                    *step,
                )]
            }
            AgentStreamEvent::Custom { event } => project_custom_event(
                context,
                record.sequence,
                run_id,
                &event.kind,
                &event.payload,
            ),
            AgentStreamEvent::SteeringGuard { step, prompt } => {
                vec![run::project_steering_guard(
                    context,
                    record.sequence,
                    run_id,
                    *step,
                    prompt,
                )]
            }
            AgentStreamEvent::RunComplete { output, .. } => {
                vec![run::project_run_completed(
                    context,
                    record.sequence,
                    run_id,
                    output,
                )]
            }
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
            AgentStreamEvent::Suspended { reason, node } => {
                vec![run::project_run_cancelled(
                    context,
                    record.sequence,
                    run_id,
                    *node,
                    reason,
                )]
            }
            _ => Vec::new(),
        }
    }
}
