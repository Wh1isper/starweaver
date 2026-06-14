//! Filesystem path operation tools.

use starweaver_environment::EnvironmentError;
use starweaver_tools::{ToolContext, ToolError, ToolResult};

use super::{tool_execution_error, DeleteArgs, FilePathArgs, ListArgs, MkdirArgs, PathPairsArgs};
use crate::bundles::environment::handle::environment_provider;

pub(super) async fn list_files(
    context: ToolContext,
    arguments: ListArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&context, "ls")?;
    let ignore = arguments.ignore.unwrap_or_default();
    let entries = provider
        .list(&arguments.path)
        .await
        .map_err(|error| tool_execution_error("ls", error))?
        .into_iter()
        .filter(|entry| !ignore.iter().any(|pattern| ignore_match(pattern, entry)))
        .collect::<Vec<_>>();
    Ok(ToolResult::new(serde_json::json!({
        "path": arguments.path,
        "ignore": ignore,
        "entries": entries,
    })))
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

fn ignore_match(pattern: &str, entry: &str) -> bool {
    entry == pattern
        || entry.ends_with(pattern)
        || entry.contains(pattern)
        || pattern
            .strip_suffix('/')
            .is_some_and(|prefix| entry.starts_with(prefix))
}
