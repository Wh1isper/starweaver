//! Filesystem text mutation tools.

use starweaver_environment::EnvironmentError;
use starweaver_tools::{ToolContext, ToolError, ToolResult};

use super::{text::ensure_file_missing, tool_execution_error, EditArgs, MultiEditArgs, WriteArgs};
use crate::bundles::{
    environment::handle::environment_provider,
    helpers::{tool_invalid_arguments, tool_model_retry},
};

pub(super) async fn write_text(
    tool_context: ToolContext,
    arguments: WriteArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&tool_context, "write")?;
    let next_content = match arguments.mode.as_deref() {
        Some("a") => match provider.read_text(&arguments.file_path).await {
            Ok(existing) => format!("{existing}{}", arguments.content),
            Err(EnvironmentError::NotFound(_)) => arguments.content,
            Err(error) => return Err(tool_execution_error("write", error)),
        },
        Some("w") | None => arguments.content,
        Some(mode) => {
            return Err(tool_invalid_arguments(
                "write",
                format!("unsupported write mode {mode:?}. Use mode \"w\" to overwrite or \"a\" to append."),
            ));
        }
    };
    provider
        .write_text(&arguments.file_path, &next_content)
        .await
        .map_err(|error| tool_execution_error("write", error))?;
    Ok(ToolResult::new(
        serde_json::json!({"file_path": arguments.file_path, "written": true}),
    ))
}

pub(super) async fn edit_text(
    tool_context: ToolContext,
    arguments: EditArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&tool_context, "edit")?;
    if arguments.old_string.is_empty() {
        ensure_file_missing(provider.as_ref(), "edit", &arguments.file_path).await?;
        provider
            .write_text(&arguments.file_path, &arguments.new_string)
            .await
            .map_err(|error| tool_execution_error("edit", error))?;
        return Ok(ToolResult::new(serde_json::json!({
            "file_path": arguments.file_path,
            "created": true,
        })));
    }
    let file_content = provider
        .read_text(&arguments.file_path)
        .await
        .map_err(|error| tool_execution_error("edit", error))?;
    let updated = apply_replacement(
        "edit",
        &file_content,
        &arguments.old_string,
        &arguments.new_string,
        arguments.replace_all,
    )?;
    provider
        .write_text(&arguments.file_path, &updated)
        .await
        .map_err(|error| tool_execution_error("edit", error))?;
    Ok(ToolResult::new(serde_json::json!({
        "file_path": arguments.file_path,
        "edited": true,
    })))
}

pub(super) async fn multi_edit_text(
    tool_context: ToolContext,
    arguments: MultiEditArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&tool_context, "multi_edit")?;
    let mut edits = arguments.edits.into_iter();
    let Some(first) = edits.next() else {
        return Err(tool_execution_error(
            "multi_edit",
            "at least one edit is required",
        ));
    };
    let mut updated_content = if first.old_string.is_empty() {
        ensure_file_missing(provider.as_ref(), "multi_edit", &arguments.file_path).await?;
        first.new_string
    } else {
        let existing = provider
            .read_text(&arguments.file_path)
            .await
            .map_err(|error| tool_execution_error("multi_edit", error))?;
        apply_replacement(
            "multi_edit",
            &existing,
            &first.old_string,
            &first.new_string,
            first.replace_all,
        )?
    };
    for edit in edits {
        updated_content = apply_replacement(
            "multi_edit",
            &updated_content,
            &edit.old_string,
            &edit.new_string,
            edit.replace_all,
        )?;
    }
    provider
        .write_text(&arguments.file_path, &updated_content)
        .await
        .map_err(|error| tool_execution_error("multi_edit", error))?;
    Ok(ToolResult::new(serde_json::json!({
        "file_path": arguments.file_path,
        "edited": true,
    })))
}

fn apply_replacement(
    tool: &str,
    content: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> Result<String, ToolError> {
    if old_string.is_empty() {
        return Err(tool_invalid_arguments(
            tool,
            "old_string must be non-empty for replacement. In multi_edit, only the first edit may use an empty old_string to create a new file.",
        ));
    }
    if !content.contains(old_string) {
        return Err(tool_model_retry(
            tool,
            "old_string was not found in the current file content. Re-read the file with view, include the exact whitespace and indentation, then retry with a matching old_string.",
        ));
    }
    if replace_all {
        return Ok(content.replace(old_string, new_string));
    }
    let occurrences = content.matches(old_string).count();
    if occurrences > 1 {
        return Err(tool_model_retry(
            tool,
            "old_string appears multiple times. Add surrounding context so old_string is unique, or set replace_all=true if every occurrence should be replaced.",
        ));
    }
    Ok(content.replacen(old_string, new_string, 1))
}
