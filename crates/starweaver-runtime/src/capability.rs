//! Capability hooks and bundles for the bare agent runtime.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use starweaver_context::AgentContext;
use starweaver_model::{
    ModelRequest, ModelRequestParameters, ModelResponse, ModelSettings, ToolCallPart,
    ToolDefinition, ToolReturnPart,
};
use starweaver_tools::{DynTool, DynToolset, ToolContext, ToolRegistry};
use thiserror::Error;

use crate::{
    executor::AgentCheckpoint,
    history::HistoryProcessor,
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

/// Hook interface for runtime extension points.
#[async_trait]
pub trait AgentCapability: Send + Sync {
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

/// Composable agent extension that contributes hooks, tools, instructions, settings, and processors.
pub trait CapabilityBundle: Send + Sync {
    /// Bundle name for diagnostics and registry surfaces.
    fn name(&self) -> &str;

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

    /// History processors contributed by this bundle.
    fn history_processors(&self) -> Vec<Arc<dyn HistoryProcessor>> {
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
    hooks: Vec<Arc<dyn AgentCapability>>,
    stream_observers: Vec<Arc<dyn AgentCapability>>,
    instructions: Vec<String>,
    dynamic_instructions: Vec<DynDynamicInstruction>,
    tools: ToolRegistry,
    model_settings: Option<ModelSettings>,
    request_params: Option<ModelRequestParameters>,
    output_functions: Vec<DynOutputFunction>,
    output_validators: Vec<Arc<dyn OutputValidator>>,
    history_processors: Vec<Arc<dyn HistoryProcessor>>,
    usage_limits: Option<UsageLimits>,
}

impl StaticCapabilityBundle {
    /// Create an empty static capability bundle.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            hooks: Vec::new(),
            stream_observers: Vec::new(),
            instructions: Vec::new(),
            dynamic_instructions: Vec::new(),
            tools: ToolRegistry::new(),
            model_settings: None,
            request_params: None,
            output_functions: Vec::new(),
            output_validators: Vec::new(),
            history_processors: Vec::new(),
            usage_limits: None,
        }
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

    /// Add a history processor.
    #[must_use]
    pub fn with_history_processor(mut self, processor: Arc<dyn HistoryProcessor>) -> Self {
        self.history_processors.push(processor);
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

    fn history_processors(&self) -> Vec<Arc<dyn HistoryProcessor>> {
        self.history_processors.clone()
    }

    fn usage_limits(&self) -> Option<UsageLimits> {
        self.usage_limits.clone()
    }
}
