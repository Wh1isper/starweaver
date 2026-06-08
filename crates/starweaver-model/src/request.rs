//! Request preparation snapshots and profile-driven normalization.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    adapter::ModelRequestParameters,
    message::{ModelMessage, ModelRequest, ModelRequestPart},
    profile::{MessageNormalization, ModelProfile, StructuredOutputMode},
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

/// Output strategy selected for a prepared request.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    /// Plain text output.
    Text,
    /// Provider-native JSON schema output.
    NativeJsonSchema,
    /// Provider-native JSON object output.
    NativeJsonObject,
    /// Tool/function output.
    Tool,
    /// Prompted output instructions.
    Prompted,
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

/// Merge defaults, prepare parameters, and normalize messages for a model request.
#[must_use]
pub fn prepare_model_request(
    messages: Vec<ModelMessage>,
    default_settings: Option<&ModelSettings>,
    request_settings: Option<ModelSettings>,
    params: ModelRequestParameters,
    profile: &ModelProfile,
) -> PreparedModelRequest {
    let settings = merge_settings(default_settings, request_settings);
    let mut prepared_params = params;
    let mut metadata = Map::new();

    let output_mode = select_output_mode(profile, &prepared_params);
    prepared_params.output_mode.get_or_insert(output_mode);

    if let Some(thinking) = settings
        .as_ref()
        .and_then(|settings| settings.thinking.clone())
    {
        prepared_params.thinking.get_or_insert(thinking);
    }

    dedupe_native_tools(&mut prepared_params, &mut metadata);
    attach_prompted_output_instruction(&mut prepared_params, output_mode);
    let messages = attach_prepared_instructions(messages, &prepared_params.instructions);

    let normalized_messages = prepare_messages(&messages, profile.message_normalization);
    if normalized_messages != messages {
        metadata.insert(
            "message_normalization".to_string(),
            serde_json::json!(profile.message_normalization),
        );
    }

    PreparedModelRequest {
        canonical_messages: messages,
        normalized_messages,
        params: prepared_params,
        settings,
        profile: profile.clone(),
        output_mode,
        thinking: None,
        metadata,
    }
    .with_thinking_from_params()
}

impl PreparedModelRequest {
    fn with_thinking_from_params(mut self) -> Self {
        self.thinking = self.params.thinking.clone();
        self
    }
}

fn merge_settings(
    default_settings: Option<&ModelSettings>,
    request_settings: Option<ModelSettings>,
) -> Option<ModelSettings> {
    match (default_settings, request_settings) {
        (Some(defaults), Some(settings)) => Some(defaults.merge(&settings)),
        (Some(defaults), None) => Some(defaults.clone()),
        (None, Some(settings)) => Some(settings),
        (None, None) => None,
    }
}

fn select_output_mode(profile: &ModelProfile, params: &ModelRequestParameters) -> OutputMode {
    params.output_mode.unwrap_or_else(|| {
        if params.output_schema.is_some() {
            OutputMode::from_structured_output_mode(profile.default_structured_output_mode)
        } else {
            OutputMode::Text
        }
    })
}

fn dedupe_native_tools(params: &mut ModelRequestParameters, metadata: &mut Map<String, Value>) {
    let before = params.native_tools.len();
    let mut seen = std::collections::BTreeSet::new();
    params.native_tools.retain(|tool| {
        let key = format!("{}:{}", tool.tool_type, Value::Object(tool.config.clone()));
        seen.insert(key)
    });
    let removed = before.saturating_sub(params.native_tools.len());
    if removed > 0 {
        metadata.insert(
            "native_tools_deduplicated".to_string(),
            serde_json::json!(removed),
        );
    }
}

fn attach_prompted_output_instruction(
    params: &mut ModelRequestParameters,
    output_mode: OutputMode,
) {
    if output_mode != OutputMode::Prompted || params.output_schema.is_none() {
        return;
    }
    let mut metadata = Map::new();
    metadata.insert(
        "starweaver_instruction_origin".to_string(),
        serde_json::json!("prompted_output"),
    );
    params.instructions.push(PreparedInstruction {
        text: "Return output that matches the provided JSON schema.".to_string(),
        metadata,
    });
}

fn attach_prepared_instructions(
    mut messages: Vec<ModelMessage>,
    instructions: &[PreparedInstruction],
) -> Vec<ModelMessage> {
    if instructions.is_empty() {
        return messages;
    }

    if let Some(ModelMessage::Request(request)) = messages
        .iter_mut()
        .rev()
        .find(|message| matches!(message, ModelMessage::Request(_)))
    {
        let missing_instructions = instructions
            .iter()
            .filter(|instruction| !request_contains_instruction(request, &instruction.text))
            .collect::<Vec<_>>();
        if missing_instructions.is_empty() {
            return messages;
        }
        let instruction_text = instruction_text(&missing_instructions);
        request.instructions = Some(match request.instructions.take() {
            Some(existing) if !existing.trim().is_empty() => {
                format!("{instruction_text}\n\n{existing}")
            }
            _ => instruction_text,
        });
        return messages;
    }

    messages.push(ModelMessage::Request(ModelRequest {
        parts: Vec::new(),
        timestamp: None,
        instructions: Some(instruction_text(&instructions.iter().collect::<Vec<_>>())),
        run_id: None,
        conversation_id: None,
        metadata: Map::new(),
    }));
    messages
}

fn instruction_text(instructions: &[&PreparedInstruction]) -> String {
    instructions
        .iter()
        .map(|instruction| instruction.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn request_contains_instruction(request: &ModelRequest, text: &str) -> bool {
    request
        .instructions
        .as_deref()
        .is_some_and(|instructions| instruction_list_contains(instructions, text))
        || request.parts.iter().any(|part| match part {
            ModelRequestPart::SystemPrompt { text: existing, .. }
            | ModelRequestPart::Instruction { text: existing, .. } => existing == text,
            _ => false,
        })
}

fn instruction_list_contains(instructions: &str, text: &str) -> bool {
    instructions == text
        || instructions
            .split("\n\n")
            .any(|instruction| instruction == text)
}

/// Normalize canonical history according to a provider profile policy.
#[must_use]
pub fn prepare_messages(
    messages: &[ModelMessage],
    normalization: MessageNormalization,
) -> Vec<ModelMessage> {
    match normalization {
        MessageNormalization::PreserveItems => messages.to_vec(),
        MessageNormalization::MergeAdjacentSameRole => merge_adjacent_requests(messages),
        MessageNormalization::SystemField | MessageNormalization::SystemInstruction => {
            lift_system_parts(messages)
        }
        MessageNormalization::WrapInlineSystem => wrap_inline_system_parts(messages),
    }
}

fn merge_adjacent_requests(messages: &[ModelMessage]) -> Vec<ModelMessage> {
    let mut output = Vec::new();
    for message in messages {
        match (output.last_mut(), message) {
            (Some(ModelMessage::Request(previous)), ModelMessage::Request(next)) => {
                previous.parts.extend(next.parts.clone());
                previous.metadata.extend(next.metadata.clone());
                previous.instructions = merge_optional_instructions(
                    previous.instructions.take(),
                    next.instructions.clone(),
                );
            }
            _ => output.push(message.clone()),
        }
    }
    output
}

fn merge_optional_instructions(left: Option<String>, right: Option<String>) -> Option<String> {
    match (left, right) {
        (Some(left), Some(right)) if !left.trim().is_empty() && !right.trim().is_empty() => {
            Some(format!(
                "{left}

{right}"
            ))
        }
        (Some(left), _) if !left.trim().is_empty() => Some(left),
        (_, Some(right)) if !right.trim().is_empty() => Some(right),
        _ => None,
    }
}

fn lift_system_parts(messages: &[ModelMessage]) -> Vec<ModelMessage> {
    let mut lifted = Vec::new();
    let mut output = Vec::new();
    for message in messages {
        match message {
            ModelMessage::Request(request) => {
                if let Some(instructions) = request.instructions.as_ref() {
                    if !instructions.trim().is_empty() {
                        lifted.push(instructions.clone());
                    }
                }
                let mut remaining = Vec::new();
                for part in &request.parts {
                    match part {
                        ModelRequestPart::SystemPrompt { text, .. }
                        | ModelRequestPart::Instruction { text, .. } => lifted.push(text.clone()),
                        other => remaining.push(other.clone()),
                    }
                }
                if !remaining.is_empty() {
                    let mut request = request.clone();
                    request.parts = remaining;
                    request.instructions = None;
                    output.push(ModelMessage::Request(request));
                }
            }
            ModelMessage::Response(_) => output.push(message.clone()),
        }
    }
    if lifted.is_empty() {
        return output;
    }
    let mut metadata = Map::new();
    metadata.insert(
        "starweaver_instruction_origin".to_string(),
        serde_json::json!("lifted_system"),
    );
    output.insert(
        0,
        ModelMessage::Request(ModelRequest {
            parts: vec![ModelRequestPart::SystemPrompt {
                text: lifted.join("\n"),
                metadata,
            }],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: Map::new(),
        }),
    );
    output
}

fn wrap_inline_system_parts(messages: &[ModelMessage]) -> Vec<ModelMessage> {
    messages
        .iter()
        .map(|message| match message {
            ModelMessage::Request(request) => {
                let mut request = request.clone();
                let request_level_instruction =
                    request
                        .instructions
                        .take()
                        .map(|text| ModelRequestPart::UserPrompt {
                            content: vec![crate::message::ContentPart::Text {
                                text: format!("<system>{text}</system>"),
                            }],
                            name: None,
                            metadata: Map::new(),
                        });
                request.parts = request_level_instruction
                    .into_iter()
                    .chain(request.parts.into_iter().map(|part| match part {
                        ModelRequestPart::SystemPrompt { text, metadata }
                        | ModelRequestPart::Instruction { text, metadata } => {
                            ModelRequestPart::UserPrompt {
                                content: vec![crate::message::ContentPart::Text {
                                    text: format!("<system>{text}</system>"),
                                }],
                                name: None,
                                metadata,
                            }
                        }
                        other => other,
                    }))
                    .collect();
                ModelMessage::Request(request)
            }
            ModelMessage::Response(_) => message.clone(),
        })
        .collect()
}

/// Prepared instruction fragment attached to request parameters.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreparedInstruction {
    /// Instruction text.
    pub text: String,
    /// Instruction metadata.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}
