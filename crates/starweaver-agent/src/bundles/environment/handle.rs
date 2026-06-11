use async_trait::async_trait;
use starweaver_context::AgentContext;
use starweaver_environment::{DynEnvironmentProvider, DynProcessShellProvider};
use starweaver_runtime::{AgentCapability, AgentRunState, CapabilityError, CapabilityResult};
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

/// Capability that injects provider-supplied environment context into model requests.
#[derive(Clone, Debug, Default)]
pub struct EnvironmentContextCapability;

#[async_trait]
impl AgentCapability for EnvironmentContextCapability {
    async fn before_model_request_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        request: &mut starweaver_model::ModelRequest,
        _settings: &mut Option<starweaver_model::ModelSettings>,
    ) -> CapabilityResult<()> {
        let Some(environment) = context.dependencies.get::<EnvironmentHandle>() else {
            return Ok(());
        };
        let Some(text) = environment
            .provider()
            .get_context_instructions()
            .await
            .map_err(|error| CapabilityError::Failed(error.to_string()))?
        else {
            return Ok(());
        };
        let mut metadata = serde_json::Map::new();
        metadata.insert(
            "starweaver_instruction_origin".to_string(),
            serde_json::json!("environment_context"),
        );
        request.parts.insert(
            0,
            starweaver_model::ModelRequestPart::Instruction { text, metadata },
        );
        Ok(())
    }
}

/// Attach the active environment to an `AgentContext`.
///
/// Process-capable environment providers also expose the background shell handle
/// from the same attachment point, so callers do not need a separate injection
/// path for foreground and background shell operations.
pub fn attach_environment(context: &mut AgentContext, provider: DynEnvironmentProvider) {
    context
        .dependencies
        .insert(EnvironmentHandle::new(provider.clone()));
    if let Some(process_provider) = provider.process_shell_provider() {
        super::shell::attach_process_shell(context, process_provider);
    }
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

pub(super) fn maybe_environment_provider(context: &ToolContext) -> Option<DynEnvironmentProvider> {
    let agent_context = context.dependency::<AgentContext>()?;
    let environment = agent_context.dependencies.get::<EnvironmentHandle>()?;
    Some(environment.provider())
}
