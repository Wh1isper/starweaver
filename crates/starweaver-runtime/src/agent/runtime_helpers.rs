//! Agent runtime helper methods.

use std::collections::BTreeSet;

use starweaver_context::{AgentContext, AgentContextHandle, AgentEvent, BusMessage};
use starweaver_core::{ConversationId, RunId};
use starweaver_model::{
    ContentPart, ModelMessage, ModelRequest, ModelRequestParameters, ModelRequestPart,
    ModelResponse, ModelResponseStreamEvent, ModelSettings, PreparedInstruction, ToolDefinition,
};

use crate::{
    agent::{Agent, AgentError},
    capability::{CapabilityError, RetryEventKind},
    executor::{AgentCheckpoint, AgentExecutionDecision, AgentExecutionNode},
    instructions::DynamicInstructionError,
    output::{
        parse_output, OutputFunctionContext, OutputSchema, OutputValidationError, OutputValue,
    },
    run::AgentRunState,
    trace::{SpanEvent, SpanSpec, SpanStatus},
};

const STEERING_GUARD_PROMPT: &str = "<system-reminder>There are pending steering messages. Continue and incorporate them before finalizing.</system-reminder>";

struct SteeringMessage {
    id: Option<String>,
    text: String,
}

pub(super) fn tool_return_media_prompt(
    tool_return: &starweaver_model::ToolReturnPart,
) -> Option<ModelRequestPart> {
    let value = tool_return
        .private_metadata
        .get("starweaver_tool_return_content_parts")?
        .clone();
    let mut content = Vec::new();
    let prompt = tool_return
        .private_metadata
        .get("starweaver_tool_return_prompt")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map_or_else(
            || {
                format!(
                    "Tool {} returned provider-native media content.",
                    tool_return.name
                )
            },
            str::to_string,
        );
    content.push(ContentPart::Text { text: prompt });
    let mut media_parts = serde_json::from_value::<Vec<ContentPart>>(value).ok()?;
    if media_parts.is_empty() {
        return None;
    }
    content.append(&mut media_parts);
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "starweaver_instruction_origin".to_string(),
        serde_json::json!("tool_return_media"),
    );
    metadata.insert(
        "tool_call_id".to_string(),
        serde_json::json!(tool_return.tool_call_id.clone()),
    );
    metadata.insert(
        "tool_name".to_string(),
        serde_json::json!(tool_return.name.clone()),
    );
    Some(ModelRequestPart::UserPrompt {
        content,
        name: None,
        metadata,
    })
}

pub(super) fn is_steering_guard_prompt(prompt: &str) -> bool {
    prompt == STEERING_GUARD_PROMPT
}

fn sanitize_incomplete_tool_call_history(messages: Vec<ModelMessage>) -> Vec<ModelMessage> {
    if messages.is_empty() {
        return messages;
    }

    let valid_tool_call_ids = valid_tool_call_ids(&messages);
    let mut pending_tool_call_ids = BTreeSet::new();
    let mut sanitized = Vec::with_capacity(messages.len());

    for message in messages {
        match message {
            ModelMessage::Response(mut response) => {
                response.parts.retain(|part| match part {
                    starweaver_model::ModelResponsePart::ToolCall(call)
                    | starweaver_model::ModelResponsePart::ProviderToolCall { call, .. } => {
                        valid_tool_call_ids.contains(&call.id)
                    }
                    starweaver_model::ModelResponsePart::Text { text }
                    | starweaver_model::ModelResponsePart::ProviderText { text, .. }
                    | starweaver_model::ModelResponsePart::Thinking { text, .. }
                    | starweaver_model::ModelResponsePart::ProviderThinking { text, .. } => {
                        !text.is_empty()
                    }
                    starweaver_model::ModelResponsePart::Compaction { summary } => {
                        !summary.is_empty()
                    }
                    starweaver_model::ModelResponsePart::NativeToolCall { .. }
                    | starweaver_model::ModelResponsePart::NativeToolReturn { .. }
                    | starweaver_model::ModelResponsePart::File { .. }
                    | starweaver_model::ModelResponsePart::ProviderOpaque { .. } => true,
                });
                for call in response.tool_calls() {
                    pending_tool_call_ids.insert(call.id);
                }
                if !response.parts.is_empty() {
                    sanitized.push(ModelMessage::Response(response));
                }
            }
            ModelMessage::Request(mut request) => {
                request.parts.retain(|part| match part {
                    ModelRequestPart::ToolReturn(tool_return) => {
                        pending_tool_call_ids.remove(&tool_return.tool_call_id)
                            || tool_return
                                .metadata
                                .get("starweaver_retry_recovery_truncated")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false)
                    }
                    ModelRequestPart::RetryPrompt {
                        tool_call_id: Some(tool_call_id),
                        ..
                    } => valid_tool_call_ids.contains(tool_call_id),
                    ModelRequestPart::SystemPrompt { .. }
                    | ModelRequestPart::UserPrompt { .. }
                    | ModelRequestPart::RetryPrompt {
                        tool_call_id: None, ..
                    }
                    | ModelRequestPart::Instruction { .. } => true,
                });
                if !request.parts.is_empty() {
                    sanitized.push(ModelMessage::Request(request));
                }
            }
        }
    }

    sanitized
}

fn valid_tool_call_ids(messages: &[ModelMessage]) -> BTreeSet<String> {
    let mut valid = BTreeSet::new();

    for (message_index, message) in messages.iter().enumerate() {
        let ModelMessage::Response(response) = message else {
            continue;
        };
        for call in response.tool_calls() {
            if has_following_tool_return_before_barrier(messages, message_index, &call.id) {
                valid.insert(call.id);
            }
        }
    }

    valid
}

fn has_following_tool_return_before_barrier(
    messages: &[ModelMessage],
    response_index: usize,
    tool_call_id: &str,
) -> bool {
    for message in messages.iter().skip(response_index.saturating_add(1)) {
        match message {
            ModelMessage::Response(_) => return false,
            ModelMessage::Request(request) => {
                let mut has_barrier = false;
                for part in &request.parts {
                    match part {
                        ModelRequestPart::ToolReturn(tool_return) => {
                            if tool_return.tool_call_id == tool_call_id {
                                return true;
                            }
                        }
                        ModelRequestPart::SystemPrompt { .. }
                        | ModelRequestPart::UserPrompt { .. }
                        | ModelRequestPart::RetryPrompt { .. }
                        | ModelRequestPart::Instruction { .. } => has_barrier = true,
                    }
                }
                if has_barrier {
                    return false;
                }
            }
        }
    }

    false
}

fn steering_message(message: &BusMessage) -> Option<SteeringMessage> {
    if message.topic != "steering" {
        return None;
    }
    let text = message
        .payload
        .get("text")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())?
        .to_string();
    let id = message
        .payload
        .get("id")
        .or_else(|| message.payload.get("message_id"))
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string);
    Some(SteeringMessage { id, text })
}

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
                let mut metadata = serde_json::Map::new();
                metadata.insert(
                    "starweaver_instruction_dynamic".to_string(),
                    serde_json::json!(true),
                );
                ModelRequestPart::Instruction {
                    text: instruction,
                    metadata,
                }
            }));
        }
        if !state.pending_tool_returns.is_empty() {
            for tool_return in &state.pending_tool_returns {
                parts.push(ModelRequestPart::ToolReturn(tool_return.clone()));
                if let Some(media_prompt) = tool_return_media_prompt(tool_return) {
                    parts.push(media_prompt);
                }
            }
        } else if state.run_step == 0 {
            parts.push(ModelRequestPart::UserPrompt {
                content: vec![starweaver_model::ContentPart::Text {
                    text: prompt.to_string(),
                }],
                name: None,
                metadata: serde_json::Map::new(),
            });
        } else {
            let mut metadata = serde_json::Map::new();
            if is_steering_guard_prompt(prompt) {
                metadata.insert(
                    "starweaver.kind".to_string(),
                    serde_json::json!("steering_guard"),
                );
                metadata.insert(
                    "starweaver_instruction_dynamic".to_string(),
                    serde_json::json!(true),
                );
                parts.push(ModelRequestPart::Instruction {
                    text: prompt.to_string(),
                    metadata,
                });
            } else {
                parts.push(ModelRequestPart::RetryPrompt {
                    text: prompt.to_string(),
                    tool_call_id: None,
                    metadata,
                });
            }
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

    pub(super) fn apply_steering_messages(context: &mut AgentContext, request: &mut ModelRequest) {
        let mut steering_messages = Vec::new();
        let mut retained_messages = Vec::new();
        while let Some(message) = context.messages.dequeue() {
            if let Some(steering) = steering_message(&message) {
                context.publish_event(AgentEvent::new(
                    "steering_received",
                    serde_json::json!({"id": steering.id, "text": steering.text}),
                ));
                steering_messages.push(steering);
            } else {
                retained_messages.push(message);
            }
        }
        for message in retained_messages {
            context.enqueue_message(message);
        }
        request
            .parts
            .extend(steering_messages.into_iter().map(|steering| {
                let mut metadata = serde_json::Map::new();
                metadata.insert(
                    "starweaver.topic".to_string(),
                    serde_json::json!("steering"),
                );
                if let Some(id) = &steering.id {
                    metadata.insert("starweaver.steering_id".to_string(), serde_json::json!(id));
                }
                ModelRequestPart::UserPrompt {
                    content: vec![ContentPart::Text {
                        text: format!("Steering update from the user:\n{}", steering.text),
                    }],
                    name: Some("steering".to_string()),
                    metadata,
                }
            }));
    }

    pub(super) fn inject_runtime_context(context: &AgentContext, messages: &mut Vec<ModelMessage>) {
        let is_user_prompt = latest_request(messages)
            .map_or(true, |request| !request_has_tool_return_or_retry(request))
            || metadata_bool(&context.metadata, "starweaver_force_inject_instructions");
        let Some(text) = context.inject_runtime_context(is_user_prompt) else {
            return;
        };
        let mut metadata = serde_json::Map::new();
        metadata.insert(
            "starweaver_instruction_origin".to_string(),
            serde_json::json!("runtime_context"),
        );
        metadata.insert(
            "starweaver_instruction_dynamic".to_string(),
            serde_json::json!(true),
        );
        insert_instruction_into_latest_request(
            messages,
            ModelRequestPart::Instruction { text, metadata },
        );
    }

    pub(super) fn has_pending_steering_messages(context: &AgentContext) -> bool {
        context.messages.has_topic("steering")
    }

    pub(super) fn pending_steering_guard_message(context: &AgentContext) -> Option<String> {
        if Self::has_pending_steering_messages(context) {
            Some(STEERING_GUARD_PROMPT.to_string())
        } else {
            None
        }
    }

    pub(super) async fn checkpoint(
        &self,
        node: AgentExecutionNode,
        state: &AgentRunState,
        context: &AgentContext,
    ) -> Result<AgentExecutionDecision, AgentError> {
        let checkpoint_span = self.trace_recorder.start_span(
            SpanSpec::new("starweaver.checkpoint")
                .with_attribute("starweaver.checkpoint.node", serde_json::json!(node)),
            &context.trace_context,
        );
        let mut checkpoint = AgentCheckpoint::new(node, state);
        checkpoint.resume.trace_context = checkpoint_span.context().clone();
        checkpoint.metadata.insert(
            "trace_id".to_string(),
            serde_json::json!(checkpoint_span.context().trace_id),
        );
        checkpoint.metadata.insert(
            "span_id".to_string(),
            serde_json::json!(checkpoint_span.context().span_id),
        );
        for capability in &self.ordered_capabilities()? {
            capability
                .on_checkpoint_with_context(state, context, &checkpoint)
                .await
                .map_err(Self::capability_error)?;
        }
        let decision = self.executor.checkpoint(checkpoint).await?;
        self.trace_recorder
            .close_span(&checkpoint_span, SpanStatus::Ok);
        Ok(decision)
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

    pub(super) fn absorb_tool_context_handle(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        handle: &AgentContextHandle,
    ) -> Result<(), AgentError> {
        let mut snapshot = handle.snapshot();
        let usage = snapshot.usage.clone();
        context.usage = usage.clone();
        state.usage = usage;
        context.notes.clone_from(&snapshot.notes);
        context.state.clone_from(&snapshot.state);
        context.events.clone_from(&snapshot.events);
        snapshot
            .message_history
            .clone_from(&context.message_history);
        snapshot.run_id.clone_from(&context.run_id);
        snapshot.trace_context.clone_from(&context.trace_context);
        handle.replace(snapshot);
        self.check_usage(state)
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

    pub(super) async fn prepare_model_messages(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
    ) -> Result<Vec<ModelMessage>, AgentError> {
        let mut messages = state.message_history.clone();
        for capability in &self.ordered_capabilities()? {
            let before_count = messages.len();
            messages = capability
                .prepare_model_messages_with_context(state, context, messages)
                .await
                .map_err(Self::capability_error)?;
            let after_count = messages.len();
            if before_count != after_count {
                let span = self.trace_recorder.start_span(
                    SpanSpec::new("starweaver.history.compaction")
                        .with_attribute(
                            "starweaver.capability.name",
                            serde_json::json!(capability.spec().id.as_str()),
                        )
                        .with_attribute(
                            "starweaver.history.messages.before",
                            serde_json::json!(before_count),
                        )
                        .with_attribute(
                            "starweaver.history.messages.after",
                            serde_json::json!(after_count),
                        ),
                    &context.trace_context,
                );
                self.trace_recorder.close_span(&span, SpanStatus::Ok);
            }
        }
        let before_count = messages.len();
        messages = sanitize_incomplete_tool_call_history(messages);
        let after_count = messages.len();
        if before_count != after_count {
            let span = self.trace_recorder.start_span(
                SpanSpec::new("starweaver.history.sanitize_incomplete_tool_calls")
                    .with_attribute(
                        "starweaver.history.messages.before",
                        serde_json::json!(before_count),
                    )
                    .with_attribute(
                        "starweaver.history.messages.after",
                        serde_json::json!(after_count),
                    ),
                &context.trace_context,
            );
            self.trace_recorder.close_span(&span, SpanStatus::Ok);
        }
        Ok(messages)
    }

    pub(super) fn record_model_request_event(
        &self,
        span: &crate::trace::SpanHandle,
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        params: &ModelRequestParameters,
    ) {
        self.trace_recorder.record_event(
            span,
            SpanEvent::new("starweaver.model.request")
                .with_attribute(
                    "starweaver.model.message_count",
                    serde_json::json!(messages.len()),
                )
                .with_attribute(
                    "starweaver.model.tool_count",
                    serde_json::json!(params.tools.len()),
                )
                .with_attribute(
                    "starweaver.model.native_tool_count",
                    serde_json::json!(params.native_tools.len()),
                )
                .with_attribute(
                    "starweaver.model.has_output_schema",
                    serde_json::json!(params.output_schema.is_some()),
                )
                .with_attribute(
                    "gen_ai.request",
                    serde_json::json!({
                        "messages": messages,
                        "settings": settings,
                        "params": params,
                    }),
                ),
        );
    }

    pub(super) fn record_model_response_event(
        &self,
        span: &crate::trace::SpanHandle,
        response: &ModelResponse,
    ) {
        self.trace_recorder.record_event(
            span,
            SpanEvent::new("starweaver.model.response")
                .with_attribute("gen_ai.response", serde_json::json!(response))
                .with_attribute(
                    "gen_ai.usage.input_tokens",
                    serde_json::json!(response.usage.input_tokens),
                )
                .with_attribute(
                    "gen_ai.usage.output_tokens",
                    serde_json::json!(response.usage.output_tokens),
                ),
        );
    }

    pub(super) fn record_model_stream_event(
        &self,
        span: &crate::trace::SpanHandle,
        event: &ModelResponseStreamEvent,
    ) {
        self.trace_recorder.record_event(
            span,
            SpanEvent::new("starweaver.model.stream_event")
                .with_attribute("gen_ai.response.stream_event", serde_json::json!(event)),
        );
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
        for instruction in self.tools.instructions() {
            let mut metadata = serde_json::Map::new();
            metadata.insert(
                "starweaver_instruction_origin".to_string(),
                serde_json::json!("toolset"),
            );
            metadata.insert(
                "starweaver_toolset_group".to_string(),
                serde_json::json!(instruction.group.clone()),
            );
            params.instructions.push(PreparedInstruction {
                text: instruction.render_xml(),
                dynamic: false,
                metadata,
            });
        }
        Ok(params)
    }

    pub(super) async fn call_run_start(
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

    pub(super) async fn prepare_tools(
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

    pub(super) async fn call_before_model_request(
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

    pub(super) async fn call_after_model_response(
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

    pub(super) async fn call_before_tool_execution(
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

    pub(super) async fn call_after_tool_result(
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
                call.arguments.execution_value(),
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
        for capability in &self.ordered_capabilities_for_validation()? {
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
        for capability in &self.ordered_capabilities_for_validation()? {
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
        for capability in &self.ordered_capabilities_for_validation()? {
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
        for capability in &self.ordered_capabilities()? {
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
        for observer in &self.ordered_stream_observers()? {
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
        for capability in &self.ordered_capabilities()? {
            capability
                .on_run_complete_with_context(state, context)
                .await
                .map_err(Self::capability_error)?;
        }
        Ok(())
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

fn latest_request(messages: &[ModelMessage]) -> Option<&ModelRequest> {
    messages.iter().rev().find_map(|message| match message {
        ModelMessage::Request(request) => Some(request),
        ModelMessage::Response(_) => None,
    })
}

fn request_has_tool_return_or_retry(request: &ModelRequest) -> bool {
    request.parts.iter().any(|part| {
        matches!(
            part,
            ModelRequestPart::ToolReturn(_) | ModelRequestPart::RetryPrompt { .. }
        )
    })
}

fn metadata_bool(metadata: &serde_json::Map<String, serde_json::Value>, key: &str) -> bool {
    metadata
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn insert_instruction_into_latest_request(
    messages: &mut Vec<ModelMessage>,
    part: ModelRequestPart,
) {
    for message in messages.iter_mut().rev() {
        if let ModelMessage::Request(request) = message {
            insert_request_part_after_control_parts(request, part);
            return;
        }
    }
    messages.push(ModelMessage::Request(ModelRequest {
        parts: vec![part],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    }));
}

fn insert_request_part_after_control_parts(request: &mut ModelRequest, part: ModelRequestPart) {
    let insert_at = request
        .parts
        .iter()
        .enumerate()
        .filter_map(|(index, part)| match part {
            ModelRequestPart::ToolReturn(_) | ModelRequestPart::RetryPrompt { .. } => {
                Some(index + 1)
            }
            ModelRequestPart::SystemPrompt { .. }
            | ModelRequestPart::UserPrompt { .. }
            | ModelRequestPart::Instruction { .. } => None,
        })
        .next_back()
        .unwrap_or(0);
    request.parts.insert(insert_at, part);
}
