use std::sync::Arc;

use async_trait::async_trait;
use starweaver_context::AgentContext;
use starweaver_model::ModelMessage;
use starweaver_runtime::{
    AgentCapability, AgentRunState, CapabilityError, CapabilityResult, CapabilitySpec,
};

use crate::filters::{
    media::{
        capability_filter, media_compress_filter, media_preflight_filter, media_upload_filter,
        MediaUploader,
    },
    message::record_filter_order,
};

use super::{
    context_injection::{
        auto_load_files_filter, background_shell_filter, bus_message_filter, cold_start_filter,
        handoff_filter, inject_instruction_from_metadata, system_prompt_filter,
        ENVIRONMENT_CONTEXT_METADATA, RUNTIME_CONTEXT_METADATA,
    },
    ordering::{filter_capability_id, filter_capability_ordering},
    reasoning::reasoning_normalize_filter,
    tool_args::tool_args_filter,
};

/// Named SDK filter capability with concrete behavior.
#[derive(Clone)]
pub struct NamedFilterCapability {
    name: &'static str,
    uploader: Option<Arc<dyn MediaUploader>>,
}

impl std::fmt::Debug for NamedFilterCapability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NamedFilterCapability")
            .field("name", &self.name)
            .field("has_uploader", &self.uploader.is_some())
            .finish()
    }
}

impl NamedFilterCapability {
    /// Create a named filter capability.
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            uploader: None,
        }
    }

    /// Create a media upload processor with an adapter.
    #[must_use]
    pub fn media_upload(uploader: Arc<dyn MediaUploader>) -> Self {
        Self {
            name: "media_upload",
            uploader: Some(uploader),
        }
    }

    /// Return processor name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }
}

#[async_trait]
impl AgentCapability for NamedFilterCapability {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new(filter_capability_id(self.name))
            .with_ordering(filter_capability_ordering(self.name))
    }

    async fn prepare_model_messages_with_context(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        let mut messages = match self.name {
            "cold_start" => cold_start_filter(state, messages),
            "capability" => capability_filter(state, context, messages),
            "media_preflight" => media_preflight_filter(state, context, messages),
            "media_compress" => media_compress_filter(state, context, messages),
            "media_upload" => {
                media_upload_filter(state, context, messages, self.uploader.as_ref()).await
            }
            "handoff" => handoff_filter(state, context, messages),
            "auto_load_files" => auto_load_files_filter(state, context, messages).await,
            "background_shell" => background_shell_filter(state, messages),
            "bus_message" => bus_message_filter(state, context, messages),
            "environment_context" => inject_instruction_from_metadata(
                state,
                messages,
                ENVIRONMENT_CONTEXT_METADATA,
                "environment",
            ),
            "runtime_context" => inject_instruction_from_metadata(
                state,
                messages,
                RUNTIME_CONTEXT_METADATA,
                "runtime",
            ),
            "system_prompt" => system_prompt_filter(state, messages),
            "tool_args" => tool_args_filter(messages),
            "reasoning_normalize" => reasoning_normalize_filter(messages),
            other => {
                return Err(CapabilityError::Failed(format!(
                    "unknown SDK filter '{other}'"
                )));
            }
        };
        record_filter_order(&mut messages, self.name);
        Ok(messages)
    }

    async fn prepare_provider_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut AgentContext,
        messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        Ok(messages)
    }
}
