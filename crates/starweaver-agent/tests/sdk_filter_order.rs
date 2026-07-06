//! SDK filter order and concrete behavior tests.

#![allow(clippy::expect_used)]

use std::{
    io::Cursor,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use chrono::{Duration, Utc};
use image::{ImageBuffer, ImageFormat, Rgba};
use starweaver_agent::{
    AgentCapability, AgentContext, AgentRunState, CacheFriendlyCompactCapability, ConversationId,
    DEFAULT_FILTER_ORDER, FunctionModel, FunctionModelInfo, MediaUploadRequest, MediaUploader,
    ModelCapability, ModelConfig, NamedFilterCapability, PerThousandRatio, RunId,
    ShellProcessSnapshot, ShellProcessStatus, Usage, attach_environment,
    default_filter_capabilities, default_filter_capabilities_with_config,
};
use starweaver_core::Metadata;
use starweaver_environment::VirtualEnvironmentProvider;
use starweaver_model::{
    CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT, CONTEXT_ORIGIN_HANDOFF, CONTEXT_ORIGIN_METADATA,
    CONTEXT_ORIGIN_RUNTIME_CONTEXT, CONTEXT_ORIGIN_TOOL_RETURN_MEDIA, ContentPart, MediaPolicy,
    ModelMessage, ModelRequest, ModelRequestParameters, ModelRequestPart, ModelResponse,
    ModelResponsePart, ModelResponseStreamEvent, ModelSettings, ProviderInfo, ProviderPartInfo,
    ProviderReplaySettings, ToolArguments, ToolCallPart, ToolDefinition, ToolReturnPart,
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
async fn default_filter_capabilities_do_not_duplicate_metadata_auto_loaded_files()
-> starweaver_agent::CapabilityResult<()> {
    let request = user_request(vec![ContentPart::Text {
        text: "hello".to_string(),
    }]);
    let mut messages = vec![ModelMessage::Request(request)];
    let mut state = AgentRunState::new(
        RunId::from_string("run_auto_load_once"),
        ConversationId::new(),
    );
    state.metadata.insert(
        "starweaver_auto_load_files".to_string(),
        serde_json::json!([{ "path": "README.md", "content": "Loaded evidence" }]),
    );
    let mut context = AgentContext::default();

    for processor in default_filter_capabilities(None) {
        messages = processor
            .prepare_model_messages_with_context(&mut state, &mut context, messages)
            .await?;
    }

    let text_parts = request_text_parts(messages.last().expect("latest request"));
    let auto_loaded_blocks = text_parts
        .iter()
        .filter(|text| text.contains("<auto-loaded-files>"))
        .collect::<Vec<_>>();
    assert_eq!(auto_loaded_blocks.len(), 1);
    assert_eq!(auto_loaded_blocks[0].matches("Loaded evidence").count(), 1);
    assert_eq!(
        latest_request_metadata(&messages)["starweaver_auto_load_state_metadata_injected"],
        serde_json::json!(true)
    );
    Ok(())
}

#[tokio::test]
async fn metadata_dynamic_instructions_are_inserted_after_tool_control_parts()
-> starweaver_agent::CapabilityResult<()> {
    let request = ModelRequest {
        parts: vec![
            ModelRequestPart::ToolReturn(ToolReturnPart::new(
                "call_1",
                "tool",
                serde_json::json!({"ok": true}),
            )),
            ModelRequestPart::UserPrompt {
                content: vec![ContentPart::Text {
                    text: "continue".to_string(),
                }],
                name: None,
                metadata: serde_json::Map::from_iter([(
                    CONTEXT_ORIGIN_METADATA.to_string(),
                    serde_json::json!(CONTEXT_ORIGIN_TOOL_RETURN_MEDIA),
                )]),
            },
        ],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    };
    let mut state = AgentRunState::new(
        RunId::from_string("run_metadata_instruction"),
        ConversationId::new(),
    );
    state.metadata.insert(
        "starweaver_environment_context".to_string(),
        serde_json::json!("<environment-context>fresh</environment-context>"),
    );

    let messages = NamedFilterCapability::new("environment_context")
        .prepare_model_messages_with_context(
            &mut state,
            &mut AgentContext::default(),
            vec![ModelMessage::Request(request)],
        )
        .await?;
    let ModelMessage::Request(request) = messages.last().expect("request") else {
        panic!("expected request");
    };

    assert!(matches!(request.parts[0], ModelRequestPart::ToolReturn(_)));
    assert!(matches!(
        &request.parts[1],
        ModelRequestPart::UserPrompt { metadata, .. }
            if metadata.get(CONTEXT_ORIGIN_METADATA) == Some(&serde_json::json!(CONTEXT_ORIGIN_TOOL_RETURN_MEDIA))
    ));
    assert!(matches!(
        &request.parts[2],
        ModelRequestPart::UserPrompt { content, metadata, .. }
            if metadata.get(CONTEXT_ORIGIN_METADATA)
                == Some(&serde_json::json!(CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT))
                && matches!(&content[0], ContentPart::Text { text } if text.contains("<environment-context>fresh</environment-context>"))
    ));
    Ok(())
}

#[tokio::test]
async fn handoff_filter_uses_shared_restored_request_builder()
-> starweaver_agent::CapabilityResult<()> {
    let request = ModelRequest {
        parts: vec![
            ModelRequestPart::SystemPrompt {
                text: "static policy".to_string(),
                metadata: serde_json::Map::new(),
            },
            ModelRequestPart::UserPrompt {
                content: vec![ContentPart::Text {
                    text: "continue".to_string(),
                }],
                name: None,
                metadata: serde_json::Map::new(),
            },
        ],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    };
    let mut state = AgentRunState::new(RunId::from_string("run_handoff"), ConversationId::new());
    state.metadata.insert(
        "starweaver_handoff".to_string(),
        serde_json::json!("Resume the delegated implementation plan."),
    );

    let mut context = AgentContext {
        user_prompts: Some(vec![ContentPart::Text {
            text: "Original request".to_string(),
        }]),
        ..AgentContext::default()
    };
    let messages = NamedFilterCapability::new("handoff")
        .prepare_model_messages_with_context(
            &mut state,
            &mut context,
            vec![ModelMessage::Request(request)],
        )
        .await?;
    let ModelMessage::Request(request) = messages.last().expect("request") else {
        panic!("expected request");
    };

    let text = request_text_parts(&ModelMessage::Request(request.clone())).join("\n");
    assert!(text.contains("<context-restored>"));
    assert!(text.contains("<original-request>"));
    assert!(text.contains("Resume the delegated implementation plan."));
    assert!(!text.contains("<current-request>"));
    assert!(matches!(
        &request.parts[0],
        ModelRequestPart::SystemPrompt { text, .. } if text == "static policy"
    ));
    assert!(request.parts[1..].iter().any(|part| matches!(
        part,
        ModelRequestPart::UserPrompt { metadata, .. }
            if metadata.get(CONTEXT_ORIGIN_METADATA)
                == Some(&serde_json::json!(CONTEXT_ORIGIN_HANDOFF))
    )));
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn system_prompt_filter_removes_stale_prompts_and_reinjects_canonical_first_prompt()
-> starweaver_agent::CapabilityResult<()> {
    let canonical_request = ModelRequest {
        parts: vec![
            ModelRequestPart::SystemPrompt {
                text: "canonical system".to_string(),
                metadata: serde_json::Map::from_iter([(
                    "source".to_string(),
                    serde_json::json!("canonical"),
                )]),
            },
            ModelRequestPart::UserPrompt {
                content: vec![ContentPart::Text {
                    text: "first".to_string(),
                }],
                name: None,
                metadata: serde_json::Map::new(),
            },
        ],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    };
    let stale_first_request = ModelRequest {
        parts: vec![
            ModelRequestPart::SystemPrompt {
                text: "stale first".to_string(),
                metadata: serde_json::Map::new(),
            },
            ModelRequestPart::UserPrompt {
                content: vec![ContentPart::Text {
                    text: "hello".to_string(),
                }],
                name: None,
                metadata: serde_json::Map::new(),
            },
        ],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    };
    let stale_second_request = ModelRequest {
        parts: vec![
            ModelRequestPart::SystemPrompt {
                text: "stale second".to_string(),
                metadata: serde_json::Map::new(),
            },
            ModelRequestPart::UserPrompt {
                content: vec![ContentPart::Text {
                    text: "continue".to_string(),
                }],
                name: None,
                metadata: serde_json::Map::new(),
            },
        ],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    };
    let mut state = AgentRunState::new(
        RunId::from_string("run_system_prompt"),
        ConversationId::new(),
    );
    state.message_history = vec![ModelMessage::Request(canonical_request)];

    let messages = NamedFilterCapability::new("system_prompt")
        .prepare_model_messages_with_context(
            &mut state,
            &mut AgentContext::default(),
            vec![
                ModelMessage::Request(stale_first_request),
                ModelMessage::Response(ModelResponse::text("ok")),
                ModelMessage::Request(stale_second_request),
            ],
        )
        .await?;

    let system_prompts = messages
        .iter()
        .filter_map(|message| match message {
            ModelMessage::Request(request) => Some(
                request
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        ModelRequestPart::SystemPrompt { text, metadata } => {
                            Some((text.clone(), metadata.clone()))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            ),
            ModelMessage::Response(_) => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(system_prompts[0].len(), 1);
    assert_eq!(system_prompts[0][0].0, "canonical system");
    assert_eq!(system_prompts[0][0].1["source"], "canonical");
    assert!(system_prompts[1].is_empty());
    Ok(())
}

#[tokio::test]
async fn metadata_dynamic_instructions_preserve_static_system_prompt_prefix()
-> starweaver_agent::CapabilityResult<()> {
    let request = ModelRequest {
        parts: vec![
            ModelRequestPart::SystemPrompt {
                text: "static policy".to_string(),
                metadata: serde_json::Map::new(),
            },
            ModelRequestPart::UserPrompt {
                content: vec![ContentPart::Text {
                    text: "hello".to_string(),
                }],
                name: None,
                metadata: serde_json::Map::new(),
            },
        ],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    };
    let mut state = AgentRunState::new(
        RunId::from_string("run_static_metadata_instruction"),
        ConversationId::new(),
    );
    state.metadata.insert(
        "starweaver_environment_context".to_string(),
        serde_json::json!("<environment-context>fresh</environment-context>"),
    );

    let messages = NamedFilterCapability::new("environment_context")
        .prepare_model_messages_with_context(
            &mut state,
            &mut AgentContext::default(),
            vec![ModelMessage::Request(request)],
        )
        .await?;
    let ModelMessage::Request(request) = messages.last().expect("request") else {
        panic!("expected request");
    };

    assert!(matches!(
        &request.parts[0],
        ModelRequestPart::SystemPrompt { text, .. } if text == "static policy"
    ));
    assert!(matches!(
        &request.parts[1],
        ModelRequestPart::UserPrompt { content, metadata, .. }
            if metadata.get(CONTEXT_ORIGIN_METADATA)
                == Some(&serde_json::json!(CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT))
                && matches!(&content[0], ContentPart::Text { text } if text.contains("<environment-context>fresh</environment-context>"))
    ));
    assert!(matches!(
        &request.parts[2],
        ModelRequestPart::UserPrompt { content, metadata, .. }
            if metadata.get(CONTEXT_ORIGIN_METADATA).is_none()
                && matches!(&content[0], ContentPart::Text { text } if text == "hello")
    ));
    Ok(())
}

#[tokio::test]
async fn media_preflight_corrects_binary_media_and_replaces_corruption()
-> starweaver_agent::CapabilityResult<()> {
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
    assert!(matches!(&content[0], ContentPart::Text { text } if text.contains("max_images=1")));
    assert!(matches!(&content[1], ContentPart::ImageUrl { url } if url.ends_with("new.png")));
    Ok(())
}

#[tokio::test]
async fn media_preflight_limits_newest_videos() -> starweaver_agent::CapabilityResult<()> {
    let request = user_request(vec![
        ContentPart::FileUrl {
            url: "https://example.test/old.mp4".to_string(),
            media_type: "video/mp4".to_string(),
        },
        ContentPart::FileUrl {
            url: "https://example.test/new.mp4".to_string(),
            media_type: "video/mp4".to_string(),
        },
    ]);
    let mut state =
        AgentRunState::new(RunId::from_string("run_video_limit"), ConversationId::new());
    state.metadata.insert(
        "starweaver_media_policy".to_string(),
        serde_json::to_value(MediaPolicy {
            max_videos: Some(1),
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
    assert!(matches!(&content[0], ContentPart::Text { text } if text.contains("max_videos=1")));
    assert!(matches!(&content[1], ContentPart::FileUrl { url, .. } if url.ends_with("new.mp4")));
    Ok(())
}

#[tokio::test]
async fn media_preflight_corrects_data_url_prefixes() -> starweaver_agent::CapabilityResult<()> {
    let image = valid_noisy_png(2, 1);
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &image);
    let jpeg_prefixed_png = format!("data:image/jpeg;base64,{encoded}");
    let request = ModelRequest {
        parts: vec![
            ModelRequestPart::UserPrompt {
                content: vec![ContentPart::DataUrl {
                    data_url: jpeg_prefixed_png.clone(),
                    media_type: "image/jpeg".to_string(),
                }],
                name: None,
                metadata: serde_json::Map::new(),
            },
            ModelRequestPart::ToolReturn(ToolReturnPart::new(
                "call_media_prefix",
                "browser_capture",
                serde_json::json!({
                    "media": {
                        "data_url": jpeg_prefixed_png,
                        "media_type": "image/jpeg"
                    }
                }),
            )),
        ],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    };

    let messages = NamedFilterCapability::new("media_preflight")
        .prepare_model_messages_with_context(
            &mut AgentRunState::new(
                RunId::from_string("run_data_url_prefix"),
                ConversationId::new(),
            ),
            &mut AgentContext::default(),
            vec![ModelMessage::Request(request)],
        )
        .await?;

    let content = latest_user_content(&messages);
    let ContentPart::DataUrl {
        data_url,
        media_type,
    } = &content[0]
    else {
        panic!("expected corrected data URL content");
    };
    assert_eq!(media_type, "image/png");
    let parsed = starweaver_model::parse_data_url(data_url).expect("parse corrected user data URL");
    assert_eq!(parsed.media_type, "image/png");

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
        tool_content["media"]["media_type"],
        serde_json::json!("image/png")
    );
    let corrected_tool_data_url = tool_content["media"]["data_url"]
        .as_str()
        .expect("corrected tool data URL");
    assert!(corrected_tool_data_url.starts_with("data:image/png;base64,"));
    Ok(())
}

#[tokio::test]
async fn media_split_segments_tall_binary_images_before_compression()
-> starweaver_agent::CapabilityResult<()> {
    let image = valid_noisy_png(16, 32);
    let request = user_request(vec![ContentPart::Binary {
        data: image,
        media_type: "image/png".to_string(),
    }]);
    let mut context = AgentContext::default();
    context.model_config.image_split_max_height = 12;
    context.model_config.image_split_overlap = 2;

    let messages = NamedFilterCapability::new("media_split")
        .prepare_model_messages_with_context(
            &mut AgentRunState::new(RunId::from_string("run_media_split"), ConversationId::new()),
            &mut context,
            vec![ModelMessage::Request(request)],
        )
        .await?;

    let split_content = latest_user_content(&messages);
    assert_eq!(split_content.len(), 3);
    assert!(split_content.iter().all(|part| {
        matches!(part, ContentPart::Binary { data, media_type } if media_type == "image/png" && !data.is_empty())
    }));
    assert_eq!(
        latest_request_metadata(&messages)["starweaver_media_split"],
        serde_json::json!({"images": 1, "segments": 3})
    );
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
    assert!(
        tool_content["items"][0]["media"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("media policy"))
    );
    Ok(())
}

#[tokio::test]
async fn compact_filter_trims_history_like_compactor_builder()
-> starweaver_agent::CapabilityResult<()> {
    let mut state = AgentRunState::new(RunId::from_string("run_compact"), ConversationId::new());
    state.metadata.insert(
        "starweaver_compact_keep_messages".to_string(),
        serde_json::json!(2),
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

    let mut context = AgentContext {
        user_prompts: Some(vec![
            ContentPart::Text {
                text: "Original user goal".to_string(),
            },
            ContentPart::ImageUrl {
                url: "https://example.test/original.png".to_string(),
            },
        ]),
        previous_assistant_response_reference: Some(
            "1. Use cached requests\n2. Add restore tests".to_string(),
        ),
        steering_messages: vec!["Keep the current approach".to_string()],
        ..AgentContext::default()
    };
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
    assert!(all_text.contains("<previous-assistant-reference>"));
    assert!(all_text.contains("1. Use cached requests\n2. Add restore tests"));
    assert!(all_text.contains("<original-request>"));
    assert!(all_text.contains("Original user goal"));
    assert!(contains_image_url(
        &output,
        "https://example.test/original.png"
    ));
    assert!(!all_text.contains("<current-request>"));
    assert!(all_text.contains("<user-steering>"));
    assert!(all_text.contains("[User Steering] Keep the current approach"));
    assert!(all_text.contains("<context-restored>"));
    assert_restore_block_order(&all_text);
    assert_eq!(latest_request_metadata(&output)["keep"], "compact");
    assert_eq!(
        latest_request_metadata(&output)["starweaver_compacted"],
        true
    );
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn compact_capability_auto_triggers_from_context_threshold_and_rewrites_history()
-> starweaver_agent::CapabilityResult<()> {
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
                cache_write_tokens: 0,
                cache_read_tokens: 0,
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
            compact_threshold: PerThousandRatio::from_per_thousand(900),
            ..ModelConfig::default()
        },
        message_history: state.message_history.clone(),
        user_prompts: Some(vec![ContentPart::Text {
            text: "Original goal".to_string(),
        }]),
        previous_assistant_response_reference: Some(
            "1. first option\n2. second option".to_string(),
        ),
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
    assert!(
        context
            .events
            .events()
            .iter()
            .any(|event| event.kind == "compact_start")
    );
    assert!(
        context
            .events
            .events()
            .iter()
            .any(|event| event.kind == "compact_complete")
    );
    assert_eq!(context.usage.total_tokens, 15);
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn cache_friendly_compactor_inherits_tools_params_and_settings_for_cache_shape()
-> starweaver_agent::CapabilityResult<()> {
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
            assert!(
                !settings
                    .extra_headers
                    .get("anthropic-beta")
                    .is_some_and(|value| value.contains("interleaved-thinking"))
            );

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
        return_schema: None,
        strict: None,
        sequential: None,
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
            compact_threshold: PerThousandRatio::from_per_thousand(900),
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
    assert!(
        context
            .events
            .events()
            .iter()
            .any(|event| event.kind == "compact_complete")
    );
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn compact_model_heals_openai_reasoning_references_before_summary_request()
-> starweaver_agent::CapabilityResult<()> {
    let captured_messages = Arc::new(Mutex::new(None::<Vec<ModelMessage>>));
    let captured_messages_model = Arc::clone(&captured_messages);
    let compact_model = FunctionModel::streaming(
        move |messages: Vec<ModelMessage>,
              settings: Option<ModelSettings>,
              _info: FunctionModelInfo| {
            let settings = settings.expect("compact settings");
            let replay = settings
                .provider_replay
                .as_ref()
                .expect("compact replay settings");
            assert!(replay.previous_response_id.is_none());
            assert!(replay.conversation_id.is_none());
            assert_eq!(replay.send_item_ids, Some(false));
            assert_eq!(replay.include_encrypted_reasoning, Some(false));
            *captured_messages_model
                .lock()
                .expect("captured messages lock should not be poisoned") = Some(messages);
            Ok(vec![ModelResponseStreamEvent::FinalResult(Box::new(
                ModelResponse::text("## Condensed conversation summary\n\n### Analysis\n\nHealed."),
            ))])
        },
    );
    let compact_model = Arc::new(compact_model) as Arc<dyn starweaver_model::ModelAdapter>;
    let mut reasoning_details = Metadata::default();
    reasoning_details.insert(
        "encrypted_content".to_string(),
        serde_json::json!("encrypted-reasoning"),
    );
    reasoning_details.insert(
        "raw_content".to_string(),
        serde_json::json!(["raw-reasoning"]),
    );
    let mut response_provider_details = Metadata::default();
    response_provider_details.insert(
        "conversation_id".to_string(),
        serde_json::json!("conv_stale"),
    );
    let response = ModelMessage::Response(ModelResponse {
        parts: vec![
            ModelResponsePart::ProviderText {
                text: "visible output".to_string(),
                provider: ProviderPartInfo::new("openai").with_id("msg_1"),
            },
            ModelResponsePart::ProviderThinking {
                text: "stale reasoning".to_string(),
                signature: Some("encrypted-reasoning".to_string()),
                provider: ProviderPartInfo::new("openai")
                    .with_id("rs_1")
                    .with_details(reasoning_details),
            },
            ModelResponsePart::ProviderToolCall {
                call: ToolCallPart {
                    id: "call_1|fc_1".to_string(),
                    name: "lookup".to_string(),
                    arguments: serde_json::Value::Null.into(),
                },
                provider: ProviderPartInfo::new("openai").with_id("fc_1"),
            },
        ],
        usage: Usage {
            requests: 1,
            input_tokens: 90,
            output_tokens: 5,
            total_tokens: 95,
            ..Usage::default()
        },
        model_name: None,
        provider: Some(ProviderInfo {
            name: "openai".to_string(),
            response_id: Some("resp_1".to_string()),
            details: response_provider_details,
        }),
        finish_reason: None,
        timestamp: None,
        run_id: None,
        conversation_id: None,
        metadata: Metadata::default(),
    });
    let tool_return = ModelMessage::Request(ModelRequest {
        parts: vec![ModelRequestPart::ToolReturn(ToolReturnPart::new(
            "call_1|fc_1",
            "lookup",
            serde_json::json!({"ok": true}),
        ))],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Metadata::default(),
    });
    let mut settings = ModelSettings {
        provider_replay: Some(ProviderReplaySettings {
            previous_response_id: Some("auto".to_string()),
            send_item_ids: Some(true),
            include_encrypted_reasoning: Some(true),
            ..ProviderReplaySettings::default()
        }),
        ..ModelSettings::default()
    };
    settings
        .extra_body
        .insert("route".to_string(), serde_json::json!("main"));
    let compact_capability =
        CacheFriendlyCompactCapability::new(Some(compact_model)).with_model_settings(settings);
    let mut state = AgentRunState::new(
        RunId::from_string("run_auto_compact_openai_heal"),
        ConversationId::new(),
    );
    state.message_history = vec![
        ModelMessage::Request(user_request(vec![ContentPart::Text {
            text: "first".to_string(),
        }])),
        response,
        tool_return,
    ];
    let mut context = AgentContext {
        model_config: ModelConfig {
            context_window: Some(100),
            compact_threshold: PerThousandRatio::from_per_thousand(900),
            ..ModelConfig::default()
        },
        message_history: state.message_history.clone(),
        ..AgentContext::default()
    };

    let input_messages = context.message_history.clone();
    let output = compact_capability
        .prepare_model_messages_with_context(&mut state, &mut context, input_messages)
        .await?;

    let captured = captured_messages
        .lock()
        .expect("captured messages lock should not be poisoned")
        .clone()
        .expect("compact model should receive messages");
    let serialized = serde_json::to_string(&captured).expect("messages should serialize");
    assert!(serialized.contains("visible output"));
    assert!(serialized.contains("stale reasoning"));
    assert!(!serialized.contains("rs_1"));
    assert!(!serialized.contains("encrypted-reasoning"));
    assert!(!serialized.contains("raw-reasoning"));
    assert!(!serialized.contains("resp_1"));
    assert!(!serialized.contains("conv_stale"));
    assert!(!serialized.contains("fc_1"));
    assert!(captured.iter().any(|message| match message {
        ModelMessage::Response(response) => response.parts.iter().any(|part| matches!(
            part,
            ModelResponsePart::ProviderThinking { text, signature, provider }
                if text == "stale reasoning"
                    && signature.is_none()
                    && provider.id.is_none()
                    && provider.details.is_empty()
        )),
        ModelMessage::Request(_) => false,
    }));
    let tool_return_id = captured.iter().find_map(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().find_map(|part| match part {
            ModelRequestPart::ToolReturn(tool_return) => Some(tool_return.tool_call_id.as_str()),
            _ => None,
        }),
        ModelMessage::Response(_) => None,
    });
    assert_eq!(tool_return_id, Some("call_1"));
    let output_serialized = serde_json::to_string(&output).expect("output should serialize");
    assert!(!output_serialized.contains("rs_1"));
    assert!(!output_serialized.contains("encrypted-reasoning"));
    Ok(())
}

#[tokio::test]
async fn manual_compact_keep_heals_openai_references_even_without_trimming()
-> starweaver_agent::CapabilityResult<()> {
    let mut reasoning_details = Metadata::default();
    reasoning_details.insert(
        "encrypted_content".to_string(),
        serde_json::json!("manual-encrypted"),
    );
    reasoning_details.insert("raw_content".to_string(), serde_json::json!(["manual-raw"]));
    let response = ModelMessage::Response(ModelResponse {
        parts: vec![
            ModelResponsePart::ProviderThinking {
                text: "manual reasoning".to_string(),
                signature: Some("manual-encrypted".to_string()),
                provider: ProviderPartInfo::new("openai")
                    .with_id("rs_manual")
                    .with_details(reasoning_details),
            },
            ModelResponsePart::ProviderToolCall {
                call: ToolCallPart {
                    id: "call_manual|fc_manual".to_string(),
                    name: "lookup".to_string(),
                    arguments: serde_json::Value::Null.into(),
                },
                provider: ProviderPartInfo::new("openai").with_id("fc_manual"),
            },
        ],
        usage: Usage::default(),
        model_name: None,
        provider: Some(ProviderInfo {
            name: "openai".to_string(),
            response_id: Some("resp_manual".to_string()),
            details: Metadata::from_iter([(
                "conversation_id".to_string(),
                serde_json::json!("conv_manual"),
            )]),
        }),
        finish_reason: None,
        timestamp: None,
        run_id: None,
        conversation_id: None,
        metadata: Metadata::default(),
    });
    let tool_return = ModelMessage::Request(ModelRequest {
        parts: vec![ModelRequestPart::ToolReturn(ToolReturnPart::new(
            "call_manual|fc_manual",
            "lookup",
            serde_json::json!({"ok": true}),
        ))],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Metadata::default(),
    });
    let mut state = AgentRunState::new(
        RunId::from_string("run_manual_compact_no_trim_heal"),
        ConversationId::new(),
    );
    state.metadata.insert(
        "starweaver_compact_keep_messages".to_string(),
        serde_json::json!(99),
    );
    state.message_history = vec![response, tool_return];
    let mut context = AgentContext {
        message_history: state.message_history.clone(),
        ..AgentContext::default()
    };

    let input_messages = context.message_history.clone();
    let output = CacheFriendlyCompactCapability::new(None)
        .prepare_model_messages_with_context(&mut state, &mut context, input_messages)
        .await?;

    let serialized = serde_json::to_string(&output).expect("messages should serialize");
    assert!(serialized.contains("manual reasoning"));
    assert!(!serialized.contains("rs_manual"));
    assert!(!serialized.contains("manual-encrypted"));
    assert!(!serialized.contains("manual-raw"));
    assert!(!serialized.contains("resp_manual"));
    assert!(!serialized.contains("conv_manual"));
    assert!(!serialized.contains("fc_manual"));
    let tool_return_id = output.iter().find_map(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().find_map(|part| match part {
            ModelRequestPart::ToolReturn(tool_return) => Some(tool_return.tool_call_id.as_str()),
            _ => None,
        }),
        ModelMessage::Response(_) => None,
    });
    assert_eq!(tool_return_id, Some("call_manual"));
    Ok(())
}

#[tokio::test]
async fn compact_trim_preserves_response_thinking_parts_like_summary_trim()
-> starweaver_agent::CapabilityResult<()> {
    let mut state = AgentRunState::new(
        RunId::from_string("run_compact_preserve_thinking"),
        ConversationId::new(),
    );
    state.metadata.insert(
        "starweaver_compact_keep_messages".to_string(),
        serde_json::json!(2),
    );
    let response = ModelMessage::Response(ModelResponse {
        parts: vec![
            ModelResponsePart::Thinking {
                text: "keep reasoning".to_string(),
                signature: Some("thinking-signature".to_string()),
            },
            ModelResponsePart::Text {
                text: "visible answer".to_string(),
            },
        ],
        usage: Usage::default(),
        model_name: None,
        provider: None,
        finish_reason: None,
        timestamp: None,
        run_id: None,
        conversation_id: None,
        metadata: Metadata::default(),
    });
    state.message_history = vec![
        ModelMessage::Request(user_request(vec![ContentPart::Text {
            text: "older request".to_string(),
        }])),
        response,
        ModelMessage::Request(user_request(vec![ContentPart::Text {
            text: "latest request".to_string(),
        }])),
    ];
    let mut context = AgentContext {
        message_history: state.message_history.clone(),
        ..AgentContext::default()
    };

    let input_messages = context.message_history.clone();
    let output = CacheFriendlyCompactCapability::new(None)
        .prepare_model_messages_with_context(&mut state, &mut context, input_messages)
        .await?;

    assert!(output.iter().any(|message| match message {
        ModelMessage::Response(response) => response.parts.iter().any(|part| matches!(
            part,
            ModelResponsePart::Thinking { text, signature }
                if text == "keep reasoning"
                    && signature.as_deref() == Some("thinking-signature")
        )),
        ModelMessage::Request(_) => false,
    }));
    assert!(output.iter().any(|message| match message {
        ModelMessage::Response(response) => response.text_output().contains("visible answer"),
        ModelMessage::Request(_) => false,
    }));
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn capability_filter_uses_model_capabilities_for_user_and_tool_media()
-> starweaver_agent::CapabilityResult<()> {
    let request = ModelRequest {
        parts: vec![
            ModelRequestPart::UserPrompt {
                content: vec![
                    ContentPart::Text {
                        text: "inspect".to_string(),
                    },
                    ContentPart::ImageUrl {
                        url: "https://example.test/image.png".to_string(),
                    },
                    ContentPart::Binary {
                        data: vec![0, 1, 2, 3],
                        media_type: "video/mp4".to_string(),
                    },
                    ContentPart::FileUrl {
                        url: "https://example.test/file.pdf".to_string(),
                        media_type: "application/pdf".to_string(),
                    },
                    ContentPart::Binary {
                        data: vec![4, 5, 6],
                        media_type: "audio/mpeg".to_string(),
                    },
                ],
                name: None,
                metadata: serde_json::Map::new(),
            },
            ModelRequestPart::ToolReturn(ToolReturnPart::new(
                "call_media",
                "media_tool",
                serde_json::json!([
                    ContentPart::ImageUrl {
                        url: "https://example.test/tool-image.png".to_string(),
                    },
                    ContentPart::DataUrl {
                        data_url: "data:video/mp4;base64,AAAA".to_string(),
                        media_type: "video/mp4".to_string(),
                    },
                    ContentPart::FileUrl {
                        url: "https://example.test/tool-file.pdf".to_string(),
                        media_type: "application/pdf".to_string(),
                    },
                    ContentPart::DataUrl {
                        data_url: "data:audio/mpeg;base64,AAAA".to_string(),
                        media_type: "audio/mpeg".to_string(),
                    },
                ]),
            )),
        ],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    };
    let mut context = AgentContext::default();
    context
        .model_config
        .capabilities
        .insert(ModelCapability::Vision);

    let messages = NamedFilterCapability::new("capability")
        .prepare_model_messages_with_context(
            &mut AgentRunState::new(
                RunId::from_string("run_capability_filter"),
                ConversationId::new(),
            ),
            &mut context,
            vec![ModelMessage::Request(request)],
        )
        .await?;

    let filtered_content = latest_user_content(&messages);
    assert_eq!(filtered_content.len(), 5);
    assert!(matches!(&filtered_content[0], ContentPart::Text { text } if text == "inspect"));
    assert!(
        matches!(&filtered_content[1], ContentPart::ImageUrl { url } if url.ends_with("image.png"))
    );
    assert!(
        matches!(&filtered_content[2], ContentPart::Text { text } if text.contains("type='video'"))
    );
    assert!(
        matches!(&filtered_content[3], ContentPart::Text { text } if text.contains("type='audio'"))
    );
    assert!(
        matches!(&filtered_content[4], ContentPart::Text { text } if text.contains("type='document'"))
    );

    let tool_content = messages
        .iter()
        .find_map(|message| match message {
            ModelMessage::Request(request) => request.parts.iter().find_map(|part| match part {
                ModelRequestPart::ToolReturn(tool_return) => tool_return.content.as_array(),
                _ => None,
            }),
            ModelMessage::Response(_) => None,
        })
        .expect("tool return array");
    assert_eq!(tool_content.len(), 4);
    assert_eq!(tool_content[0]["kind"], "image_url");
    assert!(
        tool_content[1]
            .as_str()
            .is_some_and(|text| text.contains("type='video'"))
    );
    assert!(
        tool_content[2]
            .as_str()
            .is_some_and(|text| text.contains("type='audio'"))
    );
    assert!(
        tool_content[3]
            .as_str()
            .is_some_and(|text| text.contains("type='document'"))
    );
    assert_eq!(
        latest_request_metadata(&messages)["starweaver_capability_replacements"],
        serde_json::json!(6)
    );
    Ok(())
}

#[tokio::test]
async fn media_upload_respects_url_capabilities_and_uploads_inline_media()
-> starweaver_agent::CapabilityResult<()> {
    let request = user_request(vec![ContentPart::Binary {
        data: png_bytes(1, 1),
        media_type: "image/png".to_string(),
    }]);
    let processor = NamedFilterCapability::media_upload(Arc::new(FakeUploader));
    let mut state =
        AgentRunState::new(RunId::from_string("run_upload_skip"), ConversationId::new());
    let messages = processor
        .prepare_model_messages_with_context(
            &mut state,
            &mut AgentContext::default(),
            vec![ModelMessage::Request(request.clone())],
        )
        .await?;
    assert!(matches!(
        &latest_user_content(&messages)[0],
        ContentPart::Binary { media_type, .. } if media_type == "image/png"
    ));
    assert!(!latest_request_metadata(&messages).contains_key("starweaver_media_uploaded"));

    let mut context = AgentContext::default();
    context
        .model_config
        .capabilities
        .insert(ModelCapability::ImageUrl);
    let messages = processor
        .prepare_model_messages_with_context(
            &mut AgentRunState::new(RunId::from_string("run_upload"), ConversationId::new()),
            &mut context,
            vec![ModelMessage::Request(request)],
        )
        .await?;
    let uploaded_content = latest_user_content(&messages);
    assert!(matches!(
        &uploaded_content[0],
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
async fn concrete_filters_inject_runtime_context_and_repair_tool_args()
-> starweaver_agent::CapabilityResult<()> {
    let mut state = AgentRunState::new(RunId::from_string("run_context"), ConversationId::new());
    state.metadata.insert(
        "starweaver_runtime_context".to_string(),
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
            timestamp: Some(Utc::now() - Duration::seconds(7200)),
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
    output = NamedFilterCapability::new("runtime_context")
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
            ModelRequestPart::UserPrompt {
                content, metadata, ..
            } =>
                metadata.get(CONTEXT_ORIGIN_METADATA)
                    == Some(&serde_json::json!(CONTEXT_ORIGIN_RUNTIME_CONTEXT))
                    && content.iter().any(|part| matches!(
                        part,
                        ContentPart::Text { text } if text == "Prefer concise answers."
                    )),
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
        ModelResponsePart::ToolCall(call)
            if matches!(
                &call.arguments,
                ToolArguments::Parsed(serde_json::Value::String(text)) if text == "{\"ok\":true}"
            )
    ));
    assert!(matches!(
        &response.parts[1],
        ModelResponsePart::Thinking { text, signature } if text == "keep reasoning" && signature.is_none()
    ));
    Ok(())
}

#[tokio::test]
async fn cold_start_filter_trims_only_after_idle_threshold_and_preserves_pending_tool_results()
-> starweaver_agent::CapabilityResult<()> {
    let old_tool_content = "abcdefghijklmnopqrstuvwxyz";
    let current_tool_content = "zyxwvutsrqponmlkjihgfedcba";
    let cold_messages = vec![
        ModelMessage::Request(ModelRequest {
            parts: vec![ModelRequestPart::ToolReturn(ToolReturnPart::new(
                "call_old",
                "old_tool",
                serde_json::json!(old_tool_content),
            ))],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        }),
        ModelMessage::Response(ModelResponse {
            timestamp: Some(Utc::now() - Duration::seconds(7200)),
            ..ModelResponse::text("previous response")
        }),
        ModelMessage::Request(ModelRequest {
            parts: vec![ModelRequestPart::ToolReturn(ToolReturnPart::new(
                "call_current",
                "current_tool",
                serde_json::json!(current_tool_content),
            ))],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        }),
    ];
    let mut state = AgentRunState::new(
        RunId::from_string("run_active_cold_start"),
        ConversationId::new(),
    );
    state.metadata.insert(
        "starweaver_cold_start_tool_return_limit".to_string(),
        serde_json::json!(16),
    );

    let cold_output = NamedFilterCapability::new("cold_start")
        .prepare_model_messages_with_context(
            &mut state,
            &mut AgentContext::default(),
            cold_messages.clone(),
        )
        .await?;
    let cold_tool_returns = tool_return_texts(&cold_output);
    assert!(cold_tool_returns[0].contains("chars truncated"));
    assert_eq!(cold_tool_returns[1], current_tool_content);
    assert_eq!(
        latest_request_metadata(&cold_output)["starweaver_cold_start_truncated_tool_returns"],
        serde_json::json!(1)
    );

    let mut warm_messages = cold_messages;
    if let ModelMessage::Response(response) = &mut warm_messages[1] {
        response.timestamp = Some(Utc::now() - Duration::seconds(30));
    }
    let warm_output = NamedFilterCapability::new("cold_start")
        .prepare_model_messages_with_context(
            &mut state,
            &mut AgentContext::default(),
            warm_messages,
        )
        .await?;
    assert_eq!(
        tool_return_texts(&warm_output),
        [
            old_tool_content.to_string(),
            current_tool_content.to_string()
        ]
    );
    assert!(
        latest_request_metadata(&warm_output)
            .get("starweaver_cold_start_truncated_tool_returns")
            .is_none()
    );
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn tool_args_filter_uses_retry_placeholder_for_invalid_non_empty_json()
-> starweaver_agent::CapabilityResult<()> {
    let messages = vec![ModelMessage::Response(ModelResponse {
        parts: vec![
            ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_invalid_parsed".to_string(),
                name: "json_tool".to_string(),
                arguments: serde_json::json!("not-json").into(),
            }),
            ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_invalid_raw".to_string(),
                name: "json_tool".to_string(),
                arguments: ToolArguments::raw_json_string("{bad"),
            }),
            ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_valid_raw".to_string(),
                name: "json_tool".to_string(),
                arguments: ToolArguments::raw_json_string("{\"ok\":true}"),
            }),
            ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_empty_invalid".to_string(),
                name: "json_tool".to_string(),
                arguments: ToolArguments::invalid("", "EOF while parsing"),
            }),
            ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_parsed_null".to_string(),
                name: "json_tool".to_string(),
                arguments: ToolArguments::parsed(serde_json::Value::Null),
            }),
            ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_parsed_array".to_string(),
                name: "json_tool".to_string(),
                arguments: ToolArguments::parsed(serde_json::json!(["keep"])),
            }),
            ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_parsed_number".to_string(),
                name: "json_tool".to_string(),
                arguments: ToolArguments::parsed(serde_json::json!(7)),
            }),
            ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_parsed_bool".to_string(),
                name: "json_tool".to_string(),
                arguments: ToolArguments::parsed(serde_json::json!(true)),
            }),
            ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_parsed_object".to_string(),
                name: "json_tool".to_string(),
                arguments: ToolArguments::parsed(serde_json::json!({"keep": true})),
            }),
        ],
        usage: starweaver_agent::Usage::default(),
        model_name: None,
        provider: None,
        finish_reason: None,
        timestamp: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    })];

    let mut state = AgentRunState::new(
        RunId::from_string("run_invalid_tool_args"),
        ConversationId::new(),
    );
    let output = NamedFilterCapability::new("tool_args")
        .prepare_model_messages_with_context(&mut state, &mut AgentContext::default(), messages)
        .await?;
    let response = output
        .iter()
        .find_map(|message| match message {
            ModelMessage::Response(response) => Some(response),
            ModelMessage::Request(_) => None,
        })
        .expect("response");
    let retry_placeholder = serde_json::json!({
        "system": "This tool's args is not a valid JSON. Please refer the return value of the tool to try again."
    });
    assert!(matches!(
        &response.parts[0],
        ModelResponsePart::ToolCall(call) if call.arguments == retry_placeholder
    ));
    assert!(matches!(
        &response.parts[1],
        ModelResponsePart::ToolCall(call) if call.arguments == retry_placeholder
    ));
    assert!(matches!(
        &response.parts[2],
        ModelResponsePart::ToolCall(call)
            if matches!(&call.arguments, ToolArguments::RawJsonString(raw) if raw == "{\"ok\":true}")
    ));
    assert!(matches!(
        &response.parts[3],
        ModelResponsePart::ToolCall(call) if matches!(&call.arguments, ToolArguments::Invalid { raw, .. } if raw.is_empty())
    ));
    assert!(matches!(
        &response.parts[4],
        ModelResponsePart::ToolCall(call) if matches!(&call.arguments, ToolArguments::Parsed(serde_json::Value::Null))
    ));
    assert!(matches!(
        &response.parts[5],
        ModelResponsePart::ToolCall(call)
            if matches!(&call.arguments, ToolArguments::Parsed(value) if value == &serde_json::json!(["keep"]))
    ));
    assert!(matches!(
        &response.parts[6],
        ModelResponsePart::ToolCall(call)
            if matches!(&call.arguments, ToolArguments::Parsed(value) if value == &serde_json::json!(7))
    ));
    assert!(matches!(
        &response.parts[7],
        ModelResponsePart::ToolCall(call)
            if matches!(&call.arguments, ToolArguments::Parsed(value) if value == &serde_json::json!(true))
    ));
    assert!(matches!(
        &response.parts[8],
        ModelResponsePart::ToolCall(call)
            if matches!(&call.arguments, ToolArguments::Parsed(value) if value == &serde_json::json!({"keep": true}))
    ));
    assert_eq!(
        latest_request_metadata(&output)["starweaver_tool_args_repaired"],
        serde_json::json!(2)
    );
    Ok(())
}

#[tokio::test]
async fn filters_repair_and_normalize_provider_aware_response_parts()
-> starweaver_agent::CapabilityResult<()> {
    let mut provider_details = serde_json::Map::new();
    provider_details.insert(
        "encrypted_content".to_string(),
        serde_json::json!("encrypted-only"),
    );
    let messages = vec![ModelMessage::Response(ModelResponse {
        parts: vec![
            ModelResponsePart::ProviderToolCall {
                call: ToolCallPart {
                    id: "call_provider".to_string(),
                    name: "json_tool".to_string(),
                    arguments: serde_json::json!("{\"ok\":true}").into(),
                },
                provider: ProviderPartInfo::new("openai").with_id("fc_provider"),
            },
            ModelResponsePart::ProviderThinking {
                text: "  keep provider reasoning   \n".to_string(),
                signature: Some(String::new()),
                provider: ProviderPartInfo::new("openai").with_id("rs_keep"),
            },
            ModelResponsePart::ProviderThinking {
                text: String::new(),
                signature: Some("encrypted-only".to_string()),
                provider: ProviderPartInfo::new("openai")
                    .with_id("rs_encrypted")
                    .with_details(provider_details),
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
    })];

    let mut state = AgentRunState::new(
        RunId::from_string("run_provider_filter"),
        ConversationId::new(),
    );
    let mut output = NamedFilterCapability::new("tool_args")
        .prepare_model_messages_with_context(&mut state, &mut AgentContext::default(), messages)
        .await?;
    output = NamedFilterCapability::new("reasoning_normalize")
        .prepare_model_messages_with_context(&mut state, &mut AgentContext::default(), output)
        .await?;

    let response = output
        .iter()
        .find_map(|message| match message {
            ModelMessage::Response(response) => Some(response),
            ModelMessage::Request(_) => None,
        })
        .expect("response");
    assert!(matches!(
        &response.parts[0],
        ModelResponsePart::ProviderToolCall { call, provider }
            if matches!(
                &call.arguments,
                ToolArguments::Parsed(serde_json::Value::String(text)) if text == "{\"ok\":true}"
            ) && provider.id.as_deref() == Some("fc_provider")
    ));
    assert!(matches!(
        &response.parts[1],
        ModelResponsePart::ProviderThinking { text, signature, provider }
            if text == "keep provider reasoning"
                && signature.is_none()
                && provider.id.as_deref() == Some("rs_keep")
    ));
    assert!(matches!(
        &response.parts[2],
        ModelResponsePart::ProviderThinking { text, signature, provider }
            if text.is_empty()
                && signature.as_deref() == Some("encrypted-only")
                && provider.id.as_deref() == Some("rs_encrypted")
    ));
    Ok(())
}

#[tokio::test]
async fn background_shell_filter_injects_completed_results_once_without_repeated_status()
-> starweaver_agent::CapabilityResult<()> {
    let request = user_request(vec![ContentPart::Text {
        text: "hello".to_string(),
    }]);
    let provider = Arc::new(VirtualEnvironmentProvider::new("process").with_process(
        ShellProcessSnapshot {
            process_id: "process_1".to_string(),
            command: "echo ready".to_string(),
            status: ShellProcessStatus::Completed,
            stdout: "ready & done".to_string(),
            stderr: String::new(),
            return_code: Some(0),
            metadata: Metadata::default(),
        },
    ));
    let mut context = AgentContext::default();
    attach_environment(&mut context, provider);

    let mut state = AgentRunState::new(
        RunId::from_string("run_background_shell"),
        ConversationId::new(),
    );
    let messages = NamedFilterCapability::new("background_shell")
        .prepare_model_messages_with_context(
            &mut state,
            &mut context,
            vec![ModelMessage::Request(request.clone())],
        )
        .await?;
    let text = request_text_parts(messages.last().expect("request")).join("\n");
    assert!(text.contains("<background-result process-id=\"process_1\""));
    assert!(text.contains("<stdout>ready &amp; done</stdout>"));
    assert!(!text.contains("<background-status>"));
    assert!(
        context
            .events
            .events()
            .iter()
            .any(|event| event.kind == "background_shell_complete")
    );

    let messages = NamedFilterCapability::new("background_shell")
        .prepare_model_messages_with_context(
            &mut state,
            &mut context,
            vec![ModelMessage::Request(request)],
        )
        .await?;
    let text = request_text_parts(messages.last().expect("request")).join("\n");
    assert!(!text.contains("<background-result process-id=\"process_1\""));
    assert!(!text.contains("<background-status>"));
    Ok(())
}

#[tokio::test]
async fn bus_message_filter_consumes_explicit_bus_messages_without_source_based_steering()
-> starweaver_agent::CapabilityResult<()> {
    let request = user_request(vec![ContentPart::Text {
        text: "hello".to_string(),
    }]);
    let mut state = AgentRunState::new(RunId::from_string("run_bus"), ConversationId::new());
    let mut context = AgentContext::default();
    context.send_message(
        starweaver_context::BusMessage::text("please continue", "user")
            .with_id("user-msg")
            .with_template("[urgent] {{ content }}"),
    );
    context.send_message(
        starweaver_context::BusMessage::text("worker update", "subagent").with_id("worker-msg"),
    );

    let messages = NamedFilterCapability::new("bus_message")
        .prepare_model_messages_with_context(
            &mut state,
            &mut context,
            vec![ModelMessage::Request(request)],
        )
        .await?;

    let text_parts = request_text_parts(messages.last().expect("request"));
    let text = text_parts.join("\n");
    assert_eq!(
        text_parts
            .iter()
            .filter(|text| text.contains("<bus-message"))
            .count(),
        2
    );
    assert!(text.contains("<bus-message source=\"user\">"));
    assert!(text.contains("[urgent] please continue"));
    assert!(text.contains("<bus-message source=\"subagent\">"));
    assert!(text.contains("worker update"));
    assert!(context.steering_messages.is_empty());
    assert!(!context.messages.has_pending(context.agent_id.as_str()));
    let message_received = context
        .events
        .events()
        .iter()
        .find(|event| event.kind == "message_received")
        .expect("message received event");
    assert_eq!(
        message_received.payload["messages"]
            .as_array()
            .expect("message_received payload should contain messages array")
            .len(),
        2
    );
    assert!(
        !context
            .events
            .events()
            .iter()
            .any(|event| event.kind == "steering_received")
    );
    Ok(())
}

#[tokio::test]
async fn handoff_filter_consumes_prebuilt_context_restored_message()
-> starweaver_agent::CapabilityResult<()> {
    let request = user_request(vec![ContentPart::Text {
        text: "hello".to_string(),
    }]);
    let mut state = AgentRunState::new(
        RunId::from_string("run_handoff_prebuilt"),
        ConversationId::new(),
    );
    let mut context = AgentContext {
        handoff_message: Some("<context-restored>already restored</context-restored>".to_string()),
        ..AgentContext::default()
    };

    let messages = NamedFilterCapability::new("handoff")
        .prepare_model_messages_with_context(
            &mut state,
            &mut context,
            vec![ModelMessage::Request(request)],
        )
        .await?;

    let text = request_text_parts(messages.last().expect("request")).join("\n");
    assert!(text.contains("already restored"));
    assert!(context.handoff_message.is_none());
    assert!(context.force_inject_context);
    Ok(())
}

#[tokio::test]
async fn compact_filter_strips_context_configured_injected_tags()
-> starweaver_agent::CapabilityResult<()> {
    let messages = vec![
        ModelMessage::Response(ModelResponse::text("old")),
        ModelMessage::Request(ModelRequest {
            parts: vec![ModelRequestPart::UserPrompt {
                content: vec![ContentPart::Text {
                    text: "keep\n<custom-context>stale</custom-context>".to_string(),
                }],
                name: None,
                metadata: serde_json::Map::new(),
            }],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        }),
        ModelMessage::Response(ModelResponse::text("ok")),
    ];
    let mut state = AgentRunState::new(
        RunId::from_string("run_compact_tags"),
        ConversationId::new(),
    );
    state.metadata.insert(
        "starweaver_compact_keep_messages".to_string(),
        serde_json::json!(2),
    );
    state.message_history = messages.clone();
    let mut context = AgentContext::default();
    context
        .injected_context_tags
        .push("custom-context".to_string());
    context.message_history = messages.clone();

    let compacted = CacheFriendlyCompactCapability::new(None)
        .prepare_model_messages_with_context(&mut state, &mut context, messages)
        .await?;

    let text = compacted
        .iter()
        .flat_map(request_text_parts)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("keep"));
    assert!(!text.contains("custom-context"));
    assert!(!text.contains("stale"));
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

fn assert_restore_block_order(text: &str) {
    let context_restored = text.find("<context-restored>").expect("context restored");
    let previous = text
        .find("<previous-assistant-reference>")
        .expect("previous assistant reference");
    let original = text.find("<original-request>").expect("original request");
    let steering = text.find("<user-steering>").expect("user steering");
    assert!(context_restored < previous);
    assert!(previous < original);
    assert!(original < steering);
}

fn contains_image_url(messages: &[ModelMessage], expected: &str) -> bool {
    messages.iter().any(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().any(|part| match part {
            ModelRequestPart::UserPrompt { content, .. } => content
                .iter()
                .any(|content| matches!(content, ContentPart::ImageUrl { url } if url == expected)),
            ModelRequestPart::SystemPrompt { .. }
            | ModelRequestPart::Instruction { .. }
            | ModelRequestPart::ToolReturn(_)
            | ModelRequestPart::RetryPrompt { .. } => false,
        }),
        ModelMessage::Response(_) => false,
    })
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

fn tool_return_texts(messages: &[ModelMessage]) -> Vec<String> {
    messages
        .iter()
        .flat_map(|message| match message {
            ModelMessage::Request(request) => request
                .parts
                .iter()
                .filter_map(|part| match part {
                    ModelRequestPart::ToolReturn(tool_return) => {
                        tool_return.content.as_str().map(ToString::to_string)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>(),
            ModelMessage::Response(_) => Vec::new(),
        })
        .collect()
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
