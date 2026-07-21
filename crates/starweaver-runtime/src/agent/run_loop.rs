//! Agent run loop entrypoints.

use std::collections::{BTreeMap, BTreeSet};

use starweaver_context::{AgentContext, AgentEvent};
use starweaver_core::{ConversationId, RunId, TraceContext};
use starweaver_model::{
    ContentPart, ModelMessage, ModelRequest, ModelRequestContext, ModelRequestParameters,
    ModelRequestPart, ModelResponse, ModelResponseStreamEvent, ModelSettings, ToolCallPart,
    ToolReturnPart,
};
use starweaver_tools::{ToolContext, ToolDependencyProfile, ToolRegistry};
use starweaver_usage::pricing::known_model_pricing_profile;

const DEFAULT_MODEL_ERROR_RETRIES: usize = 2;
const DURABLE_RUN_ID_METADATA_KEY: &str = "starweaver.durable_run_id";
const CLI_RUN_ID_METADATA_KEY: &str = "cli.run_id";

fn durable_run_id_from_context(context: &AgentContext) -> Option<RunId> {
    if context.parent_run_id.is_some() {
        return None;
    }
    context
        .metadata
        .get(DURABLE_RUN_ID_METADATA_KEY)
        .or_else(|| context.metadata.get(CLI_RUN_ID_METADATA_KEY))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(RunId::from_string)
}

mod entrypoints;
mod provider_invocation;

use provider_invocation::{ProviderInvocation, ProviderInvocationMode, ProviderInvocationStep};

use crate::{
    agent::{
        Agent, AgentEndStrategy, AgentError, AgentInput, AgentResult, AgentToolExecutionMode,
        helpers::{
            has_pending_tool_control_flow, is_successful_tool_return, is_tool_retry_return,
            mark_tool_retry_return, record_tool_control_flow, tool_return_control_flow,
        },
        run_loop_helpers::{
            agent_error_kind, agent_error_public_message, preserve_pending_tool_returns_for_resume,
        },
        runtime_helpers::{request_instruction_insert_index, tool_return_media_prompt},
    },
    capability::{CapabilityError, RetryEventKind},
    dependency_assembly::{ToolDependencyAssembly, assemble_tool_dependencies_for_name},
    executor::{AgentExecutionDecision, AgentExecutionNode},
    retry_recovery::{DEFAULT_MODEL_ERROR_RESUME_PROMPT, recover_retry_message_history},
    run::{AgentRunState, RunStatus},
    stream::{
        AgentStreamEvent, AgentStreamRecord, AgentStreamSink, push_stream_event, push_stream_record,
    },
    trace::{
        DynTraceRecorder, SpanEvent, SpanHandle, SpanKind, SpanSpec, SpanStatus,
        TraceRecorderHandle,
    },
};

struct ActiveSpan {
    recorder: DynTraceRecorder,
    span: SpanHandle,
    fallback_error_type: &'static str,
    closed: bool,
}

impl ActiveSpan {
    fn start(
        recorder: &DynTraceRecorder,
        spec: SpanSpec,
        parent: &TraceContext,
        fallback_error_type: &'static str,
    ) -> Self {
        let span = recorder.start_span(spec, parent);
        Self {
            recorder: recorder.clone(),
            span,
            fallback_error_type,
            closed: false,
        }
    }

    const fn context(&self) -> &TraceContext {
        self.span.context()
    }

    fn close(&mut self, status: SpanStatus) {
        if self.closed {
            return;
        }
        self.recorder.close_span(&self.span, status);
        self.closed = true;
    }
}

impl std::ops::Deref for ActiveSpan {
    type Target = SpanHandle;

    fn deref(&self) -> &Self::Target {
        &self.span
    }
}

impl Drop for ActiveSpan {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        self.recorder.close_span(
            &self.span,
            SpanStatus::Error {
                error_type: self.fallback_error_type.to_string(),
            },
        );
        self.closed = true;
    }
}

struct PrepareRequestInput<'a> {
    prompt: &'a str,
    initial_content: &'a [ContentPart],
    is_initial_request: bool,
    run_id: &'a RunId,
    conversation_id: &'a ConversationId,
}

struct PrepareRequestResult {
    request: ModelRequest,
    settings: Option<ModelSettings>,
    transition: PrepareRequestTransition,
}

enum PrepareRequestTransition {
    CallModel,
    ClassifyResponse { response: Box<ModelResponse> },
}

struct CallModelPreparation {
    messages: Vec<ModelMessage>,
    params: ModelRequestParameters,
    transition: CallModelPreparationTransition,
}

enum CallModelPreparationTransition {
    ApplySteering { request_index: usize },
    PrepareProvider,
}

struct PreparedProviderRequest {
    messages: Vec<ModelMessage>,
    settings: Option<ModelSettings>,
    params: ModelRequestParameters,
}

struct ClassifyResponseResult {
    response: ModelResponse,
    transition: ClassifyResponseTransition,
}

enum ClassifyResponseTransition {
    PrepareTools { tool_calls: Vec<ToolCallPart> },
    ValidateOutput,
}

enum PrepareToolsTransition {
    ExecuteTools {
        tool_calls: Vec<ToolCallPart>,
        final_output_after_tools: Option<(String, Option<serde_json::Value>)>,
    },
    PrepareRequestForOutputRetry {
        prompt: String,
        retries: usize,
    },
    PrepareRequestForSteering {
        prompt: String,
    },
    Finalize {
        output: String,
        structured_output: Option<serde_json::Value>,
    },
}

enum ExecuteToolsTransition {
    AwaitExternal,
    PrepareRequest {
        prompt: String,
    },
    Finalize {
        output: String,
        structured_output: Option<serde_json::Value>,
    },
}

enum AwaitExternalTransition {
    Suspend {
        node: AgentExecutionNode,
        reason: String,
    },
}

enum FinalizeTransition {
    Complete { output: String },
}

enum FailOrCancelTransition {
    Cancelled { reason: String },
    Failed { error_kind: String, message: String },
}

enum RunLoopExit {
    Completed { output: String },
    Waiting,
}

struct PreparedToolExecution {
    index: usize,
    call: ToolCallPart,
    tool_context: ToolContext,
    dependency_assembly: ToolDependencyAssembly,
    stream_sink: Option<AgentStreamSink>,
    tool_span: ActiveSpan,
    started_at: std::time::Instant,
}

impl Agent {
    async fn prepare_request_phase(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        input: PrepareRequestInput<'_>,
    ) -> Result<PrepareRequestResult, AgentError> {
        let dynamic_instruction_parts = self.dynamic_instruction_parts(state).await?;
        let mut request = self.prepare_request(
            state,
            input.prompt,
            input.initial_content,
            input.is_initial_request,
            input.run_id,
            input.conversation_id,
        );
        if !dynamic_instruction_parts.is_empty() {
            let insert_at = request_instruction_insert_index(&request);
            request
                .parts
                .splice(insert_at..insert_at, dynamic_instruction_parts);
        }
        let mut settings = self.effective_settings(context);
        let transition = self
            .call_before_model_request(state, context, &mut request, &mut settings)
            .await?
            .map_or(PrepareRequestTransition::CallModel, |response| {
                PrepareRequestTransition::ClassifyResponse {
                    response: Box::new(response),
                }
            });
        Ok(PrepareRequestResult {
            request,
            settings,
            transition,
        })
    }

    async fn prepare_model_call_phase(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        run_tools: &ToolRegistry,
        run_id: &RunId,
        conversation_id: &ConversationId,
    ) -> Result<CallModelPreparation, AgentError> {
        self.check_before_request(state)?;
        let mut messages = self.prepare_model_messages(state, context).await?;
        context.runtime.tool_id_wrapper.wrap_messages(&mut messages);
        Self::validate_model_request_messages(&messages)?;
        self.inject_missing_static_instructions(run_id, conversation_id, &mut messages);
        let params = self
            .effective_request_params(state, context, run_tools)
            .await?;
        messages = Self::attach_prepared_request_instructions(messages, &params);
        for message in &mut messages {
            Self::fill_message_metadata(message, run_id, conversation_id);
        }
        let transition = if Self::has_pending_steering_messages(context) {
            messages
                .iter()
                .rposition(|message| matches!(message, ModelMessage::Request(_)))
                .map_or(
                    CallModelPreparationTransition::PrepareProvider,
                    |request_index| CallModelPreparationTransition::ApplySteering { request_index },
                )
        } else {
            CallModelPreparationTransition::PrepareProvider
        };
        Ok(CallModelPreparation {
            messages,
            params,
            transition,
        })
    }

    async fn prepare_provider_request_phase(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
    ) -> Result<PreparedProviderRequest, AgentError> {
        let messages = self
            .prepare_provider_messages(state, context, messages)
            .await?;
        Self::validate_model_request_messages(&messages)?;
        Ok(PreparedProviderRequest {
            messages,
            settings,
            params,
        })
    }

    async fn classify_response_phase(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
    ) -> Result<ClassifyResponseResult, AgentError> {
        let mut response = state
            .latest_response
            .clone()
            .ok_or_else(|| AgentError::Capability("missing latest response".to_string()))?;
        self.call_after_model_response(state, context, &mut response)
            .await?;
        state.replace_latest_response(response.clone());
        context.message_history.clone_from(&state.message_history);
        let tool_calls = response.tool_calls();
        let transition = if tool_calls.is_empty() {
            ClassifyResponseTransition::ValidateOutput
        } else {
            ClassifyResponseTransition::PrepareTools { tool_calls }
        };
        Ok(ClassifyResponseResult {
            response,
            transition,
        })
    }

    async fn prepare_tools_phase(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        mut tool_calls: Vec<ToolCallPart>,
        run_tools: &ToolRegistry,
        output_retries_used: usize,
    ) -> Result<PrepareToolsTransition, AgentError> {
        let mut final_output_after_tools = None;
        match self
            .try_call_output_function(state, context, &tool_calls)
            .await
        {
            Ok(Some((output, structured_output))) => {
                let ordinary_tool_calls = tool_calls
                    .iter()
                    .filter(|call| {
                        !self
                            .output_functions
                            .iter()
                            .any(|function| function.definition().name == call.name)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if self.policy.end_strategy != AgentEndStrategy::Early
                    && !ordinary_tool_calls.is_empty()
                {
                    final_output_after_tools = Some((output, structured_output));
                    tool_calls = ordinary_tool_calls;
                } else if Self::has_pending_steering_messages(context) {
                    let Some(prompt) = Self::pending_steering_guard_message(context) else {
                        unreachable!("pending steering guard message must exist");
                    };
                    return Ok(PrepareToolsTransition::PrepareRequestForSteering { prompt });
                } else {
                    return Ok(PrepareToolsTransition::Finalize {
                        output,
                        structured_output,
                    });
                }
            }
            Ok(None) => {}
            Err(CapabilityError::ModelRetry(prompt)) => {
                if output_retries_used >= self.policy.output_retries {
                    return Err(AgentError::OutputRetryLimitExceeded {
                        retries: output_retries_used,
                    });
                }
                return Ok(PrepareToolsTransition::PrepareRequestForOutputRetry {
                    prompt,
                    retries: output_retries_used.saturating_add(1),
                });
            }
            Err(error) => return Err(Self::capability_error(error)),
        }
        if run_tools.is_empty() {
            return Err(AgentError::ToolCallsRequireTools);
        }
        state.pending_tool_calls.clone_from(&tool_calls);
        let projected_successful_tool_calls = tool_calls
            .iter()
            .filter(|call| run_tools.get(&call.name).is_some())
            .count() as u64;
        self.check_tool_calls(state, projected_successful_tool_calls)?;
        Ok(PrepareToolsTransition::ExecuteTools {
            tool_calls,
            final_output_after_tools,
        })
    }

    fn execute_tools_phase(
        state: &mut AgentRunState,
        context: &AgentContext,
        final_output_after_tools: Option<(String, Option<serde_json::Value>)>,
        run_id: &RunId,
        conversation_id: &ConversationId,
    ) -> ExecuteToolsTransition {
        if has_pending_tool_control_flow(state) {
            Self::append_pending_tool_returns_request(
                state,
                run_id,
                conversation_id,
                false,
                "starweaver.waiting.non_control_flow_tool_returns",
            );
            state.pending_tool_returns.clear();
            return ExecuteToolsTransition::AwaitExternal;
        }
        if let Some((output, structured_output)) = final_output_after_tools {
            Self::append_pending_tool_returns_request(
                state,
                run_id,
                conversation_id,
                true,
                "starweaver.final_output_tool_returns",
            );
            state.pending_tool_returns.clear();
            if Self::has_pending_steering_messages(context) {
                let Some(prompt) = Self::pending_steering_guard_message(context) else {
                    unreachable!("pending steering guard message must exist");
                };
                ExecuteToolsTransition::PrepareRequest { prompt }
            } else {
                ExecuteToolsTransition::Finalize {
                    output,
                    structured_output,
                }
            }
        } else {
            ExecuteToolsTransition::PrepareRequest {
                prompt: String::new(),
            }
        }
    }

    fn append_pending_tool_returns_request(
        state: &mut AgentRunState,
        run_id: &RunId,
        conversation_id: &ConversationId,
        include_control_flow: bool,
        metadata_key: &str,
    ) {
        let tool_returns = state
            .pending_tool_returns
            .iter()
            .filter(|tool_return| {
                include_control_flow || tool_return_control_flow(tool_return).is_none()
            })
            .cloned()
            .collect::<Vec<_>>();
        if tool_returns.is_empty() {
            return;
        }
        let mut parts = Vec::new();
        for tool_return in tool_returns {
            parts.push(ModelRequestPart::ToolReturn(tool_return.clone()));
            if let Some(media_prompt) = tool_return_media_prompt(&tool_return) {
                parts.push(media_prompt);
            }
        }
        state
            .message_history
            .push(ModelMessage::Request(ModelRequest {
                parts,
                timestamp: Some(chrono::Utc::now()),
                instructions: None,
                run_id: Some(run_id.clone()),
                conversation_id: Some(conversation_id.clone()),
                metadata: serde_json::json!({(metadata_key): true})
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            }));
    }

    fn await_external_phase(
        state: &mut AgentRunState,
        context: &mut AgentContext,
    ) -> AwaitExternalTransition {
        state.status = RunStatus::Waiting;
        context.message_history.clone_from(&state.message_history);
        AwaitExternalTransition::Suspend {
            node: AgentExecutionNode::ToolReturn,
            reason: "hitl_control_flow".to_string(),
        }
    }

    fn finalize_phase(
        state: &mut AgentRunState,
        output: String,
        structured_output: Option<serde_json::Value>,
    ) -> FinalizeTransition {
        state.output = Some(output.clone());
        state.structured_output = structured_output;
        state.status = RunStatus::Completed;
        FinalizeTransition::Complete { output }
    }

    fn fail_or_cancel_phase(
        state: &mut AgentRunState,
        context: &mut AgentContext,
        error: &AgentError,
    ) -> FailOrCancelTransition {
        state.status = if matches!(error, AgentError::Cancelled { .. }) {
            RunStatus::Cancelled
        } else {
            RunStatus::Failed
        };
        preserve_pending_tool_returns_for_resume(state);
        context.message_history.clone_from(&state.message_history);
        context.usage.clone_from(&state.usage);
        match error {
            AgentError::Cancelled { .. } => FailOrCancelTransition::Cancelled {
                reason: error.public_message(),
            },
            error => FailOrCancelTransition::Failed {
                error_kind: agent_error_kind(error).to_string(),
                message: agent_error_public_message(error),
            },
        }
    }

    fn should_execute_tool_calls_sequentially(
        &self,
        run_tools: &ToolRegistry,
        tool_calls: &[ToolCallPart],
    ) -> bool {
        if self.policy.tool_execution == AgentToolExecutionMode::Sequential {
            return true;
        }
        for call in tool_calls {
            let requirements = run_tools.dependency_requirements_for(&call.name);
            if requirements.profile == ToolDependencyProfile::Legacy
                || !requirements.context_capabilities.is_empty()
            {
                return true;
            }
        }
        let mut seen_tool_names = BTreeSet::new();
        tool_calls.iter().any(|call| {
            run_tools.sequential_for(&call.name) || !seen_tool_names.insert(call.name.as_str())
        })
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn prepare_tool_execution(
        &self,
        index: usize,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        run_tools: &ToolRegistry,
        call: &ToolCallPart,
        step_trace_context: &TraceContext,
        run_id: &RunId,
        conversation_id: &ConversationId,
        tool_retries: &BTreeMap<String, usize>,
        stream_enabled: bool,
    ) -> Result<PreparedToolExecution, AgentError> {
        let tool_retry = *tool_retries.get(&call.name).unwrap_or(&0);
        let tool_max_retries = run_tools.max_retries_for(&call.name);
        let tool_span = ActiveSpan::start(
            &self.trace_recorder,
            SpanSpec::new("gen_ai.execute_tool")
                .with_attribute(
                    "gen_ai.agent.id",
                    serde_json::json!(context.agent_id.as_str()),
                )
                .with_attribute(
                    "gen_ai.conversation.id",
                    serde_json::json!(conversation_id.as_str()),
                )
                .with_attribute("starweaver.run.id", serde_json::json!(run_id.as_str()))
                .with_attribute("gen_ai.tool.name", serde_json::json!(call.name.clone()))
                .with_attribute("gen_ai.tool.call.id", serde_json::json!(call.id.clone())),
            step_trace_context,
            "tool_execution_dropped",
        );
        let dependency_requirements = run_tools.dependency_requirements_for(&call.name);
        let initial_assembly = assemble_tool_dependencies_for_name(
            context,
            &call.name,
            &dependency_requirements,
            &context.tool_capability_grant(&call.name),
        );
        let mut tool_dependencies = initial_assembly.dependencies.clone();
        tool_dependencies.insert(TraceRecorderHandle::new(self.trace_recorder.clone()));
        let stream_sink = stream_enabled.then(AgentStreamSink::default);
        if let Some(stream_sink) = &stream_sink {
            tool_dependencies.insert(stream_sink.clone());
        }
        let mut tool_context = ToolContext::new(
            state.run_id.clone(),
            state.conversation_id.clone(),
            state.run_step,
        )
        .with_dependencies(tool_dependencies)
        .with_trace_context(tool_span.context().clone())
        .with_run_attachments(context.run_attachment_values().clone())
        .with_retry_budget(tool_retry, tool_max_retries);
        if let Some(token) = self.cancellation_token.as_ref() {
            tool_context = tool_context.with_cancellation_token(token.clone());
        }
        self.call_before_tool_execution(state, context, &mut tool_context, call)
            .await?;
        initial_assembly.apply_to(context);
        let dependency_assembly = assemble_tool_dependencies_for_name(
            context,
            &call.name,
            &dependency_requirements,
            &context.tool_capability_grant(&call.name),
        );
        tool_context
            .dependencies
            .extend(dependency_assembly.dependencies.clone());
        self.trace_recorder.record_event(
            &tool_span,
            SpanEvent::new("starweaver.tool.call").with_attribute(
                "gen_ai.tool.call.arguments",
                serde_json::json!({"redacted": true}),
            ),
        );

        Ok(PreparedToolExecution {
            index,
            call: call.clone(),
            tool_context,
            dependency_assembly,
            stream_sink,
            tool_span,
            started_at: std::time::Instant::now(),
        })
    }

    async fn execute_prepared_tool_call(
        run_tools: ToolRegistry,
        index: usize,
        call: ToolCallPart,
        tool_context: ToolContext,
    ) -> (usize, ToolReturnPart) {
        (index, run_tools.execute_call(tool_context, &call).await)
    }

    #[allow(clippy::large_stack_frames, clippy::too_many_lines)]
    async fn run_with_context_inner(
        &self,
        prompt: impl Into<AgentInput>,
        context: &mut AgentContext,
        stream_events: Option<&mut Vec<AgentStreamRecord>>,
    ) -> Result<AgentResult, AgentError> {
        let previous_trace_context = context.trace_context.clone();
        let mut stream_events = stream_events;
        let result = self
            .run_with_context_inner_impl(prompt, context, stream_events.as_deref_mut())
            .await;
        if let Err(error) = &result
            && context.runtime.lifecycle.entered
        {
            let run_id = context.run_id.clone().unwrap_or_default();
            let error_kind = agent_error_kind(error).to_string();
            let message = agent_error_public_message(error);
            if !context.runtime.run_toolsets_closed {
                self.close_run_toolsets(context).await;
                context.runtime.run_toolsets_closed = true;
            }
            context.finish_run();
            if let AgentError::Cancelled { .. } = error {
                context.publish_event(AgentEvent::new(
                    "run_cancelled",
                    serde_json::json!({
                        "run_id": run_id.as_str(),
                        "reason": message.clone(),
                    }),
                ));
                push_stream_event(
                    &mut stream_events,
                    AgentStreamEvent::RunCancelled {
                        run_id,
                        reason: message,
                    },
                );
            } else {
                context.publish_event(AgentEvent::new(
                    "run_failed",
                    serde_json::json!({
                        "run_id": run_id.as_str(),
                        "error_kind": error_kind.clone(),
                        "message": message.clone(),
                    }),
                ));
                push_stream_event(
                    &mut stream_events,
                    AgentStreamEvent::RunFailed {
                        run_id,
                        error_kind,
                        message,
                    },
                );
            }
        }
        context.trace_context = previous_trace_context;
        result
    }

    #[allow(clippy::large_stack_frames, clippy::too_many_lines)]
    async fn run_with_context_inner_impl(
        &self,
        prompt: impl Into<AgentInput>,
        context: &mut AgentContext,
        mut stream_events: Option<&mut Vec<AgentStreamRecord>>,
    ) -> Result<AgentResult, AgentError> {
        macro_rules! stream_event {
            ($state:expr, $event:expr) => {{
                let event = $event;
                push_stream_event(&mut stream_events, event);
                if let Some(record) = stream_events
                    .as_deref()
                    .and_then(|events| events.last())
                    .cloned()
                {
                    self.call_stream_observers($state, context, &record).await?;
                }
            }};
        }

        macro_rules! stream_record {
            ($state:expr, $record:expr) => {{
                let record = $record;
                push_stream_record(&mut stream_events, record);
                if let Some(record) = stream_events
                    .as_deref()
                    .and_then(|events| events.last())
                    .cloned()
                {
                    self.call_stream_observers($state, context, &record).await?;
                }
            }};
        }

        macro_rules! stream_context_events {
            ($state:expr, $cursor:expr) => {{
                let events = context.events.events();
                while $cursor < events.len() {
                    stream_event!(
                        $state,
                        AgentStreamEvent::Custom {
                            event: events[$cursor].clone(),
                        }
                    );
                    $cursor += 1;
                }
            }};
        }

        macro_rules! close_run_toolsets {
            ($state:expr, $cursor:expr) => {{
                if !context.runtime.run_toolsets_closed {
                    self.close_run_toolsets(context).await;
                    context.runtime.run_toolsets_closed = true;
                }
                stream_context_events!($state, $cursor);
            }};
        }

        macro_rules! fail_run {
            ($state:expr, $run_id:expr, $event_cursor:expr, $error:expr) => {{
                let error = $error;
                let transition = Self::fail_or_cancel_phase($state, context, &error);
                close_run_toolsets!($state, $event_cursor);
                context.finish_run();
                match transition {
                    FailOrCancelTransition::Cancelled { reason } => {
                        context.publish_event(AgentEvent::new(
                            "run_cancelled",
                            serde_json::json!({
                                "run_id": $run_id.as_str(),
                                "reason": reason,
                            }),
                        ));
                        stream_context_events!($state, $event_cursor);
                        stream_event!(
                            $state,
                            AgentStreamEvent::RunCancelled {
                                run_id: $run_id.clone(),
                                reason,
                            }
                        );
                    }
                    FailOrCancelTransition::Failed {
                        error_kind,
                        message,
                    } => {
                        context.publish_event(AgentEvent::new(
                            "run_failed",
                            serde_json::json!({
                                "run_id": $run_id.as_str(),
                                "error_kind": error_kind.clone(),
                                "message": message.clone(),
                            }),
                        ));
                        stream_context_events!($state, $event_cursor);
                        stream_event!(
                            $state,
                            AgentStreamEvent::RunFailed {
                                run_id: $run_id.clone(),
                                error_kind,
                                message,
                            }
                        );
                    }
                }
                return Err(error);
            }};
        }

        macro_rules! checkpoint {
            ($node:expr, $state:expr, $event_cursor:expr) => {{
                let node = $node;
                stream_event!(
                    $state,
                    AgentStreamEvent::NodeStart {
                        node,
                        step: $state.run_step,
                        status: $state.status,
                    }
                );
                let stream_cursor = stream_events
                    .as_deref()
                    .and_then(|events| events.last().map(|record| record.sequence));
                let decision = self
                    .checkpoint(node, $state, context, stream_cursor)
                    .await?;
                stream_context_events!($state, $event_cursor);
                stream_event!(
                    $state,
                    AgentStreamEvent::Checkpoint {
                        node,
                        step: $state.run_step,
                    }
                );
                if let AgentExecutionDecision::Suspend { reason } = decision {
                    stream_event!(
                        $state,
                        AgentStreamEvent::Suspended {
                            node,
                            reason: reason.clone(),
                        }
                    );
                    stream_event!(
                        $state,
                        AgentStreamEvent::NodeComplete {
                            node,
                            step: $state.run_step,
                            status: $state.status,
                        }
                    );
                    close_run_toolsets!($state, $event_cursor);
                    context.finish_run();
                    return Err(AgentError::ExecutionSuspended { node, reason });
                }
                stream_event!(
                    $state,
                    AgentStreamEvent::NodeComplete {
                        node,
                        step: $state.run_step,
                        status: $state.status,
                    }
                );
            }};
        }

        macro_rules! complete_run {
            ($state:ident, $output:expr, $run_id:expr, $event_cursor:ident, $step_span:ident, $run_span:ident, $history_len:expr) => {{
                let output = $output;
                self.call_run_complete(&mut $state, context).await?;
                checkpoint!(
                    AgentExecutionNode::RunComplete,
                    &$state,
                    $event_cursor
                );
                context.message_history.clone_from(&$state.message_history);
                close_run_toolsets!(&$state, $event_cursor);
                context.publish_event(AgentEvent::new(
                    "run_complete",
                    serde_json::json!({"run_id": $run_id.as_str()}),
                ));
                stream_context_events!(&$state, $event_cursor);
                let terminal_record = stream_events.as_deref().map(|events| {
                    AgentStreamRecord::new(
                        events.len(),
                        AgentStreamEvent::RunComplete {
                            run_id: $run_id.clone(),
                            output: output.clone(),
                        },
                    )
                });
                if let Some(record) = terminal_record {
                    push_stream_record(&mut stream_events, record.clone());
                    if let Err(error) = self.call_stream_observers(&$state, context, &record).await {
                        context.publish_event(AgentEvent::new(
                            "terminal_stream_observer_failed",
                            serde_json::json!({
                                "run_id": $run_id.as_str(),
                                "terminal_kind": "run_complete",
                                "error_kind": agent_error_kind(&error),
                                "message": agent_error_public_message(&error),
                            }),
                        ));
                    }
                }
                $step_span.close(SpanStatus::Ok);
                $run_span.close(SpanStatus::Ok);
                context.finish_run();
                return Ok(RunLoopExit::Completed { output });
            }};
        }

        macro_rules! apply_tool_return {
            ($state:ident, $context:ident, $tool_retries:ident, $run_tools:expr, $step_span:ident, $run_span:ident, $run_id:expr, $context_event_cursor:ident, $call:expr, $tool_return:expr, $tool_span:expr, $dependency_assembly:expr, $tool_duration:expr) => {{
                let call = $call;
                let mut tool_return = $tool_return;
                let mut tool_span = $tool_span;
                let tool_duration = $tool_duration;
                let duration_ms =
                    u64::try_from(tool_duration.as_millis()).unwrap_or(u64::MAX);
                tool_return
                    .metadata
                    .entry("duration_ms".to_string())
                    .or_insert_with(|| serde_json::json!(duration_ms));
                tool_return
                    .metadata
                    .entry("duration_seconds".to_string())
                    .or_insert_with(|| serde_json::json!(tool_duration.as_secs_f64()));
                self.trace_recorder.record_event(
                    &tool_span,
                    SpanEvent::new("starweaver.tool.return")
                        .with_attribute(
                            "gen_ai.tool.call.result",
                            serde_json::json!({
                                "redacted": true,
                                "is_error": tool_return.is_error,
                            }),
                        )
                        .with_attribute(
                            "starweaver.tool.duration_ms",
                            serde_json::json!(duration_ms),
                        )
                        .with_attribute(
                            "starweaver.tool.is_error",
                            serde_json::json!(tool_return.is_error),
                        ),
                );
                if tool_return.is_error && tool_return_control_flow(&tool_return).is_none() {
                    tool_span.close(SpanStatus::Error {
                        error_type: tool_return
                            .metadata
                            .get("error_kind")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("tool_error")
                            .to_string(),
                    });
                } else {
                    tool_span.close(SpanStatus::Ok);
                }
                $dependency_assembly.apply_to($context);
                $state.usage.clone_from(&$context.usage);
                self.check_usage(&$state)?;
                self.call_after_tool_result(&mut $state, $context, call, &mut tool_return)
                    .await?;
                tool_return
                    .metadata
                    .insert("duration_ms".to_string(), serde_json::json!(duration_ms));
                tool_return.metadata.insert(
                    "duration_seconds".to_string(),
                    serde_json::json!(tool_duration.as_secs_f64()),
                );
                if tool_return.is_error && is_tool_retry_return(&tool_return) {
                    let tool_retry = *$tool_retries.get(&call.name).unwrap_or(&0);
                    let tool_max_retries = $run_tools.max_retries_for(&call.name);
                    if tool_retry >= tool_max_retries {
                        tool_span.close(SpanStatus::Error {
                            error_type: "tool_retry_limit_exceeded".to_string(),
                        });
                        $step_span.close(SpanStatus::Error {
                            error_type: "tool_retry_limit_exceeded".to_string(),
                        });
                        $run_span.close(SpanStatus::Error {
                            error_type: "tool_retry_limit_exceeded".to_string(),
                        });
                        fail_run!(
                            &mut $state,
                            $run_id,
                            $context_event_cursor,
                            AgentError::ToolRetryLimitExceeded {
                                tool: call.name.clone(),
                                max_retries: tool_max_retries,
                            }
                        );
                    }
                    let next_retry = tool_retry.saturating_add(1);
                    $tool_retries.insert(call.name.clone(), next_retry);
                    self.call_retry(
                        &mut $state,
                        $context,
                        RetryEventKind::Tool,
                        next_retry,
                        &call.name,
                    )
                    .await?;
                    mark_tool_retry_return(&mut tool_return, next_retry, tool_max_retries);
                }
                stream_event!(
                    &$state,
                    AgentStreamEvent::ToolReturn {
                        step: $state.run_step,
                        tool_return: tool_return.clone(),
                    }
                );
                record_tool_control_flow(&mut $state, &tool_return);
                if is_successful_tool_return(&tool_return) {
                    $state.usage.tool_calls = $state.usage.tool_calls.saturating_add(1);
                    $context.usage.tool_calls = $context.usage.tool_calls.saturating_add(1);
                }
                $state.pending_tool_returns.push(tool_return);
                stream_context_events!(&$state, $context_event_cursor);
                checkpoint!(AgentExecutionNode::ToolReturn, &$state, $context_event_cursor);
            }};
        }

        let initial_input = prompt.into();
        let durable_run_id = durable_run_id_from_context(context);
        context.prepare_new_run();
        let run_id = durable_run_id
            .or_else(|| context.run_id.clone())
            .unwrap_or_default();
        context.run_id = Some(run_id.clone());
        let conversation_id = context.conversation_id.clone();
        let agent_id = context.agent_id.as_str().to_string();
        let agent_name = context
            .agent_registry
            .get(&agent_id)
            .map_or_else(|| agent_id.clone(), |info| info.agent_name.clone());
        let mut run_span = ActiveSpan::start(
            &self.trace_recorder,
            SpanSpec::new("gen_ai.invoke_agent")
                .with_attribute("gen_ai.operation.name", serde_json::json!("invoke_agent"))
                .with_attribute("gen_ai.agent.id", serde_json::json!(agent_id))
                .with_attribute("gen_ai.agent.name", serde_json::json!(agent_name))
                .with_attribute(
                    "gen_ai.conversation.id",
                    serde_json::json!(conversation_id.as_str()),
                )
                .with_attribute("starweaver.run.id", serde_json::json!(run_id.as_str())),
            &context.trace_context,
            "agent_run_dropped",
        );
        context.trace_context = run_span.context().clone();
        if let Some(model_config) = self.model_config.clone() {
            context.merge_model_config(model_config);
        }
        if let Some(tool_config) = self.tool_config.clone() {
            context.merge_tool_config(tool_config);
        }
        let history_len = context.message_history.len();
        let mut state = AgentRunState::new(run_id.clone(), conversation_id.clone());
        state.message_history.clone_from(&context.message_history);
        state.usage = context.usage.clone();
        state.pending_tool_returns = std::mem::take(&mut context.pending_tool_returns);
        state.parent_run_id.clone_from(&context.parent_run_id);
        state.parent_task_id.clone_from(&context.parent_task_id);
        state.status = RunStatus::Running;
        Self::sync_compact_context_metadata(context, &mut state);
        let mut context_event_cursor = context.events.len();
        let mut model_session = self.model.start_run_session();
        let run_result = Box::pin(async {
        context.publish_event(AgentEvent::new(
            "run_start",
            serde_json::json!({"run_id": run_id.as_str()}),
        ));
        stream_event!(
            &state,
            AgentStreamEvent::RunStart {
                run_id: run_id.clone(),
                conversation_id: conversation_id.clone(),
            }
        );
        let initial_input = match self
            .prepare_run_input(&mut state, context, initial_input)
            .await
        {
            Ok(input) => input,
            Err(error) => {
                fail_run!(&mut state, &run_id, context_event_cursor, error);
            }
        };
        let initial_prompt = initial_input.text_projection();
        let initial_content = if initial_input.is_empty() {
            vec![starweaver_model::ContentPart::text(initial_prompt.clone())]
        } else {
            initial_input.content
        };
        let has_pending_tool_returns = !state.pending_tool_returns.is_empty();
        context.user_prompts = if has_pending_tool_returns {
            None
        } else {
            Some(initial_content.clone())
        };
        context.previous_assistant_response_reference =
            Self::previous_assistant_response_reference(&context.message_history);
        Self::sync_compact_context_metadata(context, &mut state);
        self.call_run_start(&mut state, context).await?;
        context.runtime.current_run_step = state.run_step;
        let mut run_tools = match self.prepare_run_tools(context, true).await {
            Ok(tools) => tools,
            Err(error) => {
                fail_run!(&mut state, &run_id, context_event_cursor, error);
            }
        };
        stream_context_events!(&state, context_event_cursor);
        checkpoint!(AgentExecutionNode::RunStart, &state, context_event_cursor);

        let mut next_prompt = initial_prompt;
        let mut is_initial_request = true;
        let mut output_retries_used = 0;
        let mut model_error_retries_used = 0usize;
        let mut tool_retries = BTreeMap::<String, usize>::new();
            'agent_loop: loop {
            let mut step_span = ActiveSpan::start(
                &self.trace_recorder,
                SpanSpec::new("starweaver.loop.step")
                    .with_attribute("starweaver.run.step", serde_json::json!(state.run_step)),
                run_span.context(),
                "loop_step_dropped",
            );
            if state.run_step >= self.policy.max_steps {
                step_span.close(SpanStatus::Error {
                    error_type: "step_limit_exceeded".to_string(),
                });
                run_span.close(SpanStatus::Error {
                    error_type: "step_limit_exceeded".to_string(),
                });
                fail_run!(
                    &mut state,
                    &run_id,
                    context_event_cursor,
                    AgentError::StepLimitExceeded {
                        steps: state.run_step,
                    }
                );
            }

            context.runtime.current_run_step = state.run_step;
            if state.run_step > 0 {
                run_tools = match self.prepare_run_tools(context, false).await {
                    Ok(tools) => tools,
                    Err(error) => {
                        step_span.close(SpanStatus::Error {
                            error_type: "capability_error".to_string(),
                        });
                        run_span.close(SpanStatus::Error {
                            error_type: "capability_error".to_string(),
                        });
                        fail_run!(
                            &mut state,
                            &run_id,
                            context_event_cursor,
                            error
                        );
                    }
                };
                stream_context_events!(&state, context_event_cursor);
            }

            checkpoint!(
                AgentExecutionNode::PrepareModelRequest,
                &state,
                context_event_cursor
            );
            let PrepareRequestResult {
                request,
                settings,
                transition,
            } = self
                .prepare_request_phase(
                    &mut state,
                    context,
                    PrepareRequestInput {
                        prompt: &next_prompt,
                        initial_content: &initial_content,
                        is_initial_request,
                        run_id: &run_id,
                        conversation_id: &conversation_id,
                    },
                )
                .await?;
            is_initial_request = false;
            let response_was_skipped = matches!(
                &transition,
                PrepareRequestTransition::ClassifyResponse { .. }
            );
            if state.run_step == 0 {
                Self::capture_effective_user_prompt_for_compact_restore(context, &request);
                Self::sync_compact_context_metadata(context, &mut state);
            }
            if !response_was_skipped {
                Self::sync_compact_context_metadata(context, &mut state);
                stream_context_events!(&state, context_event_cursor);
            }
            state.message_history.push(ModelMessage::Request(request));
            context.message_history.clone_from(&state.message_history);
            stream_event!(
                &state,
                AgentStreamEvent::ModelRequest {
                    step: state.run_step,
                }
            );
            state.pending_tool_returns.clear();
            stream_context_events!(&state, context_event_cursor);
            checkpoint!(
                AgentExecutionNode::BeforeModelRequest,
                &state,
                context_event_cursor
            );

            let skipped_response = match transition {
                PrepareRequestTransition::ClassifyResponse { response } => Some(*response),
                PrepareRequestTransition::CallModel => None,
            };
            let response = if let Some(response) = skipped_response {
                response
            } else {
                let CallModelPreparation {
                    mut messages,
                    params,
                    transition: call_model_transition,
                } = self
                    .prepare_model_call_phase(
                        &mut state,
                        context,
                        &run_tools,
                        &run_id,
                        &conversation_id,
                    )
                    .await?;
                match call_model_transition {
                    CallModelPreparationTransition::ApplySteering { request_index } => {
                        let Some(ModelMessage::Request(request)) =
                            messages.get_mut(request_index)
                        else {
                            unreachable!("steering target must remain a model request");
                        };
                        Self::apply_runtime_steering_messages(context, request);
                        Self::sync_compact_context_metadata(context, &mut state);
                        stream_context_events!(&state, context_event_cursor);
                    }
                    CallModelPreparationTransition::PrepareProvider => {}
                }
                Self::validate_model_request_messages(&messages)?;
                state.message_history.clone_from(&messages);
                context.message_history.clone_from(&state.message_history);
                let prepared_provider_request = self
                    .prepare_provider_request_phase(
                        &mut state,
                        context,
                        messages,
                        settings,
                        params,
                    )
                    .await?;
                let mut model_spec = SpanSpec::new("gen_ai.inference")
                    .with_kind(SpanKind::Client)
                    .with_attribute("gen_ai.operation.name", serde_json::json!("chat"))
                    .with_attribute(
                        "gen_ai.agent.id",
                        serde_json::json!(context.agent_id.as_str()),
                    )
                    .with_attribute(
                        "gen_ai.conversation.id",
                        serde_json::json!(conversation_id.as_str()),
                    )
                    .with_attribute("starweaver.run.id", serde_json::json!(run_id.as_str()))
                    .with_attribute(
                        "gen_ai.request.model",
                        serde_json::json!(self.model.model_name()),
                    );
                if let Some(provider_name) = self.model.provider_name() {
                    model_spec = model_spec
                        .with_attribute("gen_ai.provider.name", serde_json::json!(provider_name));
                }
                let mut model_span = ActiveSpan::start(
                    &self.trace_recorder,
                    model_spec,
                    step_span.context(),
                    "model_request_dropped",
                );
                self.record_model_request_event(
                    &model_span,
                    &prepared_provider_request.messages,
                    prepared_provider_request.settings.as_ref(),
                    &prepared_provider_request.params,
                );
                let request_context =
                    ModelRequestContext::new(run_id.clone(), conversation_id.clone())
                        .with_trace_context(model_span.context().clone())
                        .with_llm_trace_metadata(context.metadata.clone());
                let request_context = if let Some(token) = self.cancellation_token.as_ref() {
                    request_context.with_cancellation_token(token.clone())
                } else {
                    request_context
                };
                macro_rules! recover_model_error {
                    ($error:expr) => {{
                        let error = $error;
                        match error {
                            starweaver_model::ModelError::Cancelled { reason } => {
                                model_span.close(SpanStatus::Error {
                                    error_type: "model_cancelled".to_string(),
                                });
                                step_span.close(SpanStatus::Error {
                                    error_type: "model_cancelled".to_string(),
                                });
                                run_span.close(SpanStatus::Error {
                                    error_type: "model_cancelled".to_string(),
                                });
                                fail_run!(
                                    &mut state,
                                    &run_id,
                                    context_event_cursor,
                                    AgentError::Cancelled { reason }
                                );
                            }
                            error => {
                                model_span.close(SpanStatus::Error {
                                    error_type: "model_error".to_string(),
                                });
                                let recovery =
                                    recover_retry_message_history(&error, &state.message_history);
                                if recovery.reasons.is_empty()
                                    || model_error_retries_used >= DEFAULT_MODEL_ERROR_RETRIES
                                {
                                    step_span.close(SpanStatus::Error {
                                        error_type: "model_error".to_string(),
                                    });
                                    run_span.close(SpanStatus::Error {
                                        error_type: "model_error".to_string(),
                                    });
                                    fail_run!(
                                        &mut state,
                                        &run_id,
                                        context_event_cursor,
                                        AgentError::Model(error)
                                    );
                                }
                                model_error_retries_used =
                                    model_error_retries_used.saturating_add(1);
                                let recovery_changed = recovery.changed;
                                let recovery_reasons = recovery.reasons.clone();
                                if recovery_changed {
                                    state.message_history = recovery.history;
                                    context.message_history.clone_from(&state.message_history);
                                }
                                context.publish_event(AgentEvent::new(
                                    "model_error_retry",
                                    serde_json::json!({
                                        "run_id": run_id.as_str(),
                                        "retry": model_error_retries_used,
                                        "max_retries": DEFAULT_MODEL_ERROR_RETRIES,
                                        "error": error.public_message(),
                                        "recovery_changed": recovery_changed,
                                        "recovery_reasons": recovery_reasons,
                                    }),
                                ));
                                stream_context_events!(&state, context_event_cursor);
                                next_prompt = DEFAULT_MODEL_ERROR_RESUME_PROMPT.to_string();
                                step_span.close(SpanStatus::Ok);
                                continue 'agent_loop;
                            }
                        }
                    }};
                }
                let invocation_mode = if stream_events.is_some() {
                    ProviderInvocationMode::Incremental
                } else {
                    ProviderInvocationMode::FinalOnly
                };
                let mut provider_invocation = ProviderInvocation::new(
                    prepared_provider_request,
                    request_context,
                    invocation_mode,
                );
                let mut response_for_attempt = None;
                let response = loop {
                    let invocation_step = provider_invocation.next(&mut *model_session).await;
                    let invocation_step = match invocation_step {
                        ProviderInvocationStep::StreamAttemptEnded => provider_invocation
                            .finish_stream_attempt(response_for_attempt.take()),
                        step => step,
                    };
                    match invocation_step {
                        ProviderInvocationStep::StreamEvent(mut model_event) => {
                            if let ModelResponseStreamEvent::FinalResult(final_response) =
                                &mut model_event
                            {
                                for part in &mut final_response.parts {
                                    context.runtime.tool_id_wrapper.wrap_response_part(part);
                                }
                            }
                            if let ModelResponseStreamEvent::Diagnostic(diagnostic) = &model_event {
                                context.publish_event(
                                    AgentEvent::new(
                                        diagnostic.kind.clone(),
                                        diagnostic.payload.clone(),
                                    )
                                    .with_metadata(diagnostic.metadata.clone()),
                                );
                                stream_context_events!(&state, context_event_cursor);
                            } else {
                                stream_event!(
                                    &state,
                                    AgentStreamEvent::ModelStream {
                                        step: state.run_step,
                                        event: model_event.clone(),
                                    }
                                );
                                self.record_model_stream_event(&model_span, &model_event);
                                if let ModelResponseStreamEvent::FinalResult(final_response) =
                                    model_event
                                {
                                    response_for_attempt = Some(*final_response);
                                }
                            }
                        }
                        ProviderInvocationStep::StreamResume(resume) => {
                            response_for_attempt = None;
                            context.publish_event(AgentEvent::new(
                                "model_stream_resume",
                                serde_json::json!({
                                    "run_id": run_id.as_str(),
                                    "retry": resume.retry,
                                    "max_retries": resume.max_retries,
                                    "error": resume.cause.public_message(),
                                }),
                            ));
                            stream_context_events!(&state, context_event_cursor);
                        }
                        ProviderInvocationStep::Complete(response) => break response,
                        ProviderInvocationStep::ModelError(error) => {
                            recover_model_error!(error);
                        }
                        ProviderInvocationStep::MissingFinalResult => {
                            model_span.close(SpanStatus::Error {
                                error_type: "missing_final_result".to_string(),
                            });
                            step_span.close(SpanStatus::Error {
                                error_type: "missing_final_result".to_string(),
                            });
                            run_span.close(SpanStatus::Error {
                                error_type: "missing_final_result".to_string(),
                            });
                            fail_run!(
                                &mut state,
                                &run_id,
                                context_event_cursor,
                                AgentError::Capability(
                                    "model stream did not produce a final result".to_string(),
                                )
                            );
                        }
                        ProviderInvocationStep::StreamAttemptEnded => {
                            unreachable!("stream attempt end must be classified before dispatch");
                        }
                    }
                };
                self.record_model_response_event(&model_span, &response);
                model_span.close(SpanStatus::Ok);
                response
            };
            let mut response = response;
            response.run_id.get_or_insert_with(|| run_id.clone());
            response
                .conversation_id
                .get_or_insert_with(|| conversation_id.clone());
            response.timestamp.get_or_insert_with(chrono::Utc::now);
            for part in &mut response.parts {
                context.runtime.tool_id_wrapper.wrap_response_part(part);
            }
            state.run_step += 1;
            context.runtime.current_run_step = state.run_step;
            let response_usage = response.usage.clone();
            stream_event!(
                &state,
                AgentStreamEvent::ModelResponse {
                    step: state.run_step,
                    response: response.clone(),
                }
            );
            state.apply_model_response(response.clone());
            context.add_usage(&response_usage);
            if !response_usage.is_empty() {
                let agent_id = context.agent_id.as_str().to_string();
                let ledger_key = agent_id.clone();
                let previous_entry = context
                    .usage_snapshot_entries
                    .get(&ledger_key)
                    .map(|entry| (entry.usage.clone(), entry.estimate_pricing.clone()));
                let agent_usage = previous_entry.as_ref().map_or_else(
                    || state.usage.clone(),
                    |(entry_usage, _)| {
                        let mut usage = entry_usage.clone();
                        usage.add_assign(&response_usage);
                        usage
                    },
                );
                let model_id = self.usage_model_id(&response);
                let estimate_pricing = self
                    .usage_limits
                    .as_ref()
                    .and_then(|limits| limits.estimate_pricing(&agent_usage))
                    .or_else(|| {
                        known_model_pricing_profile(&model_id).map(|profile| {
                            if profile.is_tiered() {
                                let mut estimate = previous_entry
                                    .as_ref()
                                    .and_then(|(_, estimate)| estimate.clone())
                                    .unwrap_or_default();
                                estimate.add_assign(&profile.estimate(&response_usage));
                                estimate
                            } else {
                                profile.estimate(&agent_usage)
                            }
                        })
                    });
                let mut snapshot = context.update_usage_snapshot_entry(
                    agent_id.clone(),
                    agent_id.clone(),
                    model_id,
                    agent_usage,
                    estimate_pricing,
                    Some(format!("{}:{agent_id}", run_id.as_str())),
                    "model_request",
                    Some(ledger_key),
                );
                snapshot.latest_usage = Some(response_usage.clone());
                context.publish_event(AgentEvent::new(
                    "usage_snapshot",
                    serde_json::to_value(snapshot).unwrap_or_else(|_| serde_json::json!({})),
                ));
            }
            stream_context_events!(&state, context_event_cursor);
            checkpoint!(
                AgentExecutionNode::ModelResponse,
                &state,
                context_event_cursor
            );
            if let Err(error) = self.check_usage(&state) {
                step_span.close(SpanStatus::Error {
                    error_type: "usage_limit".to_string(),
                });
                run_span.close(SpanStatus::Error {
                    error_type: "usage_limit".to_string(),
                });
                fail_run!(
                    &mut state,
                    &run_id,
                    context_event_cursor,
                    error
                );
            }
            context.message_history.clone_from(&state.message_history);

            let ClassifyResponseResult {
                response,
                transition: response_transition,
            } = self.classify_response_phase(&mut state, context).await?;

            let tool_calls = match response_transition {
                ClassifyResponseTransition::PrepareTools { tool_calls } => Some(tool_calls),
                ClassifyResponseTransition::ValidateOutput => None,
            };
            if let Some(tool_calls) = tool_calls {
                let tools_transition = match self
                    .prepare_tools_phase(
                        &mut state,
                        context,
                        tool_calls,
                        &run_tools,
                        output_retries_used,
                    )
                    .await
                {
                    Ok(transition) => transition,
                    Err(error) => {
                        let error_type = match &error {
                            AgentError::OutputRetryLimitExceeded { .. } => {
                                "output_retry_limit_exceeded"
                            }
                            AgentError::ToolCallsRequireTools => "tool_calls_require_tools",
                            AgentError::UsageLimit(_) => "usage_limit",
                            _ => "capability_error",
                        };
                        step_span.close(SpanStatus::Error {
                            error_type: error_type.to_string(),
                        });
                        run_span.close(SpanStatus::Error {
                            error_type: error_type.to_string(),
                        });
                        fail_run!(&mut state, &run_id, context_event_cursor, error);
                    }
                };
                let (tool_calls, final_output_after_tools) = match tools_transition {
                    PrepareToolsTransition::ExecuteTools {
                        tool_calls,
                        final_output_after_tools,
                    } => (tool_calls, final_output_after_tools),
                    PrepareToolsTransition::PrepareRequestForOutputRetry { prompt, retries } => {
                        output_retries_used = retries;
                        self.call_retry(
                            &mut state,
                            context,
                            RetryEventKind::Output,
                            output_retries_used,
                            &prompt,
                        )
                        .await?;
                        stream_event!(
                            &state,
                            AgentStreamEvent::OutputRetry {
                                retries: output_retries_used,
                                prompt: prompt.clone(),
                            }
                        );
                        next_prompt = prompt;
                        step_span.close(SpanStatus::Ok);
                        continue;
                    }
                    PrepareToolsTransition::PrepareRequestForSteering { prompt } => {
                        stream_event!(
                            &state,
                            AgentStreamEvent::SteeringGuard {
                                step: state.run_step,
                                prompt: prompt.clone(),
                            }
                        );
                        next_prompt = prompt;
                        step_span.close(SpanStatus::Ok);
                        continue;
                    }
                    PrepareToolsTransition::Finalize {
                        output,
                        structured_output,
                    } => {
                        let FinalizeTransition::Complete { output } =
                            Self::finalize_phase(&mut state, output, structured_output);
                        complete_run!(
                            state,
                            output,
                            &run_id,
                            context_event_cursor,
                            step_span,
                            run_span,
                            history_len
                        );
                    }
                };
                if self.should_execute_tool_calls_sequentially(&run_tools, &tool_calls) {
                    for call in &tool_calls {
                        checkpoint!(AgentExecutionNode::ToolCall, &state, context_event_cursor);
                        stream_event!(
                            &state,
                            AgentStreamEvent::ToolCall {
                                step: state.run_step,
                                call: call.clone(),
                            }
                        );
                        let PreparedToolExecution {
                            index: _,
                            call,
                            tool_context,
                            dependency_assembly,
                            stream_sink,
                            tool_span,
                            started_at,
                        } = self
                            .prepare_tool_execution(
                                0,
                                &mut state,
                                context,
                                &run_tools,
                                call,
                                step_span.context(),
                                &run_id,
                                &conversation_id,
                                &tool_retries,
                                stream_events.is_some(),
                            )
                            .await?;
                        let mut tool_return_future =
                            Box::pin(run_tools.execute_call(tool_context, &call));
                        let mut child_stream_tick =
                            tokio::time::interval(std::time::Duration::from_millis(50));
                        child_stream_tick
                            .set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        let tool_return = loop {
                            tokio::select! {
                                tool_return = &mut tool_return_future => {
                                    if let Some(stream_sink) = &stream_sink {
                                        for child_record in stream_sink.drain() {
                                            stream_record!(&state, child_record);
                                        }
                                    }
                                    break tool_return;
                                }
                                _ = child_stream_tick.tick(), if stream_sink.is_some() => {
                                    if let Some(stream_sink) = &stream_sink {
                                        for child_record in stream_sink.drain() {
                                            stream_record!(&state, child_record);
                                        }
                                    }
                                }
                            }
                        };
                        let tool_duration = started_at.elapsed();
                        apply_tool_return!(
                            state,
                            context,
                            tool_retries,
                            &run_tools,
                            step_span,
                            run_span,
                            &run_id,
                            context_event_cursor,
                            &call,
                            tool_return,
                            tool_span,
                            &dependency_assembly,
                            tool_duration
                        );
                    }
                } else {
                    let mut prepared_calls = Vec::with_capacity(tool_calls.len());
                    for (index, call) in tool_calls.iter().enumerate() {
                        checkpoint!(AgentExecutionNode::ToolCall, &state, context_event_cursor);
                        stream_event!(
                            &state,
                            AgentStreamEvent::ToolCall {
                                step: state.run_step,
                                call: call.clone(),
                            }
                        );
                        prepared_calls.push(
                            self.prepare_tool_execution(
                                index,
                                &mut state,
                                context,
                                &run_tools,
                                call,
                                step_span.context(),
                                &run_id,
                                &conversation_id,
                                &tool_retries,
                                stream_events.is_some(),
                            )
                            .await?,
                        );
                    }

                    let mut join_set = tokio::task::JoinSet::new();
                    for prepared_call in &prepared_calls {
                        let run_tools = run_tools.clone();
                        let index = prepared_call.index;
                        let call = prepared_call.call.clone();
                        let tool_context = prepared_call.tool_context.clone();
                        join_set.spawn(Self::execute_prepared_tool_call(
                            run_tools,
                            index,
                            call,
                            tool_context,
                        ));
                    }
                    let mut tool_returns: Vec<Option<ToolReturnPart>> =
                        vec![None; prepared_calls.len()];
                    let mut child_stream_tick =
                        tokio::time::interval(std::time::Duration::from_millis(50));
                    child_stream_tick
                        .set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    let mut remaining_tool_calls = prepared_calls.len();
                    while remaining_tool_calls > 0 {
                        tokio::select! {
                            joined = join_set.join_next() => {
                                let Some(joined) = joined else {
                                    break;
                                };
                                let (index, tool_return) = match joined {
                                    Ok(result) => result,
                                    Err(error) => {
                                        step_span.close(SpanStatus::Error {
                                            error_type: "tool_execution_task_failed".to_string(),
                                        });
                                        run_span.close(SpanStatus::Error {
                                            error_type: "tool_execution_task_failed".to_string(),
                                        });
                                        fail_run!(
                                            &mut state,
                                            &run_id,
                                            context_event_cursor,
                                            AgentError::Capability(format!(
                                                "tool execution task failed: {error}"
                                            ))
                                        );
                                    }
                                };
                                tool_returns[index] = Some(tool_return);
                                remaining_tool_calls = remaining_tool_calls.saturating_sub(1);
                                for prepared_call in &prepared_calls {
                                    if let Some(stream_sink) = &prepared_call.stream_sink {
                                        for child_record in stream_sink.drain() {
                                            stream_record!(&state, child_record);
                                        }
                                    }
                                }
                            }
                            _ = child_stream_tick.tick(), if stream_events.is_some() => {
                                for prepared_call in &prepared_calls {
                                    if let Some(stream_sink) = &prepared_call.stream_sink {
                                        for child_record in stream_sink.drain() {
                                            stream_record!(&state, child_record);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    for prepared_call in &prepared_calls {
                        if let Some(stream_sink) = &prepared_call.stream_sink {
                            for child_record in stream_sink.drain() {
                                stream_record!(&state, child_record);
                            }
                        }
                    }
                    for prepared_call in prepared_calls {
                        let PreparedToolExecution {
                            index,
                            call,
                            tool_context: _,
                            dependency_assembly,
                            stream_sink: _,
                            tool_span,
                            started_at,
                        } = prepared_call;
                        let Some(tool_return) = tool_returns[index].take() else {
                            step_span.close(SpanStatus::Error {
                                error_type: "tool_execution_missing_return".to_string(),
                            });
                            run_span.close(SpanStatus::Error {
                                error_type: "tool_execution_missing_return".to_string(),
                            });
                            fail_run!(
                                &mut state,
                                &run_id,
                                context_event_cursor,
                                AgentError::Capability(
                                    "tool execution task ended without a return".to_string()
                                )
                            );
                        };
                        let tool_duration = started_at.elapsed();
                        apply_tool_return!(
                            state,
                            context,
                            tool_retries,
                            &run_tools,
                            step_span,
                            run_span,
                            &run_id,
                            context_event_cursor,
                            &call,
                            tool_return,
                            tool_span,
                            &dependency_assembly,
                            tool_duration
                        );
                    }
                }
                match Self::execute_tools_phase(
                    &mut state,
                    context,
                    final_output_after_tools,
                    &run_id,
                    &conversation_id,
                ) {
                    ExecuteToolsTransition::AwaitExternal => {
                        let AwaitExternalTransition::Suspend { node, reason } =
                            Self::await_external_phase(&mut state, context);
                        for deferred in &state.deferred_tool_returns {
                            context.publish_event(AgentEvent::new(
                                starweaver_core::DEFERRED_TOOL_REQUESTED_EVENT_KIND,
                                serde_json::json!({
                                    "run_id": run_id.as_str(),
                                    "tool_call_id": deferred.tool_call_id.as_str(),
                                    "tool_name": deferred.name.as_str(),
                                    "deferred_id": format!(
                                        "deferred_{}_{}",
                                        run_id.as_str(),
                                        deferred.tool_call_id.as_str()
                                    ),
                                    "request": deferred.metadata.get("deferred"),
                                }),
                            ));
                        }
                        context.publish_event(AgentEvent::new(
                            "run_waiting",
                            serde_json::json!({
                                "run_id": run_id.as_str(),
                                "pending_approvals": state.pending_approval_tool_returns.len(),
                                "deferred_tools": state.deferred_tool_returns.len(),
                            }),
                        ));
                        stream_context_events!(&state, context_event_cursor);
                        stream_event!(
                            &state,
                            AgentStreamEvent::Suspended {
                                node,
                                reason,
                            }
                        );
                        checkpoint!(node, &state, context_event_cursor);
                        close_run_toolsets!(&state, context_event_cursor);
                        step_span.close(SpanStatus::Ok);
                        run_span.close(SpanStatus::Ok);
                        context.finish_run();
                        return Ok(RunLoopExit::Waiting);
                    }
                    ExecuteToolsTransition::PrepareRequest { prompt } => {
                        if !prompt.is_empty() {
                            stream_event!(
                                &state,
                                AgentStreamEvent::SteeringGuard {
                                    step: state.run_step,
                                    prompt: prompt.clone(),
                                }
                            );
                        }
                        next_prompt = prompt;
                        step_span.close(SpanStatus::Ok);
                        continue;
                    }
                    ExecuteToolsTransition::Finalize {
                        output,
                        structured_output,
                    } => {
                        let FinalizeTransition::Complete { output } =
                            Self::finalize_phase(&mut state, output, structured_output);
                        complete_run!(
                            state,
                            output,
                            &run_id,
                            context_event_cursor,
                            step_span,
                            run_span,
                            history_len
                        );
                    }
                }
            }

            let output = response.text_output();
            checkpoint!(
                AgentExecutionNode::ValidateOutput,
                &state,
                context_event_cursor
            );
            match self
                .validate_final_output(&mut state, context, &output)
                .await
            {
                Ok(()) if !response_was_skipped && Self::has_pending_steering_messages(context) => {
                    let Some(message) = Self::pending_steering_guard_message(context) else {
                        unreachable!("pending steering guard message must exist");
                    };
                    stream_event!(
                        &state,
                        AgentStreamEvent::SteeringGuard {
                            step: state.run_step,
                            prompt: message.clone(),
                        }
                    );
                    next_prompt = message;
                    step_span.close(SpanStatus::Ok);
                }
                Ok(()) => {
                    let structured_output = state.structured_output.clone();
                    let FinalizeTransition::Complete { output } =
                        Self::finalize_phase(&mut state, output, structured_output);
                    complete_run!(
                        state,
                        output,
                        &run_id,
                        context_event_cursor,
                        step_span,
                        run_span,
                        history_len
                    );
                }
                Err(CapabilityError::ModelRetry(message)) => {
                    if output_retries_used >= self.policy.output_retries {
                        step_span.close(SpanStatus::Error {
                            error_type: "output_retry_limit_exceeded".to_string(),
                        });
                        run_span.close(SpanStatus::Error {
                            error_type: "output_retry_limit_exceeded".to_string(),
                        });
                        fail_run!(
                            &mut state,
                            &run_id,
                            context_event_cursor,
                            AgentError::OutputRetryLimitExceeded {
                                retries: output_retries_used,
                            }
                        );
                    }
                    output_retries_used += 1;
                    self.call_retry(
                        &mut state,
                        context,
                        RetryEventKind::Output,
                        output_retries_used,
                        &message,
                    )
                    .await?;
                    stream_event!(
                        &state,
                        AgentStreamEvent::OutputRetry {
                            retries: output_retries_used,
                            prompt: message.clone(),
                        }
                    );
                    next_prompt = message;
                    step_span.close(SpanStatus::Ok);
                }
                Err(error) => {
                    step_span.close(SpanStatus::Error {
                        error_type: "capability_error".to_string(),
                    });
                    run_span.close(SpanStatus::Error {
                        error_type: "capability_error".to_string(),
                    });
                    fail_run!(
                        &mut state,
                        &run_id,
                        context_event_cursor,
                        Self::capability_error(error)
                    );
                }
            }
        }
        })
        .await;
        model_session.close().await;
        match run_result {
            Ok(exit) => {
                let (output, structured_output) = match exit {
                    RunLoopExit::Completed { output } => (output, state.structured_output.clone()),
                    RunLoopExit::Waiting => (String::new(), None),
                };
                Ok(AgentResult {
                    output,
                    structured_output,
                    messages: state.message_history.clone(),
                    state,
                    history_len,
                })
            }
            Err(error) if context.runtime.lifecycle.entered => {
                run_span.close(SpanStatus::Error {
                    error_type: agent_error_kind(&error).to_string(),
                });
                fail_run!(&mut state, &run_id, context_event_cursor, error);
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use starweaver_core::{AgentId, ConversationId, RunId};

    use super::*;

    fn test_state() -> AgentRunState {
        AgentRunState::new(
            RunId::from_string("run-phase"),
            ConversationId::from_string("conversation-phase"),
        )
    }

    #[test]
    fn execute_tools_routes_hitl_before_held_final_output() {
        let mut state = test_state();
        let mut approval = ToolReturnPart::new("call-1", "review", json!({"pending": true}));
        approval
            .metadata
            .insert("control_flow".to_string(), json!("approval_required"));
        state.pending_tool_returns.push(approval.clone());
        state.pending_approval_tool_returns.push(approval);
        let context = AgentContext::new(AgentId::from_string("agent"));
        let run_id = state.run_id.clone();
        let conversation_id = state.conversation_id.clone();

        let transition = Agent::execute_tools_phase(
            &mut state,
            &context,
            Some(("held".to_string(), Some(json!({"ok": true})))),
            &run_id,
            &conversation_id,
        );

        assert!(matches!(transition, ExecuteToolsTransition::AwaitExternal));
        assert!(state.pending_tool_returns.is_empty());
        assert!(state.message_history.is_empty());
    }

    #[test]
    fn execute_tools_routes_normal_and_held_outputs_explicitly() {
        let context = AgentContext::new(AgentId::from_string("agent"));
        let mut state = test_state();
        let run_id = state.run_id.clone();
        let conversation_id = state.conversation_id.clone();
        assert!(matches!(
            Agent::execute_tools_phase(
                &mut state,
                &context,
                None,
                &run_id,
                &conversation_id,
            ),
            ExecuteToolsTransition::PrepareRequest { prompt } if prompt.is_empty()
        ));

        state.pending_tool_returns.push(ToolReturnPart::new(
            "call-2",
            "ordinary",
            json!({"ok": true}),
        ));
        let transition = Agent::execute_tools_phase(
            &mut state,
            &context,
            Some(("held".to_string(), Some(json!({"ok": true})))),
            &run_id,
            &conversation_id,
        );
        assert!(matches!(
            transition,
            ExecuteToolsTransition::Finalize {
                output,
                structured_output: Some(value),
            } if output == "held" && value == json!({"ok": true})
        ));
        assert!(state.pending_tool_returns.is_empty());
        assert_eq!(state.message_history.len(), 1);
    }

    #[test]
    fn terminal_phases_classify_status_and_finalize_state() {
        let mut context = AgentContext::new(AgentId::from_string("agent"));
        let mut state = test_state();
        let cancelled = Agent::fail_or_cancel_phase(
            &mut state,
            &mut context,
            &AgentError::Cancelled {
                reason: "stop".to_string(),
            },
        );
        assert!(matches!(
            cancelled,
            FailOrCancelTransition::Cancelled { reason } if reason == "agent run cancelled"
        ));
        assert_eq!(state.status, RunStatus::Cancelled);

        let completed =
            Agent::finalize_phase(&mut state, "done".to_string(), Some(json!({"answer": 42})));
        assert!(matches!(
            completed,
            FinalizeTransition::Complete { output } if output == "done"
        ));
        assert_eq!(state.status, RunStatus::Completed);
        assert_eq!(state.output.as_deref(), Some("done"));
        assert_eq!(state.structured_output, Some(json!({"answer": 42})));

        let failed = Agent::fail_or_cancel_phase(
            &mut state,
            &mut context,
            &AgentError::Capability("internal detail".to_string()),
        );
        assert!(matches!(failed, FailOrCancelTransition::Failed { .. }));
        assert_eq!(state.status, RunStatus::Failed);
    }

    #[test]
    fn durable_run_id_prefers_starweaver_metadata() {
        let mut context = AgentContext::new(AgentId::from_string("agent"));
        context.metadata.insert(
            DURABLE_RUN_ID_METADATA_KEY.to_string(),
            json!("run_durable"),
        );
        context
            .metadata
            .insert(CLI_RUN_ID_METADATA_KEY.to_string(), json!("run_cli"));

        assert_eq!(
            durable_run_id_from_context(&context),
            Some(RunId::from_string("run_durable"))
        );
    }

    #[test]
    fn durable_run_id_does_not_apply_to_subagent_context() {
        let mut context = AgentContext::new(AgentId::from_string("agent"));
        context.parent_run_id = Some(RunId::from_string("run_parent"));
        context.metadata.insert(
            DURABLE_RUN_ID_METADATA_KEY.to_string(),
            json!("run_durable"),
        );

        assert_eq!(durable_run_id_from_context(&context), None);
    }
}
