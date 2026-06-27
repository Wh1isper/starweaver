//! JSON-RPC host service and local transports.

mod environment_manager;
mod transport;

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};

use self::environment_manager::EnvironmentAttachmentManager;
use serde_json::{json, Value};
use starweaver_rpc_core::{
    attachment_result, environment_attachment_refs, environment_attachment_result,
    handle_json_rpc_text_async, notification, output_item, replay_cursor_from_params,
    replay_result, stream_payload_format, EnvironmentActiveListParams,
    EnvironmentActiveMountParams, EnvironmentActiveUnmountParams, RpcError, StreamPayloadFormat,
    INVALID_PARAMS, METHOD_NOT_FOUND, RUN_CONFLICT, SERVER_ERROR, UNSUPPORTED_FEATURE,
};
use starweaver_stream::ReplayScope;
use tokio::runtime::Runtime;
use uuid::Uuid;

use crate::{
    args::{HitlPolicy, OutputMode, RpcCommand, RpcTransport, RunCommand},
    client_state,
    config::{get_config_value, read_current_session, write_current_session, CliConfig},
    local_store::{LocalStore, LocalStreamArchive},
    profiles::{list_config_model_profiles, list_profiles, show_profile},
    runner::CliSteeringMessage,
    runtime_coordinator::{CliRuntimeCoordinator, RunAttachment, RunStreamEvent, StartedRun},
    CliError, CliResult, CliService,
};

const PROTOCOL_VERSION: &str = "2026-06-08";

impl From<CliError> for RpcError {
    fn from(error: CliError) -> Self {
        Self::new(SERVER_ERROR, error.to_string())
    }
}

/// Run the selected JSON-RPC host transport.
pub fn run(config: &CliConfig, command: &RpcCommand) -> CliResult<()> {
    match command.transport {
        RpcTransport::Stdio => transport::run_stdio(config),
        RpcTransport::Http => transport::run_http(config, &command.host, command.port),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RpcNotificationMode {
    Live,
    ReplayOnly,
}

impl RpcNotificationMode {
    const fn supports_live_notifications(self) -> bool {
        matches!(self, Self::Live)
    }
}

struct RpcService {
    config: CliConfig,
    coordinator: CliRuntimeCoordinator,
    output_sender: mpsc::Sender<Value>,
    subscriptions: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    notifications: RpcNotificationMode,
    runtime: Arc<Runtime>,
    environment_manager: EnvironmentAttachmentManager,
}

impl RpcService {
    fn new(config: CliConfig, output_sender: mpsc::Sender<Value>) -> Self {
        Self::with_notification_mode(config, output_sender, RpcNotificationMode::Live)
    }

    fn replay_only(config: CliConfig, output_sender: mpsc::Sender<Value>) -> Self {
        Self::with_notification_mode(config, output_sender, RpcNotificationMode::ReplayOnly)
    }

    fn with_notification_mode(
        config: CliConfig,
        output_sender: mpsc::Sender<Value>,
        notifications: RpcNotificationMode,
    ) -> Self {
        let environment_manager = EnvironmentAttachmentManager::new();
        Self {
            coordinator: CliRuntimeCoordinator::new(config.clone()),
            config,
            output_sender,
            subscriptions: Arc::new(Mutex::new(HashMap::new())),
            notifications,
            environment_manager,
            runtime: build_rpc_runtime(),
        }
    }

    fn handle_line(&self, line: &str) -> (Option<Value>, bool) {
        self.handle_text(line)
    }

    fn handle_text(&self, text: &str) -> (Option<Value>, bool) {
        let outcome = self.runtime.block_on(self.handle_text_async(text));
        (outcome.response, outcome.shutdown)
    }

    async fn handle_text_async(&self, text: &str) -> starweaver_rpc_core::JsonRpcOutcome {
        handle_json_rpc_text_async(text, |method, params| async move {
            self.dispatch_async(&method, &params).await
        })
        .await
    }

    #[allow(clippy::too_many_lines)]
    async fn dispatch_async(&self, method: &str, params: &Value) -> Result<Value, RpcError> {
        let config = &self.config;
        match method {
            "initialize" => Ok(initialize_result(config, self.notifications)),
            "shutdown" => Ok(json!({"status": "shutdown"})),
            "profile.list" => Ok(json!({
                "profiles": list_profiles(config),
                "current": selected_profile_result(config, params.get("client").and_then(Value::as_str))?,
            })),
            "model.list" => Ok(json!({
                "profiles": list_config_model_profiles(config),
                "current": selected_model_profile_result(config, params.get("client").and_then(Value::as_str))?,
            })),
            "profile.get" => {
                let name = required_string(params, "name")?;
                let yaml = show_profile(config, &name).map_err(RpcError::from)?;
                Ok(json!({"name": name, "profile": yaml}))
            }
            "model.current" => {
                selected_model_profile_result(config, params.get("client").and_then(Value::as_str))
            }
            "model.select" => {
                let profile = required_string(params, "profile")?;
                ensure_client_model_profile(config, &profile)?;
                let client = params
                    .get("client")
                    .and_then(Value::as_str)
                    .unwrap_or("tui");
                client_state::write_selected_profile(config, client, &profile)
                    .map_err(RpcError::from)?;
                Ok(json!({
                    "client": client,
                    "selectedProfile": profile,
                    "modelId": model_id_for_profile(config, &profile),
                }))
            }
            "config.get" => config_get(config, params),
            "diagnostics.get" => Ok(json!({
                "sdk": starweaver_core::sdk_name(),
                "version": env!("CARGO_PKG_VERSION"),
                "globalDir": config.global_dir,
                "projectDir": config.project_dir,
                "tuiStateDir": config.tui_state_dir,
                "desktopStateDir": config.desktop_state_dir,
                "databasePath": config.database_path,
                "defaultProfile": config.default_profile,
                "profiles": list_profiles(config).len(),
            })),
            "session.create" => {
                let profile = params
                    .get("profile")
                    .and_then(Value::as_str)
                    .unwrap_or(&config.default_profile);
                let title = params
                    .get("title")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let mut store = LocalStore::open(config).map_err(RpcError::from)?;
                let session = store
                    .create_session(profile, title)
                    .map_err(RpcError::from)?;
                Ok(json!({"session": session}))
            }
            "session.list" => {
                let limit = params
                    .get("limit")
                    .and_then(Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .unwrap_or(50);
                let store = LocalStore::open(config).map_err(RpcError::from)?;
                let sessions = store.list_sessions(limit).map_err(RpcError::from)?;
                Ok(json!({"sessions": sessions}))
            }
            "session.get" => {
                let session_id = required_string(params, "sessionId")?;
                let runs_limit = params
                    .get("runs")
                    .and_then(Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .unwrap_or(20);
                let store = LocalStore::open(config).map_err(RpcError::from)?;
                let session = store.load_session(&session_id).map_err(RpcError::from)?;
                let runs = store
                    .list_runs(&session_id, runs_limit)
                    .map_err(RpcError::from)?;
                Ok(json!({"session": session, "runs": runs}))
            }
            "session.current.get" => Ok(json!({
                "sessionId": read_current_session(config).map_err(RpcError::from)?,
            })),
            "session.current.set" => {
                let session_id = required_string(params, "sessionId")?;
                write_current_session(config, &session_id).map_err(RpcError::from)?;
                Ok(json!({"sessionId": session_id}))
            }
            "session.replay" | "stream.replay" => self.stream_replay(params),
            "session.delete" => {
                let session_id = required_string(params, "sessionId")?;
                let mut store = LocalStore::open(config).map_err(RpcError::from)?;
                let deleted = store.delete_session(&session_id).map_err(RpcError::from)?;
                Ok(json!({"sessionId": session_id, "deleted": deleted}))
            }
            "session.output" => self.session_output(params),
            "stream.subscribe" => self.stream_subscribe(params),
            "stream.unsubscribe" => self.stream_unsubscribe(params),
            "run.prompt" => self.run_prompt(params).await,
            "run.start" => self.run_start(params).await,
            "run.attach" => self.run_attach(params),
            "run.status" => self.run_status(params),
            "run.await" => self.run_await(params),
            "run.cancel" => self.run_cancel(params),
            "run.steer" => self.run_steer(params),
            "session.steer" => self.session_steer(params),
            "approval.list" => {
                let store = LocalStore::open(config).map_err(RpcError::from)?;
                let records = store
                    .list_approvals(
                        params.get("sessionId").and_then(Value::as_str),
                        params.get("runId").and_then(Value::as_str),
                    )
                    .map_err(RpcError::from)?;
                Ok(json!({"approvals": records}))
            }
            "approval.show" => {
                let approval_id = required_string(params, "approvalId")?;
                let store = LocalStore::open(config).map_err(RpcError::from)?;
                let approval = store.load_approval(&approval_id).map_err(RpcError::from)?;
                Ok(json!({"approval": approval}))
            }
            "approval.decide" => {
                let approval_id = required_string(params, "approvalId")?;
                let status = match required_string(params, "status")?.as_str() {
                    "approved" | "approve" => starweaver_session::ApprovalStatus::Approved,
                    "denied" | "rejected" | "reject" => starweaver_session::ApprovalStatus::Denied,
                    other => {
                        return Err(RpcError::new(
                            INVALID_PARAMS,
                            format!("unknown approval status: {other}"),
                        ))
                    }
                };
                let reason = params
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let mut store = LocalStore::open(config).map_err(RpcError::from)?;
                let approval = store
                    .decide_approval(&approval_id, status, reason)
                    .map_err(RpcError::from)?;
                Ok(json!({"approval": approval}))
            }
            "deferred.list" => {
                let store = LocalStore::open(config).map_err(RpcError::from)?;
                let records = store
                    .list_deferred_tools(
                        params.get("sessionId").and_then(Value::as_str),
                        params.get("runId").and_then(Value::as_str),
                    )
                    .map_err(RpcError::from)?;
                Ok(json!({"deferred": records}))
            }
            "deferred.show" => {
                let deferred_id = required_string(params, "deferredId")?;
                let store = LocalStore::open(config).map_err(RpcError::from)?;
                let deferred = store
                    .load_deferred_tool(&deferred_id)
                    .map_err(RpcError::from)?;
                Ok(json!({"deferred": deferred}))
            }
            "deferred.complete" => {
                let deferred_id = required_string(params, "deferredId")?;
                let result = params.get("result").cloned().unwrap_or(Value::Null);
                let mut store = LocalStore::open(config).map_err(RpcError::from)?;
                let deferred = store
                    .complete_deferred_tool(&deferred_id, result)
                    .map_err(RpcError::from)?;
                Ok(json!({"deferred": deferred}))
            }
            "deferred.fail" => {
                let deferred_id = required_string(params, "deferredId")?;
                let error = required_string(params, "error")?;
                let mut store = LocalStore::open(config).map_err(RpcError::from)?;
                let deferred = store
                    .fail_deferred_tool(&deferred_id, &error)
                    .map_err(RpcError::from)?;
                Ok(json!({"deferred": deferred}))
            }
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
            other => Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("method not found: {other}"),
            )),
        }
    }

    async fn environment_active_mount(&self, params: &Value) -> Result<Value, RpcError> {
        let params = serde_json::from_value::<EnvironmentActiveMountParams>(params.clone())
            .map_err(|error| {
                RpcError::new(
                    INVALID_PARAMS,
                    format!("invalid active mount params: {error}"),
                )
            })?;
        let session_id = self
            .coordinator
            .active_run_session_id(&params.run_id)
            .map_err(RpcError::from)?;
        let attachment = self
            .environment_manager
            .materialize_active_attachment(params.attachment.clone(), Some(&session_id))
            .await?;
        self.environment_manager
            .mark_run_mounted(&params.run_id, &attachment)?;
        match self
            .coordinator
            .active_environment_mount(&params, attachment.clone())
        {
            Ok(result) => Ok(result),
            Err(error) => {
                let _ = self
                    .environment_manager
                    .mark_run_unmounted(&params.run_id, &attachment);
                Err(error)
            }
        }
    }

    async fn environment_active_unmount(&self, params: &Value) -> Result<Value, RpcError> {
        let params = serde_json::from_value::<EnvironmentActiveUnmountParams>(params.clone())
            .map_err(|error| {
                RpcError::new(
                    INVALID_PARAMS,
                    format!("invalid active unmount params: {error}"),
                )
            })?;
        let (result, removed) = self.coordinator.active_environment_unmount(&params).await?;
        self.environment_manager
            .mark_run_unmounted(&params.run_id, &removed)?;
        Ok(result)
    }

    fn environment_active_list(&self, params: &Value) -> Result<Value, RpcError> {
        let params = serde_json::from_value::<EnvironmentActiveListParams>(params.clone())
            .map_err(|error| {
                RpcError::new(
                    INVALID_PARAMS,
                    format!("invalid active list params: {error}"),
                )
            })?;
        self.coordinator
            .active_environment_list(&params.run_id)
            .map_err(active_run_rpc_error)
    }

    async fn run_start(&self, params: &Value) -> Result<Value, RpcError> {
        let format = stream_payload_format(params)?;
        let mut command = run_command_from_params(&self.config, params, OutputMode::Json)?;
        let run_session_id = command.session.clone();
        command.environment_attachments = self
            .environment_manager
            .materialize_run_attachments(command.environment_attachments, run_session_id.as_deref())
            .await?;
        let environment_attachments = command.environment_attachments.clone();
        let pending_environment_run_id = pending_active_environment_run_id();
        self.environment_manager
            .mark_run_started(&pending_environment_run_id, &environment_attachments)?;
        let started = match self
            .coordinator
            .start_run(command, None)
            .map_err(RpcError::from)
        {
            Ok(started) => started,
            Err(error) => {
                self.environment_manager
                    .mark_run_finished(&pending_environment_run_id)?;
                return Err(error);
            }
        };
        let session_id = started.session_id.clone();
        let run_id = started.run_id.clone();
        if let Err(error) = self
            .environment_manager
            .mark_run_started(&run_id, &environment_attachments)
        {
            let _ = self
                .environment_manager
                .mark_run_finished(&pending_environment_run_id);
            return Err(error);
        }
        self.environment_manager
            .mark_run_finished(&pending_environment_run_id)?;
        self.spawn_run_notifications(started, format);
        let mut result = json!({
            "sessionId": session_id,
            "runId": run_id,
            "status": "running",
            "payloadFormat": format.as_str(),
        });
        if !environment_attachments.is_empty() {
            merge_object(
                &mut result,
                &environment_attachment_result(&environment_attachments),
            );
        }
        Ok(result)
    }

    async fn run_prompt(&self, params: &Value) -> Result<Value, RpcError> {
        let mut command = run_command_from_params(&self.config, params, OutputMode::Json)?;
        let run_session_id = command.session.clone();
        command.environment_attachments = self
            .environment_manager
            .materialize_run_attachments(command.environment_attachments, run_session_id.as_deref())
            .await?;
        let active_environment_run_id = prompt_active_environment_run_id();
        self.environment_manager
            .mark_run_started(&active_environment_run_id, &command.environment_attachments)?;
        let config = self.config.clone();
        let join_result =
            tokio::task::spawn_blocking(move || CliService::open(config)?.run_prompt(&command))
                .await;
        let finish_result = self
            .environment_manager
            .mark_run_finished(&active_environment_run_id);
        let output_result = match join_result {
            Ok(result) => result.map_err(RpcError::from),
            Err(error) => Err(RpcError::new(SERVER_ERROR, error.to_string())),
        };
        let output = output_result?;
        finish_result?;
        serde_json::from_str(output.trim())
            .map_err(|error| RpcError::new(SERVER_ERROR, error.to_string()))
    }

    fn run_attach(&self, params: &Value) -> Result<Value, RpcError> {
        let session_id = required_string(params, "sessionId")?;
        let run_id = required_string(params, "runId")?;
        let format = stream_payload_format(params)?;
        let cursor = replay_cursor_from_params(params, ReplayScope::run(&run_id))?;
        let mut attachment = self
            .coordinator
            .attach_run(&session_id, &run_id, cursor.as_ref())
            .map_err(RpcError::from)?;
        let result = attachment_result(
            &attachment.session_id,
            attachment.run_id.as_deref(),
            attachment.active,
            &attachment.events,
            format,
        );
        self.spawn_attachment_notifications(&mut attachment, format);
        Ok(result)
    }

    fn session_output(&self, params: &Value) -> Result<Value, RpcError> {
        let session_id = required_string(params, "sessionId")?;
        let run_id = params.get("runId").and_then(Value::as_str);
        let format = stream_payload_format(params)?;
        let scope = run_id.map_or_else(|| ReplayScope::session(&session_id), ReplayScope::run);
        let cursor = replay_cursor_from_params(params, scope)?;
        let mut attachment = self
            .coordinator
            .session_output(&session_id, run_id, cursor.as_ref())
            .map_err(RpcError::from)?;
        let result = attachment_result(
            &attachment.session_id,
            attachment.run_id.as_deref(),
            attachment.active,
            &attachment.events,
            format,
        );
        self.spawn_attachment_notifications(&mut attachment, format);
        Ok(result)
    }

    fn stream_replay(&self, params: &Value) -> Result<Value, RpcError> {
        let session_id = required_string(params, "sessionId")?;
        let run_id = params.get("runId").and_then(Value::as_str);
        let scope = run_id.map_or_else(|| ReplayScope::session(&session_id), ReplayScope::run);
        let cursor = replay_cursor_from_params(params, scope)?;
        let archive = LocalStreamArchive::new(self.config.clone());
        let window = archive
            .replay_display_window(&session_id, run_id, cursor.as_ref())
            .map_err(RpcError::from)?;
        Ok(replay_result(
            &session_id,
            run_id,
            &window.scope,
            &window.events,
            cursor.as_ref(),
            window.next_sequence,
        ))
    }

    fn stream_subscribe(&self, params: &Value) -> Result<Value, RpcError> {
        if !self.notifications.supports_live_notifications() {
            return Err(RpcError::new(
                UNSUPPORTED_FEATURE,
                "stream.subscribe requires a live notification transport",
            ));
        }
        let subscription_id = subscription_id(params);
        let session_id = required_string(params, "sessionId")?;
        let run_id = params.get("runId").and_then(Value::as_str);
        let format = stream_payload_format(params)?;
        let scope = run_id.map_or_else(|| ReplayScope::session(&session_id), ReplayScope::run);
        let cursor = replay_cursor_from_params(params, scope)?;
        let mut attachment = if let Some(run_id) = run_id {
            self.coordinator
                .attach_run(&session_id, run_id, cursor.as_ref())
                .map_err(RpcError::from)?
        } else {
            self.coordinator
                .session_output(&session_id, None, cursor.as_ref())
                .map_err(RpcError::from)?
        };
        let mut result = attachment_result(
            &attachment.session_id,
            attachment.run_id.as_deref(),
            attachment.active,
            &attachment.events,
            format,
        );
        insert_subscription_id(&mut result, &subscription_id);
        if attachment.subscription.is_some() {
            let cancel = Arc::new(AtomicBool::new(false));
            let mut subscriptions = self.subscriptions.lock().map_err(|error| {
                RpcError::new(
                    SERVER_ERROR,
                    format!("subscription registry poisoned: {error}"),
                )
            })?;
            if subscriptions.contains_key(&subscription_id) {
                return Err(RpcError::new(
                    SERVER_ERROR,
                    format!("subscription already exists: {subscription_id}"),
                ));
            }
            subscriptions.insert(subscription_id.clone(), Arc::clone(&cancel));
            drop(subscriptions);
            self.spawn_stream_subscription_notifications(
                &mut attachment,
                format,
                subscription_id,
                cancel,
            );
        }
        Ok(result)
    }

    fn stream_unsubscribe(&self, params: &Value) -> Result<Value, RpcError> {
        let subscription_id = required_string(params, "subscriptionId")?;
        let subscription = self
            .subscriptions
            .lock()
            .map_err(|error| {
                RpcError::new(
                    SERVER_ERROR,
                    format!("subscription registry poisoned: {error}"),
                )
            })?
            .remove(&subscription_id);
        let unsubscribed = subscription.is_some();
        if let Some(cancel) = subscription {
            cancel.store(true, Ordering::SeqCst);
        }
        Ok(json!({
            "subscriptionId": subscription_id,
            "unsubscribed": unsubscribed,
        }))
    }

    fn run_status(&self, params: &Value) -> Result<Value, RpcError> {
        let session_id = required_string(params, "sessionId")?;
        let run_id = required_string(params, "runId")?;
        let status = self
            .coordinator
            .run_status(&session_id, &run_id)
            .map_err(RpcError::from)?;
        Ok(json!({"status": status}))
    }

    fn run_cancel(&self, params: &Value) -> Result<Value, RpcError> {
        let run_id = required_string(params, "runId")?;
        self.coordinator
            .cancel_run(&run_id)
            .map_err(RpcError::from)?;
        Ok(json!({"runId": run_id, "cancelled": true}))
    }

    fn run_steer(&self, params: &Value) -> Result<Value, RpcError> {
        let run_id = required_string(params, "runId")?;
        let text = required_string(params, "text")?;
        let id = steering_id(params);
        self.coordinator
            .steer_run(
                &run_id,
                CliSteeringMessage {
                    id: id.clone(),
                    text,
                },
            )
            .map_err(RpcError::from)?;
        Ok(json!({"runId": run_id, "steeringId": id, "queued": true}))
    }

    fn session_steer(&self, params: &Value) -> Result<Value, RpcError> {
        let session_id = required_string(params, "sessionId")?;
        let text = required_string(params, "text")?;
        let id = steering_id(params);
        let run_id = self
            .coordinator
            .steer_session(
                &session_id,
                CliSteeringMessage {
                    id: id.clone(),
                    text,
                },
            )
            .map_err(RpcError::from)?;
        Ok(json!({
            "sessionId": session_id,
            "runId": run_id,
            "steeringId": id,
            "queued": true,
        }))
    }

    fn run_await(&self, params: &Value) -> Result<Value, RpcError> {
        let session_id = required_string(params, "sessionId")?;
        let run_id = required_string(params, "runId")?;
        let timeout = params
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .map(Duration::from_millis);
        let attachment = self
            .coordinator
            .attach_run(&session_id, &run_id, None)
            .map_err(RpcError::from)?;
        let Some(receiver) = attachment.subscription else {
            return self.run_status(params);
        };
        loop {
            let event = match timeout {
                Some(timeout) => receiver
                    .recv_timeout(timeout)
                    .map_err(|error| RpcError::new(SERVER_ERROR, error.to_string()))?,
                None => receiver
                    .recv()
                    .map_err(|error| RpcError::new(SERVER_ERROR, error.to_string()))?,
            };
            if let RunStreamEvent::Status(status) = event {
                if status_is_terminal(&status.status) {
                    return Ok(json!({"status": status}));
                }
            }
        }
    }

    fn spawn_run_notifications(&self, started: StartedRun, format: StreamPayloadFormat) {
        let output_sender = self.output_sender.clone();
        let environment_manager = self.environment_manager.clone();
        let send_notifications = self.notifications.supports_live_notifications();
        thread::spawn(move || {
            let session_id = started.session_id.clone();
            let run_id = started.run_id.clone();
            if send_notifications {
                let _ = output_sender.send(notification(
                    "run.started",
                    &json!({
                        "sessionId": session_id,
                        "runId": run_id,
                        "status": "running",
                    }),
                ));
            }
            forward_started_run_events(
                &output_sender,
                started.events,
                format,
                send_notifications,
                &environment_manager,
                &run_id,
            );
        });
    }

    fn validate_environment_attach_scope(&self, params: &Value) -> Result<(), RpcError> {
        if self.notifications.supports_live_notifications() {
            return Ok(());
        }
        let scope_kind = params
            .get("scope")
            .and_then(|scope| scope.get("kind"))
            .and_then(Value::as_str)
            .unwrap_or("connection");
        if scope_kind == "connection" {
            return Err(RpcError::new(
                UNSUPPORTED_FEATURE,
                "connection-scoped environment attachments are not supported by replay-only transports; use session scope",
            ));
        }
        Ok(())
    }

    fn spawn_attachment_notifications(
        &self,
        attachment: &mut RunAttachment,
        format: StreamPayloadFormat,
    ) {
        if !self.notifications.supports_live_notifications() {
            return;
        }
        let Some(subscription) = attachment.subscription.take() else {
            return;
        };
        let output_sender = self.output_sender.clone();
        thread::spawn(move || forward_run_events(&output_sender, subscription, format));
    }

    fn spawn_stream_subscription_notifications(
        &self,
        attachment: &mut RunAttachment,
        format: StreamPayloadFormat,
        subscription_id: String,
        cancel: Arc<AtomicBool>,
    ) {
        if !self.notifications.supports_live_notifications() {
            return;
        }
        let Some(subscription) = attachment.subscription.take() else {
            return;
        };
        let output_sender = self.output_sender.clone();
        let subscriptions = Arc::clone(&self.subscriptions);
        thread::spawn(move || {
            forward_subscription_events(
                &output_sender,
                &subscription,
                format,
                &subscription_id,
                &cancel,
            );
            if let Ok(mut subscriptions) = subscriptions.lock() {
                subscriptions.remove(&subscription_id);
            }
        });
    }
}

fn build_rpc_runtime() -> Arc<Runtime> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("starweaver-rpc")
        .build();
    match runtime {
        Ok(runtime) => Arc::new(runtime),
        Err(error) => panic!("failed to build RPC async runtime: {error}"),
    }
}

fn run_command_from_params(
    config: &CliConfig,
    params: &Value,
    output: OutputMode,
) -> Result<RunCommand, RpcError> {
    let prompt = required_string(params, "prompt")?;
    let environment_attachments = environment_attachment_refs(params)?;
    validate_environment_attachments_for_run(&environment_attachments)?;
    validate_run_session_selectors(params)?;
    let client = params.get("client").and_then(Value::as_str);
    let client_profile = client
        .map(|client| client_state::read_selected_profile(config, client).map_err(RpcError::from))
        .transpose()?
        .flatten();
    let profile = params
        .get("profile")
        .or_else(|| params.get("modelProfile"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or(client_profile)
        .unwrap_or_else(|| config.default_profile.clone());
    ensure_profile(config, &profile)?;
    Ok(RunCommand {
        prompt: Some(prompt),
        prompt_parts: Vec::new(),
        session: params
            .get("sessionId")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        continue_session: params
            .get("continueLatest")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        new_session: params
            .get("newSession")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        run: params
            .get("restoreFromRunId")
            .or_else(|| params.get("runId"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        branch_from: params
            .get("branchFromRunId")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        profile: Some(profile),
        worker: None,
        worker_label: None,
        worktree: None,
        worktree_name: None,
        branch: None,
        output: Some(output),
        hitl: params
            .get("hitl")
            .and_then(Value::as_str)
            .and_then(parse_hitl),
        goal: None,
        session_affinity_id: None,
        environment_attachments,
    })
}

fn validate_run_session_selectors(params: &Value) -> Result<(), RpcError> {
    let session_id = params.get("sessionId").and_then(Value::as_str).is_some();
    let continue_latest = params
        .get("continueLatest")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let new_session = params
        .get("newSession")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let selected =
        usize::from(session_id) + usize::from(continue_latest) + usize::from(new_session);
    if selected > 1 {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "run session selectors are mutually exclusive: use only one of sessionId, continueLatest, or newSession",
        ));
    }
    Ok(())
}

fn validate_environment_attachments_for_run(
    attachments: &[starweaver_rpc_core::EnvironmentAttachmentRef],
) -> Result<(), RpcError> {
    for attachment in attachments {
        match attachment.kind.as_str() {
            "local" => {}
            "envd" => {
                let Some(endpoint) = attachment.requested_endpoint_ref() else {
                    return Err(RpcError::new(
                        INVALID_PARAMS,
                        "envd environment attachment requires endpointRef",
                    ));
                };
                validate_envd_auth_token(attachment.requested_auth_token())?;
                if !endpoint.starts_with("http://") {
                    return Err(RpcError::new(
                        UNSUPPORTED_FEATURE,
                        "envd environment attachment currently supports http:// endpoint refs",
                    ));
                }
            }
            other => {
                return Err(RpcError::new(
                    UNSUPPORTED_FEATURE,
                    format!("unsupported environment attachment kind: {other}"),
                ));
            }
        }
    }
    Ok(())
}

fn validate_envd_auth_token(auth_token: Option<&str>) -> Result<(), RpcError> {
    let Some(auth_token) = auth_token else {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "envd environment attachment requires authToken",
        ));
    };
    if auth_token.trim().is_empty() {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "envd environment attachment authToken cannot be empty",
        ));
    }
    if auth_token.bytes().any(|byte| matches!(byte, b'\r' | b'\n')) {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "envd environment attachment authToken cannot contain newlines",
        ));
    }
    Ok(())
}

fn active_run_rpc_error(error: CliError) -> RpcError {
    match error {
        CliError::NotFound(value) => {
            RpcError::new(RUN_CONFLICT, format!("active run not found: {value}"))
        }
        other => RpcError::from(other),
    }
}

fn steering_id(params: &Value) -> String {
    params
        .get("steeringId")
        .or_else(|| params.get("id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map_or_else(
            || format!("steer_{}", chrono::Utc::now().timestamp_micros()),
            ToString::to_string,
        )
}

fn subscription_id(params: &Value) -> String {
    params
        .get("subscriptionId")
        .or_else(|| params.get("id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map_or_else(
            || format!("stream_{}", chrono::Utc::now().timestamp_micros()),
            ToString::to_string,
        )
}

fn prompt_active_environment_run_id() -> String {
    format!("run_prompt_{}", Uuid::new_v4().simple())
}

fn pending_active_environment_run_id() -> String {
    format!("run_start_pending_{}", Uuid::new_v4().simple())
}

fn forward_run_events(
    output_sender: &mpsc::Sender<Value>,
    receiver: mpsc::Receiver<RunStreamEvent>,
    format: StreamPayloadFormat,
) {
    for event in receiver {
        let Some(frame) = run_event_notification(event, format) else {
            continue;
        };
        if output_sender.send(frame).is_err() {
            break;
        }
    }
}

fn forward_started_run_events(
    output_sender: &mpsc::Sender<Value>,
    receiver: mpsc::Receiver<RunStreamEvent>,
    format: StreamPayloadFormat,
    send_notifications: bool,
    environment_manager: &EnvironmentAttachmentManager,
    run_id: &str,
) {
    for event in receiver {
        let terminal = matches!(
            &event,
            RunStreamEvent::Status(status) if status_is_terminal(&status.status)
        );
        if send_notifications {
            if let Some(frame) = run_event_notification(event, format) {
                let _ = output_sender.send(frame);
            }
        }
        if terminal {
            let _ = environment_manager.mark_run_finished(run_id);
            return;
        }
    }
    let _ = environment_manager.mark_run_finished(run_id);
}

fn run_event_notification(event: RunStreamEvent, format: StreamPayloadFormat) -> Option<Value> {
    match event {
        RunStreamEvent::Output(output) => {
            let output = output_item(&output, format)?;
            Some(notification("run.output", &json!(output)))
        }
        RunStreamEvent::Status(status) => Some(notification("run.status", &json!(status))),
        RunStreamEvent::Raw(_) => None,
    }
}

fn forward_subscription_events(
    output_sender: &mpsc::Sender<Value>,
    receiver: &mpsc::Receiver<RunStreamEvent>,
    format: StreamPayloadFormat,
    subscription_id: &str,
    cancel: &AtomicBool,
) {
    while !cancel.load(Ordering::SeqCst) {
        let event = match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => event,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let terminal = matches!(
            &event,
            RunStreamEvent::Status(status) if status_is_terminal(&status.status)
        );
        let frame = match event {
            RunStreamEvent::Output(output) => {
                let Some(output) = output_item(&output, format) else {
                    continue;
                };
                let mut params = json!(output);
                insert_subscription_id(&mut params, subscription_id);
                notification("stream.output", &params)
            }
            RunStreamEvent::Status(status) => {
                let mut params = json!(status);
                insert_subscription_id(&mut params, subscription_id);
                notification("stream.status", &params)
            }
            RunStreamEvent::Raw(_) => continue,
        };
        if output_sender.send(frame).is_err() || terminal {
            break;
        }
    }
}

fn insert_subscription_id(value: &mut Value, subscription_id: &str) {
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "subscriptionId".to_string(),
            Value::String(subscription_id.to_string()),
        );
    }
}

fn merge_object(target: &mut Value, source: &Value) {
    let Some(target) = target.as_object_mut() else {
        return;
    };
    let Some(source) = source.as_object() else {
        return;
    };
    for (key, value) in source {
        target.insert(key.clone(), value.clone());
    }
}

fn status_is_terminal(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled")
}

fn initialize_result(config: &CliConfig, notifications: RpcNotificationMode) -> Value {
    let live_notifications = notifications.supports_live_notifications();
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "serverInfo": {"name": "starweaver-cli", "version": env!("CARGO_PKG_VERSION")},
        "capabilities": {
            "sessions": true,
            "runs": true,
            "management": true,
            "profiles": true,
            "clientModelSelection": true,
            "blockingRunStart": false,
            "blockingRunPrompt": true,
            "nonBlockingRunStart": true,
            "liveDisplay": live_notifications,
            "streamReplay": true,
            "streamSubscribe": live_notifications,
            "cancel": true,
            "steering": true,
            "attach": true,
            "environmentAttachments": true,
            "environmentActiveMounts": true,
            "defaultStreamPayload": "agui",
            "approvals": true,
            "deferred": true
        },
        "config": {
            "globalDir": config.global_dir,
            "projectDir": config.project_dir,
            "tuiStateDir": config.tui_state_dir,
            "desktopStateDir": config.desktop_state_dir,
            "defaultProfile": config.default_profile,
        }
    })
}

fn config_get(config: &CliConfig, params: &Value) -> Result<Value, RpcError> {
    if let Some(key) = params.get("key").and_then(Value::as_str) {
        let value = get_config_value(config, key)
            .map_err(RpcError::from)?
            .trim_end_matches('\n')
            .to_string();
        return Ok(json!({"values": {key: value}}));
    }
    let Some(keys) = params.get("keys").and_then(Value::as_array) else {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "config.get requires key or keys",
        ));
    };
    let mut values = serde_json::Map::new();
    for key in keys {
        let Some(key) = key.as_str() else {
            return Err(RpcError::new(INVALID_PARAMS, "keys must be strings"));
        };
        let value = get_config_value(config, key)
            .map_err(RpcError::from)?
            .trim_end_matches('\n')
            .to_string();
        values.insert(key.to_string(), Value::String(value));
    }
    Ok(json!({"values": values}))
}

fn selected_profile_result(config: &CliConfig, client: Option<&str>) -> Result<Value, RpcError> {
    let selected = client
        .map(|client| client_state::read_selected_profile(config, client).map_err(RpcError::from))
        .transpose()?
        .flatten()
        .unwrap_or_else(|| config.default_profile.clone());
    Ok(json!({
        "client": client,
        "selectedProfile": selected,
        "modelId": model_id_for_profile(config, &selected),
    }))
}

fn selected_model_profile_result(
    config: &CliConfig,
    client: Option<&str>,
) -> Result<Value, RpcError> {
    let configured_profiles = list_config_model_profiles(config);
    let persisted = client
        .map(|client| client_state::read_selected_profile(config, client).map_err(RpcError::from))
        .transpose()?
        .flatten();
    let selected = persisted
        .filter(|profile| {
            configured_profiles
                .iter()
                .any(|summary| summary.name == *profile)
        })
        .or_else(|| {
            configured_profiles
                .iter()
                .find(|summary| summary.name == config.default_profile)
                .map(|summary| summary.name.clone())
        })
        .or_else(|| {
            configured_profiles
                .first()
                .map(|summary| summary.name.clone())
        });
    let model_id = selected
        .as_deref()
        .and_then(|selected| {
            configured_profiles
                .iter()
                .find(|summary| summary.name == selected)
        })
        .map(|summary| summary.model_id.clone());
    Ok(json!({
        "client": client,
        "selectedProfile": selected,
        "modelId": model_id,
    }))
}

fn ensure_profile(config: &CliConfig, profile: &str) -> Result<(), RpcError> {
    if list_profiles(config)
        .iter()
        .any(|summary| summary.name == profile)
    {
        Ok(())
    } else {
        Err(RpcError::new(
            INVALID_PARAMS,
            format!("unknown profile: {profile}"),
        ))
    }
}

fn ensure_client_model_profile(config: &CliConfig, profile: &str) -> Result<(), RpcError> {
    if list_config_model_profiles(config)
        .iter()
        .any(|summary| summary.name == profile)
    {
        Ok(())
    } else {
        Err(RpcError::new(
            INVALID_PARAMS,
            format!("unknown model profile: {profile}"),
        ))
    }
}

fn model_id_for_profile(config: &CliConfig, profile: &str) -> Option<String> {
    list_profiles(config)
        .into_iter()
        .find(|summary| summary.name == profile)
        .map(|summary| summary.model_id)
}

fn parse_hitl(value: &str) -> Option<HitlPolicy> {
    match value {
        "deny" => Some(HitlPolicy::Deny),
        "defer" => Some(HitlPolicy::Defer),
        "fail" => Some(HitlPolicy::Fail),
        "prompt" => Some(HitlPolicy::Prompt),
        _ => None,
    }
}

fn required_string(params: &Value, key: &str) -> Result<String, RpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, format!("missing string param: {key}")))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::{
        io::{Read as _, Write as _},
        net::{TcpListener, TcpStream},
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
    };

    use serde_json::json;

    use super::*;
    use crate::{args, ConfigResolver};

    fn test_config(root: &std::path::Path) -> CliConfig {
        let cli = args::parse(["starweaver-cli".to_string(), "rpc".to_string()]).unwrap();
        ConfigResolver::for_tests(root).resolve(&cli).unwrap()
    }

    #[allow(clippy::needless_pass_by_value)]
    fn request(config: &CliConfig, id: u64, method: &str, params: Value) -> Value {
        let (output_sender, _output_receiver) = mpsc::channel::<Value>();
        let server = RpcService::new(config.clone(), output_sender);
        request_with_server(&server, id, method, params)
    }

    #[allow(clippy::needless_pass_by_value)]
    fn request_with_server(server: &RpcService, id: u64, method: &str, params: Value) -> Value {
        let line = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        })
        .to_string();
        let (response, shutdown) = server.handle_line(&line);
        assert!(!shutdown || method == "shutdown");
        let response = response.unwrap();
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], id);
        assert!(
            response.get("error").is_none(),
            "unexpected RPC error: {response}"
        );
        response["result"].clone()
    }

    #[allow(clippy::needless_pass_by_value)]
    fn request_error_with_server(
        server: &RpcService,
        id: u64,
        method: &str,
        params: Value,
    ) -> Value {
        let line = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        })
        .to_string();
        let (response, shutdown) = server.handle_line(&line);
        assert!(!shutdown);
        let response = response.unwrap();
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], id);
        response["error"].clone()
    }

    fn http_post(config: &CliConfig, body: &Value) -> (String, Arc<AtomicBool>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let service = Arc::new(RpcService::replay_only(
            config.clone(),
            transport::closed_notification_sender(),
        ));
        let shutdown = Arc::new(AtomicBool::new(false));
        let server_shutdown = Arc::clone(&shutdown);
        let handle = thread::spawn(move || {
            let (stream, _address) = listener.accept().unwrap();
            transport::handle_http_connection(stream, &service, &server_shutdown).unwrap();
        });
        let body = body.to_string();
        let mut client = TcpStream::connect(address).unwrap();
        write!(
            client,
            "POST /rpc HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        handle.join().unwrap();
        (response, shutdown)
    }

    fn http_body(response: &str) -> Value {
        let (headers, body) = response.split_once("\r\n\r\n").unwrap();
        assert!(headers.starts_with("HTTP/1.1 200 OK"), "{headers}");
        serde_json::from_str(body).unwrap()
    }

    #[test]
    fn initialize_capabilities_follow_notification_transport() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let (output_sender, _output_receiver) = mpsc::channel::<Value>();
        let live = RpcService::new(config.clone(), output_sender);
        let initialized = request_with_server(&live, 1, "initialize", json!({}));
        assert_eq!(initialized["capabilities"]["liveDisplay"], true);
        assert_eq!(initialized["capabilities"]["streamSubscribe"], true);
        assert_eq!(initialized["capabilities"]["environmentActiveMounts"], true);

        let replay_only = RpcService::replay_only(config, transport::closed_notification_sender());
        let initialized = request_with_server(&replay_only, 2, "initialize", json!({}));
        assert_eq!(initialized["capabilities"]["liveDisplay"], false);
        assert_eq!(initialized["capabilities"]["streamSubscribe"], false);
        assert_eq!(initialized["capabilities"]["streamReplay"], true);
        assert_eq!(initialized["capabilities"]["environmentActiveMounts"], true);
    }

    #[test]
    fn active_environment_methods_are_dispatched() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let (output_sender, _output_receiver) = mpsc::channel::<Value>();
        let server = RpcService::new(config, output_sender);

        let error = request_error_with_server(
            &server,
            1,
            "environment.active_list",
            json!({"runId": "run_missing"}),
        );

        assert_eq!(error["code"], RUN_CONFLICT);
        assert!(error["message"].as_str().unwrap().contains("run_missing"));
    }

    #[test]
    fn rpc_command_parses_http_transport() {
        let cli = args::parse([
            "starweaver-cli".to_string(),
            "rpc".to_string(),
            "http".to_string(),
            "--host".to_string(),
            "127.0.0.1".to_string(),
            "--port".to_string(),
            "0".to_string(),
        ])
        .unwrap();
        let Some(crate::CliCommand::Rpc(command)) = cli.command else {
            panic!("expected rpc command");
        };
        assert_eq!(command.transport, crate::args::RpcTransport::Http);
        assert_eq!(command.host, "127.0.0.1");
        assert_eq!(command.port, 0);
    }

    #[test]
    fn initialize_and_model_selection_use_client_state_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let global = temp.path().join("global");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(
            global.join("config.toml"),
            r#"
[general]
model = "test:default"

[model_profiles.coding]
label = "Coding"
model = "test:coding"
"#,
        )
        .unwrap();
        let config = test_config(temp.path());

        let initialized = request(
            &config,
            1,
            "initialize",
            json!({"clientInfo":{"name":"tui"}}),
        );
        assert_eq!(initialized["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(initialized["capabilities"]["clientModelSelection"], true);
        assert_eq!(initialized["config"]["globalDir"], json!(config.global_dir));
        assert_eq!(
            initialized["config"]["tuiStateDir"],
            json!(config.tui_state_dir)
        );
        assert_eq!(
            initialized["config"]["desktopStateDir"],
            json!(config.desktop_state_dir)
        );

        let listed = request(&config, 2, "model.list", json!({"client":"tui"}));
        let listed_profiles = listed["profiles"].as_array().unwrap();
        assert_eq!(listed_profiles.len(), 2);
        assert_eq!(listed_profiles[0]["name"], "default_model");
        assert_eq!(listed_profiles[0]["model_id"], "test:default");
        assert_eq!(listed_profiles[1]["name"], "coding");
        assert_eq!(listed_profiles[1]["model_id"], "test:coding");
        assert!(!listed_profiles
            .iter()
            .any(|profile| profile["source"] == "built-in" || profile["model_id"] == "local_echo"));
        assert_eq!(listed["current"]["selectedProfile"], config.default_profile);

        let selected = request(
            &config,
            3,
            "model.select",
            json!({"client":"tui", "profile":"coding"}),
        );
        assert_eq!(selected["client"], "tui");
        assert_eq!(selected["selectedProfile"], "coding");
        assert_eq!(selected["modelId"], "test:coding");
        assert!(config.tui_state_dir.join("state.json").exists());
        assert!(!config.desktop_state_dir.join("state.json").exists());

        let current = request(&config, 4, "model.current", json!({"client":"tui"}));
        assert_eq!(current["selectedProfile"], "coding");
        let desktop_current = request(&config, 5, "model.current", json!({"client":"desktop"}));
        assert_eq!(desktop_current["selectedProfile"], "default_model");
        assert_eq!(desktop_current["modelId"], "test:default");
    }

    #[test]
    fn client_model_selection_is_empty_without_configured_profiles() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());

        let listed = request(&config, 1, "model.list", json!({"client":"tui"}));
        assert!(listed["profiles"].as_array().unwrap().is_empty());
        assert!(listed["current"]["selectedProfile"].is_null());
        assert!(listed["current"]["modelId"].is_null());

        let line = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "model.select",
            "params": {"client":"tui", "profile":"general"},
        })
        .to_string();
        let (output_sender, _output_receiver) = mpsc::channel::<Value>();
        let server = RpcService::new(config, output_sender);
        let (response, shutdown) = server.handle_line(&line);
        assert!(!shutdown);
        let response = response.unwrap();
        assert_eq!(response["id"], 2);
        assert_eq!(response["error"]["code"], -32_602);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown model profile: general"));
    }

    #[test]
    fn client_model_selection_only_uses_configured_profiles() {
        let temp = tempfile::tempdir().unwrap();
        let global = temp.path().join("global");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(
            global.join("config.toml"),
            r#"
[model_profiles.coding]
model = "test:coding"
"#,
        )
        .unwrap();
        let config = test_config(temp.path());

        let listed = request(&config, 1, "model.list", json!({"client":"tui"}));
        let listed_profiles = listed["profiles"].as_array().unwrap();
        assert_eq!(listed_profiles.len(), 1);
        assert_eq!(listed_profiles[0]["name"], "coding");
        assert_eq!(listed_profiles[0]["source"], "config");
        assert_eq!(listed_profiles[0]["model_id"], "test:coding");
        assert!(!listed_profiles
            .iter()
            .any(|profile| profile["model_id"] == "local_echo"));

        let selected = request(
            &config,
            2,
            "model.select",
            json!({"client":"tui", "profile":"coding"}),
        );
        assert_eq!(selected["selectedProfile"], "coding");
        assert_eq!(selected["modelId"], "test:coding");

        let current = request(&config, 3, "model.current", json!({"client":"tui"}));
        assert_eq!(current["selectedProfile"], "coding");
        assert_eq!(current["modelId"], "test:coding");
    }

    #[test]
    fn config_get_and_run_prompt_smoke_through_rpc_dispatch() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());

        let values = request(
            &config,
            1,
            "config.get",
            json!({"keys": ["general.default_profile", "storage.database_path"]}),
        );
        assert_eq!(
            values["values"]["general.default_profile"],
            config.default_profile
        );
        assert_eq!(
            values["values"]["storage.database_path"],
            config.database_path.display().to_string()
        );

        let run = request(
            &config,
            2,
            "run.prompt",
            json!({"prompt":"hello from rpc", "newSession": true, "client":"tui"}),
        );
        assert!(run["sessionId"].as_str().unwrap().starts_with("session_"));
        assert!(run["runId"].as_str().unwrap().starts_with("run_"));
        assert_eq!(run["status"], "completed");
        assert!(run["latestCursor"]["sequence"].as_u64().is_some());

        let replay = request(
            &config,
            3,
            "session.replay",
            json!({"sessionId": run["sessionId"].as_str().unwrap()}),
        );
        assert!(replay["messages"].as_array().unwrap().len() > 1);
        assert_eq!(
            replay["scope"],
            format!("session:{}", run["sessionId"].as_str().unwrap())
        );
        assert!(replay["events"].as_array().unwrap().len() > 1);
        assert!(replay["latestCursor"]["sequence"].as_u64().is_some());

        let tail = request(
            &config,
            4,
            "session.replay",
            json!({
                "sessionId": run["sessionId"].as_str().unwrap(),
                "cursor": replay["latestCursor"],
            }),
        );
        assert_eq!(tail["messages"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn stream_methods_cover_replay_subscribe_and_unsubscribe() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let created = request(&config, 1, "session.create", json!({"title": "stream rpc"}));
        let session_id = created["session"]["session_id"].as_str().unwrap();

        let replay = request(
            &config,
            2,
            "stream.replay",
            json!({"sessionId": session_id}),
        );
        assert_eq!(replay["sessionId"], session_id);
        assert_eq!(replay["scope"], format!("session:{session_id}"));
        assert_eq!(replay["events"].as_array().unwrap().len(), 0);

        let subscribed = request(
            &config,
            3,
            "stream.subscribe",
            json!({
                "sessionId": session_id,
                "subscriptionId": "sub_stream_test",
                "payloadFormat": "display_message"
            }),
        );
        assert_eq!(subscribed["subscriptionId"], "sub_stream_test");
        assert_eq!(subscribed["sessionId"], session_id);
        assert_eq!(subscribed["active"], false);
        assert_eq!(subscribed["payloadFormat"], "display_message");

        let unsubscribed = request(
            &config,
            4,
            "stream.unsubscribe",
            json!({"subscriptionId": "sub_stream_test"}),
        );
        assert_eq!(unsubscribed["subscriptionId"], "sub_stream_test");
        assert_eq!(unsubscribed["unsubscribed"], false);
    }

    #[test]
    fn stream_subscribe_requires_live_notification_transport() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let server = RpcService::replay_only(config, transport::closed_notification_sender());
        let line = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "stream.subscribe",
            "params": {"sessionId": "session_test"},
        })
        .to_string();
        let (response, shutdown) = server.handle_line(&line);
        assert!(!shutdown);
        let response = response.unwrap();
        assert_eq!(response["id"], 1);
        assert_eq!(response["error"]["code"], UNSUPPORTED_FEATURE);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("live notification transport"));
    }

    #[test]
    fn replay_only_transport_requires_session_scoped_environment_leases() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let server = RpcService::replay_only(config, transport::closed_notification_sender());

        let connection_error = request_error_with_server(
            &server,
            1,
            "environment.attach",
            json!({
                "attachment": {"id": "workspace", "kind": "local"},
                "readiness": {"policy": "required"}
            }),
        );
        assert_eq!(connection_error["code"], UNSUPPORTED_FEATURE);
        assert!(connection_error["message"]
            .as_str()
            .unwrap()
            .contains("session scope"));

        let attached = request_with_server(
            &server,
            2,
            "environment.attach",
            json!({
                "scope": {"kind": "session", "sessionId": "session_http"},
                "attachment": {"id": "workspace", "kind": "local"},
                "readiness": {"policy": "required"}
            }),
        );
        assert_eq!(attached["attachment"]["scope"]["kind"], "session");
        assert_eq!(attached["attachment"]["scope"]["sessionId"], "session_http");
    }

    #[test]
    fn run_start_accepts_multiple_environment_attachments() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let (output_sender, _output_receiver) = mpsc::channel::<Value>();
        let server = RpcService::new(config, output_sender);
        let line = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "run.start",
            "params": {
                "prompt": "hello",
                "environmentAttachments": [
                    {"id": "local", "kind": "local", "default": true},
                    {"id": "tools"}
                ]
            },
        })
        .to_string();
        let (response, shutdown) = server.handle_line(&line);
        assert!(!shutdown);
        let response = response.unwrap();
        let result = response["result"].as_object().unwrap();
        assert_eq!(result["status"], "running");
        assert_eq!(
            result["environmentAttachments"].as_array().unwrap().len(),
            2
        );
        assert_eq!(result["environmentAttachments"][0]["id"], "local");
    }

    #[test]
    fn environment_attachment_methods_manage_local_lease_lifecycle() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let (output_sender, _output_receiver) = mpsc::channel::<Value>();
        let server = RpcService::new(config, output_sender);

        let attached = request_with_server(
            &server,
            1,
            "environment.attach",
            json!({
                "scope": {"kind": "connection"},
                "attachment": {"id": "workspace", "kind": "local", "mode": "read_only"},
                "readiness": {"policy": "required"},
                "idempotencyKey": "workspace"
            }),
        );
        let lease_id = attached["attachment"]["attachmentLeaseId"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(lease_id.starts_with("envatt_workspace_"));
        assert_eq!(attached["attachment"]["status"], "ready");
        assert_eq!(attached["attachment"]["readiness"]["transport"], "ready");
        assert_eq!(attached["attachment"]["mode"], "read_only");

        let repeated = request_with_server(
            &server,
            2,
            "environment.attach",
            json!({
                "scope": {"kind": "connection"},
                "attachment": {"id": "workspace", "kind": "local", "mode": "read_only"},
                "readiness": {"policy": "required"},
                "idempotencyKey": "workspace"
            }),
        );
        assert_eq!(
            repeated["attachment"]["attachmentLeaseId"],
            attached["attachment"]["attachmentLeaseId"]
        );

        let listed = request_with_server(&server, 3, "environment.list", json!({}));
        assert_eq!(listed["attachments"].as_array().unwrap().len(), 1);
        assert_eq!(listed["attachments"][0]["attachmentLeaseId"], lease_id);

        let health = request_with_server(
            &server,
            4,
            "environment.health",
            json!({"attachmentLeaseId": lease_id}),
        );
        assert_eq!(health["status"], "ready");
        assert_eq!(health["readiness"]["environment"], "ready");

        let detached = request_with_server(
            &server,
            5,
            "environment.detach",
            json!({"attachmentLeaseId": lease_id}),
        );
        assert_eq!(detached["detached"], true);
        let listed_detached = request_with_server(
            &server,
            6,
            "environment.list",
            json!({"status": "detached"}),
        );
        assert_eq!(listed_detached["attachments"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn run_start_materializes_environment_attachment_lease_refs() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let (output_sender, _output_receiver) = mpsc::channel::<Value>();
        let server = RpcService::new(config, output_sender);

        let attached = request_with_server(
            &server,
            1,
            "environment.attach",
            json!({
                "attachment": {"id": "workspace", "kind": "local", "mode": "read_only"},
                "readiness": {"policy": "required"}
            }),
        );
        let lease_id = attached["attachment"]["attachmentLeaseId"]
            .as_str()
            .unwrap();
        let started = request_with_server(
            &server,
            2,
            "run.start",
            json!({
                "prompt": "hello leased environment",
                "environmentAttachments": [
                    {"id": "workspace", "attachmentLeaseId": lease_id, "default": true}
                ]
            }),
        );
        assert_eq!(started["status"], "running");
        assert_eq!(started["environmentAttachments"][0]["id"], "workspace");
        assert_eq!(
            started["environmentAttachments"][0]["attachmentLeaseId"],
            lease_id
        );
        assert_eq!(started["environmentAttachments"][0]["mode"], "read_only");
    }

    #[test]
    fn run_start_rejects_session_scoped_environment_lease_for_other_session() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let (output_sender, _output_receiver) = mpsc::channel::<Value>();
        let server = RpcService::new(config, output_sender);

        let attached = request_with_server(
            &server,
            1,
            "environment.attach",
            json!({
                "scope": {"kind": "session", "sessionId": "session_a"},
                "attachment": {"id": "workspace", "kind": "local"},
                "readiness": {"policy": "required"}
            }),
        );
        let lease_id = attached["attachment"]["attachmentLeaseId"]
            .as_str()
            .unwrap();
        let error = request_error_with_server(
            &server,
            2,
            "run.start",
            json!({
                "prompt": "hello wrong session",
                "sessionId": "session_b",
                "environmentAttachments": [
                    {"id": "workspace", "attachmentLeaseId": lease_id, "default": true}
                ]
            }),
        );
        assert_eq!(error["code"], INVALID_PARAMS);
        assert!(error["message"].as_str().unwrap().contains("session_a"));
        assert!(error["message"].as_str().unwrap().contains("session_b"));
    }

    #[test]
    fn run_start_rejects_ambiguous_session_selector_for_session_scoped_lease() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let (output_sender, _output_receiver) = mpsc::channel::<Value>();
        let server = RpcService::new(config, output_sender);

        let attached = request_with_server(
            &server,
            1,
            "environment.attach",
            json!({
                "scope": {"kind": "session", "sessionId": "session_a"},
                "attachment": {"id": "workspace", "kind": "local"},
                "readiness": {"policy": "required"}
            }),
        );
        let lease_id = attached["attachment"]["attachmentLeaseId"]
            .as_str()
            .unwrap();
        let error = request_error_with_server(
            &server,
            2,
            "run.start",
            json!({
                "prompt": "hello ambiguous session",
                "sessionId": "session_a",
                "newSession": true,
                "environmentAttachments": [
                    {"id": "workspace", "attachmentLeaseId": lease_id, "default": true}
                ]
            }),
        );
        assert_eq!(error["code"], INVALID_PARAMS);
        assert!(error["message"]
            .as_str()
            .unwrap()
            .contains("mutually exclusive"));
    }

    #[test]
    fn run_prompt_materializes_environment_attachment_lease_refs() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let (output_sender, _output_receiver) = mpsc::channel::<Value>();
        let server = RpcService::new(config, output_sender);

        let attached = request_with_server(
            &server,
            1,
            "environment.attach",
            json!({
                "attachment": {"id": "workspace", "kind": "local", "mode": "read_only"},
                "readiness": {"policy": "required"}
            }),
        );
        let lease_id = attached["attachment"]["attachmentLeaseId"]
            .as_str()
            .unwrap();
        let run = request_with_server(
            &server,
            2,
            "run.prompt",
            json!({
                "prompt": "hello leased blocking environment",
                "newSession": true,
                "environmentAttachments": [
                    {"id": "workspace", "attachmentLeaseId": lease_id, "default": true}
                ]
            }),
        );
        assert_eq!(run["status"], "completed");
        assert!(run["sessionId"].as_str().unwrap().starts_with("session_"));
        assert!(run["runId"].as_str().unwrap().starts_with("run_"));
    }

    #[test]
    fn envd_attachment_health_reports_unavailable_without_creating_run() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let (output_sender, _output_receiver) = mpsc::channel::<Value>();
        let server = RpcService::new(config, output_sender);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/rpc", listener.local_addr().unwrap());
        drop(listener);

        let health = request_with_server(
            &server,
            1,
            "environment.health",
            json!({
                "attachment": {
                    "id": "remote",
                    "kind": "envd",
                    "endpointRef": endpoint,
                    "environmentId": "dataset",
                    "authToken": "secret"
                },
                "timeoutMs": 100
            }),
        );
        assert_eq!(health["status"], "unavailable");
        assert_eq!(health["readiness"]["transport"], "unavailable");
    }

    #[test]
    fn envd_attachment_requires_auth_token() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let (output_sender, _output_receiver) = mpsc::channel::<Value>();
        let server = RpcService::new(config, output_sender);

        let error = request_error_with_server(
            &server,
            1,
            "run.start",
            json!({
                "prompt": "hello",
                "environmentAttachments": [
                    {
                        "id": "workspace",
                        "kind": "envd",
                        "endpointRef": "http://127.0.0.1:8766/rpc",
                        "default": true
                    }
                ]
            }),
        );
        assert_eq!(error["code"], INVALID_PARAMS);
        assert!(error["message"].as_str().unwrap().contains("authToken"));
    }

    #[test]
    fn envd_attachment_rejects_empty_auth_token() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let (output_sender, _output_receiver) = mpsc::channel::<Value>();
        let server = RpcService::new(config, output_sender);

        let error = request_error_with_server(
            &server,
            1,
            "run.start",
            json!({
                "prompt": "hello",
                "environmentAttachments": [
                    {
                        "id": "workspace",
                        "kind": "envd",
                        "endpointRef": "http://127.0.0.1:8766/rpc",
                        "authToken": " ",
                        "default": true
                    }
                ]
            }),
        );
        assert_eq!(error["code"], INVALID_PARAMS);
        assert!(error["message"]
            .as_str()
            .unwrap()
            .contains("cannot be empty"));
    }

    #[test]
    fn run_start_fails_before_start_when_required_envd_is_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let (output_sender, _output_receiver) = mpsc::channel::<Value>();
        let server = RpcService::new(config, output_sender);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/rpc", listener.local_addr().unwrap());
        drop(listener);

        let error = request_error_with_server(
            &server,
            1,
            "run.start",
            json!({
                "prompt": "hello",
                "environmentAttachments": [
                    {
                        "id": "workspace",
                        "kind": "envd",
                        "endpointRef": endpoint,
                        "environmentId": "dataset",
                        "authToken": "secret",
                        "default": true
                    }
                ]
            }),
        );
        assert_eq!(error["code"], starweaver_rpc_core::ENVIRONMENT_UNAVAILABLE);
        assert!(error["message"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase()
            .contains("connection"));
    }

    #[test]
    fn run_start_reprobes_best_effort_envd_lease_before_starting() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let (output_sender, _output_receiver) = mpsc::channel::<Value>();
        let server = RpcService::new(config, output_sender);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/rpc", listener.local_addr().unwrap());
        drop(listener);

        let attached = request_with_server(
            &server,
            1,
            "environment.attach",
            json!({
                "attachment": {
                    "id": "remote",
                    "kind": "envd",
                    "endpointRef": endpoint,
                    "environmentId": "dataset",
                    "authToken": "secret"
                },
                "readiness": {"policy": "best_effort", "timeoutMs": 100}
            }),
        );
        assert_eq!(attached["attachment"]["status"], "unavailable");
        assert!(attached["attachment"].get("authToken").is_none());
        let lease_id = attached["attachment"]["attachmentLeaseId"]
            .as_str()
            .unwrap();

        let error = request_error_with_server(
            &server,
            2,
            "run.start",
            json!({
                "prompt": "hello",
                "environmentAttachments": [
                    {"id": "workspace", "attachmentLeaseId": lease_id, "default": true}
                ]
            }),
        );
        assert_eq!(error["code"], starweaver_rpc_core::ENVIRONMENT_UNAVAILABLE);
    }

    #[test]
    fn run_start_streams_agui_payloads_and_session_output_replays() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let (output_sender, output_receiver) = mpsc::channel::<Value>();
        let server = RpcService::new(config, output_sender);

        let started = request_with_server(
            &server,
            1,
            "run.start",
            json!({"prompt": "hello live rpc", "newSession": true}),
        );
        let session_id = started["sessionId"].as_str().unwrap().to_string();
        let run_id = started["runId"].as_str().unwrap().to_string();
        assert_eq!(started["status"], "running");
        assert_eq!(started["payloadFormat"], "agui");

        let mut saw_agui_output = false;
        let mut saw_terminal_status = false;
        for _ in 0..100 {
            let frame = output_receiver
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            match frame["method"].as_str() {
                Some("run.output") => {
                    assert_eq!(frame["params"]["payloadFormat"], "agui");
                    assert!(frame["params"]["payload"]["type"].is_string());
                    saw_agui_output = true;
                }
                Some("run.status") if frame["params"]["status"] == "completed" => {
                    saw_terminal_status = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_agui_output);
        assert!(saw_terminal_status);

        let output = request_with_server(
            &server,
            2,
            "session.output",
            json!({"sessionId": session_id, "runId": run_id}),
        );
        assert_eq!(output["payloadFormat"], "agui");
        let events = output["events"].as_array().unwrap();
        assert!(!events.is_empty());
        assert!(events
            .iter()
            .any(|event| event["payload"]["type"] == "RUN_FINISHED"));
    }

    #[test]
    fn rpc_run_prompt_expands_configured_slash_command() {
        let temp = tempfile::tempdir().unwrap();
        let global = temp.path().join("global");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(
            global.join("config.toml"),
            r#"
[general]
model = "local_echo"

[commands.review]
description = "Review changes"
aliases = ["rv"]
prompt = "Review via RPC."
"#,
        )
        .unwrap();
        let config = test_config(temp.path());

        let run = request(
            &config,
            1,
            "run.prompt",
            json!({
                "prompt":"/rv staged diff",
                "newSession": true,
                "profile":"default_model",
            }),
        );
        assert_eq!(run["status"], "completed");

        let store = crate::LocalStore::open(&config).unwrap();
        let run_record = store
            .load_run(
                run["sessionId"].as_str().unwrap(),
                run["runId"].as_str().unwrap(),
            )
            .unwrap();
        let value = serde_json::to_value(run_record).unwrap();
        assert_eq!(
            value["input"][0]["text"],
            "Review via RPC.\n\nUser instruction: staged diff"
        );
        assert_eq!(value["metadata"]["cli.slash_command.name"], "review");
        assert_eq!(value["metadata"]["cli.slash_command.invoked"], "rv");
    }

    #[test]
    fn http_transport_dispatches_json_rpc_requests() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());

        let (response, shutdown) = http_post(
            &config,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {"clientInfo": {"name": "http-test"}},
            }),
        );
        assert!(!shutdown.load(Ordering::SeqCst));
        let body = http_body(&response);
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 1);
        assert_eq!(body["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(body["result"]["serverInfo"]["name"], "starweaver-cli");
    }

    #[test]
    fn http_transport_shutdown_marks_server_shutdown() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());

        let (response, shutdown) = http_post(
            &config,
            &json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "shutdown",
                "params": {},
            }),
        );
        assert!(shutdown.load(Ordering::SeqCst));
        let body = http_body(&response);
        assert_eq!(body["id"], 7);
        assert_eq!(body["result"]["status"], "shutdown");
    }
}
