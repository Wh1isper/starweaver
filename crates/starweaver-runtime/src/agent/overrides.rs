//! Scoped agent override builder.

use std::sync::Arc;

use starweaver_model::{ModelAdapter, ModelRequestParameters, ModelSettings};
use starweaver_tools::{DynTool, DynToolset, ToolRegistry};

use crate::{
    agent::{Agent, AgentRuntimePolicy},
    capability::{AgentCapability, CapabilityBundle},
    executor::DynAgentExecutor,
    instructions::DynDynamicInstruction,
    output::{DynOutputFunction, OutputSchema, OutputValidator},
    usage::UsageLimits,
};

/// Scoped agent override builder.
pub struct AgentOverride {
    agent: Agent,
}

impl AgentOverride {
    pub(super) const fn new(agent: Agent) -> Self {
        Self { agent }
    }

    /// Override the model adapter.
    #[must_use]
    pub fn model(mut self, model: Arc<dyn ModelAdapter>) -> Self {
        self.agent.model = model;
        self
    }

    /// Override model settings.
    #[must_use]
    pub fn model_settings(mut self, settings: Option<ModelSettings>) -> Self {
        self.agent.model_settings = settings;
        self
    }

    /// Override request parameters.
    #[must_use]
    pub fn request_params(mut self, params: ModelRequestParameters) -> Self {
        self.agent.request_params = params;
        self
    }

    /// Override runtime tools.
    #[must_use]
    pub fn with_tools(mut self, tools: ToolRegistry) -> Self {
        self.agent.tools = tools;
        self
    }

    /// Add one runtime tool to the overridden agent clone.
    #[must_use]
    pub fn tool(mut self, tool: DynTool) -> Self {
        self.agent.tools.insert(tool);
        self
    }

    /// Add one runtime toolset to the overridden agent clone.
    #[must_use]
    pub fn toolset(mut self, toolset: &DynToolset) -> Self {
        self.agent.tools.insert_toolset(toolset);
        self
    }

    /// Merge runtime tools from another registry into the overridden agent clone.
    #[must_use]
    pub fn append_tools(mut self, tools: &ToolRegistry) -> Self {
        self.agent.tools.insert_registry(tools);
        self
    }

    /// Override usage limits.
    #[must_use]
    pub const fn usage_limits(mut self, limits: Option<UsageLimits>) -> Self {
        self.agent.usage_limits = limits;
        self
    }

    /// Override static instructions.
    #[must_use]
    pub fn with_instructions(mut self, instructions: Vec<String>) -> Self {
        self.agent.instructions = instructions;
        self
    }

    /// Append static instructions to the overridden agent clone.
    #[must_use]
    pub fn append_instructions(mut self, instructions: impl IntoIterator<Item = String>) -> Self {
        self.agent.instructions.extend(instructions);
        self
    }

    /// Override dynamic instructions.
    #[must_use]
    pub fn dynamic_instructions(mut self, instructions: Vec<DynDynamicInstruction>) -> Self {
        self.agent.dynamic_instructions = instructions;
        self
    }

    /// Override structured output schema.
    #[must_use]
    pub fn output_schema(mut self, schema: Option<OutputSchema>) -> Self {
        self.agent.output_schema = schema;
        self
    }

    /// Override output validators.
    #[must_use]
    pub fn output_validators(mut self, validators: Vec<Arc<dyn OutputValidator>>) -> Self {
        self.agent.output_validators = validators;
        self
    }

    /// Override output functions.
    #[must_use]
    pub fn output_functions(mut self, functions: Vec<DynOutputFunction>) -> Self {
        self.agent.output_functions = functions;
        self
    }

    /// Override capabilities.
    #[must_use]
    pub fn capabilities(mut self, capabilities: Vec<Arc<dyn AgentCapability>>) -> Self {
        self.agent.capabilities = capabilities;
        self
    }

    /// Override durable executor.
    #[must_use]
    pub fn executor(mut self, executor: DynAgentExecutor) -> Self {
        self.agent.executor = executor;
        self
    }

    /// Apply a capability bundle to the overridden agent clone.
    #[must_use]
    pub fn capability_bundle(mut self, bundle: &dyn CapabilityBundle) -> Self {
        self.agent = self.agent.with_capability_bundle(bundle);
        self
    }

    /// Override runtime policy.
    #[must_use]
    pub const fn policy(mut self, policy: AgentRuntimePolicy) -> Self {
        self.agent.policy = policy;
        self
    }

    /// Build the overridden agent clone.
    #[must_use]
    pub fn build(self) -> Agent {
        self.agent
    }
}
