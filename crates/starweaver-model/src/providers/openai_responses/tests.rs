#![allow(clippy::unwrap_used)]

use super::*;
use serde_json::{json, Value};
use starweaver_usage::Usage;

use crate::{
    message::Metadata, ModelError, ModelMessage, ModelRequest, ModelRequestPart, ModelResponse,
    ModelResponsePart, ModelResponseStreamEvent, ModelSettings, ProviderInfo, ProviderPartInfo,
    ProviderReplaySettings, StreamDelta, ThinkingSettings, CONTEXT_ORIGIN_METADATA,
    CONTEXT_ORIGIN_RUNTIME_CONTEXT,
};

fn final_response(events: &[ModelResponseStreamEvent]) -> &ModelResponse {
    events
        .iter()
        .find_map(|event| match event {
            ModelResponseStreamEvent::FinalResult(response) => Some(response.as_ref()),
            _ => None,
        })
        .unwrap()
}

fn runtime_context_part(text: impl Into<String>) -> ModelRequestPart {
    let mut metadata = Metadata::default();
    metadata.insert(
        CONTEXT_ORIGIN_METADATA.to_string(),
        json!(CONTEXT_ORIGIN_RUNTIME_CONTEXT),
    );
    ModelRequestPart::UserPrompt {
        content: vec![crate::message::ContentPart::Text { text: text.into() }],
        name: None,
        metadata,
    }
}

#[test]
fn responses_stream_function_call_deltas_become_final_tool_call() {
    let events = vec![
        json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "id": "fc_1",
                "type": "function_call",
                "call_id": "call_1",
                "name": "shell_exec",
                "arguments": ""
            }
        }),
        json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "fc_1",
            "delta": "{\"command\":\"ls"
        }),
        json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "fc_1",
            "delta": "\"}"
        }),
        json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {
                "id": "fc_1",
                "type": "function_call",
                "call_id": "call_1",
                "name": "shell_exec",
                "arguments": "{\"command\":\"ls\"}"
            }
        }),
        json!({
            "type": "response.completed",
            "response": {
                "id": "resp_1",
                "status": "completed",
                "output": []
            }
        }),
    ];

    let stream = OpenAiResponsesAdapter::parse_stream_events(&events).unwrap();
    assert!(stream.iter().any(|event| matches!(
        event,
        ModelResponseStreamEvent::PartStart(part)
            if part.part_kind == "tool_call" && part.index == 2
    )));
    assert!(stream.iter().any(|event| matches!(
        event,
        ModelResponseStreamEvent::PartDelta(delta)
            if matches!(&delta.delta, StreamDelta::ToolCallName { name } if name == "shell_exec")
    )));
    assert!(stream.iter().any(|event| matches!(
        event,
        ModelResponseStreamEvent::PartDelta(delta)
            if matches!(&delta.delta, StreamDelta::ToolCallArguments { arguments_delta } if arguments_delta.contains("command"))
    )));

    let response = final_response(&stream);
    let tool_calls = response.tool_calls();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].id, "call_1");
    assert_eq!(tool_calls[0].name, "shell_exec");
    assert_eq!(tool_calls[0].arguments.execution_value()["command"], "ls");
}

#[test]
fn responses_stream_preserves_thinking_and_text_when_completed_output_is_empty() {
    let events = vec![
        json!({
            "type": "response.output_item.added",
            "item": {
                "id": "rs_stream",
                "type": "reasoning",
                "encrypted_content": "encrypted-stream",
                "content": [{"type": "reasoning_text", "text": "raw-stream"}]
            }
        }),
        json!({"type": "response.reasoning_summary_text.delta", "item_id": "rs_stream", "delta": "inspect"}),
        json!({"type": "response.output_text.delta", "delta": "done"}),
        json!({
            "type": "response.completed",
            "response": {
                "id": "resp_text",
                "status": "completed",
                "output": []
            }
        }),
    ];

    let stream = OpenAiResponsesAdapter::parse_stream_events(&events).unwrap();
    assert!(stream.iter().any(|event| matches!(
        event,
        ModelResponseStreamEvent::PartDelta(delta)
            if matches!(&delta.delta, StreamDelta::Thinking { text } if text == "inspect")
    )));
    assert!(stream.iter().any(|event| matches!(
        event,
        ModelResponseStreamEvent::PartDelta(delta)
            if matches!(&delta.delta, StreamDelta::Text { text } if text == "done")
    )));
    let response = final_response(&stream);
    assert_eq!(response.text_output(), "done");
    assert!(response.parts.iter().any(|part| matches!(
        part,
        ModelResponsePart::ProviderThinking { text, signature, provider }
            if text == "inspect"
                && signature.as_deref() == Some("encrypted-stream")
                && provider.id.as_deref() == Some("rs_stream")
                && provider.provider_name.as_deref() == Some("openai")
                && provider.details.get("raw_content").and_then(Value::as_array).is_some_and(|items| items == &vec![json!("raw-stream")])
    )));
}

#[test]
fn responses_parse_preserves_provider_replay_metadata() {
    let response = OpenAiResponsesAdapter::parse_response(&json!({
        "id": "resp_1",
        "model": "gpt-5.5",
        "status": "completed",
        "conversation": {"id": "conv_1"},
        "service_tier": "default",
        "usage": {
            "input_tokens": 10,
            "input_tokens_details": {"cached_tokens": 6},
            "output_tokens": 4,
            "output_tokens_details": {"reasoning_tokens": 2},
            "total_tokens": 14
        },
        "output": [
            {
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "status": "completed",
                "phase": "final_answer",
                "content": [
                    {"type": "output_text", "text": "hello", "annotations": [{"kind": "note"}]}
                ]
            },
            {
                "id": "rs_1",
                "type": "reasoning",
                "encrypted_content": "encrypted",
                "summary": [{"type": "summary_text", "text": "inspect"}],
                "content": [{"type": "reasoning_text", "text": "raw"}]
            },
            {
                "id": "fc_1",
                "type": "function_call",
                "call_id": "call_1",
                "name": "lookup",
                "arguments": "{\"q\":\"x\"}",
                "namespace": "tools",
                "status": "completed"
            },
            {"id": "mcp_1", "type": "mcp_call", "name": "ask", "status": "completed"}
        ]
    }))
    .unwrap();

    assert_eq!(response.usage.cache_read_tokens, 6);
    assert_eq!(
        response
            .provider
            .as_ref()
            .and_then(|provider| provider.details.get("conversation_id"))
            .and_then(Value::as_str),
        Some("conv_1")
    );
    assert!(matches!(
        &response.parts[0],
        ModelResponsePart::ProviderText { text, provider }
            if text == "hello"
                && provider.id.as_deref() == Some("msg_1")
                && provider.details.get("phase").and_then(Value::as_str) == Some("final_answer")
    ));
    assert!(matches!(
        &response.parts[1],
        ModelResponsePart::ProviderThinking { text, signature, provider }
            if text == "inspect"
                && signature.as_deref() == Some("encrypted")
                && provider.id.as_deref() == Some("rs_1")
                && provider.details.get("raw_content").and_then(Value::as_array).is_some_and(|items| items == &vec![json!("raw")])
    ));
    assert!(matches!(
        &response.parts[2],
        ModelResponsePart::ProviderToolCall { call, provider }
            if call.id == "call_1"
                && call.name == "lookup"
                && call.arguments.execution_value() == json!({"q": "x"})
                && provider.id.as_deref() == Some("fc_1")
                && provider.details.get("namespace").and_then(Value::as_str) == Some("tools")
    ));
    assert!(matches!(
        &response.parts[3],
        ModelResponsePart::ProviderOpaque { item_type, provider, .. }
            if item_type == "mcp_call" && provider.id.as_deref() == Some("mcp_1")
    ));
}

#[test]
fn responses_replay_merges_text_and_reasoning_items_by_provider_id() {
    let mut raw_details = Metadata::default();
    raw_details.insert("raw_content".to_string(), json!(["raw-a", "raw-b"]));
    let messages = vec![ModelMessage::Response(ModelResponse {
        parts: vec![
            ModelResponsePart::ProviderText {
                text: "hello ".to_string(),
                provider: ProviderPartInfo::new("openai").with_id("msg_1"),
            },
            ModelResponsePart::ProviderText {
                text: "world".to_string(),
                provider: ProviderPartInfo::new("openai").with_id("msg_1"),
            },
            ModelResponsePart::ProviderThinking {
                text: "inspect".to_string(),
                signature: Some("encrypted".to_string()),
                provider: ProviderPartInfo::new("openai")
                    .with_id("rs_1")
                    .with_details(raw_details),
            },
            ModelResponsePart::ProviderThinking {
                text: "decide".to_string(),
                signature: None,
                provider: ProviderPartInfo::new("openai").with_id("rs_1"),
            },
        ],
        usage: Usage::default(),
        model_name: None,
        provider: Some(ProviderInfo {
            name: "openai".to_string(),
            response_id: Some("resp_1".to_string()),
            details: Metadata::default(),
        }),
        finish_reason: None,
        timestamp: None,
        run_id: None,
        conversation_id: None,
        metadata: Metadata::default(),
    })];
    let settings = ModelSettings {
        provider_replay: Some(ProviderReplaySettings {
            include_encrypted_reasoning: Some(true),
            ..ProviderReplaySettings::default()
        }),
        ..ModelSettings::default()
    };

    let request =
        OpenAiResponsesAdapter::build_request("gpt-5.5", &messages, Some(&settings), &[], &[])
            .unwrap();

    assert_eq!(request["input"].as_array().unwrap().len(), 2);
    assert_eq!(request["input"][0]["id"], "msg_1");
    assert_eq!(request["input"][0]["content"].as_array().unwrap().len(), 2);
    assert_eq!(request["input"][0]["content"][0]["text"], "hello ");
    assert_eq!(request["input"][0]["content"][1]["text"], "world");
    assert_eq!(request["input"][1]["id"], "rs_1");
    assert_eq!(request["input"][1]["encrypted_content"], "encrypted");
    assert_eq!(request["input"][1]["summary"].as_array().unwrap().len(), 2);
    assert_eq!(request["input"][1]["content"].as_array().unwrap().len(), 2);
}

#[test]
#[allow(clippy::too_many_lines)]
fn responses_full_history_keeps_durable_input_prefix_with_runtime_context_blocks() {
    let first = vec![ModelMessage::Request(ModelRequest {
        parts: vec![
            ModelRequestPart::SystemPrompt {
                text: "stable system".to_string(),
                metadata: Metadata::default(),
            },
            runtime_context_part(
                "<runtime-context><current-time>first</current-time></runtime-context>",
            ),
            ModelRequestPart::UserPrompt {
                content: vec![crate::message::ContentPart::Text {
                    text: "first user".to_string(),
                }],
                name: None,
                metadata: Metadata::default(),
            },
        ],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Metadata::default(),
    })];
    let mut second = vec![ModelMessage::Request(ModelRequest {
        parts: vec![
            ModelRequestPart::SystemPrompt {
                text: "stable system".to_string(),
                metadata: Metadata::default(),
            },
            ModelRequestPart::UserPrompt {
                content: vec![crate::message::ContentPart::Text {
                    text: "first user".to_string(),
                }],
                name: None,
                metadata: Metadata::default(),
            },
        ],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Metadata::default(),
    })];
    second.push(ModelMessage::Response(ModelResponse {
        parts: vec![ModelResponsePart::Text {
            text: "first assistant".to_string(),
        }],
        usage: Usage::default(),
        model_name: None,
        provider: None,
        finish_reason: None,
        timestamp: None,
        run_id: None,
        conversation_id: None,
        metadata: Metadata::default(),
    }));
    second.push(ModelMessage::Request(ModelRequest {
        parts: vec![
            runtime_context_part(
                "<runtime-context><current-time>second</current-time></runtime-context>",
            ),
            ModelRequestPart::UserPrompt {
                content: vec![crate::message::ContentPart::Text {
                    text: "second user".to_string(),
                }],
                name: None,
                metadata: Metadata::default(),
            },
        ],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Metadata::default(),
    }));

    let first_request =
        OpenAiResponsesAdapter::build_request("gpt-5.5", &first, None, &[], &[]).unwrap();
    let second_request =
        OpenAiResponsesAdapter::build_request("gpt-5.5", &second, None, &[], &[]).unwrap();

    assert_eq!(first_request["instructions"], "stable system");
    assert_eq!(second_request["instructions"], "stable system");

    let first_input = first_request["input"].as_array().unwrap();
    let second_input = second_request["input"].as_array().unwrap();
    assert_eq!(first_input.len(), 2);
    assert_eq!(second_input.len(), 4);
    assert!(first_input[0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("runtime-context"));
    assert!(first_input[0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("first"));
    assert_eq!(first_input[1]["content"][0]["text"], "first user");
    assert_eq!(first_input[1], second_input[0]);
    assert_eq!(second_input[1]["role"], "assistant");
    assert_eq!(second_input[1]["content"][0]["text"], "first assistant");
    assert!(second_input[2]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("runtime-context"));
    assert!(second_input[2]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("second"));
    assert_eq!(second_input[3]["content"][0]["text"], "second user");
}

#[test]
fn responses_previous_response_auto_keeps_current_runtime_context_input_after_trimming() {
    let messages = vec![
        ModelMessage::Request(ModelRequest {
            parts: vec![
                ModelRequestPart::SystemPrompt {
                    text: "stable system".to_string(),
                    metadata: Metadata::default(),
                },
                ModelRequestPart::UserPrompt {
                    content: vec![crate::message::ContentPart::Text {
                        text: "old".to_string(),
                    }],
                    name: None,
                    metadata: Metadata::default(),
                },
            ],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: Metadata::default(),
        }),
        openai_response_with_id("resp_1"),
        ModelMessage::Request(ModelRequest {
            parts: vec![
                runtime_context_part(
                    "<runtime-context><current-time>now</current-time></runtime-context>",
                ),
                ModelRequestPart::UserPrompt {
                    content: vec![crate::message::ContentPart::Text {
                        text: "new".to_string(),
                    }],
                    name: None,
                    metadata: Metadata::default(),
                },
            ],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: Metadata::default(),
        }),
    ];
    let settings = ModelSettings {
        provider_replay: Some(ProviderReplaySettings {
            previous_response_id: Some("auto".to_string()),
            ..ProviderReplaySettings::default()
        }),
        ..ModelSettings::default()
    };

    let request =
        OpenAiResponsesAdapter::build_request("gpt-5.5", &messages, Some(&settings), &[], &[])
            .unwrap();

    assert_eq!(request["previous_response_id"], "resp_1");
    assert_eq!(request["instructions"], "stable system");
    let input = request["input"].as_array().unwrap();
    assert_eq!(input.len(), 2);
    assert_eq!(input[0]["role"], "user");
    assert!(input[0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("runtime-context"));
    assert_eq!(input[1]["role"], "user");
    assert_eq!(input[1]["content"][0]["text"], "new");
}

#[test]
fn responses_previous_response_auto_trims_after_latest_same_provider_response() {
    let messages = vec![
        ModelMessage::Request(ModelRequest::user_text("old")),
        openai_response_with_id("resp_1"),
        ModelMessage::Request(ModelRequest::user_text("new")),
    ];
    let settings = ModelSettings {
        provider_replay: Some(ProviderReplaySettings {
            previous_response_id: Some("auto".to_string()),
            ..ProviderReplaySettings::default()
        }),
        ..ModelSettings::default()
    };

    let request =
        OpenAiResponsesAdapter::build_request("gpt-5.5", &messages, Some(&settings), &[], &[])
            .unwrap();

    assert_eq!(request["previous_response_id"], "resp_1");
    assert_eq!(request["input"].as_array().unwrap().len(), 1);
    assert_eq!(request["input"][0]["content"][0]["text"], "new");
}

#[test]
fn responses_previous_response_auto_does_not_cross_compaction_boundary() {
    let mut compaction = openai_response_with_id("resp_compact");
    if let ModelMessage::Response(response) = &mut compaction {
        response
            .provider
            .as_mut()
            .unwrap()
            .details
            .insert("compaction".to_string(), json!(true));
    }
    let messages = vec![
        ModelMessage::Request(ModelRequest::user_text("old")),
        compaction,
        ModelMessage::Request(ModelRequest::user_text("new")),
    ];
    let settings = ModelSettings {
        provider_replay: Some(ProviderReplaySettings {
            previous_response_id: Some("auto".to_string()),
            ..ProviderReplaySettings::default()
        }),
        ..ModelSettings::default()
    };

    let request =
        OpenAiResponsesAdapter::build_request("gpt-5.5", &messages, Some(&settings), &[], &[])
            .unwrap();

    assert!(request.get("previous_response_id").is_none());
    assert_eq!(request["input"].as_array().unwrap().len(), 3);
}

#[test]
fn responses_conversation_auto_and_concrete_trim_history() {
    let messages = vec![
        ModelMessage::Request(ModelRequest::user_text("old")),
        openai_response_with_conversation("conv_1"),
        ModelMessage::Request(ModelRequest::user_text("new")),
    ];
    let auto_settings = ModelSettings {
        provider_replay: Some(ProviderReplaySettings {
            conversation_id: Some("auto".to_string()),
            ..ProviderReplaySettings::default()
        }),
        ..ModelSettings::default()
    };
    let auto_request =
        OpenAiResponsesAdapter::build_request("gpt-5.5", &messages, Some(&auto_settings), &[], &[])
            .unwrap();
    assert_eq!(auto_request["conversation"], "conv_1");
    assert_eq!(auto_request["input"].as_array().unwrap().len(), 1);

    let concrete_settings = ModelSettings {
        provider_replay: Some(ProviderReplaySettings {
            conversation_id: Some("conv_1".to_string()),
            ..ProviderReplaySettings::default()
        }),
        ..ModelSettings::default()
    };
    let concrete_request = OpenAiResponsesAdapter::build_request(
        "gpt-5.5",
        &messages,
        Some(&concrete_settings),
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(concrete_request["conversation"], "conv_1");
    assert_eq!(concrete_request["input"].as_array().unwrap().len(), 1);
}

#[test]
fn responses_server_side_state_rejects_previous_response_and_conversation_conflict() {
    let settings = ModelSettings {
        provider_replay: Some(ProviderReplaySettings {
            previous_response_id: Some("auto".to_string()),
            conversation_id: Some("auto".to_string()),
            ..ProviderReplaySettings::default()
        }),
        ..ModelSettings::default()
    };
    let error = OpenAiResponsesAdapter::build_request("gpt-5.5", &[], Some(&settings), &[], &[])
        .unwrap_err();

    assert!(
        matches!(error, ModelError::MessageMapping(message) if message.contains("cannot both be set"))
    );
}

#[test]
fn responses_request_includes_encrypted_reasoning_when_thinking_is_enabled() {
    let settings = ModelSettings {
        thinking: Some(ThinkingSettings {
            effort: "high".to_string(),
            budget_tokens: None,
            mode: None,
            include_thoughts: None,
            summary: Some("auto".to_string()),
        }),
        ..ModelSettings::default()
    };

    let request = OpenAiResponsesAdapter::build_request(
        "gpt-5.5",
        &[ModelMessage::Request(ModelRequest::user_text("think"))],
        Some(&settings),
        &[],
        &[],
    )
    .unwrap();

    assert_eq!(request["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(request["reasoning"]["effort"], "high");
    assert_eq!(request["reasoning"]["summary"], "auto");
}

#[test]
fn responses_stream_requires_completed_event() {
    let error = OpenAiResponsesAdapter::parse_stream_events(&[
        json!({"type": "response.output_text.delta", "delta": "partial"}),
    ])
    .unwrap_err();

    assert!(
        matches!(error, ModelError::ResponseParsing(message) if message.contains("missing response.completed"))
    );
}

#[test]
fn responses_send_item_ids_false_does_not_default_encrypted_reasoning_include() {
    let settings = ModelSettings {
        thinking: Some(ThinkingSettings {
            effort: "high".to_string(),
            budget_tokens: None,
            mode: None,
            include_thoughts: None,
            summary: Some("auto".to_string()),
        }),
        provider_replay: Some(ProviderReplaySettings {
            send_item_ids: Some(false),
            ..ProviderReplaySettings::default()
        }),
        ..ModelSettings::default()
    };

    let request = OpenAiResponsesAdapter::build_request(
        "gpt-5.5",
        &[ModelMessage::Request(ModelRequest::user_text("think"))],
        Some(&settings),
        &[],
        &[],
    )
    .unwrap();

    assert!(request.get("include").is_none());
    assert_eq!(request["reasoning"]["effort"], "high");
}

#[test]
fn responses_replay_omits_encrypted_reasoning_when_disabled() {
    let messages = vec![ModelMessage::Response(ModelResponse {
        parts: vec![ModelResponsePart::ProviderThinking {
            text: "inspect".to_string(),
            signature: Some("encrypted".to_string()),
            provider: ProviderPartInfo::new("openai")
                .with_id("rs_1")
                .with_details({
                    let mut details = Metadata::default();
                    details.insert("raw_content".to_string(), json!(["raw"]));
                    details
                }),
        }],
        usage: Usage::default(),
        model_name: None,
        provider: None,
        finish_reason: None,
        timestamp: None,
        run_id: None,
        conversation_id: None,
        metadata: Metadata::default(),
    })];
    let settings = ModelSettings {
        provider_replay: Some(ProviderReplaySettings {
            include_encrypted_reasoning: Some(false),
            ..ProviderReplaySettings::default()
        }),
        ..ModelSettings::default()
    };

    let request =
        OpenAiResponsesAdapter::build_request("gpt-5.5", &messages, Some(&settings), &[], &[])
            .unwrap();

    assert_eq!(request["input"][0]["type"], "reasoning");
    assert_eq!(request["input"][0]["id"], "rs_1");
    assert!(request["input"][0].get("encrypted_content").is_none());
    assert_eq!(request["input"][0]["content"][0]["text"], "raw");
    assert!(request.get("include").is_none());
}

#[test]
fn responses_replay_send_item_ids_false_uses_safe_visible_fallbacks() {
    let messages = vec![ModelMessage::Response(ModelResponse {
        parts: vec![
            ModelResponsePart::ProviderText {
                text: "hello".to_string(),
                provider: ProviderPartInfo::new("openai").with_id("msg_1"),
            },
            ModelResponsePart::ProviderThinking {
                text: "inspect".to_string(),
                signature: Some("encrypted".to_string()),
                provider: ProviderPartInfo::new("openai").with_id("rs_1"),
            },
            ModelResponsePart::ProviderOpaque {
                item_type: "mcp_call".to_string(),
                payload: json!({"type": "mcp_call", "id": "mcp_1", "status": "completed"}),
                provider: ProviderPartInfo::new("openai").with_id("mcp_1"),
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
    })];
    let settings = ModelSettings {
        provider_replay: Some(ProviderReplaySettings {
            send_item_ids: Some(false),
            include_encrypted_reasoning: Some(false),
            ..ProviderReplaySettings::default()
        }),
        ..ModelSettings::default()
    };

    let request =
        OpenAiResponsesAdapter::build_request("gpt-5.5", &messages, Some(&settings), &[], &[])
            .unwrap();

    let input = request["input"].as_array().unwrap();
    assert_eq!(input.len(), 2);
    assert_eq!(input[0]["role"], "assistant");
    assert_eq!(input[0]["content"][0]["text"], "hello");
    assert_eq!(input[1]["content"][0]["text"], "<think>\ninspect\n</think>");
    let serialized = serde_json::to_string(&request).unwrap();
    assert!(!serialized.contains("msg_1"));
    assert!(!serialized.contains("rs_1"));
    assert!(!serialized.contains("mcp_1"));
    assert!(!serialized.contains("encrypted"));
    assert!(!serialized.contains("mcp_call"));
}

#[test]
fn responses_replays_cross_provider_thinking_as_tagged_text() {
    let messages = vec![ModelMessage::Response(ModelResponse {
        parts: vec![ModelResponsePart::ProviderThinking {
            text: "other reasoning".to_string(),
            signature: Some("foreign".to_string()),
            provider: ProviderPartInfo::new("anthropic").with_id("think_1"),
        }],
        usage: Usage::default(),
        model_name: None,
        provider: None,
        finish_reason: None,
        timestamp: None,
        run_id: None,
        conversation_id: None,
        metadata: Metadata::default(),
    })];

    let request =
        OpenAiResponsesAdapter::build_request("gpt-5.5", &messages, None, &[], &[]).unwrap();

    assert_eq!(
        request["input"][0]["content"][0]["text"],
        "<think>\nother reasoning\n</think>"
    );
}

fn openai_response_with_id(id: &str) -> ModelMessage {
    ModelMessage::Response(ModelResponse {
        parts: vec![ModelResponsePart::ProviderText {
            text: "stored".to_string(),
            provider: ProviderPartInfo::new("openai").with_id("msg_stored"),
        }],
        usage: Usage::default(),
        model_name: None,
        provider: Some(ProviderInfo {
            name: "openai".to_string(),
            response_id: Some(id.to_string()),
            details: Metadata::default(),
        }),
        finish_reason: None,
        timestamp: None,
        run_id: None,
        conversation_id: None,
        metadata: Metadata::default(),
    })
}

fn openai_response_with_conversation(conversation_id: &str) -> ModelMessage {
    let mut details = Metadata::default();
    details.insert("conversation_id".to_string(), json!(conversation_id));
    ModelMessage::Response(ModelResponse {
        parts: vec![ModelResponsePart::ProviderText {
            text: "stored".to_string(),
            provider: ProviderPartInfo::new("openai").with_id("msg_stored"),
        }],
        usage: Usage::default(),
        model_name: None,
        provider: Some(ProviderInfo {
            name: "openai".to_string(),
            response_id: Some("resp_1".to_string()),
            details,
        }),
        finish_reason: None,
        timestamp: None,
        run_id: None,
        conversation_id: None,
        metadata: Metadata::default(),
    })
}
