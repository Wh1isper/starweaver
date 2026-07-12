//! Stable event records and event-kind identifiers shared across execution layers.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Metadata;

/// Product-neutral event published by runtime context and projected into streams.
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

/// Custom event kind emitted with a full task board snapshot.
pub const TASK_SNAPSHOT_EVENT_KIND: &str = "task_snapshot";
