//! Small helpers used by the agent run loop.

use starweaver_model::{ModelMessage, ModelRequest, ModelRequestPart, ModelResponse};

use crate::{
    agent::{runtime_helpers::tool_return_media_prompt, Agent, AgentError},
    run::AgentRunState,
};

impl Agent {
    pub(in crate::agent) fn usage_model_id(&self, response: &ModelResponse) -> String {
        response.model_name.clone().unwrap_or_else(|| {
            self.model.provider_name().map_or_else(
                || self.model.model_name().to_string(),
                |provider| format!("{provider}:{}", self.model.model_name()),
            )
        })
    }
}

pub(in crate::agent) const fn agent_error_kind(error: &AgentError) -> &'static str {
    match error {
        AgentError::Model(_) => "model_error",
        AgentError::Capability(_) => "capability_error",
        AgentError::CapabilityOrder(_) => "capability_order_error",
        AgentError::StructuredOutput(_) => "structured_output_error",
        AgentError::DynamicInstruction(_) => "dynamic_instruction_error",
        AgentError::OutputRetryLimitExceeded { .. } => "output_retry_limit_exceeded",
        AgentError::ToolRetryLimitExceeded { .. } => "tool_retry_limit_exceeded",
        AgentError::StepLimitExceeded { .. } => "step_limit_exceeded",
        AgentError::UsageLimit(_) => "usage_limit_exceeded",
        AgentError::ExecutionSuspended { .. } => "execution_suspended",
        AgentError::Executor(_) => "executor_error",
        AgentError::ToolCallsRequireTools => "tool_calls_require_tools",
    }
}

pub(in crate::agent) fn preserve_pending_tool_returns_for_resume(state: &mut AgentRunState) {
    if state.pending_tool_returns.is_empty() {
        return;
    }
    let mut parts = Vec::new();
    for tool_return in &state.pending_tool_returns {
        parts.push(ModelRequestPart::ToolReturn(tool_return.clone()));
        if let Some(media_prompt) = tool_return_media_prompt(tool_return) {
            parts.push(media_prompt);
        }
    }
    state
        .message_history
        .push(ModelMessage::Request(ModelRequest {
            parts,
            timestamp: None,
            instructions: None,
            run_id: Some(state.run_id.clone()),
            conversation_id: Some(state.conversation_id.clone()),
            metadata: serde_json::json!({
                "starweaver.failed.pending_tool_returns": true,
            })
            .as_object()
            .cloned()
            .unwrap_or_default(),
        }));
    state.pending_tool_returns.clear();
}
