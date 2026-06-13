#![allow(missing_docs, clippy::unwrap_used)]

use starweaver_model::{
    ModelResponse, ModelResponsePart, ModelResponseStreamEvent, ModelStreamState, PartDelta,
    PartEnd, PartStart, StreamDelta, StreamLifecycle, ToolCallPart,
};

#[test]
fn replays_openai_chat_streaming_text_delta_fixture() {
    let events = [
        ModelResponseStreamEvent::PartStart(PartStart {
            index: 0,
            part_kind: "text".to_string(),
        }),
        ModelResponseStreamEvent::PartDelta(PartDelta::text(0, "Hel")),
        ModelResponseStreamEvent::PartDelta(PartDelta::text(0, "lo")),
        ModelResponseStreamEvent::PartEnd(PartEnd::new(0)),
        ModelResponseStreamEvent::FinalResult(Box::new(ModelResponse::text("Hello"))),
    ];

    assert_eq!(events.len(), 5);
    assert!(matches!(events[0], ModelResponseStreamEvent::PartStart(_)));
    assert!(matches!(
        events[4],
        ModelResponseStreamEvent::FinalResult(_)
    ));

    let mut state = ModelStreamState::default();
    for event in &events {
        state.apply(event);
    }
    assert_eq!(state.lifecycle, StreamLifecycle::Complete);
    assert_eq!(state.started_parts, 1);
    assert_eq!(state.ended_parts, 1);
}

#[test]
fn replays_openai_chat_streaming_tool_call_argument_delta_fixture() {
    let events = tool_call_delta_events("call_1", "lookup");

    let encoded = serde_json::to_value(&events).unwrap();
    let decoded: Vec<ModelResponseStreamEvent> = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, events);
}

#[test]
fn replays_openai_responses_streaming_function_call_delta_fixture() {
    let events = tool_call_delta_events("fc_1", "lookup");

    let encoded = serde_json::to_value(&events).unwrap();
    let decoded: Vec<ModelResponseStreamEvent> = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, events);
}

#[test]
fn replays_cross_provider_streaming_delta_and_usage_at_end_fixtures() {
    let providers = ["openai_responses", "anthropic", "gemini", "bedrock"];
    for provider in providers {
        let final_result = ModelResponse {
            usage: starweaver_core::Usage {
                requests: 1,
                input_tokens: 10,
                cache_write_tokens: 0,
                cache_read_tokens: 0,
                output_tokens: 2,
                total_tokens: 12,
                tool_calls: 0,
            },
            provider: Some(starweaver_model::ProviderInfo {
                name: provider.to_string(),
                response_id: None,
                details: serde_json::Map::new(),
            }),
            ..ModelResponse::text("ok")
        };
        let events = [
            ModelResponseStreamEvent::PartStart(PartStart {
                index: 0,
                part_kind: "text".to_string(),
            }),
            ModelResponseStreamEvent::PartDelta(PartDelta::text(0, "ok")),
            ModelResponseStreamEvent::PartEnd(PartEnd::new(0)),
            ModelResponseStreamEvent::FinalResult(Box::new(final_result.clone())),
        ];
        assert_eq!(
            events.last(),
            Some(&ModelResponseStreamEvent::FinalResult(Box::new(
                final_result
            )))
        );
    }
}

fn tool_call_delta_events(call_id: &str, name: &str) -> Vec<ModelResponseStreamEvent> {
    vec![
        ModelResponseStreamEvent::PartStart(PartStart {
            index: 0,
            part_kind: "tool_call".to_string(),
        }),
        ModelResponseStreamEvent::PartDelta(PartDelta {
            index: 0,
            delta: StreamDelta::ToolCallArguments {
                arguments_delta: "{\"query\":".to_string(),
            },
        }),
        ModelResponseStreamEvent::PartDelta(PartDelta {
            index: 0,
            delta: StreamDelta::ToolCallArguments {
                arguments_delta: "\"Paris\"}".to_string(),
            },
        }),
        ModelResponseStreamEvent::PartEnd(PartEnd::new(0)),
        ModelResponseStreamEvent::FinalResult(Box::new(ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: call_id.to_string(),
                name: name.to_string(),
                arguments: serde_json::json!({"query": "Paris"}).into(),
            })],
            ..ModelResponse::text("")
        })),
    ]
}
