//! Wire mappers for the first supported provider protocol families.

pub mod anthropic;
pub mod bedrock;
pub mod client;
pub mod gemini;
pub mod openai_chat;
pub mod openai_responses;

mod content;
mod openai_common;
mod schema;
mod settings;
mod system;
mod usage;

#[cfg(test)]
pub(crate) use content::text_from_content;
pub(crate) use content::{
    bedrock_content_from_content, gemini_parts_from_content, openai_chat_content,
    openai_responses_content,
};
pub(crate) use openai_common::{
    finish_reason_openai, openai_chat_tool_choice, openai_responses_tool_choice,
    parse_tool_call_arguments,
};
pub(crate) use schema::{insert_optional_description, provider_tool_parameters};
pub(crate) use settings::{apply_common_settings, apply_common_settings_with_max_tokens};
pub(crate) use system::{
    collect_system_and_non_system, collect_system_parts_and_non_system,
    is_dynamic_system_instruction, SystemInstructionPart,
};
#[cfg(test)]
pub(crate) use usage::usage_from_named;
pub(crate) use usage::{
    usage_from_named_including_cache_input, usage_from_named_with_output_extras, usage_from_openai,
};

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
