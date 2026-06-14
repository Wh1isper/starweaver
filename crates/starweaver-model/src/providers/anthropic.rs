//! Anthropic Messages wire mapper.

mod content;
mod request;
mod response;
mod settings;

#[cfg(test)]
mod tests;

use serde_json::Value;

use crate::{
    adapter::ToolDefinition, message::ModelMessage, ModelError, ModelResponse, ModelSettings,
};

/// Anthropic Messages wire mapper.
pub struct AnthropicMessagesAdapter;

impl AnthropicMessagesAdapter {
    /// Build a provider wire request.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical history cannot be mapped into Anthropic messages.
    pub fn build_request(
        model: &str,
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        tools: &[ToolDefinition],
    ) -> Result<Value, ModelError> {
        request::build_request(model, messages, settings, tools)
    }

    /// Parse a provider wire response.
    ///
    /// # Errors
    ///
    /// Returns an error when required Anthropic response structure is malformed.
    pub fn parse_response(value: &Value) -> Result<ModelResponse, ModelError> {
        response::parse_response(value)
    }
}
