use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    adapter::ModelRequestParameters,
    message::ModelMessage,
    profile::{ModelProfile, StructuredOutputMode},
    settings::{ModelSettings, ThinkingSettings},
};

/// Prepared model request evidence produced before provider wire mapping.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PreparedModelRequest {
    /// Canonical history before profile normalization.
    pub canonical_messages: Vec<ModelMessage>,
    /// Messages after active profile normalization.
    pub normalized_messages: Vec<ModelMessage>,
    /// Prepared request parameters after profile negotiation.
    pub params: ModelRequestParameters,
    /// Merged model settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<ModelSettings>,
    /// Active model profile.
    pub profile: ModelProfile,
    /// Selected output strategy for this request.
    pub output_mode: OutputMode,
    /// Selected thinking settings for this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingSettings>,
    /// Preparation evidence for replay and traces.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

impl PreparedModelRequest {
    pub(super) fn with_thinking_from_params(mut self) -> Self {
        self.thinking = self.params.thinking.clone();
        self
    }
}

/// Output strategy selected for a prepared request.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    /// Select the best available output strategy from model profile and schema.
    Auto,
    /// Plain text output.
    Text,
    /// Provider-native JSON schema output.
    NativeJsonSchema,
    /// Provider-native JSON object output.
    NativeJsonObject,
    /// Tool/function output.
    Tool,
    /// Tool/function output while allowing text fallback.
    ToolOrText,
    /// Prompted output instructions.
    Prompted,
    /// Provider-native image output.
    Image,
}

impl OutputMode {
    /// Convert profile structured-output mode to request output mode.
    #[must_use]
    pub const fn from_structured_output_mode(mode: StructuredOutputMode) -> Self {
        match mode {
            StructuredOutputMode::NativeJsonSchema => Self::NativeJsonSchema,
            StructuredOutputMode::NativeJsonObject => Self::NativeJsonObject,
            StructuredOutputMode::Tool => Self::Tool,
            StructuredOutputMode::Prompted => Self::Prompted,
        }
    }
}
