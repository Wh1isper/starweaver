//! Agent runtime helper methods.

use std::collections::BTreeSet;

use starweaver_context::AgentContext;
use starweaver_core::{ConversationId, RunId};
use starweaver_model::{
    ModelMessage, ModelRequest, ModelRequestParameters, ModelRequestPart, ModelResponse,
    ModelSettings, ToolDefinition,
};

use crate::{
    agent::{Agent, AgentError},
    capability::{CapabilityError, RetryEventKind},
    executor::{AgentCheckpoint, AgentExecutionDecision, AgentExecutionNode},
    history::HistoryProcessorError,
    instructions::DynamicInstructionError,
    output::{
        parse_output, OutputFunctionContext, OutputSchema, OutputValidationError, OutputValue,
    },
    run::AgentRunState,
};

impl Agent {
    pub(super) async fn prepare_request(
        &self,
        state: &AgentRunState,
        prompt: &str,
        run_id: &RunId,
        conversation_id: &ConversationId,
    ) -> Result<ModelRequest, AgentError> {
        let mut parts = Vec::new();
        if state.message_history.is_empty() {
            let dynamic_instructions = self.dynamic_instructions(state).await?;
            parts.extend(self.instructions.iter().map(|instruction| {
                ModelRequestPart::SystemPrompt {
                    text: instruction.clone(),
                    metadata: serde_json::Map::new(),
                }
            }));
            parts.extend(dynamic_instructions.into_iter().map(|instruction| {
                ModelRequestPart::Instruction {
                    text: instruction,
                    metadata: serde_json::Map::new(),
                }
            }));
            parts.extend(self.tools.instructions().into_iter().map(|instruction| {
                ModelRequestPart::Instruction {
                    text: instruction,
                    metadata: serde_json::Map::new(),
                }
            }));
        }
        if !state.pending_tool_returns.is_empty() {
            parts.extend(
                state
                    .pending_tool_returns
                    .iter()
                    .cloned()
                    .map(ModelRequestPart::ToolReturn),
            );
        } else if state.run_step == 0 {
            parts.push(ModelRequestPart::UserPrompt {
                content: vec![starweaver_model::ContentPart::Text {
                    text: prompt.to_string(),
                }],
                name: None,
                metadata: serde_json::Map::new(),
            });
        } else {
            parts.push(ModelRequestPart::RetryPrompt {
                text: prompt.to_string(),
                tool_call_id: None,
                metadata: serde_json::Map::new(),
            });
        }
        Ok(ModelRequest {
            parts,
            timestamp: None,
            instructions: None,
            run_id: Some(run_id.clone()),
            conversation_id: Some(conversation_id.clone()),
            metadata: serde_json::Map::new(),
        })
    }

    pub(super) async fn checkpoint(
        &self,
        node: AgentExecutionNode,
        state: &AgentRunState,
        context: &AgentContext,
    ) -> Result<AgentExecutionDecision, AgentError> {
        let mut checkpoint = AgentCheckpoint::new(node, state);
        checkpoint.resume.trace_context = context.trace_context.clone();
        for capability in &self.capabilities {
            capability
                .on_checkpoint_with_context(state, context, &checkpoint)
                .await
                .map_err(Self::capability_error)?;
        }
        Ok(self.executor.checkpoint(checkpoint).await?)
    }

    pub(super) async fn dynamic_instructions(
        &self,
        state: &AgentRunState,
    ) -> Result<Vec<String>, AgentError> {
        let mut instructions = Vec::new();
        for instruction in &self.dynamic_instructions {
            instructions.push(
                instruction
                    .instruction(state)
                    .await
                    .map_err(Self::dynamic_instruction_error)?,
            );
        }
        Ok(instructions)
    }

    pub(super) fn effective_settings(&self) -> Option<ModelSettings> {
        match (self.model.default_settings(), &self.model_settings) {
            (Some(defaults), Some(settings)) => Some(defaults.merge(settings)),
            (Some(defaults), None) => Some(defaults.clone()),
            (None, Some(settings)) => Some(settings.clone()),
            (None, None) => None,
        }
    }

    pub(super) fn check_before_request(&self, state: &AgentRunState) -> Result<(), AgentError> {
        if let Some(limits) = &self.usage_limits {
            limits.check_before_request(&state.usage)?;
        }
        Ok(())
    }

    pub(super) fn check_usage(&self, state: &AgentRunState) -> Result<(), AgentError> {
        if let Some(limits) = &self.usage_limits {
            limits.check_usage(&state.usage)?;
        }
        Ok(())
    }

    pub(super) fn check_tool_calls(
        &self,
        state: &AgentRunState,
        additional_successful_tool_calls: u64,
    ) -> Result<(), AgentError> {
        if let Some(limits) = &self.usage_limits {
            let projected = state
                .usage
                .clone()
                .with_additional_tool_calls(additional_successful_tool_calls);
            limits.check_tool_calls(&projected)?;
        }
        Ok(())
    }

    pub(super) async fn process_history(
        &self,
        state: &AgentRunState,
    ) -> Result<Vec<ModelMessage>, AgentError> {
        let mut messages = state.message_history.clone();
        for processor in &self.history_processors {
            messages = processor
                .process(state, messages)
                .await
                .map_err(Self::history_processor_error)?;
        }
        Ok(messages)
    }

    pub(super) async fn effective_request_params(
        &self,
        state: &AgentRunState,
        context: &AgentContext,
    ) -> Result<ModelRequestParameters, AgentError> {
        let mut params = self.request_params.clone();
        if params.output_schema.is_none() {
            params.output_schema = self
                .output_schema
                .as_ref()
                .map(OutputSchema::request_schema);
        }
        let mut names = params
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<BTreeSet<_>>();
        for function in &self.output_functions {
            let tool = function.definition().tool_definition();
            if names.insert(tool.name.clone()) {
                params.tools.push(tool);
            }
        }
        for tool in self.tools.definitions() {
            if names.insert(tool.name.clone()) {
                params.tools.push(tool);
            }
        }
        params.tools = self.prepare_tools(state, context, params.tools).await?;
        Ok(params)
    }

    pub(super) async fn call_run_start(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
    ) -> Result<(), AgentError> {
        for capability in &self.capabilities {
            capability
                .on_run_start_with_context(state, context)
                .await
                .map_err(Self::capability_error)?;
        }
        Ok(())
    }

    pub(super) async fn prepare_tools(
        &self,
        state: &AgentRunState,
        context: &AgentContext,
        mut tools: Vec<ToolDefinition>,
    ) -> Result<Vec<ToolDefinition>, AgentError> {
        for capability in &self.capabilities {
            tools = capability
                .prepare_tools_with_context(state, context, tools)
                .await
                .map_err(Self::capability_error)?;
        }
        Ok(tools)
    }

    pub(super) async fn call_before_model_request(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        request: &mut ModelRequest,
        settings: &mut Option<ModelSettings>,
    ) -> Result<Option<ModelResponse>, AgentError> {
        for capability in &self.capabilities {
            match capability
                .before_model_request_with_context(state, context, request, settings)
                .await
            {
                Ok(()) => {}
                Err(CapabilityError::SkipModelRequest(response)) => {
                    return Ok(Some(response));
                }
                Err(error) => return Err(Self::capability_error(error)),
            }
        }
        Ok(None)
    }

    pub(super) async fn call_after_model_response(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        response: &mut ModelResponse,
    ) -> Result<(), AgentError> {
        for capability in &self.capabilities {
            capability
                .after_model_response_with_context(state, context, response)
                .await
                .map_err(Self::capability_error)?;
        }
        Ok(())
    }

    pub(super) async fn call_before_tool_execution(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        tool_context: &mut starweaver_tools::ToolContext,
        call: &starweaver_model::ToolCallPart,
    ) -> Result<(), AgentError> {
        for capability in &self.capabilities {
            capability
                .before_tool_execution_with_context(state, context, tool_context, call)
                .await
                .map_err(Self::capability_error)?;
        }
        Ok(())
    }

    pub(super) async fn call_after_tool_result(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        call: &starweaver_model::ToolCallPart,
        tool_return: &mut starweaver_model::ToolReturnPart,
    ) -> Result<(), AgentError> {
        for capability in &self.capabilities {
            capability
                .after_tool_result_with_context(state, context, call, tool_return)
                .await
                .map_err(Self::capability_error)?;
        }
        Ok(())
    }

    pub(super) async fn try_call_output_function(
        &self,
        state: &AgentRunState,
        calls: &[starweaver_model::ToolCallPart],
    ) -> Result<Option<(String, Option<serde_json::Value>)>, CapabilityError> {
        let Some(call) = calls.iter().find(|call| {
            self.output_functions
                .iter()
                .any(|function| function.definition().name == call.name)
        }) else {
            return Ok(None);
        };
        let function = self
            .output_functions
            .iter()
            .find(|function| function.definition().name == call.name)
            .ok_or_else(|| {
                CapabilityError::Failed(format!("missing output function {}", call.name))
            })?;
        match function
            .call(
                OutputFunctionContext {
                    state: state.clone(),
                },
                call.arguments.clone(),
            )
            .await
            .map_err(Self::output_validation_error)
        {
            Ok(output) => Ok(Some((output.as_text(), output.as_json().cloned()))),
            Err(error) => Err(error),
        }
    }

    pub(super) async fn validate_final_output(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        output: &str,
    ) -> Result<(), CapabilityError> {
        self.call_before_output_validation(state, context, output)
            .await?;
        let parsed = parse_output(output, self.output_schema.as_ref())
            .map_err(Self::output_validation_error)?;
        state.structured_output = parsed.as_json().cloned();
        self.call_output_validators(state, &parsed).await?;
        self.call_validate_output(state, context, output).await?;
        self.call_after_output_validation(state, context, output)
            .await
    }

    pub(super) async fn call_before_output_validation(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        output: &str,
    ) -> Result<(), CapabilityError> {
        for capability in &self.capabilities {
            capability
                .before_output_validation_with_context(state, context, output)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn call_output_validators(
        &self,
        state: &mut AgentRunState,
        output: &OutputValue,
    ) -> Result<(), CapabilityError> {
        for validator in &self.output_validators {
            validator
                .validate(state, output)
                .await
                .map_err(Self::output_validation_error)?;
        }
        Ok(())
    }

    pub(super) async fn call_validate_output(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        output: &str,
    ) -> Result<(), CapabilityError> {
        for capability in &self.capabilities {
            capability
                .validate_output_with_context(state, context, output)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn call_after_output_validation(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        output: &str,
    ) -> Result<(), CapabilityError> {
        for capability in &self.capabilities {
            capability
                .after_output_validation_with_context(state, context, output)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn call_retry(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        kind: RetryEventKind,
        retries: usize,
        message: &str,
    ) -> Result<(), AgentError> {
        for capability in &self.capabilities {
            capability
                .on_retry_with_context(state, context, kind, retries, message)
                .await
                .map_err(Self::capability_error)?;
        }
        Ok(())
    }

    pub(super) async fn call_stream_observers(
        &self,
        state: &AgentRunState,
        context: &AgentContext,
        event: &crate::stream::AgentStreamRecord,
    ) -> Result<(), AgentError> {
        for observer in &self.stream_observers {
            observer
                .on_stream_event_with_context(state, context, event)
                .await
                .map_err(Self::capability_error)?;
        }
        Ok(())
    }

    pub(super) async fn call_run_complete(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
    ) -> Result<(), AgentError> {
        for capability in &self.capabilities {
            capability
                .on_run_complete_with_context(state, context)
                .await
                .map_err(Self::capability_error)?;
        }
        Ok(())
    }

    pub(super) fn history_processor_error(error: HistoryProcessorError) -> AgentError {
        match error {
            HistoryProcessorError::Failed(message) => AgentError::Capability(message),
        }
    }

    pub(super) fn dynamic_instruction_error(error: DynamicInstructionError) -> AgentError {
        match error {
            DynamicInstructionError::Failed(message) => AgentError::DynamicInstruction(message),
        }
    }

    pub(super) fn output_validation_error(error: OutputValidationError) -> CapabilityError {
        match error {
            OutputValidationError::InvalidJson(message)
            | OutputValidationError::Schema(message)
            | OutputValidationError::Retry(message) => CapabilityError::ModelRetry(message),
            OutputValidationError::Failed(message) => CapabilityError::Failed(message),
        }
    }

    pub(super) fn capability_error(error: CapabilityError) -> AgentError {
        match error {
            CapabilityError::ModelRetry(message) => AgentError::Capability(format!(
                "unexpected retry outside output validation: {message}"
            )),
            CapabilityError::SkipModelRequest(_) => {
                AgentError::Capability("unexpected skip model request".to_string())
            }
            CapabilityError::Failed(message) => AgentError::Capability(message),
        }
    }
}
