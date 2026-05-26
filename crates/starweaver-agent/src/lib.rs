//! Ergonomic SDK facade over the Starweaver bare runtime.

pub mod session;
pub mod subagent;
pub mod subagent_config;

use std::sync::Arc;

use starweaver_model::{ModelAdapter, ModelRequestParameters, ModelSettings};
use starweaver_runtime::Agent as RuntimeAgent;
use starweaver_tools::{DynTool, DynToolset};

pub use session::AgentSession;
pub use starweaver_context::{AgentContext, ResumableState};
pub use starweaver_core::{
    AgentId, CheckpointId, ConversationId, RunId, SubagentLifecycleEvent, SubagentLifecycleKind,
    SubagentSpec, TaskId, TraceContext, Usage,
};
pub use starweaver_model::{FunctionModel, FunctionModelInfo, TestModel};
pub use starweaver_runtime::{
    AgentCapability, AgentError, AgentOverride, AgentResult, AgentRunState, AgentRuntimePolicy,
    AgentStreamEvent, AgentStreamRecord, AgentStreamResult, CapabilityBundle, CapabilityResult,
    CostBudget, DynamicInstruction, DynamicInstructionError, DynamicInstructionResult,
    FunctionDynamicInstruction, FunctionHistoryProcessor, FunctionOutputFunction,
    FunctionOutputValidator, HistoryProcessor, HistoryProcessorError, HistoryProcessorResult,
    OutputFunction, OutputFunctionContext, OutputFunctionDefinition, OutputSchema,
    OutputValidationError, OutputValidationResult, OutputValidator, OutputValue,
    ReinjectSystemPromptProcessor, RetryEventKind, StaticCapabilityBundle, UsageLimitError,
    UsageLimits,
};
pub use starweaver_tools::{
    mcp_tool_definition, FunctionTool, McpToolSpec, McpToolset, McpToolsetConfig, McpTransport,
    NativeMcpServer, PrefixedTool, PrefixedToolset, StaticToolset, Tool, ToolContext, ToolError,
    ToolInstruction, ToolRegistry, ToolResult, Toolset,
};
pub use subagent::{AgentApp, SubagentConfig, SubagentRegistry, SubagentResult, SubagentTask};
pub use subagent_config::{
    load_subagent_from_file, load_subagents_from_dir, parse_subagent_markdown, SubagentConfigError,
};

/// Builder for a reusable Starweaver agent.
pub struct AgentBuilder {
    model: Arc<dyn ModelAdapter>,
    instructions: Vec<String>,
    model_settings: Option<ModelSettings>,
    request_params: ModelRequestParameters,
    output_schema: Option<OutputSchema>,
    output_validators: Vec<Arc<dyn OutputValidator>>,
    output_functions: Vec<Arc<dyn OutputFunction>>,
    dynamic_instructions: Vec<Arc<dyn DynamicInstruction>>,
    usage_limits: Option<UsageLimits>,
    history_processors: Vec<Arc<dyn HistoryProcessor>>,
    tools: ToolRegistry,
    capabilities: Vec<Arc<dyn AgentCapability>>,
    capability_bundles: Vec<Arc<dyn CapabilityBundle>>,
    subagents: SubagentRegistry,
    policy: AgentRuntimePolicy,
}

impl AgentBuilder {
    /// Create a builder from a model adapter.
    #[must_use]
    pub fn new(model: Arc<dyn ModelAdapter>) -> Self {
        Self {
            model,
            instructions: Vec::new(),
            model_settings: None,
            request_params: ModelRequestParameters::default(),
            output_schema: None,
            output_validators: Vec::new(),
            output_functions: Vec::new(),
            dynamic_instructions: Vec::new(),
            usage_limits: None,
            history_processors: Vec::new(),
            tools: ToolRegistry::new(),
            capabilities: Vec::new(),
            capability_bundles: Vec::new(),
            subagents: SubagentRegistry::new(),
            policy: AgentRuntimePolicy::default(),
        }
    }

    /// Add a static instruction.
    #[must_use]
    pub fn instruction(mut self, instruction: impl Into<String>) -> Self {
        self.instructions.push(instruction.into());
        self
    }

    /// Set model settings.
    #[must_use]
    pub fn model_settings(mut self, settings: ModelSettings) -> Self {
        self.model_settings = Some(settings);
        self
    }

    /// Add a dynamic instruction.
    #[must_use]
    pub fn dynamic_instruction(mut self, instruction: Arc<dyn DynamicInstruction>) -> Self {
        self.dynamic_instructions.push(instruction);
        self
    }

    /// Set provider-neutral request parameters.
    #[must_use]
    pub fn request_params(mut self, params: ModelRequestParameters) -> Self {
        self.request_params = params;
        self
    }

    /// Set structured output schema.
    #[must_use]
    pub fn output_schema(mut self, schema: OutputSchema) -> Self {
        self.output_schema = Some(schema);
        self
    }

    /// Add a structured output validator.
    #[must_use]
    pub fn output_validator(mut self, validator: Arc<dyn OutputValidator>) -> Self {
        self.output_validators.push(validator);
        self
    }

    /// Add an output function.
    #[must_use]
    pub fn output_function(mut self, function: Arc<dyn OutputFunction>) -> Self {
        self.output_functions.push(function);
        self
    }

    /// Set usage limits.
    #[must_use]
    pub const fn usage_limits(mut self, limits: UsageLimits) -> Self {
        self.usage_limits = Some(limits);
        self
    }

    /// Add a history processor.
    #[must_use]
    pub fn history_processor(mut self, processor: Arc<dyn HistoryProcessor>) -> Self {
        self.history_processors.push(processor);
        self
    }

    /// Add one function tool.
    #[must_use]
    pub fn tool(mut self, tool: DynTool) -> Self {
        self.tools.insert(tool);
        self
    }

    /// Add one toolset.
    #[must_use]
    pub fn toolset(mut self, toolset: &DynToolset) -> Self {
        self.tools.insert_toolset(toolset);
        self
    }

    /// Set the whole tool registry.
    #[must_use]
    pub fn tool_registry(mut self, tools: ToolRegistry) -> Self {
        self.tools = tools;
        self
    }

    /// Set the agent-level retry default for runtime tools.
    #[must_use]
    pub fn tool_retries(mut self, max_retries: usize) -> Self {
        self.tools.set_max_retries(max_retries);
        self
    }

    /// Add a capability hook.
    #[must_use]
    pub fn capability(mut self, capability: Arc<dyn AgentCapability>) -> Self {
        self.capabilities.push(capability);
        self
    }

    /// Add a capability bundle.
    #[must_use]
    pub fn capability_bundle(mut self, bundle: Arc<dyn CapabilityBundle>) -> Self {
        self.capability_bundles.push(bundle);
        self
    }

    /// Add a subagent configuration to the SDK-level registry.
    #[must_use]
    pub fn subagent(mut self, subagent: SubagentConfig) -> Self {
        self.subagents.insert(subagent);
        self
    }

    /// Set the SDK-level subagent registry.
    #[must_use]
    pub fn subagent_registry(mut self, registry: SubagentRegistry) -> Self {
        self.subagents = registry;
        self
    }

    /// Set runtime policy.
    #[must_use]
    pub const fn policy(mut self, policy: AgentRuntimePolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Return the SDK-level subagent registry configured on this builder.
    #[must_use]
    pub const fn subagents(&self) -> &SubagentRegistry {
        &self.subagents
    }

    /// Build an SDK application wrapper with runtime agent and application-level protocols.
    #[must_use]
    pub fn build_app(self) -> AgentApp {
        let subagents = self.subagents.clone();
        AgentApp::new(self.build()).with_subagents(subagents)
    }

    /// Build a reusable runtime agent.
    #[must_use]
    pub fn build(self) -> RuntimeAgent {
        let mut agent = RuntimeAgent::new(self.model)
            .with_request_params(self.request_params)
            .with_tools(self.tools)
            .with_policy(self.policy);
        for instruction in self.instructions {
            agent = agent.with_instruction(instruction);
        }
        for instruction in self.dynamic_instructions {
            agent = agent.with_dynamic_instruction(instruction);
        }
        if let Some(settings) = self.model_settings {
            agent = agent.with_model_settings(settings);
        }
        if let Some(schema) = self.output_schema {
            agent = agent.with_output_schema(schema);
        }
        if let Some(limits) = self.usage_limits {
            agent = agent.with_usage_limits(limits);
        }
        for processor in self.history_processors {
            agent = agent.with_history_processor(processor);
        }
        for function in self.output_functions {
            agent = agent.with_output_function(function);
        }
        for validator in self.output_validators {
            agent = agent.with_output_validator(validator);
        }
        for capability in self.capabilities {
            agent = agent.with_capability(capability);
        }
        for bundle in self.capability_bundles {
            agent = agent.with_capability_bundle(bundle.as_ref());
        }
        agent
    }
}

/// Create an agent builder from a model.
#[must_use]
pub fn agent(model: Arc<dyn ModelAdapter>) -> AgentBuilder {
    AgentBuilder::new(model)
}
