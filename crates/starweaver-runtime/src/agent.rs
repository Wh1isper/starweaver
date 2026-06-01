//! Bare agent runtime.

use std::sync::Arc;

use starweaver_model::{ModelAdapter, ModelRequestParameters, ModelSettings};
use starweaver_tools::ToolRegistry;

use crate::{
    agent::helpers::merge_request_params,
    capability::{AgentCapability, CapabilityBundle},
    executor::{DirectAgentExecutor, DynAgentExecutor},
    graph::{inspect_graph, AgentGraphTrace, AgentNode, GraphError},
    history::HistoryProcessor,
    instructions::DynDynamicInstruction,
    output::{DynOutputFunction, OutputPolicy, OutputSchema, OutputValidator},
    trace::{DynTraceRecorder, NoopTraceRecorder},
    usage::UsageLimits,
};

mod helpers;
mod overrides;
mod run_loop;
mod runtime_helpers;
mod types;

pub use overrides::AgentOverride;
pub use types::{AgentError, AgentResult, AgentRuntimePolicy};

/// Minimal agent builder/runtime.
#[derive(Clone)]
pub struct Agent {
    model: Arc<dyn ModelAdapter>,
    instructions: Vec<String>,
    dynamic_instructions: Vec<DynDynamicInstruction>,
    model_settings: Option<ModelSettings>,
    request_params: ModelRequestParameters,
    output_schema: Option<OutputSchema>,
    output_validators: Vec<Arc<dyn OutputValidator>>,
    output_functions: Vec<DynOutputFunction>,
    usage_limits: Option<UsageLimits>,
    history_processors: Vec<Arc<dyn HistoryProcessor>>,
    tools: ToolRegistry,
    capabilities: Vec<Arc<dyn AgentCapability>>,
    stream_observers: Vec<Arc<dyn AgentCapability>>,
    executor: DynAgentExecutor,
    trace_recorder: DynTraceRecorder,
    policy: AgentRuntimePolicy,
}

impl Agent {
    /// Create an agent with a model adapter.
    #[must_use]
    pub fn new(model: Arc<dyn ModelAdapter>) -> Self {
        Self {
            model,
            instructions: Vec::new(),
            dynamic_instructions: Vec::new(),
            model_settings: None,
            request_params: ModelRequestParameters::default(),
            output_schema: None,
            output_validators: Vec::new(),
            output_functions: Vec::new(),
            usage_limits: None,
            history_processors: Vec::new(),
            tools: ToolRegistry::new(),
            capabilities: Vec::new(),
            stream_observers: Vec::new(),
            executor: Arc::new(DirectAgentExecutor),
            trace_recorder: Arc::new(NoopTraceRecorder),
            policy: AgentRuntimePolicy::default(),
        }
    }

    /// Add a static instruction.
    #[must_use]
    pub fn with_instruction(mut self, instruction: impl Into<String>) -> Self {
        self.instructions.push(instruction.into());
        self
    }

    /// Add a dynamic instruction.
    #[must_use]
    pub fn with_dynamic_instruction(mut self, instruction: DynDynamicInstruction) -> Self {
        self.dynamic_instructions.push(instruction);
        self
    }

    /// Set default model settings.
    #[must_use]
    pub fn with_model_settings(mut self, settings: ModelSettings) -> Self {
        self.model_settings = Some(settings);
        self
    }

    /// Set default request parameters.
    #[must_use]
    pub fn with_request_params(mut self, params: ModelRequestParameters) -> Self {
        self.request_params = params;
        self
    }

    /// Set runtime tools.
    #[must_use]
    pub fn with_tools(mut self, tools: ToolRegistry) -> Self {
        self.tools = tools;
        self
    }

    /// Merge additional runtime tools into this agent.
    #[must_use]
    pub fn with_appended_tools(mut self, tools: &ToolRegistry) -> Self {
        self.tools.insert_registry(tools);
        self
    }

    /// Return a clone of the runtime tool registry.
    #[must_use]
    pub fn tools(&self) -> ToolRegistry {
        self.tools.clone()
    }

    /// Set the agent-level retry default for runtime tools.
    #[must_use]
    pub fn with_tool_retries(mut self, max_retries: usize) -> Self {
        self.tools.set_max_retries(max_retries);
        self
    }

    /// Set structured output schema.
    #[must_use]
    pub fn with_output_schema(mut self, schema: OutputSchema) -> Self {
        self.output_schema = Some(schema);
        self
    }

    /// Apply a complete output policy.
    #[must_use]
    pub fn with_output_policy(mut self, policy: OutputPolicy) -> Self {
        let (schema, validators, functions, retries) = policy.into_parts();
        if let Some(schema) = schema {
            self.output_schema = Some(schema);
        }
        self.output_validators.extend(validators);
        self.output_functions.extend(functions);
        if let Some(retries) = retries {
            self.policy.output_retries = retries;
        }
        self
    }

    /// Add an output validator.
    #[must_use]
    pub fn with_output_validator(mut self, validator: Arc<dyn OutputValidator>) -> Self {
        self.output_validators.push(validator);
        self
    }

    /// Add an output function.
    #[must_use]
    pub fn with_output_function(mut self, function: DynOutputFunction) -> Self {
        self.output_functions.push(function);
        self
    }

    /// Set usage limits.
    #[must_use]
    pub const fn with_usage_limits(mut self, limits: UsageLimits) -> Self {
        self.usage_limits = Some(limits);
        self
    }

    /// Add a history processor.
    #[must_use]
    pub fn with_history_processor(mut self, processor: Arc<dyn HistoryProcessor>) -> Self {
        self.history_processors.push(processor);
        self
    }

    /// Add a capability hook.
    #[must_use]
    pub fn with_capability(mut self, capability: Arc<dyn AgentCapability>) -> Self {
        self.capabilities.push(capability);
        self
    }

    /// Add a stream observer hook.
    #[must_use]
    pub fn with_stream_observer(mut self, observer: Arc<dyn AgentCapability>) -> Self {
        self.stream_observers.push(observer);
        self
    }

    /// Apply a composable capability bundle.
    #[must_use]
    pub fn with_capability_bundle(mut self, bundle: &dyn CapabilityBundle) -> Self {
        self.apply_capability_bundle(bundle);
        self
    }

    /// Set durable execution checkpoint handler.
    #[must_use]
    pub fn with_executor(mut self, executor: DynAgentExecutor) -> Self {
        self.executor = executor;
        self
    }

    /// Set runtime trace recorder.
    #[must_use]
    pub fn with_trace_recorder(mut self, recorder: DynTraceRecorder) -> Self {
        self.trace_recorder = recorder;
        self
    }

    /// Set runtime policy.
    #[must_use]
    pub const fn with_policy(mut self, policy: AgentRuntimePolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Inspect graph transitions from a state snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when the inspected transition is invalid for the provided state.
    pub fn inspect_graph(
        &self,
        start: AgentNode,
        state: &crate::run::AgentRunState,
    ) -> Result<AgentGraphTrace, GraphError> {
        inspect_graph(
            start,
            state,
            self.policy.max_steps,
            self.policy.max_steps.saturating_mul(4),
        )
    }

    /// Create a scoped override builder for tests and alternate run contexts.
    #[must_use]
    pub fn override_config(&self) -> AgentOverride {
        AgentOverride::new(self.clone())
    }

    fn apply_capability_bundle(&mut self, bundle: &dyn CapabilityBundle) {
        self.capabilities.extend(bundle.hooks());
        self.stream_observers.extend(bundle.stream_observers());
        self.instructions.extend(bundle.get_instructions());
        self.dynamic_instructions
            .extend(bundle.dynamic_instructions());
        if let Some(tools) = bundle.get_tools() {
            self.tools.insert_registry(&tools);
        }
        if let Some(settings) = bundle.model_settings() {
            self.model_settings = Some(match &self.model_settings {
                Some(current) => current.merge(&settings),
                None => settings,
            });
        }
        if let Some(params) = bundle.request_params() {
            self.request_params = merge_request_params(&self.request_params, &params);
        }
        self.output_functions.extend(bundle.output_functions());
        self.output_validators.extend(bundle.output_validators());
        self.history_processors.extend(bundle.history_processors());
        if let Some(limits) = bundle.usage_limits() {
            self.usage_limits = Some(limits);
        }
    }
}
