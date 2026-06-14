//! Model adapter traits and request context types.

mod context;
mod error;
mod guard;
mod params;
mod traits;

pub use context::ModelRequestContext;
pub use error::ModelError;
pub use guard::{
    allow_real_model_requests, allow_real_model_requests_guard, block_real_model_requests,
    set_allow_real_model_requests, RealModelRequestGuard,
};
pub use params::{ModelRequestParameters, NativeToolDefinition, ToolDefinition};
pub use traits::ModelAdapter;

use crate::ModelResponseStreamEvent;

/// Receiver for incremental canonical model stream events.
pub struct ModelResponseEventStream {
    receiver: tokio::sync::mpsc::Receiver<Result<ModelResponseStreamEvent, ModelError>>,
}

impl ModelResponseEventStream {
    /// Build a stream from a channel receiver.
    #[must_use]
    pub const fn new(
        receiver: tokio::sync::mpsc::Receiver<Result<ModelResponseStreamEvent, ModelError>>,
    ) -> Self {
        Self { receiver }
    }

    /// Receive the next canonical model stream event.
    pub async fn recv(&mut self) -> Option<Result<ModelResponseStreamEvent, ModelError>> {
        self.receiver.recv().await
    }
}
