use serde_json::Value;

use crate::{
    adapter::ModelRequestParameters,
    message::{ModelMessage, ModelResponse},
    profile::{ModelProfile, ProtocolFamily},
    providers::{
        anthropic::AnthropicMessagesAdapter, bedrock::BedrockConverseAdapter,
        gemini::GeminiGenerateContentAdapter, openai_chat::OpenAiChatAdapter,
        openai_responses::OpenAiResponsesAdapter,
    },
    settings::{ModelSettings, ThinkingSettings},
    transport::MaxTokensParameter,
    ModelError,
};

use super::{output_schema::apply_output_schema, ProtocolModelClient};

impl ProtocolModelClient {
    pub(super) fn build_wire_body(
        &self,
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        params: &ModelRequestParameters,
    ) -> Result<Value, ModelError> {
        let effective_settings = provider_policy_settings(settings, &self.profile);
        let settings = effective_settings.as_ref().or(settings);
        let mut body = match self.profile.protocol {
            ProtocolFamily::OpenAiChatCompletions => OpenAiChatAdapter::build_request_with_options(
                &self.model_name,
                messages,
                settings,
                &params.tools,
                self.openai_chat_max_tokens_parameter(),
            ),
            ProtocolFamily::OpenAiResponses => OpenAiResponsesAdapter::build_request_with_options(
                &self.model_name,
                messages,
                settings,
                &params.tools,
                &params.native_tools,
                self.openai_responses_max_tokens_parameter(),
            ),
            ProtocolFamily::AnthropicMessages => AnthropicMessagesAdapter::build_request(
                &self.model_name,
                messages,
                settings,
                &params.tools,
            ),
            ProtocolFamily::GeminiGenerateContent => {
                GeminiGenerateContentAdapter::build_request_with_native_tools(
                    messages,
                    settings,
                    &params.tools,
                    &params.native_tools,
                )
            }
            ProtocolFamily::BedrockConverse => BedrockConverseAdapter::build_request(
                &self.model_name,
                messages,
                settings,
                &params.tools,
            ),
        }?;
        apply_output_schema(&mut body, &self.profile, params);
        Ok(body)
    }

    fn openai_chat_max_tokens_parameter(&self) -> MaxTokensParameter {
        match self.http_config.max_tokens_parameter {
            MaxTokensParameter::Default if self.provider_name == "openai" => {
                MaxTokensParameter::MaxCompletionTokens
            }
            MaxTokensParameter::Default => MaxTokensParameter::MaxTokens,
            value => value,
        }
    }

    const fn openai_responses_max_tokens_parameter(&self) -> MaxTokensParameter {
        match self.http_config.max_tokens_parameter {
            MaxTokensParameter::Default => MaxTokensParameter::MaxOutputTokens,
            value => value,
        }
    }

    pub(super) fn parse_wire_response(&self, body: &Value) -> Result<ModelResponse, ModelError> {
        match self.profile.protocol {
            ProtocolFamily::OpenAiChatCompletions => OpenAiChatAdapter::parse_response(body),
            ProtocolFamily::OpenAiResponses => OpenAiResponsesAdapter::parse_response(body),
            ProtocolFamily::AnthropicMessages => AnthropicMessagesAdapter::parse_response(body),
            ProtocolFamily::GeminiGenerateContent => {
                GeminiGenerateContentAdapter::parse_response(body)
            }
            ProtocolFamily::BedrockConverse => BedrockConverseAdapter::parse_response(body),
        }
    }
}

fn provider_policy_settings(
    settings: Option<&ModelSettings>,
    profile: &ModelProfile,
) -> Option<ModelSettings> {
    let settings = settings?;
    if !profile.drop_sampling_parameters_when_reasoning
        || !settings.thinking.as_ref().is_some_and(thinking_is_active)
    {
        return None;
    }
    let mut policy_settings = settings.clone();
    policy_settings.temperature = None;
    policy_settings.top_p = None;
    policy_settings.top_k = None;
    policy_settings.presence_penalty = None;
    policy_settings.frequency_penalty = None;
    policy_settings.logit_bias.clear();
    Some(policy_settings)
}

fn thinking_is_active(thinking: &ThinkingSettings) -> bool {
    let mode_disabled = thinking
        .mode
        .as_deref()
        .is_some_and(|mode| matches!(mode, "disabled" | "off" | "none"));
    let effort_disabled = matches!(thinking.effort.as_str(), "" | "disabled" | "off" | "none");
    !mode_disabled && !effort_disabled
}
