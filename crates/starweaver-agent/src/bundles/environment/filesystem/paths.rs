//! Filesystem path operation tools.

use std::sync::LazyLock;

use starweaver_environment::{EnvironmentError, FileListOptions};
use starweaver_tools::{ToolContext, ToolError, ToolResult};
use tokio::sync::Semaphore;

use super::{DeleteArgs, FilePathArgs, ListArgs, MkdirArgs, PathPairsArgs, tool_execution_error};
use crate::bundles::environment::common::limit_or_unlimited;
use crate::bundles::environment::handle::environment_provider;

const LS_MAX_CONCURRENT_CALLS: usize = 16;
static LS_CONCURRENCY_LIMIT: LazyLock<Semaphore> =
    LazyLock::new(|| Semaphore::new(LS_MAX_CONCURRENT_CALLS));

pub(super) async fn list_files(
    context: ToolContext,
    arguments: ListArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&context, "ls")?;
    let ignore = arguments.ignore.unwrap_or_default();
    let max_entries = limit_or_unlimited("ls", "max_entries", arguments.max_entries)?;
    let _permit = LS_CONCURRENCY_LIMIT
        .acquire()
        .await
        .map_err(|error| tool_execution_error("ls", error))?;
    let listing = provider
        .list_with_options(
            &arguments.path,
            FileListOptions {
                ignore_patterns: ignore.clone(),
                max_entries,
            },
        )
        .await
        .map_err(|error| tool_execution_error("ls", error))?;
    let mut result = serde_json::json!({
        "path": arguments.path,
        "ignore": ignore,
        "max_entries": arguments.max_entries,
        "entries": listing.entries,
    });
    if listing.truncated {
        result["truncated"] = serde_json::json!(true);
        result["total_entries"] = serde_json::json!(listing.total_entries);
        result["showing"] =
            serde_json::json!(result["entries"].as_array().map_or(0, std::vec::Vec::len));
    }
    Ok(ToolResult::new(result))
}

pub(super) async fn mkdir_paths(
    context: ToolContext,
    arguments: MkdirArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&context, "mkdir")?;
    let mut created = Vec::new();
    for path in arguments.paths {
        provider
            .create_dir(&path, arguments.parents)
            .await
            .map_err(|error| tool_execution_error("mkdir", error))?;
        created.push(path);
    }
    Ok(ToolResult::new(serde_json::json!({
        "created": created,
        "parents": arguments.parents,
    })))
}

pub(super) async fn delete_paths(
    context: ToolContext,
    arguments: DeleteArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&context, "delete")?;
    let mut deleted = Vec::new();
    let mut missing = Vec::new();
    for path in arguments.paths {
        match provider.delete_path(&path, arguments.recursive).await {
            Ok(()) => deleted.push(path),
            Err(EnvironmentError::NotFound(_)) if arguments.force => missing.push(path),
            Err(error) => return Err(tool_execution_error("delete", error)),
        }
    }
    Ok(ToolResult::new(serde_json::json!({
        "deleted": deleted,
        "missing": missing,
        "recursive": arguments.recursive,
        "force": arguments.force,
    })))
}

pub(super) async fn move_paths(
    context: ToolContext,
    arguments: PathPairsArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&context, "move")?;
    let mut moved = Vec::new();
    for pair in arguments.pairs {
        provider
            .move_path(&pair.src, &pair.dst, arguments.overwrite)
            .await
            .map_err(|error| tool_execution_error("move", error))?;
        moved.push(serde_json::json!({"src": pair.src, "dst": pair.dst}));
    }
    Ok(ToolResult::new(serde_json::json!({
        "moved": moved,
        "overwrite": arguments.overwrite,
    })))
}

pub(super) async fn copy_paths(
    context: ToolContext,
    arguments: PathPairsArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&context, "copy")?;
    let mut copied = Vec::new();
    for pair in arguments.pairs {
        provider
            .copy_path(&pair.src, &pair.dst, arguments.overwrite)
            .await
            .map_err(|error| tool_execution_error("copy", error))?;
        copied.push(serde_json::json!({"src": pair.src, "dst": pair.dst}));
    }
    Ok(ToolResult::new(serde_json::json!({
        "copied": copied,
        "overwrite": arguments.overwrite,
    })))
}

pub(super) async fn resource_ref(
    context: ToolContext,
    arguments: FilePathArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&context, "resource_ref")?;
    Ok(ToolResult::new(serde_json::json!({
        "id": format!("{}:{}", provider.id(), arguments.file_path),
        "uri": format!("env://{}/{}", provider.id(), arguments.file_path),
        "metadata": {"provider_id": provider.id(), "file_path": arguments.file_path},
    })))
}
