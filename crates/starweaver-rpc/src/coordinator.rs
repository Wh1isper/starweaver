//! RPC-owned active-run coordination over the public Agent SDK.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::Serialize;
use serde_json::{Value, json};
use starweaver_agent::{AgentControlHandle, AgentStreamDropPolicy, AgentStreamOptions};
use starweaver_core::{ConversationId, RunId, SessionId};
use starweaver_environment::{
    ShellProcessStatus, SwitchableEnvironmentProvider, SwitchableEnvironmentTarget,
};
use starweaver_rpc_core::{
    ALREADY_EXISTS, EnvironmentActiveMountParams, EnvironmentActiveUnmountParams,
    EnvironmentAttachmentRef, INVALID_PARAMS, RUN_CONFLICT, RpcError,
};
use starweaver_runtime::{AgentInput, AgentStreamRecord};
use starweaver_session::{InputPart, RunRecord, RunStatus, SessionStore};
use starweaver_storage::SqliteStorage;
use starweaver_stream::{
    DefaultDisplayMessageProjector, DisplayMessageProjector, DisplayProjectionContext,
    EnvironmentLifecycleEvent, ReplayCursor, ReplayEvent, ReplayEventKind, ReplayEventLog,
    ReplayScope, StreamTerminalMarker,
};
use tokio::sync::Notify;

use crate::{
    RpcAgentCatalog, RpcConfig, RpcHostError, RpcHostResult,
    environment::{resolve_rpc_environment, resolve_rpc_environment_target},
    environment_manager::EnvironmentAttachmentManager,
};

const DURABLE_RUN_ID_METADATA_KEY: &str = "starweaver.durable_run_id";
const RPC_PROFILE_METADATA_KEY: &str = "rpc.profile";

/// RPC run request after wire parameters have been validated.
#[derive(Clone, Debug)]
pub struct RpcRunRequest {
    /// Durable input evidence retained in the run record.
    pub durable_input: Vec<InputPart>,
    /// Canonical model-visible runtime input.
    pub input: AgentInput,
    /// Existing session id, or `None` to create one.
    pub session_id: Option<SessionId>,
    /// Optional run whose context should be restored.
    pub restore_from_run_id: Option<RunId>,
    /// RPC-owned profile selection.
    pub profile: String,
    /// Materialized host environment attachments for this run.
    pub environment_attachments: Vec<EnvironmentAttachmentRef>,
}

/// Stable status projection returned by RPC methods.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcRunStatus {
    /// Durable session id.
    pub session_id: String,
    /// Durable run id.
    pub run_id: String,
    /// Durable status name.
    pub status: String,
    /// Final output preview when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_preview: Option<String>,
    /// Safe runtime error when the run failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl RpcRunStatus {
    /// Return whether this status is terminal for RPC await semantics.
    #[must_use]
    pub fn terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            "completed" | "failed" | "cancelled" | "waiting"
        )
    }
}

/// Result of starting an RPC-owned live run.
#[derive(Clone, Debug)]
pub struct RpcStartedRun {
    /// Durable session id.
    pub session_id: SessionId,
    /// Durable run id.
    pub run_id: RunId,
    /// Effective environment attachments bound to the run.
    pub environment_attachments: Vec<EnvironmentAttachmentRef>,
}

#[derive(Clone)]
struct ActiveRun {
    status: RpcRunStatus,
    control: AgentControlHandle,
    events: Vec<ReplayEvent>,
    next_display_sequence: usize,
    next_event_sequence: usize,
    notify: Arc<Notify>,
    environment: Arc<SwitchableEnvironmentProvider>,
    environment_attachments: Vec<EnvironmentAttachmentRef>,
    environment_binding_version: u64,
    environment_idempotency: HashMap<String, EnvironmentMutationRecord>,
}

#[derive(Clone)]
struct EnvironmentMutationRecord {
    params_digest: String,
    result: Value,
    attachment: EnvironmentAttachmentRef,
}

/// RPC-owned active mount mutation outcome used by the service boundary.
pub struct RpcActiveMountOutcome {
    /// Wire result.
    pub result: Value,
    /// Whether this request applied a new mutation rather than replaying an idempotent result.
    pub applied: bool,
}

/// RPC-owned active unmount mutation outcome used by the service boundary.
pub struct RpcActiveUnmountOutcome {
    /// Wire result.
    pub result: Value,
    /// Attachment removed by the mutation.
    pub removed: EnvironmentAttachmentRef,
    /// Whether this request applied a new mutation rather than replaying an idempotent result.
    pub applied: bool,
}

/// Thin RPC-owned registry around live SDK control handles.
#[derive(Clone)]
pub struct RpcRuntimeCoordinator {
    config: RpcConfig,
    catalog: RpcAgentCatalog,
    storage: SqliteStorage,
    environment_manager: EnvironmentAttachmentManager,
    active: Arc<Mutex<HashMap<String, ActiveRun>>>,
}

impl RpcRuntimeCoordinator {
    /// Create an RPC coordinator. It does not share state with CLI/TUI.
    #[must_use]
    pub(crate) fn new(
        config: RpcConfig,
        catalog: RpcAgentCatalog,
        storage: SqliteStorage,
        environment_manager: EnvironmentAttachmentManager,
    ) -> Self {
        Self {
            config,
            catalog,
            storage,
            environment_manager,
            active: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Start one live run directly through `AgentRuntime`.
    ///
    /// # Errors
    ///
    /// Returns storage or runtime construction failures.
    #[allow(clippy::too_many_lines)]
    pub async fn start(&self, request: RpcRunRequest) -> RpcHostResult<RpcStartedRun> {
        let session = if let Some(session_id) = request.session_id.as_ref() {
            self.storage
                .session_store()
                .load_session(session_id)
                .await?
        } else {
            let storage = self.storage.clone();
            let profile = request.profile.clone();
            tokio::task::spawn_blocking(move || storage.create_session(Some(profile), None))
                .await
                .map_err(|error| RpcHostError::Runtime(format!("storage task failed: {error}")))??
        };
        let session_id = session.session_id.clone();
        let run_id = RunId::new();
        let mut run = RunRecord::new(
            session_id.clone(),
            run_id.clone(),
            session
                .state
                .conversation_id
                .clone()
                .unwrap_or_else(ConversationId::new),
        );
        run.input.clone_from(&request.durable_input);
        run.profile = Some(request.profile.clone());
        run.restore_from_run_id
            .clone_from(&request.restore_from_run_id);
        run.trigger_type = Some("rpc".to_string());
        run.status = RunStatus::Running;
        run.metadata
            .insert(RPC_PROFILE_METADATA_KEY.to_string(), json!(request.profile));
        let storage = self.storage.clone();
        let _run = tokio::task::spawn_blocking(move || storage.begin_run(run))
            .await
            .map_err(|error| RpcHostError::Runtime(format!("storage task failed: {error}")))??;

        let mut state = match request.restore_from_run_id.as_ref() {
            Some(restore_run_id) => {
                let storage = self.storage.clone();
                let storage_session_id = session_id.clone();
                let restore_run_id_value = restore_run_id.clone();
                let restore_run_id_text = restore_run_id.as_str().to_string();
                tokio::task::spawn_blocking(move || {
                    storage.load_run_context(&storage_session_id, &restore_run_id_value)
                })
                .await
                .map_err(|error| RpcHostError::Runtime(format!("storage task failed: {error}")))??
                .ok_or_else(|| {
                    RpcHostError::NotFound(format!(
                        "run context {}:{}",
                        session_id.as_str(),
                        restore_run_id_text
                    ))
                })?
            }
            None => session.state,
        };
        state.session_id = Some(session_id.clone());
        state.metadata.insert(
            DURABLE_RUN_ID_METADATA_KEY.to_string(),
            json!(run_id.as_str()),
        );
        state
            .metadata
            .insert(RPC_PROFILE_METADATA_KEY.to_string(), json!(request.profile));

        let resolved_environment = resolve_rpc_environment(
            &self.config.workspace_root,
            session_id.as_str(),
            &request.environment_attachments,
        )?;
        let session_store = Arc::new(self.storage.session_store());
        let stream_archive = Arc::new(self.storage.stream_archive());
        let replay_log = self.storage.replay_event_log();
        let mut runtime = self
            .catalog
            .runtime_builder(&request.profile)?
            .state(state)
            .environment(resolved_environment.provider.clone())
            .durable_session_id(session_id.clone())
            .session_store(session_store)
            .stream_archive(stream_archive)
            .build();
        let input = request.input.clone();
        let mut handle = runtime
            .try_stream_with_stream_options(
                input.clone(),
                AgentStreamOptions::new().drop_policy(AgentStreamDropPolicy::Backpressure),
            )
            .map_err(|error| RpcHostError::Runtime(error.to_string()))?;
        let control = handle.control_handle();
        let notify = Arc::new(Notify::new());
        self.environment_manager
            .mark_run_started(run_id.as_str(), &resolved_environment.attachments)
            .map_err(|error| RpcHostError::Invalid(error.message))?;
        let active_run = ActiveRun {
            status: RpcRunStatus {
                session_id: session_id.as_str().to_string(),
                run_id: run_id.as_str().to_string(),
                status: "running".to_string(),
                output_preview: None,
                error: None,
            },
            control,
            events: Vec::new(),
            next_display_sequence: 0,
            next_event_sequence: 0,
            notify: Arc::clone(&notify),
            environment: Arc::clone(&resolved_environment.switchable),
            environment_attachments: resolved_environment.attachments.clone(),
            environment_binding_version: 1,
            environment_idempotency: HashMap::new(),
        };
        if let Err(error) = self
            .active
            .lock()
            .map_err(active_registry_error)
            .map(|mut registry| {
                registry.insert(run_id.as_str().to_string(), active_run);
            })
        {
            if let Err(cleanup) = self.environment_manager.mark_run_finished(run_id.as_str()) {
                return Err(RpcHostError::Runtime(format!(
                    "{error}; environment lease cleanup failed: {}",
                    cleanup.message
                )));
            }
            return Err(error);
        }

        let active = Arc::clone(&self.active);
        let environment_manager = self.environment_manager.clone();
        let worker_session_id = session_id.clone();
        let worker_run_id = run_id.clone();
        tokio::spawn(async move {
            let projection_context =
                DisplayProjectionContext::new(worker_session_id.clone(), worker_run_id.clone());
            while let Some(record) = handle.recv().await {
                publish_record(
                    &active,
                    &replay_log,
                    &worker_run_id,
                    &projection_context,
                    &record,
                )
                .await;
            }
            let completion = runtime.finish_stream(input, handle).await;
            let (status, output_preview, error) = match completion {
                Ok(result) => (
                    run_status_name(result.result.state.status).to_string(),
                    (!result.result.output.is_empty()).then_some(result.result.output),
                    None,
                ),
                Err(error) => {
                    let status = if error.to_string().contains("interrupted") {
                        "cancelled"
                    } else {
                        "failed"
                    };
                    (status.to_string(), None, Some(error.to_string()))
                }
            };
            let marker = match status.as_str() {
                "completed" => StreamTerminalMarker::RunCompleted,
                "cancelled" => StreamTerminalMarker::RunCancelled {
                    reason: error
                        .clone()
                        .unwrap_or_else(|| "agent run cancelled".to_string()),
                },
                _ => StreamTerminalMarker::RunFailed {
                    code: "agent_failed".to_string(),
                    message: error
                        .clone()
                        .unwrap_or_else(|| "agent run failed".to_string()),
                },
            };
            let terminal_event = if let Ok(mut registry) = active.lock()
                && let Some(active_run) = registry.get_mut(worker_run_id.as_str())
            {
                active_run.status.status = status;
                active_run.status.output_preview = output_preview;
                active_run.status.error = error;
                let event = ReplayEvent::new(
                    ReplayScope::run(worker_run_id.as_str()),
                    active_run.next_event_sequence,
                    ReplayEventKind::Terminal { marker },
                );
                active_run.next_event_sequence = active_run.next_event_sequence.saturating_add(1);
                active_run.events.push(event.clone());
                active_run.notify.notify_waiters();
                Some(event)
            } else {
                None
            };
            if let Some(event) = terminal_event {
                let scope = event.scope.clone();
                if let Err(persist_error) = replay_log.append(scope, event).await
                    && let Ok(mut registry) = active.lock()
                    && let Some(active_run) = registry.get_mut(worker_run_id.as_str())
                {
                    active_run.status.error.get_or_insert_with(|| {
                        format!("failed to persist terminal replay event: {persist_error}")
                    });
                }
            }
            if let Err(cleanup) = environment_manager.mark_run_finished(worker_run_id.as_str())
                && let Ok(mut registry) = active.lock()
                && let Some(active_run) = registry.get_mut(worker_run_id.as_str())
            {
                active_run.status.error.get_or_insert_with(|| {
                    format!("environment lease cleanup failed: {}", cleanup.message)
                });
            }
        });

        Ok(RpcStartedRun {
            session_id,
            run_id,
            environment_attachments: resolved_environment.attachments,
        })
    }

    /// Return the current active or durable status.
    ///
    /// # Errors
    ///
    /// Returns an error when the active registry or durable store cannot be read.
    pub async fn status(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> RpcHostResult<RpcRunStatus> {
        let active_status = self
            .active
            .lock()
            .map_err(active_registry_error)?
            .get(run_id.as_str())
            .map(|run| run.status.clone());
        if let Some(status) = active_status {
            if status.session_id != session_id.as_str() {
                return Err(RpcHostError::NotFound(format!(
                    "run {}:{}",
                    session_id.as_str(),
                    run_id.as_str()
                )));
            }
            return Ok(status);
        }
        let run = self
            .storage
            .session_store()
            .load_run(session_id, run_id)
            .await?;
        Ok(status_from_record(&run))
    }

    /// Wait until a run is terminal or the optional timeout elapses.
    ///
    /// # Errors
    ///
    /// Returns an error when status cannot be read or the wait times out.
    pub async fn await_terminal(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        timeout: Option<Duration>,
    ) -> RpcHostResult<RpcRunStatus> {
        let wait = async {
            loop {
                let status = self.status(session_id, run_id).await?;
                if status.terminal() {
                    return Ok(status);
                }
                let notify = self
                    .active
                    .lock()
                    .map_err(active_registry_error)?
                    .get(run_id.as_str())
                    .map(|run| Arc::clone(&run.notify));
                let Some(notify) = notify else {
                    return Ok(status);
                };
                notify.notified().await;
            }
        };
        match timeout {
            Some(timeout) => tokio::time::timeout(timeout, wait)
                .await
                .map_err(|_| RpcHostError::Runtime("run.await timed out".to_string()))?,
            None => wait.await,
        }
    }

    /// Queue a steering message through the SDK control handle.
    ///
    /// # Errors
    ///
    /// Returns an error when the run is not active or no longer accepts control input.
    pub async fn steer(
        &self,
        run_id: &RunId,
        steering_id: String,
        text: String,
    ) -> RpcHostResult<Value> {
        let control = self.control(run_id)?;
        let receipt = control
            .steer(steering_id, text)
            .await
            .map_err(|error| RpcHostError::Runtime(error.to_string()))?;
        Ok(json!({
            "runId": run_id.as_str(),
            "steeringId": receipt.id,
            "queued": receipt.pending_delivery,
        }))
    }

    /// Cooperatively cancel a live run.
    ///
    /// # Errors
    ///
    /// Returns an error when the run is not active in this RPC process.
    pub fn cancel(&self, run_id: &RunId, reason: Option<String>) -> RpcHostResult<Value> {
        let control = self.control(run_id)?;
        let receipt = control.interrupt(reason);
        Ok(json!({
            "runId": run_id.as_str(),
            "cancelled": true,
            "controlId": receipt.id,
        }))
    }

    /// Replay persisted events plus the process-local live tail.
    ///
    /// # Errors
    ///
    /// Returns an error when durable replay or the active registry cannot be read.
    pub async fn replay(
        &self,
        run_id: &RunId,
        cursor: Option<ReplayCursor>,
        limit: Option<usize>,
    ) -> RpcHostResult<Vec<ReplayEvent>> {
        let scope = ReplayScope::run(run_id.as_str());
        let mut events = self
            .storage
            .replay_event_log()
            .replay_after(&scope, cursor.clone(), limit)
            .await?;
        let live = self
            .active
            .lock()
            .map_err(active_registry_error)?
            .get(run_id.as_str())
            .map_or_else(Vec::new, |run| run.events.clone());
        for event in live {
            if cursor
                .as_ref()
                .is_none_or(|cursor| event.sequence > cursor.sequence)
                && !events
                    .iter()
                    .any(|persisted| persisted.sequence == event.sequence)
            {
                events.push(event);
            }
        }
        events.sort_by_key(|event| event.sequence);
        if let Some(limit) = limit {
            events.truncate(limit);
        }
        Ok(events)
    }

    #[allow(clippy::significant_drop_tightening)]
    pub(crate) fn active_run_session_id(&self, run_id: &str) -> Result<String, RpcError> {
        let registry = self
            .active
            .lock()
            .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
        let run = active_mutable_run(&registry, run_id)?;
        Ok(run.status.session_id.clone())
    }

    #[allow(clippy::significant_drop_tightening)]
    pub(crate) fn active_environment_list(&self, run_id: &str) -> Result<Value, RpcError> {
        let registry = self
            .active
            .lock()
            .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
        let run = active_mutable_run(&registry, run_id)?;
        Ok(json!({
            "runId": run_id,
            "environment": environment_summary(
                run.environment_binding_version,
                &run.environment_attachments,
            ),
        }))
    }

    #[allow(clippy::significant_drop_tightening, clippy::too_many_lines)]
    pub(crate) async fn active_environment_mount(
        &self,
        params: &EnvironmentActiveMountParams,
        attachment: EnvironmentAttachmentRef,
        params_digest: &str,
    ) -> Result<RpcActiveMountOutcome, RpcError> {
        let operation_id = format!("envop_{}", uuid::Uuid::new_v4());
        let (result, control, lifecycle_event) = {
            let mut registry = self
                .active
                .lock()
                .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
            let run = active_mutable_run_mut(&mut registry, &params.run_id)?;
            let mutation_key = params
                .idempotency_key
                .as_deref()
                .map(|key| format!("mount:{key}"));
            if let Some(key) = mutation_key.as_ref()
                && let Some(record) = run.environment_idempotency.get(key)
            {
                ensure_idempotency_digest(record, params_digest)?;
                return Ok(RpcActiveMountOutcome {
                    result: record.result.clone(),
                    applied: false,
                });
            }
            ensure_expected_binding(
                run.environment_binding_version,
                params.expected_binding_version,
            )?;
            let previous_binding_version = run.environment_binding_version;
            let existing_index = run
                .environment_attachments
                .iter()
                .position(|current| current.id == attachment.id);
            if existing_index.is_some() && !params.replace {
                return Err(RpcError::new(
                    ALREADY_EXISTS,
                    format!("environment mount already exists: {}", attachment.id),
                ));
            }
            let mut mounted = attachment;
            let mut updated = run.environment_attachments.clone();
            if let Some(index) = existing_index {
                if updated[index].is_default && !mounted.is_default {
                    mounted.is_default = true;
                }
                if updated[index].is_default_for_shell && !mounted.is_default_for_shell {
                    mounted.is_default_for_shell = true;
                }
                updated[index] = mounted.clone();
            } else {
                updated.push(mounted.clone());
            }
            normalize_default_flags(&mut updated)?;
            let target = resolve_rpc_environment_target(
                &self.config.workspace_root,
                &run.status.session_id,
                &updated,
            )
            .map_err(RpcError::from)?;
            let process_provider = target.provider.clone().process_shell_provider();
            run.environment
                .replace_target(SwitchableEnvironmentTarget::new(
                    target.provider,
                    process_provider,
                ))
                .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
            run.environment_attachments = target.attachments;
            run.environment_binding_version = run.environment_binding_version.saturating_add(1);
            let lifecycle_event = append_environment_lifecycle_event(
                run,
                &operation_id,
                "environment_mounted",
                &json!({"action": "mounted", "mount": mount_summary(&mounted, "ready")}),
            );
            let result = json!({
                "runId": params.run_id,
                "operationId": operation_id,
                "mountId": mounted.id,
                "replace": params.replace,
                "mount": mount_summary(&mounted, "ready"),
                "previousBindingVersion": previous_binding_version,
                "bindingVersion": run.environment_binding_version,
                "environment": environment_summary(
                    run.environment_binding_version,
                    &run.environment_attachments,
                ),
                "eventCursor": cursor_value(&params.run_id, lifecycle_event.sequence),
            });
            if let Some(key) = mutation_key {
                run.environment_idempotency.insert(
                    key,
                    EnvironmentMutationRecord {
                        params_digest: params_digest.to_string(),
                        result: result.clone(),
                        attachment: mounted,
                    },
                );
            }
            (result, run.control.clone(), lifecycle_event)
        };
        let lifecycle_scope = lifecycle_event.scope.clone();
        self.storage
            .replay_event_log()
            .append(lifecycle_scope, lifecycle_event)
            .await
            .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
        if params.inject_context {
            let _receipt = control
                .steer(
                    operation_id,
                    format!(
                        "Environment mount {} is now available at /environment/{}.",
                        attachment_id_from_result(&result),
                        attachment_id_from_result(&result),
                    ),
                )
                .await
                .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
        }
        Ok(RpcActiveMountOutcome {
            result,
            applied: true,
        })
    }

    #[allow(clippy::significant_drop_tightening, clippy::too_many_lines)]
    pub(crate) async fn active_environment_unmount(
        &self,
        params: &EnvironmentActiveUnmountParams,
        params_digest: &str,
    ) -> Result<RpcActiveUnmountOutcome, RpcError> {
        let operation_id = format!("envop_{}", uuid::Uuid::new_v4());
        let (removed, mut updated, binding_version, environment, session_id, control) = {
            let registry = self
                .active
                .lock()
                .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
            let run = active_mutable_run(&registry, &params.run_id)?;
            let mutation_key = params
                .idempotency_key
                .as_deref()
                .map(|key| format!("unmount:{key}"));
            if let Some(key) = mutation_key.as_ref()
                && let Some(record) = run.environment_idempotency.get(key)
            {
                ensure_idempotency_digest(record, params_digest)?;
                return Ok(RpcActiveUnmountOutcome {
                    result: record.result.clone(),
                    removed: record.attachment.clone(),
                    applied: false,
                });
            }
            ensure_expected_binding(
                run.environment_binding_version,
                params.expected_binding_version,
            )?;
            let index = run
                .environment_attachments
                .iter()
                .position(|attachment| attachment.id == params.mount_id)
                .ok_or_else(|| {
                    RpcError::new(
                        INVALID_PARAMS,
                        format!("environment mount not found: {}", params.mount_id),
                    )
                })?;
            if run.environment_attachments.len() == 1 {
                return Err(RpcError::new(
                    RUN_CONFLICT,
                    "cannot unmount the only active environment mount",
                ));
            }
            let mut updated = run.environment_attachments.clone();
            let removed = updated.remove(index);
            apply_unmount_defaults(&removed, &mut updated, params)?;
            (
                removed,
                updated,
                run.environment_binding_version,
                Arc::clone(&run.environment),
                run.status.session_id.clone(),
                run.control.clone(),
            )
        };
        if let Some(process_provider) = environment
            .process_provider()
            .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?
        {
            let live = process_provider
                .list_processes()
                .await
                .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?
                .into_iter()
                .any(|process| {
                    process.status == ShellProcessStatus::Running
                        && process
                            .process_id
                            .strip_prefix(&params.mount_id)
                            .is_some_and(|suffix| suffix.starts_with(':'))
                });
            if live {
                return Err(RpcError::new(
                    RUN_CONFLICT,
                    format!(
                        "environment mount has live background processes: {}",
                        params.mount_id
                    ),
                ));
            }
        }
        normalize_default_flags(&mut updated)?;
        let target =
            resolve_rpc_environment_target(&self.config.workspace_root, &session_id, &updated)
                .map_err(RpcError::from)?;
        let (result, lifecycle_event) = {
            let mut registry = self
                .active
                .lock()
                .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
            let run = active_mutable_run_mut(&mut registry, &params.run_id)?;
            if run.environment_binding_version != binding_version {
                return Err(RpcError::new(
                    RUN_CONFLICT,
                    "environment binding changed while unmount was being prepared",
                ));
            }
            let process_provider = target.provider.clone().process_shell_provider();
            run.environment
                .replace_target(SwitchableEnvironmentTarget::new(
                    target.provider,
                    process_provider,
                ))
                .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
            run.environment_attachments = target.attachments;
            run.environment_binding_version = run.environment_binding_version.saturating_add(1);
            let lifecycle_event = append_environment_lifecycle_event(
                run,
                &operation_id,
                "environment_unmounted",
                &json!({
                    "action": "unmounted",
                    "removedMount": mount_summary(&removed, "detached"),
                }),
            );
            let result = json!({
                "runId": params.run_id,
                "operationId": operation_id,
                "mountId": removed.id,
                "removedMount": mount_summary(&removed, "detached"),
                "previousBindingVersion": binding_version,
                "bindingVersion": run.environment_binding_version,
                "environment": environment_summary(
                    run.environment_binding_version,
                    &run.environment_attachments,
                ),
                "eventCursor": cursor_value(&params.run_id, lifecycle_event.sequence),
            });
            if let Some(key) = params.idempotency_key.as_deref() {
                run.environment_idempotency.insert(
                    format!("unmount:{key}"),
                    EnvironmentMutationRecord {
                        params_digest: params_digest.to_string(),
                        result: result.clone(),
                        attachment: removed.clone(),
                    },
                );
            }
            (result, lifecycle_event)
        };
        let lifecycle_scope = lifecycle_event.scope.clone();
        self.storage
            .replay_event_log()
            .append(lifecycle_scope, lifecycle_event)
            .await
            .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
        if params.inject_context {
            let _receipt = control
                .steer(
                    operation_id,
                    format!("Environment mount {} was removed.", params.mount_id),
                )
                .await
                .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
        }
        Ok(RpcActiveUnmountOutcome {
            result,
            removed,
            applied: true,
        })
    }

    fn control(&self, run_id: &RunId) -> RpcHostResult<AgentControlHandle> {
        self.active
            .lock()
            .map_err(active_registry_error)?
            .get(run_id.as_str())
            .map(|run| run.control.clone())
            .ok_or_else(|| RpcHostError::NotFound(format!("active run {}", run_id.as_str())))
    }
}

fn active_mutable_run<'a>(
    registry: &'a HashMap<String, ActiveRun>,
    run_id: &str,
) -> Result<&'a ActiveRun, RpcError> {
    let run = registry
        .get(run_id)
        .ok_or_else(|| RpcError::new(RUN_CONFLICT, format!("active run not found: {run_id}")))?;
    if run.status.terminal() {
        return Err(RpcError::new(
            RUN_CONFLICT,
            format!("run is no longer active: {run_id}"),
        ));
    }
    Ok(run)
}

fn active_mutable_run_mut<'a>(
    registry: &'a mut HashMap<String, ActiveRun>,
    run_id: &str,
) -> Result<&'a mut ActiveRun, RpcError> {
    let run = registry
        .get_mut(run_id)
        .ok_or_else(|| RpcError::new(RUN_CONFLICT, format!("active run not found: {run_id}")))?;
    if run.status.terminal() {
        return Err(RpcError::new(
            RUN_CONFLICT,
            format!("run is no longer active: {run_id}"),
        ));
    }
    Ok(run)
}

fn ensure_expected_binding(current: u64, expected: Option<u64>) -> Result<(), RpcError> {
    if expected.is_some_and(|expected| expected != current) {
        return Err(RpcError::new(
            RUN_CONFLICT,
            format!(
                "environment binding version conflict: expected {}, current {current}",
                expected.unwrap_or_default()
            ),
        ));
    }
    Ok(())
}

fn ensure_idempotency_digest(
    record: &EnvironmentMutationRecord,
    params_digest: &str,
) -> Result<(), RpcError> {
    if record.params_digest == params_digest {
        Ok(())
    } else {
        Err(RpcError::new(
            RUN_CONFLICT,
            "idempotency key was already used with different active environment params",
        ))
    }
}

fn normalize_default_flags(attachments: &mut [EnvironmentAttachmentRef]) -> Result<(), RpcError> {
    if attachments.is_empty() {
        return Err(RpcError::new(
            RUN_CONFLICT,
            "active environment requires at least one mount",
        ));
    }
    let default_count = attachments
        .iter()
        .filter(|attachment| attachment.is_default)
        .count();
    if default_count == 0 && attachments.len() == 1 {
        attachments[0].is_default = true;
    } else if default_count != 1 {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "active environment requires exactly one default mount",
        ));
    }
    let shell_default_count = attachments
        .iter()
        .filter(|attachment| attachment.is_default_for_shell)
        .count();
    if shell_default_count > 1 {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "active environment allows at most one default shell mount",
        ));
    }
    if let Some(shell_default) = attachments
        .iter()
        .find(|attachment| attachment.is_default_for_shell)
        && !matches!(
            shell_default.resolved_mode(),
            starweaver_rpc_core::EnvironmentAttachmentAccessMode::ReadWrite
        )
    {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "default shell mount must be read-write",
        ));
    }
    if shell_default_count == 0
        && let Some(default) = attachments.iter_mut().find(|attachment| {
            attachment.is_default
                && matches!(
                    attachment.resolved_mode(),
                    starweaver_rpc_core::EnvironmentAttachmentAccessMode::ReadWrite
                )
        })
    {
        default.is_default_for_shell = true;
    }
    Ok(())
}

fn apply_unmount_defaults(
    removed: &EnvironmentAttachmentRef,
    updated: &mut [EnvironmentAttachmentRef],
    params: &EnvironmentActiveUnmountParams,
) -> Result<(), RpcError> {
    if removed.is_default {
        let new_default = params.new_default_mount_id.as_deref().ok_or_else(|| {
            RpcError::new(
                INVALID_PARAMS,
                "newDefaultMountId is required when removing the default mount",
            )
        })?;
        let replacement = updated
            .iter_mut()
            .find(|attachment| attachment.id == new_default)
            .ok_or_else(|| {
                RpcError::new(
                    INVALID_PARAMS,
                    format!("new default environment mount not found: {new_default}"),
                )
            })?;
        replacement.is_default = true;
    }
    if removed.is_default_for_shell {
        for attachment in updated.iter_mut() {
            attachment.is_default_for_shell = false;
        }
        if let Some(new_default) = params.new_default_shell_mount_id.as_deref() {
            let replacement = updated
                .iter_mut()
                .find(|attachment| attachment.id == new_default)
                .ok_or_else(|| {
                    RpcError::new(
                        INVALID_PARAMS,
                        format!("new default shell mount not found: {new_default}"),
                    )
                })?;
            replacement.is_default_for_shell = true;
        }
    }
    Ok(())
}

fn append_environment_lifecycle_event(
    run: &mut ActiveRun,
    operation_id: &str,
    operation_kind: &str,
    extra: &Value,
) -> ReplayEvent {
    let sequence = run.next_event_sequence;
    run.next_event_sequence = run.next_event_sequence.saturating_add(1);
    let extra = extra.as_object().cloned().unwrap_or_default();
    let lifecycle = EnvironmentLifecycleEvent {
        operation_kind: operation_kind.to_string(),
        session_id: run.status.session_id.clone(),
        run_id: run.status.run_id.clone(),
        binding_version: run.environment_binding_version,
        environment: environment_summary(
            run.environment_binding_version,
            &run.environment_attachments,
        ),
        operation_id: Some(operation_id.to_string()),
        extra,
    };
    let event = ReplayEvent::new(
        ReplayScope::run(&run.status.run_id),
        sequence,
        ReplayEventKind::EnvironmentLifecycle(Box::new(lifecycle)),
    );
    run.events.push(event.clone());
    run.notify.notify_waiters();
    event
}

fn environment_summary(binding_version: u64, attachments: &[EnvironmentAttachmentRef]) -> Value {
    json!({
        "bindingVersion": binding_version,
        "defaultMountId": attachments
            .iter()
            .find(|attachment| attachment.is_default)
            .map(|attachment| attachment.id.clone()),
        "defaultShellMountId": attachments
            .iter()
            .find(|attachment| attachment.is_default_for_shell)
            .map(|attachment| attachment.id.clone()),
        "mounts": attachments
            .iter()
            .map(|attachment| mount_summary(attachment, "ready"))
            .collect::<Vec<_>>(),
    })
}

fn mount_summary(attachment: &EnvironmentAttachmentRef, status: &str) -> Value {
    json!({
        "id": attachment.id,
        "kind": attachment.kind,
        "root": format!("/environment/{}", attachment.id),
        "mode": attachment.resolved_mode(),
        "default": attachment.is_default,
        "defaultForShell": attachment.is_default_for_shell,
        "status": status,
        "environmentId": attachment.environment_id,
        "metadata": attachment.metadata,
    })
}

fn cursor_value(run_id: &str, sequence: usize) -> Value {
    json!(ReplayCursor::replay_event(
        ReplayScope::run(run_id),
        sequence,
    ))
}

fn attachment_id_from_result(result: &Value) -> &str {
    result
        .get("mountId")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
}

async fn publish_record(
    active: &Arc<Mutex<HashMap<String, ActiveRun>>>,
    replay_log: &starweaver_storage::SqliteReplayEventLog,
    run_id: &RunId,
    projection_context: &DisplayProjectionContext,
    record: &AgentStreamRecord,
) {
    let messages = DefaultDisplayMessageProjector
        .project(projection_context, record)
        .await;
    if messages.is_empty() {
        return;
    }
    let scope = ReplayScope::run(run_id.as_str());
    let events = {
        let Ok(mut registry) = active.lock() else {
            return;
        };
        let Some(active_run) = registry.get_mut(run_id.as_str()) else {
            return;
        };
        let mut events = Vec::with_capacity(messages.len());
        for mut message in messages {
            message.sequence = active_run.next_display_sequence;
            active_run.next_display_sequence = active_run.next_display_sequence.saturating_add(1);
            let event =
                ReplayEvent::display_at(scope.clone(), active_run.next_event_sequence, message);
            active_run.next_event_sequence = active_run.next_event_sequence.saturating_add(1);
            active_run.events.push(event.clone());
            events.push(event);
        }
        active_run.notify.notify_waiters();
        events
    };
    for event in events {
        if let Err(error) = replay_log.append(scope.clone(), event).await
            && let Ok(mut registry) = active.lock()
            && let Some(active_run) = registry.get_mut(run_id.as_str())
        {
            active_run
                .status
                .error
                .get_or_insert_with(|| format!("failed to persist replay event: {error}"));
        }
    }
}

fn status_from_record(run: &RunRecord) -> RpcRunStatus {
    RpcRunStatus {
        session_id: run.session_id.as_str().to_string(),
        run_id: run.run_id.as_str().to_string(),
        status: durable_run_status_name(run.status).to_string(),
        output_preview: run.output_preview.clone(),
        error: None,
    }
}

const fn durable_run_status_name(status: RunStatus) -> &'static str {
    status.as_str()
}

const fn run_status_name(status: starweaver_runtime::RunStatus) -> &'static str {
    status.as_str()
}

#[allow(clippy::needless_pass_by_value)]
fn active_registry_error<T>(error: std::sync::PoisonError<T>) -> RpcHostError {
    RpcHostError::Runtime(format!("active run registry poisoned: {error}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[tokio::test]
    async fn starts_and_awaits_a_run_without_cli_types() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        let storage = SqliteStorage::open(&config.database_path).unwrap();
        let catalog = RpcAgentCatalog::new(config.clone()).unwrap();
        let coordinator = RpcRuntimeCoordinator::new(
            config,
            catalog,
            storage,
            EnvironmentAttachmentManager::new(),
        );
        let started = coordinator
            .start(RpcRunRequest {
                durable_input: vec![InputPart::text("hello")],
                input: AgentInput::text("hello"),
                session_id: None,
                restore_from_run_id: None,
                profile: "default".to_string(),
                environment_attachments: Vec::new(),
            })
            .await
            .unwrap();
        let status = coordinator
            .await_terminal(
                &started.session_id,
                &started.run_id,
                Some(Duration::from_secs(5)),
            )
            .await
            .unwrap();
        assert_eq!(status.status, "completed", "{status:?}");
        assert_eq!(status.output_preview.as_deref(), Some("ok"));
    }
}
