//! Agent capability hook dispatch helpers.

use starweaver_context::AgentContext;
use starweaver_model::{ModelRequest, ModelResponse, ModelSettings, ToolDefinition};

use crate::{
    agent::{Agent, AgentError},
    capability::{CapabilityError, RetryEventKind},
    run::AgentRunState,
};

impl Agent {
    pub(in crate::agent) async fn call_run_start(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
    ) -> Result<(), AgentError> {
        for capability in &self.ordered_capabilities()? {
            capability
                .on_run_start_with_context(state, context)
                .await
                .map_err(Self::capability_error)?;
        }
        Ok(())
    }

    pub(in crate::agent) async fn prepare_tools(
        &self,
        state: &AgentRunState,
        context: &AgentContext,
        mut tools: Vec<ToolDefinition>,
    ) -> Result<Vec<ToolDefinition>, AgentError> {
        for capability in &self.ordered_capabilities()? {
            tools = capability
                .prepare_tools_with_context(state, context, tools)
                .await
                .map_err(Self::capability_error)?;
        }
        Ok(tools)
    }

    pub(in crate::agent) async fn call_before_model_request(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        request: &mut ModelRequest,
        settings: &mut Option<ModelSettings>,
    ) -> Result<Option<ModelResponse>, AgentError> {
        for capability in &self.ordered_capabilities()? {
            match capability
                .before_model_request_with_context(state, context, request, settings)
                .await
            {
                Ok(()) => {}
                Err(CapabilityError::SkipModelRequest(response)) => {
                    return Ok(Some(*response));
                }
                Err(error) => return Err(Self::capability_error(error)),
            }
        }
        Ok(None)
    }

    pub(in crate::agent) async fn call_after_model_response(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        response: &mut ModelResponse,
    ) -> Result<(), AgentError> {
        for capability in &self.ordered_capabilities()? {
            capability
                .after_model_response_with_context(state, context, response)
                .await
                .map_err(Self::capability_error)?;
        }
        Ok(())
    }

    pub(in crate::agent) async fn call_before_tool_execution(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        tool_context: &mut starweaver_tools::ToolContext,
        call: &starweaver_model::ToolCallPart,
    ) -> Result<(), AgentError> {
        for capability in &self.ordered_capabilities()? {
            capability
                .before_tool_execution_with_context(state, context, tool_context, call)
                .await
                .map_err(Self::capability_error)?;
        }
        Ok(())
    }

    pub(in crate::agent) async fn call_after_tool_result(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        call: &starweaver_model::ToolCallPart,
        tool_return: &mut starweaver_model::ToolReturnPart,
    ) -> Result<(), AgentError> {
        for capability in &self.ordered_capabilities()? {
            capability
                .after_tool_result_with_context(state, context, call, tool_return)
                .await
                .map_err(Self::capability_error)?;
        }
        Ok(())
    }

    pub(in crate::agent) async fn call_retry(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        kind: RetryEventKind,
        retries: usize,
        message: &str,
    ) -> Result<(), AgentError> {
        for capability in &self.ordered_capabilities()? {
            capability
                .on_retry_with_context(state, context, kind, retries, message)
                .await
                .map_err(Self::capability_error)?;
        }
        Ok(())
    }

    pub(in crate::agent) async fn call_stream_observers(
        &self,
        state: &AgentRunState,
        context: &AgentContext,
        event: &crate::stream::AgentStreamRecord,
    ) -> Result<(), AgentError> {
        for observer in &self.ordered_stream_observers()? {
            observer
                .on_stream_event_with_context(state, context, event)
                .await
                .map_err(Self::capability_error)?;
        }
        Ok(())
    }

    pub(in crate::agent) async fn call_run_complete(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
    ) -> Result<(), AgentError> {
        for capability in &self.ordered_capabilities()? {
            capability
                .on_run_complete_with_context(state, context)
                .await
                .map_err(Self::capability_error)?;
        }
        Ok(())
    }
}
