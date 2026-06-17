//! Model request assembly helpers.

use std::collections::BTreeSet;

use chrono::Utc;
use starweaver_context::AgentContext;
use starweaver_core::{ConversationId, RunId};
use starweaver_model::{
    attach_prepared_instructions, format_openai_prompt_cache_key,
    supports_automatic_openai_prompt_cache_key, CodexSettings, GatewaySettings, ModelMessage,
    ModelRequest, ModelRequestParameters, ModelRequestPart, ModelSettings, OpenAiChatSettings,
    OpenAiResponsesSettings, PreparedInstruction, ProtocolFamily, ProviderSettings,
    INSTRUCTION_DYNAMIC_METADATA, INSTRUCTION_ORIGIN_AGENT, INSTRUCTION_ORIGIN_DYNAMIC_INSTRUCTION,
    INSTRUCTION_ORIGIN_METADATA, INSTRUCTION_ORIGIN_TOOLSET,
};

use crate::{
    agent::{
        runtime_helpers::{
            history_sanitize::sanitize_incomplete_tool_call_history,
            request_parts::request_instruction_insert_index, steering::is_steering_guard_prompt,
            tool_media::tool_return_media_prompt,
        },
        Agent, AgentError,
    },
    output::OutputSchema,
    run::AgentRunState,
    trace::{SpanSpec, SpanStatus},
};

impl Agent {
    fn attach_static_instruction_parts(&self, request: &mut ModelRequest) {
        if self.instructions.is_empty() {
            return;
        }
        let parts = self
            .instructions
            .iter()
            .map(|instruction| static_agent_instruction_part(instruction))
            .collect::<Vec<_>>();
        let insert_at = request_instruction_insert_index(request);
        request.parts.splice(insert_at..insert_at, parts);
    }

    pub(in crate::agent) fn prepare_request(
        &self,
        state: &AgentRunState,
        prompt: &str,
        run_id: &RunId,
        conversation_id: &ConversationId,
    ) -> ModelRequest {
        let mut parts = Vec::new();
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
                    INSTRUCTION_DYNAMIC_METADATA.to_string(),
                    serde_json::json!(true),
                );
                metadata.insert(
                    INSTRUCTION_ORIGIN_METADATA.to_string(),
                    serde_json::json!(INSTRUCTION_ORIGIN_DYNAMIC_INSTRUCTION),
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
        let mut request = ModelRequest {
            parts,
            timestamp: Some(Utc::now()),
            instructions: None,
            run_id: Some(run_id.clone()),
            conversation_id: Some(conversation_id.clone()),
            metadata: serde_json::Map::new(),
        };
        self.attach_static_instruction_parts(&mut request);
        request
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

    pub(in crate::agent) fn inject_missing_static_instructions(
        &self,
        run_id: &RunId,
        conversation_id: &ConversationId,
        messages: &mut Vec<ModelMessage>,
    ) {
        if self.instructions.is_empty() {
            return;
        }
        let latest_request = messages.iter_mut().rev().find_map(|message| match message {
            ModelMessage::Request(request) => Some(request),
            ModelMessage::Response(_) => None,
        });
        let Some(request) = latest_request else {
            messages.push(ModelMessage::Request(ModelRequest {
                parts: self
                    .instructions
                    .iter()
                    .map(|instruction| static_agent_instruction_part(instruction))
                    .collect(),
                timestamp: Some(Utc::now()),
                instructions: None,
                run_id: Some(run_id.clone()),
                conversation_id: Some(conversation_id.clone()),
                metadata: serde_json::Map::new(),
            }));
            return;
        };
        let missing = self
            .instructions
            .iter()
            .filter(|instruction| !request_contains_instruction(request, instruction))
            .map(|instruction| static_agent_instruction_part(instruction))
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return;
        }
        let insert_at = request_instruction_insert_index(request);
        request.parts.splice(insert_at..insert_at, missing);
    }

    pub(in crate::agent) async fn dynamic_instruction_parts(
        &self,
        state: &AgentRunState,
    ) -> Result<Vec<ModelRequestPart>, AgentError> {
        Ok(self
            .dynamic_instructions(state)
            .await?
            .into_iter()
            .map(|instruction| {
                let mut metadata = serde_json::Map::new();
                metadata.insert(
                    INSTRUCTION_DYNAMIC_METADATA.to_string(),
                    serde_json::json!(true),
                );
                metadata.insert(
                    INSTRUCTION_ORIGIN_METADATA.to_string(),
                    serde_json::json!(INSTRUCTION_ORIGIN_DYNAMIC_INSTRUCTION),
                );
                ModelRequestPart::Instruction {
                    text: instruction,
                    metadata,
                }
            })
            .collect())
    }

    pub(in crate::agent) fn effective_settings(
        &self,
        context: &AgentContext,
    ) -> Option<ModelSettings> {
        merge_settings_layers([
            self.session_affinity_settings(context),
            self.model.default_settings().cloned(),
            self.model_settings.clone(),
        ])
    }

    fn session_affinity_settings(&self, context: &AgentContext) -> Option<ModelSettings> {
        let session_id = context.session_id()?.as_str();
        let provider_name = self.model.provider_name();
        let mut provider_settings = ProviderSettings::default();
        match self.model.profile().protocol {
            ProtocolFamily::OpenAiChatCompletions if provider_name != Some("codex") => {
                if let Some(prompt_cache_key) =
                    supports_automatic_openai_prompt_cache_key(self.model.model_name())
                        .then(|| format_openai_prompt_cache_key(session_id))
                        .flatten()
                {
                    provider_settings.openai_chat = Some(OpenAiChatSettings {
                        prompt_cache_key: Some(prompt_cache_key),
                        ..OpenAiChatSettings::default()
                    });
                }
            }
            ProtocolFamily::OpenAiResponses if provider_name == Some("codex") => {
                provider_settings.codex = Some(CodexSettings {
                    session_id: Some(session_id.to_string()),
                    thread_id: context
                        .run_id
                        .as_ref()
                        .map(|run_id| run_id.as_str().to_string()),
                });
            }
            ProtocolFamily::OpenAiResponses => {
                if let Some(prompt_cache_key) =
                    supports_automatic_openai_prompt_cache_key(self.model.model_name())
                        .then(|| format_openai_prompt_cache_key(session_id))
                        .flatten()
                {
                    provider_settings.openai_responses = Some(OpenAiResponsesSettings {
                        prompt_cache_key: Some(prompt_cache_key),
                        ..OpenAiResponsesSettings::default()
                    });
                }
            }
            ProtocolFamily::GeminiGenerateContent | ProtocolFamily::BedrockConverse
                if gateway_affinity_enabled(context) =>
            {
                provider_settings.gateway = Some(GatewaySettings {
                    x_session_id: Some(session_id.to_string()),
                    ..GatewaySettings::default()
                });
            }
            ProtocolFamily::OpenAiChatCompletions
            | ProtocolFamily::AnthropicMessages
            | ProtocolFamily::GeminiGenerateContent
            | ProtocolFamily::BedrockConverse => {}
        }
        if provider_settings.is_empty() {
            None
        } else {
            Some(ModelSettings {
                provider_settings,
                ..ModelSettings::default()
            })
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

    pub(in crate::agent) async fn prepare_provider_messages(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        mut messages: Vec<ModelMessage>,
    ) -> Result<Vec<ModelMessage>, AgentError> {
        for capability in &self.ordered_capabilities()? {
            messages = capability
                .prepare_provider_messages_with_context(state, context, messages)
                .await
                .map_err(Self::capability_error)?;
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
        let mut output_names = BTreeSet::new();
        for function in &self.output_functions {
            let name = function.definition().name;
            if !output_names.insert(name.clone()) {
                return Err(AgentError::Capability(format!(
                    "duplicate output function name {name:?}"
                )));
            }
        }
        for tool in &mut params.tools {
            if output_names.contains(&tool.name) {
                return Err(AgentError::Capability(format!(
                    "output function name {:?} collides with request tool",
                    tool.name
                )));
            }
            tool.metadata.insert(
                "starweaver_tool_kind".to_string(),
                serde_json::json!("function"),
            );
        }
        for function in &self.output_functions {
            let mut tool = function.definition().tool_definition();
            tool.metadata.insert(
                "starweaver_tool_kind".to_string(),
                serde_json::json!("output"),
            );
            if names.insert(tool.name.clone()) {
                params.tools.push(tool);
            }
        }
        for mut tool in self.tools.definitions() {
            if output_names.contains(&tool.name) {
                return Err(AgentError::Capability(format!(
                    "output function name {:?} collides with runtime tool",
                    tool.name
                )));
            }
            tool.metadata.insert(
                "starweaver_tool_kind".to_string(),
                serde_json::json!("function"),
            );
            if names.insert(tool.name.clone()) {
                params.tools.push(tool);
            }
        }
        params.tools = self.prepare_tools(state, context, params.tools).await?;
        for instruction in self.tools.instructions() {
            let mut metadata = serde_json::Map::new();
            metadata.insert(
                INSTRUCTION_ORIGIN_METADATA.to_string(),
                serde_json::json!(INSTRUCTION_ORIGIN_TOOLSET),
            );
            metadata.insert(
                INSTRUCTION_DYNAMIC_METADATA.to_string(),
                serde_json::json!(instruction.dynamic),
            );
            metadata.insert(
                "starweaver_toolset_group".to_string(),
                serde_json::json!(instruction.group.clone()),
            );
            params.instructions.push(PreparedInstruction {
                text: instruction.render_xml(),
                dynamic: instruction.dynamic,
                metadata,
            });
        }
        Ok(params)
    }

    pub(in crate::agent) fn attach_prepared_request_instructions(
        messages: Vec<ModelMessage>,
        params: &ModelRequestParameters,
    ) -> Vec<ModelMessage> {
        attach_prepared_instructions(messages, &params.instructions)
    }

    pub(in crate::agent) fn fill_message_metadata(
        message: &mut ModelMessage,
        run_id: &RunId,
        conversation_id: &ConversationId,
    ) {
        match message {
            ModelMessage::Request(request) => {
                request.run_id.get_or_insert_with(|| run_id.clone());
                request
                    .conversation_id
                    .get_or_insert_with(|| conversation_id.clone());
                request.timestamp.get_or_insert_with(Utc::now);
            }
            ModelMessage::Response(response) => {
                response.run_id.get_or_insert_with(|| run_id.clone());
                response
                    .conversation_id
                    .get_or_insert_with(|| conversation_id.clone());
                response.timestamp.get_or_insert_with(Utc::now);
            }
        }
    }

    pub(in crate::agent) fn validate_model_request_messages(
        messages: &[ModelMessage],
    ) -> Result<(), AgentError> {
        if messages.is_empty() {
            return Err(AgentError::Capability(
                "prepared model history cannot be empty".to_string(),
            ));
        }
        if !matches!(messages.last(), Some(ModelMessage::Request(_))) {
            return Err(AgentError::Capability(
                "prepared model history must end with a model request".to_string(),
            ));
        }
        Ok(())
    }
}

fn merge_settings_layers(
    layers: impl IntoIterator<Item = Option<ModelSettings>>,
) -> Option<ModelSettings> {
    let mut merged = None::<ModelSettings>;
    for layer in layers.into_iter().flatten() {
        merged = Some(match merged {
            Some(current) => current.merge(&layer),
            None => layer,
        });
    }
    merged
}

fn gateway_affinity_enabled(context: &AgentContext) -> bool {
    context
        .metadata
        .get("starweaver.gateway_session_affinity")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn static_agent_instruction_part(instruction: &str) -> ModelRequestPart {
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        INSTRUCTION_DYNAMIC_METADATA.to_string(),
        serde_json::json!(false),
    );
    metadata.insert(
        INSTRUCTION_ORIGIN_METADATA.to_string(),
        serde_json::json!(INSTRUCTION_ORIGIN_AGENT),
    );
    ModelRequestPart::Instruction {
        text: instruction.to_string(),
        metadata,
    }
}

fn request_contains_instruction(request: &ModelRequest, text: &str) -> bool {
    request.instructions.as_deref() == Some(text)
        || request.parts.iter().any(|part| match part {
            ModelRequestPart::SystemPrompt { text: existing, .. }
            | ModelRequestPart::Instruction { text: existing, .. } => existing == text,
            _ => false,
        })
}
