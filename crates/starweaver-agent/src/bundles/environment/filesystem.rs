use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{Map, Value};
use starweaver_context::{AgentContext, ToolConfig};
use starweaver_environment::{
    matches_path_pattern, EnvironmentError, EnvironmentProvider, FileGlobOptions, FileGrepMatch,
    FileGrepOptions, FileStat,
};
use starweaver_tools::{
    DynToolset, StaticToolset, ToolContext, ToolError, ToolInstruction, ToolResult,
};
use uuid::Uuid;

use super::{
    args::{
        default_view_line_limit, default_view_max_line_length, DeleteArgs, EditArgs, FilePathArgs,
        GlobArgs, GrepArgs, ListArgs, MkdirArgs, MultiEditArgs, PathPairsArgs, ViewArgs, WriteArgs,
    },
    common::{limit_or_unlimited, non_negative_limit},
    handle::environment_provider,
};
use crate::bundles::{
    helpers::{static_tool, tool_execution_error},
    HostMediaCapabilities, HostMediaUnderstandingClientHandle, MediaUnderstandingRequest,
};

const BINARY_CHECK_BYTES: usize = 8192;

/// Create filesystem tools backed by the `EnvironmentHandle` stored in `AgentContext`.
#[must_use]
pub fn filesystem_tools() -> DynToolset {
    Arc::new(
        StaticToolset::new("filesystem")
            .with_id("filesystem")
            .with_instruction(ToolInstruction::new(
                "filesystem",
                "Filesystem tools operate inside the active AgentContext environment. Prefer glob to discover candidate paths, grep to find matching text, view for focused reads, write for intentional writes, edit or multi_edit for precise replacements, and resource_ref when a durable provider-scoped reference is enough. Large glob, grep, and shell-style outputs are saved through the provider tmp file abstraction instead of bypassing the active environment.",
            ))
            .with_instruction(ToolInstruction::new(
                "edit",
                "<edit-tool>\nPerforms exact string replacement in files.\n\n<best-practices>\n- old_string must match file content EXACTLY, including whitespace and indentation\n- Preserve exact indentation from view output and ignore line number prefixes\n- Include 3-5 lines of context to ensure unique matches\n- Use replace_all=true for renaming variables across the file\n- Use multi_edit instead of multiple edit calls when changing the same file, especially when changes could otherwise be issued concurrently\n- Empty old_string creates a new file and fails if the file exists\n</best-practices>\n</edit-tool>",
            ))
            .with_instruction(ToolInstruction::new(
                "multi_edit",
                "<multi-edit-tool>\nPerform multiple find-and-replace operations on a single file.\n\n<best-practices>\n- Prefer multi_edit over multiple single edits for the same file\n- When making multiple changes to the same file, including changes planned in parallel, do not issue concurrent edit calls; combine them into one multi_edit call\n- Each old_string must be unique or use replace_all=true\n- Edits are applied sequentially; ensure earlier edits do not affect later ones\n- All edits must succeed or none are applied as an atomic operation\n- Empty old_string in the first edit creates a new file\n</best-practices>\n</multi-edit-tool>",
            ))
            .with_tools([
                static_tool("view", "Read a provider-scoped file. Text reads support pagination and truncation metadata; image, video, and audio files are loaded through the active environment and either attached for native model media support or analyzed by the configured fallback client.", read_text),
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
    let tool_config = tool_config_from_context(&context, "glob")?;
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
    let result = serde_json::json!({
        "root": arguments.root,
        "pattern": arguments.pattern,
        "matches": matches,
    });
    guard_glob_output(provider.as_ref(), &tool_config, result).await
}

async fn grep_files(context: ToolContext, arguments: GrepArgs) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&context, "grep")?;
    let tool_config = tool_config_from_context(&context, "grep")?;
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
    guard_grep_output(
        provider.as_ref(),
        &tool_config,
        &arguments.root,
        &arguments.pattern,
        matches,
    )
    .await
}

async fn mkdir_paths(context: ToolContext, arguments: MkdirArgs) -> Result<ToolResult, ToolError> {
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

async fn delete_paths(
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

async fn move_paths(
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

async fn copy_paths(
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

fn tool_config_from_context(context: &ToolContext, tool: &str) -> Result<ToolConfig, ToolError> {
    let agent_context = context.dependency::<AgentContext>().ok_or_else(|| {
        tool_execution_error(tool, "AgentContext dependency is missing from ToolContext")
    })?;
    Ok(agent_context.tool_config.clone())
}

fn uses_relaxed_text_limits(
    provider: &dyn EnvironmentProvider,
    path: &str,
    tool_config: &ToolConfig,
) -> Result<bool, ToolError> {
    let patterns = tool_config.view_relaxed_text_patterns();
    if patterns.is_empty() {
        return Ok(false);
    }
    let candidates = provider.path_match_candidates(path);
    for pattern in patterns {
        for candidate in &candidates {
            let matches = matches_path_pattern(candidate, pattern)
                .map_err(|error| tool_execution_error("view", error))?;
            if matches {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

async fn read_text_file(
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
        return Ok(ToolResult::new(Value::String(format!(
            "Error: {} appears to be a binary file. Use appropriate tools (e.g. pdf_convert for PDFs, office_to_markdown for Office/EPUB, or xxd for hex dumps) instead.",
            arguments.file_path
        ))));
    }

    let relaxed = uses_relaxed_text_limits(provider, &arguments.file_path, tool_config)?;
    let max_file_size = if relaxed {
        tool_config.view_relaxed_text_file_size
    } else {
        tool_config.view_max_text_file_size
    };
    if stat.size > max_file_size {
        return Ok(ToolResult::new(serde_json::json!({
            "success": false,
            "error": format!(
                "File is too large to inspect safely ({}). Maximum supported text view size is {}. Use shell tools (e.g. head, tail, sed -n) to read portions of this file.",
                format_size(stat.size),
                format_size(max_file_size),
            ),
        })));
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

#[allow(clippy::too_many_lines)]
async fn read_media_file(
    context: &ToolContext,
    provider: &dyn EnvironmentProvider,
    arguments: &ViewArgs,
    stat: FileStat,
    tool_config: &ToolConfig,
) -> Result<ToolResult, ToolError> {
    let media_kind = classify_media_path(&arguments.file_path);
    let max_inline = match media_kind {
        MediaKind::Image => tool_config.view_max_inline_image_bytes,
        MediaKind::Video => tool_config.view_max_inline_video_bytes,
        MediaKind::Audio => tool_config.view_max_inline_audio_bytes,
    };
    if stat.size > max_inline {
        return Ok(ToolResult::new(serde_json::json!({
            "success": false,
            "file_path": arguments.file_path,
            "media_kind": media_kind.as_str(),
            "error": format!(
                "{} file is too large to inline ({}). Maximum supported inline size is {}.",
                media_kind.title(),
                format_size(stat.size),
                format_size(max_inline),
            ),
        })));
    }
    let data = provider
        .read_bytes(&arguments.file_path, 0, None)
        .await
        .map_err(|error| tool_execution_error("view", error))?;
    let media_type = match media_kind {
        MediaKind::Image => detect_image_media_type(&data)
            .or_else(|| image_media_type(&arguments.file_path))
            .unwrap_or("application/octet-stream"),
        MediaKind::Video => video_media_type(&arguments.file_path),
        MediaKind::Audio => audio_media_type(&arguments.file_path),
    };
    if media_kind == MediaKind::Image && !is_supported_inline_image(media_type) {
        return Ok(ToolResult::new(Value::String(format!(
            "Error: unsupported image format '{media_type}' for {}. Supported formats: image/gif, image/jpeg, image/png, image/webp.",
            arguments.file_path
        ))));
    }

    let data_url = data_url(media_type, &data);
    let capabilities = context.dependency::<HostMediaCapabilities>();
    let native_supported = capabilities
        .as_ref()
        .is_some_and(|capabilities| media_capability_supported(capabilities, media_kind));
    if native_supported {
        let message = format!(
            "The {} is attached in a provider-native media message.",
            media_kind.as_str()
        );
        let mut private_metadata = Map::new();
        private_metadata.insert(
            "starweaver_tool_return_content_parts".to_string(),
            serde_json::json!([{
                "kind": "data_url",
                "data_url": data_url,
                "media_type": media_type,
            }]),
        );
        private_metadata.insert(
            "starweaver_tool_return_prompt".to_string(),
            serde_json::json!(media_prompt(
                media_kind,
                &arguments.file_path,
                arguments.instructions.as_deref()
            )),
        );
        return Ok(ToolResult::new(serde_json::json!({
            "success": true,
            "file_path": arguments.file_path,
            "media_kind": media_kind.as_str(),
            "media_type": media_type,
            "native_supported": true,
            "model_id": capabilities.and_then(|capabilities| capabilities.model_id.clone()),
            "message": message,
            "instructions": arguments.instructions,
        }))
        .with_private_metadata(private_metadata));
    }

    if let Some(handle) = context.dependency::<HostMediaUnderstandingClientHandle>() {
        let response = handle
            .client
            .understand(MediaUnderstandingRequest {
                media_kind: media_kind.as_str().to_string(),
                url: data_url,
                instructions: arguments.instructions.clone(),
            })
            .await
            .map_err(|error| tool_execution_error("view", error))?;
        return serde_json::to_value(response)
            .map(ToolResult::new)
            .map_err(|error| tool_execution_error("view", error));
    }

    Ok(ToolResult::new(serde_json::json!({
        "success": false,
        "file_path": arguments.file_path,
        "media_kind": media_kind.as_str(),
        "media_type": media_type,
        "native_supported": false,
        "model_id": capabilities.and_then(|capabilities| capabilities.model_id.clone()),
        "missing_dependency": "HostMediaUnderstandingClientHandle",
        "message": "The active model does not advertise native support for this local media kind. Configure a HostMediaUnderstandingClientHandle fallback adapter or switch to a media-capable model.",
    })))
}

async fn guard_glob_output(
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

async fn guard_grep_output(
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

async fn ensure_file_missing(
    provider: &dyn EnvironmentProvider,
    tool: &str,
    path: &str,
) -> Result<(), ToolError> {
    match provider.stat(path).await {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ViewFileKind {
    Text,
    Image,
    Video,
    Audio,
    Pdf,
    Office,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MediaKind {
    Image,
    Video,
    Audio,
}

impl MediaKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Video => "Video",
            Self::Audio => "Audio",
        }
    }
}

fn classify_view_path(path: &str) -> ViewFileKind {
    match extension(path).as_deref() {
        Some("jpg" | "jpeg" | "png" | "gif" | "bmp" | "ico" | "webp") => ViewFileKind::Image,
        Some(
            "mp4" | "webm" | "mov" | "avi" | "flv" | "wmv" | "mpg" | "mpeg" | "3gp" | "mkv" | "m4v"
            | "ogv",
        ) => ViewFileKind::Video,
        Some("mp3" | "wav" | "ogg" | "flac" | "m4a" | "aac" | "wma" | "opus" | "aiff" | "aif") => {
            ViewFileKind::Audio
        }
        Some("pdf") => ViewFileKind::Pdf,
        Some("doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "epub") => ViewFileKind::Office,
        Some(
            "txt" | "md" | "json" | "xml" | "csv" | "html" | "htm" | "rs" | "py" | "js" | "ts"
            | "tsx" | "jsx" | "toml" | "yaml" | "yml",
        ) => ViewFileKind::Text,
        _ => ViewFileKind::Unknown,
    }
}

fn classify_media_path(path: &str) -> MediaKind {
    match classify_view_path(path) {
        ViewFileKind::Image => MediaKind::Image,
        ViewFileKind::Video => MediaKind::Video,
        ViewFileKind::Audio => MediaKind::Audio,
        ViewFileKind::Text | ViewFileKind::Pdf | ViewFileKind::Office | ViewFileKind::Unknown => {
            MediaKind::Image
        }
    }
}

const fn media_capability_supported(capabilities: &HostMediaCapabilities, kind: MediaKind) -> bool {
    match kind {
        MediaKind::Image => capabilities.supports_image_url,
        MediaKind::Video => capabilities.supports_video_url,
        MediaKind::Audio => capabilities.supports_audio_url,
    }
}

fn extension(path: &str) -> Option<String> {
    let filename = path.rsplit(['/', '\\']).next().unwrap_or(path);
    filename
        .rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map(|(_, ext)| ext.to_ascii_lowercase())
}

fn image_media_type(path: &str) -> Option<&'static str> {
    match extension(path).as_deref() {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        Some("bmp") => Some("image/bmp"),
        Some("ico") => Some("image/x-icon"),
        _ => None,
    }
}

fn video_media_type(path: &str) -> &'static str {
    match extension(path).as_deref() {
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("avi") => "video/x-msvideo",
        Some("flv") => "video/x-flv",
        Some("wmv") => "video/x-ms-wmv",
        Some("mpg" | "mpeg") => "video/mpeg",
        Some("3gp") => "video/3gpp",
        Some("mkv") => "video/x-matroska",
        Some("m4v") => "video/x-m4v",
        Some("ogv") => "video/ogg",
        _ => "video/mp4",
    }
}

fn audio_media_type(path: &str) -> &'static str {
    match extension(path).as_deref() {
        Some("wav") => "audio/wav",
        Some("ogg") => "audio/ogg",
        Some("flac") => "audio/flac",
        Some("m4a") => "audio/mp4",
        Some("aac") => "audio/aac",
        Some("wma") => "audio/x-ms-wma",
        Some("opus") => "audio/opus",
        Some("aiff" | "aif") => "audio/aiff",
        _ => "audio/mpeg",
    }
}

fn detect_image_media_type(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if data.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

fn is_supported_inline_image(media_type: &str) -> bool {
    matches!(
        media_type,
        "image/gif" | "image/jpeg" | "image/png" | "image/webp"
    )
}

fn media_prompt(kind: MediaKind, file_path: &str, instructions: Option<&str>) -> String {
    let mut prompt = format!(
        "The view tool loaded local {kind} file `{file_path}` through the active environment. Inspect the attached media and answer accordingly.",
        kind = kind.as_str(),
    );
    if let Some(instructions) = instructions.filter(|value| !value.trim().is_empty()) {
        prompt.push_str(
            "

Analysis instructions:
",
        );
        prompt.push_str(instructions.trim());
    }
    prompt
}

fn data_url(media_type: &str, data: &[u8]) -> String {
    format!("data:{media_type};base64,{}", STANDARD.encode(data))
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

fn format_size(size_bytes: u64) -> String {
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

fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        value.to_string()
    } else {
        format!("{}...", value.chars().take(limit).collect::<String>())
    }
}
