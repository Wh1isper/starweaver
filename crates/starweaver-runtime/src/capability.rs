//! Capability hooks and bundles for the bare agent runtime.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use starweaver_context::AgentContext;
use starweaver_core::Metadata;
use starweaver_model::{
    ModelMessage, ModelRequest, ModelRequestParameters, ModelResponse, ModelSettings, ToolCallPart,
    ToolDefinition, ToolReturnPart,
};
use starweaver_tools::{DynTool, DynToolset, ToolContext, ToolRegistry};
use thiserror::Error;

use crate::{
    executor::AgentCheckpoint,
    instructions::DynDynamicInstruction,
    output::{DynOutputFunction, OutputValidator},
    run::AgentRunState,
    stream::AgentStreamRecord,
    usage::UsageLimits,
};

/// Runtime capability error.
#[derive(Debug, Error)]
pub enum CapabilityError {
    /// Ask the model to retry with this prompt.
    #[error("model retry requested: {0}")]
    ModelRetry(String),
    /// Return this response without calling the model.
    #[error("model request skipped")]
    SkipModelRequest(Box<ModelResponse>),
    /// Capability hook failed.
    #[error("capability failed: {0}")]
    Failed(String),
}

/// Capability hook result.
pub type CapabilityResult<T> = Result<T, CapabilityError>;

/// Runtime retry boundary observed by capability hooks.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryEventKind {
    /// Output validation or output function validation requested another model turn.
    Output,
    /// A tool requested semantic retry through structured metadata.
    Tool,
}

/// Stable capability identifier.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct CapabilityId(String);

impl CapabilityId {
    /// Build an identifier from a string.
    #[must_use]
    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Return the identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for CapabilityId {
    fn from(value: &str) -> Self {
        Self::from_string(value)
    }
}

impl From<String> for CapabilityId {
    fn from(value: String) -> Self {
        Self::from_string(value)
    }
}

/// Capability ordering constraints.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapabilityOrdering {
    /// Capability ids that must run before this capability.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<CapabilityId>,
    /// Capability ids that must run after this capability.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<CapabilityId>,
}

impl CapabilityOrdering {
    /// Require this capability to run after another capability.
    #[must_use]
    pub fn after(mut self, id: impl Into<CapabilityId>) -> Self {
        self.after.push(id.into());
        self
    }

    /// Require this capability to run before another capability.
    #[must_use]
    pub fn before(mut self, id: impl Into<CapabilityId>) -> Self {
        self.before.push(id.into());
        self
    }
}

/// Stable capability specification used for ordering and reconstruction evidence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapabilitySpec {
    /// Stable capability id.
    pub id: CapabilityId,
    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Ordering constraints.
    #[serde(default)]
    pub ordering: CapabilityOrdering,
    /// Whether the capability can be loaded on demand by a host registry.
    #[serde(default)]
    pub on_demand: bool,
    /// Additional metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl CapabilitySpec {
    /// Build a capability spec from an id.
    #[must_use]
    pub fn new(id: impl Into<CapabilityId>) -> Self {
        Self {
            id: id.into(),
            description: None,
            ordering: CapabilityOrdering::default(),
            on_demand: false,
            metadata: Metadata::default(),
        }
    }

    /// Attach a description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Attach ordering constraints.
    #[must_use]
    pub fn with_ordering(mut self, ordering: CapabilityOrdering) -> Self {
        self.ordering = ordering;
        self
    }

    /// Mark the capability as on-demand loadable.
    #[must_use]
    pub const fn with_on_demand(mut self, on_demand: bool) -> Self {
        self.on_demand = on_demand;
        self
    }

    /// Attach metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }
}

/// Capability ordering diagnostics.
#[derive(Debug, Error)]
pub enum CapabilityOrderError {
    /// Capability ids must be unique inside one run graph.
    #[error("capability id '{0}' is duplicated")]
    DuplicateId(String),
    /// Ordering constraint referenced a missing capability id.
    #[error("capability '{capability}' references missing dependency '{dependency}'")]
    MissingDependency {
        /// Capability that declared the dependency.
        capability: String,
        /// Missing dependency id.
        dependency: String,
    },
    /// Ordering constraints contain a cycle.
    #[error("capability ordering cycle detected among {0}")]
    Cycle(String),
}

/// Resolve capability order from stable specs.
///
/// # Errors
///
/// Returns duplicate-id, missing-dependency, or cycle diagnostics.
pub fn resolve_capability_order(
    capabilities: &[Arc<dyn AgentCapability>],
) -> Result<Vec<Arc<dyn AgentCapability>>, CapabilityOrderError> {
    let mut ids = Vec::with_capacity(capabilities.len());
    let mut by_id = BTreeMap::new();
    for (index, capability) in capabilities.iter().enumerate() {
        let id = capability.spec().id;
        if by_id.insert(id.clone(), index).is_some() {
            return Err(CapabilityOrderError::DuplicateId(id.as_str().to_string()));
        }
        ids.push(id);
    }

    let mut outgoing = BTreeMap::<CapabilityId, BTreeSet<CapabilityId>>::new();
    let mut incoming = BTreeMap::<CapabilityId, usize>::new();
    for id in &ids {
        outgoing.entry(id.clone()).or_default();
        incoming.entry(id.clone()).or_default();
    }

    for (index, capability) in capabilities.iter().enumerate() {
        let spec = capability.spec();
        let current = ids[index].clone();
        for dependency in spec.ordering.after {
            if !by_id.contains_key(&dependency) {
                return Err(CapabilityOrderError::MissingDependency {
                    capability: current.as_str().to_string(),
                    dependency: dependency.as_str().to_string(),
                });
            }
            if outgoing
                .entry(dependency.clone())
                .or_default()
                .insert(current.clone())
            {
                *incoming.entry(current.clone()).or_default() += 1;
            }
        }
        for target in spec.ordering.before {
            if !by_id.contains_key(&target) {
                return Err(CapabilityOrderError::MissingDependency {
                    capability: current.as_str().to_string(),
                    dependency: target.as_str().to_string(),
                });
            }
            if outgoing
                .entry(current.clone())
                .or_default()
                .insert(target.clone())
            {
                *incoming.entry(target).or_default() += 1;
            }
        }
    }

    let mut emitted = BTreeSet::<CapabilityId>::new();
    let mut ordered = Vec::with_capacity(capabilities.len());
    while ordered.len() < capabilities.len() {
        let Some(next) = ids
            .iter()
            .find(|id| !emitted.contains(*id) && incoming.get(*id).copied().unwrap_or(0) == 0)
            .cloned()
        else {
            let cycle = ids
                .iter()
                .filter(|id| !emitted.contains(*id))
                .map(|id| id.as_str().to_string())
                .collect::<Vec<_>>()
                .join(",");
            return Err(CapabilityOrderError::Cycle(cycle));
        };
        emitted.insert(next.clone());
        let index = by_id[&next];
        ordered.push(capabilities[index].clone());
        if let Some(targets) = outgoing.get(&next) {
            for target in targets {
                if let Some(count) = incoming.get_mut(target) {
                    *count = count.saturating_sub(1);
                }
            }
        }
    }
    Ok(ordered)
}

/// Hook interface for runtime extension points.
#[async_trait]
pub trait AgentCapability: Send + Sync {
    /// Stable capability spec used for ordering and reconstruction evidence.
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new(std::any::type_name::<Self>())
    }

    /// Called after a run state is created and before the first request is prepared.
    async fn on_run_start(&self, _state: &mut AgentRunState) -> CapabilityResult<()> {
        Ok(())
    }

    /// Context-aware run-start hook.
    async fn on_run_start_with_context(
        &self,
        state: &mut AgentRunState,
        _context: &mut AgentContext,
    ) -> CapabilityResult<()> {
        self.on_run_start(state).await
    }

    /// Called after message history is assembled and before runtime context injection/model call.
    async fn prepare_model_messages(
        &self,
        _state: &mut AgentRunState,
        messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        Ok(messages)
    }

    /// Context-aware model-message preparation hook.
    async fn prepare_model_messages_with_context(
        &self,
        state: &mut AgentRunState,
        _context: &mut AgentContext,
        messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        self.prepare_model_messages(state, messages).await
    }

    /// Called after tool definitions are collected and before request parameters are finalized.
    async fn prepare_tools(
        &self,
        _state: &AgentRunState,
        tools: Vec<ToolDefinition>,
    ) -> CapabilityResult<Vec<ToolDefinition>> {
        Ok(tools)
    }

    /// Context-aware prepare-tools hook.
    async fn prepare_tools_with_context(
        &self,
        state: &AgentRunState,
        _context: &AgentContext,
        tools: Vec<ToolDefinition>,
    ) -> CapabilityResult<Vec<ToolDefinition>> {
        self.prepare_tools(state, tools).await
    }

    /// Called after a request is prepared and before the model call.
    async fn before_model_request(
        &self,
        _state: &mut AgentRunState,
        _request: &mut ModelRequest,
        _settings: &mut Option<ModelSettings>,
    ) -> CapabilityResult<()> {
        Ok(())
    }

    /// Context-aware before-model-request hook.
    async fn before_model_request_with_context(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        request: &mut ModelRequest,
        settings: &mut Option<ModelSettings>,
    ) -> CapabilityResult<()> {
        let _ = context;
        self.before_model_request(state, request, settings).await
    }

    /// Called after a model response is received.
    async fn after_model_response(
        &self,
        _state: &mut AgentRunState,
        _response: &mut ModelResponse,
    ) -> CapabilityResult<()> {
        Ok(())
    }

    /// Called before a tool call is executed.
    async fn before_tool_execution(
        &self,
        _state: &mut AgentRunState,
        _tool_context: &mut ToolContext,
        _call: &ToolCallPart,
    ) -> CapabilityResult<()> {
        Ok(())
    }

    /// Context-aware before-tool-execution hook.
    async fn before_tool_execution_with_context(
        &self,
        state: &mut AgentRunState,
        _context: &mut AgentContext,
        tool_context: &mut ToolContext,
        call: &ToolCallPart,
    ) -> CapabilityResult<()> {
        self.before_tool_execution(state, tool_context, call).await
    }

    /// Called after a tool result is produced and before it is applied to run state.
    async fn after_tool_result(
        &self,
        _state: &mut AgentRunState,
        _call: &ToolCallPart,
        _tool_return: &mut ToolReturnPart,
    ) -> CapabilityResult<()> {
        Ok(())
    }

    /// Context-aware after-tool-result hook.
    async fn after_tool_result_with_context(
        &self,
        state: &mut AgentRunState,
        _context: &mut AgentContext,
        call: &ToolCallPart,
        tool_return: &mut ToolReturnPart,
    ) -> CapabilityResult<()> {
        self.after_tool_result(state, call, tool_return).await
    }

    /// Context-aware after-model-response hook.
    async fn after_model_response_with_context(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        response: &mut ModelResponse,
    ) -> CapabilityResult<()> {
        let _ = context;
        self.after_model_response(state, response).await
    }

    /// Called before final output validation begins.
    async fn before_output_validation(
        &self,
        _state: &mut AgentRunState,
        _output: &str,
    ) -> CapabilityResult<()> {
        Ok(())
    }

    /// Context-aware before-output-validation hook.
    async fn before_output_validation_with_context(
        &self,
        state: &mut AgentRunState,
        _context: &mut AgentContext,
        output: &str,
    ) -> CapabilityResult<()> {
        self.before_output_validation(state, output).await
    }

    /// Called after output text is selected and before finalization.
    async fn validate_output(
        &self,
        _state: &mut AgentRunState,
        _output: &str,
    ) -> CapabilityResult<()> {
        Ok(())
    }

    /// Context-aware output validation hook.
    async fn validate_output_with_context(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        output: &str,
    ) -> CapabilityResult<()> {
        let _ = context;
        self.validate_output(state, output).await
    }

    /// Called after output validation accepts the output.
    async fn after_output_validation(
        &self,
        _state: &mut AgentRunState,
        _output: &str,
    ) -> CapabilityResult<()> {
        Ok(())
    }

    /// Context-aware after-output-validation hook.
    async fn after_output_validation_with_context(
        &self,
        state: &mut AgentRunState,
        _context: &mut AgentContext,
        output: &str,
    ) -> CapabilityResult<()> {
        self.after_output_validation(state, output).await
    }

    /// Called after an executor checkpoint is emitted.
    async fn on_checkpoint(
        &self,
        _state: &AgentRunState,
        _checkpoint: &AgentCheckpoint,
    ) -> CapabilityResult<()> {
        Ok(())
    }

    /// Context-aware checkpoint hook.
    async fn on_checkpoint_with_context(
        &self,
        state: &AgentRunState,
        _context: &AgentContext,
        checkpoint: &AgentCheckpoint,
    ) -> CapabilityResult<()> {
        self.on_checkpoint(state, checkpoint).await
    }

    /// Called when semantic retry is scheduled.
    async fn on_retry(
        &self,
        _state: &mut AgentRunState,
        _kind: RetryEventKind,
        _retries: usize,
        _message: &str,
    ) -> CapabilityResult<()> {
        Ok(())
    }

    /// Called after a stream event is recorded.
    async fn on_stream_event(
        &self,
        _state: &AgentRunState,
        _event: &AgentStreamRecord,
    ) -> CapabilityResult<()> {
        Ok(())
    }

    /// Context-aware stream observer hook.
    async fn on_stream_event_with_context(
        &self,
        state: &AgentRunState,
        _context: &AgentContext,
        event: &AgentStreamRecord,
    ) -> CapabilityResult<()> {
        self.on_stream_event(state, event).await
    }

    /// Context-aware retry hook.
    async fn on_retry_with_context(
        &self,
        state: &mut AgentRunState,
        _context: &mut AgentContext,
        kind: RetryEventKind,
        retries: usize,
        message: &str,
    ) -> CapabilityResult<()> {
        self.on_retry(state, kind, retries, message).await
    }

    /// Called when a run completes.
    async fn on_run_complete(&self, _state: &mut AgentRunState) -> CapabilityResult<()> {
        Ok(())
    }

    /// Context-aware run-complete hook.
    async fn on_run_complete_with_context(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
    ) -> CapabilityResult<()> {
        let _ = context;
        self.on_run_complete(state).await
    }
}

/// Composable agent extension that contributes hooks, tools, instructions, and settings.
pub trait CapabilityBundle: Send + Sync {
    /// Bundle name for diagnostics and registry surfaces.
    fn name(&self) -> &str;

    /// Stable capability spec for bundle-level reconstruction evidence.
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new(self.name())
    }

    /// Capability hooks contributed by this bundle.
    fn hooks(&self) -> Vec<Arc<dyn AgentCapability>> {
        Vec::new()
    }

    /// Stream observer hooks contributed by this bundle.
    fn stream_observers(&self) -> Vec<Arc<dyn AgentCapability>> {
        Vec::new()
    }

    /// Static instructions contributed by this bundle.
    fn get_instructions(&self) -> Vec<String> {
        Vec::new()
    }

    /// Dynamic instructions contributed by this bundle.
    fn dynamic_instructions(&self) -> Vec<DynDynamicInstruction> {
        Vec::new()
    }

    /// Runtime tools and tool instructions contributed by this bundle.
    fn get_tools(&self) -> Option<ToolRegistry> {
        None
    }

    /// Model settings overlay contributed by this bundle.
    fn model_settings(&self) -> Option<ModelSettings> {
        None
    }

    /// Request parameter overlay contributed by this bundle.
    fn request_params(&self) -> Option<ModelRequestParameters> {
        None
    }

    /// Output functions contributed by this bundle.
    fn output_functions(&self) -> Vec<DynOutputFunction> {
        Vec::new()
    }

    /// Output validators contributed by this bundle.
    fn output_validators(&self) -> Vec<Arc<dyn OutputValidator>> {
        Vec::new()
    }

    /// Usage limits contributed by this bundle.
    fn usage_limits(&self) -> Option<UsageLimits> {
        None
    }
}

/// Static capability bundle for reusable runtime composition.
#[derive(Clone, Default)]
pub struct StaticCapabilityBundle {
    name: String,
    spec: Option<CapabilitySpec>,
    hooks: Vec<Arc<dyn AgentCapability>>,
    stream_observers: Vec<Arc<dyn AgentCapability>>,
    instructions: Vec<String>,
    dynamic_instructions: Vec<DynDynamicInstruction>,
    tools: ToolRegistry,
    model_settings: Option<ModelSettings>,
    request_params: Option<ModelRequestParameters>,
    output_functions: Vec<DynOutputFunction>,
    output_validators: Vec<Arc<dyn OutputValidator>>,
    usage_limits: Option<UsageLimits>,
}

impl StaticCapabilityBundle {
    /// Create an empty static capability bundle.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            spec: None,
            hooks: Vec::new(),
            stream_observers: Vec::new(),
            instructions: Vec::new(),
            dynamic_instructions: Vec::new(),
            tools: ToolRegistry::new(),
            model_settings: None,
            request_params: None,
            output_functions: Vec::new(),
            output_validators: Vec::new(),
            usage_limits: None,
        }
    }

    /// Set the bundle capability spec.
    #[must_use]
    pub fn with_spec(mut self, spec: CapabilitySpec) -> Self {
        self.spec = Some(spec);
        self
    }

    /// Add a capability hook.
    #[must_use]
    pub fn with_hook(mut self, hook: Arc<dyn AgentCapability>) -> Self {
        self.hooks.push(hook);
        self
    }

    /// Add a stream observer hook.
    #[must_use]
    pub fn with_stream_observer(mut self, observer: Arc<dyn AgentCapability>) -> Self {
        self.stream_observers.push(observer);
        self
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

    /// Add one tool.
    #[must_use]
    pub fn with_tool(mut self, tool: DynTool) -> Self {
        self.tools.insert(tool);
        self
    }

    /// Add one toolset.
    #[must_use]
    pub fn with_toolset(mut self, toolset: &DynToolset) -> Self {
        self.tools.insert_toolset(toolset);
        self
    }

    /// Set model settings overlay.
    #[must_use]
    pub fn with_model_settings(mut self, settings: ModelSettings) -> Self {
        self.model_settings = Some(settings);
        self
    }

    /// Set request parameter overlay.
    #[must_use]
    pub fn with_request_params(mut self, params: ModelRequestParameters) -> Self {
        self.request_params = Some(params);
        self
    }

    /// Add an output function.
    #[must_use]
    pub fn with_output_function(mut self, function: DynOutputFunction) -> Self {
        self.output_functions.push(function);
        self
    }

    /// Add an output validator.
    #[must_use]
    pub fn with_output_validator(mut self, validator: Arc<dyn OutputValidator>) -> Self {
        self.output_validators.push(validator);
        self
    }

    /// Set usage limits overlay.
    #[must_use]
    pub const fn with_usage_limits(mut self, limits: UsageLimits) -> Self {
        self.usage_limits = Some(limits);
        self
    }
}

impl CapabilityBundle for StaticCapabilityBundle {
    fn name(&self) -> &str {
        &self.name
    }

    fn spec(&self) -> CapabilitySpec {
        self.spec
            .clone()
            .unwrap_or_else(|| CapabilitySpec::new(self.name.clone()))
    }

    fn hooks(&self) -> Vec<Arc<dyn AgentCapability>> {
        self.hooks.clone()
    }

    fn stream_observers(&self) -> Vec<Arc<dyn AgentCapability>> {
        self.stream_observers.clone()
    }

    fn get_instructions(&self) -> Vec<String> {
        self.instructions.clone()
    }

    fn dynamic_instructions(&self) -> Vec<DynDynamicInstruction> {
        self.dynamic_instructions.clone()
    }

    fn get_tools(&self) -> Option<ToolRegistry> {
        if self.tools.is_empty() && self.tools.get_instructions().is_empty() {
            None
        } else {
            Some(self.tools.clone())
        }
    }

    fn model_settings(&self) -> Option<ModelSettings> {
        self.model_settings.clone()
    }

    fn request_params(&self) -> Option<ModelRequestParameters> {
        self.request_params.clone()
    }

    fn output_functions(&self) -> Vec<DynOutputFunction> {
        self.output_functions.clone()
    }

    fn output_validators(&self) -> Vec<Arc<dyn OutputValidator>> {
        self.output_validators.clone()
    }

    fn usage_limits(&self) -> Option<UsageLimits> {
        self.usage_limits.clone()
    }
}
