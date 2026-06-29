//! Text file reading and selection helpers.

use serde_json::Value;
use starweaver_context::ToolConfig;
use starweaver_environment::{EnvironmentError, EnvironmentProvider, FileStat};
use starweaver_tools::{ToolError, ToolResult};

use super::{
    context::uses_relaxed_text_limits, default_view_line_limit, default_view_max_line_length,
    tool_execution_error, ViewArgs,
};
use crate::bundles::helpers::tool_model_retry;

const BINARY_CHECK_BYTES: usize = 8192;

pub(super) async fn read_text_file(
    provider: &dyn EnvironmentProvider,
    arguments: &ViewArgs,
    stat: &FileStat,
    tool_config: &ToolConfig,
) -> Result<ToolResult, ToolError> {
    let binary_probe = provider
        .read_bytes(&arguments.file_path, 0, Some(BINARY_CHECK_BYTES))
        .await
        .map_err(|error| tool_execution_error("view", error))?;
    if binary_probe.contains(&0) {
        return Err(tool_model_retry(
            "view",
            format!(
                "{} appears to be a binary file. Use an appropriate file-specific tool instead: pdf_convert for PDFs, office_to_markdown for Office/EPUB documents, media understanding for images/audio/video, or shell tools such as xxd for hex inspection.",
                arguments.file_path
            ),
        ));
    }

    let relaxed = uses_relaxed_text_limits(provider, &arguments.file_path, tool_config)?;
    let max_file_size = if relaxed {
        tool_config.view_relaxed_text_file_size
    } else {
        tool_config.view_max_text_file_size
    };
    if stat.size > max_file_size {
        return Err(tool_model_retry(
            "view",
            format!(
                "file is too large to inspect safely ({}). Maximum supported text view size is {}. Use shell tools such as head, tail, or sed -n to read portions of this file, or narrow the request if a range-capable tool is available.",
                format_size(stat.size),
                format_size(max_file_size),
            ),
        ));
    }

    let line_limit = if relaxed && arguments.line_limit == default_view_line_limit() {
        tool_config.view_relaxed_line_limit
    } else {
        arguments.line_limit
    };
    let max_line_length = if relaxed && arguments.max_line_length == default_view_max_line_length()
    {
        tool_config.view_relaxed_max_line_length
    } else {
        arguments.max_line_length
    };
    let max_content_chars = if relaxed {
        tool_config.view_relaxed_max_content_chars
    } else {
        tool_config.view_max_content_chars
    };

    let full_content = provider
        .read_text(&arguments.file_path)
        .await
        .map_err(|error| tool_execution_error("view", error))?;
    let selection = select_text_lines(
        &arguments.file_path,
        &full_content,
        stat.size,
        arguments.line_offset,
        line_limit,
        max_line_length,
        max_content_chars,
    );
    Ok(selection.into_tool_result())
}

pub(super) async fn ensure_file_missing(
    provider: &dyn EnvironmentProvider,
    tool: &str,
    path: &str,
) -> Result<(), ToolError> {
    match provider.stat(path).await {
        Ok(_) => Err(tool_model_retry(
            tool,
            "file already exists. Use write with mode \"w\" to overwrite existing content, or choose a different file path for create operations.",
        )),
        Err(EnvironmentError::NotFound(_)) => Ok(()),
        Err(error) => Err(tool_execution_error(tool, error)),
    }
}

struct TextSelection {
    content: String,
    metadata: Option<Value>,
}

impl TextSelection {
    fn into_tool_result(self) -> ToolResult {
        if let Some(metadata) = self.metadata {
            ToolResult::new(metadata)
        } else {
            ToolResult::new(Value::String(self.content))
        }
    }
}

fn select_text_lines(
    file_path: &str,
    content: &str,
    file_size: u64,
    line_offset: Option<usize>,
    line_limit: usize,
    max_line_length: usize,
    max_content_chars: usize,
) -> TextSelection {
    let all_lines = split_lines_keepends(content);
    let total_lines = all_lines.len();
    let start_index = line_offset.filter(|offset| *offset > 0).unwrap_or(0);
    let has_offset = start_index > 0;
    let selected_lines = all_lines
        .iter()
        .skip(start_index)
        .take(line_limit)
        .copied()
        .collect::<Vec<_>>();
    let has_line_limit = all_lines.len().saturating_sub(start_index) > line_limit;
    let mut lines_truncated = false;
    let mut processed = Vec::new();
    for line in selected_lines {
        if line.chars().count() > max_line_length {
            lines_truncated = true;
            processed.push(format!(
                "{}... (line truncated)\n",
                line.chars().take(max_line_length).collect::<String>()
            ));
        } else {
            processed.push(line.to_string());
        }
    }
    let mut selected_content = processed.concat();
    let content_truncated = if selected_content.chars().count() > max_content_chars {
        selected_content = selected_content
            .chars()
            .take(max_content_chars)
            .collect::<String>();
        selected_content.push_str("\n... (content truncated)");
        true
    } else {
        false
    };

    let needs_metadata = has_offset || has_line_limit || lines_truncated || content_truncated;
    if !needs_metadata {
        return TextSelection {
            content: selected_content,
            metadata: None,
        };
    }

    let actual_lines_read = processed.len();
    let start_line = start_index + 1;
    let end_line = if actual_lines_read > 0 {
        start_line + actual_lines_read - 1
    } else {
        start_line
    };
    TextSelection {
        content: selected_content.clone(),
        metadata: Some(serde_json::json!({
            "content": selected_content,
            "metadata": {
                "file_path": file_name(file_path),
                "total_lines": total_lines,
                "total_characters": content.chars().count(),
                "file_size_bytes": file_size,
                "current_segment": {
                    "start_line": start_line,
                    "end_line": end_line,
                    "lines_to_show": actual_lines_read,
                    "has_more_content": end_line < total_lines,
                },
                "reading_parameters": {
                    "line_offset": if has_offset { serde_json::json!(start_index) } else { Value::Null },
                    "line_limit": line_limit,
                },
                "truncation_info": {
                    "lines_truncated": lines_truncated,
                    "content_truncated": content_truncated,
                    "max_line_length": max_line_length,
                },
            },
            "system": "Increase the line_limit and max_line_length if you need more context.",
        })),
    }
}

fn split_lines_keepends(content: &str) -> Vec<&str> {
    if content.is_empty() {
        Vec::new()
    } else {
        content.split_inclusive('\n').collect()
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

pub(super) fn format_size(size_bytes: u64) -> String {
    if size_bytes < 1024 {
        return format!("{size_bytes} bytes");
    }
    if size_bytes < 1024 * 1024 {
        let tenths = size_bytes.saturating_mul(10).saturating_add(512) / 1024;
        return format!("{}.{:01} KB", tenths / 10, tenths % 10);
    }
    let hundredths = size_bytes
        .saturating_mul(100)
        .saturating_add(1024 * 1024 / 2)
        / (1024 * 1024);
    format!("{}.{:02} MB", hundredths / 100, hundredths % 100)
}

pub(super) fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        value.to_string()
    } else {
        format!("{}...", value.chars().take(limit).collect::<String>())
    }
}
