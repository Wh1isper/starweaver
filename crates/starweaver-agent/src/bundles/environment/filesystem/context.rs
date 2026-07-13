//! Filesystem tool context helpers.

use starweaver_context::{ToolConfig, ToolRuntimeSnapshot};
use starweaver_environment::{EnvironmentProvider, matches_path_pattern};
use starweaver_tools::{ToolContext, ToolError};

use crate::bundles::helpers::{tool_execution_error, tool_user_error};

pub(super) fn tool_config_from_context(
    context: &ToolContext,
    tool: &str,
) -> Result<ToolConfig, ToolError> {
    let runtime = context.dependency::<ToolRuntimeSnapshot>().ok_or_else(|| {
        tool_user_error(
            tool,
            "ToolRuntimeSnapshot dependency is missing from ToolContext",
        )
    })?;
    Ok(runtime.tool_config().clone())
}

pub(super) fn uses_relaxed_text_limits(
    provider: &dyn EnvironmentProvider,
    path: &str,
    tool_config: &ToolConfig,
) -> Result<bool, ToolError> {
    let patterns = tool_config.effective_view_relaxed_text_patterns();
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
