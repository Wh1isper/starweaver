//! Trust-boundary sanitizer for client-submitted message history.

use serde::{Deserialize, Serialize};
use starweaver_model::{ContentPart, ModelMessage, ModelRequest, ModelRequestPart};

/// Trust policy for client-submitted history.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientHistoryTrust {
    /// Treat all client history as untrusted user input.
    #[default]
    Untrusted,
    /// Allow client-supplied system/developer instructions.
    TrustedSystemPrompts,
}

/// Sanitizer configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClientHistorySanitizerConfig {
    /// Trust mode.
    #[serde(default)]
    pub trust: ClientHistoryTrust,
    /// Allowed URL schemes for file and media attachments.
    #[serde(default = "default_allowed_url_schemes")]
    pub allowed_url_schemes: Vec<String>,
    /// Whether tool call/result pairs must be internally consistent.
    #[serde(default = "default_true")]
    pub reject_dangling_tool_pairs: bool,
}

impl Default for ClientHistorySanitizerConfig {
    fn default() -> Self {
        Self {
            trust: ClientHistoryTrust::Untrusted,
            allowed_url_schemes: default_allowed_url_schemes(),
            reject_dangling_tool_pairs: true,
        }
    }
}

/// Sanitizer decision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SanitizerDecision {
    /// Decision kind.
    pub kind: String,
    /// Human-readable reason.
    pub reason: String,
    /// Message index.
    pub message_index: usize,
    /// Optional part index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_index: Option<usize>,
}

/// Sanitizer result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SanitizedClientHistory {
    /// Sanitized messages.
    pub messages: Vec<ModelMessage>,
    /// Decisions made by the sanitizer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<SanitizerDecision>,
}

/// Sanitize client-submitted model history at a host trust boundary.
#[must_use]
pub fn sanitize_client_history(
    messages: Vec<ModelMessage>,
    config: &ClientHistorySanitizerConfig,
) -> SanitizedClientHistory {
    let mut decisions = Vec::new();
    let mut sanitized = Vec::new();
    let mut open_tool_calls = std::collections::BTreeSet::new();
    for (message_index, message) in messages.into_iter().enumerate() {
        match message {
            ModelMessage::Request(request) => {
                let request = sanitize_request(
                    request,
                    message_index,
                    config,
                    &open_tool_calls,
                    &mut decisions,
                );
                sanitized.push(ModelMessage::Request(request));
            }
            ModelMessage::Response(response) => {
                for call in response.tool_calls() {
                    open_tool_calls.insert(call.id);
                }
                sanitized.push(ModelMessage::Response(response));
            }
        }
    }
    SanitizedClientHistory {
        messages: sanitized,
        decisions,
    }
}

fn sanitize_request(
    request: ModelRequest,
    message_index: usize,
    config: &ClientHistorySanitizerConfig,
    open_tool_calls: &std::collections::BTreeSet<String>,
    decisions: &mut Vec<SanitizerDecision>,
) -> ModelRequest {
    let mut parts = Vec::new();
    for (part_index, part) in request.parts.into_iter().enumerate() {
        match part {
            ModelRequestPart::SystemPrompt { text, metadata }
                if config.trust == ClientHistoryTrust::Untrusted =>
            {
                decisions.push(decision(
                    "demoted_system_prompt",
                    "client-submitted system prompt was demoted to user text",
                    message_index,
                    Some(part_index),
                ));
                parts.push(ModelRequestPart::UserPrompt {
                    content: vec![ContentPart::Text { text }],
                    name: None,
                    metadata,
                });
            }
            ModelRequestPart::Instruction { text, metadata }
                if config.trust == ClientHistoryTrust::Untrusted =>
            {
                decisions.push(decision(
                    "demoted_instruction",
                    "client-submitted instruction was demoted to user text",
                    message_index,
                    Some(part_index),
                ));
                parts.push(ModelRequestPart::UserPrompt {
                    content: vec![ContentPart::Text { text }],
                    name: None,
                    metadata,
                });
            }
            ModelRequestPart::ToolReturn(return_part)
                if config.reject_dangling_tool_pairs
                    && !open_tool_calls.contains(&return_part.tool_call_id) =>
            {
                decisions.push(decision(
                    "dropped_dangling_tool_return",
                    "tool return did not match any prior tool call",
                    message_index,
                    Some(part_index),
                ));
            }
            ModelRequestPart::UserPrompt {
                content,
                name,
                metadata,
            } => {
                let content =
                    sanitize_content_parts(content, message_index, part_index, config, decisions);
                parts.push(ModelRequestPart::UserPrompt {
                    content,
                    name,
                    metadata,
                });
            }
            other => parts.push(other),
        }
    }
    ModelRequest { parts, ..request }
}

fn sanitize_content_parts(
    content: Vec<ContentPart>,
    message_index: usize,
    part_index: usize,
    config: &ClientHistorySanitizerConfig,
    decisions: &mut Vec<SanitizerDecision>,
) -> Vec<ContentPart> {
    content
        .into_iter()
        .filter_map(|part| {
            let url = match &part {
                ContentPart::ImageUrl { url } | ContentPart::FileUrl { url, .. } => Some(url),
                ContentPart::ResourceRef { uri, .. } => Some(uri),
                ContentPart::Text { .. }
                | ContentPart::Binary { .. }
                | ContentPart::DataUrl { .. } => None,
            };
            if let Some(url) = url
                && !url_scheme_allowed(url, &config.allowed_url_schemes)
            {
                decisions.push(decision(
                    "dropped_disallowed_url",
                    "attachment URL scheme is outside the trust-boundary allowlist",
                    message_index,
                    Some(part_index),
                ));
                return None;
            }
            Some(part)
        })
        .collect()
}

fn url_scheme_allowed(url: &str, allowed: &[String]) -> bool {
    let Some((scheme, _rest)) = url.split_once(':') else {
        return false;
    };
    allowed.iter().any(|allowed| allowed == scheme)
}

fn decision(
    kind: &str,
    reason: &str,
    message_index: usize,
    part_index: Option<usize>,
) -> SanitizerDecision {
    SanitizerDecision {
        kind: kind.to_string(),
        reason: reason.to_string(),
        message_index,
        part_index,
    }
}

fn default_allowed_url_schemes() -> Vec<String> {
    vec![
        "https".to_string(),
        "data".to_string(),
        "starweaver".to_string(),
    ]
}

const fn default_true() -> bool {
    true
}
