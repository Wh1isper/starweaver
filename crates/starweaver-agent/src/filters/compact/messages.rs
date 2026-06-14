use serde_json::{json, Map, Value};
use starweaver_model::{
    ContentPart, ModelMessage, ModelRequest, ModelRequestPart, ModelResponse, ToolReturnPart,
};
use starweaver_runtime::AgentRunState;

use super::super::message::{metadata_text, request_metadata_mut};
use super::constants::{
    COMPACT_KEEP_MESSAGES_METADATA, COMPACT_LIMIT_PROMPT, MAX_COMPACT_INSTRUCTION_CHARS,
    MAX_COMPACT_REPLAY_INSTRUCTION_CHARS, PROJECT_GUIDANCE_TAG, USER_RULES_TAG,
};

pub(super) fn build_cache_friendly_compacted_messages(
    state: &AgentRunState,
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
        context_restored_request(state, messages),
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
    trim_instruction_text(text, MAX_COMPACT_REPLAY_INSTRUCTION_CHARS)
}

fn trim_instruction_text_for_compact(text: &str) -> Option<String> {
    trim_instruction_text(text, MAX_COMPACT_INSTRUCTION_CHARS)
}

fn trim_instruction_text(text: &str, max_chars: usize) -> Option<String> {
    strip_injected_context_text(text).map(|text| truncate_compact_chars(&text, max_chars))
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
    messages: &[ModelMessage],
    keep: usize,
) -> Vec<ModelMessage> {
    if keep == 0 || messages.len() <= keep {
        return messages.to_vec();
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
            .filter_map(|message| trim_message_for_compact(message.clone())),
    );

    if !has_context_restored_marker(&compacted) {
        compacted.push(context_restored_request(state, messages));
    }
    request_metadata_mut(&mut compacted).insert("starweaver_compacted".to_string(), json!(true));
    compacted
}

pub(super) fn trim_message_for_compact(message: ModelMessage) -> Option<ModelMessage> {
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
                            .filter_map(trim_content_for_compact)
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
                        if let Some(text) = trim_instruction_text_for_compact(&text) {
                            parts.push(ModelRequestPart::SystemPrompt { text, metadata });
                        }
                    }
                    ModelRequestPart::Instruction { text, metadata } => {
                        if let Some(text) = trim_instruction_text_for_compact(&text) {
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

fn trim_content_for_compact(content: ContentPart) -> Option<ContentPart> {
    match content {
        ContentPart::Text { text } => {
            strip_injected_context_text(&text).map(|text| ContentPart::Text { text })
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

fn strip_injected_context_text(text: &str) -> Option<String> {
    let mut cleaned = text.to_string();
    for tag in [
        "runtime-context",
        "environment-context",
        PROJECT_GUIDANCE_TAG,
        USER_RULES_TAG,
    ] {
        cleaned = strip_xml_tag_blocks(&cleaned, tag);
    }
    let cleaned = cleaned.trim().to_string();
    (!cleaned.is_empty()).then_some(cleaned)
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
    if let Some(user_content) = &mut tool_return.user_content {
        if let Some(text) = truncate_compact_text(&user_content.to_string()) {
            *user_content = json!(text);
        }
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

fn latest_user_prompt_replay_part(messages: &[ModelMessage]) -> Option<ModelRequestPart> {
    messages.iter().rev().find_map(|message| {
        let ModelMessage::Request(request) = message else {
            return None;
        };
        request.parts.iter().rev().find_map(|part| {
            let ModelRequestPart::UserPrompt {
                content,
                name,
                metadata,
            } = part
            else {
                return None;
            };
            let content = content
                .iter()
                .cloned()
                .filter_map(trim_content_for_compact)
                .collect::<Vec<_>>();
            (!content.is_empty()).then(|| ModelRequestPart::UserPrompt {
                content,
                name: name.clone(),
                metadata: metadata.clone(),
            })
        })
    })
}

fn context_restored_request(state: &AgentRunState, messages: &[ModelMessage]) -> ModelMessage {
    let mut parts = Vec::new();
    if let Some(original) = metadata_text(&state.metadata, "starweaver_original_request") {
        parts.push(ModelRequestPart::UserPrompt {
            content: vec![ContentPart::Text {
                text: "<original-request>Below is the user's original request from the start of the conversation:</original-request>".to_string(),
            }],
            name: None,
            metadata: Map::new(),
        });
        parts.push(ModelRequestPart::UserPrompt {
            content: vec![ContentPart::Text { text: original }],
            name: None,
            metadata: Map::new(),
        });
    }
    if let Some(current_request) = latest_user_prompt_replay_part(messages) {
        parts.push(ModelRequestPart::UserPrompt {
            content: vec![ContentPart::Text {
                text: "<current-request>Below is the user's latest request that triggered compaction:</current-request>".to_string(),
            }],
            name: None,
            metadata: Map::new(),
        });
        parts.push(current_request);
    }
    if let Some(steering) = metadata_text(&state.metadata, "starweaver_user_steering") {
        parts.push(ModelRequestPart::UserPrompt {
            content: vec![ContentPart::Text {
                text: format!("<user-steering>Below are messages the user sent during your previous work session:</user-steering>\n{steering}"),
            }],
            name: None,
            metadata: Map::new(),
        });
    }
    parts.push(ModelRequestPart::UserPrompt {
        content: vec![ContentPart::Text {
            text: "<context-restored>Context was compacted from a long conversation. The summary above is the most authoritative source for current state. Synthesize the summary, original request, and any user steering messages to resume work. Do NOT repeat questions, confirmations, or actions documented in the summary. If the summary records a user decision, respect it without re-asking.</context-restored>".to_string(),
        }],
        name: None,
        metadata: Map::new(),
    });
    ModelMessage::Request(ModelRequest {
        parts,
        timestamp: None,
        instructions: None,
        run_id: Some(state.run_id.clone()),
        conversation_id: Some(state.conversation_id.clone()),
        metadata: Map::from_iter([("keep".to_string(), json!("compact"))]),
    })
}
