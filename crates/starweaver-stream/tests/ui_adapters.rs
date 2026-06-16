//! UI adapter and trust-boundary sanitizer tests.

use serde_json::json;
use starweaver_core::{RunId, SessionId};
use starweaver_model::{
    ContentPart, ModelMessage, ModelRequest, ModelRequestPart, ModelResponse, ModelResponsePart,
    ToolArguments, ToolCallPart, ToolReturnPart,
};
use starweaver_stream::{
    display_to_agui_event, display_to_agui_jsonl, display_to_vercel_data_stream,
    sanitize_client_history, ClientHistorySanitizerConfig, DisplayMessage, DisplayMessageKind,
};

fn display_message(
    sequence: usize,
    kind: DisplayMessageKind,
    payload: serde_json::Value,
) -> DisplayMessage {
    DisplayMessage::new(
        sequence,
        SessionId::from_string("session-ui"),
        RunId::from_string("run-ui"),
        kind,
    )
    .with_payload(payload)
}

#[test]
fn agui_adapter_projects_display_messages_to_top_level_events() -> serde_json::Result<()> {
    let message = display_message(
        7,
        DisplayMessageKind::AssistantTextDelta,
        json!({"delta": "hello"}),
    )
    .with_preview("hello");

    let event = display_to_agui_event(&message);
    assert_eq!(event.event_type, "TEXT_MESSAGE_CONTENT");
    assert_eq!(event.id, "7");
    assert_eq!(event.session_id, "session-ui");
    assert_eq!(event.run_id, "run-ui");
    assert_eq!(event.payload["delta"], "hello");
    assert_eq!(event.payload["preview"], "hello");

    let jsonl = display_to_agui_jsonl(&[message])?;
    assert!(jsonl.contains("TEXT_MESSAGE_CONTENT"));
    assert!(jsonl.ends_with('\n'));
    Ok(())
}

#[test]
fn vercel_adapter_projects_text_and_terminal_parts() {
    let text = display_message(
        1,
        DisplayMessageKind::AssistantTextDelta,
        json!({"delta": "hel"}),
    );
    let terminal = display_message(
        2,
        DisplayMessageKind::RunCompleted,
        json!({"output": "done"}),
    );

    let text_parts = display_to_vercel_data_stream(&text);
    assert_eq!(text_parts[0].part_type, "text-delta");
    assert_eq!(text_parts[0].value["textDelta"], "hel");

    let terminal_parts = display_to_vercel_data_stream(&terminal);
    assert_eq!(terminal_parts[0].part_type, "finish");
    assert_eq!(terminal_parts[0].value["output"], "done");
}

#[test]
fn sanitizer_demotes_system_prompts_and_drops_dangling_tool_returns_and_file_urls() {
    let messages = vec![ModelMessage::Request(ModelRequest {
        parts: vec![
            ModelRequestPart::SystemPrompt {
                text: "secret system".to_string(),
                metadata: serde_json::Map::new(),
            },
            ModelRequestPart::UserPrompt {
                content: vec![
                    ContentPart::Text {
                        text: "hello".to_string(),
                    },
                    ContentPart::FileUrl {
                        url: "file:///etc/passwd".to_string(),
                        media_type: "text/plain".to_string(),
                    },
                    ContentPart::ImageUrl {
                        url: "https://example.com/image.png".to_string(),
                    },
                ],
                name: None,
                metadata: serde_json::Map::new(),
            },
            ModelRequestPart::ToolReturn(ToolReturnPart::new(
                "missing-call",
                "lookup",
                json!({"ok": true}),
            )),
        ],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    })];

    let sanitized = sanitize_client_history(messages, &ClientHistorySanitizerConfig::default());

    assert_eq!(sanitized.messages.len(), 1);
    assert_eq!(sanitized.decisions.len(), 3);
    assert!(sanitized
        .decisions
        .iter()
        .any(|decision| decision.kind == "demoted_system_prompt"));
    assert!(sanitized
        .decisions
        .iter()
        .any(|decision| decision.kind == "dropped_disallowed_url"));
    assert!(sanitized
        .decisions
        .iter()
        .any(|decision| decision.kind == "dropped_dangling_tool_return"));

    let ModelMessage::Request(request) = &sanitized.messages[0] else {
        panic!("request expected");
    };
    assert!(matches!(
        &request.parts[0],
        ModelRequestPart::UserPrompt { content, .. }
            if matches!(&content[0], ContentPart::Text { text } if text == "secret system")
    ));
    assert!(matches!(
        &request.parts[1],
        ModelRequestPart::UserPrompt { content, .. } if content.len() == 2
    ));
}

#[test]
fn sanitizer_accepts_matching_tool_return_after_prior_tool_call() {
    let messages = vec![
        ModelMessage::Response(ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: "call-1".to_string(),
                name: "lookup".to_string(),
                arguments: ToolArguments::parsed(json!({})),
            })],
            usage: starweaver_usage::Usage::default(),
            model_name: None,
            provider: None,
            finish_reason: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        }),
        ModelMessage::Request(ModelRequest {
            parts: vec![ModelRequestPart::ToolReturn(ToolReturnPart::new(
                "call-1",
                "lookup",
                json!({"ok": true}),
            ))],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        }),
    ];

    let sanitized = sanitize_client_history(messages, &ClientHistorySanitizerConfig::default());

    assert!(sanitized.decisions.is_empty());
    let ModelMessage::Request(request) = &sanitized.messages[1] else {
        panic!("request expected");
    };
    assert!(matches!(request.parts[0], ModelRequestPart::ToolReturn(_)));
}
