//! Small helpers used by the agent run loop.

use std::time::Duration;

use starweaver_model::{
    ModelMessage, ModelRequest, ModelRequestPart, ModelResponse, ToolReturnPart,
};

use starweaver_context::AgentContext;
use starweaver_tools::{TOOL_METADATA_CONTEXT_MANAGEMENT_KEY, ToolRegistry};

use crate::{
    agent::{Agent, AgentError, runtime_helpers::tool_return_media_prompt},
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

    pub(in crate::agent) async fn prepare_run_tools(
        &self,
        context: &mut AgentContext,
        enter_toolsets: bool,
    ) -> Result<ToolRegistry, AgentError> {
        let mut tools = self.tools.clone();
        for toolset in &self.toolsets {
            let result = if enter_toolsets {
                tools.insert_toolset_with_context(context, toolset).await
            } else {
                tools.refresh_toolset_with_context(context, toolset).await
            };
            result.map_err(|error| AgentError::Capability(error.to_string()))?;
        }
        for name in &self.denied_tool_names {
            tools.remove(name);
        }
        context.runtime.context_manage_tool_names = tools
            .tools()
            .into_iter()
            .filter(|tool| {
                tool.metadata()
                    .get(TOOL_METADATA_CONTEXT_MANAGEMENT_KEY)
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
            })
            .map(|tool| tool.name().to_string())
            .collect();
        Ok(tools)
    }

    pub(in crate::agent) async fn close_run_toolsets(&self, context: &mut AgentContext) {
        for toolset in self.toolsets.iter().rev() {
            let policy = toolset.lifecycle_policy();
            if !policy.exit_after_run {
                continue;
            }
            let exit_result = if let Some(timeout_ms) = policy.exit_timeout_ms {
                tokio::time::timeout(
                    Duration::from_millis(timeout_ms),
                    toolset.exit_with_context(context),
                )
                .await
                .map_err(|_| {
                    starweaver_tools::ToolsetLifecycleError::timeout(toolset.name(), timeout_ms)
                })
            } else {
                Ok(toolset.exit_with_context(context).await)
            };
            match exit_result {
                Ok(Ok(report)) => context.publish_event(report.into_event()),
                Ok(Err(error)) | Err(error) => {
                    let report = error.to_report(toolset.id().map(ToOwned::to_owned));
                    context.publish_event(report.into_event());
                }
            }
        }
    }
}

pub(in crate::agent) fn agent_error_public_message(error: &AgentError) -> String {
    error.public_message()
}

pub(in crate::agent) const fn agent_error_kind(error: &AgentError) -> &'static str {
    error.public_code()
}

pub(in crate::agent) fn preserve_pending_tool_returns_for_resume(state: &mut AgentRunState) {
    if state.pending_tool_returns.is_empty() && state.pending_tool_calls.is_empty() {
        return;
    }
    let mut parts = Vec::new();
    let returned_call_ids = state
        .pending_tool_returns
        .iter()
        .map(|tool_return| tool_return.tool_call_id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    for tool_return in &state.pending_tool_returns {
        parts.push(ModelRequestPart::ToolReturn(tool_return.clone()));
        if let Some(media_prompt) = tool_return_media_prompt(tool_return) {
            parts.push(media_prompt);
        }
    }
    for call in &state.pending_tool_calls {
        if returned_call_ids.contains(&call.id) {
            continue;
        }
        let mut metadata = starweaver_core::Metadata::default();
        metadata.insert(
            "starweaver.repaired_dangling_tool_call".to_string(),
            serde_json::json!(true),
        );
        metadata.insert(
            "reason".to_string(),
            serde_json::json!("run_failed_before_tool_return"),
        );
        parts.push(ModelRequestPart::ToolReturn(
            ToolReturnPart::new(
                call.id.clone(),
                call.name.clone(),
                serde_json::json!({
                    "error": "tool_call_interrupted",
                    "message": "run failed before tool return was recorded",
                }),
            )
            .with_error(true)
            .with_metadata(metadata),
        ));
    }
    if parts.is_empty() {
        return;
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
    state.pending_tool_calls.clear();
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use starweaver_model::ModelError;

    use super::{AgentError, agent_error_public_message};

    #[test]
    fn agent_error_public_message_redacts_provider_response_body() {
        let error = AgentError::Model(ModelError::ProviderStatus {
            status: 403,
            body: json!({"echoed_credential": "provider-secret"}),
            retryable: false,
        });

        let message = agent_error_public_message(&error);
        assert_eq!(message, "provider status 403");
        assert!(!message.contains("provider-secret"));
    }

    #[test]
    fn agent_error_public_messages_redact_free_form_runtime_details() {
        let secret = "provider-secret";
        let cases = [
            AgentError::Capability(secret.to_string()),
            AgentError::Cancelled {
                reason: secret.to_string(),
            },
            AgentError::StructuredOutput(secret.to_string()),
            AgentError::DynamicInstruction(secret.to_string()),
            AgentError::ExecutionSuspended {
                node: starweaver_core::AgentExecutionNode::ModelResponse,
                reason: secret.to_string(),
            },
            AgentError::Executor(starweaver_context::AgentExecutorError::Failed(
                secret.to_string(),
            )),
        ];

        for error in cases {
            assert!(!error.public_message().contains(secret));
        }
    }
}
