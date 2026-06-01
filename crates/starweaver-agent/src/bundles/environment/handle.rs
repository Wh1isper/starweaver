use starweaver_context::AgentContext;
use starweaver_environment::{DynEnvironmentProvider, DynProcessShellProvider};
use starweaver_tools::{ToolContext, ToolError};

use crate::bundles::helpers::tool_execution_error;

/// `AgentContext` dependency that exposes the active SDK environment.
#[derive(Clone)]
pub struct EnvironmentHandle {
    provider: DynEnvironmentProvider,
}

impl EnvironmentHandle {
    /// Create an environment handle from a provider.
    #[must_use]
    pub fn new(provider: DynEnvironmentProvider) -> Self {
        Self { provider }
    }

    /// Return the underlying provider.
    #[must_use]
    pub fn provider(&self) -> DynEnvironmentProvider {
        self.provider.clone()
    }
}

/// Attach the active environment to an `AgentContext`.
pub fn attach_environment(context: &mut AgentContext, provider: DynEnvironmentProvider) {
    context
        .dependencies
        .insert(EnvironmentHandle::new(provider));
}

/// Collect environment-provided toolsets for a provider.
#[must_use]
pub fn environment_toolsets() -> Vec<starweaver_tools::DynToolset> {
    vec![
        super::filesystem::filesystem_tools(),
        super::shell::shell_tools(),
    ]
}

/// Collect resource-backed toolsets for a process-capable provider.
#[must_use]
pub fn process_shell_toolsets(
    _provider: DynProcessShellProvider,
) -> Vec<starweaver_tools::DynToolset> {
    vec![super::shell::shell_tools()]
}

pub(super) fn environment_provider(
    context: &ToolContext,
    tool: &str,
) -> Result<DynEnvironmentProvider, ToolError> {
    let agent_context = context.dependency::<AgentContext>().ok_or_else(|| {
        tool_execution_error(tool, "AgentContext dependency is missing from ToolContext")
    })?;
    let environment = agent_context
        .dependencies
        .get::<EnvironmentHandle>()
        .ok_or_else(|| {
            tool_execution_error(tool, "EnvironmentHandle is missing from AgentContext")
        })?;
    Ok(environment.provider())
}
