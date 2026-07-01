//! Cache-friendly conversation compaction filter capability.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use starweaver_context::{AgentContext, AgentEvent};
use starweaver_model::{
    ModelAdapter, ModelError, ModelMessage, ModelRequestContext, ModelRequestParameters,
    ModelRequestPart, ModelResponse, ModelSettings,
};
use starweaver_runtime::{
    AgentCapability, AgentRunState, CapabilityResult, CapabilitySpec, DynTraceRecorder,
    NoopTraceRecorder, SpanEvent, SpanHandle, SpanKind, SpanSpec, SpanStatus,
};

use super::message::record_filter_order;
use super::{filter_capability_id, filter_capability_ordering};

mod constants;
mod messages;
mod request;
mod settings;
mod threshold;

use constants::{COMPACT_DEPTH_METADATA, DEFAULT_AUTO_COMPACT_KEEP_MESSAGES};
use messages::{
    build_cache_friendly_compacted_messages, build_trimmed_compact_messages, manual_compact_keep,
};
use request::build_compact_summary_request;
use settings::{compact_model_settings, compact_request_params};
use threshold::need_auto_compact;

const COMPACT_TRACE_RECORDED_METADATA: &str = "starweaver.history.compaction.trace_recorded";
const COMPACT_AGENT_ID: &str = "starweaver.compact";
const COMPACT_AGENT_NAME: &str = "Compact-Agent";

pub(super) fn instruction_parts(messages: &[ModelMessage]) -> Vec<ModelRequestPart> {
    messages::instruction_parts(messages)
}

/// Cache-friendly compaction capability for automatic compaction.
#[derive(Clone)]
pub struct CacheFriendlyCompactCapability {
    model: Option<Arc<dyn ModelAdapter>>,
    model_settings: Option<ModelSettings>,
    request_params: ModelRequestParameters,
    trace_recorder: DynTraceRecorder,
}

impl CacheFriendlyCompactCapability {
    /// Create a compaction capability using the current agent model when available.
    #[must_use]
    pub fn new(model: Option<Arc<dyn ModelAdapter>>) -> Self {
        Self {
            model,
            model_settings: None,
            request_params: ModelRequestParameters::default(),
            trace_recorder: Arc::new(NoopTraceRecorder),
        }
    }

    /// Inherit model settings from the parent agent.
    #[must_use]
    pub fn with_model_settings(mut self, settings: ModelSettings) -> Self {
        self.model_settings = Some(settings);
        self
    }

    /// Inherit request parameters from the parent agent.
    #[must_use]
    pub fn with_request_params(mut self, params: ModelRequestParameters) -> Self {
        self.request_params = params;
        self
    }

    /// Attach the runtime trace recorder used for compact model spans.
    #[must_use]
    pub fn with_trace_recorder(mut self, recorder: DynTraceRecorder) -> Self {
        self.trace_recorder = recorder;
        self
    }
}

#[async_trait]
impl AgentCapability for CacheFriendlyCompactCapability {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new(filter_capability_id("compact"))
            .with_ordering(filter_capability_ordering("compact"))
    }

    async fn prepare_model_messages_with_context(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        let mut compacted = if let Some(keep) = manual_compact_keep(state) {
            build_trimmed_compact_messages(state, context, &messages, keep)
        } else if need_auto_compact(context, &messages) {
            self.compact_with_model(state, context, &messages).await?
        } else {
            messages
        };
        record_filter_order(&mut compacted, "compact");
        let changed = compacted != state.message_history;
        if changed {
            state.message_history.clone_from(&compacted);
            context.message_history.clone_from(&compacted);
        }
        Ok(compacted)
    }
}

impl CacheFriendlyCompactCapability {
    async fn compact_with_model(
        &self,
        state: &AgentRunState,
        context: &mut AgentContext,
        messages: &[ModelMessage],
    ) -> CapabilityResult<Vec<ModelMessage>> {
        if context
            .metadata
            .get(COMPACT_DEPTH_METADATA)
            .and_then(Value::as_u64)
            .unwrap_or_default()
            > 0
        {
            return Ok(messages.to_vec());
        }
        let Some(model) = &self.model else {
            return Ok(build_trimmed_compact_messages(
                state,
                context,
                messages,
                DEFAULT_AUTO_COMPACT_KEEP_MESSAGES,
            ));
        };
        let previous_trace_context = context.trace_context.clone();
        let compact_span = self.trace_recorder.start_span(
            SpanSpec::new("starweaver.history.compaction")
                .with_attribute("starweaver.capability.name", json!(self.spec().id.as_str()))
                .with_attribute("starweaver.history.messages.before", json!(messages.len()))
                .with_attribute("starweaver.run.id", json!(state.run_id.as_str()))
                .with_attribute("starweaver.run.step", json!(state.run_step)),
            &previous_trace_context,
        );
        context.trace_context = compact_span.context().clone();
        context
            .metadata
            .insert(COMPACT_DEPTH_METADATA.to_string(), json!(1));
        context.lifecycle.compact_depth = context.lifecycle.compact_depth.saturating_add(1);
        let event_id = format!("{}-{}", state.run_id.as_str(), state.run_step);
        context.publish_event(AgentEvent::new(
            "compact_start",
            json!({"event_id": event_id, "message_count": messages.len()}),
        ));
        let compact_messages =
            build_compact_summary_request(messages, &context.injected_context_tags);
        let response = match self
            .request_compact_summary(
                model.as_ref(),
                state,
                context,
                compact_messages,
                &event_id,
                &compact_span,
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                self.trace_recorder.close_span(
                    &compact_span,
                    SpanStatus::Error {
                        error_type: "model_error".to_string(),
                    },
                );
                context.metadata.remove(COMPACT_DEPTH_METADATA);
                context.lifecycle.compact_depth = context.lifecycle.compact_depth.saturating_sub(1);
                context.trace_context = previous_trace_context;
                context.publish_event(AgentEvent::new(
                    "compact_failed",
                    json!({"event_id": event_id, "message": error.to_string()}),
                ));
                return Ok(messages.to_vec());
            }
        };
        context.metadata.remove(COMPACT_DEPTH_METADATA);
        context.lifecycle.compact_depth = context.lifecycle.compact_depth.saturating_sub(1);
        context.trace_context = previous_trace_context;
        context.add_usage(&response.usage);
        let summary = response.text_output();
        let compacted = build_cache_friendly_compacted_messages(state, context, messages, &summary);
        context.force_inject_context = true;
        context.publish_event(AgentEvent::new(
            "compact_complete",
            json!({
                "event_id": event_id,
                "message_count_before": messages.len(),
                "message_count_after": compacted.len(),
            }),
        ));
        self.trace_recorder.record_event(
            &compact_span,
            SpanEvent::new("starweaver.history.compaction.result")
                .with_attribute("starweaver.history.messages.after", json!(compacted.len())),
        );
        self.trace_recorder
            .close_span(&compact_span, SpanStatus::Ok);
        context
            .metadata
            .insert(COMPACT_TRACE_RECORDED_METADATA.to_string(), json!(true));
        Ok(compacted)
    }

    async fn request_compact_summary(
        &self,
        model: &dyn ModelAdapter,
        state: &AgentRunState,
        context: &AgentContext,
        compact_messages: Vec<ModelMessage>,
        event_id: &str,
        compact_span: &SpanHandle,
    ) -> Result<ModelResponse, ModelError> {
        let compact_agent_span = self.trace_recorder.start_span(
            compact_agent_span_spec(state, event_id),
            compact_span.context(),
        );
        let model_span = self.trace_recorder.start_span(
            compact_model_span_spec(model, state, event_id),
            compact_agent_span.context(),
        );
        let model_settings =
            compact_model_settings(model.default_settings(), self.model_settings.as_ref());
        let request_params = compact_request_params(&self.request_params);
        self.trace_recorder.record_event(
            &model_span,
            compact_model_request_event(
                &compact_messages,
                model_settings.as_ref(),
                &request_params,
            ),
        );
        let request_context =
            ModelRequestContext::new(state.run_id.clone(), state.conversation_id.clone())
                .with_trace_context(model_span.context().clone())
                .with_llm_trace_metadata(context.metadata.clone());
        match model
            .request_stream_final(
                compact_messages,
                model_settings,
                request_params,
                request_context,
            )
            .await
        {
            Ok(response) => {
                self.trace_recorder
                    .record_event(&model_span, compact_model_response_event(&response));
                self.trace_recorder.close_span(&model_span, SpanStatus::Ok);
                self.trace_recorder
                    .close_span(&compact_agent_span, SpanStatus::Ok);
                Ok(response)
            }
            Err(error) => {
                self.close_compact_model_error_spans(&model_span, &compact_agent_span);
                Err(error)
            }
        }
    }

    fn close_compact_model_error_spans(
        &self,
        model_span: &SpanHandle,
        compact_agent_span: &SpanHandle,
    ) {
        let error_status = || SpanStatus::Error {
            error_type: "model_error".to_string(),
        };
        self.trace_recorder.close_span(model_span, error_status());
        self.trace_recorder
            .close_span(compact_agent_span, error_status());
    }
}

fn compact_agent_span_spec(state: &AgentRunState, event_id: &str) -> SpanSpec {
    SpanSpec::new("gen_ai.invoke_agent")
        .with_attribute("gen_ai.operation.name", json!("invoke_agent"))
        .with_attribute("gen_ai.agent.id", json!(COMPACT_AGENT_ID))
        .with_attribute("gen_ai.agent.name", json!(COMPACT_AGENT_NAME))
        .with_attribute(
            "gen_ai.conversation.id",
            json!(state.conversation_id.as_str()),
        )
        .with_attribute("starweaver.run.id", json!(state.run_id.as_str()))
        .with_attribute("starweaver.compact.event_id", json!(event_id))
}

fn compact_model_span_spec(
    model: &dyn ModelAdapter,
    state: &AgentRunState,
    event_id: &str,
) -> SpanSpec {
    let spec = SpanSpec::new("gen_ai.inference")
        .with_kind(SpanKind::Client)
        .with_attribute("gen_ai.operation.name", json!("chat"))
        .with_attribute("gen_ai.agent.id", json!(COMPACT_AGENT_ID))
        .with_attribute(
            "gen_ai.conversation.id",
            json!(state.conversation_id.as_str()),
        )
        .with_attribute("starweaver.run.id", json!(state.run_id.as_str()))
        .with_attribute("starweaver.compact.event_id", json!(event_id))
        .with_attribute("gen_ai.request.model", json!(model.model_name()));
    match model.provider_name() {
        Some(provider_name) => spec.with_attribute("gen_ai.provider.name", json!(provider_name)),
        None => spec,
    }
}

fn compact_model_request_event(
    messages: &[ModelMessage],
    settings: Option<&ModelSettings>,
    params: &ModelRequestParameters,
) -> SpanEvent {
    SpanEvent::new("starweaver.model.request")
        .with_attribute("starweaver.model.message_count", json!(messages.len()))
        .with_attribute("starweaver.model.tool_count", json!(params.tools.len()))
        .with_attribute(
            "starweaver.model.native_tool_count",
            json!(params.native_tools.len()),
        )
        .with_attribute(
            "starweaver.model.has_output_schema",
            json!(params.output_schema.is_some()),
        )
        .with_attribute(
            "gen_ai.request",
            json!({
                "redacted": true,
                "message_count": messages.len(),
                "tool_names": params.tools.iter().map(|tool| tool.name.clone()).collect::<Vec<_>>(),
                "native_tool_count": params.native_tools.len(),
                "output_schema_name": params.output_schema.as_ref().and_then(|schema| {
                    schema.get("name").and_then(serde_json::Value::as_str).map(str::to_string)
                }),
                "settings": settings.map(|settings| json!({
                    "temperature": settings.temperature,
                    "max_tokens": settings.max_tokens,
                    "has_thinking": settings.thinking.is_some(),
                })),
            }),
        )
}

fn compact_model_response_event(response: &ModelResponse) -> SpanEvent {
    let tool_calls = response
        .tool_calls()
        .into_iter()
        .map(|call| call.name)
        .collect::<Vec<_>>();
    SpanEvent::new("starweaver.model.response")
        .with_attribute(
            "gen_ai.response",
            json!({
                "redacted": true,
                "part_count": response.parts.len(),
                "text_chars": response.text_output().chars().count(),
                "tool_call_names": tool_calls,
                "finish_reason": response.finish_reason.clone(),
                "model_name": response.model_name.clone(),
            }),
        )
        .with_attribute(
            "gen_ai.usage.input_tokens",
            json!(response.usage.input_tokens),
        )
        .with_attribute(
            "gen_ai.usage.output_tokens",
            json!(response.usage.output_tokens),
        )
        .with_attribute(
            "gen_ai.usage.cache_read.input_tokens",
            json!(response.usage.cache_read_tokens),
        )
        .with_attribute(
            "gen_ai.usage.cache_creation.input_tokens",
            json!(response.usage.cache_write_tokens),
        )
}
