//! Media preflight filtering.

use serde_json::{json, Value};
use starweaver_context::AgentContext;
use starweaver_model::{
    parse_data_url, ContentPart, MediaPolicy, MediaPreflight, ModelMessage, ModelRequestPart,
};
use starweaver_runtime::AgentRunState;

use crate::{
    filters::message::request_metadata_mut, media_compression::data_url as encode_data_url,
};

use super::policy::{is_image_content, is_video_content, media_policy_from_state_and_context};

pub(in crate::filters) fn media_preflight_filter(
    state: &AgentRunState,
    context: &AgentContext,
    mut messages: Vec<ModelMessage>,
) -> Vec<ModelMessage> {
    let policy = media_policy_from_state_and_context(state, context);
    let mut reports = Vec::new();
    let mut replacements = 0usize;
    for message in &mut messages {
        if let ModelMessage::Request(request) = message {
            for part in &mut request.parts {
                match part {
                    ModelRequestPart::UserPrompt { content, .. } => {
                        for item in content {
                            let result = preflight_content_part(item, &policy);
                            if result.replaced {
                                replacements += 1;
                            }
                            reports.extend(result.reports);
                        }
                    }
                    ModelRequestPart::ToolReturn(tool_return) => {
                        let result = preflight_tool_value(&mut tool_return.content, &policy);
                        if result.replaced {
                            replacements += 1;
                        }
                        reports.extend(result.reports);
                    }
                    ModelRequestPart::SystemPrompt { .. }
                    | ModelRequestPart::RetryPrompt { .. }
                    | ModelRequestPart::Instruction { .. } => {}
                }
            }
        }
    }
    let limits = enforce_media_count_limits(&mut messages, &policy);
    replacements += limits;
    if !reports.is_empty() {
        request_metadata_mut(&mut messages).insert(
            "starweaver_media_preflight".to_string(),
            Value::Array(reports),
        );
    }
    if replacements > 0 {
        request_metadata_mut(&mut messages).insert(
            "starweaver_media_replacements".to_string(),
            json!(replacements),
        );
    }
    messages
}

fn preflight_content_part(item: &mut ContentPart, policy: &MediaPolicy) -> PreflightOutcome {
    match item {
        ContentPart::Binary { data, media_type } => {
            let preflight = MediaPreflight::inspect_with_policy(data, Some(media_type), policy);
            let report = preflight_report(&preflight);
            if let Some(corrected) = preflight.corrected_media_type.clone() {
                *media_type = corrected;
            }
            if preflight.corrupt || !preflight.allowed_by_policy {
                *item = ContentPart::Text {
                    text: replacement_text(&preflight),
                };
                PreflightOutcome::replaced(report)
            } else {
                PreflightOutcome::reported(report)
            }
        }
        ContentPart::DataUrl {
            data_url,
            media_type,
        } => match parse_data_url(data_url) {
            Ok(parsed) => {
                let preflight = MediaPreflight::inspect_with_policy(
                    &parsed.data,
                    Some(parsed.media_type.as_str()),
                    policy,
                );
                let report = preflight_report(&preflight);
                if let Some(corrected) = preflight.corrected_media_type.clone() {
                    if parsed.media_type != corrected {
                        *data_url = encode_data_url(&corrected, &parsed.data);
                    }
                    *media_type = corrected;
                }
                if preflight.corrupt || !preflight.allowed_by_policy {
                    *item = ContentPart::Text {
                        text: replacement_text(&preflight),
                    };
                    PreflightOutcome::replaced(report)
                } else {
                    PreflightOutcome::reported(report)
                }
            }
            Err(error) => {
                *item = ContentPart::Text {
                    text: format!("System reminder: data URL media was removed: {error}."),
                };
                PreflightOutcome::replaced(json!({ "error": error }))
            }
        },
        ContentPart::ImageUrl { .. }
        | ContentPart::FileUrl { .. }
        | ContentPart::ResourceRef { .. }
        | ContentPart::Text { .. } => PreflightOutcome::default(),
    }
}

fn preflight_tool_value(value: &mut Value, policy: &MediaPolicy) -> PreflightOutcome {
    let mut outcome = PreflightOutcome::default();
    match value {
        Value::Array(items) => {
            for item in items {
                outcome.merge(preflight_tool_value(item, policy));
            }
        }
        Value::Object(object) => {
            if let Some(data_url) = object
                .get("data_url")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
            {
                match parse_data_url(&data_url) {
                    Ok(parsed) => {
                        let preflight = MediaPreflight::inspect_with_policy(
                            &parsed.data,
                            object
                                .get("media_type")
                                .and_then(Value::as_str)
                                .or(Some(parsed.media_type.as_str())),
                            policy,
                        );
                        outcome.reports.push(preflight_report(&preflight));
                        if let Some(media_type) = preflight.corrected_media_type.clone() {
                            if parsed.media_type != media_type {
                                object.insert(
                                    "data_url".to_string(),
                                    json!(encode_data_url(&media_type, &parsed.data)),
                                );
                            }
                            object.insert("media_type".to_string(), json!(media_type));
                        }
                        if preflight.corrupt || !preflight.allowed_by_policy {
                            *value = json!({ "type": "system_reminder", "text": replacement_text(&preflight) });
                            outcome.replaced = true;
                        }
                    }
                    Err(error) => {
                        *value = json!({ "type": "system_reminder", "text": format!("System reminder: data URL media was removed: {error}.") });
                        outcome.replaced = true;
                    }
                }
                return outcome;
            }
            for item in object.values_mut() {
                outcome.merge(preflight_tool_value(item, policy));
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    outcome
}

fn enforce_media_count_limits(messages: &mut [ModelMessage], policy: &MediaPolicy) -> usize {
    let mut image_count = 0usize;
    let mut video_count = 0usize;
    let mut replaced = 0usize;
    for message in messages.iter_mut().rev() {
        if let ModelMessage::Request(request) = message {
            for part in request.parts.iter_mut().rev() {
                if let ModelRequestPart::UserPrompt { content, .. } = part {
                    for item in content.iter_mut().rev() {
                        if is_image_content(item) {
                            image_count += 1;
                            if let Some(limit) = policy.max_images {
                                if image_count > limit {
                                    *item = ContentPart::Text {
                                        text: image_count_limit_message(limit),
                                    };
                                    replaced += 1;
                                }
                            }
                        } else if is_video_content(item) {
                            video_count += 1;
                            if let Some(limit) = policy.max_videos {
                                if video_count > limit {
                                    *item = ContentPart::Text {
                                        text: video_count_limit_message(limit),
                                    };
                                    replaced += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    replaced
}

fn image_count_limit_message(limit: usize) -> String {
    format!(
        "<system-reminder>This image content has been dropped as it exceeds the maximum allowed images (max_images={limit}).</system-reminder>"
    )
}

fn video_count_limit_message(limit: usize) -> String {
    format!(
        "<system-reminder>This video content has been dropped as it exceeds the maximum allowed videos (max_videos={limit}).</system-reminder>"
    )
}

fn preflight_report(preflight: &MediaPreflight) -> Value {
    serde_json::to_value(preflight).unwrap_or_else(|_| json!({}))
}

fn replacement_text(preflight: &MediaPreflight) -> String {
    preflight.corruption_reason.as_ref().map_or_else(
        || {
            preflight.policy_reason.as_ref().map_or_else(
                || "System reminder: media payload was removed by media preflight.".to_string(),
                |reason| {
                    format!("System reminder: media payload was removed by media policy: {reason}.")
                },
            )
        },
        |reason| {
            format!("System reminder: media payload was removed because it is corrupt: {reason}.")
        },
    )
}

#[derive(Default)]
struct PreflightOutcome {
    reports: Vec<Value>,
    replaced: bool,
}

impl PreflightOutcome {
    fn reported(report: Value) -> Self {
        Self {
            reports: vec![report],
            replaced: false,
        }
    }

    fn replaced(report: Value) -> Self {
        Self {
            reports: vec![report],
            replaced: true,
        }
    }

    fn merge(&mut self, other: Self) {
        self.reports.extend(other.reports);
        self.replaced |= other.replaced;
    }
}
