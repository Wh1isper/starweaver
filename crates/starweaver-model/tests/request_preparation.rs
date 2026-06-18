#![allow(missing_docs, clippy::unwrap_used)]

use std::collections::BTreeMap;

use serde_json::{json, Map};
use starweaver_model::{
    prepare_messages, prepare_model_request, ContentPart, MessageNormalization, ModelMessage,
    ModelProfile, ModelRequest, ModelRequestParameters, ModelRequestPart, ModelSettings,
    NativeToolDefinition, OutputMode, PreparedInstruction, ProtocolFamily, StructuredOutputMode,
    ThinkingSettings, ToolReturnPart, CONTEXT_ORIGIN_METADATA, CONTEXT_ORIGIN_TOOL_RETURN_MEDIA,
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
fn unsupported_image_output_falls_back_to_text_with_diagnostic_metadata() {
    let params = ModelRequestParameters {
        output_mode: Some(OutputMode::Image),
        allow_image_output: Some(true),
        allow_text_output: Some(false),
        ..ModelRequestParameters::default()
    };

    let prepared = prepare_model_request(
        vec![ModelMessage::Request(ModelRequest::user_text("draw"))],
        None,
        None,
        params,
        &ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
    );

    assert_eq!(prepared.output_mode, OutputMode::Text);
    assert_eq!(prepared.params.output_mode, Some(OutputMode::Text));
    assert_eq!(prepared.params.allow_image_output, Some(false));
    assert_eq!(prepared.params.allow_text_output, Some(true));
    assert_eq!(
        prepared.metadata["image_output_fallback"]["reason"],
        "unsupported_by_model_profile"
    );
}

#[test]
fn supported_openai_responses_image_output_adds_native_generation_tool() {
    let profile = ModelProfile {
        supports_image_output: true,
        ..ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses)
    };
    let params = ModelRequestParameters {
        output_mode: Some(OutputMode::Image),
        allow_image_output: Some(true),
        allow_text_output: Some(false),
        ..ModelRequestParameters::default()
    };

    let prepared = prepare_model_request(
        vec![ModelMessage::Request(ModelRequest::user_text("draw"))],
        None,
        None,
        params,
        &profile,
    );

    assert_eq!(prepared.output_mode, OutputMode::Image);
    assert_eq!(prepared.params.output_mode, Some(OutputMode::Image));
    assert_eq!(prepared.params.allow_image_output, Some(true));
    assert_eq!(prepared.params.allow_text_output, Some(false));
    assert_eq!(prepared.metadata["image_generation_tool_added"], true);
    assert!(prepared
        .params
        .native_tools
        .iter()
        .any(|tool| tool.tool_type == "image_generation"));
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
fn prepared_instructions_preserve_static_system_prompt_prefix() {
    let params = ModelRequestParameters {
        instructions: vec![PreparedInstruction::dynamic_text("dynamic instruction")],
        ..ModelRequestParameters::default()
    };

    let prepared = prepare_model_request(
        vec![request(vec![
            ModelRequestPart::SystemPrompt {
                text: "static system".to_string(),
                metadata: Map::new(),
            },
            ModelRequestPart::UserPrompt {
                content: vec![ContentPart::Text {
                    text: "answer".to_string(),
                }],
                name: None,
                metadata: Map::new(),
            },
        ])],
        None,
        None,
        params,
        &ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
    );

    let ModelMessage::Request(request) = &prepared.canonical_messages[0] else {
        panic!("expected request")
    };
    assert!(matches!(
        request.parts.first(),
        Some(ModelRequestPart::SystemPrompt { text, .. }) if text == "static system"
    ));
    assert!(matches!(
        request.parts.get(1),
        Some(ModelRequestPart::Instruction { text, metadata })
            if text == "dynamic instruction"
                && metadata["starweaver_instruction_dynamic"] == true
    ));
}

#[test]
fn prepared_instructions_preserve_tool_return_media_control_block() {
    let mut media_metadata = Map::new();
    media_metadata.insert(
        CONTEXT_ORIGIN_METADATA.to_string(),
        json!(CONTEXT_ORIGIN_TOOL_RETURN_MEDIA),
    );
    let params = ModelRequestParameters {
        instructions: vec![PreparedInstruction::dynamic_text("dynamic instruction")],
        ..ModelRequestParameters::default()
    };

    let prepared = prepare_model_request(
        vec![request(vec![
            ModelRequestPart::ToolReturn(ToolReturnPart::new("call_1", "view", json!("ok"))),
            ModelRequestPart::UserPrompt {
                content: vec![
                    ContentPart::Text {
                        text: "Tool view returned provider-native media content.".to_string(),
                    },
                    ContentPart::ImageUrl {
                        url: "https://example.test/image.png".to_string(),
                    },
                ],
                name: None,
                metadata: media_metadata,
            },
        ])],
        None,
        None,
        params,
        &ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
    );

    let ModelMessage::Request(request) = &prepared.canonical_messages[0] else {
        panic!("expected request")
    };
    assert!(matches!(
        request.parts.first(),
        Some(ModelRequestPart::ToolReturn(_))
    ));
    assert!(matches!(
        request.parts.get(1),
        Some(ModelRequestPart::UserPrompt { metadata, .. })
            if metadata.get(CONTEXT_ORIGIN_METADATA) == Some(&json!(CONTEXT_ORIGIN_TOOL_RETURN_MEDIA))
    ));
    assert!(matches!(
        request.parts.get(2),
        Some(ModelRequestPart::Instruction { text, .. }) if text == "dynamic instruction"
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
    let ModelMessage::Request(user_request) = &normalized[1] else {
        panic!("expected user request")
    };
    assert!(matches!(
        &user_request.parts[0],
        ModelRequestPart::UserPrompt { .. }
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

#[test]
fn merge_adjacent_requests_does_not_reapply_historical_instruction_material() {
    let mut old_dynamic = Map::new();
    old_dynamic.insert("starweaver_instruction_dynamic".to_string(), json!(true));
    let mut new_dynamic = Map::new();
    new_dynamic.insert("starweaver_instruction_dynamic".to_string(), json!(true));
    let messages = vec![
        ModelMessage::Request(ModelRequest {
            parts: vec![
                ModelRequestPart::Instruction {
                    text: "old dynamic".to_string(),
                    metadata: old_dynamic,
                },
                ModelRequestPart::UserPrompt {
                    content: vec![ContentPart::Text {
                        text: "old user".to_string(),
                    }],
                    name: None,
                    metadata: Map::new(),
                },
            ],
            timestamp: None,
            instructions: Some("old request instruction".to_string()),
            run_id: None,
            conversation_id: None,
            metadata: Map::new(),
        }),
        ModelMessage::Request(ModelRequest {
            parts: vec![
                ModelRequestPart::Instruction {
                    text: "new dynamic".to_string(),
                    metadata: new_dynamic,
                },
                ModelRequestPart::UserPrompt {
                    content: vec![ContentPart::Text {
                        text: "new user".to_string(),
                    }],
                    name: None,
                    metadata: Map::new(),
                },
            ],
            timestamp: None,
            instructions: Some("new request instruction".to_string()),
            run_id: None,
            conversation_id: None,
            metadata: Map::new(),
        }),
    ];

    let normalized = prepare_messages(&messages, MessageNormalization::MergeAdjacentSameRole);

    assert_eq!(normalized.len(), 1);
    let ModelMessage::Request(request) = &normalized[0] else {
        panic!("expected merged request")
    };
    assert_eq!(
        request.instructions.as_deref(),
        Some("new request instruction")
    );
    assert!(request.parts.iter().any(|part| matches!(
        part,
        ModelRequestPart::UserPrompt { content, .. }
            if matches!(&content[0], ContentPart::Text { text } if text == "old user")
    )));
    assert!(request.parts.iter().any(|part| matches!(
        part,
        ModelRequestPart::UserPrompt { content, .. }
            if matches!(&content[0], ContentPart::Text { text } if text == "new user")
    )));
    assert!(request.parts.iter().any(|part| matches!(
        part,
        ModelRequestPart::Instruction { text, .. } if text == "new dynamic"
    )));
    assert!(!request.parts.iter().any(|part| matches!(
        part,
        ModelRequestPart::Instruction { text, .. } if text == "old dynamic"
    )));
}
