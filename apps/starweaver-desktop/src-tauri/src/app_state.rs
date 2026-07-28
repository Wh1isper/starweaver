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
    supervisor::{
        BackendHostEvent, HostChildState, LocalHostSupervisor, LocalLaunchSpec, SupervisorError,
    },
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

pub struct RuntimeLaunchPlan {
    pub primary: LocalLaunchSpec,
    pub bundled_fallback: Option<LocalLaunchSpec>,
}

impl From<LocalLaunchSpec> for RuntimeLaunchPlan {
    fn from(primary: LocalLaunchSpec) -> Self {
        Self {
            primary,
            bundled_fallback: None,
        }
    }
}

struct HostSubscriptionControl {
    key: EventViewKey,
    cancel: watch::Sender<bool>,
    completed: watch::Receiver<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DesktopWindowRoute {
    Main,
    Conversation { session_id: String },
}

fn default_event_window_label() -> String {
    "main".to_string()
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EventViewKey {
    pub(crate) execution_domain: String,
    #[serde(default = "default_event_window_label")]
    window_label: String,
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
    conversation_window_gate: Mutex<()>,
    conversation_window_routes: Mutex<BTreeMap<String, String>>,
    event_storage_root: Mutex<Option<PathBuf>>,
    event_cursors: Mutex<BTreeMap<EventViewKey, EventCursorRecord>>,
    pending_event_acknowledgements: Mutex<BTreeMap<String, PendingEventAcknowledgement>>,
    runtime_issue: Mutex<Option<SupervisorError>>,
    runtime_start_gate: AsyncMutex<()>,
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
            conversation_window_gate: Mutex::new(()),
            conversation_window_routes: Mutex::new(BTreeMap::new()),
            event_storage_root: Mutex::new(None),
            event_cursors: Mutex::new(BTreeMap::new()),
            pending_event_acknowledgements: Mutex::new(BTreeMap::new()),
            runtime_issue: Mutex::new(None),
            runtime_start_gate: AsyncMutex::new(()),
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

    pub fn window_route(&self, window_label: &str) -> Option<DesktopWindowRoute> {
        if window_label == "main" {
            return Some(DesktopWindowRoute::Main);
        }
        self.conversation_window_routes
            .lock()
            .ok()
            .and_then(|routes| routes.get(window_label).cloned())
            .map(|session_id| DesktopWindowRoute::Conversation { session_id })
    }

    pub fn lock_conversation_windows(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, ()>, SupervisorError> {
        self.conversation_window_gate
            .lock()
            .map_err(|_| SupervisorError::transport())
    }

    pub fn reserve_conversation_window(
        &self,
        session_id: &str,
    ) -> Result<(String, bool), SupervisorError> {
        let mut routes = self
            .conversation_window_routes
            .lock()
            .map_err(|_| SupervisorError::transport())?;
        if let Some((label, _)) = routes
            .iter()
            .find(|(_, existing)| existing.as_str() == session_id)
        {
            return Ok((label.clone(), true));
        }
        if routes.len() >= MAX_ACTIVATION_SUBSCRIPTIONS {
            return Err(SupervisorError::not_ready());
        }
        let label = format!("conversation-{}", uuid::Uuid::new_v4().simple());
        routes.insert(label.clone(), session_id.to_string());
        drop(routes);
        Ok((label, false))
    }

    pub fn release_window(&self, window_label: &str) {
        if window_label != "main"
            && let Ok(mut routes) = self.conversation_window_routes.lock()
        {
            routes.remove(window_label);
        }
        if let Ok(mut subscriptions) = self.host_subscriptions.lock() {
            for control in subscriptions
                .values_mut()
                .filter(|control| control.key.window_label == window_label)
            {
                let _ = control.cancel.send(true);
            }
        }
    }

    pub(crate) fn active_runtime_identity(
        &self,
    ) -> Option<crate::supervisor::ReadyRuntimeIdentity> {
        self.supervisor.ready_runtime_identity()
    }

    pub fn runtime_issue(&self) -> Option<SupervisorError> {
        self.runtime_issue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn record_runtime_issue(&self, issue: SupervisorError) {
        *self
            .runtime_issue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(issue);
    }

    pub async fn prepare_and_start_managed_runtime<F, P>(
        &self,
        prepare: F,
    ) -> Result<(), SupervisorError>
    where
        F: FnOnce() -> Result<P, SupervisorError> + Send + 'static,
        P: Into<RuntimeLaunchPlan> + Send + 'static,
    {
        let _attempt = self.runtime_start_gate.lock().await;
        if self.exit_shutdown_started.load(Ordering::Acquire) {
            return Err(SupervisorError::not_ready());
        }
        match self.supervisor.status().state {
            HostChildState::Unconfigured | HostChildState::Stopped | HostChildState::Failed => {}
            HostChildState::Starting
            | HostChildState::Handshaking
            | HostChildState::Ready
            | HostChildState::Draining
            | HostChildState::Recovering => return Ok(()),
            HostChildState::Incompatible => return Err(SupervisorError::not_ready()),
        }
        *self
            .runtime_issue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        let result = match tokio::task::spawn_blocking(prepare).await {
            Ok(Ok(_)) if self.exit_shutdown_started.load(Ordering::Acquire) => {
                return Err(SupervisorError::not_ready());
            }
            Ok(Ok(plan)) => {
                let plan = plan.into();
                match self.supervisor.start(plan.primary).await {
                    Ok(()) => Ok(()),
                    Err(primary_issue) => match plan.bundled_fallback {
                        Some(fallback) if !self.exit_shutdown_started.load(Ordering::Acquire) => {
                            self.supervisor
                                .start_bundled_fallback_after_failure(fallback)
                                .await
                        }
                        _ => Err(primary_issue),
                    },
                }
            }
            Ok(Err(issue)) => Err(issue),
            Err(_) => Err(SupervisorError::transport()),
        };
        if let Err(issue) = &result {
            self.record_runtime_issue(issue.clone());
        }
        result
    }

    pub async fn shutdown_managed_runtime(&self) -> Result<(), SupervisorError> {
        let _ = self.supervisor.shutdown().await;
        let _attempt = self.runtime_start_gate.lock().await;
        self.supervisor.shutdown().await
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

    pub fn event_view_key(
        execution_domain: String,
        window_label: String,
        scope: &DesktopHostEventScope,
    ) -> EventViewKey {
        EventViewKey {
            execution_domain,
            window_label,
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

    pub async fn acknowledge_event(
        &self,
        window_label: &str,
        token: &str,
    ) -> Result<(), SupervisorError> {
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
            if entry.key.window_label != window_label {
                return Err(SupervisorError::invalid_configuration(
                    "event acknowledgement is owned by another Desktop window",
                ));
            }
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

    pub async fn reset_event_cursor(&self, key: &EventViewKey) -> Result<(), SupervisorError> {
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
        if next.remove(key).is_none() {
            return Ok(());
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

    pub fn begin_host_unsubscribe(
        &self,
        window_label: &str,
        token: &str,
    ) -> Result<Option<watch::Receiver<bool>>, SupervisorError> {
        let mut subscriptions = self
            .host_subscriptions
            .lock()
            .map_err(|_| SupervisorError::transport())?;
        let Some(control) = subscriptions.get_mut(token) else {
            return Ok(None);
        };
        if control.key.window_label != window_label {
            return Err(SupervisorError::invalid_configuration(
                "host subscription is owned by another Desktop window",
            ));
        }
        let _ = control.cancel.send(true);
        let completed = control.completed.clone();
        drop(subscriptions);
        Ok(Some(completed))
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
    async fn managed_runtime_preparation_is_single_flight() {
        use std::{
            sync::{
                Arc,
                atomic::{AtomicUsize, Ordering as AtomicOrdering},
            },
            time::Duration,
        };

        let state = Arc::new(DesktopState::default());
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let launch = |message: &'static str,
                      state: Arc<DesktopState>,
                      active: Arc<AtomicUsize>,
                      maximum: Arc<AtomicUsize>| {
            tokio::spawn(async move {
                state
                    .prepare_and_start_managed_runtime(move || {
                        let current = active.fetch_add(1, AtomicOrdering::AcqRel) + 1;
                        maximum.fetch_max(current, AtomicOrdering::AcqRel);
                        std::thread::sleep(Duration::from_millis(25));
                        active.fetch_sub(1, AtomicOrdering::AcqRel);
                        Err::<LocalLaunchSpec, _>(SupervisorError::invalid_configuration(message))
                    })
                    .await
            })
        };
        let first = launch(
            "first preparation failed",
            state.clone(),
            active.clone(),
            maximum.clone(),
        );
        let second = launch(
            "second preparation failed",
            state.clone(),
            active,
            maximum.clone(),
        );

        let _ = first.await.expect("first task");
        let _ = second.await.expect("second task");
        assert_eq!(maximum.load(AtomicOrdering::Acquire), 1);
        assert!(state.runtime_issue().is_some());
    }

    #[tokio::test]
    async fn failed_managed_start_attempts_the_bundled_fallback_as_a_new_generation() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let state = DesktopState::default();
        state
            .configure_supervisor_storage(temp.path().join("supervisor"))
            .expect("configure supervisor storage");
        let invalid_spec = |name: &str, version: &str| LocalLaunchSpec {
            runtime_path: temp.path().join(name),
            runtime_digest: format!("sha256:{}", "a".repeat(64)),
            runtime_size: 1,
            runtime_source: crate::supervisor::RuntimeLaunchSource::Managed,
            runtime_version: version.to_string(),
            build_revision: "0123456789abcdef0123456789abcdef01234567".to_string(),
            target: env!("STARWEAVER_TARGET_TRIPLE").to_string(),
            launch_envelope_path: temp.path().join(format!("{name}.json")),
            launch_envelope_digest: format!("sha256:{}", "b".repeat(64)),
            configuration_generation: 1,
            execution_domain_id: "local-default".to_string(),
        };
        let plan = RuntimeLaunchPlan {
            primary: invalid_spec("missing-managed-runtime", "1.2.4"),
            bundled_fallback: Some(invalid_spec("missing-bundled-runtime", "1.2.3")),
        };

        let error = state
            .prepare_and_start_managed_runtime(move || Ok(plan))
            .await
            .expect_err("both invalid launch choices must fail");

        assert_eq!(
            error.code,
            crate::supervisor::SupervisorErrorCode::InvalidConfiguration
        );
        assert_eq!(state.supervisor().status().generation, 2);
        assert_eq!(state.supervisor().status().state, HostChildState::Failed);
        assert_eq!(state.runtime_issue(), Some(error));
    }

    #[tokio::test]
    async fn exit_waits_for_preparation_without_starting_a_child_after_shutdown() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool as TestAtomicBool, Ordering as AtomicOrdering},
        };

        let state = Arc::new(DesktopState::default());
        let started = Arc::new(TestAtomicBool::new(false));
        let release = Arc::new(TestAtomicBool::new(false));
        let attempt_state = state.clone();
        let attempt_started = started.clone();
        let attempt_release = release.clone();
        let attempt = tokio::spawn(async move {
            attempt_state
                .prepare_and_start_managed_runtime(move || {
                    attempt_started.store(true, AtomicOrdering::Release);
                    while !attempt_release.load(AtomicOrdering::Acquire) {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Ok(LocalLaunchSpec {
                        runtime_path: PathBuf::from("/unused/runtime"),
                        runtime_digest: "sha256:unused".to_string(),
                        runtime_size: 1,
                        runtime_source: crate::supervisor::RuntimeLaunchSource::Bundled,
                        runtime_version: "unused".to_string(),
                        build_revision: "unused".to_string(),
                        target: "unused".to_string(),
                        launch_envelope_path: PathBuf::from("/unused/launch.json"),
                        launch_envelope_digest: "sha256:unused".to_string(),
                        configuration_generation: 1,
                        execution_domain_id: "unused".to_string(),
                    })
                })
                .await
        });
        while !started.load(AtomicOrdering::Acquire) {
            tokio::task::yield_now().await;
        }

        assert!(state.begin_exit_shutdown());
        let shutdown_state = state.clone();
        let shutdown = tokio::spawn(async move { shutdown_state.shutdown_managed_runtime().await });
        release.store(true, AtomicOrdering::Release);

        assert!(attempt.await.expect("startup task").is_err());
        shutdown
            .await
            .expect("shutdown task")
            .expect("shutdown barrier");
        assert_eq!(state.supervisor().status().generation, 0);
        assert!(state.runtime_issue().is_none());
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
        let key =
            DesktopState::event_view_key("domain-test".to_string(), "main".to_string(), &scope);
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
            .acknowledge_event("main", &delivery.acknowledgement_token.0)
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
    async fn fresh_renderer_subscription_resets_persisted_cursor_to_origin() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let state = DesktopState::default();
        state
            .configure_supervisor_storage(temp.path().to_path_buf())
            .expect("configure storage");
        let key = EventViewKey {
            execution_domain: "domain".to_string(),
            window_label: "main".to_string(),
            session_id: "session".to_string(),
            run_id: "run".to_string(),
        };
        state
            .persist_event_cursor(
                key.clone(),
                "cursor-before-reload".to_string(),
                "event-before-reload".to_string(),
            )
            .await
            .expect("persist cursor");

        state
            .reset_event_cursor(&key)
            .await
            .expect("reset renderer projection cursor");

        assert_eq!(state.acknowledged_event_cursor(&key), None);
        assert!(!state.event_was_acknowledged(&key, "event-before-reload"));
        let restarted = DesktopState::default();
        restarted
            .configure_supervisor_storage(temp.path().to_path_buf())
            .expect("reload storage");
        assert_eq!(restarted.acknowledged_event_cursor(&key), None);
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
                window_label: "main".to_string(),
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
            window_label: "main".to_string(),
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
            window_label: "main".to_string(),
            session_id: "session".to_string(),
            run_id: "run".to_string(),
        };
        let (token, mut cancelled, completion) = state
            .register_host_subscription(key)
            .expect("register subscription");
        let mut first = state
            .begin_host_unsubscribe("main", &token)
            .expect("unsubscribe state")
            .expect("first unsubscribe");
        let mut second = state
            .begin_host_unsubscribe("main", &token)
            .expect("unsubscribe state")
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
            window_label: "main".to_string(),
            session_id: "session".to_string(),
            run_id: "run".to_string(),
        };
        assert!(state.register_host_subscription(replacement_key).is_ok());
    }

    #[tokio::test]
    async fn same_run_subscriptions_are_owned_per_window() {
        let state = DesktopState::default();
        let main_key = EventViewKey {
            execution_domain: "domain".to_string(),
            window_label: "main".to_string(),
            session_id: "session".to_string(),
            run_id: "run".to_string(),
        };
        let conversation_key = EventViewKey {
            window_label: "conversation-one".to_string(),
            ..main_key.clone()
        };
        let (_, main_cancelled, _main_completion) = state
            .register_host_subscription(main_key)
            .expect("main subscription");
        let (_, mut old_conversation_cancelled, old_conversation_completion) = state
            .register_host_subscription(conversation_key.clone())
            .expect("conversation subscription");

        let ((), replacement) = tokio::join!(
            async {
                old_conversation_cancelled
                    .changed()
                    .await
                    .expect("conversation replacement cancel");
                old_conversation_completion
                    .send(true)
                    .expect("conversation completion");
            },
            state.replace_host_subscription(conversation_key)
        );

        assert!(replacement.is_ok());
        assert!(!*main_cancelled.borrow());
        assert!(
            !main_cancelled
                .has_changed()
                .expect("main cancellation state")
        );
    }

    #[test]
    fn conversation_window_route_is_reused_and_released_by_session() {
        let state = DesktopState::default();
        let (label, reused) = state
            .reserve_conversation_window("session-one")
            .expect("reserve conversation window");
        assert!(!reused);
        assert_eq!(
            state.window_route(&label),
            Some(DesktopWindowRoute::Conversation {
                session_id: "session-one".to_string()
            })
        );
        assert_eq!(
            state
                .reserve_conversation_window("session-one")
                .expect("reuse conversation window"),
            (label.clone(), true)
        );

        state.release_window(&label);
        assert_eq!(state.window_route(&label), None);
        let (replacement, reused) = state
            .reserve_conversation_window("session-one")
            .expect("reserve replacement window");
        assert!(!reused);
        assert_ne!(replacement, label);
    }
}
