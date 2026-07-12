//! Replay event log contracts and in-memory implementation.

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::Metadata;
use tokio::sync::broadcast;

use crate::{
    display::DisplayMessage,
    error::{ReplayError, ReplayResult},
};

/// Replay stream scope.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ReplayScope(String);

impl ReplayScope {
    /// Create a scope from string.
    #[must_use]
    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Build a run-local scope.
    #[must_use]
    pub fn run(run_id: impl AsRef<str>) -> Self {
        Self(format!("run:{}", run_id.as_ref()))
    }

    /// Build a session scope.
    #[must_use]
    pub fn session(session_id: impl AsRef<str>) -> Self {
        Self(format!("session:{}", session_id.as_ref()))
    }

    /// Return string representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Replay cursor family.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayCursorFamily {
    /// Raw runtime stream records.
    RawRuntime,
    /// Projected display messages.
    Display,
    /// Typed replay event log entries.
    ReplayEvent,
}

impl ReplayCursorFamily {
    /// Return the stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RawRuntime => "raw_runtime",
            Self::Display => "display",
            Self::ReplayEvent => "replay_event",
        }
    }
}

/// Family-aware replay cursor.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReplayCursor {
    /// Stream family. This prevents a raw-runtime cursor from entering a display or event API.
    pub family: ReplayCursorFamily,
    /// Replay scope.
    pub scope: ReplayScope,
    /// Last delivered sequence.
    pub sequence: usize,
    /// Optional backend-specific cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_cursor: Option<String>,
}

impl ReplayCursor {
    /// Build a raw-runtime cursor.
    #[must_use]
    pub const fn raw_runtime(scope: ReplayScope, sequence: usize) -> Self {
        Self::for_family(ReplayCursorFamily::RawRuntime, scope, sequence)
    }

    /// Build a projected-display cursor.
    #[must_use]
    pub const fn display(scope: ReplayScope, sequence: usize) -> Self {
        Self::for_family(ReplayCursorFamily::Display, scope, sequence)
    }

    /// Build a typed replay-event cursor.
    #[must_use]
    pub const fn replay_event(scope: ReplayScope, sequence: usize) -> Self {
        Self::for_family(ReplayCursorFamily::ReplayEvent, scope, sequence)
    }

    /// Build a cursor for an explicit family.
    #[must_use]
    pub const fn for_family(
        family: ReplayCursorFamily,
        scope: ReplayScope,
        sequence: usize,
    ) -> Self {
        Self {
            family,
            scope,
            sequence,
            backend_cursor: None,
        }
    }

    /// Validate this cursor against a requested family and scope.
    ///
    /// # Errors
    ///
    /// Returns `ReplayError::InvalidCursor` when the cursor belongs to another family or scope.
    pub fn validate(&self, family: ReplayCursorFamily, scope: &ReplayScope) -> ReplayResult<()> {
        if self.family != family {
            return Err(ReplayError::InvalidCursor(format!(
                "cursor family {} does not match requested family {}",
                self.family.as_str(),
                family.as_str()
            )));
        }
        if &self.scope != scope {
            return Err(ReplayError::InvalidCursor(format!(
                "cursor scope {} does not match requested scope {}",
                self.scope.as_str(),
                scope.as_str()
            )));
        }
        Ok(())
    }
}

/// Terminal marker carried by replay events.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamTerminalMarker {
    /// Run completed successfully.
    RunCompleted,
    /// Run failed.
    RunFailed {
        /// Failure code.
        code: String,
        /// Failure message.
        message: String,
    },
    /// Run was cancelled or interrupted.
    RunCancelled {
        /// Human-readable reason.
        reason: String,
    },
}

/// Environment lifecycle item carried by typed replay events.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentLifecycleEvent {
    /// Stable lifecycle kind, such as `environment_info`.
    pub operation_kind: String,
    /// Session that owns the run.
    pub session_id: String,
    /// Run that owns the active environment binding.
    pub run_id: String,
    /// Environment binding version described by this event.
    pub binding_version: u64,
    /// Current environment summary.
    pub environment: Value,
    /// Operation id for active mutations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    /// Additional lifecycle payload fields.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
}

impl EnvironmentLifecycleEvent {
    /// Project this typed event to the legacy display-message shape.
    #[must_use]
    pub fn to_display_message(&self, sequence: usize) -> DisplayMessage {
        let mut payload = serde_json::Map::new();
        payload.insert(
            "operationKind".to_string(),
            Value::String(self.operation_kind.clone()),
        );
        payload.insert(
            "kind".to_string(),
            Value::String(self.operation_kind.clone()),
        );
        if let Some(operation_id) = self.operation_id.as_ref() {
            payload.insert(
                "operationId".to_string(),
                Value::String(operation_id.clone()),
            );
        }
        payload.insert("runId".to_string(), Value::String(self.run_id.clone()));
        payload.insert(
            "bindingVersion".to_string(),
            Value::Number(self.binding_version.into()),
        );
        payload.insert("environment".to_string(), self.environment.clone());
        for (key, value) in &self.extra {
            payload.insert(key.clone(), value.clone());
        }
        DisplayMessage::new(
            sequence,
            starweaver_core::SessionId::from_string(self.session_id.clone()),
            starweaver_core::RunId::from_string(self.run_id.clone()),
            crate::DisplayMessageKind::HostEvent,
        )
        .with_payload(Value::Object(payload))
        .with_preview(format!("environment {}", self.operation_kind))
    }
}

/// Replay event kind.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReplayEventKind {
    /// Display message event.
    DisplayMessage(Box<DisplayMessage>),
    /// Typed environment lifecycle event.
    EnvironmentLifecycle(Box<EnvironmentLifecycleEvent>),
    /// Raw payload event.
    Raw(Value),
    /// Compact snapshot marker.
    Snapshot(ReplaySnapshot),
    /// Heartbeat event.
    Heartbeat,
    /// Terminal event marker.
    Terminal {
        /// Terminal marker payload.
        marker: StreamTerminalMarker,
    },
}

/// Sequenced replay event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReplayEvent {
    /// Replay scope.
    pub scope: ReplayScope,
    /// Monotonic sequence within the scope.
    pub sequence: usize,
    /// Event timestamp.
    pub timestamp: DateTime<Utc>,
    /// Event kind.
    pub event: ReplayEventKind,
    /// Event metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl starweaver_core::VersionedRecord for ReplayEvent {
    const SCHEMA: &'static str = "starweaver.stream.replay_event";
    const ALLOW_BARE_V0: bool = true;
}

impl ReplayEvent {
    /// Build a replay event from a kind.
    #[must_use]
    pub fn new(scope: ReplayScope, sequence: usize, event: ReplayEventKind) -> Self {
        Self {
            scope,
            sequence,
            timestamp: Utc::now(),
            event,
            metadata: Metadata::default(),
        }
    }

    /// Build a replay event from a display message whose display and event sequences align.
    #[must_use]
    pub fn display(scope: ReplayScope, message: DisplayMessage) -> Self {
        Self::display_at(scope, message.sequence, message)
    }

    /// Build a replay event with an event-family sequence independent from display sequencing.
    #[must_use]
    pub fn display_at(scope: ReplayScope, event_sequence: usize, message: DisplayMessage) -> Self {
        let timestamp = message.timestamp;
        let mut event = Self::new(
            scope,
            event_sequence,
            ReplayEventKind::DisplayMessage(Box::new(message)),
        );
        event.timestamp = timestamp;
        event
    }
}

/// Compact replay snapshot.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReplaySnapshot {
    /// Replay scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ReplayScope>,
    /// Snapshot revision.
    pub revision: usize,
    /// Last included cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<ReplayCursor>,
    /// Compacted display messages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub display_messages: Vec<DisplayMessage>,
    /// Snapshot metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl starweaver_core::VersionedRecord for ReplaySnapshot {
    const SCHEMA: &'static str = "starweaver.stream.replay_snapshot";
    const ALLOW_BARE_V0: bool = true;

    fn decode_bare_v0(mut payload: Value) -> Result<Self, starweaver_core::VersionedRecordError> {
        if let Some(cursor) = payload
            .as_object_mut()
            .and_then(|object| object.get_mut("cursor"))
            .and_then(Value::as_object_mut)
            && !cursor.contains_key("family")
        {
            cursor.insert("family".to_string(), Value::String("display".to_string()));
        }
        serde_json::from_value(payload).map_err(starweaver_core::VersionedRecordError::Json)
    }
}

impl ReplaySnapshot {
    /// Validate a snapshot against its persistence family and scope.
    ///
    /// # Errors
    ///
    /// Returns an invalid-cursor error when the snapshot or its cursor belongs
    /// to another scope or stream family.
    pub fn validate(&self, family: ReplayCursorFamily, scope: &ReplayScope) -> ReplayResult<()> {
        if self.scope.as_ref() != Some(scope) {
            return Err(ReplayError::InvalidCursor(format!(
                "snapshot scope {} does not match requested scope {}",
                self.scope.as_ref().map_or("<missing>", ReplayScope::as_str),
                scope.as_str()
            )));
        }
        if let Some(cursor) = self.cursor.as_ref() {
            cursor.validate(family, scope)?;
        }
        Ok(())
    }
}

/// Replay subscription that yields live events after replay.
pub struct ReplaySubscription {
    receiver: broadcast::Receiver<ReplayEvent>,
    scope: ReplayScope,
    cursor: Option<ReplayCursor>,
    backlog: VecDeque<ReplayEvent>,
    catchup: Option<Arc<dyn ReplayCatchupSource>>,
    baseline_initialized: bool,
}

impl ReplaySubscription {
    /// Receive the next live replay event for this subscription.
    ///
    /// # Errors
    ///
    /// Returns an error if the live subscription channel is closed or lagged.
    pub async fn recv(&mut self) -> ReplayResult<ReplayEvent> {
        loop {
            if !self.baseline_initialized && !self.backlog.is_empty() {
                self.refill_from_durable().await?;
                if self.catchup.is_none() {
                    self.baseline_initialized = true;
                }
            }
            if let Some(event) = self.backlog.pop_front() {
                let expected = self
                    .cursor
                    .as_ref()
                    .map_or(event.sequence, |cursor| cursor.sequence.saturating_add(1));
                if event.scope != self.scope || event.sequence < expected {
                    continue;
                }
                if event.sequence == expected {
                    self.cursor = Some(ReplayCursor::replay_event(
                        event.scope.clone(),
                        event.sequence,
                    ));
                    return Ok(event);
                }
                self.backlog.push_front(event);
            }

            match self.receiver.recv().await {
                Ok(event) => {
                    if event.scope == self.scope {
                        self.prepend_backlog(vec![event]);
                        self.refill_from_durable().await?;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    self.refill_from_durable().await?;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    self.refill_from_durable().await?;
                    if self.backlog.is_empty() {
                        return Err(ReplayError::Failed(
                            "replay subscription channel closed".to_string(),
                        ));
                    }
                }
            }
        }
    }

    async fn refill_from_durable(&mut self) -> ReplayResult<()> {
        let Some(catchup) = self.catchup.clone() else {
            return Ok(());
        };
        let events = catchup
            .catch_up_after(&self.scope, self.cursor.clone(), None)
            .await?;
        self.prepend_backlog(events);
        if self.cursor.is_some() || !self.backlog.is_empty() {
            self.baseline_initialized = true;
        }
        Ok(())
    }

    /// Prepend a durable catch-up batch captured after the live receiver was established.
    pub fn prepend_backlog(&mut self, events: Vec<ReplayEvent>) {
        let after = self.cursor.as_ref().map(|cursor| cursor.sequence);
        let mut merged = BTreeMap::new();
        for event in events.into_iter().chain(self.backlog.drain(..)) {
            if event.scope == self.scope && after.is_none_or(|sequence| event.sequence > sequence) {
                merged.entry(event.sequence).or_insert(event);
            }
        }
        self.backlog = merged.into_values().collect();
    }

    /// Initialize the durable backlog captured after the live receiver was established.
    pub fn initialize_backlog(&mut self, events: Vec<ReplayEvent>) {
        self.prepend_backlog(events);
        self.baseline_initialized = true;
    }

    /// Replace the durable source used to refill sequence gaps and broadcast lag.
    pub fn set_catchup_source(&mut self, catchup: Arc<dyn ReplayCatchupSource>) {
        self.catchup = Some(catchup);
    }
}

/// Durable replay reader used by subscriptions to refill gaps and channel lag.
#[async_trait]
pub trait ReplayCatchupSource: Send + Sync {
    /// Replay events after an optional cursor.
    async fn catch_up_after(
        &self,
        scope: &ReplayScope,
        cursor: Option<ReplayCursor>,
        limit: Option<usize>,
    ) -> ReplayResult<Vec<ReplayEvent>>;
}

/// Replay event-log contract.
#[async_trait]
pub trait ReplayEventLog: Send + Sync {
    /// Append an ordered event with idempotency by `(scope, sequence)`.
    async fn append(&self, scope: ReplayScope, event: ReplayEvent) -> ReplayResult<()>;

    /// Replay events after an optional cursor.
    async fn replay_after(
        &self,
        scope: &ReplayScope,
        cursor: Option<ReplayCursor>,
        limit: Option<usize>,
    ) -> ReplayResult<Vec<ReplayEvent>>;

    /// Subscribe to live events for a scope after an optional cursor.
    async fn subscribe(
        &self,
        scope: ReplayScope,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<ReplaySubscription>;

    /// Return a compact snapshot for a scope.
    async fn compact_snapshot(&self, scope: &ReplayScope) -> ReplayResult<ReplaySnapshot>;
}

/// In-memory replay event log with replayable per-scope buffers.
#[derive(Clone, Debug)]
pub struct InMemoryReplayEventLog {
    inner: Arc<Mutex<EventLogInner>>,
    sender: broadcast::Sender<ReplayEvent>,
}

#[derive(Debug, Default)]
struct EventLogInner {
    events: BTreeMap<ReplayScope, Vec<ReplayEvent>>,
    seen: HashSet<(ReplayScope, usize)>,
    snapshots: BTreeMap<ReplayScope, ReplaySnapshot>,
}

impl Default for InMemoryReplayEventLog {
    fn default() -> Self {
        let (sender, _receiver) = broadcast::channel(256);
        Self {
            inner: Arc::new(Mutex::new(EventLogInner::default())),
            sender,
        }
    }
}

impl InMemoryReplayEventLog {
    /// Create an empty event log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[allow(clippy::needless_pass_by_value)]
fn failed(error: std::sync::PoisonError<std::sync::MutexGuard<'_, EventLogInner>>) -> ReplayError {
    ReplayError::Failed(error.to_string())
}

#[async_trait]
impl ReplayEventLog for InMemoryReplayEventLog {
    async fn append(&self, scope: ReplayScope, mut event: ReplayEvent) -> ReplayResult<()> {
        event.scope = scope.clone();
        let should_send = {
            let mut inner = self.inner.lock().map_err(failed)?;
            if inner.seen.contains(&(scope.clone(), event.sequence)) {
                let persisted = inner
                    .events
                    .get(&scope)
                    .and_then(|events| {
                        events
                            .iter()
                            .find(|persisted| persisted.sequence == event.sequence)
                    })
                    .ok_or_else(|| {
                        ReplayError::Failed(format!(
                            "replay event index is inconsistent for scope {} at sequence {}",
                            scope.as_str(),
                            event.sequence
                        ))
                    })?;
                if persisted != &event {
                    return Err(ReplayError::Failed(format!(
                        "replay event conflict for scope {} at sequence {}",
                        scope.as_str(),
                        event.sequence
                    )));
                }
                false
            } else {
                inner.seen.insert((scope.clone(), event.sequence));
                let events = inner.events.entry(scope).or_default();
                events.push(event.clone());
                events.sort_by_key(|event| event.sequence);
                true
            }
        };
        if should_send {
            let _send_result = self.sender.send(event);
        }
        Ok(())
    }

    async fn replay_after(
        &self,
        scope: &ReplayScope,
        cursor: Option<ReplayCursor>,
        limit: Option<usize>,
    ) -> ReplayResult<Vec<ReplayEvent>> {
        if let Some(cursor) = cursor.as_ref() {
            cursor.validate(ReplayCursorFamily::ReplayEvent, scope)?;
        }
        let inner = self.inner.lock().map_err(failed)?;
        let after = cursor.map_or(0, |cursor| cursor.sequence.saturating_add(1));
        let mut events = inner
            .events
            .get(scope)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|event| event.sequence >= after)
            .collect::<Vec<_>>();
        events.sort_by_key(|event| event.sequence);
        if let Some(limit) = limit {
            events.truncate(limit);
        }
        Ok(events)
    }

    async fn subscribe(
        &self,
        scope: ReplayScope,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<ReplaySubscription> {
        if let Some(cursor) = cursor.as_ref() {
            cursor.validate(ReplayCursorFamily::ReplayEvent, &scope)?;
        }
        let baseline_initialized = cursor.is_some();
        let mut subscription = ReplaySubscription {
            receiver: self.sender.subscribe(),
            scope: scope.clone(),
            cursor: cursor.clone(),
            backlog: VecDeque::new(),
            catchup: Some(Arc::new(self.clone())),
            baseline_initialized,
        };
        let backlog = <Self as ReplayEventLog>::replay_after(self, &scope, cursor, None).await?;
        if !backlog.is_empty() {
            subscription.initialize_backlog(backlog);
        }
        Ok(subscription)
    }

    async fn compact_snapshot(&self, scope: &ReplayScope) -> ReplayResult<ReplaySnapshot> {
        let inner = self.inner.lock().map_err(failed)?;
        if let Some(snapshot) = inner.snapshots.get(scope) {
            return Ok(snapshot.clone());
        }
        let events = inner.events.get(scope).cloned().unwrap_or_default();
        let display_messages = events
            .iter()
            .filter_map(|event| match &event.event {
                ReplayEventKind::DisplayMessage(message) => Some((**message).clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let cursor = events
            .last()
            .map(|event| ReplayCursor::replay_event(scope.clone(), event.sequence));
        Ok(ReplaySnapshot {
            scope: Some(scope.clone()),
            revision: events.len(),
            cursor,
            display_messages,
            metadata: Metadata::default(),
        })
    }
}

#[async_trait]
impl ReplayCatchupSource for InMemoryReplayEventLog {
    async fn catch_up_after(
        &self,
        scope: &ReplayScope,
        cursor: Option<ReplayCursor>,
        limit: Option<usize>,
    ) -> ReplayResult<Vec<ReplayEvent>> {
        <Self as ReplayEventLog>::replay_after(self, scope, cursor, limit).await
    }
}

impl InMemoryReplayEventLog {
    /// Save a compact snapshot for later replay views.
    ///
    /// # Errors
    ///
    /// Returns an error when the in-memory lock is poisoned.
    pub fn save_snapshot(&self, scope: ReplayScope, snapshot: ReplaySnapshot) -> ReplayResult<()> {
        snapshot.validate(ReplayCursorFamily::ReplayEvent, &scope)?;
        let mut inner = self.inner.lock().map_err(failed)?;
        inner.snapshots.insert(scope, snapshot);
        Ok(())
    }
}
