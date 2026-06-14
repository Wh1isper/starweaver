//! Shell review decision parsing.

use starweaver_model::ModelResponse;

use super::ShellReviewDecision;

pub(super) fn parse_shell_review_decision(response: &ModelResponse) -> Option<ShellReviewDecision> {
    parse_decision_value(&response.text_output())
}

fn parse_decision_value(text: &str) -> Option<ShellReviewDecision> {
    let trimmed = strip_json_fence(text.trim());
    serde_json::from_str::<ShellReviewDecision>(trimmed)
        .ok()
        .or_else(|| extract_json_object(trimmed).and_then(|json| serde_json::from_str(&json).ok()))
}

fn strip_json_fence(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    let rest = rest.strip_prefix("json").unwrap_or(rest).trim_start();
    rest.strip_suffix("```").map_or(rest, str::trim)
}

fn extract_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| text[start..=end].to_string())
}
