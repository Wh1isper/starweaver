use serde_json::{json, Value};
use starweaver_model::{ModelMessage, ModelResponsePart, ToolArguments};

use crate::filters::message::request_metadata_mut;

pub(super) fn tool_args_filter(mut messages: Vec<ModelMessage>) -> Vec<ModelMessage> {
    let mut repaired = 0usize;
    for message in &mut messages {
        if let ModelMessage::Response(response) = message {
            for part in &mut response.parts {
                match part {
                    ModelResponsePart::ToolCall(call)
                    | ModelResponsePart::ProviderToolCall { call, .. } => {
                        if let Some(repaired_args) = repair_tool_arguments(&call.arguments) {
                            call.arguments = repaired_args;
                            repaired += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    if repaired > 0 {
        request_metadata_mut(&mut messages)
            .insert("starweaver_tool_args_repaired".to_string(), json!(repaired));
    }
    messages
}

fn repair_tool_arguments(arguments: &ToolArguments) -> Option<ToolArguments> {
    match arguments {
        ToolArguments::RawJsonString(text) => serde_json::from_str::<Value>(text)
            .ok()
            .map(ToolArguments::parsed),
        ToolArguments::Invalid { raw, .. } => Some(ToolArguments::parsed(json!({
            "starweaver_argument_repair": "invalid_json_string",
            "raw": raw,
        }))),
        ToolArguments::Parsed(Value::String(text)) => serde_json::from_str::<Value>(text)
            .ok()
            .map(ToolArguments::parsed)
            .or_else(|| {
                Some(ToolArguments::parsed(json!({
                    "starweaver_argument_repair": "invalid_json_string",
                    "raw": text,
                })))
            }),
        ToolArguments::Parsed(Value::Null) => Some(ToolArguments::parsed(json!({}))),
        ToolArguments::Parsed(Value::Object(_)) => None,
        ToolArguments::Parsed(other) => Some(ToolArguments::parsed(json!({
            "starweaver_argument_repair": "non_object_arguments",
            "value": other,
        }))),
    }
}
