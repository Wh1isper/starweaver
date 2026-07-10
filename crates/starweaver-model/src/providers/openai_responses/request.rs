//! `OpenAI` Responses request construction and replay mapping.

use serde_json::{Map, Value, json};

use crate::{
    ModelError, ModelSettings,
    adapter::{NativeToolDefinition, ToolDefinition},
    message::{ModelMessage, ModelRequestPart},
    providers::{apply_common_settings_without_seed, openai_responses_content_with_cache_points},
    settings::supports_openai_prompt_cache_breakpoints,
    transport::MaxTokensParameter,
};

mod instructions;
mod options;
mod replay_items;
mod server_state;
mod tools;

use instructions::collect_openai_instructions;
use options::OpenAiReplayOptions;
use replay_items::push_response_replay_items;
use server_state::resolve_server_side_state;
use tools::response_tool_defs;

#[allow(clippy::too_many_lines)]
pub(super) fn build_request_with_options(
    model: &str,
    messages: &[ModelMessage],
    settings: Option<&ModelSettings>,
    tools: &[ToolDefinition],
    native_tools: &[NativeToolDefinition],
    max_tokens_parameter: MaxTokensParameter,
) -> Result<Value, ModelError> {
    let supports_cache_points = supports_openai_prompt_cache_breakpoints(model);
    let replay = OpenAiReplayOptions::from_settings(settings);
    let instructions = collect_openai_instructions(messages);
    let (previous_response_id, conversation_id, messages) =
        resolve_server_side_state(messages, &replay)?;
    let mut input = Vec::new();

    for message in &messages {
        match message {
            ModelMessage::Request(request) => {
                for part in &request.parts {
                    match part {
                        ModelRequestPart::SystemPrompt { .. }
                        | ModelRequestPart::Instruction { .. } => {}
                        ModelRequestPart::UserPrompt { content, .. } => input.push(json!({
                            "role": "user",
                            "content": openai_responses_content_with_cache_points(
                                content,
                                supports_cache_points,
                            )?
                        })),
                        ModelRequestPart::ToolReturn(tool_return) => input.push(json!({
                            "type": "function_call_output",
                            "call_id": tool_return.tool_call_id,
                            "output": tool_return.content.to_string(),
                        })),
                        ModelRequestPart::RetryPrompt { text, .. } => input.push(json!({
                            "role": "user",
                            "content": [{"type": "input_text", "text": text}]
                        })),
                    }
                }
            }
            ModelMessage::Response(response) => {
                push_response_replay_items(response, &replay, &mut input);
            }
        }
    }

    let mut request = serde_json::Map::new();
    if input.is_empty() && previous_response_id.is_none() && conversation_id.is_none() {
        input.push(json!({"role": "user", "content": ""}));
    }
    request.insert("model".to_string(), json!(model));
    request.insert("input".to_string(), json!(input));
    if !instructions.is_empty() {
        request.insert("instructions".to_string(), json!(instructions.join("\n\n")));
    }
    apply_common_settings_without_seed(&mut request, settings, max_tokens_parameter);
    if let Some(openai_settings) =
        settings.and_then(|settings| settings.provider_settings.openai_responses.as_ref())
    {
        if let Some(store) = openai_settings.store {
            request.insert("store".to_string(), json!(store));
        }
        if let Some(user) = &openai_settings.user {
            request.insert("user".to_string(), json!(user));
        }
        if let Some(truncation) = &openai_settings.truncation {
            request.insert("truncation".to_string(), json!(truncation));
        }
        if let Some(context_management) = &openai_settings.context_management {
            request.insert("context_management".to_string(), context_management.clone());
        }
        if let Some(prompt_cache_key) = &openai_settings.prompt_cache_key {
            request.insert("prompt_cache_key".to_string(), json!(prompt_cache_key));
        }
        if openai_settings.prompt_cache_retention.is_some()
            && openai_settings.prompt_cache_options.is_some()
        {
            return Err(ModelError::MessageMapping(
                "OpenAI prompt_cache_retention and prompt_cache_options cannot both be configured"
                    .to_string(),
            ));
        }
        if let Some(prompt_cache_retention) = &openai_settings.prompt_cache_retention {
            request.insert(
                "prompt_cache_retention".to_string(),
                json!(prompt_cache_retention),
            );
        }
        if let Some(prompt_cache_options) = &openai_settings.prompt_cache_options {
            if !supports_cache_points {
                return Err(ModelError::MessageMapping(format!(
                    "model {model} does not support OpenAI prompt_cache_options"
                )));
            }
            request.insert(
                "prompt_cache_options".to_string(),
                serde_json::to_value(prompt_cache_options).map_err(|error| {
                    ModelError::MessageMapping(format!(
                        "invalid OpenAI prompt cache options: {error}"
                    ))
                })?,
            );
        }
        for include in &openai_settings.include {
            ensure_include(&mut request, include);
        }
        if let Some(text_verbosity) = &openai_settings.text_verbosity {
            let text = request
                .entry("text".to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if let Some(text) = text.as_object_mut() {
                text.insert("verbosity".to_string(), json!(text_verbosity));
            }
        }
    }
    if let Some(previous_response_id) = previous_response_id {
        request.insert(
            "previous_response_id".to_string(),
            json!(previous_response_id),
        );
    }
    if let Some(conversation_id) = conversation_id {
        request.insert("conversation".to_string(), json!(conversation_id));
    }
    if replay.include_encrypted_reasoning {
        ensure_include(&mut request, "reasoning.encrypted_content");
    }
    if let Some(thinking) = settings.and_then(|settings| settings.thinking.as_ref()) {
        let mut reasoning = serde_json::Map::new();
        reasoning.insert("effort".to_string(), json!(thinking.effort));
        if let Some(mode) = &thinking.mode {
            reasoning.insert("mode".to_string(), json!(mode));
        }
        if let Some(summary) = &thinking.summary {
            reasoning.insert("summary".to_string(), json!(summary));
        }
        request.insert("reasoning".to_string(), Value::Object(reasoning));
        request.remove("reasoning_effort");
    }
    if let Some(tool_choice) = settings.and_then(|settings| settings.tool_choice.as_ref()) {
        request.insert(
            "tool_choice".to_string(),
            crate::providers::openai_responses_tool_choice(tool_choice),
        );
    }
    let tool_defs = response_tool_defs(tools, native_tools);
    if !tool_defs.is_empty() {
        request.insert("tools".to_string(), json!(tool_defs));
    }
    Ok(Value::Object(request))
}

pub(super) fn response_replay_items(
    response: &crate::message::ModelResponse,
    settings: Option<&ModelSettings>,
) -> Vec<Value> {
    let replay = OpenAiReplayOptions::from_settings(settings);
    replay_items::response_replay_items(response, &replay)
}

fn ensure_include(request: &mut Map<String, Value>, include: &str) {
    let entry = request
        .entry("include".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(items) = entry.as_array_mut()
        && !items.iter().any(|item| item.as_str() == Some(include))
    {
        items.push(Value::String(include.to_string()));
    }
}
