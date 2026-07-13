//! Runtime event compatibility export and append-only event bus.

use serde::{Deserialize, Serialize};

pub use starweaver_core::AgentEvent;

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
