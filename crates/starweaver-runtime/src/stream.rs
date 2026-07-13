//! Runtime stream-result type and compatibility exports for raw stream protocol records.

use serde::{Deserialize, Serialize};

use crate::AgentResult;

pub use starweaver_stream::{
    AgentSidebandEvent, AgentSidebandEventCategory, AgentStreamEvent, AgentStreamRecord,
    AgentStreamSink, AgentStreamSource, AgentStreamSourceKind,
};

/// Result returned by collection-based stream runs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentStreamResult {
    /// Final agent result.
    pub result: AgentResult,
    /// Events captured while the run progressed.
    pub events: Vec<AgentStreamRecord>,
}

impl AgentStreamResult {
    /// Return captured stream records.
    #[must_use]
    pub fn events(&self) -> &[AgentStreamRecord] {
        &self.events
    }

    /// Return the final result.
    #[must_use]
    pub const fn result(&self) -> &AgentResult {
        &self.result
    }

    /// Project captured raw runtime stream records into JSON values.
    ///
    /// # Errors
    ///
    /// Returns a serialization error if a nested event payload cannot be encoded.
    pub fn raw_json_records(&self) -> serde_json::Result<Vec<serde_json::Value>> {
        self.events
            .iter()
            .map(AgentStreamRecord::to_raw_json)
            .collect()
    }
}

pub(crate) fn push_stream_event(
    events: &mut Option<&mut Vec<AgentStreamRecord>>,
    event: AgentStreamEvent,
) {
    if let Some(events) = events.as_deref_mut() {
        events.push(AgentStreamRecord::new(events.len(), event));
    }
}

pub(crate) fn push_stream_record(
    events: &mut Option<&mut Vec<AgentStreamRecord>>,
    record: AgentStreamRecord,
) {
    if let Some(events) = events.as_deref_mut() {
        events.push(record.with_sequence(events.len()));
    }
}
