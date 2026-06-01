//! Stream archive contracts and in-memory implementation.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::{Metadata, RunId, SessionId};
use starweaver_runtime::AgentStreamRecord;

use crate::{
    display::DisplayMessage,
    error::{ReplayError, ReplayResult},
    replay::{ReplayCursor, ReplayScope, ReplaySnapshot},
};

/// Archived stream record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StreamArchiveRecord {
    /// Session id.
    pub session_id: SessionId,
    /// Run id.
    pub run_id: RunId,
    /// Record sequence.
    pub sequence: usize,
    /// Raw record family.
    pub family: String,
    /// Record payload.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub payload: Value,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Record metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Durable stream archive contract.
#[async_trait]
pub trait StreamArchive: Send + Sync {
    /// Append raw runtime stream records.
    async fn append_raw_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        records: Vec<AgentStreamRecord>,
    ) -> ReplayResult<()>;

    /// Replay raw runtime stream records after a cursor.
    async fn replay_raw_after(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<Vec<AgentStreamRecord>>;

    /// Append projected display messages.
    async fn append_display_messages(
        &self,
        scope: ReplayScope,
        messages: Vec<DisplayMessage>,
    ) -> ReplayResult<()>;

    /// Replay projected display messages after a cursor.
    async fn replay_display_after(
        &self,
        scope: &ReplayScope,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<Vec<DisplayMessage>>;

    /// Append compact snapshot.
    async fn append_snapshot(
        &self,
        scope: ReplayScope,
        snapshot: ReplaySnapshot,
    ) -> ReplayResult<()>;

    /// Load latest compact snapshot.
    async fn latest_snapshot(&self, scope: &ReplayScope) -> ReplayResult<Option<ReplaySnapshot>>;

    /// Return cursor range for a display scope.
    async fn cursor_range(
        &self,
        scope: &ReplayScope,
    ) -> ReplayResult<Option<(ReplayCursor, ReplayCursor)>>;
}

/// In-memory stream archive for deterministic tests and single-process hosts.
#[derive(Clone, Debug, Default)]
pub struct InMemoryStreamArchive {
    inner: Arc<Mutex<ArchiveInner>>,
}

#[derive(Clone, Debug, Default)]
struct ArchiveInner {
    raw: BTreeMap<(SessionId, RunId), Vec<AgentStreamRecord>>,
    display: BTreeMap<ReplayScope, Vec<DisplayMessage>>,
    snapshots: BTreeMap<ReplayScope, ReplaySnapshot>,
}

impl InMemoryStreamArchive {
    /// Create an empty archive.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

fn raw_key(session_id: &SessionId, run_id: &RunId) -> (SessionId, RunId) {
    (session_id.clone(), run_id.clone())
}

#[allow(clippy::needless_pass_by_value)]
fn failed(error: std::sync::PoisonError<std::sync::MutexGuard<'_, ArchiveInner>>) -> ReplayError {
    ReplayError::Failed(error.to_string())
}

#[async_trait]
impl StreamArchive for InMemoryStreamArchive {
    async fn append_raw_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        records: Vec<AgentStreamRecord>,
    ) -> ReplayResult<()> {
        let mut inner = self.inner.lock().map_err(failed)?;
        let raw = inner.raw.entry(raw_key(session_id, run_id)).or_default();
        for record in records {
            if raw
                .iter()
                .all(|existing| existing.sequence != record.sequence)
            {
                raw.push(record);
            }
        }
        raw.sort_by_key(|record| record.sequence);
        Ok(())
    }

    async fn replay_raw_after(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<Vec<AgentStreamRecord>> {
        if let Some(cursor) = cursor.as_ref() {
            let scope = ReplayScope::run(run_id.as_str());
            cursor.validate_scope(&scope)?;
        }
        let inner = self.inner.lock().map_err(failed)?;
        let after = cursor.map_or(0, |cursor| cursor.sequence.saturating_add(1));
        Ok(inner
            .raw
            .get(&raw_key(session_id, run_id))
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|record| record.sequence >= after)
            .collect())
    }

    async fn append_display_messages(
        &self,
        scope: ReplayScope,
        messages: Vec<DisplayMessage>,
    ) -> ReplayResult<()> {
        let mut inner = self.inner.lock().map_err(failed)?;
        let display = inner.display.entry(scope).or_default();
        for message in messages {
            if display
                .iter()
                .all(|existing| existing.sequence != message.sequence)
            {
                display.push(message);
            }
        }
        display.sort_by_key(|message| message.sequence);
        Ok(())
    }

    async fn replay_display_after(
        &self,
        scope: &ReplayScope,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<Vec<DisplayMessage>> {
        if let Some(cursor) = cursor.as_ref() {
            cursor.validate_scope(scope)?;
        }
        let inner = self.inner.lock().map_err(failed)?;
        let after = cursor.map_or(0, |cursor| cursor.sequence.saturating_add(1));
        Ok(inner
            .display
            .get(scope)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|message| message.sequence >= after)
            .collect())
    }

    async fn append_snapshot(
        &self,
        scope: ReplayScope,
        snapshot: ReplaySnapshot,
    ) -> ReplayResult<()> {
        let mut inner = self.inner.lock().map_err(failed)?;
        inner.snapshots.insert(scope, snapshot);
        Ok(())
    }

    async fn latest_snapshot(&self, scope: &ReplayScope) -> ReplayResult<Option<ReplaySnapshot>> {
        let inner = self.inner.lock().map_err(failed)?;
        Ok(inner.snapshots.get(scope).cloned())
    }

    async fn cursor_range(
        &self,
        scope: &ReplayScope,
    ) -> ReplayResult<Option<(ReplayCursor, ReplayCursor)>> {
        let inner = self.inner.lock().map_err(failed)?;
        let Some(messages) = inner.display.get(scope) else {
            return Ok(None);
        };
        let Some(first) = messages.first() else {
            return Ok(None);
        };
        let Some(last) = messages.last() else {
            return Ok(None);
        };
        Ok(Some((
            ReplayCursor::new(scope.clone(), first.sequence),
            ReplayCursor::new(scope.clone(), last.sequence),
        )))
    }
}
