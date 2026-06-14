//! Output truncation and temporary file helpers for filesystem search tools.

use serde_json::Value;
use starweaver_context::ToolConfig;
use starweaver_environment::{EnvironmentProvider, FileGrepMatch};
use starweaver_tools::{ToolError, ToolResult};
use uuid::Uuid;

use crate::bundles::helpers::tool_execution_error;

use super::truncate_chars;

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
        "note": output_path.as_ref().map_or_else(
            || format!("Output too large ({} chars). Failed to save temp file; showing truncated preview.", serialized.len()),
            |_| format!("Output too large ({} chars). Full results saved to output_file_path.", serialized.len()),
        ),
    });
    if let Some(path) = output_path {
        preview["output_file_path"] = Value::String(path);
    }
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

pub(super) async fn guard_grep_output(
    provider: &dyn EnvironmentProvider,
    tool_config: &ToolConfig,
    root: &str,
    pattern: &str,
    matches: Vec<FileGrepMatch>,
) -> Result<ToolResult, ToolError> {
    let original = serde_json::json!({
        "root": root,
        "pattern": pattern,
        "matches": matches,
    });
    let serialized =
        serde_json::to_string(&original).map_err(|error| tool_execution_error("grep", error))?;
    if serialized.len() <= tool_config.grep_truncation_threshold {
        return Ok(ToolResult::new(original));
    }

    let simplified_matches = original["matches"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|value| simplify_grep_match(value, tool_config.grep_truncated_line_max))
        .collect::<Vec<_>>();
    let simplified = serde_json::json!({
        "root": root,
        "pattern": pattern,
        "matches": simplified_matches,
        "system": "Context dropped to reduce output size. Use view to read specific files.",
    });
    let simplified_serialized =
        serde_json::to_string(&simplified).map_err(|error| tool_execution_error("grep", error))?;
    let output_truncate_limit = tool_config.filesystem_output_truncate_limit;
    if simplified_serialized.len() <= output_truncate_limit {
        return Ok(ToolResult::new(simplified));
    }

    let output_path = write_tool_output(provider, "grep", "json", &simplified_serialized).await;
    let matches = simplified["matches"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut preview = serde_json::json!({
        "root": root,
        "pattern": pattern,
        "matches": [],
        "system": output_path.as_ref().map_or_else(
            || format!("Output too large ({} chars). Failed to save temp file; showing truncated preview.", simplified_serialized.len()),
            |_| format!("Output too large ({} chars). Full results saved to temp file. Use view to read it.", simplified_serialized.len()),
        ),
        "total_matches": matches.len(),
        "showing": 0,
    });
    if let Some(path) = output_path {
        preview["output_file_path"] = Value::String(path);
    }
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
