use std::sync::Arc;

use starweaver_environment::{
    EnvironmentError, EnvironmentProvider, FileGlobOptions, FileGrepOptions,
};
use starweaver_tools::{
    DynToolset, StaticToolset, ToolContext, ToolError, ToolInstruction, ToolResult,
};

use super::{
    args::{
        DeleteArgs, EditArgs, FilePathArgs, GlobArgs, GrepArgs, ListArgs, MkdirArgs, MultiEditArgs,
        PathPairsArgs, ViewArgs, WriteArgs,
    },
    common::{limit_or_unlimited, non_negative_limit, operation},
    handle::environment_provider,
};
use crate::bundles::helpers::{static_tool, tool_execution_error};

/// Create filesystem tools backed by the `EnvironmentHandle` stored in `AgentContext`.
#[must_use]
pub fn filesystem_tools() -> DynToolset {
    Arc::new(
        StaticToolset::new("filesystem")
            .with_id("filesystem")
            .with_instruction(ToolInstruction::new(
                "filesystem",
                "Filesystem tools operate inside the active AgentContext environment. Prefer glob to discover candidate paths, grep to find matching text, view for focused reads, write for intentional writes, edit or multi_edit for precise replacements, and resource_ref when a durable provider-scoped reference is enough.",
            ))
            .with_tools([
                static_tool("view", "Read a provider-scoped UTF-8 text file.", read_text),
                static_tool("ls", "List provider-scoped file entries.", list_files),
                static_tool("write", "Write a provider-scoped UTF-8 text file.", write_text),
                static_tool("edit", "Perform exact string replacement in files.", edit_text),
                static_tool("multi_edit", "Perform multiple exact replacements in one file.", multi_edit_text),
                static_tool("glob", "Find provider-scoped paths with ripgrep-style glob semantics.", glob_files),
                static_tool("grep", "Search provider-scoped text files with ripgrep regex semantics.", grep_files),
                static_tool("mkdir", "Create directories through a host/provider operation envelope.", mkdir_paths),
                static_tool("delete", "Delete files or directories through a host/provider operation envelope.", delete_paths),
                static_tool("move", "Move files or directories through a host/provider operation envelope.", move_paths),
                static_tool("copy", "Copy files through a host/provider operation envelope.", copy_paths),
                static_tool("resource_ref", "Create a stable provider-scoped resource reference for a path.", resource_ref),
            ]),
    )
}

async fn read_text(
    tool_context: ToolContext,
    arguments: ViewArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&tool_context, "view")?;
    let content = provider
        .read_text(&arguments.file_path)
        .await
        .map_err(|error| tool_execution_error("view", error))?;
    let selected = select_text_lines(
        &content,
        arguments.line_offset.unwrap_or(0),
        arguments.line_limit,
        arguments.max_line_length,
    );
    Ok(ToolResult::new(serde_json::json!({
        "file_path": arguments.file_path,
        "content": selected.content,
        "line_offset": arguments.line_offset,
        "line_limit": arguments.line_limit,
        "max_line_length": arguments.max_line_length,
        "instructions": arguments.instructions,
        "truncated": selected.truncated,
    })))
}

async fn list_files(context: ToolContext, arguments: ListArgs) -> Result<ToolResult, ToolError> {
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

async fn write_text(
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
            return Err(tool_execution_error(
                "write",
                format!("unsupported write mode {mode:?}"),
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

async fn edit_text(
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

async fn multi_edit_text(
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
        if edits.len() > 0 {
            return Err(tool_execution_error(
                "multi_edit",
                "create operation must be the only edit when old_string is empty",
            ));
        }
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

async fn glob_files(context: ToolContext, arguments: GlobArgs) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&context, "glob")?;
    let matches = provider
        .glob(
            &arguments.root,
            &arguments.pattern,
            FileGlobOptions {
                include_hidden: arguments.include_hidden,
                include_ignored: arguments.include_ignored,
                max_results: limit_or_unlimited("glob", "max_results", arguments.max_results)?,
            },
        )
        .await
        .map_err(|error| tool_execution_error("glob", error))?;
    Ok(ToolResult::new(serde_json::json!({
        "root": arguments.root,
        "pattern": arguments.pattern,
        "matches": matches,
    })))
}

async fn grep_files(context: ToolContext, arguments: GrepArgs) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&context, "grep")?;
    let matches = provider
        .grep(
            &arguments.root,
            &arguments.pattern,
            FileGrepOptions {
                include: Some(arguments.include),
                context_lines: non_negative_limit(
                    "grep",
                    "context_lines",
                    arguments.context_lines,
                )?,
                max_results: limit_or_unlimited("grep", "max_results", arguments.max_results)?,
                max_matches_per_file: limit_or_unlimited(
                    "grep",
                    "max_matches_per_file",
                    arguments.max_matches_per_file,
                )?,
                max_files: limit_or_unlimited("grep", "max_files", arguments.max_files)?,
                include_hidden: arguments.include_hidden,
                include_ignored: arguments.include_ignored,
            },
        )
        .await
        .map_err(|error| tool_execution_error("grep", error))?;
    Ok(ToolResult::new(serde_json::json!({
        "root": arguments.root,
        "pattern": arguments.pattern,
        "matches": matches,
    })))
}

async fn mkdir_paths(_context: ToolContext, arguments: MkdirArgs) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "mkdir",
        serde_json::json!({
            "paths": arguments.paths,
            "parents": arguments.parents,
        }),
    ))
}

async fn delete_paths(
    _context: ToolContext,
    arguments: DeleteArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "delete",
        serde_json::json!({
            "paths": arguments.paths,
            "recursive": arguments.recursive,
            "force": arguments.force,
        }),
    ))
}

async fn move_paths(
    _context: ToolContext,
    arguments: PathPairsArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "move",
        serde_json::json!({
            "pairs": arguments.pairs,
            "overwrite": arguments.overwrite,
        }),
    ))
}

async fn copy_paths(
    _context: ToolContext,
    arguments: PathPairsArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "copy",
        serde_json::json!({
            "pairs": arguments.pairs,
            "overwrite": arguments.overwrite,
        }),
    ))
}

async fn resource_ref(
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

fn apply_replacement(
    tool: &str,
    content: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> Result<String, ToolError> {
    if old_string.is_empty() {
        return Err(tool_execution_error(
            tool,
            "old_string must be non-empty for replacement",
        ));
    }
    if !content.contains(old_string) {
        return Err(tool_execution_error(tool, "text not found"));
    }
    if replace_all {
        return Ok(content.replace(old_string, new_string));
    }
    let occurrences = content.matches(old_string).count();
    if occurrences > 1 {
        return Err(tool_execution_error(
            tool,
            "text appears multiple times; add context or use replace_all=true",
        ));
    }
    Ok(content.replacen(old_string, new_string, 1))
}

struct TextSelection {
    content: String,
    truncated: bool,
}

fn select_text_lines(
    content: &str,
    line_offset: usize,
    line_limit: usize,
    max_line_length: usize,
) -> TextSelection {
    let lines = content.lines().skip(line_offset).take(line_limit);
    let mut truncated = content.lines().count() > line_offset.saturating_add(line_limit);
    let content = lines
        .map(|line| {
            if line.chars().count() > max_line_length {
                truncated = true;
                line.chars().take(max_line_length).collect::<String>()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    TextSelection { content, truncated }
}

async fn ensure_file_missing(
    provider: &dyn EnvironmentProvider,
    tool: &str,
    path: &str,
) -> Result<(), ToolError> {
    match provider.read_text(path).await {
        Ok(_) => Err(tool_execution_error(
            tool,
            "file already exists; use write to overwrite existing content",
        )),
        Err(EnvironmentError::NotFound(_)) => Ok(()),
        Err(error) => Err(tool_execution_error(tool, error)),
    }
}

fn ignore_match(pattern: &str, entry: &str) -> bool {
    entry == pattern
        || entry.ends_with(pattern)
        || entry.contains(pattern)
        || pattern
            .strip_suffix('/')
            .is_some_and(|prefix| entry.starts_with(prefix))
}
