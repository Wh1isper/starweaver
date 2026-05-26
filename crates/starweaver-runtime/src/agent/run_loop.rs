//! Agent run loop entrypoints.

use std::collections::BTreeMap;

use starweaver_context::{AgentContext, AgentEvent};
use starweaver_core::RunId;
use starweaver_model::{ModelMessage, ModelRequestContext};
use starweaver_tools::ToolContext;

use crate::{
    agent::{
        helpers::{is_tool_retry_return, mark_tool_retry_return, record_tool_control_flow},
        Agent, AgentError, AgentResult,
    },
    capability::CapabilityError,
    executor::AgentExecutionNode,
    run::{AgentRunState, RunStatus},
    stream::{push_stream_event, AgentStreamEvent, AgentStreamRecord, AgentStreamResult},
};

impl Agent {
    /// Run the agent with a user prompt.
    ///
    /// # Errors
    ///
    /// Returns an error when the model, capabilities, validation, tools, or runtime policy fails.
    pub async fn run(&self, prompt: impl Into<String>) -> Result<AgentResult, AgentError> {
        self.run_with_history(prompt, Vec::new()).await
    }

    /// Run the agent and collect typed stream events emitted during execution.
    ///
    /// # Errors
    ///
    /// Returns an error when the model, capabilities, validation, tools, or runtime policy fails.
    pub async fn run_stream(
        &self,
        prompt: impl Into<String>,
    ) -> Result<AgentStreamResult, AgentError> {
        let mut events = Vec::new();
        let result = self.run_with_stream_events(prompt, &mut events).await?;
        Ok(AgentStreamResult { result, events })
    }

    /// Run the agent with an explicit typed stream event collector.
    ///
    /// # Errors
    ///
    /// Returns an error when the model, capabilities, validation, tools, or runtime policy fails.
    pub async fn run_with_stream_events(
        &self,
        prompt: impl Into<String>,
        events: &mut Vec<AgentStreamRecord>,
    ) -> Result<AgentResult, AgentError> {
        self.run_with_history_and_stream_events(prompt, Vec::new(), events)
            .await
    }

    /// Run the agent with prior history and collect typed stream events.
    ///
    /// # Errors
    ///
    /// Returns an error when the model, capabilities, validation, tools, or runtime policy fails.
    pub async fn run_with_history_and_stream_events(
        &self,
        prompt: impl Into<String>,
        message_history: Vec<ModelMessage>,
        events: &mut Vec<AgentStreamRecord>,
    ) -> Result<AgentResult, AgentError> {
        let mut context = AgentContext {
            message_history,
            ..AgentContext::default()
        };
        self.run_with_context_and_stream_events(prompt, &mut context, events)
            .await
    }

    /// Run the agent with prior canonical message history.
    ///
    /// # Errors
    ///
    /// Returns an error when the model, capabilities, validation, tools, or runtime policy fails.
    pub async fn run_with_history(
        &self,
        prompt: impl Into<String>,
        message_history: Vec<ModelMessage>,
    ) -> Result<AgentResult, AgentError> {
        let mut context = AgentContext {
            message_history,
            ..AgentContext::default()
        };
        self.run_with_context(prompt, &mut context).await
    }

    /// Run the agent using a lifecycle-wide context and typed stream event collector.
    ///
    /// # Errors
    ///
    /// Returns an error when the model, capabilities, validation, tools, or runtime policy fails.
    pub async fn run_with_context_and_stream_events(
        &self,
        prompt: impl Into<String>,
        context: &mut AgentContext,
        events: &mut Vec<AgentStreamRecord>,
    ) -> Result<AgentResult, AgentError> {
        self.run_with_context_inner(prompt, context, Some(events))
            .await
    }

    /// Run the agent using a lifecycle-wide context.
    ///
    /// # Errors
    ///
    /// Returns an error when the model, capabilities, validation, tools, or runtime policy fails.
    #[allow(clippy::too_many_lines)]
    pub async fn run_with_context(
        &self,
        prompt: impl Into<String>,
        context: &mut AgentContext,
    ) -> Result<AgentResult, AgentError> {
        self.run_with_context_inner(prompt, context, None).await
    }

    #[allow(clippy::too_many_lines)]
    async fn run_with_context_inner(
        &self,
        prompt: impl Into<String>,
        context: &mut AgentContext,
        mut stream_events: Option<&mut Vec<AgentStreamRecord>>,
    ) -> Result<AgentResult, AgentError> {
        let run_id = RunId::new();
        context.run_id = Some(run_id.clone());
        let conversation_id = context.conversation_id.clone();
        let history_len = context.message_history.len();
        let mut state = AgentRunState::new(run_id.clone(), conversation_id.clone());
        state.message_history = context.message_history.clone();
        state.usage = context.usage.clone();
        state.status = RunStatus::Running;
        context.publish_event(AgentEvent::new(
            "run_start",
            serde_json::json!({"run_id": run_id.as_str()}),
        ));
        push_stream_event(
            &mut stream_events,
            AgentStreamEvent::RunStart {
                run_id: run_id.clone(),
                conversation_id: conversation_id.clone(),
            },
        );
        self.call_run_start(&mut state, context).await?;
        self.checkpoint(AgentExecutionNode::RunStart, &state)
            .await?;

        let mut next_prompt = prompt.into();
        let mut output_retries_used = 0;
        let mut tool_retries = BTreeMap::<String, usize>::new();

        loop {
            if state.run_step >= self.policy.max_steps {
                state.status = RunStatus::Failed;
                return Err(AgentError::StepLimitExceeded {
                    steps: state.run_step,
                });
            }

            self.checkpoint(AgentExecutionNode::PrepareModelRequest, &state)
                .await?;
            let mut request = self
                .prepare_request(&state, &next_prompt, &run_id, &conversation_id)
                .await?;
            let mut settings = self.effective_settings();
            let skipped_response = self
                .call_before_model_request(&mut state, context, &mut request, &mut settings)
                .await?;
            state.message_history.push(ModelMessage::Request(request));
            context.message_history.clone_from(&state.message_history);
            push_stream_event(
                &mut stream_events,
                AgentStreamEvent::ModelRequest {
                    step: state.run_step,
                },
            );
            state.pending_tool_returns.clear();
            self.checkpoint(AgentExecutionNode::BeforeModelRequest, &state)
                .await?;

            let response = if let Some(response) = skipped_response {
                response
            } else {
                self.check_before_request(&state)?;
                let messages = self.process_history(&state).await?;
                let params = self.effective_request_params(&state, context).await?;
                self.model
                    .request(
                        messages,
                        settings,
                        params,
                        ModelRequestContext {
                            run_id: run_id.clone(),
                            conversation_id: conversation_id.clone(),
                        },
                    )
                    .await?
            };
            state.run_step += 1;
            let response_usage = response.usage.clone();
            push_stream_event(
                &mut stream_events,
                AgentStreamEvent::ModelResponse {
                    step: state.run_step,
                    response: response.clone(),
                },
            );
            state.apply_model_response(response);
            context.add_usage(&response_usage);
            self.checkpoint(AgentExecutionNode::ModelResponse, &state)
                .await?;
            self.check_usage(&state)?;
            context.message_history.clone_from(&state.message_history);

            let mut response = state
                .latest_response
                .clone()
                .ok_or_else(|| AgentError::Capability("missing latest response".to_string()))?;
            self.call_after_model_response(&mut state, context, &mut response)
                .await?;
            state.latest_response = Some(response.clone());

            let tool_calls = response.tool_calls();
            if !tool_calls.is_empty() {
                match self.try_call_output_function(&state, &tool_calls).await {
                    Ok(Some((output, structured_output))) => {
                        state.output = Some(output.clone());
                        state.structured_output = structured_output;
                        state.status = RunStatus::Completed;
                        self.call_run_complete(&mut state, context).await?;
                        self.checkpoint(AgentExecutionNode::RunComplete, &state)
                            .await?;
                        context.message_history.clone_from(&state.message_history);
                        context.publish_event(AgentEvent::new(
                            "run_complete",
                            serde_json::json!({"run_id": run_id.as_str()}),
                        ));
                        push_stream_event(
                            &mut stream_events,
                            AgentStreamEvent::RunComplete {
                                run_id: run_id.clone(),
                                output: output.clone(),
                            },
                        );
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
                            state.status = RunStatus::Failed;
                            return Err(AgentError::OutputRetryLimitExceeded {
                                retries: output_retries_used,
                            });
                        }
                        output_retries_used += 1;
                        push_stream_event(
                            &mut stream_events,
                            AgentStreamEvent::OutputRetry {
                                retries: output_retries_used,
                                prompt: message.clone(),
                            },
                        );
                        next_prompt = message;
                        continue;
                    }
                    Err(error) => return Err(Self::capability_error(error)),
                }
                if self.tools.is_empty() {
                    state.status = RunStatus::Failed;
                    return Err(AgentError::ToolCallsRequireTools);
                }
                state.pending_tool_calls = tool_calls.clone();
                let projected_successful_tool_calls = tool_calls
                    .iter()
                    .filter(|call| self.tools.get(&call.name).is_some())
                    .count() as u64;
                self.check_tool_calls(&state, projected_successful_tool_calls)?;
                for call in &tool_calls {
                    self.checkpoint(AgentExecutionNode::ToolCall, &state)
                        .await?;
                    push_stream_event(
                        &mut stream_events,
                        AgentStreamEvent::ToolCall {
                            step: state.run_step,
                            call: call.clone(),
                        },
                    );
                    let tool_retry = *tool_retries.get(&call.name).unwrap_or(&0);
                    let tool_max_retries = self.tools.max_retries_for(&call.name);
                    let tool_context = ToolContext::new(
                        state.run_id.clone(),
                        state.conversation_id.clone(),
                        state.run_step,
                    )
                    .with_dependencies(context.dependencies.clone())
                    .with_retry_budget(tool_retry, tool_max_retries);
                    let mut tool_return = self.tools.execute_call(tool_context, call).await;
                    if tool_return.is_error && is_tool_retry_return(&tool_return) {
                        if tool_retry >= tool_max_retries {
                            state.status = RunStatus::Failed;
                            return Err(AgentError::ToolRetryLimitExceeded {
                                tool: call.name.clone(),
                                max_retries: tool_max_retries,
                            });
                        }
                        let next_retry = tool_retry.saturating_add(1);
                        tool_retries.insert(call.name.clone(), next_retry);
                        mark_tool_retry_return(&mut tool_return, next_retry, tool_max_retries);
                    }
                    push_stream_event(
                        &mut stream_events,
                        AgentStreamEvent::ToolReturn {
                            step: state.run_step,
                            tool_return: tool_return.clone(),
                        },
                    );
                    record_tool_control_flow(&mut state, &tool_return);
                    if !tool_return.is_error {
                        state.usage.tool_calls = state.usage.tool_calls.saturating_add(1);
                        context.usage.tool_calls = context.usage.tool_calls.saturating_add(1);
                    }
                    state.pending_tool_returns.push(tool_return);
                    self.checkpoint(AgentExecutionNode::ToolReturn, &state)
                        .await?;
                }
                next_prompt.clear();
                continue;
            }

            let output = response.text_output();
            self.checkpoint(AgentExecutionNode::ValidateOutput, &state)
                .await?;
            match self
                .validate_final_output(&mut state, context, &output)
                .await
            {
                Ok(()) => {
                    state.output = Some(output.clone());
                    state.status = RunStatus::Completed;
                    self.call_run_complete(&mut state, context).await?;
                    self.checkpoint(AgentExecutionNode::RunComplete, &state)
                        .await?;
                    context.message_history.clone_from(&state.message_history);
                    context.publish_event(AgentEvent::new(
                        "run_complete",
                        serde_json::json!({"run_id": run_id.as_str()}),
                    ));
                    push_stream_event(
                        &mut stream_events,
                        AgentStreamEvent::RunComplete {
                            run_id: run_id.clone(),
                            output: output.clone(),
                        },
                    );
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
                        state.status = RunStatus::Failed;
                        return Err(AgentError::OutputRetryLimitExceeded {
                            retries: output_retries_used,
                        });
                    }
                    output_retries_used += 1;
                    push_stream_event(
                        &mut stream_events,
                        AgentStreamEvent::OutputRetry {
                            retries: output_retries_used,
                            prompt: message.clone(),
                        },
                    );
                    next_prompt = message;
                }
                Err(error) => return Err(Self::capability_error(error)),
            }
        }
    }
}
