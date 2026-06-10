//! SDK filter parity order and concrete behavior tests.

#![allow(clippy::expect_used)]

use std::sync::Arc;

use async_trait::async_trait;
use starweaver_agent::{
    default_filter_processors, AgentRunState, ConversationId, HistoryProcessor, MediaUploadRequest,
    MediaUploader, NamedFilterProcessor, RunId, DEFAULT_FILTER_ORDER,
};
use starweaver_model::{
    ContentPart, MediaPolicy, ModelMessage, ModelRequest, ModelRequestPart, ModelResponse,
    ModelResponsePart, ToolCallPart, ToolReturnPart,
};

#[tokio::test]
async fn default_filter_processors_record_order() -> starweaver_agent::HistoryProcessorResult<()> {
    let request = user_request(vec![ContentPart::Text {
        text: "hello".to_string(),
    }]);
    let mut messages = vec![ModelMessage::Request(request)];
    let state = AgentRunState::new(RunId::from_string("run_filter"), ConversationId::new());
    for processor in default_filter_processors() {
        messages = processor.process(&state, messages).await?;
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
) -> starweaver_agent::HistoryProcessorResult<()> {
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

    let messages = NamedFilterProcessor::new("media_preflight")
        .process(&state, vec![ModelMessage::Request(request)])
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
async fn media_preflight_limits_newest_images() -> starweaver_agent::HistoryProcessorResult<()> {
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

    let messages = NamedFilterProcessor::new("media_preflight")
        .process(&state, vec![ModelMessage::Request(request)])
        .await?;
    let content = latest_user_content(&messages);
    assert!(
        matches!(&content[0], ContentPart::Text { text } if text.contains("image count limit"))
    );
    assert!(matches!(&content[1], ContentPart::ImageUrl { url } if url.ends_with("new.png")));
    Ok(())
}

#[tokio::test]
async fn media_preflight_traverses_nested_tool_returns(
) -> starweaver_agent::HistoryProcessorResult<()> {
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

    let messages = NamedFilterProcessor::new("media_preflight")
        .process(&state, vec![ModelMessage::Request(request)])
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
) -> starweaver_agent::HistoryProcessorResult<()> {
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
        text: "before <runtime-context>stale</runtime-context> after".to_string(),
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

    let output = NamedFilterProcessor::new("compact")
        .process(
            &state,
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
    assert!(all_text.contains("[image: https://example.test/image.png]"));
    assert!(all_text.contains("chars truncated"));
    assert!(all_text.contains("<original-request>"));
    assert!(all_text.contains("Original user goal"));
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
async fn media_upload_replaces_oversized_binary_with_resource_ref(
) -> starweaver_agent::HistoryProcessorResult<()> {
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

    let processor = NamedFilterProcessor::media_upload(Arc::new(FakeUploader));
    let messages = processor
        .process(&state, vec![ModelMessage::Request(request)])
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
) -> starweaver_agent::HistoryProcessorResult<()> {
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

    let mut output = NamedFilterProcessor::new("cold_start")
        .process(&state, messages)
        .await?;
    output = NamedFilterProcessor::new("auto_load_files")
        .process(&state, output)
        .await?;
    output = NamedFilterProcessor::new("runtime_instructions")
        .process(&state, output)
        .await?;
    output = NamedFilterProcessor::new("tool_args")
        .process(&state, output)
        .await?;
    output = NamedFilterProcessor::new("reasoning_normalize")
        .process(&state, output)
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
