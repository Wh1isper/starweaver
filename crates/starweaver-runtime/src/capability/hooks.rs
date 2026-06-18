//! Agent capability hook interface.

use async_trait::async_trait;
use starweaver_context::AgentContext;
use starweaver_model::{
    ModelMessage, ModelRequest, ModelResponse, ModelSettings, ToolCallPart, ToolDefinition,
    ToolReturnPart,
};
use starweaver_tools::ToolContext;

use crate::{
    agent::AgentInput, executor::AgentCheckpoint, run::AgentRunState, stream::AgentStreamRecord,
};

use super::{CapabilityResult, CapabilitySpec, RetryEventKind};

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

    /// Called after run state is initialized and before the first model request is built.
    async fn prepare_run_input(
        &self,
        _state: &mut AgentRunState,
        input: AgentInput,
    ) -> CapabilityResult<AgentInput> {
        Ok(input)
    }

    /// Context-aware run-input preparation hook.
    async fn prepare_run_input_with_context(
        &self,
        state: &mut AgentRunState,
        _context: &mut AgentContext,
        input: AgentInput,
    ) -> CapabilityResult<AgentInput> {
        self.prepare_run_input(state, input).await
    }

    /// Called after message history is assembled and before provider-bound preparation/model call.
    ///
    /// Mutations from this hook are captured in canonical session history. Use this hook
    /// for model-visible context that must remain part of future request prefixes; use
    /// `prepare_provider_messages` for provider-only transient rewrites. The runtime
    /// context injector is implemented as a built-in capability in this canonical pipeline.
    async fn prepare_model_messages(
        &self,
        _state: &mut AgentRunState,
        messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        Ok(messages)
    }

    /// Context-aware model-message preparation hook.
    ///
    /// Mutations from this hook are captured in canonical session history. Use this hook
    /// for model-visible context that must remain part of future request prefixes; use
    /// `prepare_provider_messages_with_context` for provider-only transient rewrites.
    async fn prepare_model_messages_with_context(
        &self,
        state: &mut AgentRunState,
        _context: &mut AgentContext,
        messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        self.prepare_model_messages(state, messages).await
    }

    /// Called after canonical model-message capabilities and durable history capture, before the model call.
    ///
    /// Mutations from this hook are provider-bound only and are not copied back into the
    /// session message history.
    async fn prepare_provider_messages(
        &self,
        _state: &mut AgentRunState,
        messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        Ok(messages)
    }

    /// Context-aware provider-bound message preparation hook.
    async fn prepare_provider_messages_with_context(
        &self,
        state: &mut AgentRunState,
        _context: &mut AgentContext,
        messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        self.prepare_provider_messages(state, messages).await
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
