//! Direct model and tool execution helpers.

use serde::{Deserialize, Serialize};
use starweaver_core::{ConversationId, RunId, TraceContext};
use starweaver_model::{
    ModelAdapter, ModelError, ModelMessage, ModelRequestContext, ModelRequestParameters,
    ModelResponse, ModelResponseStreamEvent, ModelSettings, ToolCallPart, ToolReturnPart,
};
use starweaver_tools::{ToolContext, ToolRegistry};

/// Options for a direct model request.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct DirectModelRequest {
    /// Canonical request history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<ModelMessage>,
    /// Per-call model settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<ModelSettings>,
    /// Provider-neutral request parameters.
    #[serde(default)]
    pub params: ModelRequestParameters,
    /// Run id for tracing and provider metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Conversation id for tracing and provider metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    /// Trace correlation context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
}

impl DirectModelRequest {
    /// Build a direct request from canonical messages.
    #[must_use]
    pub fn new(messages: Vec<ModelMessage>) -> Self {
        Self {
            messages,
            ..Self::default()
        }
    }

    /// Attach model settings.
    #[must_use]
    pub fn with_settings(mut self, settings: ModelSettings) -> Self {
        self.settings = Some(settings);
        self
    }

    /// Attach request parameters.
    #[must_use]
    pub fn with_params(mut self, params: ModelRequestParameters) -> Self {
        self.params = params;
        self
    }

    /// Attach run and conversation identifiers.
    #[must_use]
    pub fn with_ids(mut self, run_id: RunId, conversation_id: ConversationId) -> Self {
        self.run_id = Some(run_id);
        self.conversation_id = Some(conversation_id);
        self
    }

    /// Attach trace correlation context.
    #[must_use]
    pub fn with_trace_context(mut self, trace_context: TraceContext) -> Self {
        self.trace_context = trace_context;
        self
    }

    fn context(&self) -> ModelRequestContext {
        ModelRequestContext::new(
            self.run_id.clone().unwrap_or_default(),
            self.conversation_id.clone().unwrap_or_default(),
        )
        .with_trace_context(self.trace_context.clone())
    }
}

/// Execute one model request directly through a model adapter.
///
/// # Errors
///
/// Returns an error when the model adapter fails.
pub async fn model_request(
    model: &dyn ModelAdapter,
    request: DirectModelRequest,
) -> Result<ModelResponse, ModelError> {
    let context = request.context();
    model
        .request(request.messages, request.settings, request.params, context)
        .await
}

/// Execute one model request directly and collect canonical stream events.
///
/// # Errors
///
/// Returns an error when the model adapter fails.
pub async fn model_request_stream(
    model: &dyn ModelAdapter,
    request: DirectModelRequest,
) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
    let context = request.context();
    model
        .request_stream(request.messages, request.settings, request.params, context)
        .await
}

/// Execute one tool call directly through a tool registry.
pub async fn tool_call(
    tools: &ToolRegistry,
    context: ToolContext,
    call: &ToolCallPart,
) -> ToolReturnPart {
    tools.execute_call(context, call).await
}
