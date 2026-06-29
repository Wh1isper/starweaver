//! `OpenAI` Responses wire mapper.

use serde_json::Value;

use crate::{
    adapter::{NativeToolDefinition, ToolDefinition},
    message::{ModelMessage, ModelResponse},
    transport::MaxTokensParameter,
    ModelError, ModelResponseStreamEvent, ModelSettings,
};

mod request;
mod response;
mod stream;

pub use stream::OpenAiResponsesStreamParser;

/// `OpenAI` Responses wire mapper.
pub struct OpenAiResponsesAdapter;

impl OpenAiResponsesAdapter {
    /// Build a provider wire request.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical history cannot be mapped into response items.
    pub fn build_request(
        model: &str,
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        tools: &[ToolDefinition],
        native_tools: &[NativeToolDefinition],
    ) -> Result<Value, ModelError> {
        Self::build_request_with_options(
            model,
            messages,
            settings,
            tools,
            native_tools,
            MaxTokensParameter::Default,
        )
    }

    /// Build a provider wire request with explicit gateway/provider options.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical history cannot be mapped into response items.
    pub fn build_request_with_options(
        model: &str,
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        tools: &[ToolDefinition],
        native_tools: &[NativeToolDefinition],
        max_tokens_parameter: MaxTokensParameter,
    ) -> Result<Value, ModelError> {
        request::build_request_with_options(
            model,
            messages,
            settings,
            tools,
            native_tools,
            max_tokens_parameter,
        )
    }

    /// Parse a provider wire response.
    ///
    /// # Errors
    ///
    /// Returns an error when required response item structure is malformed.
    pub fn parse_response(value: &Value) -> Result<ModelResponse, ModelError> {
        response::parse_response(value)
    }

    /// Parse `OpenAI` Responses server-sent JSON events into canonical stream events.
    ///
    /// # Errors
    ///
    /// Returns an error when no completed response is present in the event list.
    pub fn parse_stream_events(
        events: &[Value],
    ) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        stream::parse_stream_events(events)
    }

    pub(crate) fn response_replay_items(
        response: &ModelResponse,
        settings: Option<&ModelSettings>,
    ) -> Vec<Value> {
        request::response_replay_items(response, settings)
    }
}

#[cfg(test)]
mod tests;
