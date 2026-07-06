//! Agent run loop entrypoints.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use starweaver_context::{AgentContext, AgentContextHandle, AgentEvent};
use starweaver_core::{ConversationId, RunId, TraceContext};
use starweaver_model::{
    ModelMessage, ModelRequest, ModelRequestContext, ModelRequestPart, ModelResponseStreamEvent,
    ToolCallPart, ToolReturnPart,
    transport::{RetryPolicy, should_retry_error},
};
use starweaver_tools::{ToolContext, ToolRegistry};
use starweaver_usage::pricing::estimate_pricing_for_model;

const DEFAULT_MODEL_ERROR_RETRIES: usize = 2;

mod entrypoints;

use crate::{
    agent::{
        Agent, AgentEndStrategy, AgentError, AgentInput, AgentResult, AgentToolExecutionMode,
        helpers::{
            has_pending_tool_control_flow, is_tool_retry_return, mark_tool_retry_return,
            record_tool_control_flow, tool_return_control_flow,
        },
        run_loop_helpers::{agent_error_kind, preserve_pending_tool_returns_for_resume},
        runtime_helpers::{request_instruction_insert_index, tool_return_media_prompt},
    },
    capability::{CapabilityError, RetryEventKind},
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

struct PreparedToolExecution {
    index: usize,
    call: ToolCallPart,
    tool_context: ToolContext,
    context_handle: AgentContextHandle,
    stream_sink: Option<AgentStreamSink>,
    tool_span: ActiveSpan,
    started_at: std::time::Instant,
}

#[inline(never)]
fn shared_context_dependency(context: &AgentContext) -> Arc<AgentContext> {
    Arc::new(context.clone())
}

#[inline(never)]
fn context_handle_snapshot(context: &AgentContext) -> AgentContextHandle {
    AgentContextHandle::new(context.clone())
}

#[inline(never)]
fn replace_context_handle_snapshot(handle: &AgentContextHandle, context: &AgentContext) {
    handle.replace(context.clone());
}

fn should_resume_provider_stream(error: &starweaver_model::ModelError) -> bool {
    should_retry_error(error, &RetryPolicy::default())
}

impl Agent {
    fn should_execute_tool_calls_sequentially(
        &self,
        run_tools: &ToolRegistry,
        tool_calls: &[ToolCallPart],
    ) -> bool {
        if self.policy.tool_execution == AgentToolExecutionMode::Sequential {
            return true;
        }
        let mut seen_tool_names = BTreeSet::new();
        tool_calls.iter().any(|call| {
            run_tools.sequential_for(&call.name) || !seen_tool_names.insert(call.name.as_str())
        })
    }

    #[allow(clippy::too_many_arguments)]
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
        let context_handle = context_handle_snapshot(context);
        let mut tool_dependencies = context.dependencies.clone();
        tool_dependencies.insert_arc(shared_context_dependency(context));
        tool_dependencies.insert(context_handle.clone());
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
        .with_retry_budget(tool_retry, tool_max_retries);
        if let Some(token) = self.cancellation_token.as_ref() {
            tool_context = tool_context.with_cancellation_token(token.clone());
        }
        self.call_before_tool_execution(state, context, &mut tool_context, call)
            .await?;
        replace_context_handle_snapshot(&context_handle, context);
        tool_context
            .dependencies
            .insert_arc(shared_context_dependency(context));
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
            context_handle,
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
        let result = self
            .run_with_context_inner_impl(prompt, context, stream_events)
            .await;
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
                self.close_run_toolsets(context).await;
                stream_context_events!($state, $cursor);
            }};
        }

        macro_rules! fail_run {
            ($state:expr, $run_id:expr, $event_cursor:expr, $previous_trace_context:expr, $error:expr) => {{
                let error = $error;
                let error_kind = agent_error_kind(&error).to_string();
                let message = error.to_string();
                $state.status = RunStatus::Failed;
                preserve_pending_tool_returns_for_resume($state);
                context.message_history.clone_from(&$state.message_history);
                context.usage.clone_from(&$state.usage);
                close_run_toolsets!($state, $event_cursor);
                context.finish_run();
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
                context.trace_context = $previous_trace_context;
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

        macro_rules! apply_tool_return {
            ($state:ident, $context:ident, $tool_retries:ident, $run_tools:expr, $step_span:ident, $run_span:ident, $run_id:expr, $context_event_cursor:ident, $previous_trace_context:expr, $call:expr, $tool_return:expr, $tool_span:expr, $context_handle:expr, $tool_duration:expr) => {{
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
                self.absorb_tool_context_handle(&mut $state, $context, $context_handle)?;
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
                            $previous_trace_context.clone(),
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
                if !tool_return.is_error {
                    $state.usage.tool_calls = $state.usage.tool_calls.saturating_add(1);
                    $context.usage.tool_calls = $context.usage.tool_calls.saturating_add(1);
                }
                $state.pending_tool_returns.push(tool_return);
                stream_context_events!(&$state, $context_event_cursor);
                checkpoint!(AgentExecutionNode::ToolReturn, &$state, $context_event_cursor);
            }};
        }

        let initial_input = prompt.into();
        context.prepare_new_run();
        let run_id = context.run_id.clone().unwrap_or_default();
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
        let previous_trace_context = context.trace_context.clone();
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
        state.status = RunStatus::Running;
        Self::sync_compact_context_metadata(context, &mut state);
        let mut context_event_cursor = context.events.len();
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
                fail_run!(
                    &mut state,
                    &run_id,
                    context_event_cursor,
                    previous_trace_context.clone(),
                    error
                );
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
        context.current_run_step = state.run_step;
        let mut run_tools = match self.prepare_run_tools(context, true).await {
            Ok(tools) => tools,
            Err(error) => {
                fail_run!(
                    &mut state,
                    &run_id,
                    context_event_cursor,
                    previous_trace_context.clone(),
                    error
                );
            }
        };
        stream_context_events!(&state, context_event_cursor);
        checkpoint!(AgentExecutionNode::RunStart, &state, context_event_cursor);

        let mut next_prompt = initial_prompt;
        let mut output_retries_used = 0;
        let mut model_error_retries_used = 0usize;
        let mut tool_retries = BTreeMap::<String, usize>::new();
        let mut model_session = self.model.start_run_session();
        let run_result = async {
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
                    previous_trace_context.clone(),
                    AgentError::StepLimitExceeded {
                        steps: state.run_step,
                    }
                );
            }

            context.current_run_step = state.run_step;
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
                            previous_trace_context.clone(),
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
            let dynamic_instruction_parts = self.dynamic_instruction_parts(&state).await?;
            let mut request = self.prepare_request(
                &state,
                &next_prompt,
                &initial_content,
                &run_id,
                &conversation_id,
            );
            if !dynamic_instruction_parts.is_empty() {
                let insert_at = request_instruction_insert_index(&request);
                request
                    .parts
                    .splice(insert_at..insert_at, dynamic_instruction_parts);
            }
            let mut settings = self.effective_settings(context);
            let skipped_response = self
                .call_before_model_request(&mut state, context, &mut request, &mut settings)
                .await?;
            if state.run_step == 0 {
                Self::capture_effective_user_prompt_for_compact_restore(context, &request);
                Self::sync_compact_context_metadata(context, &mut state);
            }
            if skipped_response.is_none() {
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

            let response_was_skipped = skipped_response.is_some();
            let response = if let Some(response) = skipped_response {
                response
            } else {
                self.check_before_request(&state)?;
                let mut messages = self.prepare_model_messages(&mut state, context).await?;
                context.tool_id_wrapper.wrap_messages(&mut messages);
                Self::validate_model_request_messages(&messages)?;
                self.inject_missing_static_instructions(&run_id, &conversation_id, &mut messages);
                let params = self
                    .effective_request_params(&state, context, &run_tools)
                    .await?;
                messages = Self::attach_prepared_request_instructions(messages, &params);
                for message in &mut messages {
                    Self::fill_message_metadata(message, &run_id, &conversation_id);
                }
                if Self::has_pending_steering_messages(context)
                    && let Some(ModelMessage::Request(request)) = messages
                        .iter_mut()
                        .rev()
                        .find(|message| matches!(message, ModelMessage::Request(_)))
                    {
                        Self::apply_runtime_steering_messages(context, request);
                        Self::sync_compact_context_metadata(context, &mut state);
                        stream_context_events!(&state, context_event_cursor);
                    }
                Self::validate_model_request_messages(&messages)?;
                state.message_history.clone_from(&messages);
                context.message_history.clone_from(&state.message_history);
                messages = self
                    .prepare_provider_messages(&mut state, context, messages)
                    .await?;
                Self::validate_model_request_messages(&messages)?;
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
                self.record_model_request_event(&model_span, &messages, settings.as_ref(), &params);
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
                                    previous_trace_context.clone(),
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
                                    fail_run!(&mut state, &run_id, context_event_cursor, previous_trace_context.clone(), AgentError::Model(error));
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
                                        "error": error.to_string(),
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
                if stream_events.is_some() {
                    let mut stream_resume_retries_used = 0usize;
                    let response = 'model_stream_resume: loop {
                        let mut response = None;
                        let mut model_stream = match model_session
                            .request_stream_incremental(
                                messages.clone(),
                                settings.clone(),
                                params.clone(),
                                request_context.clone(),
                            )
                            .await
                        {
                            Ok(stream) => stream,
                            Err(error)
                                if stream_resume_retries_used < DEFAULT_MODEL_ERROR_RETRIES
                                    && should_resume_provider_stream(&error) =>
                            {
                                stream_resume_retries_used =
                                    stream_resume_retries_used.saturating_add(1);
                                context.publish_event(AgentEvent::new(
                                    "model_stream_resume",
                                    serde_json::json!({
                                        "run_id": run_id.as_str(),
                                        "retry": stream_resume_retries_used,
                                        "max_retries": DEFAULT_MODEL_ERROR_RETRIES,
                                        "error": error.to_string(),
                                    }),
                                ));
                                stream_context_events!(&state, context_event_cursor);
                                continue 'model_stream_resume;
                            }
                            Err(error) => recover_model_error!(error),
                        };
                        while let Some(model_event) = model_stream.recv().await {
                            let mut model_event = match model_event {
                                Ok(event) => event,
                                Err(error)
                                    if stream_resume_retries_used < DEFAULT_MODEL_ERROR_RETRIES
                                        && should_resume_provider_stream(&error) =>
                                {
                                    stream_resume_retries_used =
                                        stream_resume_retries_used.saturating_add(1);
                                    context.publish_event(AgentEvent::new(
                                        "model_stream_resume",
                                        serde_json::json!({
                                            "run_id": run_id.as_str(),
                                            "retry": stream_resume_retries_used,
                                            "max_retries": DEFAULT_MODEL_ERROR_RETRIES,
                                            "error": error.to_string(),
                                        }),
                                    ));
                                    stream_context_events!(&state, context_event_cursor);
                                    continue 'model_stream_resume;
                                }
                                Err(error) => recover_model_error!(error),
                            };
                            if let ModelResponseStreamEvent::FinalResult(final_response) =
                                &mut model_event
                            {
                                for part in &mut final_response.parts {
                                    context.tool_id_wrapper.wrap_response_part(part);
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
                                    response = Some(*final_response);
                                }
                            }
                        }
                        if let Some(response) = response {
                            break response;
                        }
                        if stream_resume_retries_used < DEFAULT_MODEL_ERROR_RETRIES {
                            stream_resume_retries_used =
                                stream_resume_retries_used.saturating_add(1);
                            context.publish_event(AgentEvent::new(
                                "model_stream_resume",
                                serde_json::json!({
                                    "run_id": run_id.as_str(),
                                    "retry": stream_resume_retries_used,
                                    "max_retries": DEFAULT_MODEL_ERROR_RETRIES,
                                    "error": "model stream ended before final result",
                                }),
                            ));
                            stream_context_events!(&state, context_event_cursor);
                            continue 'model_stream_resume;
                        }
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
                            previous_trace_context.clone(),
                            AgentError::Capability(
                                "model stream did not produce a final result".to_string(),
                            )
                        );
                    };
                    self.record_model_response_event(&model_span, &response);
                    model_span.close(SpanStatus::Ok);
                    response
                } else {
                    let response = match model_session
                        .request_stream_final(messages, settings, params, request_context)
                        .await
                    {
                        Ok(response) => response,
                        Err(error) => recover_model_error!(error),
                    };
                    self.record_model_response_event(&model_span, &response);
                    model_span.close(SpanStatus::Ok);
                    response
                }
            };
            let mut response = response;
            response.run_id.get_or_insert_with(|| run_id.clone());
            response
                .conversation_id
                .get_or_insert_with(|| conversation_id.clone());
            response.timestamp.get_or_insert_with(chrono::Utc::now);
            for part in &mut response.parts {
                context.tool_id_wrapper.wrap_response_part(part);
            }
            state.run_step += 1;
            context.current_run_step = state.run_step;
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
                let agent_usage = context.usage_snapshot_entries.get(&ledger_key).map_or_else(
                    || state.usage.clone(),
                    |entry| {
                        let mut usage = entry.usage.clone();
                        usage.add_assign(&response_usage);
                        usage
                    },
                );
                let model_id = self.usage_model_id(&response);
                let estimate_pricing = self
                    .usage_limits
                    .as_ref()
                    .and_then(|limits| limits.estimate_pricing(&agent_usage))
                    .or_else(|| estimate_pricing_for_model(&model_id, &agent_usage));
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
                    previous_trace_context.clone(),
                    error
                );
            }
            context.message_history.clone_from(&state.message_history);

            let mut response = state
                .latest_response
                .clone()
                .ok_or_else(|| AgentError::Capability("missing latest response".to_string()))?;
            self.call_after_model_response(&mut state, context, &mut response)
                .await?;
            state.replace_latest_response(response.clone());
            context.message_history.clone_from(&state.message_history);

            let mut tool_calls = response.tool_calls();
            if !tool_calls.is_empty() {
                let mut final_output_after_tools = None;
                match self
                    .try_call_output_function(&mut state, context, &tool_calls)
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
                        } else if !Self::has_pending_steering_messages(context) {
                            state.output = Some(output.clone());
                            state.structured_output = structured_output;
                            state.status = RunStatus::Completed;
                            self.call_run_complete(&mut state, context).await?;
                            checkpoint!(
                                AgentExecutionNode::RunComplete,
                                &state,
                                context_event_cursor
                            );
                            context.message_history.clone_from(&state.message_history);
                            close_run_toolsets!(&state, context_event_cursor);
                            context.publish_event(AgentEvent::new(
                                "run_complete",
                                serde_json::json!({"run_id": run_id.as_str()}),
                            ));
                            stream_context_events!(&state, context_event_cursor);
                            stream_event!(
                                &state,
                                AgentStreamEvent::RunComplete {
                                    run_id: run_id.clone(),
                                    output: output.clone(),
                                }
                            );
                            step_span.close(SpanStatus::Ok);
                            run_span.close(SpanStatus::Ok);
                            context.finish_run();
                            context.trace_context = previous_trace_context;
                            return Ok(AgentResult {
                                output,
                                structured_output: state.structured_output.clone(),
                                messages: state.message_history.clone(),
                                state,
                                history_len,
                            });
                        } else {
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
                            continue;
                        }
                    }
                    Ok(None) => {}
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
                                previous_trace_context.clone(),
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
                        continue;
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
                            previous_trace_context.clone(),
                            Self::capability_error(error)
                        );
                    }
                }
                if run_tools.is_empty() {
                    step_span.close(SpanStatus::Error {
                        error_type: "tool_calls_require_tools".to_string(),
                    });
                    run_span.close(SpanStatus::Error {
                        error_type: "tool_calls_require_tools".to_string(),
                    });
                    fail_run!(
                        &mut state,
                        &run_id,
                        context_event_cursor,
                        previous_trace_context.clone(),
                        AgentError::ToolCallsRequireTools
                    );
                }
                state.pending_tool_calls.clone_from(&tool_calls);
                let projected_successful_tool_calls = tool_calls
                    .iter()
                    .filter(|call| run_tools.get(&call.name).is_some())
                    .count() as u64;
                if let Err(error) = self.check_tool_calls(&state, projected_successful_tool_calls) {
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
                        previous_trace_context.clone(),
                        error
                    );
                }
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
                            context_handle,
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
                            previous_trace_context,
                            &call,
                            tool_return,
                            tool_span,
                            &context_handle,
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
                                            previous_trace_context.clone(),
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
                            context_handle,
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
                                previous_trace_context.clone(),
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
                            previous_trace_context,
                            &call,
                            tool_return,
                            tool_span,
                            &context_handle,
                            tool_duration
                        );
                    }
                }
                if has_pending_tool_control_flow(&state) {
                    let non_control_flow_tool_returns = state
                        .pending_tool_returns
                        .iter()
                        .filter(|tool_return| tool_return_control_flow(tool_return).is_none())
                        .cloned()
                        .collect::<Vec<_>>();
                    if !non_control_flow_tool_returns.is_empty() {
                        let mut parts = Vec::new();
                        for tool_return in non_control_flow_tool_returns {
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
                                metadata: serde_json::json!({
                                    "starweaver.waiting.non_control_flow_tool_returns": true,
                                })
                                .as_object()
                                .cloned()
                                .unwrap_or_default(),
                            }));
                    }
                    state.pending_tool_returns.clear();
                    state.status = RunStatus::Waiting;
                    context.message_history.clone_from(&state.message_history);
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
                            node: AgentExecutionNode::ToolReturn,
                            reason: "hitl_control_flow".to_string(),
                        }
                    );
                    checkpoint!(AgentExecutionNode::ToolReturn, &state, context_event_cursor);
                    close_run_toolsets!(&state, context_event_cursor);
                    step_span.close(SpanStatus::Ok);
                    run_span.close(SpanStatus::Ok);
                    context.finish_run();
                    context.trace_context = previous_trace_context;
                    return Ok(AgentResult {
                        output: String::new(),
                        structured_output: None,
                        messages: state.message_history.clone(),
                        state,
                        history_len,
                    });
                }
                if let Some((output, structured_output)) = final_output_after_tools {
                    if !state.pending_tool_returns.is_empty() {
                        let mut parts = Vec::new();
                        for tool_return in &state.pending_tool_returns {
                            parts.push(ModelRequestPart::ToolReturn(tool_return.clone()));
                            if let Some(media_prompt) = tool_return_media_prompt(tool_return) {
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
                                metadata: serde_json::json!({
                                    "starweaver.final_output_tool_returns": true,
                                })
                                .as_object()
                                .cloned()
                                .unwrap_or_default(),
                            }));
                    }
                    state.pending_tool_returns.clear();
                    if Self::has_pending_steering_messages(context) {
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
                        continue;
                    }
                    state.output = Some(output.clone());
                    state.structured_output = structured_output;
                    state.status = RunStatus::Completed;
                    self.call_run_complete(&mut state, context).await?;
                    checkpoint!(
                        AgentExecutionNode::RunComplete,
                        &state,
                        context_event_cursor
                    );
                    context.message_history.clone_from(&state.message_history);
                    close_run_toolsets!(&state, context_event_cursor);
                    context.publish_event(AgentEvent::new(
                        "run_complete",
                        serde_json::json!({"run_id": run_id.as_str()}),
                    ));
                    stream_context_events!(&state, context_event_cursor);
                    stream_event!(
                        &state,
                        AgentStreamEvent::RunComplete {
                            run_id: run_id.clone(),
                            output: output.clone(),
                        }
                    );
                    step_span.close(SpanStatus::Ok);
                    run_span.close(SpanStatus::Ok);
                    context.finish_run();
                    context.trace_context = previous_trace_context;
                    return Ok(AgentResult {
                        output,
                        structured_output: state.structured_output.clone(),
                        messages: state.message_history.clone(),
                        state,
                        history_len,
                    });
                }
                next_prompt.clear();
                step_span.close(SpanStatus::Ok);
                continue;
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
                    state.output = Some(output.clone());
                    state.status = RunStatus::Completed;
                    self.call_run_complete(&mut state, context).await?;
                    checkpoint!(
                        AgentExecutionNode::RunComplete,
                        &state,
                        context_event_cursor
                    );
                    context.message_history.clone_from(&state.message_history);
                    close_run_toolsets!(&state, context_event_cursor);
                    context.publish_event(AgentEvent::new(
                        "run_complete",
                        serde_json::json!({"run_id": run_id.as_str()}),
                    ));
                    stream_event!(
                        &state,
                        AgentStreamEvent::RunComplete {
                            run_id: run_id.clone(),
                            output: output.clone(),
                        }
                    );
                    step_span.close(SpanStatus::Ok);
                    run_span.close(SpanStatus::Ok);
                    context.finish_run();
                    context.trace_context = previous_trace_context;
                    return Ok(AgentResult {
                        output,
                        structured_output: state.structured_output.clone(),
                        messages: state.message_history.clone(),
                        state,
                        history_len,
                    });
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
                            previous_trace_context.clone(),
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
                        previous_trace_context.clone(),
                        Self::capability_error(error)
                    );
                }
            }
        }
        }
        .await;
        model_session.close().await;
        run_result
    }
}
