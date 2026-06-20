//! Newline-delimited JSON-RPC stdio runtime for local host integrations.

use std::{
    io::{self, BufRead as _, Write as _},
    sync::mpsc,
    thread,
    time::Duration,
};

use serde::Deserialize;
use serde_json::{json, Value};
use starweaver_stream::ReplayScope;

use crate::{
    args::{HitlPolicy, OutputMode, RunCommand},
    client_state,
    config::{get_config_value, read_current_session, write_current_session, CliConfig},
    local_store::{LocalStore, LocalStreamArchive},
    profiles::{list_config_model_profiles, list_profiles, show_profile},
    runner::CliSteeringMessage,
    runtime_coordinator::{CliRuntimeCoordinator, RunAttachment, RunStreamEvent, StartedRun},
    CliError, CliResult, CliService,
};

mod protocol;

use protocol::{
    attachment_result, notification, output_item, replay_cursor_from_params, replay_result,
    stream_payload_format, StreamPayloadFormat,
};

const PROTOCOL_VERSION: &str = "2026-06-08";

#[derive(Debug, Deserialize)]
struct RpcRequest {
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug)]
struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<CliError> for RpcError {
    fn from(error: CliError) -> Self {
        Self::new(-32_000, error.to_string())
    }
}

/// Run the JSON-RPC stdio server until stdin closes or `shutdown` is requested.
pub fn run_stdio(config: &CliConfig) -> CliResult<()> {
    let (output_sender, output_receiver) = mpsc::channel::<Value>();
    let writer = thread::spawn(move || {
        let mut stdout = io::stdout();
        while let Ok(response) = output_receiver.recv() {
            if serde_json::to_writer(&mut stdout, &response).is_err() {
                break;
            }
            if stdout.write_all(b"\n").is_err() || stdout.flush().is_err() {
                break;
            }
        }
    });
    let server = RpcServer::new(config.clone(), output_sender.clone());
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = line.map_err(|error| CliError::Run(error.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        let (response, shutdown) = server.handle_line(&line);
        if let Some(response) = response {
            output_sender
                .send(response)
                .map_err(|error| CliError::Run(error.to_string()))?;
        }
        if shutdown {
            break;
        }
    }
    drop(output_sender);
    let _ = writer.join();
    Ok(())
}

struct RpcServer {
    config: CliConfig,
    coordinator: CliRuntimeCoordinator,
    output_sender: mpsc::Sender<Value>,
}

impl RpcServer {
    fn new(config: CliConfig, output_sender: mpsc::Sender<Value>) -> Self {
        Self {
            coordinator: CliRuntimeCoordinator::new(config.clone()),
            config,
            output_sender,
        }
    }

    fn handle_line(&self, line: &str) -> (Option<Value>, bool) {
        let request = match serde_json::from_str::<RpcRequest>(line) {
            Ok(request) => request,
            Err(error) => {
                return (
                    Some(error_response(
                        &Value::Null,
                        -32_700,
                        &format!("parse error: {error}"),
                    )),
                    false,
                )
            }
        };
        let id = request.id.clone();
        let result = self.dispatch(&request.method, &request.params);
        let shutdown = request.method == "shutdown" && result.is_ok();
        let Some(id) = id else {
            return (None, shutdown);
        };
        let response = match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => error_response(&id, error.code, &error.message),
        };
        (Some(response), shutdown)
    }

    #[allow(clippy::too_many_lines)]
    fn dispatch(&self, method: &str, params: &Value) -> Result<Value, RpcError> {
        let config = &self.config;
        match method {
            "initialize" => Ok(initialize_result(config)),
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
            "session.replay" => {
                let session_id = required_string(params, "sessionId")?;
                let run_id = params.get("runId").and_then(Value::as_str);
                let scope =
                    run_id.map_or_else(|| ReplayScope::session(&session_id), ReplayScope::run);
                let cursor = replay_cursor_from_params(params, scope)?;
                let archive = LocalStreamArchive::new(config.clone());
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
            "session.delete" => {
                let session_id = required_string(params, "sessionId")?;
                let mut store = LocalStore::open(config).map_err(RpcError::from)?;
                let deleted = store.delete_session(&session_id).map_err(RpcError::from)?;
                Ok(json!({"sessionId": session_id, "deleted": deleted}))
            }
            "session.output" => self.session_output(params),
            "run.prompt" => run_prompt(config, params),
            "run.start" => self.run_start(params),
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
                            -32_602,
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
            other => Err(RpcError::new(-32_601, format!("method not found: {other}"))),
        }
    }

    fn run_start(&self, params: &Value) -> Result<Value, RpcError> {
        let format = stream_payload_format(params)?;
        let command = run_command_from_params(&self.config, params, OutputMode::Json)?;
        let started = self
            .coordinator
            .start_run(command, None)
            .map_err(RpcError::from)?;
        let session_id = started.session_id.clone();
        let run_id = started.run_id.clone();
        self.spawn_run_notifications(started, format);
        Ok(json!({
            "sessionId": session_id,
            "runId": run_id,
            "status": "running",
            "payloadFormat": format.as_str(),
        }))
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
        let result = attachment_result(&attachment, format);
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
        let result = attachment_result(&attachment, format);
        self.spawn_attachment_notifications(&mut attachment, format);
        Ok(result)
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
                    .map_err(|error| RpcError::new(-32_000, error.to_string()))?,
                None => receiver
                    .recv()
                    .map_err(|error| RpcError::new(-32_000, error.to_string()))?,
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
        thread::spawn(move || {
            let session_id = started.session_id.clone();
            let run_id = started.run_id.clone();
            let _ = output_sender.send(notification(
                "run.started",
                &json!({
                    "sessionId": session_id,
                    "runId": run_id,
                    "status": "running",
                }),
            ));
            forward_run_events(&output_sender, started.events, format);
        });
    }

    fn spawn_attachment_notifications(
        &self,
        attachment: &mut RunAttachment,
        format: StreamPayloadFormat,
    ) {
        let Some(subscription) = attachment.subscription.take() else {
            return;
        };
        let output_sender = self.output_sender.clone();
        thread::spawn(move || forward_run_events(&output_sender, subscription, format));
    }
}

fn run_command_from_params(
    config: &CliConfig,
    params: &Value,
    output: OutputMode,
) -> Result<RunCommand, RpcError> {
    let prompt = required_string(params, "prompt")?;
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
        session_affinity_id: None,
    })
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

fn forward_run_events(
    output_sender: &mpsc::Sender<Value>,
    receiver: mpsc::Receiver<RunStreamEvent>,
    format: StreamPayloadFormat,
) {
    for event in receiver {
        let frame = match event {
            RunStreamEvent::Output(output) => {
                let Some(output) = output_item(&output, format) else {
                    continue;
                };
                notification("run.output", &json!(output))
            }
            RunStreamEvent::Status(status) => notification("run.status", &json!(status)),
            RunStreamEvent::Raw(_) => continue,
        };
        if output_sender.send(frame).is_err() {
            break;
        }
    }
}

fn status_is_terminal(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled")
}

fn initialize_result(config: &CliConfig) -> Value {
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
            "liveDisplay": true,
            "cancel": true,
            "steering": true,
            "attach": true,
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

fn run_prompt(config: &CliConfig, params: &Value) -> Result<Value, RpcError> {
    let command = run_command_from_params(config, params, OutputMode::Json)?;
    let output = CliService::open(config.clone())
        .map_err(RpcError::from)?
        .run_prompt(&command)
        .map_err(RpcError::from)?;
    serde_json::from_str(output.trim()).map_err(|error| RpcError::new(-32_000, error.to_string()))
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
        return Err(RpcError::new(-32_602, "config.get requires key or keys"));
    };
    let mut values = serde_json::Map::new();
    for key in keys {
        let Some(key) = key.as_str() else {
            return Err(RpcError::new(-32_602, "keys must be strings"));
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
            -32_602,
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
            -32_602,
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
        .ok_or_else(|| RpcError::new(-32_602, format!("missing string param: {key}")))
}

fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message},
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

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
        let server = RpcServer::new(config.clone(), output_sender);
        request_with_server(&server, id, method, params)
    }

    #[allow(clippy::needless_pass_by_value)]
    fn request_with_server(server: &RpcServer, id: u64, method: &str, params: Value) -> Value {
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
        let server = RpcServer::new(config, output_sender);
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
    fn run_start_streams_agui_payloads_and_session_output_replays() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let (output_sender, output_receiver) = mpsc::channel::<Value>();
        let server = RpcServer::new(config, output_sender);

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
}
