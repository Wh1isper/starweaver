//! Agent run loop entrypoints.

use std::collections::BTreeMap;

use starweaver_context::{AgentContext, AgentContextHandle, AgentEvent};
use starweaver_core::Usage;
use starweaver_model::{
    ModelMessage, ModelRequest, ModelRequestContext, ModelRequestPart, ModelResponseStreamEvent,
    ModelSettings,
};
use starweaver_tools::ToolContext;

const DEFAULT_MODEL_ERROR_RETRIES: usize = 2;

mod entrypoints;

use crate::{
    agent::{
        helpers::{
            has_pending_tool_control_flow, is_tool_retry_return, mark_tool_retry_return,
            record_tool_control_flow, tool_return_control_flow,
        },
        run_loop_helpers::{agent_error_kind, preserve_pending_tool_returns_for_resume},
        runtime_helpers::{request_instruction_insert_index, tool_return_media_prompt},
        Agent, AgentError, AgentResult,
    },
    capability::{CapabilityError, RetryEventKind},
    executor::{AgentExecutionDecision, AgentExecutionNode},
    retry_recovery::{recover_retry_message_history, DEFAULT_MODEL_ERROR_RESUME_PROMPT},
    run::{AgentRunState, RunStatus},
    stream::{push_stream_event, AgentStreamEvent, AgentStreamRecord},
    trace::{SpanEvent, SpanKind, SpanSpec, SpanStatus},
};

impl Agent {
    fn merge_context_model_headers(
        &self,
        context: &AgentContext,
        settings: &mut Option<ModelSettings>,
    ) {
        let should_add_headers = self.model.provider_name() == Some("codex")
            || context.provider_session_id.is_some()
            || context.provider_thread_id.is_some();
        if !should_add_headers {
            return;
        }
        let headers = context.get_model_extra_headers();
        if headers.values().all(std::string::String::is_empty) {
            return;
        }
        let settings = settings.get_or_insert_with(ModelSettings::default);
        for (key, value) in headers {
            if !value.is_empty() {
                settings.extra_headers.entry(key).or_insert(value);
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn run_with_context_inner(
        &self,
        prompt: impl Into<String>,
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

        macro_rules! fail_run {
            ($state:expr, $run_id:expr, $event_cursor:expr, $previous_trace_context:expr, $error:expr) => {{
                let error = $error;
                let error_kind = agent_error_kind(&error).to_string();
                let message = error.to_string();
                $state.status = RunStatus::Failed;
                preserve_pending_tool_returns_for_resume($state);
                context.message_history.clone_from(&$state.message_history);
                context.usage.clone_from(&$state.usage);
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
                let decision = self.checkpoint(node, $state, context).await?;
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

        let initial_prompt = prompt.into();
        context.prepare_new_run();
        let run_span = self.trace_recorder.start_span(
            SpanSpec::new("gen_ai.invoke_agent")
                .with_attribute("gen_ai.operation.name", serde_json::json!("invoke_agent")),
            &context.trace_context,
        );
        let previous_trace_context = context.trace_context.clone();
        context.trace_context = run_span.context().clone();
        let run_id = context.run_id.clone().unwrap_or_default();
        context.run_id = Some(run_id.clone());
        if let Some(model_config) = self.model_config.clone() {
            context.merge_model_config(model_config);
        }
        if let Some(tool_config) = self.tool_config.clone() {
            context.merge_tool_config(tool_config);
        }
        let conversation_id = context.conversation_id.clone();
        let history_len = context.message_history.len();
        context.user_prompts = Some(vec![starweaver_model::ContentPart::Text {
            text: initial_prompt.clone(),
        }]);
        context.previous_assistant_response_reference =
            Self::previous_assistant_response_reference(&context.message_history);
        let mut state = AgentRunState::new(run_id.clone(), conversation_id.clone());
        state.message_history = context.message_history.clone();
        state.usage = context.usage.clone();
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
        self.call_run_start(&mut state, context).await?;
        stream_context_events!(&state, context_event_cursor);
        checkpoint!(AgentExecutionNode::RunStart, &state, context_event_cursor);

        let mut next_prompt = initial_prompt;
        let mut output_retries_used = 0;
        let mut model_error_retries_used = 0usize;
        let mut tool_retries = BTreeMap::<String, usize>::new();

        'agent_loop: loop {
            let step_span = self.trace_recorder.start_span(
                SpanSpec::new("starweaver.loop.step")
                    .with_attribute("starweaver.run.step", serde_json::json!(state.run_step)),
                run_span.context(),
            );
            if state.run_step >= self.policy.max_steps {
                self.trace_recorder.close_span(
                    &step_span,
                    SpanStatus::Error {
                        error_type: "step_limit_exceeded".to_string(),
                    },
                );
                self.trace_recorder.close_span(
                    &run_span,
                    SpanStatus::Error {
                        error_type: "step_limit_exceeded".to_string(),
                    },
                );
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

            checkpoint!(
                AgentExecutionNode::PrepareModelRequest,
                &state,
                context_event_cursor
            );
            let dynamic_instruction_parts = self.dynamic_instruction_parts(&state).await?;
            let mut request = self.prepare_request(&state, &next_prompt, &run_id, &conversation_id);
            if !dynamic_instruction_parts.is_empty() {
                let insert_at = request_instruction_insert_index(&request);
                request
                    .parts
                    .splice(insert_at..insert_at, dynamic_instruction_parts);
            }
            let mut settings = self.effective_settings();
            self.merge_context_model_headers(context, &mut settings);
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
                let params = self.effective_request_params(&state, context).await?;
                messages = Self::attach_prepared_request_instructions(messages, &params);
                for message in &mut messages {
                    Self::fill_message_metadata(message, &run_id, &conversation_id);
                }
                if Self::has_pending_steering_messages(context) {
                    if let Some(ModelMessage::Request(request)) = messages
                        .iter_mut()
                        .rev()
                        .find(|message| matches!(message, ModelMessage::Request(_)))
                    {
                        Self::apply_runtime_steering_messages(context, request);
                        Self::sync_compact_context_metadata(context, &mut state);
                        stream_context_events!(&state, context_event_cursor);
                    }
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
                        "gen_ai.request.model",
                        serde_json::json!(self.model.model_name()),
                    );
                if let Some(provider_name) = self.model.provider_name() {
                    model_spec = model_spec
                        .with_attribute("gen_ai.provider.name", serde_json::json!(provider_name));
                }
                let model_span = self
                    .trace_recorder
                    .start_span(model_spec, step_span.context());
                self.record_model_request_event(&model_span, &messages, settings.as_ref(), &params);
                let request_context =
                    ModelRequestContext::new(run_id.clone(), conversation_id.clone())
                        .with_trace_context(model_span.context().clone());
                macro_rules! recover_model_error {
                    ($error:expr) => {{
                        let error = $error;
                        self.trace_recorder.close_span(
                            &model_span,
                            SpanStatus::Error {
                                error_type: "model_error".to_string(),
                            },
                        );
                        let recovery = recover_retry_message_history(&error, &state.message_history);
                        if recovery.reasons.is_empty()
                            || model_error_retries_used >= DEFAULT_MODEL_ERROR_RETRIES
                        {
                            self.trace_recorder.close_span(
                                &step_span,
                                SpanStatus::Error {
                                    error_type: "model_error".to_string(),
                                },
                            );
                            self.trace_recorder.close_span(
                                &run_span,
                                SpanStatus::Error {
                                    error_type: "model_error".to_string(),
                                },
                            );
                            fail_run!(&mut state, &run_id, context_event_cursor, previous_trace_context.clone(), AgentError::Model(error));
                        }
                        model_error_retries_used = model_error_retries_used.saturating_add(1);
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
                        self.trace_recorder.close_span(&step_span, SpanStatus::Ok);
                        continue 'agent_loop;
                    }};
                }
                if stream_events.is_some() {
                    let mut response = None;
                    let mut model_stream = match self
                        .model
                        .request_stream_incremental(messages, settings, params, request_context)
                        .await
                    {
                        Ok(stream) => stream,
                        Err(error) => recover_model_error!(error),
                    };
                    while let Some(model_event) = model_stream.recv().await {
                        let mut model_event = match model_event {
                            Ok(event) => event,
                            Err(error) => recover_model_error!(error),
                        };
                        if let ModelResponseStreamEvent::FinalResult(final_response) =
                            &mut model_event
                        {
                            for part in &mut final_response.parts {
                                context.tool_id_wrapper.wrap_response_part(part);
                            }
                        }
                        stream_event!(
                            &state,
                            AgentStreamEvent::ModelStream {
                                step: state.run_step,
                                event: model_event.clone(),
                            }
                        );
                        self.record_model_stream_event(&model_span, &model_event);
                        if let ModelResponseStreamEvent::FinalResult(final_response) = model_event {
                            response = Some(*final_response);
                        }
                    }
                    let Some(response) = response else {
                        self.trace_recorder.close_span(
                            &model_span,
                            SpanStatus::Error {
                                error_type: "missing_final_result".to_string(),
                            },
                        );
                        self.trace_recorder.close_span(
                            &step_span,
                            SpanStatus::Error {
                                error_type: "missing_final_result".to_string(),
                            },
                        );
                        self.trace_recorder.close_span(
                            &run_span,
                            SpanStatus::Error {
                                error_type: "missing_final_result".to_string(),
                            },
                        );
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
                    self.trace_recorder.close_span(&model_span, SpanStatus::Ok);
                    response
                } else {
                    let response = match self
                        .model
                        .request_stream_final(messages, settings, params, request_context)
                        .await
                    {
                        Ok(response) => response,
                        Err(error) => recover_model_error!(error),
                    };
                    self.record_model_response_event(&model_span, &response);
                    self.trace_recorder.close_span(&model_span, SpanStatus::Ok);
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
                let mut agent_usage = context
                    .usage_snapshot_entries
                    .get(&ledger_key)
                    .map_or_else(Usage::default, |entry| entry.usage.clone());
                agent_usage.add_assign(&response_usage);
                let mut snapshot = context.update_usage_snapshot_entry(
                    agent_id.clone(),
                    agent_id.clone(),
                    self.usage_model_id(&response),
                    agent_usage,
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
                self.trace_recorder.close_span(
                    &step_span,
                    SpanStatus::Error {
                        error_type: "usage_limit".to_string(),
                    },
                );
                self.trace_recorder.close_span(
                    &run_span,
                    SpanStatus::Error {
                        error_type: "usage_limit".to_string(),
                    },
                );
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

            let tool_calls = response.tool_calls();
            if !tool_calls.is_empty() {
                match self.try_call_output_function(&state, &tool_calls).await {
                    Ok(Some((output, structured_output))) => {
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
                        self.trace_recorder.close_span(&step_span, SpanStatus::Ok);
                        self.trace_recorder.close_span(&run_span, SpanStatus::Ok);
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
                    Ok(None) => {}
                    Err(CapabilityError::ModelRetry(message)) => {
                        if output_retries_used >= self.policy.output_retries {
                            self.trace_recorder.close_span(
                                &step_span,
                                SpanStatus::Error {
                                    error_type: "output_retry_limit_exceeded".to_string(),
                                },
                            );
                            self.trace_recorder.close_span(
                                &run_span,
                                SpanStatus::Error {
                                    error_type: "output_retry_limit_exceeded".to_string(),
                                },
                            );
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
                        self.trace_recorder.close_span(&step_span, SpanStatus::Ok);
                        continue;
                    }
                    Err(error) => {
                        self.trace_recorder.close_span(
                            &step_span,
                            SpanStatus::Error {
                                error_type: "capability_error".to_string(),
                            },
                        );
                        self.trace_recorder.close_span(
                            &run_span,
                            SpanStatus::Error {
                                error_type: "capability_error".to_string(),
                            },
                        );
                        fail_run!(
                            &mut state,
                            &run_id,
                            context_event_cursor,
                            previous_trace_context.clone(),
                            Self::capability_error(error)
                        );
                    }
                }
                if self.tools.is_empty() {
                    self.trace_recorder.close_span(
                        &step_span,
                        SpanStatus::Error {
                            error_type: "tool_calls_require_tools".to_string(),
                        },
                    );
                    self.trace_recorder.close_span(
                        &run_span,
                        SpanStatus::Error {
                            error_type: "tool_calls_require_tools".to_string(),
                        },
                    );
                    fail_run!(
                        &mut state,
                        &run_id,
                        context_event_cursor,
                        previous_trace_context.clone(),
                        AgentError::ToolCallsRequireTools
                    );
                }
                state.pending_tool_calls = tool_calls.clone();
                let projected_successful_tool_calls = tool_calls
                    .iter()
                    .filter(|call| self.tools.get(&call.name).is_some())
                    .count() as u64;
                if let Err(error) = self.check_tool_calls(&state, projected_successful_tool_calls) {
                    self.trace_recorder.close_span(
                        &step_span,
                        SpanStatus::Error {
                            error_type: "usage_limit".to_string(),
                        },
                    );
                    self.trace_recorder.close_span(
                        &run_span,
                        SpanStatus::Error {
                            error_type: "usage_limit".to_string(),
                        },
                    );
                    fail_run!(
                        &mut state,
                        &run_id,
                        context_event_cursor,
                        previous_trace_context.clone(),
                        error
                    );
                }
                for call in &tool_calls {
                    checkpoint!(AgentExecutionNode::ToolCall, &state, context_event_cursor);
                    stream_event!(
                        &state,
                        AgentStreamEvent::ToolCall {
                            step: state.run_step,
                            call: call.clone(),
                        }
                    );
                    let tool_retry = *tool_retries.get(&call.name).unwrap_or(&0);
                    let tool_max_retries = self.tools.max_retries_for(&call.name);
                    let tool_span = self.trace_recorder.start_span(
                        SpanSpec::new("gen_ai.execute_tool")
                            .with_attribute(
                                "gen_ai.tool.name",
                                serde_json::json!(call.name.clone()),
                            )
                            .with_attribute(
                                "gen_ai.tool.call.id",
                                serde_json::json!(call.id.clone()),
                            ),
                        step_span.context(),
                    );
                    let context_handle = AgentContextHandle::new(context.clone());
                    let mut tool_dependencies = context.dependencies.clone();
                    tool_dependencies.insert(context.clone());
                    tool_dependencies.insert(context_handle.clone());
                    let mut tool_context = ToolContext::new(
                        state.run_id.clone(),
                        state.conversation_id.clone(),
                        state.run_step,
                    )
                    .with_dependencies(tool_dependencies)
                    .with_trace_context(tool_span.context().clone())
                    .with_retry_budget(tool_retry, tool_max_retries);
                    self.call_before_tool_execution(&mut state, context, &mut tool_context, call)
                        .await?;
                    context_handle.replace(context.clone());
                    tool_context.dependencies.insert(context.clone());
                    self.trace_recorder.record_event(
                        &tool_span,
                        SpanEvent::new("starweaver.tool.call").with_attribute(
                            "gen_ai.tool.call.arguments",
                            call.arguments.replay_value(),
                        ),
                    );
                    let tool_started_at = std::time::Instant::now();
                    let mut tool_return = self.tools.execute_call(tool_context, call).await;
                    let tool_duration = tool_started_at.elapsed();
                    let duration_ms = u64::try_from(tool_duration.as_millis()).unwrap_or(u64::MAX);
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
                            .with_attribute("gen_ai.tool.call.result", tool_return.content.clone())
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
                        self.trace_recorder.close_span(
                            &tool_span,
                            SpanStatus::Error {
                                error_type: tool_return
                                    .metadata
                                    .get("error_kind")
                                    .and_then(serde_json::Value::as_str)
                                    .unwrap_or("tool_error")
                                    .to_string(),
                            },
                        );
                    } else {
                        self.trace_recorder.close_span(&tool_span, SpanStatus::Ok);
                    }
                    self.absorb_tool_context_handle(&mut state, context, &context_handle)?;
                    self.call_after_tool_result(&mut state, context, call, &mut tool_return)
                        .await?;
                    tool_return
                        .metadata
                        .insert("duration_ms".to_string(), serde_json::json!(duration_ms));
                    tool_return.metadata.insert(
                        "duration_seconds".to_string(),
                        serde_json::json!(tool_duration.as_secs_f64()),
                    );
                    if tool_return.is_error && is_tool_retry_return(&tool_return) {
                        if tool_retry >= tool_max_retries {
                            self.trace_recorder.close_span(
                                &tool_span,
                                SpanStatus::Error {
                                    error_type: "tool_retry_limit_exceeded".to_string(),
                                },
                            );
                            self.trace_recorder.close_span(
                                &step_span,
                                SpanStatus::Error {
                                    error_type: "tool_retry_limit_exceeded".to_string(),
                                },
                            );
                            self.trace_recorder.close_span(
                                &run_span,
                                SpanStatus::Error {
                                    error_type: "tool_retry_limit_exceeded".to_string(),
                                },
                            );
                            fail_run!(
                                &mut state,
                                &run_id,
                                context_event_cursor,
                                previous_trace_context.clone(),
                                AgentError::ToolRetryLimitExceeded {
                                    tool: call.name.clone(),
                                    max_retries: tool_max_retries,
                                }
                            );
                        }
                        let next_retry = tool_retry.saturating_add(1);
                        tool_retries.insert(call.name.clone(), next_retry);
                        self.call_retry(
                            &mut state,
                            context,
                            RetryEventKind::Tool,
                            next_retry,
                            &call.name,
                        )
                        .await?;
                        mark_tool_retry_return(&mut tool_return, next_retry, tool_max_retries);
                    }
                    stream_event!(
                        &state,
                        AgentStreamEvent::ToolReturn {
                            step: state.run_step,
                            tool_return: tool_return.clone(),
                        }
                    );
                    record_tool_control_flow(&mut state, &tool_return);
                    if !tool_return.is_error {
                        state.usage.tool_calls = state.usage.tool_calls.saturating_add(1);
                        context.usage.tool_calls = context.usage.tool_calls.saturating_add(1);
                    }
                    state.pending_tool_returns.push(tool_return);
                    stream_context_events!(&state, context_event_cursor);
                    checkpoint!(AgentExecutionNode::ToolReturn, &state, context_event_cursor);
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
                    self.trace_recorder.close_span(&step_span, SpanStatus::Ok);
                    self.trace_recorder.close_span(&run_span, SpanStatus::Ok);
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
                next_prompt.clear();
                self.trace_recorder.close_span(&step_span, SpanStatus::Ok);
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
                    self.trace_recorder.close_span(&step_span, SpanStatus::Ok);
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
                    self.trace_recorder.close_span(&step_span, SpanStatus::Ok);
                    self.trace_recorder.close_span(&run_span, SpanStatus::Ok);
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
                        self.trace_recorder.close_span(
                            &step_span,
                            SpanStatus::Error {
                                error_type: "output_retry_limit_exceeded".to_string(),
                            },
                        );
                        self.trace_recorder.close_span(
                            &run_span,
                            SpanStatus::Error {
                                error_type: "output_retry_limit_exceeded".to_string(),
                            },
                        );
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
                    self.trace_recorder.close_span(&step_span, SpanStatus::Ok);
                }
                Err(error) => {
                    self.trace_recorder.close_span(
                        &step_span,
                        SpanStatus::Error {
                            error_type: "capability_error".to_string(),
                        },
                    );
                    self.trace_recorder.close_span(
                        &run_span,
                        SpanStatus::Error {
                            error_type: "capability_error".to_string(),
                        },
                    );
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
}
