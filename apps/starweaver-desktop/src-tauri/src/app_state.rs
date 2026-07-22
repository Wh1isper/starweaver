use std::{
    collections::{BTreeMap, VecDeque},
    io::Write as _,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use serde::Serialize;
use tauri::ipc::Channel;
use tokio::sync::{Mutex as AsyncMutex, oneshot, watch};

use crate::{
    generated::host::{
        DesktopHostEventAcknowledgementToken, DesktopHostEventDelivery, DesktopHostEventScope,
    },
    supervisor::{BackendHostEvent, LocalHostSupervisor, SupervisorError},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopActivation {
    pub kind: ActivationKind,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationKind {
    SecondaryLaunch,
}

/// Process-owned state that survives renderer reloads and window recreation.
const MAX_ACTIVATION_SUBSCRIPTIONS: usize = 16;
const MAX_EVENT_CURSOR_VIEWS: usize = 256;
const MAX_RECENT_EVENT_IDS: usize = 1_024;
const MAX_PENDING_EVENT_ACKNOWLEDGEMENTS: usize = 32;

struct HostSubscriptionControl {
    key: EventViewKey,
    cancel: watch::Sender<bool>,
    completed: watch::Receiver<bool>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EventViewKey {
    pub(crate) execution_domain: String,
    session_id: String,
    run_id: String,
}

#[derive(Clone, Debug, serde::Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct EventCursorRecord {
    key: EventViewKey,
    cursor: String,
    recent_event_ids: VecDeque<String>,
}

#[derive(serde::Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct EventCursorSnapshot {
    schema_version: u32,
    records: Vec<EventCursorRecord>,
}

struct PendingEventAcknowledgement {
    key: EventViewKey,
    cursor: String,
    event_id: String,
    completed: oneshot::Sender<()>,
}

pub struct DesktopState {
    launch_generation: AtomicU64,
    exit_shutdown_started: AtomicBool,
    exit_shutdown_completed: AtomicBool,
    next_subscription_token: AtomicU64,
    activation_subscriptions: Mutex<BTreeMap<u64, Channel<DesktopActivation>>>,
    host_subscriptions: Mutex<BTreeMap<String, HostSubscriptionControl>>,
    event_storage_root: Mutex<Option<PathBuf>>,
    event_cursors: Mutex<BTreeMap<EventViewKey, EventCursorRecord>>,
    pending_event_acknowledgements: Mutex<BTreeMap<String, PendingEventAcknowledgement>>,
    event_cursor_gate: AsyncMutex<()>,
    supervisor: LocalHostSupervisor,
}

impl Default for DesktopState {
    fn default() -> Self {
        Self {
            launch_generation: AtomicU64::new(1),
            exit_shutdown_started: AtomicBool::new(false),
            exit_shutdown_completed: AtomicBool::new(false),
            next_subscription_token: AtomicU64::new(1),
            activation_subscriptions: Mutex::new(BTreeMap::new()),
            host_subscriptions: Mutex::new(BTreeMap::new()),
            event_storage_root: Mutex::new(None),
            event_cursors: Mutex::new(BTreeMap::new()),
            pending_event_acknowledgements: Mutex::new(BTreeMap::new()),
            event_cursor_gate: AsyncMutex::new(()),
            supervisor: LocalHostSupervisor::default(),
        }
    }
}

impl DesktopState {
    pub fn launch_generation(&self) -> u64 {
        self.launch_generation.load(Ordering::Acquire)
    }

    pub fn begin_exit_shutdown(&self) -> bool {
        self.exit_shutdown_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub fn complete_exit_shutdown(&self) {
        self.exit_shutdown_completed.store(true, Ordering::Release);
    }

    pub fn exit_shutdown_completed(&self) -> bool {
        self.exit_shutdown_completed.load(Ordering::Acquire)
    }

    pub fn record_secondary_launch(&self) -> DesktopActivation {
        DesktopActivation {
            kind: ActivationKind::SecondaryLaunch,
            generation: self.launch_generation.fetch_add(1, Ordering::AcqRel) + 1,
        }
    }

    pub fn subscribe_to_activations(&self, channel: Channel<DesktopActivation>) -> u64 {
        let token = self.next_subscription_token.fetch_add(1, Ordering::AcqRel);
        let mut subscriptions = self
            .activation_subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while subscriptions.len() >= MAX_ACTIVATION_SUBSCRIPTIONS {
            subscriptions.pop_first();
        }
        subscriptions.insert(token, channel);
        token
    }

    pub fn unsubscribe_from_activations(&self, token: u64) {
        self.activation_subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&token);
    }

    pub const fn supervisor(&self) -> &LocalHostSupervisor {
        &self.supervisor
    }

    pub fn configure_supervisor_storage(&self, root: PathBuf) -> Result<(), SupervisorError> {
        let cursors = load_event_cursors(&root)?;
        self.supervisor.configure_storage_root(root.clone())?;
        *self
            .event_cursors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = cursors;
        *self
            .event_storage_root
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(root);
        Ok(())
    }

    pub fn event_view_key(execution_domain: String, scope: &DesktopHostEventScope) -> EventViewKey {
        EventViewKey {
            execution_domain,
            session_id: scope.session_id.0.clone(),
            run_id: scope.run_id.0.clone(),
        }
    }

    pub fn acknowledged_event_cursor(&self, key: &EventViewKey) -> Option<String> {
        self.event_cursors
            .lock()
            .ok()
            .and_then(|records| records.get(key).map(|record| record.cursor.clone()))
    }

    pub fn event_was_acknowledged(&self, key: &EventViewKey, event_id: &str) -> bool {
        self.event_cursors.lock().is_ok_and(|records| {
            records.get(key).is_some_and(|record| {
                record
                    .recent_event_ids
                    .iter()
                    .any(|value| value == event_id)
            })
        })
    }

    pub fn prepare_event_acknowledgement(
        &self,
        key: EventViewKey,
        event: BackendHostEvent,
    ) -> Result<(DesktopHostEventDelivery, oneshot::Receiver<()>), SupervisorError> {
        let token = format!("desktop-event-ack-v1-{}", uuid::Uuid::new_v4());
        let (completed, acknowledged) = oneshot::channel();
        let mut pending = self
            .pending_event_acknowledgements
            .lock()
            .map_err(|_| SupervisorError::transport())?;
        if pending.len() >= MAX_PENDING_EVENT_ACKNOWLEDGEMENTS
            || pending.values().any(|entry| entry.key == key)
        {
            return Err(SupervisorError::not_ready());
        }
        pending.insert(
            token.clone(),
            PendingEventAcknowledgement {
                key,
                cursor: event.cursor,
                event_id: event.event_id,
                completed,
            },
        );
        drop(pending);
        Ok((
            DesktopHostEventDelivery {
                acknowledgement_token: DesktopHostEventAcknowledgementToken(token),
                event: event.event,
            },
            acknowledged,
        ))
    }

    pub fn cancel_event_acknowledgement(&self, token: &str) {
        if let Ok(mut pending) = self.pending_event_acknowledgements.lock() {
            pending.remove(token);
        }
    }

    pub async fn acknowledge_event(&self, token: &str) -> Result<(), SupervisorError> {
        let (key, cursor, event_id) = {
            let pending = self
                .pending_event_acknowledgements
                .lock()
                .map_err(|_| SupervisorError::transport())?;
            let entry = pending.get(token).ok_or_else(|| {
                SupervisorError::invalid_configuration(
                    "event acknowledgement token is invalid or expired",
                )
            })?;
            let values = (
                entry.key.clone(),
                entry.cursor.clone(),
                entry.event_id.clone(),
            );
            drop(pending);
            values
        };
        self.persist_event_cursor(key, cursor, event_id).await?;
        let completed = self
            .pending_event_acknowledgements
            .lock()
            .map_err(|_| SupervisorError::transport())?
            .remove(token)
            .ok_or_else(SupervisorError::transport)?
            .completed;
        let _ = completed.send(());
        Ok(())
    }

    pub async fn advance_acknowledged_duplicate(
        &self,
        key: EventViewKey,
        event: &BackendHostEvent,
    ) -> Result<(), SupervisorError> {
        self.persist_event_cursor(key, event.cursor.clone(), event.event_id.clone())
            .await
    }

    async fn persist_event_cursor(
        &self,
        key: EventViewKey,
        cursor: String,
        event_id: String,
    ) -> Result<(), SupervisorError> {
        let _gate = self.event_cursor_gate.lock().await;
        let root = self
            .event_storage_root
            .lock()
            .map_err(|_| SupervisorError::transport())?
            .clone()
            .ok_or_else(SupervisorError::not_ready)?;
        let mut next = self
            .event_cursors
            .lock()
            .map_err(|_| SupervisorError::transport())?
            .clone();
        if !next.contains_key(&key) && next.len() >= MAX_EVENT_CURSOR_VIEWS {
            let mut protected = self
                .host_subscriptions
                .lock()
                .map_err(|_| SupervisorError::transport())?
                .values()
                .map(|control| control.key.clone())
                .collect::<std::collections::BTreeSet<_>>();
            protected.extend(
                self.pending_event_acknowledgements
                    .lock()
                    .map_err(|_| SupervisorError::transport())?
                    .values()
                    .map(|entry| entry.key.clone()),
            );
            let evicted = next
                .keys()
                .find(|candidate| !protected.contains(*candidate))
                .cloned()
                .ok_or_else(SupervisorError::not_ready)?;
            next.remove(&evicted);
        }
        let record = next
            .entry(key.clone())
            .or_insert_with(|| EventCursorRecord {
                key,
                cursor: cursor.clone(),
                recent_event_ids: VecDeque::new(),
            });
        record.cursor = cursor;
        record.recent_event_ids.retain(|value| value != &event_id);
        record.recent_event_ids.push_back(event_id);
        while record.recent_event_ids.len() > MAX_RECENT_EVENT_IDS {
            record.recent_event_ids.pop_front();
        }
        let persisted = tokio::task::spawn_blocking({
            let snapshot = next.clone();
            move || persist_event_cursors(&root, &snapshot)
        })
        .await
        .map_err(|_| SupervisorError::transport())?;
        persisted?;
        *self
            .event_cursors
            .lock()
            .map_err(|_| SupervisorError::transport())? = next;
        Ok(())
    }

    pub fn register_host_subscription(
        &self,
        key: EventViewKey,
    ) -> Result<(String, watch::Receiver<bool>, watch::Sender<bool>), SupervisorError> {
        let token = format!(
            "desktop-host-subscription-{}",
            self.next_subscription_token.fetch_add(1, Ordering::AcqRel)
        );
        let (cancel, cancelled) = watch::channel(false);
        let (completion, completed) = watch::channel(false);
        let mut subscriptions = self
            .host_subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        subscriptions.retain(|_, control| {
            !*control.completed.borrow() && control.completed.has_changed().is_ok()
        });
        if subscriptions.len() >= MAX_ACTIVATION_SUBSCRIPTIONS
            || subscriptions.values().any(|control| control.key == key)
        {
            return Err(SupervisorError::not_ready());
        }
        subscriptions.insert(
            token.clone(),
            HostSubscriptionControl {
                key,
                cancel,
                completed,
            },
        );
        drop(subscriptions);
        Ok((token, cancelled, completion))
    }

    pub async fn replace_host_subscription(
        &self,
        key: EventViewKey,
    ) -> Result<(String, watch::Receiver<bool>, watch::Sender<bool>), SupervisorError> {
        loop {
            let existing = {
                let mut subscriptions = self
                    .host_subscriptions
                    .lock()
                    .map_err(|_| SupervisorError::transport())?;
                subscriptions.retain(|_, control| {
                    !*control.completed.borrow() && control.completed.has_changed().is_ok()
                });
                subscriptions
                    .values_mut()
                    .find(|control| control.key == key)
                    .map(|control| {
                        let _ = control.cancel.send(true);
                        control.completed.clone()
                    })
            };
            let Some(mut completed) = existing else {
                return self.register_host_subscription(key);
            };
            if !*completed.borrow() {
                // A panicked/aborted owner drops the completion sender. Either outcome loops so
                // the closed/completed control is removed before admitting the replacement.
                let _ = completed.changed().await;
            }
        }
    }

    pub fn begin_host_unsubscribe(&self, token: &str) -> Option<watch::Receiver<bool>> {
        let mut subscriptions = self
            .host_subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let control = subscriptions.get_mut(token)?;
        let _ = control.cancel.send(true);
        let completed = control.completed.clone();
        drop(subscriptions);
        Some(completed)
    }

    pub fn complete_host_subscription(&self, token: &str) {
        if let Some(control) = self
            .host_subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(token)
        {
            let _ = control.cancel.send(true);
        }
    }

    pub fn publish_activation(&self, activation: DesktopActivation) {
        let mut subscriptions = self
            .activation_subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = subscriptions.len();
        subscriptions.retain(|_, channel| channel.send(activation).is_ok());
        let failed = before - subscriptions.len();
        drop(subscriptions);
        if failed > 0 {
            eprintln!("failed to notify {failed} renderer activation subscription(s)");
        }
    }
}

fn event_cursor_path(root: &Path) -> PathBuf {
    root.join("event-cursors-v1.json")
}

fn load_event_cursors(
    root: &Path,
) -> Result<BTreeMap<EventViewKey, EventCursorRecord>, SupervisorError> {
    let path = event_cursor_path(root);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(_) => return Err(SupervisorError::transport()),
    };
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(SupervisorError::invalid_configuration(
            "event cursor state is invalid",
        ));
    }
    let snapshot: EventCursorSnapshot = serde_json::from_slice(&bytes)
        .map_err(|_| SupervisorError::invalid_configuration("event cursor state is invalid"))?;
    if snapshot.schema_version != 1 || snapshot.records.len() > MAX_EVENT_CURSOR_VIEWS {
        return Err(SupervisorError::invalid_configuration(
            "event cursor state is invalid",
        ));
    }
    let mut records = BTreeMap::new();
    for record in snapshot.records {
        if record.cursor.is_empty()
            || record.key.execution_domain.is_empty()
            || record.key.session_id.is_empty()
            || record.key.run_id.is_empty()
            || record.recent_event_ids.len() > MAX_RECENT_EVENT_IDS
            || records.insert(record.key.clone(), record).is_some()
        {
            return Err(SupervisorError::invalid_configuration(
                "event cursor state is invalid",
            ));
        }
    }
    Ok(records)
}

fn persist_event_cursors(
    root: &Path,
    records: &BTreeMap<EventViewKey, EventCursorRecord>,
) -> Result<(), SupervisorError> {
    let snapshot = EventCursorSnapshot {
        schema_version: 1,
        records: records.values().cloned().collect(),
    };
    let bytes = serde_json::to_vec(&snapshot).map_err(|_| SupervisorError::transport())?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(root).map_err(|_| SupervisorError::transport())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| SupervisorError::transport())?;
    }
    temporary
        .write_all(&bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|_| SupervisorError::transport())?;
    temporary
        .persist(event_cursor_path(root))
        .map_err(|_| SupervisorError::transport())?;
    #[cfg(unix)]
    std::fs::File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| SupervisorError::transport())?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn secondary_launch_advances_generation() {
        let state = DesktopState::default();

        assert_eq!(state.launch_generation(), 1);
        assert_eq!(
            state.record_secondary_launch(),
            DesktopActivation {
                kind: ActivationKind::SecondaryLaunch,
                generation: 2,
            }
        );
        assert_eq!(state.launch_generation(), 2);
    }

    #[tokio::test]
    async fn event_acknowledgement_persists_cursor_across_restart() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let state = DesktopState::default();
        state
            .configure_supervisor_storage(temp.path().to_path_buf())
            .expect("configure storage");
        let scope: DesktopHostEventScope = serde_json::from_value(serde_json::json!({
            "sessionId": "session-test",
            "runId": "run-test"
        }))
        .expect("event scope");
        let key = DesktopState::event_view_key("domain-test".to_string(), &scope);
        let event = BackendHostEvent {
            event: crate::generated::host::SafeHostEvent {
                delivery: serde_json::json!({"record": {"eventId": "event-test"}}),
            },
            cursor: "cursor-test".to_string(),
            event_id: "event-test".to_string(),
        };
        let (delivery, acknowledged) = state
            .prepare_event_acknowledgement(key.clone(), event)
            .expect("prepare acknowledgement");
        state
            .acknowledge_event(&delivery.acknowledgement_token.0)
            .await
            .expect("acknowledge event");
        acknowledged.await.expect("acknowledgement barrier");
        assert_eq!(
            state.acknowledged_event_cursor(&key).as_deref(),
            Some("cursor-test")
        );
        assert!(state.event_was_acknowledged(&key, "event-test"));

        let restarted = DesktopState::default();
        restarted
            .configure_supervisor_storage(temp.path().to_path_buf())
            .expect("reload storage");
        assert_eq!(
            restarted.acknowledged_event_cursor(&key).as_deref(),
            Some("cursor-test")
        );
        assert!(restarted.event_was_acknowledged(&key, "event-test"));
    }

    #[tokio::test]
    async fn cursor_capacity_evicts_inactive_views_and_reloads_from_origin() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let state = DesktopState::default();
        state
            .configure_supervisor_storage(temp.path().to_path_buf())
            .expect("configure storage");

        let mut keys = Vec::new();
        for index in 0..=MAX_EVENT_CURSOR_VIEWS {
            let key = EventViewKey {
                execution_domain: "domain".to_string(),
                session_id: format!("session-{index:04}"),
                run_id: "run".to_string(),
            };
            state
                .persist_event_cursor(
                    key.clone(),
                    format!("cursor-{index}"),
                    format!("event-{index}"),
                )
                .await
                .expect("persist cursor");
            keys.push(key);
        }

        assert_eq!(
            state
                .event_cursors
                .lock()
                .expect("event cursor state")
                .len(),
            MAX_EVENT_CURSOR_VIEWS
        );
        assert_eq!(state.acknowledged_event_cursor(&keys[0]), None);
        let newest_cursor = format!("cursor-{MAX_EVENT_CURSOR_VIEWS}");
        assert_eq!(
            state
                .acknowledged_event_cursor(keys.last().expect("newest key"))
                .as_deref(),
            Some(newest_cursor.as_str())
        );

        let restarted = DesktopState::default();
        restarted
            .configure_supervisor_storage(temp.path().to_path_buf())
            .expect("reload storage");
        assert_eq!(restarted.acknowledged_event_cursor(&keys[0]), None);
        assert!(
            restarted
                .acknowledged_event_cursor(keys.last().expect("newest key"))
                .is_some()
        );
    }

    #[tokio::test]
    async fn renderer_reload_replaces_same_view_after_old_tail_barrier() {
        let state = DesktopState::default();
        let key = EventViewKey {
            execution_domain: "domain".to_string(),
            session_id: "session".to_string(),
            run_id: "run".to_string(),
        };
        let (old_token, mut old_cancelled, old_completion) = state
            .register_host_subscription(key.clone())
            .expect("register old renderer");
        let ((), replacement) = tokio::join!(
            async {
                old_cancelled.changed().await.expect("replacement cancel");
                assert!(*old_cancelled.borrow());
                old_completion.send(true).expect("old tail completion");
            },
            state.replace_host_subscription(key)
        );
        let (new_token, _, _) = replacement.expect("register replacement renderer");
        assert_ne!(old_token, new_token);
    }

    #[tokio::test]
    async fn duplicate_unsubscribe_calls_share_completion_barrier() {
        let state = DesktopState::default();
        let key = EventViewKey {
            execution_domain: "domain".to_string(),
            session_id: "session".to_string(),
            run_id: "run".to_string(),
        };
        let (token, mut cancelled, completion) = state
            .register_host_subscription(key)
            .expect("register subscription");
        let mut first = state
            .begin_host_unsubscribe(&token)
            .expect("first unsubscribe");
        let mut second = state
            .begin_host_unsubscribe(&token)
            .expect("second unsubscribe");
        cancelled.changed().await.expect("cancellation signal");
        assert!(*cancelled.borrow());
        assert!(!*first.borrow());
        assert!(!*second.borrow());
        completion.send(true).expect("completion signal");
        first.changed().await.expect("first completion");
        second.changed().await.expect("second completion");
        assert!(*first.borrow());
        assert!(*second.borrow());
        state.complete_host_subscription(&token);
        let replacement_key = EventViewKey {
            execution_domain: "domain".to_string(),
            session_id: "session".to_string(),
            run_id: "run".to_string(),
        };
        assert!(state.register_host_subscription(replacement_key).is_ok());
    }
}
