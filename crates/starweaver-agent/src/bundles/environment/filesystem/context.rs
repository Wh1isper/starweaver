//! Filesystem tool context helpers.

use starweaver_context::{AgentContext, ToolConfig};
use starweaver_environment::{matches_path_pattern, EnvironmentProvider};
use starweaver_tools::{ToolContext, ToolError};

use super::tool_execution_error;

pub(super) fn tool_config_from_context(
    context: &ToolContext,
    tool: &str,
) -> Result<ToolConfig, ToolError> {
    let agent_context = context.dependency::<AgentContext>().ok_or_else(|| {
        tool_execution_error(tool, "AgentContext dependency is missing from ToolContext")
    })?;
    Ok(agent_context.tool_config.clone())
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
