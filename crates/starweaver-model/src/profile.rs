//! Model capability profiles and protocol families.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

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

/// Provider-executed native tool families.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeToolKind {
    /// Provider web search tool.
    WebSearch,
    /// Provider code execution tool.
    CodeExecution,
    /// Provider file search tool.
    FileSearch,
    /// Provider image generation tool.
    ImageGeneration,
    /// Provider-hosted MCP server tool.
    McpServer,
    /// Provider tool-search/discovery tool.
    ToolSearch,
    /// Gemini Google Search tool.
    GoogleSearch,
}

impl NativeToolKind {
    /// Return the provider-neutral native tool type string used in `NativeToolDefinition`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WebSearch => "web_search",
            Self::CodeExecution => "code_execution",
            Self::FileSearch => "file_search",
            Self::ImageGeneration => "image_generation",
            Self::McpServer => "mcp_server",
            Self::ToolSearch => "tool_search",
            Self::GoogleSearch => "google_search",
        }
    }

    /// Resolve a provider-neutral native tool type string.
    #[must_use]
    pub fn from_tool_type(tool_type: &str) -> Option<Self> {
        match tool_type {
            "web_search" | "web_search_preview" => Some(Self::WebSearch),
            "code_execution" | "code_interpreter" => Some(Self::CodeExecution),
            "file_search" => Some(Self::FileSearch),
            "image_generation" => Some(Self::ImageGeneration),
            "mcp_server" => Some(Self::McpServer),
            "tool_search" => Some(Self::ToolSearch),
            "google_search" => Some(Self::GoogleSearch),
            _ => None,
        }
    }
}

fn default_prompted_output_template() -> String {
    "Always respond with a JSON object that matches this schema:\n\n{schema}\n\nDon't include any text or Markdown fencing before or after."
        .to_string()
}

fn default_thinking_tags() -> (String, String) {
    ("<think>".to_string(), "</think>".to_string())
}

/// JSON schema normalization strategy for provider-specific schema subsets.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonSchemaTransformer {
    /// Inline local `$defs` / `definitions` references and remove the top-level definition map.
    InlineDefinitions,
}

impl JsonSchemaTransformer {
    /// Transform a JSON schema value according to this strategy.
    #[must_use]
    pub fn transform_schema(self, schema: &Value) -> Value {
        match self {
            Self::InlineDefinitions => inline_schema_definitions(schema),
        }
    }
}

fn inline_schema_definitions(schema: &Value) -> Value {
    let mut schema = schema.clone();
    let definitions = schema
        .get("$defs")
        .or_else(|| schema.get("definitions"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if !definitions.is_empty() {
        inline_schema_refs(&mut schema, &definitions);
    }
    if let Value::Object(object) = &mut schema {
        object.remove("$defs");
        object.remove("definitions");
    }
    schema
}

fn inline_schema_refs(value: &mut Value, definitions: &Map<String, Value>) {
    match value {
        Value::Object(object) => {
            if let Some(definition) = object
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|reference| schema_ref_name(reference, definitions))
                .and_then(|name| definitions.get(name))
                .cloned()
            {
                object.remove("$ref");
                if object.is_empty() {
                    *value = definition;
                    inline_schema_refs(value, definitions);
                    return;
                }
                if let Value::Object(definition_object) = definition {
                    for (key, nested) in definition_object {
                        object.entry(key).or_insert(nested);
                    }
                }
            }
            object.remove("$defs");
            object.remove("definitions");
            for nested in object.values_mut() {
                inline_schema_refs(nested, definitions);
            }
        }
        Value::Array(items) => {
            for item in items {
                inline_schema_refs(item, definitions);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn schema_ref_name<'a>(reference: &'a str, definitions: &Map<String, Value>) -> Option<&'a str> {
    let name = reference
        .strip_prefix("#/$defs/")
        .or_else(|| reference.strip_prefix("#/definitions/"))?;
    definitions.contains_key(name).then_some(name)
}

fn all_native_tools() -> BTreeSet<NativeToolKind> {
    [
        NativeToolKind::WebSearch,
        NativeToolKind::CodeExecution,
        NativeToolKind::FileSearch,
        NativeToolKind::ImageGeneration,
        NativeToolKind::McpServer,
        NativeToolKind::ToolSearch,
        NativeToolKind::GoogleSearch,
    ]
    .into_iter()
    .collect()
}

const fn no_native_tools() -> BTreeSet<NativeToolKind> {
    BTreeSet::new()
}

/// Capability metadata and request-shaping policy.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelProfile {
    /// Protocol family used by the adapter.
    pub protocol: ProtocolFamily,
    /// Function/tool calling support.
    pub supports_tools: bool,
    /// Native support for tool return schemas.
    pub supports_tool_return_schema: bool,
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
    /// Drop sampling settings such as temperature/top-p/top-k/penalties/logit-bias when reasoning is enabled.
    #[serde(default)]
    pub drop_sampling_parameters_when_reasoning: bool,
    /// Reasoning is always enabled.
    pub thinking_always_enabled: bool,
    /// Tags used by text-stream adapters to split thinking parts from text.
    #[serde(default = "default_thinking_tags")]
    pub thinking_tags: (String, String),
    /// Ignore leading whitespace-only stream content before semantic parts.
    pub ignore_streamed_leading_whitespace: bool,
    /// Default structured output mode.
    pub default_structured_output_mode: StructuredOutputMode,
    /// Prompt template used for prompted structured output.
    #[serde(default = "default_prompted_output_template")]
    pub prompted_output_template: String,
    /// Whether native structured output still needs schema instructions in the prompt.
    pub native_output_requires_schema_in_instructions: bool,
    /// Provider-neutral native tool families supported by this profile.
    #[serde(default = "all_native_tools")]
    pub supported_native_tools: BTreeSet<NativeToolKind>,
    /// JSON schema transformer applied to tools and structured output schemas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_schema_transformer: Option<JsonSchemaTransformer>,
    /// Message normalization policy.
    pub message_normalization: MessageNormalization,
}

impl ModelProfile {
    /// Create a default profile for a protocol family.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn for_protocol(protocol: ProtocolFamily) -> Self {
        let base = Self {
            protocol,
            supports_tools: true,
            supports_tool_return_schema: false,
            supports_json_schema_output: false,
            supports_json_object_output: false,
            supports_image_output: false,
            supports_image_input: false,
            supports_video_input: false,
            supports_audio_input: false,
            supports_document_input: false,
            supports_inline_system_prompts: false,
            supports_thinking: false,
            drop_sampling_parameters_when_reasoning: false,
            thinking_always_enabled: false,
            thinking_tags: default_thinking_tags(),
            ignore_streamed_leading_whitespace: false,
            default_structured_output_mode: StructuredOutputMode::Tool,
            prompted_output_template: default_prompted_output_template(),
            native_output_requires_schema_in_instructions: false,
            supported_native_tools: no_native_tools(),
            json_schema_transformer: None,
            message_normalization: MessageNormalization::MergeAdjacentSameRole,
        };
        match protocol {
            ProtocolFamily::OpenAiChatCompletions => Self {
                supports_json_schema_output: true,
                supports_json_object_output: true,
                supports_image_input: true,
                supports_inline_system_prompts: true,
                supports_thinking: true,
                drop_sampling_parameters_when_reasoning: true,
                default_structured_output_mode: StructuredOutputMode::NativeJsonSchema,
                supported_native_tools: no_native_tools(),
                message_normalization: MessageNormalization::MergeAdjacentSameRole,
                ..base
            },
            ProtocolFamily::OpenAiResponses => Self {
                supports_json_schema_output: true,
                supports_json_object_output: true,
                supports_image_input: true,
                supports_inline_system_prompts: true,
                supports_thinking: true,
                drop_sampling_parameters_when_reasoning: true,
                default_structured_output_mode: StructuredOutputMode::NativeJsonSchema,
                supported_native_tools: [
                    NativeToolKind::WebSearch,
                    NativeToolKind::CodeExecution,
                    NativeToolKind::FileSearch,
                    NativeToolKind::ImageGeneration,
                    NativeToolKind::McpServer,
                    NativeToolKind::ToolSearch,
                ]
                .into_iter()
                .collect(),
                message_normalization: MessageNormalization::PreserveItems,
                ..base
            },
            ProtocolFamily::AnthropicMessages => Self {
                supports_json_schema_output: true,
                supports_json_object_output: false,
                supports_image_input: true,
                supports_document_input: true,
                supports_thinking: true,
                drop_sampling_parameters_when_reasoning: true,
                default_structured_output_mode: StructuredOutputMode::NativeJsonSchema,
                supported_native_tools: no_native_tools(),
                message_normalization: MessageNormalization::SystemField,
                ..base
            },
            ProtocolFamily::GeminiGenerateContent => Self {
                supports_tool_return_schema: true,
                supports_json_schema_output: true,
                supports_json_object_output: true,
                supports_image_input: true,
                supports_video_input: true,
                supports_audio_input: true,
                supports_document_input: true,
                supports_thinking: true,
                default_structured_output_mode: StructuredOutputMode::NativeJsonSchema,
                supported_native_tools: [
                    NativeToolKind::GoogleSearch,
                    NativeToolKind::CodeExecution,
                ]
                .into_iter()
                .collect(),
                message_normalization: MessageNormalization::SystemInstruction,
                ..base
            },
            ProtocolFamily::BedrockConverse => Self {
                supports_image_input: true,
                supports_document_input: true,
                default_structured_output_mode: StructuredOutputMode::Tool,
                message_normalization: MessageNormalization::SystemField,
                ..base
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
