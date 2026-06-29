//! Filesystem view tool dispatch.

use starweaver_environment::EnvironmentError;
use starweaver_tools::{ToolContext, ToolError, ToolResult};

use super::{
    media::{classify_view_path, read_media_file, ViewFileKind},
    read_text_file, tool_config_from_context, tool_execution_error, ViewArgs,
};
use crate::bundles::{environment::handle::environment_provider, helpers::tool_model_retry};

pub(super) async fn read_text(
    tool_context: ToolContext,
    arguments: ViewArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&tool_context, "view")?;
    let tool_config = tool_config_from_context(&tool_context, "view")?;
    let stat = match provider.stat(&arguments.file_path).await {
        Ok(stat) => stat,
        Err(EnvironmentError::NotFound(_)) => {
            return Err(tool_model_retry(
                "view",
                format!(
                    "file not found: {}. Verify the path with ls/glob, then call view with an existing file path.",
                    arguments.file_path
                ),
            ));
        }
        Err(error) => return Err(tool_execution_error("view", error)),
    };
    if stat.is_dir {
        return Err(tool_model_retry(
            "view",
            format!(
                "path is a directory, not a file: {}. Use ls to list directory entries, then call view on a specific file.",
                arguments.file_path
            ),
        ));
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
        ViewFileKind::Pdf => Err(tool_model_retry(
            "view",
            format!(
                "PDF files are not parsed by view: {}. Use pdf_convert for provider-scoped PDF conversion.",
                arguments.file_path
            ),
        )),
        ViewFileKind::Office => Err(tool_model_retry(
            "view",
            format!(
                "Office and EPUB files are not parsed by view: {}. Use office_to_markdown for provider-scoped conversion.",
                arguments.file_path
            ),
        )),
        ViewFileKind::Text | ViewFileKind::Unknown => {
            read_text_file(provider.as_ref(), &arguments, &stat, &tool_config).await
        }
    }
}
