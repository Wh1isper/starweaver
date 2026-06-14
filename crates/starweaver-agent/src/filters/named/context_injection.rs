use serde_json::{json, Map, Value};
use starweaver_model::{ModelMessage, ModelRequest, ModelRequestPart, ToolReturnPart};
use starweaver_runtime::AgentRunState;

use crate::filters::{
    compact::instruction_parts,
    message::{
        insert_request_part_after_control_parts, metadata_text, push_user_text,
        request_metadata_mut,
    },
};

pub(super) const AUTO_LOAD_METADATA: &str = "starweaver_auto_load_files";
pub(super) const BACKGROUND_SHELL_METADATA: &str = "starweaver_background_shell";
pub(super) const BUS_MESSAGE_METADATA: &str = "starweaver_bus_messages";
pub(super) const ENVIRONMENT_INSTRUCTIONS_METADATA: &str = "starweaver_environment_instructions";
pub(super) const RUNTIME_INSTRUCTIONS_METADATA: &str = "starweaver_runtime_instructions";
pub(super) const HANDOFF_METADATA: &str = "starweaver_handoff";
const COLD_START_TOOL_RETURN_LIMIT_METADATA: &str = "starweaver_cold_start_tool_return_limit";

pub(super) fn cold_start_filter(
    state: &AgentRunState,
    mut messages: Vec<ModelMessage>,
) -> Vec<ModelMessage> {
    let limit = state
        .metadata
        .get(COLD_START_TOOL_RETURN_LIMIT_METADATA)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(500);
    let trim_end = messages
        .iter()
        .rposition(|message| matches!(message, ModelMessage::Response(_)))
        .unwrap_or(messages.len());
    let mut truncated = 0usize;
    for message in messages.iter_mut().take(trim_end) {
        if let ModelMessage::Request(request) = message {
            for part in &mut request.parts {
                if let ModelRequestPart::ToolReturn(tool_return) = part {
                    if truncate_tool_return(tool_return, limit) {
                        truncated += 1;
                    }
                }
            }
        }
    }
    if truncated > 0 {
        request_metadata_mut(&mut messages).insert(
            "starweaver_cold_start_truncated_tool_returns".to_string(),
            json!(truncated),
        );
    }
    if !state.idle_messages.is_empty() {
        push_user_text(
            &mut messages,
            format!("Cold-start context: {}", state.idle_messages.join("\n")),
            "cold_start",
        );
    }
    messages
}

pub(super) fn system_prompt_filter(
    state: &AgentRunState,
    mut messages: Vec<ModelMessage>,
) -> Vec<ModelMessage> {
    let source_parts = instruction_parts(&state.message_history);
    if source_parts.is_empty() {
        return messages;
    }
    let existing = instruction_parts(&messages);
    let has_all = source_parts.iter().all(|part| existing.contains(part));
    if has_all {
        return messages;
    }
    messages.insert(
        0,
        ModelMessage::Request(ModelRequest {
            parts: source_parts,
            timestamp: None,
            instructions: None,
            run_id: Some(state.run_id.clone()),
            conversation_id: Some(state.conversation_id.clone()),
            metadata: Map::new(),
        }),
    );
    messages
}

pub(super) fn inject_instruction_from_metadata(
    state: &AgentRunState,
    mut messages: Vec<ModelMessage>,
    metadata_key: &str,
    instruction_type: &str,
) -> Vec<ModelMessage> {
    let Some(text) = metadata_text(&state.metadata, metadata_key) else {
        return messages;
    };
    let part = ModelRequestPart::Instruction {
        text,
        metadata: Map::from_iter([(
            "starweaver_instruction_type".to_string(),
            json!(instruction_type),
        )]),
    };
    insert_request_part_after_control_parts(&mut messages, part);
    messages
}

pub(super) fn auto_load_files_filter(
    state: &AgentRunState,
    mut messages: Vec<ModelMessage>,
) -> Vec<ModelMessage> {
    let Some(files) = state
        .metadata
        .get(AUTO_LOAD_METADATA)
        .and_then(Value::as_array)
    else {
        return messages;
    };
    let mut loaded = Vec::new();
    for file in files {
        let path = file
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let content = file
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        loaded.push(format!("## {path}\n{content}"));
    }
    if loaded.is_empty() {
        return messages;
    }
    push_user_text(
        &mut messages,
        format!("Auto-loaded files:\n{}", loaded.join("\n\n")),
        "auto_load_files",
    );
    messages
}

pub(super) fn background_shell_filter(
    state: &AgentRunState,
    mut messages: Vec<ModelMessage>,
) -> Vec<ModelMessage> {
    let Some(processes) = state
        .metadata
        .get(BACKGROUND_SHELL_METADATA)
        .and_then(Value::as_array)
    else {
        return messages;
    };
    if processes.is_empty() {
        return messages;
    }
    push_user_text(
        &mut messages,
        format!(
            "Background shell updates: {}",
            Value::Array(processes.clone())
        ),
        "background_shell",
    );
    messages
}

pub(super) fn bus_message_filter(
    state: &AgentRunState,
    mut messages: Vec<ModelMessage>,
) -> Vec<ModelMessage> {
    let Some(bus_messages) = state
        .metadata
        .get(BUS_MESSAGE_METADATA)
        .and_then(Value::as_array)
    else {
        return messages;
    };
    if bus_messages.is_empty() {
        return messages;
    }
    push_user_text(
        &mut messages,
        format!(
            "Message bus updates: {}",
            Value::Array(bus_messages.clone())
        ),
        "bus_message",
    );
    messages
}

fn truncate_tool_return(tool_return: &mut ToolReturnPart, limit: usize) -> bool {
    let content_text = value_model_response_text(&tool_return.content);
    if content_text.chars().count() <= limit {
        return false;
    }
    let original_chars = content_text.chars().count();
    tool_return.content = json!(truncate_head_tail(&content_text, limit));
    if let Some(user_content) = &mut tool_return.user_content {
        let user_text = value_model_response_text(user_content);
        if user_text.chars().count() > limit {
            *user_content = json!(truncate_head_tail(&user_text, limit));
        }
    }
    tool_return
        .metadata
        .insert("starweaver_truncated".to_string(), json!(true));
    tool_return.metadata.insert(
        "starweaver_original_chars".to_string(),
        json!(original_chars),
    );
    true
}

fn value_model_response_text(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_string)
}

fn truncate_head_tail(text: &str, limit: usize) -> String {
    let total = text.chars().count();
    if total <= limit {
        return text.to_string();
    }
    let head_len = 200.min(limit / 2);
    let tail_len = 200.min(limit.saturating_sub(head_len));
    let head = text.chars().take(head_len).collect::<String>();
    let tail = text
        .chars()
        .rev()
        .take(tail_len)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    let truncated = total.saturating_sub(head_len + tail_len);
    format!("{head}\n[... {truncated} chars truncated ...]\n{tail}")
}
