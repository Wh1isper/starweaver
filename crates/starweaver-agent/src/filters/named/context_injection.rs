use serde_json::{json, Map, Value};
use starweaver_model::{
    ContentPart, ModelMessage, ModelRequest, ModelRequestPart, ToolReturnPart,
    CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT, CONTEXT_ORIGIN_HANDOFF, CONTEXT_ORIGIN_METADATA,
    CONTEXT_ORIGIN_RUNTIME_CONTEXT, CONTEXT_TYPE_METADATA, INSTRUCTION_DYNAMIC_METADATA,
    INSTRUCTION_ORIGIN_DYNAMIC_INSTRUCTION, INSTRUCTION_ORIGIN_METADATA,
};
use starweaver_runtime::AgentRunState;

use crate::filters::{
    compact::instruction_parts,
    message::{
        build_handoff_request_parts, build_restored_request_parts,
        insert_context_part_after_control_parts, insert_context_parts_after_control_parts,
        insert_request_part_after_control_parts, metadata_text, push_user_text,
        request_metadata_mut,
    },
};

pub(super) const AUTO_LOAD_METADATA: &str = "starweaver_auto_load_files";
pub(super) const BACKGROUND_SHELL_METADATA: &str = "starweaver_background_shell";
pub(super) const BUS_MESSAGE_METADATA: &str = "starweaver_bus_messages";
pub(super) const ENVIRONMENT_CONTEXT_METADATA: &str = "starweaver_environment_context";
pub(super) const RUNTIME_CONTEXT_METADATA: &str = "starweaver_runtime_context";
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
    if let Some(origin) = context_origin_for_type(instruction_type) {
        if instruction_type == "handoff" && !text.contains("<context-restored>") {
            let mut parts = build_restored_request_parts(
                Some(vec![ContentPart::Text { text }]),
                None,
                Vec::new(),
            );
            annotate_context_parts(&mut parts, instruction_type, &origin);
            insert_context_parts_after_control_parts(&mut messages, parts);
            return messages;
        }
        insert_context_part_after_control_parts(
            &mut messages,
            ModelRequestPart::UserPrompt {
                content: vec![ContentPart::Text { text }],
                name: None,
                metadata: context_metadata(instruction_type, &origin),
            },
        );
        return messages;
    }
    let origin = instruction_origin(instruction_type);
    let mut metadata = Map::from_iter([
        (
            "starweaver_instruction_type".to_string(),
            json!(instruction_type),
        ),
        (INSTRUCTION_ORIGIN_METADATA.to_string(), json!(origin)),
    ]);
    metadata.insert(
        INSTRUCTION_DYNAMIC_METADATA.to_string(),
        json!(instruction_is_dynamic(&origin)),
    );
    let part = ModelRequestPart::Instruction { text, metadata };
    insert_request_part_after_control_parts(&mut messages, part);
    messages
}

pub(super) fn handoff_filter(
    state: &AgentRunState,
    context: &mut starweaver_context::AgentContext,
    mut messages: Vec<ModelMessage>,
) -> Vec<ModelMessage> {
    // The handoff processor owns this flag and clears it at the start
    // of every filter pipeline pass, then sets it again only after a handoff restore.
    context.force_inject_context = false;

    let handoff_message = context
        .handoff_message
        .clone()
        .filter(|message| !message.trim().is_empty())
        .or_else(|| metadata_text(&state.metadata, HANDOFF_METADATA));
    let Some(handoff_message) = handoff_message else {
        return messages;
    };

    if handoff_message.contains("<context-restored>") {
        context.handoff_message = None;
        context.force_inject_context = true;
        return inject_instruction_text(messages, handoff_message, "handoff");
    }

    let original_request = context
        .user_prompts
        .clone()
        .filter(|content| !content.is_empty());
    let previous_reference = context
        .previous_assistant_response_reference
        .as_deref()
        .filter(|reference| !reference.trim().is_empty());
    let steering_messages = context.steering_messages.clone();
    let mut parts = instruction_parts(&messages);
    parts.extend(build_handoff_request_parts(
        handoff_message,
        original_request,
        previous_reference,
        steering_messages,
    ));
    if let Some(origin) = context_origin_for_type("handoff") {
        annotate_context_parts(&mut parts, "handoff", &origin);
    }

    messages = vec![ModelMessage::Request(ModelRequest {
        parts,
        timestamp: None,
        instructions: None,
        run_id: Some(state.run_id.clone()),
        conversation_id: Some(state.conversation_id.clone()),
        metadata: Map::from_iter([("keep".to_string(), json!("handoff"))]),
    })];

    context.handoff_message = None;
    context.steering_messages.clear();
    context.force_inject_context = true;
    messages
}

pub(super) fn inject_instruction_text(
    mut messages: Vec<ModelMessage>,
    text: String,
    instruction_type: &str,
) -> Vec<ModelMessage> {
    if let Some(origin) = context_origin_for_type(instruction_type) {
        insert_context_part_after_control_parts(
            &mut messages,
            ModelRequestPart::UserPrompt {
                content: vec![ContentPart::Text { text }],
                name: None,
                metadata: context_metadata(instruction_type, &origin),
            },
        );
        return messages;
    }
    let origin = instruction_origin(instruction_type);
    let mut metadata = Map::from_iter([
        (
            "starweaver_instruction_type".to_string(),
            json!(instruction_type),
        ),
        (
            INSTRUCTION_ORIGIN_METADATA.to_string(),
            json!(origin.as_str()),
        ),
    ]);
    metadata.insert(
        INSTRUCTION_DYNAMIC_METADATA.to_string(),
        json!(instruction_is_dynamic(&origin)),
    );
    insert_request_part_after_control_parts(
        &mut messages,
        ModelRequestPart::Instruction { text, metadata },
    );
    messages
}

pub(super) async fn auto_load_files_filter(
    state: &AgentRunState,
    context: &mut starweaver_context::AgentContext,
    mut messages: Vec<ModelMessage>,
) -> Vec<ModelMessage> {
    let mut loaded = Vec::new();
    if let Some(files) = state
        .metadata
        .get(AUTO_LOAD_METADATA)
        .and_then(Value::as_array)
    {
        for file in files {
            let path = file
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let file_text = file
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            loaded.push(format!("### `{path}`\n\n```\n{file_text}\n```"));
        }
    }

    if !context.auto_load_files.is_empty() {
        if let Some(environment) = context
            .dependencies
            .get::<crate::bundles::EnvironmentHandle>()
        {
            let files_to_load = context.auto_load_files.clone();
            for path in &files_to_load {
                match environment.provider().read_text(path).await {
                    Ok(file_text) => loaded.push(format!("### `{path}`\n\n```\n{file_text}\n```")),
                    Err(error) => loaded.push(format!("### `{path}`\n\n[Failed to load: {error}]")),
                }
            }
            context.auto_load_files.clear();
        }
    }

    if loaded.is_empty() {
        return messages;
    }
    append_user_text_to_last_request(
        &mut messages,
        format!(
            "<auto-loaded-files>\n\n{}\n\n</auto-loaded-files>",
            loaded.join("\n\n")
        ),
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
    context: &mut starweaver_context::AgentContext,
    mut messages: Vec<ModelMessage>,
) -> Vec<ModelMessage> {
    let mut rendered = Vec::new();
    if let Some(bus_messages) = state
        .metadata
        .get(BUS_MESSAGE_METADATA)
        .and_then(Value::as_array)
    {
        for message in bus_messages {
            rendered.push(message.to_string());
        }
    }

    let pending = context.consume_messages();
    for message in pending {
        let rendered_text = message.render_text();
        context.publish_event(starweaver_context::AgentEvent::new(
            "message_received",
            serde_json::json!({
                "id": message.id,
                "source": message.source,
                "target": message.target,
                "content_text": message.content_text(),
            }),
        ));
        if message.source == "user" || message.topic == "steering" {
            context.publish_event(starweaver_context::AgentEvent::new(
                "steering_received",
                serde_json::json!({"id": message.id, "text": rendered_text}),
            ));
            context.steering_messages.push(rendered_text.clone());
        }
        rendered.push(format!(
            "<bus-message source=\"{}\">\n{}\n</bus-message>",
            escape_xml_attr(&message.source),
            rendered_text
        ));
    }

    if rendered.is_empty() {
        return messages;
    }
    append_user_text_to_last_request(&mut messages, rendered.join("\n\n"), "bus_message");
    messages
}

fn append_user_text_to_last_request(messages: &mut Vec<ModelMessage>, text: String, source: &str) {
    let mut metadata = Map::new();
    metadata.insert("starweaver_filter_source".to_string(), json!(source));
    let part = ModelRequestPart::UserPrompt {
        content: vec![ContentPart::Text { text }],
        name: None,
        metadata,
    };
    for message in messages.iter_mut().rev() {
        if let ModelMessage::Request(request) = message {
            request.parts.push(part);
            return;
        }
    }
    messages.push(ModelMessage::Request(ModelRequest {
        parts: vec![part],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Map::new(),
    }));
}

fn escape_xml_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn annotate_context_parts(parts: &mut [ModelRequestPart], context_type: &str, origin: &str) {
    for part in parts {
        if let ModelRequestPart::UserPrompt { metadata, .. } = part {
            metadata.insert(CONTEXT_TYPE_METADATA.to_string(), json!(context_type));
            metadata.insert(CONTEXT_ORIGIN_METADATA.to_string(), json!(origin));
        }
    }
}

fn context_metadata(context_type: &str, origin: &str) -> Map<String, Value> {
    Map::from_iter([
        (CONTEXT_TYPE_METADATA.to_string(), json!(context_type)),
        (CONTEXT_ORIGIN_METADATA.to_string(), json!(origin)),
    ])
}

fn context_origin_for_type(context_type: &str) -> Option<String> {
    match context_type {
        "environment" => Some(CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT.to_string()),
        "runtime" => Some(CONTEXT_ORIGIN_RUNTIME_CONTEXT.to_string()),
        "handoff" => Some(CONTEXT_ORIGIN_HANDOFF.to_string()),
        _ => None,
    }
}

fn instruction_origin(instruction_type: &str) -> String {
    instruction_type.to_string()
}

fn instruction_is_dynamic(origin: &str) -> bool {
    matches!(origin, INSTRUCTION_ORIGIN_DYNAMIC_INSTRUCTION)
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
