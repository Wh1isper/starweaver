//! Standalone RPC method dispatch.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};
use starweaver_core::{ProtocolIdentity, RunId, SessionId};
use starweaver_rpc_core::{
    DiagnosticLevel, DiagnosticNotificationParams, EnvironmentAttachmentRef, HostInitializeParams,
    HostNotificationKind, HostRunStatus, INVALID_PARAMS, JsonRpcOutcome, METHOD_NOT_FOUND,
    NOT_INITIALIZED, ProfileConfig, ProfileGetResult, RpcError, RunInput, RunResumeParams,
    RunResumeResult, SessionSearchFeatureCapabilities, SessionSearchParams, SessionSearchResult,
    StorageImportLegacyParams, StorageImportLegacyResult, StreamEventParams, StreamPayloadFormat,
    SubscriptionClosedParams, SubscriptionClosedReason, SubscriptionReadyParams,
    UNSUPPORTED_FEATURE, attachment_result, error_response, handle_json_rpc_text_async,
    host_protocol_identity_with_session_search, output_item, replay_cursor_from_params,
    replay_result, stream_payload_format, typed_notification, validate_host_initialize,
};
use starweaver_runtime::AgentInput;
use starweaver_session::{
    ApprovalStatus, ExecutionStatus, InputPart, SessionFilter, SessionSearchError,
    SessionSearchProvider, SessionSearchScope, SessionStore, SessionStoreResult,
};
use starweaver_storage::{LocalSessionSearchLimits, LocalSessionSearchProvider, SqliteStorage};
use starweaver_stream::{ReplayCursor, ReplayScope};
use tokio::{
    runtime::Runtime,
    sync::{mpsc, watch},
};
use uuid::Uuid;

use crate::{
    RpcAgentCatalog, RpcConfig, RpcHitlResumeRequest, RpcHostError, RpcHostResult, RpcRunRequest,
    RpcRuntimeCoordinator, environment::effective_rpc_environment_attachments,
    environment_manager::EnvironmentAttachmentManager, session_management::command_fingerprint,
    state::RpcStateRepository,
};

const MAX_RUN_AWAIT: Duration = Duration::from_secs(30);
const DEFAULT_CLIENT_STATE_SCOPE: &str = "rpc";
const MAX_CONNECTION_SUBSCRIPTIONS: usize = 32;
const SUBSCRIPTION_REPLAY_PAGE: usize = 256;

async fn run_storage<T, F>(storage: SqliteStorage, operation: F) -> Result<T, RpcError>
where
    T: Send + 'static,
    F: FnOnce(SqliteStorage) -> SessionStoreResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(move || operation(storage))
        .await
        .map_err(|error| {
            rpc_error(RpcHostError::Runtime(format!(
                "storage task failed: {error}"
            )))
        })?
        .map_err(rpc_error)
}

/// Notification capability of the selected transport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RpcNotificationMode {
    /// Transport can emit out-of-band notifications.
    Live,
    /// Transport supports request/response replay only.
    ReplayOnly,
}

/// RPC-owned application service.
pub struct RpcService {
    config: RpcConfig,
    catalog: RpcAgentCatalog,
    storage: SqliteStorage,
    coordinator: RpcRuntimeCoordinator,
    environment_manager: EnvironmentAttachmentManager,
    session_search: Option<Arc<dyn SessionSearchProvider>>,
    session_search_scope: SessionSearchScope,
    notifications: RpcNotificationMode,
    state: RpcStateRepository,
    runtime: Arc<Runtime>,
}

/// Per-connection host protocol negotiation state.
pub struct RpcConnection<'a> {
    service: &'a RpcService,
    initialized: AtomicBool,
    connection_id: String,
    output: Option<mpsc::Sender<Value>>,
    subscriptions: Arc<Mutex<HashMap<String, ConnectionSubscription>>>,
}

struct ConnectionSubscription {
    session_id: SessionId,
    run_id: RunId,
    cancel: watch::Sender<bool>,
    ready: watch::Sender<bool>,
}

impl Drop for RpcConnection<'_> {
    fn drop(&mut self) {
        self.service
            .environment_manager
            .release_connection_leases(&self.connection_id);
        if let Ok(mut subscriptions) = self.subscriptions.lock() {
            for subscription in subscriptions.values() {
                let _ = subscription.cancel.send(true);
            }
            subscriptions.clear();
        }
    }
}

impl RpcConnection<'_> {
    /// Handle one frame using this connection's initialization state.
    #[must_use]
    pub fn handle_text(&self, text: &str) -> JsonRpcOutcome {
        self.service.runtime.block_on(self.handle_text_async(text))
    }

    async fn handle_text_async(&self, text: &str) -> JsonRpcOutcome {
        handle_json_rpc_text_async(text, |method, params| async move {
            if method == "initialize" {
                let result = self
                    .service
                    .initialize_result(&params, self.output.is_some());
                if result.is_ok() {
                    self.initialized.store(true, Ordering::Release);
                }
                return result;
            }
            if !self.initialized.load(Ordering::Acquire) {
                return Err(RpcError::new(
                    NOT_INITIALIZED,
                    "host protocol initialize must succeed before calling other methods",
                ));
            }
            match method.as_str() {
                "stream.subscribe" => self.stream_subscribe(&params).await,
                "stream.unsubscribe" => self.stream_unsubscribe(&params),
                _ => {
                    self.service
                        .dispatch(&method, &params, Some(&self.connection_id))
                        .await
                }
            }
        })
        .await
    }

    async fn stream_subscribe(&self, params: &Value) -> Result<Value, RpcError> {
        let Some(output) = self.output.clone() else {
            return Err(RpcError::new(
                UNSUPPORTED_FEATURE,
                "stream.subscribe requires a live notification transport",
            ));
        };
        let (session_id, run_id) = run_identity(params)?;
        let scope = ReplayScope::run(run_id.as_str());
        let cursor = replay_cursor_from_params(params, scope.clone())?;
        let limit = subscription_replay_limit(params)?;
        let format = stream_payload_format(params)?;
        let subscription_id = params
            .get("subscriptionId")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map_or_else(|| format!("sub_{}", Uuid::new_v4()), ToString::to_string);
        {
            let subscriptions = self
                .subscriptions
                .lock()
                .map_err(subscription_registry_error)?;
            validate_subscription_slot(&subscriptions, &subscription_id, &session_id, &run_id)?;
        }
        let events = self
            .service
            .coordinator
            .replay(&session_id, &run_id, cursor.clone(), Some(limit))
            .await
            .map_err(rpc_error)?;
        let status = self
            .service
            .coordinator
            .status(&session_id, &run_id)
            .await
            .map_err(rpc_error)?;
        let live_cursor = events.last().map_or_else(
            || cursor.clone(),
            |event| Some(ReplayCursor::replay_event(scope.clone(), event.sequence)),
        );
        let (cancel, cancel_receiver) = watch::channel(false);
        let (ready, ready_receiver) = watch::channel(false);
        {
            let mut subscriptions = self
                .subscriptions
                .lock()
                .map_err(subscription_registry_error)?;
            validate_subscription_slot(&subscriptions, &subscription_id, &session_id, &run_id)?;
            subscriptions.insert(
                subscription_id.clone(),
                ConnectionSubscription {
                    session_id: session_id.clone(),
                    run_id: run_id.clone(),
                    cancel,
                    ready,
                },
            );
        }
        self.spawn_subscription_tail(
            subscription_id.clone(),
            session_id.clone(),
            run_id.clone(),
            live_cursor,
            format,
            cancel_receiver,
            ready_receiver,
            output,
        );
        let mut result = attachment_result(
            session_id.as_str(),
            Some(run_id.as_str()),
            !status.terminal(),
            &events,
            format,
        );
        if let Some(object) = result.as_object_mut() {
            object.insert("subscriptionId".to_string(), Value::String(subscription_id));
        }
        Ok(result)
    }

    fn stream_unsubscribe(&self, params: &Value) -> Result<Value, RpcError> {
        if self.output.is_none() {
            return Err(RpcError::new(
                UNSUPPORTED_FEATURE,
                "stream.unsubscribe requires a live notification transport",
            ));
        }
        let subscription_id = required_string(params, "subscriptionId")?;
        let removed = self
            .subscriptions
            .lock()
            .map_err(|error| {
                RpcError::new(
                    starweaver_rpc_core::SERVER_ERROR,
                    format!("subscription registry poisoned: {error}"),
                )
            })?
            .remove(&subscription_id);
        if let Some(subscription) = removed.as_ref() {
            let _ = subscription.cancel.send(true);
        }
        Ok(json!({
            "subscriptionId": subscription_id,
            "closed": true,
            "wasActive": removed.is_some(),
        }))
    }

    /// Release notifications only after the corresponding JSON-RPC response was flushed.
    pub fn activate_pending_subscriptions(&self) {
        if let Ok(subscriptions) = self.subscriptions.lock() {
            for subscription in subscriptions.values() {
                let _ = subscription.ready.send(true);
            }
        }
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn spawn_subscription_tail(
        &self,
        subscription_id: String,
        session_id: SessionId,
        run_id: RunId,
        mut cursor: Option<ReplayCursor>,
        format: StreamPayloadFormat,
        mut cancel: watch::Receiver<bool>,
        mut ready: watch::Receiver<bool>,
        output: mpsc::Sender<Value>,
    ) {
        let coordinator = self.service.coordinator.clone();
        let subscriptions = Arc::clone(&self.subscriptions);
        self.service.runtime.spawn(async move {
            loop {
                if *cancel.borrow() {
                    return;
                }
                if *ready.borrow() {
                    break;
                }
                tokio::select! {
                    changed = ready.changed() => {
                        if changed.is_err() {
                            return;
                        }
                    }
                    changed = cancel.changed() => {
                        if changed.is_err() || *cancel.borrow() {
                            return;
                        }
                    }
                }
            }
            let scope = ReplayScope::run(run_id.as_str());
            if !send_subscription_frame(
                &output,
                &mut cancel,
                typed_notification(HostNotificationKind::SubscriptionReady(
                    SubscriptionReadyParams {
                        subscription_id: subscription_id.clone(),
                        scope: scope.clone(),
                        cursor: cursor.clone(),
                    },
                )),
            )
            .await
            {
                return;
            }
            let mut terminal_observed = false;
            let mut terminal = false;
            'tail: loop {
                if *cancel.borrow() {
                    break;
                }
                let event_count = match coordinator
                    .replay(
                        &session_id,
                        &run_id,
                        cursor.clone(),
                        Some(SUBSCRIPTION_REPLAY_PAGE),
                    )
                    .await
                {
                    Ok(events) => {
                        let event_count = events.len();
                        for event in events {
                            let event_cursor = ReplayCursor::replay_event(
                                ReplayScope::run(run_id.as_str()),
                                event.sequence,
                            );
                            cursor = Some(event_cursor.clone());
                            let Some(item) = output_item(&event, format) else {
                                continue;
                            };
                            if !send_subscription_frame(
                                &output,
                                &mut cancel,
                                typed_notification(HostNotificationKind::StreamEvent(Box::new(
                                    StreamEventParams {
                                        subscription_id: subscription_id.clone(),
                                        scope: event.scope,
                                        cursor: event_cursor,
                                        item,
                                    },
                                ))),
                            )
                            .await
                            {
                                break 'tail;
                            }
                        }
                        event_count
                    }
                    Err(error) => {
                        let _ = send_subscription_frame(
                            &output,
                            &mut cancel,
                            typed_notification(HostNotificationKind::Diagnostic(
                                DiagnosticNotificationParams {
                                    level: DiagnosticLevel::Error,
                                    message: error.to_string(),
                                    subscription_id: Some(subscription_id.clone()),
                                    code: Some("replay_failed".to_string()),
                                },
                            )),
                        )
                        .await;
                        break;
                    }
                };
                if let Ok(status) = coordinator.status(&session_id, &run_id).await
                    && status.terminal()
                {
                    // A durable terminal status can become visible just before the terminal event
                    // append. Require one additional empty replay page after observing terminal so
                    // every retained page, including the terminal marker, is drained first.
                    if terminal_observed && event_count == 0 {
                        terminal = send_subscription_frame(
                            &output,
                            &mut cancel,
                            typed_notification(HostNotificationKind::RunStatus(HostRunStatus {
                                session_id: SessionId::from_string(status.session_id.clone()),
                                run_id: RunId::from_string(status.run_id.clone()),
                                status: status.status,
                                output_preview: status.output_preview,
                                error: status.error,
                            })),
                        )
                        .await;
                        break;
                    }
                    terminal_observed = true;
                    if event_count == 0 {
                        tokio::select! {
                            () = tokio::time::sleep(Duration::from_millis(25)) => {}
                            changed = cancel.changed() => {
                                if changed.is_err() || *cancel.borrow() {
                                    break;
                                }
                            }
                        }
                    }
                    continue;
                }
                terminal_observed = false;
                if event_count == SUBSCRIPTION_REPLAY_PAGE {
                    continue;
                }
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_millis(25)) => {}
                    changed = cancel.changed() => {
                        if changed.is_err() || *cancel.borrow() {
                            break;
                        }
                    }
                }
            }
            if terminal {
                let _ = send_subscription_frame(
                    &output,
                    &mut cancel,
                    typed_notification(HostNotificationKind::SubscriptionClosed(
                        SubscriptionClosedParams {
                            subscription_id: subscription_id.clone(),
                            scope,
                            reason: SubscriptionClosedReason::Terminal,
                        },
                    )),
                )
                .await;
            }
            if let Ok(mut subscriptions) = subscriptions.lock() {
                subscriptions.remove(&subscription_id);
            }
        });
    }
}

#[allow(clippy::needless_pass_by_value)]
fn subscription_registry_error(
    error: std::sync::PoisonError<
        std::sync::MutexGuard<'_, HashMap<String, ConnectionSubscription>>,
    >,
) -> RpcError {
    RpcError::new(
        starweaver_rpc_core::SERVER_ERROR,
        format!("subscription registry poisoned: {error}"),
    )
}

fn validate_subscription_slot(
    subscriptions: &HashMap<String, ConnectionSubscription>,
    subscription_id: &str,
    session_id: &SessionId,
    run_id: &RunId,
) -> Result<(), RpcError> {
    if subscriptions.contains_key(subscription_id) {
        return Err(RpcError::new(
            starweaver_rpc_core::ALREADY_EXISTS,
            format!("subscription already exists: {subscription_id}"),
        ));
    }
    if subscriptions.values().any(|subscription| {
        &subscription.session_id == session_id && &subscription.run_id == run_id
    }) {
        return Err(RpcError::new(
            starweaver_rpc_core::ALREADY_EXISTS,
            format!(
                "connection already has a subscription for session {} run {}",
                session_id.as_str(),
                run_id.as_str()
            ),
        ));
    }
    if subscriptions.len() >= MAX_CONNECTION_SUBSCRIPTIONS {
        return Err(RpcError::new(
            starweaver_rpc_core::RUN_CONFLICT,
            format!("connection subscription limit reached ({MAX_CONNECTION_SUBSCRIPTIONS})"),
        ));
    }
    Ok(())
}

async fn send_subscription_frame(
    output: &mpsc::Sender<Value>,
    cancel: &mut watch::Receiver<bool>,
    frame: Value,
) -> bool {
    if *cancel.borrow() {
        return false;
    }
    tokio::select! {
        result = output.send(frame) => result.is_ok(),
        changed = cancel.changed() => changed.is_ok() && !*cancel.borrow(),
    }
}

impl RpcService {
    /// Construct a service for stdio/live transports.
    ///
    /// # Errors
    ///
    /// Returns storage or Tokio runtime initialization errors.
    pub fn live(config: RpcConfig) -> Result<Self, RpcHostError> {
        Self::new(config, RpcNotificationMode::Live)
    }

    /// Construct a service for unary replay-only transports.
    ///
    /// # Errors
    ///
    /// Returns storage or Tokio runtime initialization errors.
    pub fn replay_only(config: RpcConfig) -> Result<Self, RpcHostError> {
        Self::new(config, RpcNotificationMode::ReplayOnly)
    }

    fn new(config: RpcConfig, notifications: RpcNotificationMode) -> Result<Self, RpcHostError> {
        if let Some(parent) = config.database_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir_all(&config.workspace_root)?;
        let storage = SqliteStorage::open(&config.database_path)?;
        let catalog = RpcAgentCatalog::new(config.clone())?;
        let environment_manager = EnvironmentAttachmentManager::new();
        let coordinator = RpcRuntimeCoordinator::new(
            config.clone(),
            catalog.clone(),
            storage.clone(),
            environment_manager.clone(),
        );
        let session_search_scope =
            SessionSearchScope::local(config.database_path.to_string_lossy().into_owned());
        let session_search = if config.session_search.enabled {
            let limits = LocalSessionSearchLimits {
                max_query_bytes: config.session_search.max_query_bytes,
                max_page_size: config.session_search.max_page_size,
                max_display_files: config.session_search.max_display_files,
                max_total_display_bytes: config.session_search.max_total_display_bytes,
                max_display_hits: config.session_search.max_display_hits,
                max_scan_duration: Duration::from_millis(config.session_search.scan_timeout_ms),
                ..LocalSessionSearchLimits::default()
            };
            let mut provider = LocalSessionSearchProvider::new(
                Arc::new(storage.session_store()),
                &session_search_scope,
            )
            .with_limits(limits);
            if let Some(root) = config.session_search.display_root.as_ref() {
                provider = provider.with_display_root(root.clone());
            }
            Some(Arc::new(provider) as Arc<dyn SessionSearchProvider>)
        } else {
            None
        };
        let runtime = Runtime::new().map_err(|error| RpcHostError::Runtime(error.to_string()))?;
        runtime.block_on(coordinator.reconcile_startup())?;
        let state = RpcStateRepository::new(config.state_dir.clone());
        Ok(Self {
            config,
            catalog,
            storage,
            coordinator,
            environment_manager,
            session_search,
            session_search_scope,
            notifications,
            state,
            runtime: Arc::new(runtime),
        })
    }

    pub(crate) fn shutdown_owned_runtime(&self, timeout: Duration) -> RpcHostResult<()> {
        self.runtime.block_on(self.coordinator.shutdown(timeout))
    }

    /// Open an uninitialized stateful protocol connection.
    #[must_use]
    pub fn connection(&self) -> RpcConnection<'_> {
        RpcConnection {
            service: self,
            initialized: AtomicBool::new(false),
            connection_id: format!("connection_{}", Uuid::new_v4()),
            output: None,
            subscriptions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Open an uninitialized stdio connection with a bounded notification sink.
    #[must_use]
    pub fn live_connection(&self, output: mpsc::Sender<Value>) -> RpcConnection<'_> {
        RpcConnection {
            service: self,
            initialized: AtomicBool::new(false),
            connection_id: format!("connection_{}", Uuid::new_v4()),
            output: Some(output),
            subscriptions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Handle one unary JSON-RPC frame.
    ///
    /// Unary HTTP has no connection state, so every non-`initialize` request
    /// must carry a matching typed protocol identity in the top-level
    /// `protocol` extension field.
    #[must_use]
    pub fn handle_text(&self, text: &str) -> JsonRpcOutcome {
        let connection = self.connection();
        if let Some(outcome) = negotiate_unary_protocol(text, &connection) {
            return outcome;
        }
        connection.handle_text(text)
    }

    #[allow(clippy::too_many_lines)]
    async fn dispatch(
        &self,
        method: &str,
        params: &Value,
        connection_id: Option<&str>,
    ) -> Result<Value, RpcError> {
        match method {
            "initialize" => {
                self.initialize_result(params, self.notifications == RpcNotificationMode::Live)
            }
            "shutdown" => {
                self.coordinator
                    .shutdown(Duration::from_secs(10))
                    .await
                    .map_err(rpc_error)?;
                Ok(json!({"status": "shutdown"}))
            }
            "diagnostics.get" => Ok(json!({
                "sdk": starweaver_core::sdk_name(),
                "version": env!("CARGO_PKG_VERSION"),
                "configPath": self.config.config_path,
                "databasePath": self.config.database_path,
                "stateDir": self.config.state_dir,
                "workspaceRoot": self.config.workspace_root,
                "defaultProfile": self.config.default_profile,
            })),
            "profile.list" | "model.list" => {
                let scope = resolved_client_state_scope(params)?;
                let selected = self.selected_profile(Some(&scope))?;
                let current = self.catalog.profile(&selected)?;
                Ok(json!({
                    "profiles": self.catalog.profiles(),
                    "current": {
                        "clientStateScope": scope,
                        "selectedProfile": selected,
                        "modelId": current.model_id,
                    },
                }))
            }
            "profile.get" => {
                let name = required_string(params, "name")?;
                let profile = self.catalog.profile(&name)?;
                Ok(json!(ProfileGetResult {
                    name,
                    profile: ProfileConfig {
                        label: profile.label.clone(),
                        model_id: profile.model_id.clone(),
                        model_settings: profile.model_settings.clone(),
                        model_config: profile.model_config.clone(),
                        instructions: profile.instructions.clone(),
                        toolsets: profile.toolsets.clone(),
                        subagents: profile.subagents.clone(),
                    },
                }))
            }
            "model.current" => {
                let scope = resolved_client_state_scope(params)?;
                let selected = self.selected_profile(Some(&scope))?;
                let profile = self.catalog.profile(&selected)?;
                Ok(json!({
                    "clientStateScope": scope,
                    "selectedProfile": selected,
                    "modelId": profile.model_id,
                }))
            }
            "model.select" => {
                let profile_name = required_string(params, "profile")?;
                let profile = self.catalog.profile(&profile_name)?;
                let scope = resolved_client_state_scope(params)?;
                self.state
                    .write_selected_profile(&scope, &profile_name)
                    .map_err(rpc_error)?;
                Ok(json!({
                    "clientStateScope": scope,
                    "selectedProfile": profile_name,
                    "modelId": profile.model_id,
                }))
            }
            "config.get" => {
                let key = required_string(params, "key")?;
                let value = match key.as_str() {
                    "storage.database" | "database_path" => {
                        self.config.database_path.display().to_string()
                    }
                    "runtime.default_profile" | "default_profile" => {
                        self.config.default_profile.clone()
                    }
                    "environment.workspace_root" | "workspace_root" => {
                        self.config.workspace_root.display().to_string()
                    }
                    other => {
                        return Err(RpcError::new(
                            INVALID_PARAMS,
                            format!("unsupported RPC config key: {other}"),
                        ));
                    }
                };
                Ok(json!({"key": key, "value": value}))
            }
            "storage.importLegacy" => {
                let params = serde_json::from_value::<StorageImportLegacyParams>(params.clone())
                    .map_err(|error| {
                        RpcError::new(
                            INVALID_PARAMS,
                            format!("invalid storage.importLegacy params: {error}"),
                        )
                    })?;
                let report = run_storage(self.storage.clone(), move |storage| {
                    storage.import_legacy_project_database(params.source_path, params.workspace)
                })
                .await?;
                serde_json::to_value(StorageImportLegacyResult {
                    source_path: report.source_path,
                    workspace: report.workspace,
                    sessions_imported: report.sessions_imported,
                    rows_imported: report.rows_imported,
                    imported: report.imported,
                })
                .map_err(|error| {
                    RpcError::new(
                        starweaver_rpc_core::SERVER_ERROR,
                        format!("failed to encode storage.importLegacy result: {error}"),
                    )
                })
            }
            "session.create" => {
                let profile = params
                    .get("profile")
                    .and_then(Value::as_str)
                    .unwrap_or(&self.config.default_profile);
                self.catalog.profile(profile)?;
                let title = params
                    .get("title")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let profile = profile.to_string();
                let workspace = std::fs::canonicalize(&self.config.workspace_root)
                    .unwrap_or_else(|_| self.config.workspace_root.clone())
                    .to_string_lossy()
                    .into_owned();
                let session = run_storage(self.storage.clone(), move |storage| {
                    storage.create_session_for_product(
                        Some(profile),
                        title,
                        Some(workspace),
                        Some("rpc"),
                    )
                })
                .await?;
                Ok(json!({"session": session}))
            }
            "session.list" => {
                let limit = optional_usize(params, "limit")?.unwrap_or(50);
                let sessions = self
                    .storage
                    .session_store()
                    .list_sessions(SessionFilter {
                        limit: Some(limit),
                        ..SessionFilter::default()
                    })
                    .await
                    .map_err(rpc_error)?;
                Ok(json!({"sessions": sessions}))
            }
            "session.search" => {
                let provider = self.session_search.as_ref().ok_or_else(|| {
                    RpcError::new(
                        starweaver_rpc_core::UNSUPPORTED_FEATURE,
                        "session.search is not installed",
                    )
                })?;
                let params = serde_json::from_value::<SessionSearchParams>(params.clone())
                    .map_err(|error| {
                        RpcError::new(
                            INVALID_PARAMS,
                            format!("invalid session.search params: {error}"),
                        )
                    })?;
                let page = provider
                    .search(&self.session_search_scope, params.into_query())
                    .await
                    .map_err(session_search_error)?;
                serde_json::to_value(SessionSearchResult::from(page)).map_err(|error| {
                    RpcError::new(
                        starweaver_rpc_core::SERVER_ERROR,
                        format!("failed to encode session search result: {error}"),
                    )
                })
            }
            "session.get" => {
                let session_id = SessionId::from_string(required_string(params, "sessionId")?);
                let runs_limit = optional_usize(params, "runs")?.unwrap_or(20);
                let store = self.storage.session_store();
                let session = store.load_session(&session_id).await.map_err(rpc_error)?;
                let mut runs = store.list_runs(&session_id).await.map_err(rpc_error)?;
                if runs.len() > runs_limit {
                    runs = runs.split_off(runs.len() - runs_limit);
                }
                Ok(json!({"session": session, "runs": runs}))
            }
            "session.current.get" => Ok(json!({
                "sessionId": self.state.read_current_session().map_err(rpc_error)?,
            })),
            "session.current.set" => {
                let session_id = required_string(params, "sessionId")?;
                self.storage
                    .session_store()
                    .load_session(&SessionId::from_string(session_id.clone()))
                    .await
                    .map_err(rpc_error)?;
                self.state
                    .write_current_session(&session_id)
                    .map_err(rpc_error)?;
                Ok(json!({"sessionId": session_id}))
            }
            "session.delete" => {
                let session_id = SessionId::from_string(required_string(params, "sessionId")?);
                let session = self
                    .coordinator
                    .tombstone_session_fenced(&session_id, Duration::from_secs(10))
                    .await
                    .map_err(rpc_error)?;
                Ok(json!({
                    "sessionId": session_id,
                    "deleted": session.status == starweaver_session::SessionStatus::Deleted,
                    "revision": session.revision,
                }))
            }
            "run.start" => self.run_start(params, connection_id).await,
            "run.resume" => self.run_resume(params, connection_id).await,
            "run.prompt" => self.run_prompt(params, connection_id).await,
            "run.status" => {
                let (session_id, run_id) = run_identity(params)?;
                let status = self
                    .coordinator
                    .status(&session_id, &run_id)
                    .await
                    .map_err(rpc_error)?;
                Ok(json!({"status": status}))
            }
            "run.await" => {
                let (session_id, run_id) = run_identity(params)?;
                let requested = params
                    .get("timeoutMs")
                    .and_then(Value::as_u64)
                    .map(Duration::from_millis);
                // Both stdio and HTTP prohibit unbounded blocking so a control connection cannot
                // be held forever by run.await.
                let timeout = Some(requested.unwrap_or(MAX_RUN_AWAIT).min(MAX_RUN_AWAIT));
                let status = self
                    .coordinator
                    .await_terminal(&session_id, &run_id, timeout)
                    .await
                    .map_err(rpc_error)?;
                Ok(json!({"status": status}))
            }
            "run.cancel" => {
                let (session_id, run_id) = run_identity(params)?;
                self.coordinator
                    .cancel(
                        &session_id,
                        &run_id,
                        params
                            .get("reason")
                            .and_then(Value::as_str)
                            .map(ToString::to_string),
                    )
                    .await
                    .map_err(rpc_error)
            }
            "run.steer" => {
                let (session_id, run_id) = run_identity(params)?;
                let text = required_string(params, "text")?;
                let steering_id = params
                    .get("steeringId")
                    .and_then(Value::as_str)
                    .map_or_else(
                        || format!("steering_{}", Uuid::new_v4()),
                        ToString::to_string,
                    );
                self.coordinator
                    .steer(&session_id, &run_id, steering_id, text)
                    .await
                    .map_err(rpc_error)
            }
            "run.attach" | "session.output" => self.run_attach(params).await,
            "stream.replay" | "session.replay" => self.stream_replay(params).await,
            "approval.list" => {
                let session_id = optional_session_id(params, "sessionId");
                let run_id = optional_run_id(params, "runId");
                let approvals = run_storage(self.storage.clone(), move |storage| {
                    storage.list_approvals(session_id.as_ref(), run_id.as_ref())
                })
                .await?;
                Ok(json!({"approvals": approvals}))
            }
            "approval.show" => {
                let id = required_string(params, "approvalId")?;
                let approval = run_storage(self.storage.clone(), move |storage| {
                    storage.load_approval(&id)
                })
                .await?;
                Ok(json!({"approval": approval}))
            }
            "approval.decide" => {
                let id = required_string(params, "approvalId")?;
                let status = match required_string(params, "status")?.as_str() {
                    "approved" | "approve" => ApprovalStatus::Approved,
                    "denied" | "rejected" | "reject" => ApprovalStatus::Denied,
                    other => {
                        return Err(RpcError::new(
                            INVALID_PARAMS,
                            format!("unknown approval status: {other}"),
                        ));
                    }
                };
                let decided_by = params
                    .get("decidedBy")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let reason = params
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let approval = run_storage(self.storage.clone(), move |storage| {
                    storage.decide_approval(&id, status, decided_by, reason)
                })
                .await?;
                Ok(json!({"approval": approval}))
            }
            "deferred.list" => {
                let session_id = optional_session_id(params, "sessionId");
                let run_id = optional_run_id(params, "runId");
                let deferred = run_storage(self.storage.clone(), move |storage| {
                    storage.list_deferred_tools(session_id.as_ref(), run_id.as_ref())
                })
                .await?;
                Ok(json!({"deferred": deferred}))
            }
            "deferred.show" => {
                let id = required_string(params, "deferredId")?;
                let deferred = run_storage(self.storage.clone(), move |storage| {
                    storage.load_deferred_tool(&id)
                })
                .await?;
                Ok(json!({"deferred": deferred}))
            }
            "deferred.complete" => {
                self.resolve_deferred(params, ExecutionStatus::Completed)
                    .await
            }
            "deferred.fail" => self.resolve_deferred(params, ExecutionStatus::Failed).await,
            "environment.attach" => {
                self.validate_environment_attach_scope(params)?;
                self.environment_manager.attach(params, connection_id).await
            }
            "environment.detach" => self.environment_manager.detach(params, connection_id),
            "environment.list" => self.environment_manager.list(params, connection_id),
            "environment.health" => self.environment_manager.health(params, connection_id).await,
            "environment.active_mount" => {
                self.environment_active_mount(params, connection_id).await
            }
            "environment.active_unmount" => self.environment_active_unmount(params).await,
            "environment.active_list" => self.environment_active_list(params),
            "stream.subscribe" | "stream.unsubscribe" => Err(RpcError::new(
                starweaver_rpc_core::UNSUPPORTED_FEATURE,
                "live subscriptions are not yet available on this transport",
            )),
            other => Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("method not found: {other}"),
            )),
        }
    }

    async fn environment_active_mount(
        &self,
        params: &Value,
        connection_id: Option<&str>,
    ) -> Result<Value, RpcError> {
        let params_digest = canonical_json(params)?;
        let params = serde_json::from_value::<starweaver_rpc_core::EnvironmentActiveMountParams>(
            params.clone(),
        )
        .map_err(|error| {
            RpcError::new(
                INVALID_PARAMS,
                format!("invalid active mount params: {error}"),
            )
        })?;
        let session_id = self.coordinator.active_run_session_id(&params.run_id)?;
        let attachment = self
            .environment_manager
            .materialize_active_attachment(
                params.attachment.clone(),
                Some(&session_id),
                connection_id,
            )
            .await?;
        self.coordinator
            .active_environment_mount(&params, attachment, &params_digest)
            .await
            .map(|outcome| outcome.result)
    }

    async fn environment_active_unmount(&self, params: &Value) -> Result<Value, RpcError> {
        let params_digest = canonical_json(params)?;
        let params = serde_json::from_value::<starweaver_rpc_core::EnvironmentActiveUnmountParams>(
            params.clone(),
        )
        .map_err(|error| {
            RpcError::new(
                INVALID_PARAMS,
                format!("invalid active unmount params: {error}"),
            )
        })?;
        self.coordinator
            .active_environment_unmount(&params, &params_digest)
            .await
            .map(|outcome| outcome.result)
    }

    fn environment_active_list(&self, params: &Value) -> Result<Value, RpcError> {
        let params = serde_json::from_value::<starweaver_rpc_core::EnvironmentActiveListParams>(
            params.clone(),
        )
        .map_err(|error| {
            RpcError::new(
                INVALID_PARAMS,
                format!("invalid active list params: {error}"),
            )
        })?;
        self.coordinator.active_environment_list(&params.run_id)
    }

    fn validate_environment_attach_scope(&self, params: &Value) -> Result<(), RpcError> {
        if self.notifications == RpcNotificationMode::Live {
            return Ok(());
        }
        let scope_kind = params
            .get("scope")
            .and_then(|scope| scope.get("kind"))
            .and_then(Value::as_str)
            .unwrap_or("connection");
        if scope_kind == "connection" {
            return Err(RpcError::new(
                starweaver_rpc_core::UNSUPPORTED_FEATURE,
                "connection-scoped environment attachments are not supported by replay-only transports; use session scope",
            ));
        }
        Ok(())
    }

    async fn run_start(
        &self,
        params: &Value,
        connection_id: Option<&str>,
    ) -> Result<Value, RpcError> {
        let started = self.start_run_from_params(params, connection_id).await?;
        Ok(json!({
            "sessionId": started.session_id,
            "runId": started.run_id,
            "status": started.status,
            "idempotentReplay": started.idempotent_replay,
            "payloadFormat": "display",
            "environmentAttachments": started.environment_attachments,
        }))
    }

    async fn run_prompt(
        &self,
        params: &Value,
        connection_id: Option<&str>,
    ) -> Result<Value, RpcError> {
        let started = self.start_run_from_params(params, connection_id).await?;
        // run.prompt is bounded on every transport; clients use run.start plus status/replay for
        // long-lived work.
        let timeout = Some(MAX_RUN_AWAIT);
        let status = self
            .coordinator
            .await_terminal(&started.session_id, &started.run_id, timeout)
            .await
            .map_err(rpc_error)?;
        Ok(json!({
            "sessionId": started.session_id,
            "runId": started.run_id,
            "status": status.status,
            "output": status.output_preview,
            "error": status.error,
            "environmentAttachments": started.environment_attachments,
        }))
    }

    #[allow(clippy::too_many_lines)]
    async fn run_resume(
        &self,
        params: &Value,
        connection_id: Option<&str>,
    ) -> Result<Value, RpcError> {
        let params =
            serde_json::from_value::<RunResumeParams>(params.clone()).map_err(|error| {
                RpcError::new(
                    INVALID_PARAMS,
                    format!("invalid run.resume params: {error}"),
                )
            })?;
        if params.idempotency_key.trim().is_empty() {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "run.resume requires a non-empty idempotencyKey",
            ));
        }
        let source = self
            .storage
            .session_store()
            .load_run(&params.session_id, &params.run_id)
            .await
            .map_err(rpc_error)?;
        let session = self
            .storage
            .session_store()
            .load_session(&params.session_id)
            .await
            .map_err(rpc_error)?;
        let profile = params
            .profile
            .clone()
            .or(source.profile)
            .or(session.profile)
            .unwrap_or_else(|| self.catalog.default_profile().to_string());
        self.catalog.profile(&profile).map_err(rpc_error)?;
        let environment_attachments =
            effective_rpc_environment_attachments(&params.environment_attachments);
        let fingerprint_attachments = run_attachment_fingerprint(&environment_attachments)?;
        let fingerprint = command_fingerprint(
            "rpc_run_resume",
            &json!({
                "sessionId": params.session_id,
                "sourceRunId": params.run_id,
                "profile": profile,
                "environmentAttachments": fingerprint_attachments,
            }),
        )
        .map_err(|error| RpcError::new(INVALID_PARAMS, error.to_string()))?;
        let source_run_id = params.run_id.clone();
        if let Some(started) = self
            .coordinator
            .lookup_started_run(&params.idempotency_key, &fingerprint)
            .await
            .map_err(rpc_error)?
        {
            self.environment_manager.authorize_run_attachment_replay(
                &environment_attachments,
                Some(params.session_id.as_str()),
                connection_id,
            )?;
            return serde_json::to_value(RunResumeResult {
                session_id: started.session_id,
                run_id: started.run_id,
                source_run_id,
                status: started.status,
                environment_attachments: started.environment_attachments,
                idempotent_replay: true,
            })
            .map_err(|error| RpcError::new(starweaver_rpc_core::SERVER_ERROR, error.to_string()));
        }
        let materialized = self
            .environment_manager
            .materialize_run_attachments(
                environment_attachments,
                Some(params.session_id.as_str()),
                connection_id,
            )
            .await?;
        let reservation_id = format!("pending_{}", Uuid::new_v4());
        self.environment_manager
            .mark_run_started(&reservation_id, &materialized)?;
        let result = self
            .coordinator
            .resume_waiting(RpcHitlResumeRequest {
                session_id: params.session_id,
                source_run_id: source_run_id.clone(),
                profile,
                environment_attachments: materialized,
                idempotency_key: params.idempotency_key,
                command_fingerprint: fingerprint,
                install_session_management: self.notifications == RpcNotificationMode::Live,
            })
            .await
            .map_err(rpc_error);
        let cleanup = self.environment_manager.mark_run_finished(&reservation_id);
        let started = match (result, cleanup) {
            (Ok(started), Ok(())) => started,
            (Err(error), Ok(())) => return Err(error),
            (Ok(started), Err(cleanup)) => {
                let _ = self
                    .coordinator
                    .cancel(
                        &started.session_id,
                        &started.run_id,
                        Some("environment lease reservation cleanup failed".to_string()),
                    )
                    .await;
                return Err(RpcError::new(
                    starweaver_rpc_core::SERVER_ERROR,
                    format!(
                        "environment lease reservation cleanup failed: {}",
                        cleanup.message
                    ),
                ));
            }
            (Err(error), Err(cleanup)) => {
                return Err(RpcError::new(
                    starweaver_rpc_core::SERVER_ERROR,
                    format!(
                        "{}; environment lease reservation cleanup failed: {}",
                        error.message, cleanup.message
                    ),
                ));
            }
        };
        serde_json::to_value(RunResumeResult {
            session_id: started.session_id,
            run_id: started.run_id,
            source_run_id,
            status: started.status,
            environment_attachments: started.environment_attachments,
            idempotent_replay: started.idempotent_replay,
        })
        .map_err(|error| RpcError::new(starweaver_rpc_core::SERVER_ERROR, error.to_string()))
    }

    async fn start_run_from_params(
        &self,
        params: &Value,
        connection_id: Option<&str>,
    ) -> Result<crate::RpcStartedRun, RpcError> {
        let refs = starweaver_rpc_core::environment_attachment_refs(params)?;
        let refs = effective_rpc_environment_attachments(&refs);
        let mut request = run_request(&self.catalog, &self.state, params, &refs)?;
        // HTTP/replay-only callers do not acquire model-visible mutation authority merely by
        // selecting a profile. A future typed HTTP session-management scope can opt in.
        request.install_session_management = self.notifications == RpcNotificationMode::Live;
        if let Some(started) = self
            .coordinator
            .lookup_started_run(&request.idempotency_key, &request.command_fingerprint)
            .await
            .map_err(rpc_error)?
        {
            self.environment_manager.authorize_run_attachment_replay(
                &refs,
                Some(started.session_id.as_str()),
                connection_id,
            )?;
            return Ok(started);
        }
        let materialized = self
            .environment_manager
            .materialize_run_attachments(
                refs,
                request.session_id.as_ref().map(SessionId::as_str),
                connection_id,
            )
            .await?;
        let reservation_id = format!("pending_{}", Uuid::new_v4());
        self.environment_manager
            .mark_run_started(&reservation_id, &materialized)?;
        request.environment_attachments = materialized;
        let result = self.coordinator.start(request).await.map_err(rpc_error);
        match (
            result,
            self.environment_manager.mark_run_finished(&reservation_id),
        ) {
            (Ok(started), Ok(())) => Ok(started),
            (Err(error), Ok(())) => Err(error),
            (Ok(started), Err(cleanup)) => {
                let _receipt = self
                    .coordinator
                    .cancel(
                        &started.session_id,
                        &started.run_id,
                        Some("environment lease reservation cleanup failed".to_string()),
                    )
                    .await;
                Err(RpcError::new(
                    starweaver_rpc_core::SERVER_ERROR,
                    format!(
                        "environment lease reservation cleanup failed: {}",
                        cleanup.message
                    ),
                ))
            }
            (Err(error), Err(cleanup)) => Err(RpcError::new(
                starweaver_rpc_core::SERVER_ERROR,
                format!(
                    "{}; environment lease reservation cleanup failed: {}",
                    error.message, cleanup.message
                ),
            )),
        }
    }

    async fn run_attach(&self, params: &Value) -> Result<Value, RpcError> {
        let (session_id, run_id) = run_identity(params)?;
        let scope = ReplayScope::run(run_id.as_str());
        let cursor = replay_cursor_from_params(params, scope)?;
        let events = self
            .coordinator
            .replay(&session_id, &run_id, cursor, None)
            .await
            .map_err(rpc_error)?;
        let status = self
            .coordinator
            .status(&session_id, &run_id)
            .await
            .map_err(rpc_error)?;
        Ok(starweaver_rpc_core::attachment_result(
            session_id.as_str(),
            Some(run_id.as_str()),
            !status.terminal(),
            &events,
            starweaver_rpc_core::StreamPayloadFormat::DisplayMessage,
        ))
    }

    async fn stream_replay(&self, params: &Value) -> Result<Value, RpcError> {
        let (session_id, run_id) = run_identity(params)?;
        let scope = ReplayScope::run(run_id.as_str());
        let cursor = replay_cursor_from_params(params, scope.clone())?;
        let limit = optional_usize(params, "limit")?;
        let events = self
            .coordinator
            .replay(&session_id, &run_id, cursor.clone(), limit)
            .await
            .map_err(rpc_error)?;
        let next_sequence = events.last().map_or_else(
            || cursor.as_ref().map_or(0, |cursor| cursor.sequence + 1),
            |event| event.sequence.saturating_add(1),
        );
        Ok(replay_result(
            session_id.as_str(),
            Some(run_id.as_str()),
            &scope,
            &events,
            cursor.as_ref(),
            next_sequence,
        ))
    }

    async fn resolve_deferred(
        &self,
        params: &Value,
        status: ExecutionStatus,
    ) -> Result<Value, RpcError> {
        let id = required_string(params, "deferredId")?;
        let response = if status == ExecutionStatus::Failed {
            json!({"error": required_string(params, "error")?})
        } else {
            params.get("result").cloned().unwrap_or(Value::Null)
        };
        let deferred = run_storage(self.storage.clone(), move |storage| {
            storage.resolve_deferred_tool(&id, status, response)
        })
        .await?;
        Ok(json!({"deferred": deferred}))
    }

    fn selected_profile(&self, scope: Option<&str>) -> Result<String, RpcError> {
        let selected = scope
            .map(|scope| self.state.read_selected_profile(scope).map_err(rpc_error))
            .transpose()?
            .flatten()
            .filter(|profile| self.catalog.profile(profile).is_ok());
        Ok(selected.unwrap_or_else(|| self.catalog.default_profile().to_string()))
    }

    fn initialize_result(&self, params: &Value, live: bool) -> Result<Value, RpcError> {
        let params =
            serde_json::from_value::<HostInitializeParams>(params.clone()).map_err(|error| {
                RpcError::new(
                    INVALID_PARAMS,
                    format!("invalid initialize params: {error}"),
                )
            })?;
        validate_host_initialize(&params)?;
        let search_capabilities =
            self.session_search
                .as_ref()
                .map(|provider| SessionSearchFeatureCapabilities {
                    available: true,
                    provider: provider.capabilities(),
                });
        let mut protocol =
            host_protocol_identity_with_session_search(self.session_search.is_some());
        if live {
            protocol.features.push("stream.subscribe".to_string());
        }
        Ok(json!({
            "protocol": protocol,
            "serverInfo": {
                "name": "starweaver-rpc",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "capabilities": {
                "sessions": true,
                "runs": true,
                "management": true,
                "profiles": true,
                "clientModelSelection": true,
                "blockingRunStart": false,
                "blockingRunPrompt": true,
                "nonBlockingRunStart": true,
                "liveDisplay": live,
                "streamReplay": true,
                "streamSubscribe": live,
                "cancel": true,
                "steering": true,
                "attach": true,
                "environmentAttachments": true,
                "environmentActiveMounts": true,
                "defaultStreamPayload": "display_message",
                "approvals": true,
                "deferred": true,
                "sessionSearch": search_capabilities,
            },
            "config": {
                "globalDir": self.config.state_dir.parent(),
                "projectDir": self.config.workspace_root,
                "defaultProfile": self.config.default_profile,
            },
        }))
    }
}

fn negotiate_unary_protocol(text: &str, connection: &RpcConnection<'_>) -> Option<JsonRpcOutcome> {
    let value = serde_json::from_str::<Value>(text).ok()?;
    if value.get("method").and_then(Value::as_str) == Some("initialize") {
        return None;
    }
    let protocol_value = value.get("protocol")?;
    let negotiated = serde_json::from_value::<ProtocolIdentity>(protocol_value.clone())
        .map_err(|error| format!("invalid unary protocol identity: {error}"))
        .and_then(|protocol| {
            validate_host_initialize(&HostInitializeParams {
                protocol: Some(protocol),
            })
            .map_err(|error| error.message)
        });
    match negotiated {
        Ok(()) => {
            connection.initialized.store(true, Ordering::Release);
            None
        }
        Err(message) => Some(JsonRpcOutcome {
            response: value
                .get("id")
                .map(|id| error_response(id, INVALID_PARAMS, &message)),
            shutdown: false,
        }),
    }
}

fn run_attachment_fingerprint(
    attachments: &[EnvironmentAttachmentRef],
) -> Result<Vec<Value>, RpcError> {
    attachments
        .iter()
        .map(|attachment| {
            let mut value = serde_json::to_value(attachment)
                .map_err(|error| RpcError::new(INVALID_PARAMS, error.to_string()))?;
            if let Some(token) = attachment.requested_auth_token() {
                let digest = command_fingerprint("rpc.environment.auth", &token)
                    .map_err(|error| RpcError::new(INVALID_PARAMS, error.to_string()))?;
                let Some(object) = value.as_object_mut() else {
                    return Err(RpcError::new(
                        INVALID_PARAMS,
                        "environment attachment fingerprint is not an object",
                    ));
                };
                object.insert("authTokenDigest".to_string(), json!(digest));
            }
            Ok(value)
        })
        .collect()
}

fn run_request(
    catalog: &RpcAgentCatalog,
    state: &RpcStateRepository,
    params: &Value,
    environment_attachments: &[EnvironmentAttachmentRef],
) -> Result<RpcRunRequest, RpcError> {
    let (durable_input, input) = run_input(params)?;
    let explicit_profile = explicit_profile_selector(params)?;
    let scope = resolved_client_state_scope(params)?;
    let selected_profile = state
        .read_selected_profile(&scope)
        .map_err(rpc_error)?
        .filter(|profile| catalog.profile(profile).is_ok());
    let profile = explicit_profile
        .or(selected_profile)
        .unwrap_or_else(|| catalog.default_profile().to_string());
    catalog.profile(&profile)?;
    let session_id = optional_session_id(params, "sessionId");
    let idempotency_key = params
        .get("idempotencyKey")
        .and_then(Value::as_str)
        .map_or_else(|| format!("run_{}", Uuid::new_v4()), ToString::to_string);
    let restore_from_run_id = params
        .get("restoreFromRunId")
        .or_else(|| params.get("runId"))
        .and_then(Value::as_str)
        .map(|value| RunId::from_string(value.to_string()));
    let environment_attachments = run_attachment_fingerprint(environment_attachments)?;
    let fingerprint_input = json!({
        "sessionId": session_id,
        "profile": profile,
        "input": durable_input,
        "restoreFromRunId": restore_from_run_id,
        "environmentAttachments": environment_attachments,
    });
    let command_fingerprint = command_fingerprint("rpc_run_start", &fingerprint_input)
        .map_err(|error| RpcError::new(INVALID_PARAMS, error.to_string()))?;
    Ok(RpcRunRequest {
        durable_input,
        input,
        session_id,
        restore_from_run_id,
        profile,
        environment_attachments: Vec::new(),
        idempotency_key,
        command_fingerprint,
        install_session_management: false,
    })
}

fn run_input(params: &Value) -> Result<(Vec<InputPart>, AgentInput), RpcError> {
    let prompt = params.get("prompt");
    let structured = params.get("input");
    let durable_input = match (prompt, structured) {
        (Some(_), Some(_)) => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "run input accepts either prompt or input.parts, not both",
            ));
        }
        (Some(_), None) => vec![InputPart::text(required_string(params, "prompt")?)],
        (None, Some(value)) => {
            let input = serde_json::from_value::<RunInput>(value.clone()).map_err(|error| {
                RpcError::new(INVALID_PARAMS, format!("invalid input.parts: {error}"))
            })?;
            if input.parts.is_empty() {
                return Err(RpcError::new(
                    INVALID_PARAMS,
                    "input.parts must contain at least one part",
                ));
            }
            input.parts
        }
        (None, None) => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "run input requires prompt or input.parts",
            ));
        }
    };
    let content = durable_input
        .iter()
        .cloned()
        .map(starweaver_model::ContentPart::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| RpcError::new(INVALID_PARAMS, error.to_string()))?;
    Ok((durable_input, AgentInput::parts(content)))
}

fn run_identity(params: &Value) -> Result<(SessionId, RunId), RpcError> {
    Ok((
        SessionId::from_string(required_string(params, "sessionId")?),
        RunId::from_string(required_string(params, "runId")?),
    ))
}

fn optional_session_id(params: &Value, key: &str) -> Option<SessionId> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(|value| SessionId::from_string(value.to_string()))
}

fn optional_run_id(params: &Value, key: &str) -> Option<RunId> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(|value| RunId::from_string(value.to_string()))
}

fn explicit_profile_selector(params: &Value) -> Result<Option<String>, RpcError> {
    let profile = optional_string(params, "profile")?;
    let model_profile = optional_string(params, "modelProfile")?;
    if let (Some(profile), Some(model_profile)) = (&profile, &model_profile)
        && profile != model_profile
    {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "profile and modelProfile must match when both are supplied",
        ));
    }
    let selected = profile.or(model_profile);
    if selected
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "profile selector must not be empty",
        ));
    }
    Ok(selected)
}

fn subscription_replay_limit(params: &Value) -> Result<usize, RpcError> {
    let value = params
        .get("replay")
        .and_then(|replay| replay.get("limit"))
        .or_else(|| params.get("limit"));
    value.map_or(Ok(1_000), |value| {
        value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value > 0 && *value <= 10_000)
            .ok_or_else(|| {
                RpcError::new(
                    INVALID_PARAMS,
                    "subscription replay limit must be between 1 and 10000",
                )
            })
    })
}

fn resolved_client_state_scope(params: &Value) -> Result<String, RpcError> {
    Ok(
        client_state_scope(params, false)?
            .unwrap_or_else(|| DEFAULT_CLIENT_STATE_SCOPE.to_string()),
    )
}

fn client_state_scope(params: &Value, required: bool) -> Result<Option<String>, RpcError> {
    let scoped = optional_string(params, "clientStateScope")?;
    let legacy = optional_string(params, "client")?;
    if let (Some(scoped), Some(legacy)) = (&scoped, &legacy)
        && scoped != legacy
    {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "clientStateScope and legacy client must match when both are supplied",
        ));
    }
    let scope = scoped.or(legacy);
    if required && scope.is_none() {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "missing required string: clientStateScope",
        ));
    }
    if let Some(scope) = scope.as_deref()
        && !valid_client_state_scope(scope)
    {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "clientStateScope must be a 1-64 character ASCII slug",
        ));
    }
    Ok(scope)
}

fn optional_string(params: &Value, key: &str) -> Result<Option<String>, RpcError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(RpcError::new(
            INVALID_PARAMS,
            format!("{key} must be a string"),
        )),
    }
}

fn valid_client_state_scope(scope: &str) -> bool {
    !scope.is_empty()
        && scope.len() <= 64
        && scope
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && scope
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
}

fn required_string(params: &Value, key: &str) -> Result<String, RpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, format!("missing required string: {key}")))
}

fn optional_usize(params: &Value, key: &str) -> Result<Option<usize>, RpcError> {
    params
        .get(key)
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| RpcError::new(INVALID_PARAMS, format!("invalid integer: {key}")))
        })
        .transpose()
}

fn canonical_json(value: &Value) -> Result<String, RpcError> {
    serde_json::to_string(value).map_err(|error| {
        RpcError::new(
            INVALID_PARAMS,
            format!("invalid active environment params: {error}"),
        )
    })
}

fn rpc_error(error: impl Into<RpcHostError>) -> RpcError {
    error.into().into()
}

fn session_search_error(error: SessionSearchError) -> RpcError {
    match error {
        SessionSearchError::InvalidQuery(message) | SessionSearchError::InvalidCursor(message) => {
            RpcError::new(INVALID_PARAMS, message)
        }
        SessionSearchError::Unsupported(message) => {
            RpcError::new(starweaver_rpc_core::UNSUPPORTED_FEATURE, message)
        }
        SessionSearchError::Unavailable(message) => {
            RpcError::new(starweaver_rpc_core::SESSION_SEARCH_UNAVAILABLE, message)
        }
        SessionSearchError::PermissionDenied => RpcError::new(
            starweaver_rpc_core::SERVER_ERROR,
            "session search permission denied",
        ),
        SessionSearchError::Failed(_) => {
            RpcError::new(starweaver_rpc_core::SERVER_ERROR, "session search failed")
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[allow(clippy::needless_pass_by_value)]
    fn request(id: usize, method: &str, params: Value) -> String {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "protocol": starweaver_rpc_core::host_protocol_identity(),
            "params": params
        })
        .to_string()
    }

    #[test]
    fn service_creates_and_reads_session_without_cli() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        let service = RpcService::live(config).unwrap();
        let created = service.handle_text(&request(
            1,
            "session.create",
            json!({"title": "RPC session"}),
        ));
        let response = created.response.unwrap();
        let session_id = response["result"]["session"]["session_id"]
            .as_str()
            .unwrap();
        let loaded =
            service.handle_text(&request(2, "session.get", json!({"sessionId": session_id})));
        assert_eq!(
            loaded.response.unwrap()["result"]["session"]["title"],
            "RPC session"
        );
    }

    #[test]
    fn rpc_continues_cli_style_run_context_from_shared_database() {
        use starweaver_context::ResumableState;
        use starweaver_core::ConversationId;
        use starweaver_session::{RunEvidenceCommit, RunRecord, RunStatus};

        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        let service = RpcService::live(config).unwrap();
        let session = service
            .storage
            .create_session_for_product(
                Some("general".to_string()),
                Some("CLI session continued by RPC".to_string()),
                Some(temp.path().to_string_lossy().into_owned()),
                Some("cli"),
            )
            .unwrap();
        let source_run_id = RunId::from_string("run_cli_context_source");
        let mut source_run = RunRecord::new(
            session.session_id.clone(),
            source_run_id.clone(),
            ConversationId::new(),
        );
        source_run.trigger_type = Some("cli".to_string());
        source_run.profile = Some("general".to_string());
        source_run.status = RunStatus::Completed;
        source_run.output_preview = Some("CLI source output".to_string());
        let source_run = service.storage.begin_run(source_run).unwrap();
        let mut source_state = ResumableState {
            session_id: Some(session.session_id.clone()),
            run_id: Some(source_run_id.clone()),
            conversation_id: Some(source_run.conversation_id.clone()),
            ..ResumableState::default()
        };
        source_state.metadata.insert(
            "cross_product_context_marker".to_string(),
            json!("from-cli"),
        );
        service
            .storage
            .commit_run_evidence(RunEvidenceCommit::new(source_run, source_state))
            .unwrap();

        let started = service.handle_text(&request(
            1,
            "run.start",
            json!({
                "sessionId": session.session_id.as_str(),
                "restoreFromRunId": source_run_id.as_str(),
                "profile": "default",
                "prompt": "continue from the CLI context"
            }),
        ));
        let started = started.response.unwrap();
        assert!(started.get("error").is_none(), "{started}");
        let continued_run_id = started["result"]["runId"].as_str().unwrap();
        let awaited = service.handle_text(&request(
            2,
            "run.await",
            json!({
                "sessionId": session.session_id.as_str(),
                "runId": continued_run_id,
                "timeoutMs": 5_000
            }),
        ));
        let awaited = awaited.response.unwrap();
        assert_eq!(awaited["result"]["status"]["status"], "completed");

        let continued_run_id = RunId::from_string(continued_run_id);
        let continued = service
            .storage
            .load_run(&session.session_id, &continued_run_id)
            .unwrap();
        assert_eq!(continued.trigger_type.as_deref(), Some("rpc"));
        assert_eq!(continued.restore_from_run_id.as_ref(), Some(&source_run_id));
        let continued_state = service
            .storage
            .load_run_context(&session.session_id, &continued_run_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            continued_state.metadata.get("cross_product_context_marker"),
            Some(&json!("from-cli"))
        );
    }

    #[test]
    fn stream_replay_reads_cli_style_display_only_evidence() {
        use starweaver_core::ConversationId;
        use starweaver_session::RunRecord;
        use starweaver_stream::{
            DisplayMessage, DisplayMessageKind, ReplayCursorFamily, StreamArchive,
        };

        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        let service = RpcService::live(config).unwrap();
        let session = service
            .storage
            .create_session_for_product(
                Some("test".to_string()),
                Some("CLI session".to_string()),
                Some(temp.path().to_string_lossy().into_owned()),
                Some("cli"),
            )
            .unwrap();
        let run_id = RunId::from_string("run_cli_display_only");
        service
            .storage
            .begin_run(RunRecord::new(
                session.session_id.clone(),
                run_id.clone(),
                ConversationId::new(),
            ))
            .unwrap();
        let display = DisplayMessage::new(
            1,
            session.session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::RunCompleted,
        )
        .with_preview("CLI durable output");
        service.runtime.block_on(async {
            service
                .storage
                .stream_archive()
                .append_display_messages(ReplayScope::run(run_id.as_str()), vec![display])
                .await
                .unwrap();
        });

        let replay = service.handle_text(&request(
            1,
            "stream.replay",
            json!({
                "sessionId": session.session_id.as_str(),
                "runId": run_id.as_str()
            }),
        ));
        let result = &replay.response.unwrap()["result"];
        assert_eq!(result["messages"].as_array().unwrap().len(), 1);
        assert_eq!(result["messages"][0]["preview"], "CLI durable output");
        assert_eq!(result["events"].as_array().unwrap().len(), 1);
        assert_eq!(
            result["latestCursor"]["family"],
            ReplayCursorFamily::ReplayEvent.as_str()
        );

        let resumed = service.handle_text(&request(
            2,
            "run.attach",
            json!({
                "sessionId": session.session_id.as_str(),
                "runId": run_id.as_str(),
                "cursor": {
                    "family": "replay_event",
                    "scope": format!("run:{}", run_id.as_str()),
                    "sequence": 0
                }
            }),
        ));
        let resumed = resumed.response.unwrap();
        assert_eq!(resumed["result"]["events"].as_array().unwrap().len(), 1);
        assert_eq!(
            resumed["result"]["events"][0]["cursor"]["family"],
            ReplayCursorFamily::ReplayEvent.as_str()
        );
    }

    #[test]
    fn replay_source_is_stable_across_mixed_evidence_pages() {
        use starweaver_core::ConversationId;
        use starweaver_session::RunRecord;
        use starweaver_stream::{
            DisplayMessage, DisplayMessageKind, ReplayEvent, ReplayEventKind, ReplayEventLog,
            StreamArchive,
        };

        let temp = tempfile::tempdir().unwrap();
        let service = RpcService::live(RpcConfig::for_tests(temp.path())).unwrap();
        let session = service
            .storage
            .create_session_for_product(None, None, None, Some("cli"))
            .unwrap();
        let run_id = RunId::from_string("run_mixed_replay_evidence");
        let mut run = RunRecord::new(
            session.session_id.clone(),
            run_id.clone(),
            ConversationId::new(),
        );
        run.trigger_type = Some("cli".to_string());
        service.storage.begin_run(run).unwrap();
        let scope = ReplayScope::run(run_id.as_str());
        service.runtime.block_on(async {
            service
                .storage
                .stream_archive()
                .append_display_messages(
                    scope.clone(),
                    vec![
                        DisplayMessage::new(
                            0,
                            session.session_id.clone(),
                            run_id.clone(),
                            DisplayMessageKind::RunStarted,
                        ),
                        DisplayMessage::new(
                            1,
                            session.session_id.clone(),
                            run_id.clone(),
                            DisplayMessageKind::RunCompleted,
                        ),
                    ],
                )
                .await
                .unwrap();
        });

        let first = service.handle_text(&request(
            1,
            "stream.replay",
            json!({
                "sessionId": session.session_id.as_str(),
                "runId": run_id.as_str(),
                "limit": 1
            }),
        ));
        let first = first.response.unwrap();
        assert_eq!(first["result"]["events"].as_array().unwrap().len(), 1);
        assert_eq!(first["result"]["messages"].as_array().unwrap().len(), 1);
        assert_eq!(first["result"]["latestCursor"]["family"], "replay_event");

        service.runtime.block_on(async {
            service
                .storage
                .replay_event_log()
                .append(
                    scope.clone(),
                    ReplayEvent::new(scope, 0, ReplayEventKind::Heartbeat),
                )
                .await
                .unwrap();
        });

        let second = service.handle_text(&request(
            2,
            "stream.replay",
            json!({
                "sessionId": session.session_id.as_str(),
                "runId": run_id.as_str(),
                "cursor": {
                    "family": "replay_event",
                    "scope": format!("run:{}", run_id.as_str()),
                    "sequence": 0
                },
                "limit": 1
            }),
        ));
        let second = second.response.unwrap();
        assert_eq!(second["result"]["events"].as_array().unwrap().len(), 1);
        assert_eq!(second["result"]["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn terminal_subscription_drains_every_backlog_page_and_rejects_duplicate_run() {
        use starweaver_core::ConversationId;
        use starweaver_session::{RunRecord, RunStatus};
        use starweaver_stream::{DisplayMessage, DisplayMessageKind, StreamArchive};

        let temp = tempfile::tempdir().unwrap();
        let service = RpcService::live(RpcConfig::for_tests(temp.path())).unwrap();
        let session = service
            .storage
            .create_session_for_product(None, None, None, Some("cli"))
            .unwrap();
        let run_id = RunId::from_string("run_subscription_backlog");
        let mut run = RunRecord::new(
            session.session_id.clone(),
            run_id.clone(),
            ConversationId::new(),
        );
        run.trigger_type = Some("cli".to_string());
        run.status = RunStatus::Completed;
        service.storage.begin_run(run).unwrap();
        let messages = (0..300)
            .map(|sequence| {
                DisplayMessage::new(
                    sequence,
                    session.session_id.clone(),
                    run_id.clone(),
                    DisplayMessageKind::AssistantTextDelta,
                )
                .with_payload(json!({"delta": format!("chunk-{sequence}")}))
            })
            .collect();
        service.runtime.block_on(async {
            service
                .storage
                .stream_archive()
                .append_display_messages(ReplayScope::run(run_id.as_str()), messages)
                .await
                .unwrap();
        });

        let (sender, mut receiver) = mpsc::channel(512);
        let connection = service.live_connection(sender);
        connection.initialized.store(true, Ordering::Release);
        let subscribe = connection.handle_text(&request(
            1,
            "stream.subscribe",
            json!({
                "sessionId": session.session_id.as_str(),
                "runId": run_id.as_str(),
                "subscriptionId": "backlog",
                "limit": 1
            }),
        ));
        let subscribe = subscribe.response.unwrap();
        assert_eq!(subscribe["result"]["events"].as_array().unwrap().len(), 1);

        let duplicate = connection.handle_text(&request(
            2,
            "stream.subscribe",
            json!({
                "sessionId": session.session_id.as_str(),
                "runId": run_id.as_str(),
                "subscriptionId": "duplicate"
            }),
        ));
        assert_eq!(
            duplicate.response.unwrap()["error"]["code"],
            starweaver_rpc_core::ALREADY_EXISTS
        );

        connection.activate_pending_subscriptions();
        let mut stream_events = 0;
        let mut closed = false;
        for _ in 0..400 {
            let frame = service
                .runtime
                .block_on(async {
                    tokio::time::timeout(Duration::from_secs(3), receiver.recv()).await
                })
                .unwrap()
                .unwrap();
            match frame["method"].as_str() {
                Some("stream.event") => stream_events += 1,
                Some("subscription.closed") => {
                    closed = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(closed, "terminal subscription must close after draining");
        assert_eq!(stream_events, 299);
    }

    #[test]
    fn connection_subscription_registry_enforces_its_limit() {
        let mut subscriptions = HashMap::new();
        for index in 0..MAX_CONNECTION_SUBSCRIPTIONS {
            let (cancel, _) = watch::channel(false);
            let (ready, _) = watch::channel(false);
            subscriptions.insert(
                format!("sub-{index}"),
                ConnectionSubscription {
                    session_id: SessionId::from_string(format!("session-{index}")),
                    run_id: RunId::from_string(format!("run-{index}")),
                    cancel,
                    ready,
                },
            );
        }
        let error = validate_subscription_slot(
            &subscriptions,
            "overflow",
            &SessionId::from_string("session-overflow"),
            &RunId::from_string("run-overflow"),
        )
        .unwrap_err();
        assert_eq!(error.code, starweaver_rpc_core::RUN_CONFLICT);
        assert!(error.message.contains("subscription limit"));
    }

    #[test]
    fn environment_attachment_methods_manage_rpc_owned_leases() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        let service = RpcService::live(config).unwrap();
        let connection = service.connection();
        let initialized = connection.handle_text(&request(0, "initialize", json!({})));
        assert!(initialized.response.unwrap().get("result").is_some());
        let attached = connection.handle_text(&request(
            1,
            "environment.attach",
            json!({
                "attachment": {"id": "workspace", "kind": "local", "mode": "read_only"},
                "readiness": {"policy": "required"},
                "idempotencyKey": "workspace"
            }),
        ));
        let attached = attached.response.unwrap()["result"].clone();
        let lease_id = attached["attachment"]["attachmentLeaseId"]
            .as_str()
            .unwrap();
        assert_eq!(attached["attachment"]["mode"], "read_only");

        let listed = connection.handle_text(&request(2, "environment.list", json!({})));
        assert_eq!(
            listed.response.unwrap()["result"]["attachments"][0]["attachmentLeaseId"],
            lease_id
        );
        let health = connection.handle_text(&request(
            3,
            "environment.health",
            json!({"attachmentLeaseId": lease_id}),
        ));
        assert_eq!(health.response.unwrap()["result"]["status"], "ready");
        let detached = connection.handle_text(&request(
            4,
            "environment.detach",
            json!({"attachmentLeaseId": lease_id}),
        ));
        assert_eq!(detached.response.unwrap()["result"]["detached"], true);
    }

    #[test]
    fn exact_run_retry_reads_receipt_before_detached_lease_readiness() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        let service = RpcService::live(config).unwrap();
        let connection = service.connection();
        let _ = connection.handle_text(&request(0, "initialize", json!({})));
        let attached = connection
            .handle_text(&request(
                1,
                "environment.attach",
                json!({
                    "attachment": {"id": "workspace", "kind": "local"},
                    "readiness": {"policy": "required"},
                    "idempotencyKey": "detached-retry-lease"
                }),
            ))
            .response
            .unwrap();
        let lease_id = attached["result"]["attachment"]["attachmentLeaseId"]
            .as_str()
            .unwrap();
        let params = json!({
            "prompt": "hello",
            "idempotencyKey": "detached-run-retry",
            "environment": {"id": "workspace", "attachmentLeaseId": lease_id}
        });
        let first = connection
            .handle_text(&request(2, "run.start", params.clone()))
            .response
            .unwrap();
        let session_id = first["result"]["sessionId"].clone();
        let run_id = first["result"]["runId"].clone();
        let awaited = connection
            .handle_text(&request(
                3,
                "run.await",
                json!({
                    "sessionId": session_id,
                    "runId": run_id,
                    "timeoutMs": 5_000
                }),
            ))
            .response
            .unwrap();
        assert_eq!(awaited["result"]["status"]["status"], "completed");
        let detached = connection
            .handle_text(&request(
                4,
                "environment.detach",
                json!({"attachmentLeaseId": lease_id}),
            ))
            .response
            .unwrap();
        assert_eq!(detached["result"]["detached"], true);

        let replay = connection
            .handle_text(&request(5, "run.start", params))
            .response
            .unwrap();
        assert!(replay.get("error").is_none(), "{replay}");
        assert_eq!(replay["result"]["sessionId"], session_id);
        assert_eq!(replay["result"]["runId"], run_id);
        assert_eq!(replay["result"]["idempotentReplay"], true);
    }

    #[test]
    fn replay_only_transport_requires_session_scoped_environment_lease() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        let service = RpcService::replay_only(config).unwrap();
        let outcome = service.handle_text(&request(
            1,
            "environment.attach",
            json!({"attachment": {"id": "workspace", "kind": "local"}}),
        ));
        assert_eq!(
            outcome.response.unwrap()["error"]["code"],
            starweaver_rpc_core::UNSUPPORTED_FEATURE
        );
    }

    #[test]
    fn ordinary_run_start_exact_retry_returns_original_receipt_and_status() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        let service = RpcService::live(config).unwrap();
        let params = json!({
            "prompt": "hello",
            "idempotencyKey": "wire-exact-retry"
        });

        let first = service
            .handle_text(&request(1, "run.start", params.clone()))
            .response
            .unwrap();
        assert!(first.get("error").is_none(), "{first}");
        assert_eq!(first["result"]["idempotentReplay"], false);
        let session_id = first["result"]["sessionId"].as_str().unwrap();
        let run_id = first["result"]["runId"].as_str().unwrap();
        let awaited = service
            .handle_text(&request(
                2,
                "run.await",
                json!({
                    "sessionId": session_id,
                    "runId": run_id,
                    "timeoutMs": 5_000
                }),
            ))
            .response
            .unwrap();
        assert_eq!(awaited["result"]["status"]["status"], "completed");

        let replay = service
            .handle_text(&request(3, "run.start", params))
            .response
            .unwrap();
        assert_eq!(replay["result"]["sessionId"], session_id);
        assert_eq!(replay["result"]["runId"], run_id);
        assert_eq!(replay["result"]["status"], "completed");
        assert_eq!(replay["result"]["idempotentReplay"], true);

        let conflict = service
            .handle_text(&request(
                4,
                "run.start",
                json!({
                    "prompt": "different input",
                    "idempotencyKey": "wire-exact-retry"
                }),
            ))
            .response
            .unwrap();
        assert_eq!(
            conflict["error"]["code"],
            starweaver_rpc_core::IDEMPOTENCY_CONFLICT
        );
        let runs = service.runtime.block_on(
            service
                .storage
                .session_store()
                .list_runs(&SessionId::from_string(session_id)),
        );
        assert_eq!(runs.unwrap().len(), 1);
    }

    #[test]
    fn run_start_fingerprints_canonical_environment_aliases_and_rejects_drift() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        let service = RpcService::live(config).unwrap();
        let first = service
            .handle_text(&request(
                1,
                "run.start",
                json!({
                    "prompt": "hello",
                    "idempotencyKey": "environment-alias-retry",
                    "environment": {"id": "workspace", "kind": "local"}
                }),
            ))
            .response
            .unwrap();
        assert!(first.get("error").is_none(), "{first}");
        let session_id = first["result"]["sessionId"].clone();
        let run_id = first["result"]["runId"].clone();
        assert_eq!(
            first["result"]["environmentAttachments"][0]["id"],
            "workspace"
        );

        let replay = service
            .handle_text(&request(
                2,
                "run.start",
                json!({
                    "prompt": "hello",
                    "idempotencyKey": "environment-alias-retry",
                    "environmentAttachments": [
                        {
                            "id": "workspace",
                            "kind": "local",
                            "mode": "read_write",
                            "default": true
                        }
                    ]
                }),
            ))
            .response
            .unwrap();
        assert_eq!(replay["result"]["sessionId"], session_id);
        assert_eq!(replay["result"]["runId"], run_id);
        assert_eq!(replay["result"]["idempotentReplay"], true);
        assert_eq!(
            replay["result"]["environmentAttachments"],
            first["result"]["environmentAttachments"]
        );

        let conflict = service
            .handle_text(&request(
                3,
                "run.start",
                json!({
                    "prompt": "hello",
                    "idempotencyKey": "environment-alias-retry",
                    "environments": [{"id": "data", "kind": "local"}]
                }),
            ))
            .response
            .unwrap();
        assert_eq!(
            conflict["error"]["code"],
            starweaver_rpc_core::IDEMPOTENCY_CONFLICT
        );
    }

    #[test]
    fn run_prompt_materializes_multiple_rpc_environment_attachments() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        let service = RpcService::live(config).unwrap();
        let outcome = service.handle_text(&request(
            1,
            "run.prompt",
            json!({
                "prompt": "hello",
                "environmentAttachments": [
                    {
                        "id": "workspace",
                        "kind": "local",
                        "default": true,
                        "defaultForShell": true
                    },
                    {"id": "data", "kind": "local", "mode": "read_only"}
                ]
            }),
        ));
        let result = &outcome.response.unwrap()["result"];
        assert_eq!(result["status"], "completed");
        assert_eq!(
            result["environmentAttachments"].as_array().unwrap().len(),
            2
        );
        assert_eq!(result["environmentAttachments"][0]["id"], "workspace");
        assert_eq!(result["environmentAttachments"][1]["mode"], "read_only");
    }

    #[test]
    fn run_prompt_materializes_session_scoped_environment_lease() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        let service = RpcService::live(config).unwrap();
        let created = service.handle_text(&request(1, "session.create", json!({})));
        let session_id = created.response.unwrap()["result"]["session"]["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let attached = service.handle_text(&request(
            2,
            "environment.attach",
            json!({
                "scope": {"kind": "session", "sessionId": session_id},
                "attachment": {"id": "workspace", "kind": "local"}
            }),
        ));
        let lease_id = attached.response.unwrap()["result"]["attachment"]["attachmentLeaseId"]
            .as_str()
            .unwrap()
            .to_string();
        let outcome = service.handle_text(&request(
            3,
            "run.prompt",
            json!({
                "prompt": "hello",
                "sessionId": session_id,
                "environmentAttachments": [{
                    "id": "workspace",
                    "attachmentLeaseId": lease_id
                }]
            }),
        ));
        let result = &outcome.response.unwrap()["result"];
        assert_eq!(result["status"], "completed");
        assert_eq!(
            result["environmentAttachments"][0]["attachmentLeaseId"],
            lease_id
        );
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn structured_run_input_converts_durable_parts_and_rejects_prompt_ambiguity() {
        let params = json!({
            "input": {
                "parts": [
                    {"kind": "text", "text": "describe"},
                    {"kind": "image_url", "url": "https://example.com/image.png"}
                ]
            }
        });
        let (durable, input) = run_input(&params).expect("structured input");
        assert_eq!(durable.len(), 2);
        assert_eq!(input.content.len(), 2);
        assert!(matches!(durable[0], InputPart::Text { .. }));
        assert!(matches!(
            input.content[1],
            starweaver_model::ContentPart::ImageUrl { .. }
        ));

        let error = run_input(&json!({
            "prompt": "ambiguous",
            "input": {"parts": [{"kind": "text", "text": "also ambiguous"}]}
        }))
        .expect_err("prompt and structured input must conflict");
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(error.message.contains("not both"));
    }

    #[test]
    fn initialize_advertises_only_implemented_stream_and_environment_capabilities() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        let service = RpcService::live(config).unwrap();
        let initialized = service.handle_text(&request(1, "initialize", json!({})));
        let response = initialized.response.unwrap();
        let result = &response["result"];
        let capabilities = &result["capabilities"];
        assert_eq!(result["protocol"]["name"], "starweaver.host");
        assert_eq!(result["protocol"]["major"], 1);
        assert!(result.get("protocolVersion").is_none());
        assert!(result.get("protocol_version").is_none());
        assert_eq!(capabilities["streamSubscribe"], false);
        assert_eq!(capabilities["liveDisplay"], false);
        let (sender, _receiver) = mpsc::channel(8);
        let connection = service.live_connection(sender);
        let live = connection.handle_text(&request(2, "initialize", json!({})));
        let live = live.response.unwrap();
        assert_eq!(live["result"]["capabilities"]["streamSubscribe"], true);
        assert_eq!(live["result"]["capabilities"]["liveDisplay"], true);
        assert!(
            live["result"]["protocol"]["features"]
                .as_array()
                .unwrap()
                .iter()
                .any(|feature| feature == "stream.subscribe")
        );
        assert_eq!(capabilities["environmentAttachments"], true);
        assert_eq!(capabilities["environmentActiveMounts"], true);
        assert!(
            result["protocol"]["features"]
                .as_array()
                .unwrap()
                .iter()
                .any(|feature| feature == "session.search")
        );
        assert_eq!(capabilities["sessionSearch"]["available"], true);
        assert_eq!(
            capabilities["sessionSearch"]["provider"]["provider"],
            "local"
        );
    }

    #[test]
    fn model_selection_persists_by_scope_and_controls_run_profile() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = RpcConfig::for_tests(temp.path());
        config.profiles.insert(
            "coding".to_string(),
            crate::RpcProfileConfig {
                model_id: "test:coding".to_string(),
                test_response: Some("coding".to_string()),
                ..crate::RpcProfileConfig::default()
            },
        );
        let service = RpcService::live(config.clone()).unwrap();
        let selected = service.handle_text(&request(
            1,
            "model.select",
            json!({"clientStateScope": "desktop", "profile": "coding"}),
        ));
        assert_eq!(
            selected.response.unwrap()["result"]["selectedProfile"],
            "coding"
        );
        drop(service);

        let reopened = RpcService::live(config).unwrap();
        let current =
            reopened.handle_text(&request(2, "model.current", json!({"client": "desktop"})));
        assert_eq!(
            current.response.unwrap()["result"]["selectedProfile"],
            "coding"
        );
        let scoped = run_request(
            &reopened.catalog,
            &reopened.state,
            &json!({"prompt": "hello", "clientStateScope": "desktop"}),
            &[],
        )
        .unwrap();
        assert_eq!(scoped.profile, "coding");
        let default_profile = reopened.catalog.default_profile().to_string();
        let explicit = run_request(
            &reopened.catalog,
            &reopened.state,
            &json!({
                "prompt": "hello",
                "clientStateScope": "desktop",
                "profile": default_profile
            }),
            &[],
        )
        .unwrap();
        assert_eq!(explicit.profile, reopened.catalog.default_profile());

        let conflict = reopened.handle_text(&request(
            3,
            "model.current",
            json!({"clientStateScope": "desktop", "client": "tui"}),
        ));
        assert_eq!(conflict.response.unwrap()["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn model_methods_and_runs_share_rpc_scope_when_scope_is_omitted() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = RpcConfig::for_tests(temp.path());
        config.profiles.insert(
            "coding".to_string(),
            crate::RpcProfileConfig {
                model_id: "test:coding".to_string(),
                test_response: Some("coding".to_string()),
                ..crate::RpcProfileConfig::default()
            },
        );
        let service = RpcService::live(config).unwrap();
        let selected =
            service.handle_text(&request(1, "model.select", json!({"profile": "coding"})));
        let selected = selected.response.unwrap();
        assert_eq!(selected["result"]["clientStateScope"], "rpc");

        for method in ["model.current", "model.list"] {
            let outcome = service.handle_text(&request(2, method, json!({})));
            let result = &outcome.response.unwrap()["result"];
            assert_eq!(
                result["current"]["clientStateScope"]
                    .as_str()
                    .or_else(|| { result["clientStateScope"].as_str() }),
                Some("rpc")
            );
            assert_eq!(
                result["current"]["selectedProfile"]
                    .as_str()
                    .or_else(|| { result["selectedProfile"].as_str() }),
                Some("coding")
            );
        }
        let run = run_request(
            &service.catalog,
            &service.state,
            &json!({"prompt": "hello"}),
            &[],
        )
        .unwrap();
        assert_eq!(run.profile, "coding");
    }

    #[test]
    fn session_search_uses_rpc_owned_provider_and_typed_projection() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        let service = RpcService::live(config).unwrap();
        let run = service.handle_text(&request(
            1,
            "run.prompt",
            json!({"prompt": "rpc searchable literal [value]*"}),
        ));
        assert!(run.response.unwrap().get("result").is_some());
        let outcome = service.handle_text(&request(
            2,
            "session.search",
            json!({
                "query": "[value]*",
                "sources": ["run_input"],
                "granularity": "run",
                "limit": 20
            }),
        ));
        let response = outcome.response.unwrap();
        let result = &response["result"];
        assert_eq!(result["hits"].as_array().unwrap().len(), 1);
        assert_eq!(result["hits"][0]["source"], "run_input");
        assert_eq!(result["coverage"]["state"], "complete");
        assert!(result["hits"][0].get("location").is_some());
        assert!(result["hits"][0]["session"].get("state").is_none());
    }

    #[test]
    fn session_search_is_not_advertised_or_dispatched_when_disabled() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = RpcConfig::for_tests(temp.path());
        config.session_search.enabled = false;
        let service = RpcService::live(config).unwrap();
        let initialized = service.handle_text(&request(1, "initialize", json!({})));
        let result = &initialized.response.unwrap()["result"];
        assert!(
            !result["protocol"]["features"]
                .as_array()
                .unwrap()
                .iter()
                .any(|feature| feature == "session.search")
        );
        assert!(result["capabilities"]["sessionSearch"].is_null());
        let outcome = service.handle_text(&request(
            2,
            "session.search",
            json!({"query": "anything", "limit": 20}),
        ));
        assert_eq!(
            outcome.response.unwrap()["error"]["code"],
            starweaver_rpc_core::UNSUPPORTED_FEATURE
        );
    }

    #[test]
    fn initialize_rejects_wrong_host_protocol_name_and_major() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        let service = RpcService::live(config).unwrap();
        for protocol in [
            json!({"name": "starweaver.other", "major": 1, "revision": "fixture"}),
            json!({"name": "starweaver.host", "major": 2, "revision": "fixture"}),
        ] {
            let initialized =
                service.handle_text(&request(1, "initialize", json!({"protocol": protocol})));
            let response = initialized.response.unwrap();
            assert_eq!(response["error"]["code"], INVALID_PARAMS);
        }
    }

    #[test]
    fn run_prompt_executes_direct_agent_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        let service = RpcService::live(config).unwrap();
        let outcome = service.handle_text(&request(1, "run.prompt", json!({"prompt": "hello"})));
        let result = &outcome.response.unwrap()["result"];
        assert_eq!(result["status"], "completed");
        assert_eq!(result["output"], "ok");
    }
}
