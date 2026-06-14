//! Filesystem view tool dispatch.

use serde_json::Value;
use starweaver_environment::EnvironmentError;
use starweaver_tools::{ToolContext, ToolError, ToolResult};

use super::{
    media::{classify_view_path, read_media_file, ViewFileKind},
    read_text_file, tool_config_from_context, tool_execution_error, ViewArgs,
};
use crate::bundles::environment::handle::environment_provider;

pub(super) async fn read_text(
    tool_context: ToolContext,
    arguments: ViewArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&tool_context, "view")?;
    let tool_config = tool_config_from_context(&tool_context, "view")?;
    let stat = match provider.stat(&arguments.file_path).await {
        Ok(stat) => stat,
        Err(EnvironmentError::NotFound(_)) => {
            return Ok(ToolResult::new(Value::String(format!(
                "Error: File not found: {}",
                arguments.file_path
            ))));
        }
        Err(error) => return Err(tool_execution_error("view", error)),
    };
    if stat.is_dir {
        return Ok(ToolResult::new(Value::String(format!(
            "Error: Path is a directory, not a file: {}",
            arguments.file_path
        ))));
    }

    match classify_view_path(&arguments.file_path) {
        ViewFileKind::Image | ViewFileKind::Video | ViewFileKind::Audio => {
            read_media_file(
                &tool_context,
                provider.as_ref(),
                &arguments,
                stat,
                &tool_config,
            )
            .await
        }
        ViewFileKind::Pdf => Ok(ToolResult::new(serde_json::json!({
            "success": false,
            "file_path": arguments.file_path,
            "media_kind": "document",
            "message": "PDF files are not parsed by view. Use pdf_convert for provider-scoped PDF conversion.",
            "next_tool": "pdf_convert",
        }))),
        ViewFileKind::Office => Ok(ToolResult::new(serde_json::json!({
            "success": false,
            "file_path": arguments.file_path,
            "media_kind": "document",
            "message": "Office and EPUB files are not parsed by view. Use office_to_markdown for provider-scoped conversion.",
            "next_tool": "office_to_markdown",
        }))),
        ViewFileKind::Text | ViewFileKind::Unknown => {
            read_text_file(provider.as_ref(), &arguments, &stat, &tool_config).await
        }
    }
}
