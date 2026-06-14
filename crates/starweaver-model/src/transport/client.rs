use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::ModelError;

use super::{HttpRequest, HttpResponse};

/// Async HTTP client abstraction used by production model adapters.
#[async_trait]
pub trait ModelHttpClient: Send + Sync {
    /// Send a JSON model request.
    ///
    /// # Errors
    ///
    /// Returns an error when transport, status, or response decoding fails.
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, ModelError>;

    /// Send a server-sent events model request and return JSON `data:` payloads.
    ///
    /// # Errors
    ///
    /// Returns an error when transport, status, or event decoding fails.
    async fn send_event_stream(&self, request: HttpRequest) -> Result<Vec<Value>, ModelError> {
        let mut stream = self.send_event_stream_incremental(request).await?;
        let mut events = Vec::new();
        while let Some(event) = stream.recv().await {
            events.push(event?);
        }
        Ok(events)
    }

    /// Send a server-sent events model request and return events as they arrive.
    ///
    /// # Errors
    ///
    /// Returns an error when transport setup fails.
    async fn send_event_stream_incremental(
        &self,
        request: HttpRequest,
    ) -> Result<ModelEventStream, ModelError> {
        Err(ModelError::Transport(format!(
            "server-sent event streaming is not implemented for {}",
            request.url
        )))
    }
}

/// Receiver for incremental model SSE JSON events.
pub struct ModelEventStream {
    receiver: tokio::sync::mpsc::Receiver<Result<Value, ModelError>>,
}

impl ModelEventStream {
    /// Build a stream from a channel receiver.
    #[must_use]
    pub const fn new(receiver: tokio::sync::mpsc::Receiver<Result<Value, ModelError>>) -> Self {
        Self { receiver }
    }

    /// Receive the next JSON event from the stream.
    pub async fn recv(&mut self) -> Option<Result<Value, ModelError>> {
        self.receiver.recv().await
    }
}

/// Shared reference to an HTTP client.
pub type DynHttpClient = Arc<dyn ModelHttpClient>;
