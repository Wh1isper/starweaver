use serde_json::{Map, Value};

use crate::{
    adapter::ModelRequestParameters,
    message::{ModelMessage, ModelRequest, ModelRequestPart},
    profile::{ModelProfile, NativeToolKind},
    request::{
        context_origin_metadata, CONTEXT_ORIGIN_TOOL_RETURN_MEDIA, INSTRUCTION_DYNAMIC_METADATA,
        INSTRUCTION_ORIGIN_METADATA,
    },
    settings::ModelSettings,
};

use super::{
    normalization::prepare_messages, OutputMode, PreparedInstruction, PreparedModelRequest,
};

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
        .filter(|_| profile.supports_thinking || profile.thinking_always_enabled)
    {
        prepared_params.thinking.get_or_insert(thinking);
    }

    filter_function_tools_by_choice(&mut prepared_params, settings.as_ref(), &mut metadata);
    filter_native_tools(&mut prepared_params, profile, &mut metadata);
    dedupe_native_tools(&mut prepared_params, &mut metadata);
    transform_json_schemas(&mut prepared_params, profile, &mut metadata);
    attach_structured_output_instruction(&mut prepared_params, output_mode, profile);
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

fn filter_function_tools_by_choice(
    params: &mut ModelRequestParameters,
    settings: Option<&ModelSettings>,
    metadata: &mut Map<String, Value>,
) {
    if matches!(
        settings.and_then(|settings| settings.tool_choice.as_ref()),
        Some(crate::settings::ToolChoice::None)
    ) {
        let removed = params.tools.len();
        params.tools.clear();
        if removed > 0 {
            metadata.insert(
                "function_tools_filtered".to_string(),
                serde_json::json!(removed),
            );
        }
        return;
    }

    let Some(names) = settings.and_then(|settings| match &settings.tool_choice {
        Some(crate::settings::ToolChoice::Tools { names }) => Some(names.as_slice()),
        Some(crate::settings::ToolChoice::ToolOrOutput { function_tools }) => {
            Some(function_tools.as_slice())
        }
        Some(
            crate::settings::ToolChoice::Auto
            | crate::settings::ToolChoice::None
            | crate::settings::ToolChoice::Required
            | crate::settings::ToolChoice::Tool { .. },
        )
        | None => None,
    }) else {
        return;
    };
    let allowed = names
        .iter()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let before = params.tools.len();
    params
        .tools
        .retain(|tool| allowed.contains(tool.name.as_str()));
    let removed = before.saturating_sub(params.tools.len());
    if removed > 0 {
        metadata.insert(
            "function_tools_filtered".to_string(),
            serde_json::json!(removed),
        );
    }
}

fn filter_native_tools(
    params: &mut ModelRequestParameters,
    profile: &ModelProfile,
    metadata: &mut Map<String, Value>,
) {
    let before = params.native_tools.len();
    params.native_tools.retain(|tool| {
        NativeToolKind::from_tool_type(&tool.tool_type)
            .is_some_and(|kind| profile.supported_native_tools.contains(&kind))
    });
    let removed = before.saturating_sub(params.native_tools.len());
    if removed > 0 {
        metadata.insert(
            "native_tools_filtered".to_string(),
            serde_json::json!(removed),
        );
    }
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

fn transform_json_schemas(
    params: &mut ModelRequestParameters,
    profile: &ModelProfile,
    metadata: &mut Map<String, Value>,
) {
    let Some(transformer) = profile.json_schema_transformer else {
        return;
    };
    for tool in &mut params.tools {
        tool.parameters = transformer.transform_schema(&tool.parameters);
    }
    if let Some(output_schema) = &mut params.output_schema {
        if let Some(schema) = output_schema.get_mut("schema") {
            *schema = transformer.transform_schema(schema);
        } else {
            *output_schema = transformer.transform_schema(output_schema);
        }
    }
    metadata.insert(
        "json_schema_transformer".to_string(),
        serde_json::json!(transformer),
    );
}

fn attach_structured_output_instruction(
    params: &mut ModelRequestParameters,
    output_mode: OutputMode,
    profile: &ModelProfile,
) {
    let needs_instruction = output_mode == OutputMode::Prompted
        || (output_mode == OutputMode::NativeJsonSchema
            && profile.native_output_requires_schema_in_instructions);
    if !needs_instruction || params.output_schema.is_none() {
        return;
    }
    let mut metadata = Map::new();
    metadata.insert(
        INSTRUCTION_ORIGIN_METADATA.to_string(),
        serde_json::json!(if output_mode == OutputMode::Prompted {
            "prompted_output"
        } else {
            "native_output_schema"
        }),
    );
    let Some(schema) = params.output_schema.as_ref() else {
        return;
    };
    let schema_placeholder = ["{", "schema", "}"].concat();
    let text = profile
        .prompted_output_template
        .replace(&schema_placeholder, &schema.to_string());
    params.instructions.push(PreparedInstruction {
        text,
        dynamic: false,
        metadata,
    });
}

/// Attach prepared instruction fragments to the latest request, preserving static-before-dynamic order.
#[must_use]
pub fn attach_prepared_instructions(
    mut messages: Vec<ModelMessage>,
    instructions: &[PreparedInstruction],
) -> Vec<ModelMessage> {
    if instructions.is_empty() {
        return messages;
    }
    let sorted_instructions = PreparedInstruction::sorted(instructions);

    if let Some(ModelMessage::Request(request)) = messages
        .iter_mut()
        .rev()
        .find(|message| matches!(message, ModelMessage::Request(_)))
    {
        let missing_instructions = sorted_instructions
            .into_iter()
            .filter(|instruction| !request_contains_instruction(request, &instruction.text))
            .collect::<Vec<_>>();
        if missing_instructions.is_empty() {
            return messages;
        }
        let parts = missing_instructions
            .into_iter()
            .map(|instruction| instruction.to_request_part())
            .collect::<Vec<_>>();
        let insert_at = request_instruction_insert_index(request);
        request.parts.splice(insert_at..insert_at, parts);
        return messages;
    }

    messages.push(ModelMessage::Request(ModelRequest {
        parts: sorted_instructions
            .into_iter()
            .map(|instruction| instruction.to_request_part())
            .collect(),
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Map::new(),
    }));
    messages
}

fn request_control_prefix_len(request: &ModelRequest) -> usize {
    request
        .parts
        .iter()
        .take_while(|part| is_control_prefix_part(part))
        .count()
}

fn request_instruction_insert_index(request: &ModelRequest) -> usize {
    let control_prefix_len = request_control_prefix_len(request);
    control_prefix_len
        + request.parts[control_prefix_len..]
            .iter()
            .take_while(|part| is_static_instruction_prefix_part(part))
            .count()
}

fn is_control_prefix_part(part: &ModelRequestPart) -> bool {
    match part {
        ModelRequestPart::ToolReturn(_) | ModelRequestPart::RetryPrompt { .. } => true,
        ModelRequestPart::UserPrompt { metadata, .. } => context_origin_metadata(metadata)
            .is_some_and(|origin| origin == CONTEXT_ORIGIN_TOOL_RETURN_MEDIA),
        ModelRequestPart::SystemPrompt { .. } | ModelRequestPart::Instruction { .. } => false,
    }
}

fn is_static_instruction_prefix_part(part: &ModelRequestPart) -> bool {
    match part {
        ModelRequestPart::SystemPrompt { .. } => true,
        ModelRequestPart::Instruction { metadata, .. } => !metadata
            .get(INSTRUCTION_DYNAMIC_METADATA)
            .and_then(Value::as_bool)
            .unwrap_or(false),
        ModelRequestPart::UserPrompt { .. }
        | ModelRequestPart::ToolReturn(_)
        | ModelRequestPart::RetryPrompt { .. } => false,
    }
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
