//! Shared helpers for keeping model-visible tool outputs within hard limits.

use serde_json::Value;
use starweaver_context::HostCapabilities;
use starweaver_environment::{DynEnvironmentProvider, EnvironmentProvider};
use starweaver_tools::ToolContext;
use uuid::Uuid;

use crate::bundles::environment::EnvironmentHandle;

/// Default hard limit for model-visible structured tool output previews.
pub const DEFAULT_TOOL_OUTPUT_TRUNCATE_LIMIT: usize = 20_000;

/// Serialize a JSON tool result for size checks and scratch spill files.
#[must_use]
pub fn dump_tool_output(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

/// Return the serialized character count of a JSON tool result.
#[must_use]
pub fn tool_output_size(value: &Value) -> usize {
    dump_tool_output(value).chars().count()
}

/// Return the active environment provider from a tool context, when available.
#[must_use]
pub fn environment_provider_from_context(context: &ToolContext) -> Option<DynEnvironmentProvider> {
    context
        .dependency::<HostCapabilities>()
        .and_then(|capabilities| capabilities.get::<EnvironmentHandle>())
        .or_else(|| context.dependency::<EnvironmentHandle>())
        .map(|handle| handle.provider())
}

/// Build consistent guidance for oversized tool outputs.
#[must_use]
pub fn output_too_large_message(size: usize, output_path: Option<&str>, noun: &str) -> String {
    if output_path.is_some() {
        format!(
            "Output too large ({size} chars). Full {noun} saved to `output_file_path`. Use `view` to inspect it."
        )
    } else {
        format!(
            "Output too large ({size} chars). Failed to save full {noun}; showing a bounded preview."
        )
    }
}

/// Append a guidance sentence to an existing hint or note.
#[must_use]
pub fn append_guidance(value: Option<&str>, guidance: &str) -> String {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return guidance.to_string();
    };
    if value.ends_with(['.', '!', '?']) {
        format!("{value} {guidance}")
    } else {
        format!("{value}. {guidance}")
    }
}

/// Write large tool output to the environment scratch area when available.
pub async fn write_scratch_output(
    provider: Option<&dyn EnvironmentProvider>,
    prefix: &str,
    extension: &str,
    content: &[u8],
) -> Option<String> {
    let provider = provider?;
    let filename = format!(
        "{prefix}-{}.{}",
        Uuid::new_v4().simple(),
        extension.trim_start_matches('.')
    );
    provider.write_scratch_file(&filename, content).await.ok()
}

/// Truncate text to a hard character budget including the suffix.
#[must_use]
pub fn truncate_text_to_budget(text: &str, max_chars: usize, suffix: &str) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    let suffix_len = suffix.chars().count();
    if max_chars <= suffix_len {
        return suffix.chars().take(max_chars).collect();
    }
    let keep = max_chars - suffix_len;
    format!("{}{}", text.chars().take(keep).collect::<String>(), suffix)
}

/// Truncate text to a hard character budget while preserving both ends.
#[must_use]
pub fn truncate_text_ends_to_budget(text: &str, max_chars: usize, infix: &str) -> String {
    let text_len = text.chars().count();
    if text_len <= max_chars {
        return text.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    if max_chars == 1 {
        return text.chars().take(1).collect();
    }

    let infix_len = infix.chars().count();
    let effective_infix = if infix_len <= max_chars - 2 {
        infix
    } else if max_chars > 2 {
        "…"
    } else {
        ""
    };
    let keep = max_chars - effective_infix.chars().count();
    let head_len = keep.saturating_add(1) / 2;
    let tail_len = keep / 2;
    let head = text.chars().take(head_len).collect::<String>();
    let tail = text
        .chars()
        .rev()
        .take(tail_len)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{head}{effective_infix}{tail}")
}

/// Shrink selected string fields until the serialized object fits the limit.
#[must_use]
pub fn fit_text_fields_to_limit(
    result: Value,
    text_fields: &[&str],
    limit: usize,
    suffix: &str,
) -> Value {
    fit_text_fields_to_limit_with(result, text_fields, limit, suffix, truncate_text_to_budget)
}

/// Shrink selected string fields while preserving both ends of each field.
#[must_use]
pub fn fit_text_fields_to_limit_preserving_ends(
    result: Value,
    text_fields: &[&str],
    limit: usize,
    infix: &str,
) -> Value {
    fit_text_fields_to_limit_with(
        result,
        text_fields,
        limit,
        infix,
        truncate_text_ends_to_budget,
    )
}

fn fit_text_fields_to_limit_with(
    result: Value,
    text_fields: &[&str],
    limit: usize,
    marker: &str,
    truncate: fn(&str, usize, &str) -> String,
) -> Value {
    if tool_output_size(&result) <= limit {
        return result;
    }
    let Value::Object(mut preview) = result else {
        return result;
    };

    let originals = text_fields
        .iter()
        .filter_map(|field| {
            preview
                .get(*field)
                .and_then(Value::as_str)
                .map(|value| ((*field).to_string(), value.to_string()))
        })
        .collect::<Vec<_>>();
    if originals.is_empty() {
        return Value::Object(preview);
    }

    for (field, _) in &originals {
        preview.insert(field.clone(), Value::String(String::new()));
    }
    let base_size = tool_output_size(&Value::Object(preview.clone()));
    let mut per_field = limit.saturating_sub(base_size).saturating_sub(1) / originals.len();

    loop {
        for (field, value) in &originals {
            preview.insert(
                field.clone(),
                Value::String(truncate(value, per_field, marker)),
            );
        }
        let candidate = Value::Object(preview.clone());
        if tool_output_size(&candidate) <= limit || per_field == 0 {
            break;
        }
        per_field = per_field.saturating_mul(8) / 10;
    }

    let candidate = Value::Object(preview.clone());
    if tool_output_size(&candidate) <= limit {
        return candidate;
    }

    let marker = marker.trim_matches('\n');
    for (field, _) in &originals {
        preview.insert(field.clone(), Value::String(marker.to_string()));
    }
    Value::Object(preview)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_text_ends_to_budget_keeps_head_and_tail_within_budget() {
        let truncated = truncate_text_ends_to_budget("abcdefghijklmnopqrstuvwxyz", 15, "[cut]");

        assert_eq!(truncated, "abcde[cut]vwxyz");
        assert_eq!(truncated.chars().count(), 15);
    }

    #[test]
    fn truncate_text_ends_to_budget_respects_utf8_character_boundaries() {
        let truncated = truncate_text_ends_to_budget("甲乙丙丁戊己庚辛", 6, "省略");

        assert_eq!(truncated, "甲乙省略庚辛");
        assert_eq!(truncated.chars().count(), 6);
    }

    #[test]
    fn truncate_text_ends_to_budget_preserves_ends_when_marker_does_not_fit() {
        assert_eq!(
            truncate_text_ends_to_budget("abcdef", 3, "[marker too long]"),
            "a…f"
        );
        assert_eq!(
            truncate_text_ends_to_budget("abcdef", 2, "[marker too long]"),
            "af"
        );
        assert_eq!(
            truncate_text_ends_to_budget("abcdef", 1, "[marker too long]"),
            "a"
        );
        assert_eq!(
            truncate_text_ends_to_budget("abcdef", 0, "[marker too long]"),
            ""
        );
    }
}
