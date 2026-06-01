//! Replay transport contracts.

use async_trait::async_trait;

use crate::{
    envelope::{JsonlEnvelope, ReplayEnvelope, SseEnvelope},
    error::ReplayResult,
    replay::{ReplayCursor, ReplayEventLog, ReplayScope},
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
