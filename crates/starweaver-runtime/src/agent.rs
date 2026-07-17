//! Bare agent runtime.

use std::{collections::BTreeSet, sync::Arc};

use starweaver_context::{AgentContext, AgentInfo, ModelConfig, ToolConfig};
use starweaver_core::{AgentId, CancellationToken};
use starweaver_model::{ModelAdapter, ModelRequestParameters, ModelSettings, OutputMode};
use starweaver_tools::{DynToolset, ToolRegistry};

use starweaver_usage::UsageLimits;

use crate::{
    agent::helpers::merge_request_params,
    capability::{AgentCapability, CapabilityBundle, resolve_capability_order},
    executor::{DirectAgentExecutor, DynAgentExecutor},
    graph::{AgentGraphTrace, AgentNode, GraphError, inspect_graph},
    instructions::DynDynamicInstruction,
    output::{
        DynOutputFunction, OutputPolicy, OutputSchema, OutputValidator, SchemaOutputFunction,
    },
    trace::{DynTraceRecorder, NoopTraceRecorder},
};

mod helpers;
mod overrides;
mod run_loop;
mod run_loop_helpers;
mod runtime_helpers;
mod types;

pub use overrides::AgentOverride;
pub use types::{
    AgentEndStrategy, AgentError, AgentInput, AgentResult, AgentRuntimePolicy,
    AgentToolExecutionMode,
};

/// Minimal agent builder/runtime.
#[derive(Clone)]
pub struct Agent {
    default_id: AgentId,
    default_name: String,
    model: Arc<dyn ModelAdapter>,
    instructions: Vec<String>,
    dynamic_instructions: Vec<DynDynamicInstruction>,
    model_settings: Option<ModelSettings>,
    request_params: ModelRequestParameters,
    output_schema: Option<OutputSchema>,
    output_validators: Vec<Arc<dyn OutputValidator>>,
    output_functions: Vec<DynOutputFunction>,
    usage_limits: Option<UsageLimits>,
    tools: ToolRegistry,
    denied_tool_names: BTreeSet<String>,
    toolsets: Vec<DynToolset>,
    capabilities: Vec<Arc<dyn AgentCapability>>,
    stream_observers: Vec<Arc<dyn AgentCapability>>,
    cancellation_token: Option<CancellationToken>,
    executor: DynAgentExecutor,
    trace_recorder: DynTraceRecorder,
    policy: AgentRuntimePolicy,
    model_config: Option<ModelConfig>,
    tool_config: Option<ToolConfig>,
}

impl Agent {
    /// Create an agent with a model adapter.
    #[must_use]
    pub fn new(model: Arc<dyn ModelAdapter>) -> Self {
        Self {
            default_id: AgentId::default(),
            default_name: AgentId::default().as_str().to_string(),
            model,
            instructions: Vec::new(),
            dynamic_instructions: Vec::new(),
            model_settings: None,
            request_params: ModelRequestParameters::default(),
            output_schema: None,
            output_validators: Vec::new(),
            output_functions: Vec::new(),
            usage_limits: None,
            tools: ToolRegistry::new(),
            denied_tool_names: BTreeSet::new(),
            toolsets: Vec::new(),
            capabilities: Vec::new(),
            stream_observers: Vec::new(),
            cancellation_token: None,
            executor: Arc::new(DirectAgentExecutor),
            trace_recorder: Arc::new(NoopTraceRecorder),
            policy: AgentRuntimePolicy::default(),
            model_config: None,
            tool_config: None,
        }
    }

    /// Return the default agent id used when this agent creates a context.
    #[must_use]
    pub const fn agent_id(&self) -> &AgentId {
        &self.default_id
    }

    /// Return the default human-readable agent name.
    #[must_use]
    pub fn agent_name(&self) -> &str {
        &self.default_name
    }

    /// Set the default agent id used when this agent creates a context.
    #[must_use]
    pub fn with_agent_id(mut self, agent_id: AgentId) -> Self {
        self.default_id = agent_id;
        self
    }

    /// Set the default human-readable agent name.
    #[must_use]
    pub fn with_agent_name(mut self, agent_name: impl Into<String>) -> Self {
        self.default_name = agent_name.into();
        self
    }

    /// Set both the default agent id and human-readable agent name.
    #[must_use]
    pub fn with_agent_identity(mut self, agent_id: AgentId, agent_name: impl Into<String>) -> Self {
        self.default_id = agent_id;
        self.default_name = agent_name.into();
        self
    }

    /// Create a fresh context using this agent's configured identity.
    #[must_use]
    pub fn new_context(&self) -> AgentContext {
        let mut context = AgentContext::new(self.default_id.clone());
        self.apply_context_identity(&mut context);
        context
    }

    fn apply_context_identity(&self, context: &mut AgentContext) {
        context.agent_registry.insert(
            self.default_id.as_str().to_string(),
            AgentInfo::new(self.default_id.as_str(), self.default_name.clone()),
        );
        if self.default_name != self.default_id.as_str() {
            context.metadata.insert(
                "agent_name".to_string(),
                serde_json::json!(self.default_name.as_str()),
            );
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

    /// Add one runtime toolset that is materialized for each agent context.
    #[must_use]
    pub fn with_toolset(mut self, toolset: DynToolset) -> Self {
        self.toolsets.push(toolset);
        self
    }

    /// Add many runtime toolsets that are materialized for each agent context.
    #[must_use]
    pub fn with_toolsets(mut self, toolsets: impl IntoIterator<Item = DynToolset>) -> Self {
        self.toolsets.extend(toolsets);
        self
    }

    /// Merge additional runtime tools into this agent.
    #[must_use]
    pub fn with_appended_tools(mut self, tools: &ToolRegistry) -> Self {
        self.tools.insert_registry(tools);
        self
    }

    /// Deny tool names after all static, dynamic, and capability toolsets are prepared.
    #[must_use]
    pub fn with_denied_tool_names(
        mut self,
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.denied_tool_names
            .extend(names.into_iter().map(Into::into));
        self
    }

    /// Return a clone of the runtime tool registry.
    #[must_use]
    pub fn tools(&self) -> ToolRegistry {
        let mut tools = self.tools.clone();
        for toolset in &self.toolsets {
            tools.insert_toolset(toolset);
        }
        for name in &self.denied_tool_names {
            tools.remove(name);
        }
        tools
    }

    /// Prepare this agent's static tools and context-aware toolsets for a concrete context.
    ///
    /// This uses the same lifecycle-aware path as the normal run loop and is intended for
    /// host operations that need to execute a previously suspended tool call.
    ///
    /// # Errors
    ///
    /// Returns an agent error when a context-aware toolset cannot be prepared.
    pub async fn prepare_tools_for_context(
        &self,
        context: &mut AgentContext,
    ) -> Result<ToolRegistry, AgentError> {
        self.prepare_run_tools(context, true).await
    }

    /// Close context-aware toolsets after host-side execution outside the normal run loop.
    pub async fn close_toolsets_for_context(&self, context: &mut AgentContext) {
        self.close_run_toolsets(context).await;
    }

    /// Set the agent-level retry default for runtime tools.
    #[must_use]
    pub const fn with_tool_retries(mut self, max_retries: usize) -> Self {
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
        let (schema, validators, functions, retries, mode, allow_text_output, allow_image_output) =
            policy.into_parts();
        let schema_output_function = match (schema.as_ref(), mode) {
            (Some(schema), Some(OutputMode::Tool | OutputMode::ToolOrText)) => {
                Some(Arc::new(SchemaOutputFunction::new(schema.clone())) as DynOutputFunction)
            }
            _ => None,
        };
        if let Some(schema) = schema {
            self.output_schema = Some(schema);
        }
        self.output_validators.extend(validators);
        if let Some(function) = schema_output_function {
            self.output_functions.push(function);
        }
        self.output_functions.extend(functions);
        if let Some(retries) = retries {
            self.policy.output_retries = retries;
        }
        if let Some(mode) = mode {
            self.request_params.output_mode = Some(mode);
        }
        if let Some(allow_text_output) = allow_text_output {
            self.request_params.allow_text_output = Some(allow_text_output);
        }
        if let Some(allow_image_output) = allow_image_output {
            self.request_params.allow_image_output = Some(allow_image_output);
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

    /// Set the full model config exposed to `AgentContext`.
    #[must_use]
    pub fn with_model_config(mut self, model_config: ModelConfig) -> Self {
        self.model_config = Some(model_config);
        self
    }

    /// Set tool-level configuration exposed to runtime tools.
    #[must_use]
    pub fn with_tool_config(mut self, tool_config: ToolConfig) -> Self {
        self.tool_config = Some(tool_config);
        self
    }

    /// Set the model context window exposed to `AgentContext` runtime instructions.
    #[must_use]
    pub fn with_context_window(mut self, context_window: u64) -> Self {
        let mut model_config = self.model_config.unwrap_or_default();
        model_config.context_window = Some(context_window);
        self.model_config = Some(model_config);
        self
    }

    /// Add a capability hook.
    #[must_use]
    pub fn with_capability(mut self, capability: Arc<dyn AgentCapability>) -> Self {
        self.capabilities.push(capability);
        self
    }

    pub(super) fn ordered_capabilities(&self) -> Result<Vec<Arc<dyn AgentCapability>>, AgentError> {
        resolve_capability_order(&self.capabilities).map_err(AgentError::from)
    }

    pub(super) fn ordered_stream_observers(
        &self,
    ) -> Result<Vec<Arc<dyn AgentCapability>>, AgentError> {
        resolve_capability_order(&self.stream_observers).map_err(AgentError::from)
    }

    pub(super) fn ordered_capabilities_for_validation(
        &self,
    ) -> Result<Vec<Arc<dyn AgentCapability>>, crate::capability::CapabilityError> {
        resolve_capability_order(&self.capabilities)
            .map_err(|error| crate::capability::CapabilityError::Failed(error.to_string()))
    }

    /// Add a stream observer hook.
    #[must_use]
    pub fn with_stream_observer(mut self, observer: Arc<dyn AgentCapability>) -> Self {
        self.stream_observers.push(observer);
        self
    }

    /// Set a cooperative cancellation token used by streaming callers.
    #[must_use]
    pub fn with_cancellation_token(mut self, token: CancellationToken) -> Self {
        self.cancellation_token = Some(token);
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
        if let Some(limits) = bundle.usage_limits() {
            self.usage_limits = Some(limits);
        }
    }
}
