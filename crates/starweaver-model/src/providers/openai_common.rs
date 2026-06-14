//! `OpenAI` provider shared helpers.

use serde_json::{json, Value};

use crate::{
    message::{FinishReason, ToolArguments},
    settings::ToolChoice,
};

pub fn finish_reason_openai(reason: &str) -> FinishReason {
    match reason {
        "stop" | "completed" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" => FinishReason::ToolCalls,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Unknown,
    }
}

pub fn openai_chat_tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto | ToolChoice::ToolOrOutput { .. } => json!("auto"),
        ToolChoice::None => json!("none"),
        ToolChoice::Required | ToolChoice::Tools { .. } => json!("required"),
        ToolChoice::Tool { name } => json!({
            "type": "function",
            "function": {"name": name}
        }),
    }
}

pub fn openai_responses_tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::None => json!("none"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Tools { names } => json!({
            "type": "allowed_tools",
            "mode": "required",
            "tools": function_tool_descriptors(names),
        }),
        ToolChoice::ToolOrOutput { function_tools } => json!({
            "type": "allowed_tools",
            "mode": "auto",
            "tools": function_tool_descriptors(function_tools),
        }),
        ToolChoice::Tool { name } => json!({
            "type": "function",
            "name": name,
        }),
    }
}

fn function_tool_descriptors(names: &[String]) -> Vec<Value> {
    names
        .iter()
        .map(|name| json!({"type": "function", "name": name}))
        .collect()
}

pub fn parse_tool_call_arguments(value: &Value) -> ToolArguments {
    ToolArguments::from_provider_value(value)
}
