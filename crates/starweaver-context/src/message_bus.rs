//! FIFO message bus records for steering active and future runs.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::Metadata;

/// Steering or coordination message.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BusMessage {
    /// Message topic.
    pub topic: String,
    /// Message payload.
    #[serde(default)]
    pub payload: Value,
    /// Message metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl BusMessage {
    /// Create a bus message.
    #[must_use]
    pub fn new(topic: impl Into<String>, payload: Value) -> Self {
        Self {
            topic: topic.into(),
            payload,
            metadata: Metadata::default(),
        }
    }
}

/// FIFO message bus for steering active and future runs.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct MessageBus {
    messages: VecDeque<BusMessage>,
}

impl MessageBus {
    /// Create an empty message bus.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue one message.
    pub fn enqueue(&mut self, message: BusMessage) {
        self.messages.push_back(message);
    }

    /// Dequeue one message.
    pub fn dequeue(&mut self) -> Option<BusMessage> {
        self.messages.pop_front()
    }

    /// Return number of queued messages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Return whether any queued message has the provided topic.
    #[must_use]
    pub fn has_topic(&self, topic: &str) -> bool {
        self.messages.iter().any(|message| message.topic == topic)
    }

    /// Return whether the bus has no messages.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}
