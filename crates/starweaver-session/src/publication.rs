//! Durable stream-publication outbox contracts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use starweaver_core::{RunId, SessionId};
use starweaver_stream::{AgentStreamRecord, DisplayMessage, ReplayEvent, ReplaySnapshot};

/// External sink families selected for reliable publication.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StreamPublicationTargets {
    /// Publish raw and projected records to a [`starweaver_stream::StreamArchive`].
    pub archive: bool,
    /// Publish typed events to a [`starweaver_stream::ReplayEventLog`].
    pub replay: bool,
}

impl StreamPublicationTargets {
    /// Build target flags from configured sinks.
    #[must_use]
    pub const fn new(archive: bool, replay: bool) -> Self {
        Self { archive, replay }
    }

    /// Return true when no external sink was selected.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        !self.archive && !self.replay
    }
}

/// One transactionally enqueued stream-publication batch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PendingStreamPublication {
    /// Stable outbox identity. One sealed run evidence bundle owns one publication.
    pub publication_id: String,
    /// Session that owns the evidence.
    pub session_id: SessionId,
    /// Run that owns the evidence.
    pub run_id: RunId,
    /// Raw runtime records for the archive sink.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stream_records: Vec<AgentStreamRecord>,
    /// Projected display messages for the archive sink.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub display_messages: Vec<DisplayMessage>,
    /// Typed replay events for the replay-log sink.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replay_events: Vec<ReplayEvent>,
    /// Optional compact display snapshot for the archive sink.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_snapshot: Option<ReplaySnapshot>,
    /// Whether archive delivery still requires acknowledgement.
    pub archive_pending: bool,
    /// Whether replay-log delivery still requires acknowledgement.
    pub replay_pending: bool,
    /// Time at which the evidence transaction enqueued this batch.
    pub created_at: DateTime<Utc>,
}

impl starweaver_core::VersionedRecord for PendingStreamPublication {
    const SCHEMA: &'static str = "starweaver.session.stream_publication";
}

fn publication_id(session_id: &SessionId, run_id: &RunId) -> String {
    let mut digest = Sha256::new();
    for component in [session_id.as_str(), run_id.as_str()] {
        digest.update(component.len().to_string().as_bytes());
        digest.update(b":");
        digest.update(component.as_bytes());
        digest.update(b";");
    }
    format!("publication-sha256:{:x}", digest.finalize())
}

impl PendingStreamPublication {
    /// Build a deterministic publication from one run's sealed evidence.
    #[must_use]
    pub fn new(
        session_id: SessionId,
        run_id: RunId,
        targets: StreamPublicationTargets,
        created_at: DateTime<Utc>,
    ) -> Self {
        let publication_id = publication_id(&session_id, &run_id);
        Self {
            publication_id,
            session_id,
            run_id,
            stream_records: Vec::new(),
            display_messages: Vec::new(),
            replay_events: Vec::new(),
            display_snapshot: None,
            archive_pending: targets.archive,
            replay_pending: targets.replay,
            created_at,
        }
    }

    /// Return true when every configured sink acknowledged the batch.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        !self.archive_pending && !self.replay_pending
    }
}

/// One independently acknowledged external stream sink.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamPublicationTarget {
    /// Raw records, display messages, and snapshots in a stream archive.
    Archive,
    /// Typed events in a replay event log.
    Replay,
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use starweaver_core::{RunId, SessionId};

    use super::{PendingStreamPublication, StreamPublicationTargets};

    #[test]
    fn publication_identity_is_unambiguous_for_delimiter_bearing_ids() {
        let first = PendingStreamPublication::new(
            SessionId::from_string("a:b"),
            RunId::from_string("c"),
            StreamPublicationTargets::new(true, true),
            Utc::now(),
        );
        let second = PendingStreamPublication::new(
            SessionId::from_string("a"),
            RunId::from_string("b:c"),
            StreamPublicationTargets::new(true, true),
            Utc::now(),
        );
        assert_ne!(first.publication_id, second.publication_id);
    }
}
