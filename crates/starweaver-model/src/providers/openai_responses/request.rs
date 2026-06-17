//! `OpenAI` Responses request construction and replay mapping.

use serde_json::{json, Map, Value};

use crate::{
    adapter::{NativeToolDefinition, ToolDefinition},
    message::{ModelMessage, ModelRequestPart},
    providers::{apply_common_settings_without_seed, openai_responses_content},
    transport::MaxTokensParameter,
    ModelError, ModelSettings,
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
                            "content": openai_responses_content(content)
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
    if let Some(openai) =
        settings.and_then(|settings| settings.provider_settings.openai_responses.as_ref())
    {
        if let Some(store) = openai.store {
            request.insert("store".to_string(), json!(store));
        }
        if let Some(user) = &openai.user {
            request.insert("user".to_string(), json!(user));
        }
        if let Some(truncation) = &openai.truncation {
            request.insert("truncation".to_string(), json!(truncation));
        }
        if let Some(context_management) = &openai.context_management {
            request.insert("context_management".to_string(), context_management.clone());
        }
        if let Some(prompt_cache_key) = &openai.prompt_cache_key {
            request.insert("prompt_cache_key".to_string(), json!(prompt_cache_key));
        }
        if let Some(prompt_cache_retention) = &openai.prompt_cache_retention {
            request.insert(
                "prompt_cache_retention".to_string(),
                json!(prompt_cache_retention),
            );
        }
        for include in &openai.include {
            ensure_include(&mut request, include);
        }
        if let Some(text_verbosity) = &openai.text_verbosity {
            let text = request
                .entry("text".to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if let Some(text) = text.as_object_mut() {
                text.insert("verbosity".to_string(), json!(text_verbosity));
            }
        }
    }
    strip_openai_replay_aliases(&mut request);
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

fn strip_openai_replay_aliases(request: &mut Map<String, Value>) {
    for key in [
        "openai_previous_response_id",
        "openai_conversation_id",
        "openai_send_reasoning_ids",
        "openai_include_encrypted_reasoning",
    ] {
        request.remove(key);
    }
}

fn ensure_include(request: &mut Map<String, Value>, include: &str) {
    let entry = request
        .entry("include".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(items) = entry.as_array_mut() {
        if !items.iter().any(|item| item.as_str() == Some(include)) {
            items.push(Value::String(include.to_string()));
        }
    }
}
