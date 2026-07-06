//! Filesystem search tools.

use starweaver_environment::{FileGlobOptions, FileGrepOptions};
use starweaver_tools::{ToolContext, ToolError, ToolResult};

use super::{
    GlobArgs, GrepArgs,
    output::{guard_glob_output, guard_grep_output},
    tool_config_from_context,
};
use crate::bundles::{
    environment::{
        common::{limit_or_unlimited, non_negative_limit},
        handle::environment_provider,
    },
    helpers::{tool_environment_error, tool_feedback},
};

pub(super) async fn glob_files(
    context: ToolContext,
    arguments: GlobArgs,
) -> Result<ToolResult, ToolError> {
    validate_single_search_root("glob", &arguments.root)?;
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
        .map_err(|error| tool_environment_error("glob", error))?;
    let result = serde_json::json!({
        "root": arguments.root,
        "pattern": arguments.pattern,
        "matches": matches,
    });
    guard_glob_output(provider.as_ref(), &tool_config, result).await
}

pub(super) async fn grep_files(
    context: ToolContext,
    arguments: GrepArgs,
) -> Result<ToolResult, ToolError> {
    validate_single_search_root("grep", &arguments.root)?;
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
        .map_err(|error| tool_environment_error("grep", error))?;
    guard_grep_output(
        provider.as_ref(),
        &tool_config,
        &arguments.root,
        &arguments.pattern,
        matches,
    )
    .await
}

fn validate_single_search_root(tool: &str, root: &str) -> Result<(), ToolError> {
    if root.chars().any(|ch| matches!(ch, '\n' | '\r' | '\0')) {
        return Err(tool_feedback(
            tool,
            "root must be a single directory path. For multiple directories, issue parallel tool calls with one root per call, or use a shared parent root with a narrower file pattern.",
        ));
    }
    Ok(())
}
