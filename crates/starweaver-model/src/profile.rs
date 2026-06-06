//! Model capability profiles and protocol families.

use serde::{Deserialize, Serialize};

/// Supported provider protocol families.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolFamily {
    /// `OpenAI` Chat Completions protocol.
    OpenAiChatCompletions,
    /// `OpenAI` Responses protocol.
    OpenAiResponses,
    /// Anthropic Messages protocol.
    AnthropicMessages,
    /// Gemini generateContent protocol.
    GeminiGenerateContent,
    /// Bedrock Converse protocol.
    BedrockConverse,
}

/// Capability metadata and request-shaping policy.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelProfile {
    /// Protocol family used by the adapter.
    pub protocol: ProtocolFamily,
    /// Function/tool calling support.
    pub supports_tools: bool,
    /// Native JSON schema output support.
    pub supports_json_schema_output: bool,
    /// JSON object mode support.
    pub supports_json_object_output: bool,
    /// Image return support.
    pub supports_image_output: bool,
    /// Image URL input support.
    pub supports_image_input: bool,
    /// Video URL input support.
    pub supports_video_input: bool,
    /// Audio URL input support.
    pub supports_audio_input: bool,
    /// Document URL input support.
    pub supports_document_input: bool,
    /// Flexible system prompt placement support.
    pub supports_inline_system_prompts: bool,
    /// Configurable thinking support.
    pub supports_thinking: bool,
    /// Reasoning is always enabled.
    pub thinking_always_enabled: bool,
    /// Default structured output mode.
    pub default_structured_output_mode: StructuredOutputMode,
    /// Message normalization policy.
    pub message_normalization: MessageNormalization,
}

impl ModelProfile {
    /// Create a default profile for a protocol family.
    #[must_use]
    pub const fn for_protocol(protocol: ProtocolFamily) -> Self {
        match protocol {
            ProtocolFamily::OpenAiChatCompletions => Self {
                protocol,
                supports_tools: true,
                supports_json_schema_output: true,
                supports_json_object_output: true,
                supports_image_output: false,
                supports_image_input: true,
                supports_video_input: false,
                supports_audio_input: false,
                supports_document_input: false,
                supports_inline_system_prompts: true,
                supports_thinking: false,
                thinking_always_enabled: false,
                default_structured_output_mode: StructuredOutputMode::NativeJsonSchema,
                message_normalization: MessageNormalization::MergeAdjacentSameRole,
            },
            ProtocolFamily::OpenAiResponses => Self {
                protocol,
                supports_tools: true,
                supports_json_schema_output: true,
                supports_json_object_output: true,
                supports_image_output: false,
                supports_image_input: true,
                supports_video_input: false,
                supports_audio_input: false,
                supports_document_input: false,
                supports_inline_system_prompts: true,
                supports_thinking: true,
                thinking_always_enabled: false,
                default_structured_output_mode: StructuredOutputMode::NativeJsonSchema,
                message_normalization: MessageNormalization::PreserveItems,
            },
            ProtocolFamily::AnthropicMessages => Self {
                protocol,
                supports_tools: true,
                supports_json_schema_output: false,
                supports_json_object_output: false,
                supports_image_output: false,
                supports_image_input: true,
                supports_video_input: false,
                supports_audio_input: false,
                supports_document_input: true,
                supports_inline_system_prompts: false,
                supports_thinking: true,
                thinking_always_enabled: false,
                default_structured_output_mode: StructuredOutputMode::Tool,
                message_normalization: MessageNormalization::SystemField,
            },
            ProtocolFamily::GeminiGenerateContent => Self {
                protocol,
                supports_tools: true,
                supports_json_schema_output: true,
                supports_json_object_output: true,
                supports_image_output: false,
                supports_image_input: true,
                supports_video_input: true,
                supports_audio_input: true,
                supports_document_input: true,
                supports_inline_system_prompts: false,
                supports_thinking: true,
                thinking_always_enabled: false,
                default_structured_output_mode: StructuredOutputMode::NativeJsonSchema,
                message_normalization: MessageNormalization::SystemInstruction,
            },
            ProtocolFamily::BedrockConverse => Self {
                protocol,
                supports_tools: true,
                supports_json_schema_output: false,
                supports_json_object_output: false,
                supports_image_output: false,
                supports_image_input: true,
                supports_video_input: false,
                supports_audio_input: false,
                supports_document_input: true,
                supports_inline_system_prompts: false,
                supports_thinking: false,
                thinking_always_enabled: false,
                default_structured_output_mode: StructuredOutputMode::Tool,
                message_normalization: MessageNormalization::SystemField,
            },
        }
    }
}

/// Structured output strategy.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuredOutputMode {
    /// Use provider-native JSON schema mode.
    NativeJsonSchema,
    /// Use JSON object mode.
    NativeJsonObject,
    /// Use tool/function call output.
    Tool,
    /// Use prompt-level instructions.
    Prompted,
}

/// Provider message normalization strategy.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageNormalization {
    /// Merge adjacent messages with the same provider role.
    MergeAdjacentSameRole,
    /// Preserve item boundaries.
    PreserveItems,
    /// Move system prompts to a top-level system field.
    SystemField,
    /// Move system prompts to a top-level system instruction object.
    SystemInstruction,
    /// Wrap system fragments into tagged user content.
    WrapInlineSystem,
}
