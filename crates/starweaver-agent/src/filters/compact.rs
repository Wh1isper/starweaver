//! Cache-friendly conversation compaction filter capability.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use starweaver_context::{AgentContext, AgentEvent};
use starweaver_model::{
    ModelAdapter, ModelMessage, ModelRequestContext, ModelRequestParameters, ModelRequestPart,
    ModelSettings,
};
use starweaver_runtime::{AgentCapability, AgentRunState, CapabilityResult, CapabilitySpec};

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

pub(super) fn instruction_parts(messages: &[ModelMessage]) -> Vec<ModelRequestPart> {
    messages::instruction_parts(messages)
}

/// Cache-friendly compaction capability for automatic compaction.
#[derive(Clone)]
pub struct CacheFriendlyCompactCapability {
    model: Option<Arc<dyn ModelAdapter>>,
    model_settings: Option<ModelSettings>,
    request_params: ModelRequestParameters,
}

impl CacheFriendlyCompactCapability {
    /// Create a compaction capability using the current agent model when available.
    #[must_use]
    pub fn new(model: Option<Arc<dyn ModelAdapter>>) -> Self {
        Self {
            model,
            model_settings: None,
            request_params: ModelRequestParameters::default(),
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
        let request_context =
            ModelRequestContext::new(state.run_id.clone(), state.conversation_id.clone())
                .with_trace_context(context.trace_context.clone());
        let response = match model
            .request_stream_final(
                compact_messages,
                compact_model_settings(model.default_settings(), self.model_settings.as_ref()),
                compact_request_params(&self.request_params),
                request_context,
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                context.metadata.remove(COMPACT_DEPTH_METADATA);
                context.lifecycle.compact_depth = context.lifecycle.compact_depth.saturating_sub(1);
                context.publish_event(AgentEvent::new(
                    "compact_failed",
                    json!({"event_id": event_id, "message": error.to_string()}),
                ));
                return Ok(messages.to_vec());
            }
        };
        context.metadata.remove(COMPACT_DEPTH_METADATA);
        context.lifecycle.compact_depth = context.lifecycle.compact_depth.saturating_sub(1);
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
        Ok(compacted)
    }
}
