use serde_json::{Map, Value, json};
use starweaver_context::AgentContext;
use starweaver_model::{
    ContentPart, ModelMessage, ModelRequest, ModelRequestPart, ModelResponse, ToolReturnPart,
};
use starweaver_runtime::{AgentRunState, heal_openai_item_reference_history};

use super::super::message::{
    build_restored_request_parts, metadata_content_parts, metadata_string_array, metadata_text,
    request_metadata_mut,
};
use super::constants::{
    COMPACT_KEEP_MESSAGES_METADATA, COMPACT_LIMIT_PROMPT, MAX_COMPACT_INSTRUCTION_CHARS,
    MAX_COMPACT_REPLAY_INSTRUCTION_CHARS, PROJECT_GUIDANCE_TAG, USER_RULES_TAG,
};

pub(super) fn build_cache_friendly_compacted_messages(
    state: &AgentRunState,
    context: &AgentContext,
    messages: &[ModelMessage],
    summary: &str,
) -> Vec<ModelMessage> {
    let mut summary_response = ModelResponse::text(truncate_compact_chars(
        summary,
        MAX_COMPACT_REPLAY_INSTRUCTION_CHARS,
    ));
    summary_response
        .metadata
        .insert("keep".to_string(), json!("compact"));
    let mut request_parts = compact_replay_instruction_parts(messages);
    if request_parts.is_empty() {
        request_parts.push(ModelRequestPart::SystemPrompt {
            text: "Placeholder system prompt".to_string(),
            metadata: Map::new(),
        });
    }
    request_parts.push(ModelRequestPart::UserPrompt {
        content: vec![ContentPart::Text {
            text: COMPACT_LIMIT_PROMPT.to_string(),
        }],
        name: None,
        metadata: Map::new(),
    });
    vec![
        ModelMessage::Request(ModelRequest {
            parts: request_parts,
            timestamp: None,
            instructions: None,
            run_id: Some(state.run_id.clone()),
            conversation_id: Some(state.conversation_id.clone()),
            metadata: Map::new(),
        }),
        ModelMessage::Response(summary_response),
        context_restored_request(state, context),
    ]
}

pub(super) fn instruction_parts(messages: &[ModelMessage]) -> Vec<ModelRequestPart> {
    messages
        .iter()
        .flat_map(|message| match message {
            ModelMessage::Request(request) => request
                .parts
                .iter()
                .filter(|part| {
                    matches!(
                        part,
                        ModelRequestPart::SystemPrompt { .. }
                            | ModelRequestPart::Instruction { .. }
                    )
                })
                .cloned()
                .collect::<Vec<_>>(),
            ModelMessage::Response(_) => Vec::new(),
        })
        .collect()
}

fn compact_replay_instruction_parts(messages: &[ModelMessage]) -> Vec<ModelRequestPart> {
    dedupe_instruction_parts(
        instruction_parts(messages)
            .into_iter()
            .filter_map(trim_instruction_part_for_replay)
            .collect(),
    )
}

fn trim_instruction_part_for_replay(part: ModelRequestPart) -> Option<ModelRequestPart> {
    match part {
        ModelRequestPart::SystemPrompt { text, metadata } => {
            trim_instruction_text_for_replay(&text)
                .map(|text| ModelRequestPart::SystemPrompt { text, metadata })
        }
        ModelRequestPart::Instruction { text, metadata } => trim_instruction_text_for_replay(&text)
            .map(|text| ModelRequestPart::Instruction { text, metadata }),
        _ => None,
    }
}

fn dedupe_instruction_parts(parts: Vec<ModelRequestPart>) -> Vec<ModelRequestPart> {
    let mut output = Vec::new();
    for part in parts {
        if !output.contains(&part) {
            output.push(part);
        }
    }
    output
}

fn trim_instruction_text_for_replay(text: &str) -> Option<String> {
    trim_instruction_text(text, MAX_COMPACT_REPLAY_INSTRUCTION_CHARS, &[])
}

fn trim_instruction_text_for_compact(text: &str, injected_tags: &[String]) -> Option<String> {
    trim_instruction_text(text, MAX_COMPACT_INSTRUCTION_CHARS, injected_tags)
}

fn trim_instruction_text(text: &str, max_chars: usize, injected_tags: &[String]) -> Option<String> {
    strip_injected_context_text(text, injected_tags)
        .map(|text| truncate_compact_chars(&text, max_chars))
}

fn truncate_compact_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(80);
    let head = text.chars().take(keep).collect::<String>();
    let omitted = text.chars().count().saturating_sub(keep);
    format!("{head}\n[... {omitted} chars truncated for compact replay ...]")
}

pub(super) fn manual_compact_keep(state: &AgentRunState) -> Option<usize> {
    state
        .metadata
        .get(COMPACT_KEEP_MESSAGES_METADATA)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

pub(super) fn build_trimmed_compact_messages(
    state: &AgentRunState,
    context: &AgentContext,
    messages: &[ModelMessage],
    keep: usize,
) -> Vec<ModelMessage> {
    let messages = compact_safe_messages(messages);
    if keep == 0 || messages.len() <= keep {
        return messages;
    }

    let mut compacted = Vec::new();
    for message in messages.iter().take(messages.len().saturating_sub(keep)) {
        if has_keep_tag(message) {
            compacted.push(message.clone());
        }
    }
    compacted.extend(
        messages
            .iter()
            .skip(messages.len().saturating_sub(keep))
            .filter_map(|message| {
                trim_message_for_compact(message.clone(), &context.injected_context_tags)
            }),
    );

    if !has_context_restored_marker(&compacted) {
        compacted.push(context_restored_request(state, context));
    }
    request_metadata_mut(&mut compacted).insert("starweaver_compacted".to_string(), json!(true));
    compacted
}

pub(super) fn compact_safe_messages(messages: &[ModelMessage]) -> Vec<ModelMessage> {
    let mut messages = messages.to_vec();
    heal_openai_item_reference_history(&mut messages);
    messages
}

pub(super) fn trim_message_for_compact(
    message: ModelMessage,
    injected_tags: &[String],
) -> Option<ModelMessage> {
    match message {
        ModelMessage::Response(_) => Some(message),
        ModelMessage::Request(mut request) => {
            let mut parts = Vec::new();
            for part in request.parts {
                match part {
                    ModelRequestPart::ToolReturn(mut tool_return) => {
                        trim_tool_return_for_compact(&mut tool_return);
                        parts.push(ModelRequestPart::ToolReturn(tool_return));
                    }
                    ModelRequestPart::UserPrompt {
                        content,
                        name,
                        metadata,
                    } => {
                        let content = content
                            .into_iter()
                            .filter_map(|content| trim_content_for_compact(content, injected_tags))
                            .collect::<Vec<_>>();
                        if !content.is_empty() {
                            parts.push(ModelRequestPart::UserPrompt {
                                content,
                                name,
                                metadata,
                            });
                        }
                    }
                    ModelRequestPart::SystemPrompt { text, metadata } => {
                        if let Some(text) = trim_instruction_text_for_compact(&text, injected_tags)
                        {
                            parts.push(ModelRequestPart::SystemPrompt { text, metadata });
                        }
                    }
                    ModelRequestPart::Instruction { text, metadata } => {
                        if let Some(text) = trim_instruction_text_for_compact(&text, injected_tags)
                        {
                            parts.push(ModelRequestPart::Instruction { text, metadata });
                        }
                    }
                    other @ ModelRequestPart::RetryPrompt { .. } => parts.push(other),
                }
            }
            if parts.is_empty() {
                return None;
            }
            request.parts = parts;
            Some(ModelMessage::Request(request))
        }
    }
}

fn trim_content_for_compact(content: ContentPart, injected_tags: &[String]) -> Option<ContentPart> {
    match content {
        ContentPart::CachePoint { .. } => None,
        ContentPart::Text { text } => {
            strip_injected_context_text(&text, injected_tags).map(|text| ContentPart::Text { text })
        }
        ContentPart::ImageUrl { url } => Some(ContentPart::Text {
            text: format!("[image: {url}]"),
        }),
        ContentPart::FileUrl { url, media_type } => Some(ContentPart::Text {
            text: format!("[{media_type}: {url}]"),
        }),
        ContentPart::Binary { media_type, .. }
        | ContentPart::DataUrl { media_type, .. }
        | ContentPart::ResourceRef { media_type, .. } => Some(ContentPart::Text {
            text: format!("[{media_type} content removed]"),
        }),
    }
}

fn strip_injected_context_text(text: &str, injected_tags: &[String]) -> Option<String> {
    let mut cleaned = text.to_string();
    for tag in default_and_context_tags(injected_tags) {
        cleaned = strip_xml_tag_blocks(&cleaned, &tag);
    }
    let cleaned = cleaned.trim().to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

fn default_and_context_tags(injected_tags: &[String]) -> Vec<String> {
    let mut tags = vec![
        "runtime-context".to_string(),
        "environment-context".to_string(),
        PROJECT_GUIDANCE_TAG.to_string(),
        USER_RULES_TAG.to_string(),
    ];
    for tag in injected_tags {
        if !tag.trim().is_empty() && !tags.iter().any(|existing| existing == tag) {
            tags.push(tag.clone());
        }
    }
    tags
}

fn strip_xml_tag_blocks(text: &str, tag: &str) -> String {
    let mut remaining = text;
    let mut output = String::new();
    let open_prefix = format!("<{tag}");
    let close_tag = format!("</{tag}>");
    while let Some(start) = remaining.find(&open_prefix) {
        output.push_str(&remaining[..start]);
        let after_start = &remaining[start..];
        let Some(open_end) = after_start.find('>') else {
            output.push_str(after_start);
            return output;
        };
        let after_open = &after_start[open_end + 1..];
        if let Some(close_start) = after_open.find(&close_tag) {
            remaining = &after_open[close_start + close_tag.len()..];
        } else {
            remaining = after_open;
            break;
        }
    }
    output.push_str(remaining);
    output
}

fn trim_tool_return_for_compact(tool_return: &mut ToolReturnPart) {
    if let Some(text) = truncate_compact_text(&tool_return.content.to_string()) {
        tool_return.content = json!(text);
        tool_return
            .metadata
            .insert("starweaver_compact_trimmed".to_string(), json!(true));
    }
    if let Some(user_content) = &mut tool_return.user_content
        && let Some(text) = truncate_compact_text(&user_content.to_string())
    {
        *user_content = json!(text);
    }
}

fn truncate_compact_text(text: &str) -> Option<String> {
    const MAX: usize = 500;
    const HEAD: usize = 200;
    const TAIL: usize = 200;
    if text.chars().count() <= MAX {
        return None;
    }
    let head = text.chars().take(HEAD).collect::<String>();
    let tail = text
        .chars()
        .rev()
        .take(TAIL)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    let truncated = text.chars().count().saturating_sub(HEAD + TAIL);
    Some(format!(
        "{head}\n[... {truncated} chars truncated ...]\n{tail}"
    ))
}

fn has_keep_tag(message: &ModelMessage) -> bool {
    match message {
        ModelMessage::Request(request) => keep_tag_value(&request.metadata).is_some(),
        ModelMessage::Response(response) => keep_tag_value(&response.metadata).is_some(),
    }
}

fn keep_tag_value(metadata: &Map<String, Value>) -> Option<&str> {
    metadata
        .get("keep")
        .or_else(|| metadata.get("ya_keep"))
        .and_then(Value::as_str)
}

fn has_context_restored_marker(messages: &[ModelMessage]) -> bool {
    messages.iter().any(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().any(|part| match part {
            ModelRequestPart::UserPrompt { content, .. } => content.iter().any(|item| match item {
                ContentPart::Text { text } => text.contains("<context-restored>"),
                _ => false,
            }),
            _ => false,
        }),
        ModelMessage::Response(_) => false,
    })
}

fn context_restored_request(state: &AgentRunState, context: &AgentContext) -> ModelMessage {
    let previous_reference = context
        .previous_assistant_response_reference
        .clone()
        .filter(|reference| !reference.trim().is_empty())
        .or_else(|| {
            metadata_text(
                &state.metadata,
                "starweaver_previous_assistant_response_reference",
            )
        });
    let original_request = context
        .user_prompts
        .clone()
        .filter(|content| !content.is_empty())
        .or_else(|| metadata_content_parts(&state.metadata, "starweaver_original_request_content"))
        .or_else(|| {
            metadata_text(&state.metadata, "starweaver_original_request")
                .map(|text| vec![ContentPart::Text { text }])
        });
    let steering_messages = if context.steering_messages.is_empty() {
        metadata_string_array(&state.metadata, "starweaver_user_steering")
    } else {
        context.steering_messages.clone()
    };
    ModelMessage::Request(ModelRequest {
        parts: build_restored_request_parts(
            original_request,
            previous_reference.as_deref(),
            steering_messages,
        ),
        timestamp: None,
        instructions: None,
        run_id: Some(state.run_id.clone()),
        conversation_id: Some(state.conversation_id.clone()),
        metadata: Map::from_iter([("keep".to_string(), json!("compact"))]),
    })
}
