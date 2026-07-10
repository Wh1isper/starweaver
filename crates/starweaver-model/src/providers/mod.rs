//! Wire mappers for the first supported provider protocol families.

pub mod anthropic;
pub mod bedrock;
pub mod client;
pub mod gemini;
pub mod openai_chat;
pub mod openai_responses;

mod content;
mod openai_common;
mod settings;
mod system;
mod usage;

#[cfg(test)]
pub(crate) use content::text_from_content;
pub(crate) use content::{
    bedrock_content_from_content, gemini_parts_from_content, openai_chat_content_with_cache_points,
    openai_responses_content_with_cache_points,
};
pub(crate) use openai_common::{
    finish_reason_openai, openai_chat_tool_choice, openai_responses_tool_choice,
    parse_tool_call_arguments,
};
#[cfg(test)]
pub(crate) use settings::apply_common_settings;
pub(crate) use settings::{
    apply_common_settings_with_max_tokens, apply_common_settings_without_seed,
};
pub(crate) use system::{
    SystemInstructionPart, collect_system_and_non_system, collect_system_parts_and_non_system,
};
#[cfg(test)]
pub(crate) use usage::usage_from_named;
pub(crate) use usage::{
    usage_from_named_including_cache_input, usage_from_named_with_output_extras, usage_from_openai,
};

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;

use serde_json::{Map, Value, json};

pub(crate) fn provider_tool_schema_without_meta(parameters: &Value) -> Value {
    let mut schema = parameters.clone();
    remove_json_schema_meta(&mut schema);
    schema
}

pub(crate) fn insert_nonempty_description(
    object: &mut Map<String, Value>,
    description: Option<&String>,
) {
    if let Some(description) = description
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        object.insert("description".to_string(), json!(description));
    }
}

fn remove_json_schema_meta(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("$schema");
            for nested in object.values_mut() {
                remove_json_schema_meta(nested);
            }
        }
        Value::Array(items) => {
            for item in items {
                remove_json_schema_meta(item);
            }
        }
        _ => {}
    }
}
