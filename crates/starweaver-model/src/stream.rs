//! Canonical model stream events.

use serde::{Deserialize, Serialize};

use crate::message::ModelResponse;

/// Stream event emitted by model adapters.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelResponseStreamEvent {
    /// A response part started.
    PartStart(PartStart),
    /// A response part delta arrived.
    PartDelta(PartDelta),
    /// A response part ended.
    PartEnd(PartEnd),
    /// Final response is available.
    FinalResult(ModelResponse),
}

/// Part start event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PartStart {
    /// Part index in response.
    pub index: usize,
    /// Part kind.
    pub part_kind: String,
}

/// Part delta event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PartDelta {
    /// Part index in response.
    pub index: usize,
    /// Delta text or JSON payload encoded as text.
    pub delta: String,
}

/// Part end event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PartEnd {
    /// Part index in response.
    pub index: usize,
}
