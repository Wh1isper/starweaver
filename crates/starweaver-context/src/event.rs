//! Runtime event records and append-only event bus.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::Metadata;

/// Runtime event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentEvent {
    /// Event type.
    pub kind: String,
    /// Event payload.
    #[serde(default)]
    pub payload: Value,
    /// Event metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl AgentEvent {
    /// Create an event.
    #[must_use]
    pub fn new(kind: impl Into<String>, payload: Value) -> Self {
        Self {
            kind: kind.into(),
            payload,
            metadata: Metadata::default(),
        }
    }

    /// Attach event metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }
}

/// Append-only in-memory event bus.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EventBus {
    events: Vec<AgentEvent>,
}

impl EventBus {
    /// Create an empty event bus.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish one event.
    pub fn publish(&mut self, event: AgentEvent) {
        self.events.push(event);
    }

    /// Return the number of retained events.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.events.len()
    }

    /// Return whether the event bus is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Return all events.
    #[must_use]
    pub fn events(&self) -> &[AgentEvent] {
        &self.events
    }

    /// Drain all events.
    pub fn drain(&mut self) -> Vec<AgentEvent> {
        std::mem::take(&mut self.events)
    }
}
