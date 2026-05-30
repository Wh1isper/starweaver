//! Model adapter traits and request context types.

use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use starweaver_core::{ConversationId, RunId, TraceContext, Usage};
use thiserror::Error;

use crate::{
    message::ModelMessage, profile::ModelProfile, settings::ModelSettings,
    stream::ModelResponseStreamEvent, transport::HttpRequestOptions, ModelResponse,
};

static ALLOW_REAL_MODEL_REQUESTS: AtomicBool = AtomicBool::new(true);

/// Return whether production model requests are globally allowed.
#[must_use]
pub fn allow_real_model_requests() -> bool {
    ALLOW_REAL_MODEL_REQUESTS.load(Ordering::SeqCst)
}

/// Set whether production model requests are globally allowed.
pub fn set_allow_real_model_requests(allow: bool) {
    ALLOW_REAL_MODEL_REQUESTS.store(allow, Ordering::SeqCst);
}

/// Scoped guard that restores the previous production-request setting when dropped.
#[derive(Debug)]
pub struct RealModelRequestGuard {
    previous: bool,
}

impl RealModelRequestGuard {
    /// Set the production-request setting for this scope.
    #[must_use]
    pub fn set(allow: bool) -> Self {
        let previous = ALLOW_REAL_MODEL_REQUESTS.swap(allow, Ordering::SeqCst);
        Self { previous }
    }
}

impl Drop for RealModelRequestGuard {
    fn drop(&mut self) {
        ALLOW_REAL_MODEL_REQUESTS.store(self.previous, Ordering::SeqCst);
    }
}

/// Block production model requests until the returned guard is dropped.
#[must_use]
pub fn block_real_model_requests() -> RealModelRequestGuard {
    RealModelRequestGuard::set(false)
}

/// Allow production model requests until the returned guard is dropped.
#[must_use]
pub fn allow_real_model_requests_guard() -> RealModelRequestGuard {
    RealModelRequestGuard::set(true)
}

/// Model adapter error.
#[derive(Debug, Error)]
pub enum ModelError {
    /// Canonical history cannot be mapped into a provider request.
    #[error("message mapping failed: {0}")]
    MessageMapping(String),
    /// Provider response cannot be parsed into canonical response.
    #[error("response parsing failed: {0}")]
    ResponseParsing(String),
    /// Transport failed.
    #[error("transport failed: {0}")]
    Transport(String),
    /// A real HTTP model request was blocked by the global test guard.
    #[error("real model request blocked for {url}")]
    RealModelRequestBlocked {
        /// Target request URL.
        url: String,
    },
    /// Provider returned a non-success status.
    #[error("provider status {status}: {body}")]
    ProviderStatus {
        /// HTTP status code.
        status: u16,
        /// Provider response body.
        body: Value,
        /// Whether retry policy may retry this status.
        retryable: bool,
    },
    /// Retry attempts were exhausted.
    #[error("retry attempts exhausted after {attempts} attempts: {source}")]
    RetryExhausted {
        /// Attempt count.
        attempts: u32,
        /// Last error.
        source: Box<Self>,
    },
    /// Provider returned an unsupported response shape.
    #[error("unsupported provider response: {0}")]
    UnsupportedResponse(String),
}

/// Request parameters derived from tools, output schemas, and runtime policy.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelRequestParameters {
    /// Tool definitions in provider-neutral JSON schema form.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    /// Provider-executed native tool definitions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_tools: Vec<NativeToolDefinition>,
    /// Optional output schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    /// Request-level HTTP overrides for gateway, audit, and routing integrations.
    #[serde(default)]
    pub http: HttpRequestOptions,
    /// Provider-specific JSON object merged into the top-level request body.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub extra_body: Map<String, Value>,
}

/// Provider-neutral tool definition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolDefinition {
    /// Tool name.
    pub name: String,
    /// Tool description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON schema parameters.
    #[serde(default)]
    pub parameters: Value,
    /// Runtime metadata for capability hooks, filtering, approval, and provider adaptation.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

/// Provider-executed native tool definition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NativeToolDefinition {
    /// Provider-neutral native tool type, such as `web_search` or `code_interpreter`.
    pub tool_type: String,
    /// Provider-specific native tool configuration.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub config: Map<String, Value>,
    /// Runtime metadata for capability hooks, filtering, and audit.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

impl NativeToolDefinition {
    /// Create a native tool definition.
    #[must_use]
    pub fn new(tool_type: impl Into<String>) -> Self {
        Self {
            tool_type: tool_type.into(),
            config: Map::new(),
            metadata: Map::new(),
        }
    }

    /// Attach provider-specific configuration.
    #[must_use]
    pub fn with_config(mut self, config: Map<String, Value>) -> Self {
        self.config = config;
        self
    }

    /// Attach runtime metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Map<String, Value>) -> Self {
        self.metadata = metadata;
        self
    }
}

/// Per-request context attached by the runtime.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelRequestContext {
    /// Run identifier.
    pub run_id: RunId,
    /// Conversation identifier.
    pub conversation_id: ConversationId,
    /// Trace correlation context propagated from the runtime context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
}

impl ModelRequestContext {
    /// Build request context for one model call.
    #[must_use]
    pub fn new(run_id: RunId, conversation_id: ConversationId) -> Self {
        Self {
            run_id,
            conversation_id,
            trace_context: TraceContext::default(),
        }
    }

    /// Attach trace correlation context.
    #[must_use]
    pub fn with_trace_context(mut self, trace_context: TraceContext) -> Self {
        self.trace_context = trace_context;
        self
    }
}

/// Provider-neutral model adapter.
#[async_trait]
pub trait ModelAdapter: Send + Sync {
    /// Provider model name.
    fn model_name(&self) -> &str;

    /// Provider name.
    fn provider_name(&self) -> Option<&str>;

    /// Model capability profile.
    fn profile(&self) -> &ModelProfile;

    /// Default generation settings.
    fn default_settings(&self) -> Option<&ModelSettings>;

    /// Perform a complete model request.
    async fn request(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError>;

    /// Stream a model request as canonical response part deltas.
    async fn request_stream(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        let response = self.request(messages, settings, params, context).await?;
        Ok(vec![ModelResponseStreamEvent::FinalResult(response)])
    }

    /// Count tokens for a request where provider support exists.
    async fn count_tokens(
        &self,
        _messages: &[ModelMessage],
        _settings: Option<&ModelSettings>,
        _params: &ModelRequestParameters,
    ) -> Result<Usage, ModelError> {
        Ok(Usage::default())
    }
}
