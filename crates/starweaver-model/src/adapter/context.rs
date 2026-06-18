use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use starweaver_core::{CancellationToken, ConversationId, RunId, TraceContext};

/// Per-request context attached by the runtime.
#[derive(Clone, Deserialize, Serialize)]
pub struct ModelRequestContext {
    /// Run identifier.
    pub run_id: RunId,
    /// Conversation identifier.
    pub conversation_id: ConversationId,
    /// Trace correlation context propagated from the runtime context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
    /// Debug metadata for raw provider request/response/event-stream evidence.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub llm_trace_metadata: Map<String, Value>,
    /// Cancellation token for streaming and long-running provider requests.
    #[serde(skip, default)]
    pub cancellation_token: CancellationToken,
}

impl std::fmt::Debug for ModelRequestContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelRequestContext")
            .field("run_id", &self.run_id)
            .field("conversation_id", &self.conversation_id)
            .field("trace_context", &self.trace_context)
            .field("cancelled", &self.cancellation_token.is_cancelled())
            .finish_non_exhaustive()
    }
}

impl PartialEq for ModelRequestContext {
    fn eq(&self, other: &Self) -> bool {
        self.run_id == other.run_id
            && self.conversation_id == other.conversation_id
            && self.trace_context == other.trace_context
            && self.llm_trace_metadata == other.llm_trace_metadata
            && self.cancellation_token == other.cancellation_token
    }
}

impl Eq for ModelRequestContext {}

impl ModelRequestContext {
    /// Build request context for one model call.
    #[must_use]
    pub fn new(run_id: RunId, conversation_id: ConversationId) -> Self {
        Self {
            run_id,
            conversation_id,
            trace_context: TraceContext::default(),
            llm_trace_metadata: Map::new(),
            cancellation_token: CancellationToken::default(),
        }
    }

    /// Attach trace correlation context.
    #[must_use]
    pub fn with_trace_context(mut self, trace_context: TraceContext) -> Self {
        self.trace_context = trace_context;
        self
    }

    /// Attach LLM request debug metadata.
    #[must_use]
    pub fn with_llm_trace_metadata(mut self, metadata: Map<String, Value>) -> Self {
        self.llm_trace_metadata = metadata;
        self
    }

    /// Attach a cancellation token.
    #[must_use]
    pub fn with_cancellation_token(mut self, token: CancellationToken) -> Self {
        self.cancellation_token = token;
        self
    }

    /// Return the shared cancellation token.
    #[must_use]
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation_token.clone()
    }
}
