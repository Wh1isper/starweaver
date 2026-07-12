//! Standalone RPC method dispatch.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};
use starweaver_core::{ProtocolIdentity, RunId, SessionId};
use starweaver_rpc_core::{
    HostInitializeParams, INVALID_PARAMS, JsonRpcOutcome, METHOD_NOT_FOUND, NOT_INITIALIZED,
    RpcError, RunInput, error_response, handle_json_rpc_text_async, host_protocol_identity,
    replay_cursor_from_params, replay_result, validate_host_initialize,
};
use starweaver_runtime::AgentInput;
use starweaver_session::{
    ApprovalStatus, ExecutionStatus, InputPart, SessionFilter, SessionStore, SessionStoreResult,
};
use starweaver_storage::SqliteStorage;
use starweaver_stream::ReplayScope;
use tokio::runtime::Runtime;
use uuid::Uuid;

use crate::{
    RpcAgentCatalog, RpcConfig, RpcHostError, RpcRunRequest, RpcRuntimeCoordinator,
    environment_manager::EnvironmentAttachmentManager,
    state::{read_current_session, write_current_session},
};

const MAX_HTTP_RUN_AWAIT: Duration = Duration::from_secs(30);

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
    notifications: RpcNotificationMode,
    runtime: Arc<Runtime>,
}

/// Per-connection host protocol negotiation state.
pub struct RpcConnection<'a> {
    service: &'a RpcService,
    initialized: AtomicBool,
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
                let result = self.service.dispatch(&method, &params).await;
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
            self.service.dispatch(&method, &params).await
        })
        .await
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
        let runtime = Runtime::new().map_err(|error| RpcHostError::Runtime(error.to_string()))?;
        Ok(Self {
            config,
            catalog,
            storage,
            coordinator,
            environment_manager,
            notifications,
            runtime: Arc::new(runtime),
        })
    }

    /// Open an uninitialized stateful protocol connection.
    #[must_use]
    pub const fn connection(&self) -> RpcConnection<'_> {
        RpcConnection {
            service: self,
            initialized: AtomicBool::new(false),
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
    async fn dispatch(&self, method: &str, params: &Value) -> Result<Value, RpcError> {
        match method {
            "initialize" => self.initialize_result(params),
            "shutdown" => Ok(json!({"status": "shutdown"})),
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
                let current = self.catalog.profile(self.catalog.default_profile())?;
                Ok(json!({
                    "profiles": self.catalog.profiles(),
                    "current": {
                        "selectedProfile": self.catalog.default_profile(),
                        "modelId": current.model_id,
                    },
                }))
            }
            "profile.get" => {
                let name = required_string(params, "name")?;
                let profile = self.catalog.profile(&name)?;
                Ok(json!({
                    "name": name,
                    "profile": profile,
                }))
            }
            "model.current" => {
                let profile = self.catalog.profile(self.catalog.default_profile())?;
                Ok(json!({
                    "selectedProfile": self.catalog.default_profile(),
                    "modelId": profile.model_id,
                }))
            }
            "model.select" => {
                let profile_name = required_string(params, "profile")?;
                let profile = self.catalog.profile(&profile_name)?;
                Ok(json!({
                    "client": params.get("client").and_then(Value::as_str).unwrap_or("rpc"),
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
                let session = run_storage(self.storage.clone(), move |storage| {
                    storage.create_session(Some(profile), title)
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
                "sessionId": read_current_session(&self.config.state_dir)
                    .await
                    .map_err(rpc_error)?,
            })),
            "session.current.set" => {
                let session_id = required_string(params, "sessionId")?;
                self.storage
                    .session_store()
                    .load_session(&SessionId::from_string(session_id.clone()))
                    .await
                    .map_err(rpc_error)?;
                write_current_session(&self.config.state_dir, &session_id)
                    .await
                    .map_err(rpc_error)?;
                Ok(json!({"sessionId": session_id}))
            }
            "session.delete" => {
                let session_id = SessionId::from_string(required_string(params, "sessionId")?);
                let storage_session_id = session_id.clone();
                let deleted = run_storage(self.storage.clone(), move |storage| {
                    storage.delete_session(&storage_session_id)
                })
                .await?;
                Ok(json!({"sessionId": session_id, "deleted": deleted}))
            }
            "run.start" => self.run_start(params).await,
            "run.prompt" => self.run_prompt(params).await,
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
                let timeout = if self.notifications == RpcNotificationMode::ReplayOnly {
                    Some(
                        requested
                            .unwrap_or(MAX_HTTP_RUN_AWAIT)
                            .min(MAX_HTTP_RUN_AWAIT),
                    )
                } else {
                    requested
                };
                let status = self
                    .coordinator
                    .await_terminal(&session_id, &run_id, timeout)
                    .await
                    .map_err(rpc_error)?;
                Ok(json!({"status": status}))
            }
            "run.cancel" => {
                let run_id = RunId::from_string(required_string(params, "runId")?);
                self.coordinator
                    .cancel(
                        &run_id,
                        params
                            .get("reason")
                            .and_then(Value::as_str)
                            .map(ToString::to_string),
                    )
                    .map_err(rpc_error)
            }
            "run.steer" => {
                let run_id = RunId::from_string(required_string(params, "runId")?);
                let text = required_string(params, "text")?;
                let steering_id = params
                    .get("steeringId")
                    .and_then(Value::as_str)
                    .map_or_else(
                        || format!("steering_{}", Uuid::new_v4()),
                        ToString::to_string,
                    );
                self.coordinator
                    .steer(&run_id, steering_id, text)
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
                self.environment_manager.attach(params).await
            }
            "environment.detach" => self.environment_manager.detach(params),
            "environment.list" => self.environment_manager.list(params),
            "environment.health" => self.environment_manager.health(params).await,
            "environment.active_mount" => self.environment_active_mount(params).await,
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

    async fn environment_active_mount(&self, params: &Value) -> Result<Value, RpcError> {
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
            .materialize_active_attachment(params.attachment.clone(), Some(&session_id))
            .await?;
        self.environment_manager
            .mark_run_mounted(&params.run_id, &attachment)?;
        match self
            .coordinator
            .active_environment_mount(&params, attachment.clone(), &params_digest)
            .await
        {
            Ok(outcome) => {
                if !outcome.applied {
                    self.environment_manager
                        .mark_run_unmounted(&params.run_id, &attachment)?;
                }
                Ok(outcome.result)
            }
            Err(error) => {
                match self
                    .environment_manager
                    .mark_run_unmounted(&params.run_id, &attachment)
                {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(RpcError::new(
                        starweaver_rpc_core::SERVER_ERROR,
                        format!(
                            "{}; active environment lease rollback failed: {}",
                            error.message, cleanup.message
                        ),
                    )),
                }
            }
        }
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
        let outcome = self
            .coordinator
            .active_environment_unmount(&params, &params_digest)
            .await?;
        if outcome.applied {
            self.environment_manager
                .mark_run_unmounted(&params.run_id, &outcome.removed)?;
        }
        Ok(outcome.result)
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

    async fn run_start(&self, params: &Value) -> Result<Value, RpcError> {
        let started = self.start_run_from_params(params).await?;
        Ok(json!({
            "sessionId": started.session_id,
            "runId": started.run_id,
            "status": "running",
            "payloadFormat": "display",
            "environmentAttachments": started.environment_attachments,
        }))
    }

    async fn run_prompt(&self, params: &Value) -> Result<Value, RpcError> {
        let started = self.start_run_from_params(params).await?;
        let timeout =
            (self.notifications == RpcNotificationMode::ReplayOnly).then_some(MAX_HTTP_RUN_AWAIT);
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

    async fn start_run_from_params(
        &self,
        params: &Value,
    ) -> Result<crate::RpcStartedRun, RpcError> {
        let mut request = run_request(&self.catalog, params)?;
        let refs = starweaver_rpc_core::environment_attachment_refs(params)?;
        let materialized = self
            .environment_manager
            .materialize_run_attachments(refs, request.session_id.as_ref().map(SessionId::as_str))
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
                let _receipt = self.coordinator.cancel(
                    &started.run_id,
                    Some("environment lease reservation cleanup failed".to_string()),
                );
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
            .replay(&run_id, cursor, None)
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
            .replay(&run_id, cursor.clone(), limit)
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

    fn initialize_result(&self, params: &Value) -> Result<Value, RpcError> {
        let params =
            serde_json::from_value::<HostInitializeParams>(params.clone()).map_err(|error| {
                RpcError::new(
                    INVALID_PARAMS,
                    format!("invalid initialize params: {error}"),
                )
            })?;
        validate_host_initialize(&params)?;
        let live = self.notifications == RpcNotificationMode::Live;
        Ok(json!({
            "protocol": host_protocol_identity(),
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
                "streamSubscribe": false,
                "cancel": true,
                "steering": true,
                "attach": true,
                "environmentAttachments": true,
                "environmentActiveMounts": true,
                "defaultStreamPayload": "display_message",
                "approvals": true,
                "deferred": true,
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

fn run_request(catalog: &RpcAgentCatalog, params: &Value) -> Result<RpcRunRequest, RpcError> {
    let (durable_input, input) = run_input(params)?;
    let profile = params
        .get("profile")
        .or_else(|| params.get("modelProfile"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| catalog.default_profile())
        .to_string();
    catalog.profile(&profile)?;
    Ok(RpcRunRequest {
        durable_input,
        input,
        session_id: optional_session_id(params, "sessionId"),
        restore_from_run_id: params
            .get("restoreFromRunId")
            .or_else(|| params.get("runId"))
            .and_then(Value::as_str)
            .map(|value| RunId::from_string(value.to_string())),
        profile,
        environment_attachments: Vec::new(),
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
            "protocol": host_protocol_identity(),
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
    fn environment_attachment_methods_manage_rpc_owned_leases() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        let service = RpcService::live(config).unwrap();
        let attached = service.handle_text(&request(
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

        let listed = service.handle_text(&request(2, "environment.list", json!({})));
        assert_eq!(
            listed.response.unwrap()["result"]["attachments"][0]["attachmentLeaseId"],
            lease_id
        );
        let health = service.handle_text(&request(
            3,
            "environment.health",
            json!({"attachmentLeaseId": lease_id}),
        ));
        assert_eq!(health.response.unwrap()["result"]["status"], "ready");
        let detached = service.handle_text(&request(
            4,
            "environment.detach",
            json!({"attachmentLeaseId": lease_id}),
        ));
        assert_eq!(detached.response.unwrap()["result"]["detached"], true);
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
        assert_eq!(capabilities["environmentAttachments"], true);
        assert_eq!(capabilities["environmentActiveMounts"], true);
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
