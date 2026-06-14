use serde_json::Value;

use crate::{
    adapter::ModelRequestParameters,
    message::{ModelMessage, ModelResponse},
    profile::ProtocolFamily,
    providers::{
        anthropic::AnthropicMessagesAdapter, bedrock::BedrockConverseAdapter,
        gemini::GeminiGenerateContentAdapter, openai_chat::OpenAiChatAdapter,
        openai_responses::OpenAiResponsesAdapter,
    },
    settings::ModelSettings,
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
        let mut body = match self.profile.protocol {
            ProtocolFamily::OpenAiChatCompletions => OpenAiChatAdapter::build_request(
                &self.model_name,
                messages,
                settings,
                &params.tools,
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
