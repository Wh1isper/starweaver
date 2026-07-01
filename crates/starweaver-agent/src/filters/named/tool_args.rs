use serde_json::{Value, json};
use starweaver_model::{ModelMessage, ModelResponsePart, ToolArguments};

use crate::filters::message::request_metadata_mut;

const INVALID_TOOL_ARGS_MESSAGE: &str =
    "This tool's args is not a valid JSON. Please refer the return value of the tool to try again.";

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
        ToolArguments::Invalid { raw, .. } if !raw.is_empty() => {
            Some(invalid_tool_args_placeholder())
        }
        ToolArguments::RawJsonString(text) | ToolArguments::Parsed(Value::String(text)) => {
            invalid_placeholder_if_json_is_invalid(text)
        }
        ToolArguments::Invalid { .. } | ToolArguments::Parsed(_) => None,
    }
}

fn invalid_placeholder_if_json_is_invalid(text: &str) -> Option<ToolArguments> {
    if text.is_empty() || serde_json::from_str::<Value>(text).is_ok() {
        None
    } else {
        Some(invalid_tool_args_placeholder())
    }
}

fn invalid_tool_args_placeholder() -> ToolArguments {
    ToolArguments::parsed(json!({ "system": INVALID_TOOL_ARGS_MESSAGE }))
}
