use crate::{ModelError, ModelResponseStreamEvent};

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
