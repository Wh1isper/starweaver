//! Output truncation and temporary file helpers for filesystem search tools.

use serde_json::Value;
use starweaver_context::ToolConfig;
use starweaver_environment::{EnvironmentProvider, FileGrepMatch};
use starweaver_tools::{ToolError, ToolResult};
use uuid::Uuid;

use crate::bundles::helpers::tool_execution_error;

use super::{add_skill_document_reminder, is_skill_document, truncate_chars};

pub(super) async fn guard_glob_output(
    provider: &dyn EnvironmentProvider,
    tool_config: &ToolConfig,
    result: Value,
) -> Result<ToolResult, ToolError> {
    let serialized =
        serde_json::to_string(&result).map_err(|error| tool_execution_error("glob", error))?;
    let output_truncate_limit = tool_config.filesystem_output_truncate_limit;
    if serialized.len() <= output_truncate_limit {
        return Ok(ToolResult::new(result));
    }
    let output_path = write_tool_output(provider, "glob", "json", &serialized).await;
    let matches = result
        .get("matches")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let total = matches.len();
    let mut preview = serde_json::json!({
        "matches": [],
        "truncated": true,
        "total_matches": total,
        "showing": 0,
    });
    ensure_preview_fits("glob", &preview, output_truncate_limit)?;
    if let Some(reminder) = result.get("system-reminder").cloned() {
        insert_required(
            &mut preview,
            "glob",
            "system-reminder",
            reminder,
            output_truncate_limit,
        )?;
    }
    let note = output_path.as_ref().map_or_else(
        || {
            format!(
                "Output too large ({} chars). Failed to save temp file; showing truncated preview.",
                serialized.len()
            )
        },
        |_| {
            format!(
                "Output too large ({} chars). Full results saved to output_file_path.",
                serialized.len()
            )
        },
    );
    if let Some(path) = output_path {
        insert_if_fits(
            &mut preview,
            "output_file_path",
            Value::String(path),
            output_truncate_limit,
        );
    }
    insert_if_fits(
        &mut preview,
        "note",
        Value::String(note),
        output_truncate_limit,
    );
    let mut kept = Vec::new();
    for entry in matches {
        kept.push(entry);
        preview["matches"] = Value::Array(kept.clone());
        preview["showing"] = serde_json::json!(kept.len());
        if serde_json::to_string(&preview).map_or(true, |value| value.len() > output_truncate_limit)
        {
            kept.pop();
            preview["matches"] = Value::Array(kept.clone());
            preview["showing"] = serde_json::json!(kept.len());
            break;
        }
    }
    Ok(ToolResult::new(preview))
}

pub(super) async fn guard_ls_output(
    provider: &dyn EnvironmentProvider,
    tool_config: &ToolConfig,
    result: Value,
) -> Result<ToolResult, ToolError> {
    let serialized =
        serde_json::to_string(&result).map_err(|error| tool_execution_error("ls", error))?;
    let output_truncate_limit = tool_config.filesystem_output_truncate_limit;
    if serialized.len() <= output_truncate_limit {
        return Ok(ToolResult::new(result));
    }
    let output_path = write_tool_output(provider, "ls", "json", &serialized).await;
    let entries = result
        .get("entries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let total = result
        .get("total_entries")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(entries.len());
    let mut preview = serde_json::json!({
        "entries": [],
        "truncated": true,
        "total_entries": total,
        "showing": 0,
    });
    ensure_preview_fits("ls", &preview, output_truncate_limit)?;
    if let Some(reminder) = result.get("system-reminder").cloned() {
        insert_required(
            &mut preview,
            "ls",
            "system-reminder",
            reminder,
            output_truncate_limit,
        )?;
    }
    if let Some(path) = result.get("path").cloned() {
        insert_if_fits(&mut preview, "path", path, output_truncate_limit);
    }
    let note = output_path.as_ref().map_or_else(
        || {
            format!(
                "Output too large ({} chars). Failed to save temp file; showing truncated preview.",
                serialized.len()
            )
        },
        |_| {
            format!(
                "Output too large ({} chars). Full results saved to output_file_path.",
                serialized.len()
            )
        },
    );
    if let Some(path) = output_path {
        insert_if_fits(
            &mut preview,
            "output_file_path",
            Value::String(path),
            output_truncate_limit,
        );
    }
    insert_if_fits(
        &mut preview,
        "note",
        Value::String(note),
        output_truncate_limit,
    );
    let mut kept = Vec::new();
    for entry in entries {
        kept.push(entry);
        preview["entries"] = Value::Array(kept.clone());
        preview["showing"] = serde_json::json!(kept.len());
        if serde_json::to_string(&preview).map_or(true, |value| value.len() > output_truncate_limit)
        {
            kept.pop();
            preview["entries"] = Value::Array(kept.clone());
            preview["showing"] = serde_json::json!(kept.len());
            break;
        }
    }
    Ok(ToolResult::new(preview))
}

pub(super) async fn guard_grep_output(
    provider: &dyn EnvironmentProvider,
    tool_config: &ToolConfig,
    root: &str,
    pattern: &str,
    matches: Vec<FileGrepMatch>,
) -> Result<ToolResult, ToolError> {
    let contains_skill_document = matches.iter().any(|entry| is_skill_document(&entry.path));
    let mut original = serde_json::json!({
        "root": root,
        "pattern": pattern,
        "matches": matches,
    });
    if contains_skill_document {
        add_skill_document_reminder(&mut original);
    }
    let serialized =
        serde_json::to_string(&original).map_err(|error| tool_execution_error("grep", error))?;
    let output_truncate_limit = tool_config.filesystem_output_truncate_limit;
    if serialized.len()
        <= tool_config
            .grep_truncation_threshold
            .min(output_truncate_limit)
    {
        return Ok(ToolResult::new(original));
    }

    let simplified_matches = original["matches"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|value| simplify_grep_match(value, tool_config.grep_truncated_line_max))
        .collect::<Vec<_>>();
    let mut simplified = serde_json::json!({
        "root": root,
        "pattern": pattern,
        "matches": simplified_matches,
        "system": "Context dropped to reduce output size. Use view to read specific files.",
    });
    if contains_skill_document {
        add_skill_document_reminder(&mut simplified);
    }
    let simplified_serialized =
        serde_json::to_string(&simplified).map_err(|error| tool_execution_error("grep", error))?;
    if simplified_serialized.len() <= output_truncate_limit {
        return Ok(ToolResult::new(simplified));
    }

    let output_path = write_tool_output(provider, "grep", "json", &simplified_serialized).await;
    let matches = simplified["matches"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let reminder = simplified.get("system-reminder").cloned();
    Ok(ToolResult::new(build_grep_preview(
        root,
        pattern,
        matches,
        reminder,
        output_path,
        simplified_serialized.len(),
        output_truncate_limit,
    )?))
}

fn build_grep_preview(
    root: &str,
    pattern: &str,
    matches: Vec<Value>,
    reminder: Option<Value>,
    output_path: Option<String>,
    serialized_len: usize,
    limit: usize,
) -> Result<Value, ToolError> {
    let mut preview = serde_json::json!({
        "matches": [],
        "total_matches": matches.len(),
        "showing": 0,
    });
    ensure_preview_fits("grep", &preview, limit)?;
    if let Some(reminder) = reminder {
        insert_required(&mut preview, "grep", "system-reminder", reminder, limit)?;
    }
    insert_if_fits(&mut preview, "root", Value::String(root.to_string()), limit);
    insert_if_fits(
        &mut preview,
        "pattern",
        Value::String(pattern.to_string()),
        limit,
    );
    let system = output_path.as_ref().map_or_else(
        || format!("Output too large ({serialized_len} chars). Failed to save temp file; showing truncated preview."),
        |_| format!("Output too large ({serialized_len} chars). Full results saved to temp file. Use view to read it."),
    );
    if let Some(path) = output_path {
        insert_if_fits(&mut preview, "output_file_path", Value::String(path), limit);
    }
    insert_if_fits(&mut preview, "system", Value::String(system), limit);
    let mut kept = Vec::new();
    for entry in matches {
        kept.push(entry);
        preview["matches"] = Value::Array(kept.clone());
        preview["showing"] = serde_json::json!(kept.len());
        if serde_json::to_string(&preview).map_or(true, |value| value.len() > limit) {
            kept.pop();
            preview["matches"] = Value::Array(kept.clone());
            preview["showing"] = serde_json::json!(kept.len());
            break;
        }
    }
    Ok(preview)
}

fn ensure_preview_fits(tool: &str, preview: &Value, limit: usize) -> Result<(), ToolError> {
    let required = serde_json::to_string(preview)
        .map_err(|error| tool_execution_error(tool, error))?
        .len();
    if required <= limit {
        return Ok(());
    }
    Err(tool_execution_error(
        tool,
        format!(
            "filesystem_output_truncate_limit ({limit}) is too small for the minimum {tool} response ({required} bytes)"
        ),
    ))
}

fn insert_required(
    preview: &mut Value,
    tool: &str,
    key: &str,
    value: Value,
    limit: usize,
) -> Result<(), ToolError> {
    preview[key] = value;
    ensure_preview_fits(tool, preview, limit)
}

fn insert_if_fits(preview: &mut Value, key: &str, value: Value, limit: usize) {
    let mut candidate = preview.clone();
    candidate[key] = value;
    if serde_json::to_string(&candidate).is_ok_and(|serialized| serialized.len() <= limit) {
        *preview = candidate;
    }
}

async fn write_tool_output(
    provider: &dyn EnvironmentProvider,
    prefix: &str,
    extension: &str,
    content: &str,
) -> Option<String> {
    let filename = format!("{prefix}-{}.{}", Uuid::new_v4().simple(), extension);
    provider
        .write_tmp_file(&filename, content.as_bytes())
        .await
        .ok()
}

fn simplify_grep_match(value: &Value, truncated_line_max: usize) -> Value {
    let matching_line = value
        .get("matching_line")
        .and_then(Value::as_str)
        .unwrap_or_default();
    serde_json::json!({
        "path": value.get("path").cloned().unwrap_or(Value::Null),
        "line_number": value.get("line_number").cloned().unwrap_or(Value::Null),
        "matching_line": truncate_chars(matching_line, truncated_line_max),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grep_preview_rejects_a_limit_that_cannot_preserve_the_skill_reminder() {
        let result = build_grep_preview(
            ".",
            "needle",
            Vec::new(),
            Some(Value::String(
                super::super::SKILL_DOCUMENT_REMINDER.to_string(),
            )),
            None,
            1_000,
            100,
        );

        assert!(matches!(
            result,
            Err(error)
                if error
                    .to_string()
                    .contains("filesystem_output_truncate_limit (100) is too small")
        ));
    }
}
