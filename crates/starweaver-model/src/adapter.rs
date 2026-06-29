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
pub use traits::{ModelAdapter, ModelRunSession};

use crate::ModelResponseStreamEvent;
use starweaver_core::CancellationToken;

/// Receiver for incremental canonical model stream events.
pub struct ModelResponseEventStream {
    receiver: tokio::sync::mpsc::Receiver<Result<ModelResponseStreamEvent, ModelError>>,
    cancellation_token: CancellationToken,
}

impl ModelResponseEventStream {
    /// Build a stream from a channel receiver.
    #[must_use]
    pub fn new(
        receiver: tokio::sync::mpsc::Receiver<Result<ModelResponseStreamEvent, ModelError>>,
    ) -> Self {
        Self::new_with_cancellation(receiver, CancellationToken::default())
    }

    /// Build a stream from a channel receiver and cancellation token.
    #[must_use]
    pub const fn new_with_cancellation(
        receiver: tokio::sync::mpsc::Receiver<Result<ModelResponseStreamEvent, ModelError>>,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            receiver,
            cancellation_token,
        }
    }

    /// Receive the next canonical model stream event.
    pub async fn recv(&mut self) -> Option<Result<ModelResponseStreamEvent, ModelError>> {
        if self.cancellation_token.is_cancelled() {
            return Some(Err(ModelError::Cancelled {
                reason: "model stream cancellation requested".to_string(),
            }));
        }
        tokio::select! {
            biased;
            () = self.cancellation_token.cancelled() => Some(Err(ModelError::Cancelled {
                reason: "model stream cancellation requested".to_string(),
            })),
            event = self.receiver.recv() => event,
        }
    }
}
