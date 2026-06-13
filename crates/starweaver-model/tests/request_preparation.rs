#![allow(missing_docs, clippy::unwrap_used)]

use std::collections::BTreeMap;

use serde_json::{json, Map};
use starweaver_model::{
    prepare_messages, prepare_model_request, ContentPart, MessageNormalization, ModelMessage,
    ModelProfile, ModelRequest, ModelRequestParameters, ModelRequestPart, ModelSettings,
    NativeToolDefinition, OutputMode, PreparedInstruction, ProtocolFamily, StructuredOutputMode,
    ThinkingSettings,
};

fn request(parts: Vec<ModelRequestPart>) -> ModelMessage {
    ModelMessage::Request(ModelRequest {
        parts,
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Map::new(),
    })
}

#[test]
fn prepared_request_merges_settings_selects_output_and_dedupes_native_tools() {
    let profile = ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses);
    let defaults = ModelSettings {
        max_tokens: Some(128),
        temperature: Some(0.2),
        extra_headers: BTreeMap::from([("x-default".to_string(), "yes".to_string())]),
        ..ModelSettings::default()
    };
    let request_settings = ModelSettings {
        temperature: Some(0.7),
        thinking: Some(ThinkingSettings {
            effort: "high".to_string(),
            budget_tokens: Some(1024),
            mode: Some("enabled".to_string()),
            include_thoughts: Some(true),
            summary: Some("auto".to_string()),
        }),
        ..ModelSettings::default()
    };
    let mut native_config = Map::new();
    native_config.insert("search_context_size".to_string(), json!("low"));
    let params = ModelRequestParameters {
        output_schema: Some(json!({
            "name": "answer",
            "schema": {"type": "object"},
            "strict": true,
        })),
        native_tools: vec![
            NativeToolDefinition::new("web_search_preview").with_config(native_config.clone()),
            NativeToolDefinition::new("web_search_preview").with_config(native_config),
        ],
        ..ModelRequestParameters::default()
    };

    let prepared = prepare_model_request(
        vec![ModelMessage::Request(ModelRequest::user_text("search"))],
        Some(&defaults),
        Some(request_settings),
        params,
        &profile,
    );

    assert_eq!(prepared.output_mode, OutputMode::NativeJsonSchema);
    assert_eq!(
        prepared.params.output_mode,
        Some(OutputMode::NativeJsonSchema)
    );
    assert_eq!(prepared.params.native_tools.len(), 1);
    assert_eq!(prepared.metadata["native_tools_deduplicated"], 1);
    assert_eq!(prepared.settings.unwrap().temperature, Some(0.7));
    assert_eq!(prepared.thinking.unwrap().effort, "high");
}

#[test]
fn prompted_output_mode_attaches_instruction_with_origin_metadata() {
    let profile = ModelProfile {
        default_structured_output_mode: StructuredOutputMode::Prompted,
        ..ModelProfile::for_protocol(ProtocolFamily::AnthropicMessages)
    };
    let params = ModelRequestParameters {
        output_schema: Some(json!({"type": "object"})),
        ..ModelRequestParameters::default()
    };

    let prepared = prepare_model_request(
        vec![ModelMessage::Request(ModelRequest::user_text("answer"))],
        None,
        None,
        params,
        &profile,
    );

    assert_eq!(prepared.output_mode, OutputMode::Prompted);
    assert_eq!(prepared.params.instructions.len(), 1);
    assert_eq!(
        prepared.params.instructions[0].metadata["starweaver_instruction_origin"],
        "prompted_output"
    );
}

#[test]
fn prepared_instructions_are_sorted_and_attached_as_structured_parts() {
    let params = ModelRequestParameters {
        instructions: vec![
            PreparedInstruction::dynamic_text("dynamic instruction"),
            PreparedInstruction::static_text("static instruction"),
        ],
        ..ModelRequestParameters::default()
    };

    let prepared = prepare_model_request(
        vec![ModelMessage::Request(ModelRequest::user_text("answer"))],
        None,
        None,
        params,
        &ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
    );

    let ModelMessage::Request(request) = &prepared.canonical_messages[0] else {
        panic!("expected request")
    };
    assert!(matches!(
        &request.parts[0],
        ModelRequestPart::Instruction { text, metadata }
            if text == "static instruction"
                && metadata["starweaver_instruction_dynamic"] == false
    ));
    assert!(matches!(
        &request.parts[1],
        ModelRequestPart::Instruction { text, metadata }
            if text == "dynamic instruction"
                && metadata["starweaver_instruction_dynamic"] == true
    ));
}

#[test]
fn prepare_messages_merges_adjacent_requests() {
    let messages = vec![
        request(vec![ModelRequestPart::UserPrompt {
            content: vec![ContentPart::Text {
                text: "first".to_string(),
            }],
            name: None,
            metadata: Map::new(),
        }]),
        request(vec![ModelRequestPart::UserPrompt {
            content: vec![ContentPart::Text {
                text: "second".to_string(),
            }],
            name: None,
            metadata: Map::new(),
        }]),
    ];

    let normalized = prepare_messages(&messages, MessageNormalization::MergeAdjacentSameRole);

    assert_eq!(normalized.len(), 1);
    let ModelMessage::Request(request) = &normalized[0] else {
        panic!("expected request")
    };
    assert_eq!(request.parts.len(), 2);
}

#[test]
fn prepare_messages_lifts_system_parts_for_system_field_profiles() {
    let mut dynamic_metadata = Map::new();
    dynamic_metadata.insert("starweaver_instruction_dynamic".to_string(), json!(true));
    let messages = vec![request(vec![
        ModelRequestPart::SystemPrompt {
            text: "system".to_string(),
            metadata: Map::new(),
        },
        ModelRequestPart::Instruction {
            text: "instruction".to_string(),
            metadata: dynamic_metadata,
        },
        ModelRequestPart::UserPrompt {
            content: vec![ContentPart::Text {
                text: "user".to_string(),
            }],
            name: None,
            metadata: Map::new(),
        },
    ])];

    let normalized = prepare_messages(&messages, MessageNormalization::SystemField);

    assert_eq!(normalized.len(), 2);
    let ModelMessage::Request(system_request) = &normalized[0] else {
        panic!("expected system request")
    };
    assert_eq!(
        system_request.metadata["starweaver_instruction_origin"],
        "lifted_system"
    );
    assert!(matches!(
        &system_request.parts[0],
        ModelRequestPart::SystemPrompt { text, .. } if text == "system"
    ));
    assert!(matches!(
        &system_request.parts[1],
        ModelRequestPart::Instruction { text, metadata }
            if text == "instruction" && metadata["starweaver_instruction_dynamic"] == true
    ));
}

#[test]
fn prepare_messages_wraps_inline_system_parts() {
    let messages = vec![request(vec![ModelRequestPart::SystemPrompt {
        text: "system".to_string(),
        metadata: Map::new(),
    }])];

    let normalized = prepare_messages(&messages, MessageNormalization::WrapInlineSystem);

    let ModelMessage::Request(request) = &normalized[0] else {
        panic!("expected request")
    };
    let ModelRequestPart::UserPrompt { content, .. } = &request.parts[0] else {
        panic!("expected user prompt")
    };
    let ContentPart::Text { text } = &content[0] else {
        panic!("expected text")
    };
    assert_eq!(text, "<system>system</system>");
}

#[test]
fn prepared_request_serializes_snapshot_evidence() {
    let profile = ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions);
    let prepared = prepare_model_request(
        vec![ModelMessage::Request(ModelRequest::user_text("hello"))],
        None,
        None,
        ModelRequestParameters::default(),
        &profile,
    );

    let encoded = serde_json::to_value(&prepared).unwrap();
    let decoded: starweaver_model::PreparedModelRequest = serde_json::from_value(encoded).unwrap();

    assert_eq!(
        decoded.profile.protocol,
        ProtocolFamily::OpenAiChatCompletions
    );
    assert_eq!(decoded.output_mode, OutputMode::Text);
    assert_eq!(decoded.params.output_mode, Some(OutputMode::Text));
}
