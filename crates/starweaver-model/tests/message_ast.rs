#![allow(missing_docs, clippy::unwrap_used)]

use serde_json::{Value, json};
use starweaver_model::{ModelResponse, ModelResponsePart, ToolArguments, ToolCallPart};

#[test]
fn tool_arguments_preserve_parsed_raw_and_invalid_states() {
    let parsed = ToolArguments::from_provider_value(&json!("{\"query\":\"starweaver\"}"));
    assert_eq!(parsed.execution_value(), json!({"query": "starweaver"}));
    assert_eq!(parsed.wire_json_string(), "{\"query\":\"starweaver\"}");
    assert_eq!(
        serde_json::to_value(&parsed).unwrap(),
        json!({"query": "starweaver"})
    );

    let raw = ToolArguments::raw_json_string("{\"delayed\":true}");
    assert_eq!(raw.execution_value(), json!({"delayed": true}));
    assert_eq!(raw.wire_json_string(), "{\"delayed\":true}");
    assert_eq!(
        serde_json::to_value(&raw).unwrap(),
        json!("{\"delayed\":true}")
    );

    let invalid = ToolArguments::from_provider_value(&json!("{bad-json"));
    assert_eq!(invalid.execution_value(), json!("{bad-json"));
    assert_eq!(invalid.wire_json_string(), "{bad-json");
    assert!(invalid.invalid_error().is_some());
    assert_eq!(
        serde_json::to_value(&invalid).unwrap()["kind"],
        "starweaver_invalid_tool_arguments"
    );
}

#[test]
fn tool_arguments_invalid_marker_round_trips_through_json() {
    let invalid = ToolArguments::invalid("{bad-json", "expected key");
    let encoded = serde_json::to_value(&invalid).unwrap();
    let decoded: ToolArguments = serde_json::from_value(encoded).unwrap();

    assert_eq!(decoded, invalid);
    assert_eq!(decoded.execution_value(), json!("{bad-json"));
    assert_eq!(decoded.invalid_error(), Some("expected key"));
}

#[test]
fn tool_call_part_serializes_arguments_as_replay_value() {
    let response = ModelResponse {
        parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
            id: "call_1".to_string(),
            name: "lookup".to_string(),
            arguments: ToolArguments::invalid("{bad-json", "expected value"),
        })],
        ..ModelResponse::text("")
    };

    let encoded = serde_json::to_value(&response).unwrap();
    assert_eq!(encoded["parts"][0]["arguments"]["raw"], "{bad-json");
    assert_eq!(encoded["parts"][0]["arguments"]["error"], "expected value");

    let decoded: ModelResponse = serde_json::from_value(encoded).unwrap();
    let ModelResponsePart::ToolCall(call) = &decoded.parts[0] else {
        panic!("expected tool call")
    };
    assert_eq!(call.arguments.wire_json_string(), "{bad-json");
}

#[test]
fn parsed_tool_arguments_compare_to_json_values_for_equivalence() {
    let args = ToolArguments::parsed(json!({"ok": true}));
    let value: Value = json!({"ok": true});

    assert_eq!(args, value);
    assert_eq!(value, args);
}
