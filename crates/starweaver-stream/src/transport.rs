//! Replay transport contracts.

use async_trait::async_trait;

use crate::{
    envelope::{JsonlEnvelope, ReplayEnvelope, SseEnvelope},
    error::{ReplayError, ReplayResult},
    replay::{ReplayCursor, ReplayEvent, ReplayEventKind, ReplayEventLog, ReplayScope},
};

/// Replay transport contract.
#[async_trait]
pub trait ReplayTransport: Send + Sync {
    /// Replay after cursor and return protocol envelopes.
    async fn replay(
        &self,
        scope: ReplayScope,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<Vec<ReplayEnvelope>>;
}

/// SSE replay frame independent from an HTTP framework.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplaySseFrame {
    /// SSE event id.
    pub id: String,
    /// SSE event name.
    pub event: String,
    /// JSON string payload.
    pub data: String,
}

impl ReplaySseFrame {
    /// Build a replay frame with a ready payload.
    #[must_use]
    pub fn ready() -> Self {
        Self {
            id: String::new(),
            event: "ready".to_string(),
            data: "{}".to_string(),
        }
    }

    /// Build a replay frame from an event and serialize the full replay event as payload.
    ///
    /// # Errors
    ///
    /// Returns a replay error when JSON serialization fails.
    pub fn from_event(event: &ReplayEvent) -> ReplayResult<Self> {
        Ok(Self {
            id: event.sequence.to_string(),
            event: replay_sse_event_name(&event.event).to_string(),
            data: serde_json::to_string(event)
                .map_err(|error| ReplayError::Failed(error.to_string()))?,
        })
    }

    /// Render a textual SSE frame.
    #[must_use]
    pub fn to_frame(&self) -> String {
        let id = if self.id.is_empty() {
            String::new()
        } else {
            format!("id: {}\n", self.id)
        };
        format!("{id}event: {}\ndata: {}\n\n", self.event, self.data)
    }
}

/// Build SSE replay frames from events.
///
/// # Errors
///
/// Returns a replay error when JSON serialization fails.
pub fn replay_sse_frames(events: &[ReplayEvent]) -> ReplayResult<Vec<ReplaySseFrame>> {
    events.iter().map(ReplaySseFrame::from_event).collect()
}

/// Return the canonical SSE event name for a replay event kind.
#[must_use]
pub const fn replay_sse_event_name(kind: &ReplayEventKind) -> &'static str {
    match kind {
        ReplayEventKind::DisplayMessage(_) => "display_message",
        ReplayEventKind::Raw(_) => "raw",
        ReplayEventKind::Snapshot(_) => "snapshot",
        ReplayEventKind::Heartbeat => "heartbeat",
        ReplayEventKind::Terminal { .. } => "terminal",
    }
}

/// In-memory transport adapter over a replay event log.
pub struct InMemoryReplayTransport<L> {
    log: L,
    protocol: ReplayTransportProtocol,
}

/// Built-in replay transport protocols.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplayTransportProtocol {
    /// SSE envelopes.
    Sse,
    /// JSON Lines envelopes.
    Jsonl,
}

impl<L> InMemoryReplayTransport<L> {
    /// Build an SSE transport.
    #[must_use]
    pub const fn sse(log: L) -> Self {
        Self {
            log,
            protocol: ReplayTransportProtocol::Sse,
        }
    }

    /// Build a JSONL transport.
    #[must_use]
    pub const fn jsonl(log: L) -> Self {
        Self {
            log,
            protocol: ReplayTransportProtocol::Jsonl,
        }
    }
}

#[async_trait]
impl<L> ReplayTransport for InMemoryReplayTransport<L>
where
    L: ReplayEventLog,
{
    async fn replay(
        &self,
        scope: ReplayScope,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<Vec<ReplayEnvelope>> {
        let events = self.log.replay_after(&scope, cursor, None).await?;
        Ok(events
            .iter()
            .map(|event| match self.protocol {
                ReplayTransportProtocol::Sse => ReplayEnvelope::Sse(SseEnvelope::from_event(event)),
                ReplayTransportProtocol::Jsonl => {
                    ReplayEnvelope::Jsonl(JsonlEnvelope::from_event(event))
                }
            })
            .collect())
    }
}
