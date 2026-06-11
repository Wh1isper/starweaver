//! SDK filter parity order and concrete behavior tests.

#![allow(clippy::expect_used)]

use std::{io::Cursor, sync::Arc};

use async_trait::async_trait;
use image::{ImageBuffer, ImageFormat, Rgba};
use starweaver_agent::{
    default_filter_capabilities, default_filter_capabilities_with_config, AgentCapability,
    AgentContext, AgentRunState, CacheFriendlyCompactCapability, ConversationId, FunctionModel,
    FunctionModelInfo, MediaUploadRequest, MediaUploader, ModelConfig, NamedFilterCapability,
    Ratio, RunId, Usage, DEFAULT_FILTER_ORDER,
};
use starweaver_model::{
    ContentPart, MediaPolicy, ModelMessage, ModelRequest, ModelRequestParameters, ModelRequestPart,
    ModelResponse, ModelResponsePart, ModelResponseStreamEvent, ModelSettings, ToolCallPart,
    ToolDefinition, ToolReturnPart,
};

#[tokio::test]
async fn default_filter_capabilities_record_order() -> starweaver_agent::CapabilityResult<()> {
    let request = user_request(vec![ContentPart::Text {
        text: "hello".to_string(),
    }]);
    let mut messages = vec![ModelMessage::Request(request)];
    let mut state = AgentRunState::new(RunId::from_string("run_filter"), ConversationId::new());
    let mut context = AgentContext::default();
    for processor in default_filter_capabilities(None) {
        messages = processor
            .prepare_model_messages_with_context(&mut state, &mut context, messages)
            .await?;
    }

    let Some(ModelMessage::Request(request)) = messages.last() else {
        panic!("last message should be request");
    };
    let Some(order) = request
        .metadata
        .get("starweaver_filter_order")
        .and_then(serde_json::Value::as_array)
    else {
        panic!("filter order metadata should exist");
    };
    let observed = order
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>();
    assert_eq!(observed, DEFAULT_FILTER_ORDER);
    Ok(())
}

#[tokio::test]
async fn media_preflight_corrects_binary_media_and_replaces_corruption(
) -> starweaver_agent::CapabilityResult<()> {
    let request = user_request(vec![
        ContentPart::Binary {
            data: png_bytes(2, 1),
            media_type: "image/jpeg".to_string(),
        },
        ContentPart::Binary {
            data: b"GIF89a".to_vec(),
            media_type: "image/gif".to_string(),
        },
    ]);
    let mut state = AgentRunState::new(RunId::from_string("run_media"), ConversationId::new());
    state.metadata.insert(
        "starweaver_media_policy".to_string(),
        serde_json::to_value(MediaPolicy {
            allow_gif: false,
            ..MediaPolicy::default()
        })
        .expect("media policy"),
    );

    let messages = NamedFilterCapability::new("media_preflight")
        .prepare_model_messages_with_context(
            &mut state,
            &mut AgentContext::default(),
            vec![ModelMessage::Request(request)],
        )
        .await?;
    let content = latest_user_content(&messages);
    assert!(matches!(
        &content[0],
        ContentPart::Binary { media_type, .. } if media_type == "image/png"
    ));
    assert!(matches!(
        &content[1],
        ContentPart::Text { text } if text.contains("media payload was removed")
    ));
    let metadata = latest_request_metadata(&messages);
    assert_eq!(metadata["starweaver_media_replacements"], 1);
    Ok(())
}

#[tokio::test]
async fn media_preflight_limits_newest_images() -> starweaver_agent::CapabilityResult<()> {
    let request = user_request(vec![
        ContentPart::ImageUrl {
            url: "https://example.test/old.png".to_string(),
        },
        ContentPart::ImageUrl {
            url: "https://example.test/new.png".to_string(),
        },
    ]);
    let mut state =
        AgentRunState::new(RunId::from_string("run_media_limit"), ConversationId::new());
    state.metadata.insert(
        "starweaver_media_policy".to_string(),
        serde_json::to_value(MediaPolicy {
            max_images: Some(1),
            ..MediaPolicy::default()
        })
        .expect("media policy"),
    );

    let messages = NamedFilterCapability::new("media_preflight")
        .prepare_model_messages_with_context(
            &mut state,
            &mut AgentContext::default(),
            vec![ModelMessage::Request(request)],
        )
        .await?;
    let content = latest_user_content(&messages);
    assert!(
        matches!(&content[0], ContentPart::Text { text } if text.contains("image count limit"))
    );
    assert!(matches!(&content[1], ContentPart::ImageUrl { url } if url.ends_with("new.png")));
    Ok(())
}

#[tokio::test]
async fn media_compress_reduces_oversized_binary_image() -> starweaver_agent::CapabilityResult<()> {
    let image = valid_noisy_png(240, 240);
    let mut agent_context = AgentContext::default();
    agent_context.model_config.max_image_bytes = 24_000;
    let max_raw =
        starweaver_model::raw_budget_from_base64_limit(agent_context.model_config.max_image_bytes);
    assert!(image.len() > max_raw);
    let request = user_request(vec![ContentPart::Binary {
        data: image,
        media_type: "image/png".to_string(),
    }]);
    let mut state = AgentRunState::new(
        RunId::from_string("run_media_compress"),
        ConversationId::new(),
    );

    let messages = NamedFilterCapability::new("media_compress")
        .prepare_model_messages_with_context(
            &mut state,
            &mut agent_context,
            vec![ModelMessage::Request(request)],
        )
        .await?;
    let compressed_content = latest_user_content(&messages);
    assert!(matches!(
        &compressed_content[0],
        ContentPart::Binary { data, media_type } if media_type == "image/jpeg" && data.len() <= max_raw
    ));
    assert_eq!(
        latest_request_metadata(&messages)["starweaver_media_compressed"],
        serde_json::json!(1)
    );
    Ok(())
}

#[tokio::test]
async fn media_compress_updates_nested_tool_data_url() -> starweaver_agent::CapabilityResult<()> {
    let image = valid_noisy_png(220, 220);
    let mut agent_context = AgentContext::default();
    agent_context.model_config.max_image_bytes = 22_000;
    let max_raw =
        starweaver_model::raw_budget_from_base64_limit(agent_context.model_config.max_image_bytes);
    assert!(image.len() > max_raw);
    let data_url = format!(
        "data:image/png;base64,{}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &image)
    );
    let request = ModelRequest {
        parts: vec![ModelRequestPart::ToolReturn(ToolReturnPart::new(
            "call_media_compress",
            "browser_capture",
            serde_json::json!({
                "items": [{
                    "media": {
                        "data_url": data_url,
                        "media_type": "image/png"
                    }
                }]
            }),
        ))],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    };
    let mut state = AgentRunState::new(
        RunId::from_string("run_tool_media_compress"),
        ConversationId::new(),
    );

    let messages = NamedFilterCapability::new("media_compress")
        .prepare_model_messages_with_context(
            &mut state,
            &mut agent_context,
            vec![ModelMessage::Request(request)],
        )
        .await?;
    let tool_content = messages
        .iter()
        .find_map(|message| match message {
            ModelMessage::Request(request) => request.parts.iter().find_map(|part| match part {
                ModelRequestPart::ToolReturn(tool_return) => Some(&tool_return.content),
                _ => None,
            }),
            ModelMessage::Response(_) => None,
        })
        .expect("tool content");
    let media = &tool_content["items"][0]["media"];
    assert_eq!(media["media_type"], serde_json::json!("image/jpeg"));
    let parsed =
        starweaver_model::parse_data_url(media["data_url"].as_str().expect("compressed data URL"))
            .expect("parse compressed data URL");
    assert!(parsed.data.len() <= max_raw);
    assert_eq!(
        latest_request_metadata(&messages)["starweaver_media_compressed"],
        serde_json::json!(1)
    );
    Ok(())
}

#[tokio::test]
async fn media_preflight_traverses_nested_tool_returns() -> starweaver_agent::CapabilityResult<()> {
    let request = ModelRequest {
        parts: vec![ModelRequestPart::ToolReturn(ToolReturnPart::new(
            "call_media",
            "browser_capture",
            serde_json::json!({
                "items": [{
                    "media": {
                        "data_url": "data:image/gif;base64,R0lGODlhAQABAAAAAA==",
                        "media_type": "image/gif"
                    }
                }]
            }),
        ))],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    };
    let mut state = AgentRunState::new(
        RunId::from_string("run_nested_media"),
        ConversationId::new(),
    );
    state.metadata.insert(
        "starweaver_media_policy".to_string(),
        serde_json::to_value(MediaPolicy {
            allow_gif: false,
            ..MediaPolicy::default()
        })
        .expect("media policy"),
    );

    let messages = NamedFilterCapability::new("media_preflight")
        .prepare_model_messages_with_context(
            &mut state,
            &mut AgentContext::default(),
            vec![ModelMessage::Request(request)],
        )
        .await?;
    let tool_content = messages
        .iter()
        .find_map(|message| match message {
            ModelMessage::Request(request) => request.parts.iter().find_map(|part| match part {
                ModelRequestPart::ToolReturn(tool_return) => Some(&tool_return.content),
                _ => None,
            }),
            ModelMessage::Response(_) => None,
        })
        .expect("tool content");
    assert_eq!(
        tool_content["items"][0]["media"]["type"],
        serde_json::json!("system_reminder")
    );
    assert!(tool_content["items"][0]["media"]["text"]
        .as_str()
        .is_some_and(|text| text.contains("media policy")));
    Ok(())
}

#[tokio::test]
async fn compact_filter_trims_history_like_compactor_builder(
) -> starweaver_agent::CapabilityResult<()> {
    let mut state = AgentRunState::new(RunId::from_string("run_compact"), ConversationId::new());
    state.metadata.insert(
        "starweaver_compact_keep_messages".to_string(),
        serde_json::json!(2),
    );
    state.metadata.insert(
        "starweaver_original_request".to_string(),
        serde_json::json!("Original user goal"),
    );
    state.metadata.insert(
        "starweaver_user_steering".to_string(),
        serde_json::json!(["Keep the current approach"]),
    );
    let kept_summary = ModelMessage::Response(ModelResponse {
        parts: vec![ModelResponsePart::Text {
            text: "previous compact summary".to_string(),
        }],
        usage: starweaver_agent::Usage::default(),
        model_name: None,
        provider: None,
        finish_reason: None,
        timestamp: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::from_iter([("keep".to_string(), serde_json::json!("compact"))]),
    });
    let old_context = user_request(vec![ContentPart::Text {
        text: "before <runtime-context>stale</runtime-context> <project-guidance name=AGENTS.md>old project</project-guidance> <user-rules location=/tmp/RULES.md>old rules</user-rules> after".to_string(),
    }]);
    let media_request = user_request(vec![ContentPart::ImageUrl {
        url: "https://example.test/image.png".to_string(),
    }]);
    let tool_return = ModelRequest {
        parts: vec![ModelRequestPart::ToolReturn(ToolReturnPart::new(
            "call_big",
            "big_tool",
            serde_json::json!({"value": "x".repeat(700)}),
        ))],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    };

    let mut context = AgentContext::default();
    let output = CacheFriendlyCompactCapability::new(None)
        .prepare_model_messages_with_context(
            &mut state,
            &mut context,
            vec![
                kept_summary,
                ModelMessage::Request(old_context),
                ModelMessage::Request(media_request),
                ModelMessage::Request(tool_return),
            ],
        )
        .await?;

    assert!(output.iter().any(|message| match message {
        ModelMessage::Response(response) =>
            response.text_output().contains("previous compact summary"),
        ModelMessage::Request(_) => false,
    }));
    let all_text = output
        .iter()
        .flat_map(request_text_parts)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!all_text.contains("runtime-context"));
    assert!(!all_text.contains("project-guidance"));
    assert!(!all_text.contains("user-rules"));
    assert!(!all_text.contains("old project"));
    assert!(!all_text.contains("old rules"));
    assert!(all_text.contains("[image: https://example.test/image.png]"));
    assert!(all_text.contains("chars truncated"));
    assert!(all_text.contains("<original-request>"));
    assert!(all_text.contains("Original user goal"));
    assert!(all_text.contains("<current-request>"));
    assert!(all_text.contains("[image: https://example.test/image.png]"));
    assert!(all_text.contains("<user-steering>"));
    assert!(all_text.contains("<context-restored>"));
    assert_eq!(latest_request_metadata(&output)["keep"], "compact");
    assert_eq!(
        latest_request_metadata(&output)["starweaver_compacted"],
        true
    );
    Ok(())
}

#[tokio::test]
async fn compact_capability_auto_triggers_from_context_threshold_and_rewrites_history(
) -> starweaver_agent::CapabilityResult<()> {
    let compact_model = FunctionModel::streaming(
        |messages: Vec<ModelMessage>,
         _settings: Option<starweaver_agent::ModelSettings>,
         _info: FunctionModelInfo| {
            let text = messages
                .iter()
                .flat_map(request_text_parts)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(text.contains("Compact the conversation history"));
            let mut response = ModelResponse::text(
                "## Condensed conversation summary\n\n### Analysis\n\nAuto compacted.",
            );
            response.usage = Usage {
                requests: 1,
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
                tool_calls: 0,
            };
            Ok(vec![ModelResponseStreamEvent::FinalResult(Box::new(
                response,
            ))])
        },
    );
    let mut state = AgentRunState::new(
        RunId::from_string("run_auto_compact"),
        ConversationId::new(),
    );
    state.metadata.insert(
        "starweaver_original_request".to_string(),
        serde_json::json!("Original goal"),
    );
    let request = ModelMessage::Request(ModelRequest {
        parts: vec![ModelRequestPart::SystemPrompt {
            text: "Real system prompt".to_string(),
            metadata: serde_json::Map::new(),
        }],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    });
    let mut response = ModelResponse::text("large prior response");
    response.usage = Usage {
        requests: 1,
        input_tokens: 90,
        output_tokens: 5,
        total_tokens: 95,
        ..Usage::default()
    };
    state.message_history = vec![request.clone(), ModelMessage::Response(response)];
    let mut context = AgentContext {
        model_config: ModelConfig {
            context_window: Some(100),
            compact_threshold: Ratio::from_parts_per_thousand(900),
            ..ModelConfig::default()
        },
        message_history: state.message_history.clone(),
        ..AgentContext::default()
    };

    let input_messages = context.message_history.clone();
    let output = CacheFriendlyCompactCapability::new(Some(Arc::new(compact_model)))
        .prepare_model_messages_with_context(&mut state, &mut context, input_messages)
        .await?;

    assert_eq!(output.len(), 3);
    assert_eq!(state.message_history, output);
    assert_eq!(context.message_history, output);
    assert!(matches!(
        &output[0],
        ModelMessage::Request(request) if request.parts.iter().any(|part| matches!(
            part,
            ModelRequestPart::SystemPrompt { text, .. } if text == "Real system prompt"
        ))
    ));
    assert!(matches!(
        &output[1],
        ModelMessage::Response(response)
            if response.text_output().contains("Auto compacted")
                && response.metadata.get("keep") == Some(&serde_json::json!("compact"))
    ));
    assert!(context
        .events
        .events()
        .iter()
        .any(|event| event.kind == "compact_start"));
    assert!(context
        .events
        .events()
        .iter()
        .any(|event| event.kind == "compact_complete"));
    assert_eq!(context.usage.total_tokens, 15);
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn cache_friendly_compactor_inherits_tools_params_and_settings_for_cache_shape(
) -> starweaver_agent::CapabilityResult<()> {
    let compact_model = FunctionModel::streaming(
        |_messages: Vec<ModelMessage>, settings: Option<ModelSettings>, info: FunctionModelInfo| {
            let settings = settings.expect("compact settings");
            assert_eq!(settings.temperature, Some(0.2));
            assert_eq!(
                settings.extra_body.get("route"),
                Some(&serde_json::json!("main"))
            );
            assert!(!settings.extra_body.contains_key("anthropic_cache"));
            assert!(!settings.extra_body.contains_key("thinking"));
            assert!(!settings
                .extra_headers
                .get("anthropic-beta")
                .is_some_and(|value| value.contains("interleaved-thinking")));

            assert_eq!(info.params.tools.len(), 1);
            assert_eq!(info.params.tools[0].name, "view");
            assert_eq!(
                info.params.extra_body.get("route"),
                Some(&serde_json::json!("main"))
            );
            assert!(!info.params.extra_body.contains_key("anthropic_cache"));
            assert!(!info.params.http.extra_body.contains_key("thinking"));
            assert_eq!(info.params.allow_text_output, Some(true));
            assert!(info.params.output_schema.is_none());

            Ok(vec![ModelResponseStreamEvent::FinalResult(Box::new(
                ModelResponse::text(
                    "## Condensed conversation summary\n\n### Analysis\n\nInherited.",
                ),
            ))])
        },
    );
    let compact_model = Arc::new(compact_model) as Arc<dyn starweaver_model::ModelAdapter>;
    let mut settings = ModelSettings {
        temperature: Some(0.2),
        ..ModelSettings::default()
    };
    settings
        .extra_body
        .insert("route".to_string(), serde_json::json!("main"));
    settings
        .extra_body
        .insert("anthropic_cache".to_string(), serde_json::json!(true));
    settings.extra_body.insert(
        "thinking".to_string(),
        serde_json::json!({"type":"enabled"}),
    );
    settings.extra_headers.insert(
        "anthropic-beta".to_string(),
        "interleaved-thinking-2025-05-14,fine-grained-tool-streaming-2025-05-14".to_string(),
    );

    let mut params = ModelRequestParameters::default();
    params.tools.push(ToolDefinition {
        name: "view".to_string(),
        description: Some("View file".to_string()),
        parameters: serde_json::json!({"type":"object"}),
        metadata: serde_json::Map::new(),
    });
    params
        .extra_body
        .insert("route".to_string(), serde_json::json!("main"));
    params
        .extra_body
        .insert("anthropic_cache".to_string(), serde_json::json!(true));
    params.http.extra_body.insert(
        "thinking".to_string(),
        serde_json::json!({"type":"enabled"}),
    );
    params.output_schema = Some(serde_json::json!({"type":"object"}));

    let compact_capability = default_filter_capabilities_with_config(
        Some(&compact_model),
        Some(&settings),
        Some(&params),
    )
    .into_iter()
    .find(|capability| capability.spec().id.as_str() == "starweaver.filter.compact")
    .expect("compact capability");

    let mut state = AgentRunState::new(
        RunId::from_string("run_auto_compact_inherit"),
        ConversationId::new(),
    );
    let request = ModelMessage::Request(user_request(vec![ContentPart::Text {
        text: "hello".to_string(),
    }]));
    let mut response = ModelResponse::text("large prior response");
    response.usage = Usage {
        requests: 1,
        input_tokens: 90,
        output_tokens: 5,
        total_tokens: 95,
        ..Usage::default()
    };
    state.message_history = vec![request.clone(), ModelMessage::Response(response)];
    let mut context = AgentContext {
        model_config: ModelConfig {
            context_window: Some(100),
            compact_threshold: Ratio::from_parts_per_thousand(900),
            ..ModelConfig::default()
        },
        message_history: state.message_history.clone(),
        ..AgentContext::default()
    };

    let input_messages = context.message_history.clone();
    let output = compact_capability
        .prepare_model_messages_with_context(&mut state, &mut context, input_messages)
        .await?;
    assert_eq!(output.len(), 3);
    assert!(context
        .events
        .events()
        .iter()
        .any(|event| event.kind == "compact_complete"));
    Ok(())
}

#[tokio::test]
async fn media_upload_replaces_oversized_binary_with_resource_ref(
) -> starweaver_agent::CapabilityResult<()> {
    let request = user_request(vec![ContentPart::Binary {
        data: png_bytes(1, 1),
        media_type: "image/png".to_string(),
    }]);
    let mut state = AgentRunState::new(RunId::from_string("run_upload"), ConversationId::new());
    state.metadata.insert(
        "starweaver_media_policy".to_string(),
        serde_json::to_value(MediaPolicy {
            max_inline_base64_bytes: Some(4),
            ..MediaPolicy::default()
        })
        .expect("media policy"),
    );

    let processor = NamedFilterCapability::media_upload(Arc::new(FakeUploader));
    let messages = processor
        .prepare_model_messages_with_context(
            &mut state,
            &mut AgentContext::default(),
            vec![ModelMessage::Request(request)],
        )
        .await?;
    let content = latest_user_content(&messages);
    assert!(matches!(
        &content[0],
        ContentPart::ResourceRef { uri, media_type, resource_type, .. }
            if uri == "resource://uploaded/image" && media_type == "image/png" && resource_type == "image"
    ));
    assert_eq!(
        latest_request_metadata(&messages)["starweaver_media_uploaded"],
        1
    );
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn concrete_filters_inject_runtime_context_and_repair_tool_args(
) -> starweaver_agent::CapabilityResult<()> {
    let mut state = AgentRunState::new(RunId::from_string("run_context"), ConversationId::new());
    state.metadata.insert(
        "starweaver_runtime_instructions".to_string(),
        serde_json::json!("Prefer concise answers."),
    );
    state.metadata.insert(
        "starweaver_auto_load_files".to_string(),
        serde_json::json!([{ "path": "README.md", "content": "Loaded evidence" }]),
    );
    state.metadata.insert(
        "starweaver_cold_start_tool_return_limit".to_string(),
        serde_json::json!(16),
    );

    let old_tool_content = "abcdefghijklmnopqrstuvwxyz";
    let messages = vec![
        ModelMessage::Request(ModelRequest {
            parts: vec![ModelRequestPart::ToolReturn(ToolReturnPart::new(
                "call_1",
                "big_tool",
                serde_json::json!(old_tool_content),
            ))],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        }),
        ModelMessage::Response(ModelResponse {
            parts: vec![
                ModelResponsePart::ToolCall(ToolCallPart {
                    id: "call_2".to_string(),
                    name: "json_tool".to_string(),
                    arguments: serde_json::json!("{\"ok\":true}").into(),
                }),
                ModelResponsePart::Thinking {
                    text: "  keep reasoning   \n".to_string(),
                    signature: Some(String::new()),
                },
            ],
            usage: starweaver_agent::Usage::default(),
            model_name: None,
            provider: None,
            finish_reason: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        }),
    ];

    let mut output = NamedFilterCapability::new("cold_start")
        .prepare_model_messages_with_context(&mut state, &mut AgentContext::default(), messages)
        .await?;
    output = NamedFilterCapability::new("auto_load_files")
        .prepare_model_messages_with_context(&mut state, &mut AgentContext::default(), output)
        .await?;
    output = NamedFilterCapability::new("runtime_instructions")
        .prepare_model_messages_with_context(&mut state, &mut AgentContext::default(), output)
        .await?;
    output = NamedFilterCapability::new("tool_args")
        .prepare_model_messages_with_context(&mut state, &mut AgentContext::default(), output)
        .await?;
    output = NamedFilterCapability::new("reasoning_normalize")
        .prepare_model_messages_with_context(&mut state, &mut AgentContext::default(), output)
        .await?;

    assert!(output.iter().any(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().any(|part| match part {
            ModelRequestPart::Instruction { text, .. } => text == "Prefer concise answers.",
            _ => false,
        }),
        ModelMessage::Response(_) => false,
    }));
    assert!(output.iter().any(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().any(|part| match part {
            ModelRequestPart::UserPrompt { content, .. } => content.iter().any(|part| match part {
                ContentPart::Text { text } => text.contains("Loaded evidence"),
                _ => false,
            }),
            _ => false,
        }),
        ModelMessage::Response(_) => false,
    }));
    let old_tool_return = output
        .iter()
        .find_map(|message| match message {
            ModelMessage::Request(request) => request.parts.iter().find_map(|part| match part {
                ModelRequestPart::ToolReturn(tool_return) => tool_return.content.as_str(),
                _ => None,
            }),
            ModelMessage::Response(_) => None,
        })
        .expect("tool return");
    assert!(old_tool_return.contains("chars truncated"));
    assert!(old_tool_return.starts_with(&old_tool_content[..8]));
    assert!(old_tool_return.ends_with(&old_tool_content[old_tool_content.len() - 8..]));

    let response = output
        .iter()
        .find_map(|message| match message {
            ModelMessage::Response(response) => Some(response),
            ModelMessage::Request(_) => None,
        })
        .expect("response");
    assert!(matches!(
        &response.parts[0],
        ModelResponsePart::ToolCall(call) if call.arguments == serde_json::json!({"ok": true})
    ));
    assert!(matches!(
        &response.parts[1],
        ModelResponsePart::Thinking { text, signature } if text == "keep reasoning" && signature.is_none()
    ));
    Ok(())
}

struct FakeUploader;

#[async_trait]
impl MediaUploader for FakeUploader {
    async fn upload(&self, request: MediaUploadRequest) -> Result<ContentPart, String> {
        Ok(ContentPart::ResourceRef {
            uri: "resource://uploaded/image".to_string(),
            media_type: request.media_type,
            resource_type: "image".to_string(),
            metadata: serde_json::Map::new(),
        })
    }
}

fn user_request(content: Vec<ContentPart>) -> ModelRequest {
    ModelRequest {
        parts: vec![ModelRequestPart::UserPrompt {
            content,
            name: None,
            metadata: serde_json::Map::new(),
        }],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    }
}

fn request_text_parts(message: &ModelMessage) -> Vec<String> {
    match message {
        ModelMessage::Request(request) => request
            .parts
            .iter()
            .flat_map(|part| match part {
                ModelRequestPart::UserPrompt { content, .. } => content
                    .iter()
                    .filter_map(|content| match content {
                        ContentPart::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                ModelRequestPart::ToolReturn(tool_return) => vec![tool_return.content.to_string()],
                ModelRequestPart::SystemPrompt { text, .. }
                | ModelRequestPart::RetryPrompt { text, .. }
                | ModelRequestPart::Instruction { text, .. } => vec![text.clone()],
            })
            .collect(),
        ModelMessage::Response(response) => response
            .parts
            .iter()
            .filter_map(|part| match part {
                ModelResponsePart::Text { text } | ModelResponsePart::Thinking { text, .. } => {
                    Some(text.clone())
                }
                _ => None,
            })
            .collect(),
    }
}

fn latest_user_content(messages: &[ModelMessage]) -> Vec<ContentPart> {
    messages
        .iter()
        .rev()
        .find_map(|message| match message {
            ModelMessage::Request(request) => {
                request.parts.iter().rev().find_map(|part| match part {
                    ModelRequestPart::UserPrompt { content, .. } => Some(content.clone()),
                    _ => None,
                })
            }
            ModelMessage::Response(_) => None,
        })
        .expect("user content")
}

fn latest_request_metadata(
    messages: &[ModelMessage],
) -> serde_json::Map<String, serde_json::Value> {
    messages
        .iter()
        .rev()
        .find_map(|message| match message {
            ModelMessage::Request(request) => Some(request.metadata.clone()),
            ModelMessage::Response(_) => None,
        })
        .expect("request metadata")
}

fn valid_noisy_png(width: u32, height: u32) -> Vec<u8> {
    let image = ImageBuffer::from_fn(width, height, |x, y| {
        Rgba([
            u8::try_from((x * 37 + y * 17) % 256).expect("red channel"),
            u8::try_from((x * 11 + y * 53) % 256).expect("green channel"),
            u8::try_from((x * 97 + y * 7) % 256).expect("blue channel"),
            if (x + y) % 5 == 0 { 128 } else { 255 },
        ])
    });
    let mut bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode png");
    bytes.into_inner()
}

fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    bytes.extend_from_slice(&13u32.to_be_bytes());
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&[8, 2, 0, 0, 0]);
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(b"IEND");
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes
}
