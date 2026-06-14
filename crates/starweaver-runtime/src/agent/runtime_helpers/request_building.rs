//! Model request assembly helpers.

use std::collections::BTreeSet;

use starweaver_context::AgentContext;
use starweaver_core::{ConversationId, RunId};
use starweaver_model::{
    ModelMessage, ModelRequest, ModelRequestParameters, ModelRequestPart, ModelSettings,
    PreparedInstruction,
};

use crate::{
    agent::{
        runtime_helpers::{
            history_sanitize::sanitize_incomplete_tool_call_history,
            steering::is_steering_guard_prompt, tool_media::tool_return_media_prompt,
        },
        Agent, AgentError,
    },
    output::OutputSchema,
    run::AgentRunState,
    trace::{SpanSpec, SpanStatus},
};

impl Agent {
    pub(in crate::agent) async fn prepare_request(
        &self,
        state: &AgentRunState,
        prompt: &str,
        run_id: &RunId,
        conversation_id: &ConversationId,
    ) -> Result<ModelRequest, AgentError> {
        let mut parts = Vec::new();
        if state.message_history.is_empty() {
            let dynamic_instructions = self.dynamic_instructions(state).await?;
            parts.extend(self.instructions.iter().map(|instruction| {
                ModelRequestPart::SystemPrompt {
                    text: instruction.clone(),
                    metadata: serde_json::Map::new(),
                }
            }));
            parts.extend(dynamic_instructions.into_iter().map(|instruction| {
                let mut metadata = serde_json::Map::new();
                metadata.insert(
                    "starweaver_instruction_dynamic".to_string(),
                    serde_json::json!(true),
                );
                ModelRequestPart::Instruction {
                    text: instruction,
                    metadata,
                }
            }));
        }
        if !state.pending_tool_returns.is_empty() {
            for tool_return in &state.pending_tool_returns {
                parts.push(ModelRequestPart::ToolReturn(tool_return.clone()));
                if let Some(media_prompt) = tool_return_media_prompt(tool_return) {
                    parts.push(media_prompt);
                }
            }
        } else if state.run_step == 0 {
            parts.push(ModelRequestPart::UserPrompt {
                content: vec![starweaver_model::ContentPart::Text {
                    text: prompt.to_string(),
                }],
                name: None,
                metadata: serde_json::Map::new(),
            });
        } else {
            let mut metadata = serde_json::Map::new();
            if is_steering_guard_prompt(prompt) {
                metadata.insert(
                    "starweaver.kind".to_string(),
                    serde_json::json!("steering_guard"),
                );
                metadata.insert(
                    "starweaver_instruction_dynamic".to_string(),
                    serde_json::json!(true),
                );
                parts.push(ModelRequestPart::Instruction {
                    text: prompt.to_string(),
                    metadata,
                });
            } else {
                parts.push(ModelRequestPart::RetryPrompt {
                    text: prompt.to_string(),
                    tool_call_id: None,
                    metadata,
                });
            }
        }
        Ok(ModelRequest {
            parts,
            timestamp: None,
            instructions: None,
            run_id: Some(run_id.clone()),
            conversation_id: Some(conversation_id.clone()),
            metadata: serde_json::Map::new(),
        })
    }

    pub(in crate::agent) async fn dynamic_instructions(
        &self,
        state: &AgentRunState,
    ) -> Result<Vec<String>, AgentError> {
        let mut instructions = Vec::new();
        for instruction in &self.dynamic_instructions {
            instructions.push(
                instruction
                    .instruction(state)
                    .await
                    .map_err(Self::dynamic_instruction_error)?,
            );
        }
        Ok(instructions)
    }

    pub(in crate::agent) fn effective_settings(&self) -> Option<ModelSettings> {
        match (self.model.default_settings(), &self.model_settings) {
            (Some(defaults), Some(settings)) => Some(defaults.merge(settings)),
            (Some(defaults), None) => Some(defaults.clone()),
            (None, Some(settings)) => Some(settings.clone()),
            (None, None) => None,
        }
    }

    pub(in crate::agent) async fn prepare_model_messages(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
    ) -> Result<Vec<ModelMessage>, AgentError> {
        let mut messages = state.message_history.clone();
        for capability in &self.ordered_capabilities()? {
            let before_count = messages.len();
            messages = capability
                .prepare_model_messages_with_context(state, context, messages)
                .await
                .map_err(Self::capability_error)?;
            let after_count = messages.len();
            if before_count != after_count {
                let span = self.trace_recorder.start_span(
                    SpanSpec::new("starweaver.history.compaction")
                        .with_attribute(
                            "starweaver.capability.name",
                            serde_json::json!(capability.spec().id.as_str()),
                        )
                        .with_attribute(
                            "starweaver.history.messages.before",
                            serde_json::json!(before_count),
                        )
                        .with_attribute(
                            "starweaver.history.messages.after",
                            serde_json::json!(after_count),
                        ),
                    &context.trace_context,
                );
                self.trace_recorder.close_span(&span, SpanStatus::Ok);
            }
        }
        let before_count = messages.len();
        messages = sanitize_incomplete_tool_call_history(messages);
        let after_count = messages.len();
        if before_count != after_count {
            let span = self.trace_recorder.start_span(
                SpanSpec::new("starweaver.history.sanitize_incomplete_tool_calls")
                    .with_attribute(
                        "starweaver.history.messages.before",
                        serde_json::json!(before_count),
                    )
                    .with_attribute(
                        "starweaver.history.messages.after",
                        serde_json::json!(after_count),
                    ),
                &context.trace_context,
            );
            self.trace_recorder.close_span(&span, SpanStatus::Ok);
        }
        Ok(messages)
    }

    pub(in crate::agent) async fn effective_request_params(
        &self,
        state: &AgentRunState,
        context: &AgentContext,
    ) -> Result<ModelRequestParameters, AgentError> {
        let mut params = self.request_params.clone();
        if params.output_schema.is_none() {
            params.output_schema = self
                .output_schema
                .as_ref()
                .map(OutputSchema::request_schema);
        }
        let mut names = params
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<BTreeSet<_>>();
        for function in &self.output_functions {
            let tool = function.definition().tool_definition();
            if names.insert(tool.name.clone()) {
                params.tools.push(tool);
            }
        }
        for tool in self.tools.definitions() {
            if names.insert(tool.name.clone()) {
                params.tools.push(tool);
            }
        }
        params.tools = self.prepare_tools(state, context, params.tools).await?;
        for instruction in self.tools.instructions() {
            let mut metadata = serde_json::Map::new();
            metadata.insert(
                "starweaver_instruction_origin".to_string(),
                serde_json::json!("toolset"),
            );
            metadata.insert(
                "starweaver_toolset_group".to_string(),
                serde_json::json!(instruction.group.clone()),
            );
            params.instructions.push(PreparedInstruction {
                text: instruction.render_xml(),
                dynamic: false,
                metadata,
            });
        }
        Ok(params)
    }
}
