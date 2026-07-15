//! RPC-owned active-run coordination over the public Agent SDK.

use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use serde::Serialize;
use serde_json::{Value, json};
use starweaver_agent::{
    AgentContext, AgentControlHandle, AgentSessionControlHandle, AgentSessionQueryHandle,
    AgentStreamDropPolicy, AgentStreamOptions, BackgroundSubagentSupervisor,
    BackgroundSubagentTaskResult, SubagentDelegationMode, attach_agent_session_control,
    attach_agent_session_query,
};
use starweaver_core::{ConversationId, RunId, SessionId, SubagentAttemptId};
use starweaver_environment::{
    ShellProcessStatus, SwitchableEnvironmentProvider, SwitchableEnvironmentTarget,
};
use starweaver_rpc_core::{
    ALREADY_EXISTS, EnvironmentActiveMountParams, EnvironmentActiveUnmountParams,
    EnvironmentAttachmentRef, INVALID_PARAMS, RUN_CONFLICT, RpcError,
};
use starweaver_runtime::{AgentInput, AgentStreamRecord};
use starweaver_session::{
    AcquireBackgroundSubagentContinuation, AcquireRunAdmission, AgentSessionOperation,
    AgentSessionScope, BackgroundSubagentContinuationCause, BackgroundSubagentRecord,
    DurableBackgroundSubagentDeliveryStatus, DurableBackgroundSubagentRetentionStatus,
    DurableControlReceipt, InputPart, LOCAL_SESSION_NAMESPACE, ManagedRunTarget, RunAdmissionLease,
    RunAdmissionReceipt, RunRecord, RunStatus, SessionDeletionFence, SessionStatus, SessionStore,
    SessionStoreError,
};
use starweaver_storage::SqliteStorage;
use starweaver_stream::{
    DefaultDisplayMessageProjector, DisplayMessageProjector, DisplayProjectionContext,
    EnvironmentLifecycleEvent, ReplayCursor, ReplayEvent, ReplayEventKind, ReplayEventLog,
    ReplayScope, StreamTerminalMarker,
};
use tokio::{sync::watch, task::JoinHandle};

use crate::{
    RpcAgentCatalog, RpcConfig, RpcHostError, RpcHostResult,
    environment::{resolve_rpc_environment, resolve_rpc_environment_target},
    environment_manager::EnvironmentAttachmentManager,
    session_management::{RpcAgentSessionAdapter, command_fingerprint},
};

const DURABLE_RUN_ID_METADATA_KEY: &str = "starweaver.durable_run_id";
const RPC_PROFILE_METADATA_KEY: &str = "rpc.profile";
const ACTIVE_LEASE_TTL: Duration = Duration::from_secs(30);
const ACTIVE_LEASE_HEARTBEAT: Duration = Duration::from_secs(10);
const TERMINAL_CACHE_LIMIT: usize = 64;
const BACKGROUND_COMPLETION_TASK_LIMIT: usize = 256;
const BACKGROUND_RECORD_SCAN_LIMIT: usize = 1_024;
const BACKGROUND_RETENTION_CLEANUP_LIMIT: usize = 256;
const BACKGROUND_CONTINUATION_LEASE_TTL: Duration = Duration::from_secs(30);

type RpcBoxFuture<'a, T> = Pin<Box<dyn Future<Output = RpcHostResult<T>> + Send + 'a>>;

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
    /// Stable start idempotency key.
    pub idempotency_key: String,
    /// Normalized typed start command fingerprint.
    pub command_fingerprint: String,
    /// Install profile-granted session query/control handles for this run.
    pub install_session_management: bool,
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
        matches!(self.status.as_str(), "completed" | "failed" | "cancelled")
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
    /// Stable durable admission receipt id.
    pub admission_id: String,
    /// Fencing generation accepted for this run.
    pub fencing_generation: u64,
    /// Durable status observed when the receipt was returned.
    pub status: RunStatus,
    /// True when this is an exact same-key, same-command replay.
    pub idempotent_replay: bool,
}

#[derive(Clone)]
struct ActiveRun {
    status_tx: watch::Sender<RpcRunStatus>,
    control: AgentControlHandle,
    lease: RunAdmissionLease,
    events: Vec<ReplayEvent>,
    next_display_sequence: usize,
    next_event_sequence: usize,
    environment: Arc<SwitchableEnvironmentProvider>,
    environment_attachments: Vec<EnvironmentAttachmentRef>,
    environment_binding_version: u64,
    environment_idempotency: HashMap<String, EnvironmentMutationRecord>,
}

#[derive(Clone)]
struct TerminalRun {
    target: ManagedRunTarget,
    status: RpcRunStatus,
    events: Vec<ReplayEvent>,
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
    active: Arc<Mutex<HashMap<ManagedRunTarget, ActiveRun>>>,
    terminal: Arc<Mutex<VecDeque<TerminalRun>>>,
    tasks: Arc<Mutex<HashMap<ManagedRunTarget, JoinHandle<()>>>>,
    background_tasks: Arc<Mutex<HashMap<SubagentAttemptId, JoinHandle<()>>>>,
    background_reconciler: Arc<Mutex<Option<JoinHandle<()>>>>,
    supervisors: Arc<Mutex<HashMap<SessionId, Arc<BackgroundSubagentSupervisor>>>>,
    accepting: Arc<AtomicBool>,
    host_instance_id: Arc<String>,
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
            terminal: Arc::new(Mutex::new(VecDeque::with_capacity(TERMINAL_CACHE_LIMIT))),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            background_tasks: Arc::new(Mutex::new(HashMap::new())),
            background_reconciler: Arc::new(Mutex::new(None)),
            supervisors: Arc::new(Mutex::new(HashMap::new())),
            accepting: Arc::new(AtomicBool::new(true)),
            host_instance_id: Arc::new(format!("rpc-host-{}", uuid::Uuid::new_v4())),
        }
    }

    fn supervisor_for_session(
        &self,
        session_id: &SessionId,
    ) -> RpcHostResult<Arc<BackgroundSubagentSupervisor>> {
        let mut supervisors = self.supervisors.lock().map_err(active_registry_error)?;
        if let Some(supervisor) = supervisors.get(session_id) {
            return Ok(supervisor.clone());
        }
        let coordinator = self.clone();
        let callback = Arc::new(move |result: &BackgroundSubagentTaskResult| {
            coordinator.spawn_background_completion_task(result.attempt_id.clone());
        });
        let store: Arc<dyn SessionStore> = Arc::new(self.storage.session_store());
        let supervisor = Arc::new(
            BackgroundSubagentSupervisor::new()
                .with_durable_store(store, LOCAL_SESSION_NAMESPACE)
                .with_durable_owner((*self.host_instance_id).clone(), 1, ACTIVE_LEASE_TTL)
                .with_completion_callback(callback),
        );
        supervisors.insert(session_id.clone(), supervisor.clone());
        drop(supervisors);
        Ok(supervisor)
    }

    fn ensure_background_reconciler(&self) -> RpcHostResult<()> {
        let mut slot = self
            .background_reconciler
            .lock()
            .map_err(active_registry_error)?;
        if slot.as_ref().is_some_and(|task| !task.is_finished()) {
            return Ok(());
        }
        let coordinator = self.clone();
        *slot = Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(ACTIVE_LEASE_HEARTBEAT);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            loop {
                interval.tick().await;
                if !coordinator.accepting.load(Ordering::Acquire) {
                    return;
                }
                let store = coordinator.storage.session_store();
                let _ = store
                    .expire_background_subagent_retention(
                        LOCAL_SESSION_NAMESPACE,
                        chrono::Utc::now(),
                        BACKGROUND_RETENTION_CLEANUP_LIMIT,
                    )
                    .await;
                if store
                    .reconcile_background_subagents(LOCAL_SESSION_NAMESPACE, chrono::Utc::now())
                    .await
                    .is_err()
                {
                    continue;
                }
                let Ok(records) = store
                    .list_pending_background_subagents(
                        LOCAL_SESSION_NAMESPACE,
                        None,
                        BACKGROUND_RECORD_SCAN_LIMIT,
                    )
                    .await
                else {
                    continue;
                };
                for record in records {
                    coordinator.spawn_background_completion_task(record.attempt_id);
                }
            }
        }));
        drop(slot);
        Ok(())
    }

    fn spawn_background_completion_task(&self, attempt_id: SubagentAttemptId) {
        if !self.accepting.load(Ordering::Acquire) {
            return;
        }
        let Ok(mut tasks) = self.background_tasks.lock() else {
            return;
        };
        tasks.retain(|_, task| !task.is_finished());
        if tasks.contains_key(&attempt_id) || tasks.len() >= BACKGROUND_COMPLETION_TASK_LIMIT {
            return;
        }
        let coordinator = self.clone();
        let task_attempt_id = attempt_id.clone();
        let task = tokio::spawn(async move {
            let mut delay = Duration::from_millis(25);
            for attempt in 0..3 {
                match Box::pin(coordinator.handle_background_completion(&task_attempt_id)).await {
                    Err(_) if attempt < 2 => {
                        tokio::time::sleep(delay).await;
                        delay = delay.saturating_mul(4);
                    }
                    Ok(_) | Err(_) => return,
                }
            }
        });
        tasks.insert(attempt_id, task);
    }

    /// Start one live run directly through `AgentRuntime`.
    ///
    /// # Errors
    ///
    /// Returns storage or runtime construction failures.
    #[must_use]
    pub fn start(&self, request: RpcRunRequest) -> RpcBoxFuture<'_, RpcStartedRun> {
        Box::pin(self.start_inner(request, None))
    }

    #[allow(clippy::too_many_lines)]
    async fn start_inner(
        &self,
        request: RpcRunRequest,
        preadmitted: Option<RunAdmissionReceipt>,
    ) -> RpcHostResult<RpcStartedRun> {
        if !self.accepting.load(Ordering::Acquire) {
            return Err(RpcHostError::Runtime(
                "RPC coordinator is shutting down and no longer accepts runs".to_string(),
            ));
        }
        self.reap_finished_tasks().await?;
        self.reap_finished_background_tasks().await?;
        let session = if let Some(session_id) = request.session_id.as_ref() {
            self.storage
                .session_store()
                .load_session(session_id)
                .await?
        } else {
            let mut session = starweaver_session::SessionRecord::new(SessionId::new());
            session.profile = Some(request.profile.clone());
            self.storage
                .session_store()
                .create_session_idempotent(
                    session,
                    &format!("run-session:{}", request.idempotency_key),
                    &format!("run-session:{}", request.command_fingerprint),
                )
                .await?
        };
        let session_id = session.session_id.clone();
        let launch_preadmitted = preadmitted.is_some();
        let admission = if let Some(admission) = preadmitted {
            if admission.run.session_id != session_id
                || admission.run.input != request.durable_input
                || admission.run.profile.as_deref() != Some(request.profile.as_str())
                || admission.run.restore_from_run_id != request.restore_from_run_id
                || admission.lease.host_instance_id != *self.host_instance_id
            {
                return Err(RpcHostError::Invalid(
                    "pre-admitted continuation does not match its runtime request".to_string(),
                ));
            }
            admission
        } else {
            let mut run = RunRecord::new(
                session_id.clone(),
                RunId::new(),
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
            run.status = RunStatus::Queued;
            run.metadata
                .insert(RPC_PROFILE_METADATA_KEY.to_string(), json!(request.profile));
            self.storage
                .session_store()
                .acquire_run_admission(AcquireRunAdmission {
                    run,
                    namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
                    host_instance_id: (*self.host_instance_id).clone(),
                    admission_id: format!("admission_{}", uuid::Uuid::new_v4()),
                    lease_expires_at: chrono::Utc::now()
                        + chrono::Duration::from_std(ACTIVE_LEASE_TTL).unwrap_or_default(),
                    idempotency_key: request.idempotency_key.clone(),
                    command_fingerprint: request.command_fingerprint.clone(),
                })
                .await?
        };
        let run_id = admission.run.run_id.clone();
        let target = admission.lease.target.clone();
        if admission.idempotent_replay && !launch_preadmitted {
            let status = self
                .storage
                .session_store()
                .load_run(&session_id, &run_id)
                .await
                .map_or(admission.run.status, |run| run.status);
            return Ok(RpcStartedRun {
                session_id,
                run_id,
                environment_attachments: request.environment_attachments,
                admission_id: admission.lease.admission_id,
                fencing_generation: admission.lease.fencing_generation,
                status,
                idempotent_replay: true,
            });
        }
        let supervisor = self.supervisor_for_session(&session_id)?;
        if admission.run.trigger_type.as_deref() != Some("async_subagent_result") {
            let pending = self
                .storage
                .session_store()
                .list_pending_background_subagents(
                    LOCAL_SESSION_NAMESPACE,
                    Some(&session_id),
                    BACKGROUND_RECORD_SCAN_LIMIT,
                )
                .await?;
            for mut record in pending {
                if record.delivery_status == DurableBackgroundSubagentDeliveryStatus::Undelivered {
                    let resolved_content = resolve_background_result_content(
                        &self.storage.session_store(),
                        &mut record,
                    )
                    .await?;
                    supervisor
                        .hydrate_durable_result(&record, resolved_content)
                        .map_err(|error| RpcHostError::Runtime(error.to_string()))?;
                }
            }
        }

        let prepared: RpcHostResult<_> = async {
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
                    .map_err(|error| {
                        RpcHostError::Runtime(format!("storage task failed: {error}"))
                    })??
                    .ok_or_else(|| {
                        RpcHostError::NotFound(format!(
                            "run context {}:{}",
                            session_id.as_str(),
                            restore_run_id_text
                        ))
                    })?
                }
                None => session.state.clone(),
            };
            state.session_id = Some(session_id.clone());
            state.run_id = Some(run_id.clone());
            state.parent_run_id.clone_from(&admission.run.parent_run_id);
            state
                .parent_task_id
                .clone_from(&admission.run.parent_task_id);
            if !admission.run.trace_context.is_empty() {
                state
                    .trace_snapshot
                    .clone_from(&admission.run.trace_context);
            }
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
            let mut context = AgentContext::from_state(state);
            if request.install_session_management {
                let query_granted = self
                    .catalog
                    .grants_toolset(&request.profile, "agent_session_query");
                let control_granted = self
                    .catalog
                    .grants_toolset(&request.profile, "agent_session_control");
                let mut operations = BTreeSet::new();
                if query_granted {
                    operations.insert(AgentSessionOperation::Read);
                }
                if control_granted {
                    operations.extend([
                        AgentSessionOperation::Create,
                        AgentSessionOperation::Update,
                        AgentSessionOperation::Control,
                        AgentSessionOperation::Delete,
                    ]);
                }
                let scope = AgentSessionScope {
                    namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
                    owner_id: None,
                    source_product: "rpc".to_string(),
                    source_session_id: Some(session_id.clone()),
                    source_run_id: Some(run_id.clone()),
                    operations,
                    allowed_session_ids: BTreeSet::new(),
                    allow_self_query: true,
                    allow_self_control: false,
                    policy_fingerprint: format!("rpc-profile:{}:v1", request.profile),
                    deadline: None,
                    max_page_size: 50,
                };
                let adapter = Arc::new(RpcAgentSessionAdapter::new(
                    self.storage.clone(),
                    self.clone(),
                    self.catalog.clone(),
                    self.config.workspace_root.clone(),
                ));
                if query_granted {
                    attach_agent_session_query(
                        &mut context,
                        AgentSessionQueryHandle::new(adapter.clone(), scope.clone()),
                    );
                }
                if control_granted {
                    attach_agent_session_control(
                        &mut context,
                        AgentSessionControlHandle::new(adapter, scope),
                    );
                }
            }
            let mut runtime = self
                .catalog
                .runtime_builder(&request.profile)?
                .subagent_delegation_mode(SubagentDelegationMode::Async)
                .background_subagent_supervisor(supervisor.clone())
                .context(context)
                .environment(resolved_environment.provider.clone())
                .durable_session_id(session_id.clone())
                .session_store(session_store)
                .stream_archive(stream_archive)
                .build();
            let input = request.input.clone();
            let handle = runtime
                .try_stream_with_stream_options(
                    input.clone(),
                    AgentStreamOptions::new().drop_policy(AgentStreamDropPolicy::Backpressure),
                )
                .map_err(|error| RpcHostError::Runtime(error.to_string()))?;
            Ok((runtime, input, handle, resolved_environment))
        }
        .await;
        let (mut runtime, input, mut handle, resolved_environment) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = self
                    .storage
                    .session_store()
                    .update_run_status(
                        &session_id,
                        &run_id,
                        RunStatus::Failed,
                        Some("runtime preparation failed".to_string()),
                    )
                    .await;
                let _ = self
                    .storage
                    .session_store()
                    .release_run_admission(&admission.lease)
                    .await;
                return Err(error);
            }
        };
        let control = handle.control_handle();
        supervisor.begin_parent_run(run_id.clone());
        if let Err(error) = self
            .environment_manager
            .mark_run_started(run_id.as_str(), &resolved_environment.attachments)
        {
            let _ = control.interrupt(Some("environment lease registration failed".to_string()));
            let _ = runtime.finish_stream(input, handle).await;
            let _ = self
                .storage
                .session_store()
                .update_run_status(
                    &session_id,
                    &run_id,
                    RunStatus::Failed,
                    Some("environment lease registration failed".to_string()),
                )
                .await;
            let _ = self
                .storage
                .session_store()
                .release_run_admission(&admission.lease)
                .await;
            supervisor.end_parent_run(&run_id);
            return Err(RpcHostError::Invalid(error.message));
        }
        let initial_status = RpcRunStatus {
            session_id: session_id.as_str().to_string(),
            run_id: run_id.as_str().to_string(),
            status: "running".to_string(),
            output_preview: None,
            error: None,
        };
        let (status_tx, _status_rx) = watch::channel(initial_status);
        let active_run = ActiveRun {
            status_tx,
            control,
            lease: admission.lease.clone(),
            events: Vec::new(),
            next_display_sequence: 0,
            next_event_sequence: 0,
            environment: Arc::clone(&resolved_environment.switchable),
            environment_attachments: resolved_environment.attachments.clone(),
            environment_binding_version: 1,
            environment_idempotency: HashMap::new(),
        };
        self.active
            .lock()
            .map_err(active_registry_error)?
            .insert(target.clone(), active_run);
        if let Err(error) = self
            .storage
            .session_store()
            .update_run_status(&session_id, &run_id, RunStatus::Running, None)
            .await
        {
            let removed = self
                .active
                .lock()
                .map_err(active_registry_error)?
                .remove(&target);
            if let Some(removed) = removed {
                let _ = removed
                    .control
                    .interrupt(Some("durable running transition failed".to_string()));
            }
            let _ = runtime.finish_stream(input, handle).await;
            let _ = self.environment_manager.mark_run_finished(run_id.as_str());
            let _ = self
                .storage
                .session_store()
                .release_run_admission(&admission.lease)
                .await;
            supervisor.end_parent_run(&run_id);
            return Err(error.into());
        }

        let active = Arc::clone(&self.active);
        let terminal = Arc::clone(&self.terminal);
        let environment_manager = self.environment_manager.clone();
        let replay_log = self.storage.replay_event_log();
        let store = self.storage.session_store();
        let worker_session_id = session_id.clone();
        let worker_run_id = run_id.clone();
        let worker_target = target.clone();
        let admission_id = admission.lease.admission_id.clone();
        let fencing_generation = admission.lease.fencing_generation;
        let mut worker_lease = admission.lease;
        let worker_supervisor = supervisor.clone();
        let completion_coordinator = self.clone();
        let task = tokio::spawn(async move {
            let projection_context =
                DisplayProjectionContext::new(worker_session_id.clone(), worker_run_id.clone());
            let mut heartbeat = tokio::time::interval(ACTIVE_LEASE_HEARTBEAT);
            heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    record = handle.recv() => {
                        let Some(record) = record else { break; };
                        publish_record(
                            &active,
                            &replay_log,
                            &worker_target,
                            &projection_context,
                            &record,
                        )
                        .await;
                    }
                    _ = heartbeat.tick() => {
                        let expires = chrono::Utc::now()
                            + chrono::Duration::from_std(ACTIVE_LEASE_TTL).unwrap_or_default();
                        match store.heartbeat_run_admission(&worker_lease, expires).await {
                            Ok(renewed) => worker_lease = renewed,
                            Err(_) => {
                                if let Ok(registry) = active.lock()
                                    && let Some(run) = registry.get(&worker_target)
                                {
                                    let _ = run.control.interrupt(Some("admission lease lost".to_string()));
                                }
                            }
                        }
                    }
                }
            }
            let completion = runtime.finish_stream(input, handle).await;
            let (status, durable_status, output_preview, error) = match completion {
                Ok(result) => (
                    run_status_name(result.result.state.status).to_string(),
                    durable_status_from_runtime(result.result.state.status),
                    (!result.result.output.is_empty()).then_some(result.result.output),
                    None,
                ),
                Err(error) => {
                    let cancelled = error.to_string().contains("interrupted");
                    (
                        if cancelled { "cancelled" } else { "failed" }.to_string(),
                        if cancelled {
                            RunStatus::Cancelled
                        } else {
                            RunStatus::Failed
                        },
                        None,
                        Some(error.to_string()),
                    )
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
            let (terminal_event, mut final_status) = if let Ok(mut registry) = active.lock()
                && let Some(active_run) = registry.get_mut(&worker_target)
            {
                let final_status = RpcRunStatus {
                    session_id: worker_session_id.as_str().to_string(),
                    run_id: worker_run_id.as_str().to_string(),
                    status,
                    output_preview: output_preview.clone(),
                    error,
                };
                let event = ReplayEvent::new(
                    ReplayScope::run(worker_run_id.as_str()),
                    active_run.next_event_sequence,
                    ReplayEventKind::Terminal { marker },
                );
                active_run.next_event_sequence = active_run.next_event_sequence.saturating_add(1);
                active_run.events.push(event.clone());
                (Some(event), final_status)
            } else {
                (
                    None,
                    RpcRunStatus {
                        session_id: worker_session_id.as_str().to_string(),
                        run_id: worker_run_id.as_str().to_string(),
                        status,
                        output_preview: output_preview.clone(),
                        error,
                    },
                )
            };
            if let Some(event) = terminal_event {
                let scope = event.scope.clone();
                if let Err(persist_error) = replay_log.append(scope, event).await {
                    final_status.error.get_or_insert_with(|| {
                        format!("failed to persist terminal replay event: {persist_error}")
                    });
                }
            }
            let terminal_durable = store
                .update_run_status(
                    &worker_session_id,
                    &worker_run_id,
                    durable_status,
                    output_preview,
                )
                .await
                .is_ok();
            if terminal_durable {
                let _ = store.release_run_admission(&worker_lease).await;
            } else {
                final_status.status = "failed".to_string();
                final_status.error.get_or_insert_with(|| {
                    "failed to persist terminal durable run status".to_string()
                });
            }
            if let Err(delivery_error) = finalize_parent_deliveries_with_retry(
                &worker_supervisor,
                &worker_run_id,
                terminal_durable && durable_status == RunStatus::Completed,
            )
            .await
            {
                final_status.error.get_or_insert_with(|| {
                    format!("failed to finalize background result delivery: {delivery_error}")
                });
            }
            worker_supervisor.end_parent_run(&worker_run_id);
            if let Ok(registry) = active.lock()
                && let Some(active_run) = registry.get(&worker_target)
            {
                let _ = active_run.status_tx.send(final_status.clone());
            }
            if let Err(cleanup) = environment_manager.mark_run_finished(worker_run_id.as_str()) {
                final_status.error.get_or_insert_with(|| {
                    format!("environment lease cleanup failed: {}", cleanup.message)
                });
            }
            let removed = active
                .lock()
                .ok()
                .and_then(|mut registry| registry.remove(&worker_target));
            if let Some(removed) = removed
                && let Ok(mut cache) = terminal.lock()
            {
                cache.push_back(TerminalRun {
                    target: worker_target.clone(),
                    status: final_status,
                    events: removed.events,
                });
                while cache.len() > TERMINAL_CACHE_LIMIT {
                    cache.pop_front();
                }
            }
            let _ = completion_coordinator
                .schedule_session_background_results(&worker_session_id)
                .await;
        });
        self.tasks
            .lock()
            .map_err(active_registry_error)?
            .insert(target, task);

        Ok(RpcStartedRun {
            session_id,
            run_id,
            environment_attachments: resolved_environment.attachments,
            admission_id,
            fencing_generation,
            status: RunStatus::Running,
            idempotent_replay: false,
        })
    }

    fn take_finished_tasks(&self) -> RpcHostResult<Vec<JoinHandle<()>>> {
        let mut tasks = self.tasks.lock().map_err(active_registry_error)?;
        let all = std::mem::take(&mut *tasks);
        let (finished, running): (HashMap<_, _>, HashMap<_, _>) =
            all.into_iter().partition(|(_, task)| task.is_finished());
        *tasks = running;
        drop(tasks);
        Ok(finished.into_values().collect())
    }

    async fn reap_finished_tasks(&self) -> RpcHostResult<()> {
        for task in self.take_finished_tasks()? {
            task.await
                .map_err(|error| RpcHostError::Runtime(format!("RPC run task failed: {error}")))?;
        }
        Ok(())
    }

    async fn reap_finished_background_tasks(&self) -> RpcHostResult<()> {
        let finished = {
            let mut tasks = self
                .background_tasks
                .lock()
                .map_err(active_registry_error)?;
            let all = std::mem::take(&mut *tasks);
            let (finished, running): (HashMap<_, _>, HashMap<_, _>) =
                all.into_iter().partition(|(_, task)| task.is_finished());
            *tasks = running;
            drop(tasks);
            finished.into_values().collect::<Vec<_>>()
        };
        for task in finished {
            task.await.map_err(|error| {
                RpcHostError::Runtime(format!("RPC background continuation task failed: {error}"))
            })?;
        }
        Ok(())
    }

    /// Return the current process-local, bounded terminal-cache, or durable status.
    ///
    /// # Errors
    ///
    /// Returns an error when the registry is unavailable or durable status cannot be loaded.
    pub async fn status(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> RpcHostResult<RpcRunStatus> {
        let target = Self::target(session_id, run_id);
        let active_status = {
            let registry = self.active.lock().map_err(active_registry_error)?;
            let status = registry
                .get(&target)
                .map(|run| run.status_tx.borrow().clone());
            drop(registry);
            status
        };
        if let Some(status) = active_status {
            return Ok(status);
        }
        let terminal_status = {
            let cache = self.terminal.lock().map_err(active_registry_error)?;
            let status = cache
                .iter()
                .rev()
                .find(|run| run.target == target)
                .map(|run| run.status.clone());
            drop(cache);
            status
        };
        if let Some(status) = terminal_status {
            return Ok(status);
        }
        let run = self
            .storage
            .session_store()
            .load_run(session_id, run_id)
            .await?;
        Ok(status_from_record(&run))
    }

    /// Wait on a state-carrying watch channel, avoiding the check/notification lost-wakeup race.
    ///
    /// # Errors
    ///
    /// Returns an error when status cannot be loaded or the requested timeout elapses.
    pub async fn await_terminal(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        timeout: Option<Duration>,
    ) -> RpcHostResult<RpcRunStatus> {
        let target = Self::target(session_id, run_id);
        let receiver = self
            .active
            .lock()
            .map_err(active_registry_error)?
            .get(&target)
            .map(|run| run.status_tx.subscribe());
        let wait = async {
            let Some(mut receiver) = receiver else {
                return self.status(session_id, run_id).await;
            };
            loop {
                let status = receiver.borrow().clone();
                if status.terminal() {
                    return Ok(status);
                }
                if receiver.changed().await.is_err() {
                    return self.status(session_id, run_id).await;
                }
            }
        };
        match timeout {
            Some(timeout) => tokio::time::timeout(timeout, wait)
                .await
                .map_err(|_| RpcHostError::Runtime("run.await timed out".to_string()))?,
            None => wait.await,
        }
    }

    /// Queue steering through the current composite target and matching fenced durable owner.
    ///
    /// # Errors
    ///
    /// Returns an error when the target is not locally active or the fenced receipt is rejected.
    pub async fn steer(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        steering_id: String,
        text: String,
    ) -> RpcHostResult<Value> {
        self.steer_idempotent(session_id, run_id, steering_id, text, None)
            .await
    }

    /// Queue steering with an explicit idempotency key for an agent-facing control command.
    pub(crate) async fn steer_idempotent(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        steering_id: String,
        text: String,
        idempotency_key: Option<String>,
    ) -> RpcHostResult<Value> {
        let target = Self::target(session_id, run_id);
        let idempotency_key = idempotency_key.unwrap_or_else(|| steering_id.clone());
        let fingerprint = command_fingerprint("steer_session_run", &(&target, &steering_id, &text))
            .map_err(|error| RpcHostError::Runtime(error.to_string()))?;
        let store = self.storage.session_store();
        let existing = store
            .load_control_receipt(&target, &idempotency_key)
            .await?;
        if let Some(existing) = existing.as_ref()
            && existing.command_fingerprint != fingerprint
        {
            return Err(starweaver_session::SessionStoreError::IdempotencyConflict(
                idempotency_key,
            )
            .into());
        }
        if let Some(existing) = existing.as_ref()
            && existing.state == "accepted"
        {
            return Ok(json!({
                "sessionId": session_id.as_str(),
                "runId": run_id.as_str(),
                "steeringId": existing.operation_id,
                "queued": true,
                "receiptId": existing.receipt_id,
                "fencingGeneration": existing.fencing_generation,
                "idempotent": true,
            }));
        }
        let (control, lease) = self.control(&target)?;
        let reserved = if let Some(existing) = existing {
            if existing.fencing_generation != lease.fencing_generation {
                return Err(RpcHostError::NotFound(
                    "control receipt belongs to a stale fencing generation".to_string(),
                ));
            }
            existing
        } else {
            store
                .reserve_control_receipt(DurableControlReceipt {
                    receipt_id: format!("control_{}", uuid::Uuid::new_v4()),
                    target: target.clone(),
                    operation_id: steering_id.clone(),
                    operation: "steer".to_string(),
                    idempotency_key,
                    command_fingerprint: fingerprint,
                    fencing_generation: lease.fencing_generation,
                    state: "reserved".to_string(),
                    created_at: chrono::Utc::now(),
                })
                .await?
        };
        if reserved.state == "accepted" {
            return Ok(json!({
                "sessionId": session_id.as_str(),
                "runId": run_id.as_str(),
                "steeringId": reserved.operation_id,
                "queued": true,
                "receiptId": reserved.receipt_id,
                "fencingGeneration": reserved.fencing_generation,
                "idempotent": true,
            }));
        }
        let receipt = control
            .steer(steering_id, text)
            .await
            .map_err(|error| RpcHostError::Runtime(error.to_string()))?;
        let accepted = self
            .storage
            .session_store()
            .update_control_receipt_state(&reserved.receipt_id, "accepted")
            .await?;
        Ok(json!({
            "sessionId": session_id.as_str(),
            "runId": run_id.as_str(),
            "steeringId": receipt.id,
            "queued": receipt.pending_delivery,
            "receiptId": accepted.receipt_id,
            "fencingGeneration": accepted.fencing_generation,
            "idempotent": false,
        }))
    }

    /// Cooperatively interrupt the current composite target and fenced durable owner.
    ///
    /// # Errors
    ///
    /// Returns an error when the target is not locally active or the fenced receipt is rejected.
    pub async fn cancel(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        reason: Option<String>,
    ) -> RpcHostResult<Value> {
        self.cancel_idempotent(
            session_id,
            run_id,
            format!("interrupt_{}", uuid::Uuid::new_v4()),
            reason,
            None,
        )
        .await
    }

    /// Cooperatively interrupt with stable operation and idempotency identities.
    pub(crate) async fn cancel_idempotent(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        operation_id: String,
        reason: Option<String>,
        idempotency_key: Option<String>,
    ) -> RpcHostResult<Value> {
        let target = Self::target(session_id, run_id);
        let idempotency_key = idempotency_key.unwrap_or_else(|| operation_id.clone());
        let fingerprint =
            command_fingerprint("interrupt_session_run", &(&target, &operation_id, &reason))
                .map_err(|error| RpcHostError::Runtime(error.to_string()))?;
        let store = self.storage.session_store();
        let existing = store
            .load_control_receipt(&target, &idempotency_key)
            .await?;
        if let Some(existing) = existing.as_ref()
            && existing.command_fingerprint != fingerprint
        {
            return Err(starweaver_session::SessionStoreError::IdempotencyConflict(
                idempotency_key,
            )
            .into());
        }
        if let Some(existing) = existing.as_ref()
            && existing.state == "accepted"
        {
            return Ok(json!({
                "sessionId": session_id.as_str(),
                "runId": run_id.as_str(),
                "cancelled": true,
                "controlId": existing.operation_id,
                "receiptId": existing.receipt_id,
                "fencingGeneration": existing.fencing_generation,
                "idempotent": true,
            }));
        }
        let (control, lease) = self.control(&target)?;
        let reserved = if let Some(existing) = existing {
            if existing.fencing_generation != lease.fencing_generation {
                return Err(RpcHostError::NotFound(
                    "control receipt belongs to a stale fencing generation".to_string(),
                ));
            }
            existing
        } else {
            store
                .reserve_control_receipt(DurableControlReceipt {
                    receipt_id: format!("control_{}", uuid::Uuid::new_v4()),
                    target,
                    operation_id: operation_id.clone(),
                    operation: "interrupt".to_string(),
                    idempotency_key,
                    command_fingerprint: fingerprint,
                    fencing_generation: lease.fencing_generation,
                    state: "reserved".to_string(),
                    created_at: chrono::Utc::now(),
                })
                .await?
        };
        if reserved.state == "accepted" {
            return Ok(json!({
                "sessionId": session_id.as_str(),
                "runId": run_id.as_str(),
                "cancelled": true,
                "controlId": reserved.operation_id,
                "receiptId": reserved.receipt_id,
                "fencingGeneration": reserved.fencing_generation,
                "idempotent": true,
            }));
        }
        let receipt = control.interrupt(reason);
        let accepted = self
            .storage
            .session_store()
            .update_control_receipt_state(&reserved.receipt_id, "accepted")
            .await?;
        Ok(json!({
            "sessionId": session_id.as_str(),
            "runId": run_id.as_str(),
            "cancelled": true,
            "controlId": receipt.id,
            "receiptId": accepted.receipt_id,
            "fencingGeneration": accepted.fencing_generation,
            "idempotent": false,
        }))
    }

    /// Replay persisted events plus the process-local live or bounded terminal tail.
    ///
    /// # Errors
    ///
    /// Returns an error when the composite run or replay storage cannot be loaded.
    pub async fn replay(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        cursor: Option<ReplayCursor>,
        limit: Option<usize>,
    ) -> RpcHostResult<Vec<ReplayEvent>> {
        self.storage
            .session_store()
            .load_run(session_id, run_id)
            .await?;
        let target = Self::target(session_id, run_id);
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
            .get(&target)
            .map(|run| run.events.clone())
            .or_else(|| {
                self.terminal.lock().ok().and_then(|cache| {
                    cache
                        .iter()
                        .rev()
                        .find(|run| run.target == target)
                        .map(|run| run.events.clone())
                })
            })
            .unwrap_or_default();
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

    #[allow(clippy::too_many_lines)]
    async fn handle_background_completion(
        &self,
        attempt_id: &SubagentAttemptId,
    ) -> RpcHostResult<Option<RpcStartedRun>> {
        if !self.accepting.load(Ordering::Acquire) {
            return Ok(None);
        }
        let store = self.storage.session_store();
        let mut background = store.load_background_subagent(attempt_id).await?;
        if !background.execution_status.is_terminal()
            || background.delivery_status == DurableBackgroundSubagentDeliveryStatus::Delivered
            || background
                .automatic_continuation_suppressed_by_run_id
                .is_some()
        {
            return Ok(None);
        }
        if background.delivery_status == DurableBackgroundSubagentDeliveryStatus::Claimed {
            store
                .reconcile_background_subagents(&background.namespace_id, chrono::Utc::now())
                .await?;
            background = store.load_background_subagent(attempt_id).await?;
            if background.delivery_status != DurableBackgroundSubagentDeliveryStatus::Undelivered
                || background
                    .automatic_continuation_suppressed_by_run_id
                    .is_some()
            {
                return Ok(None);
            }
        }
        let session = store.load_session(&background.parent_session_id).await?;
        let fence = store
            .session_continuation_fence(&background.namespace_id, &background.parent_session_id)
            .await?;
        if !fence.continuation_allowed || session.status != SessionStatus::Active {
            return Ok(None);
        }
        let parent = store
            .load_run(&background.parent_session_id, &background.parent_run_id)
            .await?;
        if parent.status == RunStatus::Cancelled {
            return Ok(None);
        }
        self.catalog.profile(&background.profile)?;
        let artifact_content = resolve_background_artifact_content(&store, &mut background).await?;
        let continuation_text = background.continuation_text(artifact_content.as_deref());
        let durable_input = background.continuation_input(artifact_content.as_deref());
        let continuation_run_id =
            RunId::from_string(format!("run_async_subagent_{}", attempt_id.as_str()));
        let mut run = RunRecord::new(
            background.parent_session_id.clone(),
            continuation_run_id.clone(),
            session
                .state
                .conversation_id
                .clone()
                .unwrap_or_else(ConversationId::new),
        );
        run.input.clone_from(&durable_input);
        run.profile = Some(background.profile.clone());
        run.restore_from_run_id = Some(background.parent_run_id.clone());
        run.parent_run_id = Some(background.parent_run_id.clone());
        run.trace_context = background.trace_context.clone().unwrap_or_default();
        run.trigger_type = Some("async_subagent_result".to_string());
        run.status = RunStatus::Queued;
        run.metadata.insert(
            RPC_PROFILE_METADATA_KEY.to_string(),
            json!(background.profile),
        );
        run.metadata.insert(
            "starweaver.async_subagent.attempt_id".to_string(),
            json!(background.attempt_id.as_str()),
        );
        run.metadata.insert(
            "starweaver.async_subagent.agent_id".to_string(),
            json!(background.agent_id),
        );
        run.metadata.insert(
            "starweaver.async_subagent.parent_run_id".to_string(),
            json!(background.parent_run_id.as_str()),
        );
        if let Some(child_run_id) = background.child_run_id.as_ref() {
            run.metadata.insert(
                "starweaver.async_subagent.child_run_id".to_string(),
                json!(child_run_id.as_str()),
            );
        }
        let idempotency_key = format!("async-subagent:{}", attempt_id.as_str());
        let command_fingerprint = command_fingerprint(
            "async_subagent_result",
            &(
                background.parent_session_id.as_str(),
                background.parent_run_id.as_str(),
                background.attempt_id.as_str(),
                background.agent_id.as_str(),
                background.child_run_id.as_ref().map(RunId::as_str),
                continuation_text.as_str(),
                background.profile.as_str(),
            ),
        )
        .map_err(|error| RpcHostError::Runtime(error.to_string()))?;
        let claim_id = format!("rpc-continuation:{}", attempt_id.as_str());
        let cause = BackgroundSubagentContinuationCause::new(&background, &durable_input)
            .map_err(|error| RpcHostError::Runtime(error.to_string()))?;
        let receipt = store
            .acquire_background_subagent_continuation(AcquireBackgroundSubagentContinuation {
                attempt_id: attempt_id.clone(),
                claim_id: claim_id.clone(),
                claim_deadline: chrono::Utc::now()
                    + chrono::Duration::from_std(BACKGROUND_CONTINUATION_LEASE_TTL)
                        .unwrap_or_default(),
                cause: cause.clone(),
                admission: AcquireRunAdmission {
                    run,
                    namespace_id: background.namespace_id.clone(),
                    host_instance_id: (*self.host_instance_id).clone(),
                    admission_id: format!("admission_{}", uuid::Uuid::new_v4()),
                    lease_expires_at: chrono::Utc::now()
                        + chrono::Duration::from_std(ACTIVE_LEASE_TTL).unwrap_or_default(),
                    idempotency_key: idempotency_key.clone(),
                    command_fingerprint: command_fingerprint.clone(),
                },
            })
            .await?;
        if receipt.cause != cause {
            return Err(RpcHostError::Runtime(
                "continuation admission receipt did not attest the submitted cause".to_string(),
            ));
        }
        let admitted_continuation_run_id = receipt.admission.run.run_id.clone();
        let restore_from_run_id = receipt.admission.run.restore_from_run_id.clone();
        let started = Box::pin(self.start_inner(
            RpcRunRequest {
                durable_input,
                input: AgentInput::text(continuation_text),
                session_id: Some(background.parent_session_id),
                restore_from_run_id,
                profile: background.profile,
                environment_attachments: Vec::new(),
                idempotency_key,
                command_fingerprint,
                install_session_management: true,
            },
            Some(receipt.admission),
        ))
        .await?;
        store
            .acknowledge_background_subagent_delivery(attempt_id, &claim_id)
            .await?;
        if let Ok(supervisors) = self.supervisors.lock()
            && let Some(supervisor) = supervisors.get(&started.session_id)
        {
            let _ = supervisor.mark_delivery_from_host(
                attempt_id,
                &claim_id,
                &admitted_continuation_run_id,
            );
        }
        Ok(Some(started))
    }

    async fn schedule_session_background_results(
        &self,
        session_id: &SessionId,
    ) -> RpcHostResult<()> {
        if !self.accepting.load(Ordering::Acquire) {
            return Ok(());
        }
        let records = self
            .storage
            .session_store()
            .list_pending_background_subagents(
                LOCAL_SESSION_NAMESPACE,
                Some(session_id),
                BACKGROUND_RECORD_SCAN_LIMIT,
            )
            .await?;
        for record in records {
            if record.execution_status.is_terminal()
                && record.delivery_status != DurableBackgroundSubagentDeliveryStatus::Delivered
                && record.automatic_continuation_suppressed_by_run_id.is_none()
            {
                self.spawn_background_completion_task(record.attempt_id);
            }
        }
        Ok(())
    }

    /// Reconcile expired owners during host startup. Durable running alone is not controllable.
    ///
    /// # Errors
    ///
    /// Returns an error when durable admission reconciliation fails.
    pub async fn reconcile_startup(&self) -> RpcHostResult<Vec<ManagedRunTarget>> {
        let store = self.storage.session_store();
        let reconciled_runs = store
            .reconcile_expired_run_admissions(LOCAL_SESSION_NAMESPACE, chrono::Utc::now())
            .await?;
        store
            .expire_background_subagent_retention(
                LOCAL_SESSION_NAMESPACE,
                chrono::Utc::now(),
                BACKGROUND_RETENTION_CLEANUP_LIMIT,
            )
            .await?;
        store
            .reconcile_background_subagents(LOCAL_SESSION_NAMESPACE, chrono::Utc::now())
            .await?;
        let backgrounds = store
            .list_pending_background_subagents(
                LOCAL_SESSION_NAMESPACE,
                None,
                BACKGROUND_RECORD_SCAN_LIMIT,
            )
            .await?;
        for background in backgrounds {
            if background.execution_status.is_terminal()
                && background.delivery_status != DurableBackgroundSubagentDeliveryStatus::Delivered
            {
                self.spawn_background_completion_task(background.attempt_id);
            }
        }
        self.ensure_background_reconciler()?;
        Ok(reconciled_runs)
    }

    /// Stop admission, cooperatively interrupt live runs, and join owned finalizers.
    ///
    /// # Errors
    ///
    /// Returns an error when the registry is unavailable or finalizers exceed the timeout.
    pub async fn shutdown(&self, timeout: Duration) -> RpcHostResult<()> {
        self.accepting.store(false, Ordering::Release);
        let deadline = tokio::time::Instant::now() + timeout;
        let supervisors = self
            .supervisors
            .lock()
            .map_err(active_registry_error)?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut supervisor_error = None;
        for supervisor in supervisors {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if let Err(error) = supervisor.shutdown_checked(Some(remaining)).await {
                supervisor_error.get_or_insert_with(|| error.to_string());
            }
        }
        let reconciler = self
            .background_reconciler
            .lock()
            .map_err(active_registry_error)?
            .take();
        if let Some(reconciler) = reconciler {
            reconciler.abort();
            let _ = reconciler.await;
        }
        let controls = self
            .active
            .lock()
            .map_err(active_registry_error)?
            .values()
            .map(|run| run.control.clone())
            .collect::<Vec<_>>();
        for control in controls {
            let _ = control.interrupt(Some("RPC host shutdown".to_string()));
        }
        let mut tasks = self
            .tasks
            .lock()
            .map_err(active_registry_error)?
            .drain()
            .map(|(_, task)| task)
            .chain(
                self.background_tasks
                    .lock()
                    .map_err(active_registry_error)?
                    .drain()
                    .map(|(_, task)| task),
            )
            .collect::<Vec<_>>();
        let mut timed_out = false;
        for task in &mut tasks {
            if tokio::time::timeout_at(deadline, &mut *task).await.is_err() {
                timed_out = true;
                task.abort();
            }
        }
        for task in tasks {
            if !task.is_finished() {
                task.abort();
            }
            let _ = task.await;
        }
        self.storage
            .session_store()
            .reconcile_background_subagents(LOCAL_SESSION_NAMESPACE, chrono::Utc::now())
            .await?;
        if timed_out {
            return Err(RpcHostError::Runtime(
                "RPC shutdown exceeded its drain deadline after aborting owned tasks".to_string(),
            ));
        }
        if let Some(error) = supervisor_error {
            return Err(RpcHostError::Runtime(format!(
                "RPC shutdown could not durably terminalize every background subagent: {error}"
            )));
        }
        Ok(())
    }

    /// Inspect one durable background attempt through the trusted RPC host application boundary.
    ///
    /// # Errors
    ///
    /// Returns an error when the durable attempt does not exist or storage fails.
    pub async fn background_attempt(
        &self,
        attempt_id: &SubagentAttemptId,
    ) -> RpcHostResult<BackgroundSubagentRecord> {
        self.storage
            .session_store()
            .load_background_subagent(attempt_id)
            .await
            .map_err(Into::into)
    }

    /// Request cancellation by durable attempt identity without requiring a parent model turn.
    ///
    /// # Errors
    ///
    /// Returns an error when the attempt is unknown, has no live owner, or cancellation fails.
    pub async fn cancel_background_attempt(
        &self,
        attempt_id: &SubagentAttemptId,
        reason: Option<String>,
    ) -> RpcHostResult<starweaver_agent::BackgroundSubagentCancellationReceipt> {
        let supervisors = self
            .supervisors
            .lock()
            .map_err(active_registry_error)?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for supervisor in supervisors {
            if supervisor
                .active_tasks()
                .iter()
                .any(|info| &info.attempt_id == attempt_id)
            {
                return supervisor
                    .request_cancellation_with_reason(
                        attempt_id,
                        format!("rpc-admin-cancel:{}", uuid::Uuid::new_v4()),
                        reason,
                    )
                    .map_err(|error| RpcHostError::Runtime(error.to_string()));
            }
        }
        let record = self.background_attempt(attempt_id).await?;
        if record.execution_status.is_terminal() {
            return Ok(starweaver_agent::BackgroundSubagentCancellationReceipt {
                attempt_id: record.attempt_id,
                agent_id: record.agent_id,
                cancellation_id: format!("rpc-admin-terminal:{}", uuid::Uuid::new_v4()),
                status: record.execution_status.as_str().to_string(),
            });
        }
        Err(RpcHostError::NotFound(
            "durable background attempt has no live owner in this host process".to_string(),
        ))
    }

    pub(crate) async fn cancel_session_subagents(
        &self,
        session_id: &SessionId,
        timeout: Duration,
    ) -> RpcHostResult<()> {
        let supervisor = self
            .supervisors
            .lock()
            .map_err(active_registry_error)?
            .get(session_id)
            .cloned();
        if let Some(supervisor) = supervisor {
            let attempts = supervisor
                .active_tasks()
                .into_iter()
                .map(|info| info.attempt_id)
                .collect::<Vec<_>>();
            for attempt_id in &attempts {
                let _ = supervisor.request_cancellation_with_reason(
                    attempt_id,
                    format!("session-delete:{}", uuid::Uuid::new_v4()),
                    Some("owning session deletion".to_string()),
                );
            }
            supervisor.wait_for_attempts(&attempts, timeout).await;
        }
        let remaining = self
            .storage
            .session_store()
            .list_background_subagents(
                LOCAL_SESSION_NAMESPACE,
                Some(session_id),
                BACKGROUND_RECORD_SCAN_LIMIT,
            )
            .await?
            .into_iter()
            .any(|record| !record.execution_status.is_terminal());
        if remaining {
            return Err(RpcHostError::Runtime(
                "session still owns active background subagents".to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) async fn tombstone_session_fenced(
        &self,
        session_id: &SessionId,
        timeout: Duration,
    ) -> RpcHostResult<starweaver_session::SessionRecord> {
        let store = self.storage.session_store();
        let session = store.load_session(session_id).await?;
        if session.status == SessionStatus::Deleted {
            return Ok(session);
        }
        let fence_id = match &session.deletion_fence {
            SessionDeletionFence::Stable => {
                let fence_id = format!("rpc-admin-delete:{}", uuid::Uuid::new_v4());
                let idempotency_key = fence_id.clone();
                let fingerprint = command_fingerprint(
                    "rpc_admin_delete_session",
                    &(session_id.as_str(), session.revision, fence_id.as_str()),
                )
                .map_err(|error| RpcHostError::Runtime(error.to_string()))?;
                store
                    .acquire_session_deletion_fence(
                        session_id,
                        session.revision,
                        &fence_id,
                        "rpc-admin",
                        &idempotency_key,
                        &fingerprint,
                    )
                    .await?;
                fence_id
            }
            SessionDeletionFence::Deleting { fence_id, .. }
            | SessionDeletionFence::Deleted { fence_id, .. } => fence_id.clone(),
        };
        self.cancel_session_subagents(session_id, timeout).await?;
        for run in store.list_runs(session_id).await? {
            if run.status.is_active() {
                let target = Self::target(session_id, &run.run_id);
                if self.is_controllable(&target) {
                    self.cancel(
                        session_id,
                        &run.run_id,
                        Some("session deletion fence".to_string()),
                    )
                    .await?;
                    self.await_terminal(session_id, &run.run_id, Some(timeout))
                        .await?;
                }
            }
        }
        let supervisor = self
            .supervisors
            .lock()
            .map_err(active_registry_error)?
            .get(session_id)
            .cloned();
        if let Some(supervisor) = supervisor.as_ref() {
            supervisor
                .shutdown_checked(Some(timeout))
                .await
                .map_err(|error| RpcHostError::Runtime(error.to_string()))?;
        }
        let tombstoned = store.tombstone_session(session_id, &fence_id).await?;
        if supervisor.is_some() {
            self.supervisors
                .lock()
                .map_err(active_registry_error)?
                .remove(session_id);
        }
        Ok(tombstoned)
    }

    pub(crate) fn is_controllable(&self, target: &ManagedRunTarget) -> bool {
        self.active
            .lock()
            .is_ok_and(|registry| registry.contains_key(target))
    }

    fn target(session_id: &SessionId, run_id: &RunId) -> ManagedRunTarget {
        ManagedRunTarget::new(LOCAL_SESSION_NAMESPACE, session_id.clone(), run_id.clone())
    }

    #[allow(clippy::significant_drop_tightening)]
    pub(crate) fn active_run_session_id(&self, run_id: &str) -> Result<String, RpcError> {
        let registry = self
            .active
            .lock()
            .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
        let run = active_mutable_run(&registry, run_id)?;
        Ok(run.lease.target.session_id.as_str().to_string())
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
                run.lease.target.session_id.as_str(),
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
                run.lease.target.session_id.as_str().to_string(),
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

    fn control(
        &self,
        target: &ManagedRunTarget,
    ) -> RpcHostResult<(AgentControlHandle, RunAdmissionLease)> {
        self.active
            .lock()
            .map_err(active_registry_error)?
            .get(target)
            .map(|run| (run.control.clone(), run.lease.clone()))
            .ok_or_else(|| {
                RpcHostError::NotFound(format!(
                    "active run {}:{}",
                    target.session_id.as_str(),
                    target.run_id.as_str()
                ))
            })
    }
}

fn active_mutable_run<'a>(
    registry: &'a HashMap<ManagedRunTarget, ActiveRun>,
    run_id: &str,
) -> Result<&'a ActiveRun, RpcError> {
    let mut matches = registry
        .iter()
        .filter(|(target, _)| target.run_id.as_str() == run_id)
        .map(|(_, run)| run);
    let run = matches
        .next()
        .ok_or_else(|| RpcError::new(RUN_CONFLICT, format!("active run not found: {run_id}")))?;
    if matches.next().is_some() {
        return Err(RpcError::new(
            RUN_CONFLICT,
            format!("ambiguous run id requires composite session identity: {run_id}"),
        ));
    }
    Ok(run)
}

fn active_mutable_run_mut<'a>(
    registry: &'a mut HashMap<ManagedRunTarget, ActiveRun>,
    run_id: &str,
) -> Result<&'a mut ActiveRun, RpcError> {
    let keys = registry
        .keys()
        .filter(|target| target.run_id.as_str() == run_id)
        .cloned()
        .collect::<Vec<_>>();
    if keys.len() > 1 {
        return Err(RpcError::new(
            RUN_CONFLICT,
            format!("ambiguous run id requires composite session identity: {run_id}"),
        ));
    }
    let key = keys
        .first()
        .ok_or_else(|| RpcError::new(RUN_CONFLICT, format!("active run not found: {run_id}")))?;
    registry
        .get_mut(key)
        .ok_or_else(|| RpcError::new(RUN_CONFLICT, format!("active run not found: {run_id}")))
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
        session_id: run.lease.target.session_id.as_str().to_string(),
        run_id: run.lease.target.run_id.as_str().to_string(),
        binding_version: run.environment_binding_version,
        environment: environment_summary(
            run.environment_binding_version,
            &run.environment_attachments,
        ),
        operation_id: Some(operation_id.to_string()),
        extra,
    };
    let event = ReplayEvent::new(
        ReplayScope::run(run.lease.target.run_id.as_str()),
        sequence,
        ReplayEventKind::EnvironmentLifecycle(Box::new(lifecycle)),
    );
    run.events.push(event.clone());
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
    active: &Arc<Mutex<HashMap<ManagedRunTarget, ActiveRun>>>,
    replay_log: &starweaver_storage::SqliteReplayEventLog,
    target: &ManagedRunTarget,
    projection_context: &DisplayProjectionContext,
    record: &AgentStreamRecord,
) {
    let messages = DefaultDisplayMessageProjector
        .project(projection_context, record)
        .await;
    if messages.is_empty() {
        return;
    }
    let scope = ReplayScope::run(target.run_id.as_str());
    let events = {
        let Ok(mut registry) = active.lock() else {
            return;
        };
        let Some(active_run) = registry.get_mut(target) else {
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
        events
    };
    for event in events {
        if let Err(error) = replay_log.append(scope.clone(), event).await
            && let Ok(mut registry) = active.lock()
            && let Some(active_run) = registry.get_mut(target)
        {
            let mut status = active_run.status_tx.borrow().clone();
            status
                .error
                .get_or_insert_with(|| format!("failed to persist replay event: {error}"));
            let _ = active_run.status_tx.send(status);
        }
    }
}

async fn finalize_parent_deliveries_with_retry(
    supervisor: &BackgroundSubagentSupervisor,
    run_id: &RunId,
    committed: bool,
) -> Result<(), starweaver_agent::BackgroundSubagentError> {
    let mut delay = Duration::from_millis(10);
    for attempt in 0..3 {
        match supervisor
            .finalize_parent_deliveries(run_id, committed)
            .await
        {
            Ok(_) => return Ok(()),
            Err(error) if attempt == 2 => return Err(error),
            Err(_) => {
                tokio::time::sleep(delay).await;
                delay = delay.saturating_mul(4);
            }
        }
    }
    unreachable!("bounded delivery retry loop always returns")
}

async fn resolve_background_artifact_content<S: SessionStore + ?Sized>(
    store: &S,
    record: &mut BackgroundSubagentRecord,
) -> RpcHostResult<Option<String>> {
    if record.retention_status != DurableBackgroundSubagentRetentionStatus::Artifact {
        return Ok(None);
    }
    let artifact_ref = record
        .result_ref
        .as_ref()
        .and_then(|result| result.artifact_ref.as_deref())
        .ok_or_else(|| {
            RpcHostError::Runtime(
                "artifact-retained background result is missing its reference".to_string(),
            )
        })?
        .to_string();
    match store.load_background_subagent_artifact(&artifact_ref).await {
        Ok(artifact) => Ok(Some(artifact.content)),
        Err(SessionStoreError::NotFound(_)) => {
            store
                .expire_background_subagent_retention(
                    &record.namespace_id,
                    chrono::Utc::now(),
                    BACKGROUND_RETENTION_CLEANUP_LIMIT,
                )
                .await?;
            *record = store.load_background_subagent(&record.attempt_id).await?;
            if record.retention_status == DurableBackgroundSubagentRetentionStatus::Expired {
                Ok(None)
            } else {
                Err(RpcHostError::Runtime(
                    "background result artifact is unavailable before its retention state expired"
                        .to_string(),
                ))
            }
        }
        Err(error) => Err(error.into()),
    }
}

async fn resolve_background_result_content<S: SessionStore + ?Sized>(
    store: &S,
    record: &mut BackgroundSubagentRecord,
) -> RpcHostResult<Option<String>> {
    if let Some(content) = resolve_background_artifact_content(store, record).await? {
        return Ok(Some(content));
    }
    Ok(
        (record.retention_status == DurableBackgroundSubagentRetentionStatus::Expired)
            .then(|| record.continuation_outcome(None)),
    )
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

const fn durable_status_from_runtime(status: starweaver_runtime::RunStatus) -> RunStatus {
    match status {
        starweaver_runtime::RunStatus::Starting => RunStatus::Starting,
        starweaver_runtime::RunStatus::Running => RunStatus::Running,
        starweaver_runtime::RunStatus::Waiting => RunStatus::Waiting,
        starweaver_runtime::RunStatus::Completed => RunStatus::Completed,
        starweaver_runtime::RunStatus::Failed => RunStatus::Failed,
        starweaver_runtime::RunStatus::Cancelled => RunStatus::Cancelled,
    }
}

#[allow(clippy::needless_pass_by_value)]
fn active_registry_error<T>(error: std::sync::PoisonError<T>) -> RpcHostError {
    RpcHostError::Runtime(format!("active run registry poisoned: {error}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use starweaver_session::DurableBackgroundSubagentDeliveryRelease;

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
                idempotency_key: "test-start".to_string(),
                command_fingerprint: "test-start-v1".to_string(),
                install_session_management: false,
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
        assert!(!coordinator.is_controllable(&ManagedRunTarget::new(
            LOCAL_SESSION_NAMESPACE,
            started.session_id,
            started.run_id,
        )));
    }

    #[tokio::test]
    async fn exact_start_retry_replays_receipt_and_conflicting_retry_is_rejected() {
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
        let request = RpcRunRequest {
            durable_input: vec![InputPart::text("hello")],
            input: AgentInput::text("hello"),
            session_id: None,
            restore_from_run_id: None,
            profile: "default".to_string(),
            environment_attachments: Vec::new(),
            idempotency_key: "same-start".to_string(),
            command_fingerprint: "same-fingerprint".to_string(),
            install_session_management: false,
        };
        let first = coordinator.start(request.clone()).await.unwrap();
        coordinator
            .await_terminal(
                &first.session_id,
                &first.run_id,
                Some(Duration::from_secs(5)),
            )
            .await
            .unwrap();
        let replay = coordinator.start(request.clone()).await.unwrap();
        assert_eq!(replay.session_id, first.session_id);
        assert_eq!(replay.run_id, first.run_id);
        assert_eq!(replay.admission_id, first.admission_id);
        assert!(replay.idempotent_replay);
        assert_eq!(replay.status, RunStatus::Completed);

        let conflict = coordinator
            .start(RpcRunRequest {
                command_fingerprint: "different-fingerprint".to_string(),
                ..request
            })
            .await
            .unwrap_err();
        assert!(conflict.to_string().contains("idempotency"));
    }

    #[tokio::test]
    #[allow(clippy::expect_used, clippy::too_many_lines)]
    async fn durable_background_result_starts_exactly_one_causal_continuation() {
        use starweaver_session::{
            BACKGROUND_SUBAGENT_RECORD_VERSION, DurableBackgroundSubagentExecutionStatus,
            DurableBackgroundSubagentResultRef, DurableBackgroundSubagentRetentionStatus,
        };

        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        let storage = SqliteStorage::open(&config.database_path).unwrap();
        let catalog = RpcAgentCatalog::new(config.clone()).unwrap();
        let coordinator = RpcRuntimeCoordinator::new(
            config,
            catalog,
            storage.clone(),
            EnvironmentAttachmentManager::new(),
        );
        let parent = coordinator
            .start(RpcRunRequest {
                durable_input: vec![InputPart::text("parent input")],
                input: AgentInput::text("parent input"),
                session_id: None,
                restore_from_run_id: None,
                profile: "default".to_string(),
                environment_attachments: Vec::new(),
                idempotency_key: "background-parent".to_string(),
                command_fingerprint: "background-parent-v1".to_string(),
                install_session_management: false,
            })
            .await
            .unwrap();
        coordinator
            .await_terminal(
                &parent.session_id,
                &parent.run_id,
                Some(Duration::from_secs(5)),
            )
            .await
            .unwrap();

        let store = storage.session_store();
        let parent_before = store
            .load_run(&parent.session_id, &parent.run_id)
            .await
            .unwrap();
        let intervening = coordinator
            .start(RpcRunRequest {
                durable_input: vec![InputPart::text("intervening input")],
                input: AgentInput::text("intervening input"),
                session_id: Some(parent.session_id.clone()),
                restore_from_run_id: Some(parent.run_id.clone()),
                profile: "default".to_string(),
                environment_attachments: Vec::new(),
                idempotency_key: "background-intervening".to_string(),
                command_fingerprint: "background-intervening-v1".to_string(),
                install_session_management: false,
            })
            .await
            .unwrap();
        coordinator
            .await_terminal(
                &intervening.session_id,
                &intervening.run_id,
                Some(Duration::from_secs(5)),
            )
            .await
            .unwrap();
        let now = chrono::Utc::now();
        let attempt_id = SubagentAttemptId::from_string("rpc-background-attempt");
        let child_run_id = RunId::from_string("rpc-background-child");
        let mut background = BackgroundSubagentRecord {
            schema_version: BACKGROUND_SUBAGENT_RECORD_VERSION,
            attempt_id: attempt_id.clone(),
            agent_id: "rpc-background-agent".to_string(),
            linked_task_id: None,
            subagent_name: "researcher".to_string(),
            namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
            parent_session_id: parent.session_id.clone(),
            parent_run_id: parent.run_id.clone(),
            child_run_id: None,
            continuation_run_id: None,
            profile: "default".to_string(),
            owner_lease: starweaver_session::DurableBackgroundSubagentOwnerLease {
                host_instance_id: (*coordinator.host_instance_id).clone(),
                fencing_generation: 1,
                heartbeat_at: now,
                lease_expires_at: now + chrono::Duration::minutes(1),
            },
            execution_status: DurableBackgroundSubagentExecutionStatus::Accepted,
            result_ref: None,
            failure_category: None,
            cancellation_reason: None,
            delivery_status: DurableBackgroundSubagentDeliveryStatus::Undelivered,
            delivery_claim: None,
            delivered_claim_id: None,
            automatic_continuation_suppressed_by_run_id: None,
            retention_status: DurableBackgroundSubagentRetentionStatus::Inline,
            retention_expires_at: None,
            trace_context: None,
            accepted_at: now,
            updated_at: now,
            terminal_at: None,
        };
        store
            .record_background_subagent_acceptance(background.clone())
            .await
            .unwrap();
        background.execution_status = DurableBackgroundSubagentExecutionStatus::Starting;
        background.updated_at = now + chrono::Duration::milliseconds(1);
        store
            .update_background_subagent_execution(background.clone())
            .await
            .unwrap();
        background.execution_status = DurableBackgroundSubagentExecutionStatus::Running;
        background.child_run_id = Some(child_run_id.clone());
        background.updated_at = now + chrono::Duration::milliseconds(2);
        store
            .update_background_subagent_execution(background.clone())
            .await
            .unwrap();
        let full_artifact_content = "durable child full artifact result".to_string();
        let artifact_ref = format!(
            "starweaver:background-subagent-result:{}",
            attempt_id.as_str()
        );
        let artifact_digest =
            starweaver_session::BackgroundSubagentArtifact::content_digest(&full_artifact_content);
        background.execution_status = DurableBackgroundSubagentExecutionStatus::Completed;
        background.result_ref = Some(DurableBackgroundSubagentResultRef {
            content: Some("durable child preview".to_string()),
            artifact_ref: Some(artifact_ref.clone()),
            digest: Some(artifact_digest.clone()),
            size_bytes: u64::try_from(full_artifact_content.len()).unwrap(),
            ..DurableBackgroundSubagentResultRef::default()
        });
        background.retention_status = DurableBackgroundSubagentRetentionStatus::Artifact;
        background.updated_at = now + chrono::Duration::milliseconds(3);
        background.terminal_at = Some(background.updated_at);
        background.retention_expires_at = Some(background.updated_at + chrono::Duration::hours(1));
        store
            .commit_background_subagent_terminal(
                starweaver_session::BackgroundSubagentTerminalCommit {
                    record: background,
                    artifact: Some(starweaver_session::BackgroundSubagentArtifact {
                        artifact_ref,
                        namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
                        attempt_id: attempt_id.clone(),
                        content: full_artifact_content.clone(),
                        digest: artifact_digest.clone(),
                        size_bytes: u64::try_from(full_artifact_content.len()).unwrap(),
                        created_at: now + chrono::Duration::milliseconds(3),
                        expires_at: now
                            + chrono::Duration::milliseconds(3)
                            + chrono::Duration::hours(1),
                    }),
                    artifact_limits: Some(starweaver_session::BackgroundSubagentArtifactLimits {
                        max_single_bytes: 1_000_000,
                        max_retained_bytes: 10_000_000,
                    }),
                },
            )
            .await
            .unwrap();

        let continuation = Box::pin(coordinator.handle_background_completion(&attempt_id))
            .await
            .unwrap()
            .expect("terminal result should admit a continuation");
        coordinator
            .await_terminal(
                &continuation.session_id,
                &continuation.run_id,
                Some(Duration::from_secs(5)),
            )
            .await
            .unwrap();

        let delivered = store.load_background_subagent(&attempt_id).await.unwrap();
        assert_eq!(
            delivered.delivery_status,
            DurableBackgroundSubagentDeliveryStatus::Delivered
        );
        assert_eq!(
            delivered.continuation_run_id.as_ref(),
            Some(&continuation.run_id)
        );
        let mut expired_projection = delivered.clone();
        expired_projection.retention_status = DurableBackgroundSubagentRetentionStatus::Expired;
        expired_projection.retention_expires_at = None;
        if let Some(result_ref) = expired_projection.result_ref.as_mut() {
            result_ref.content = None;
            result_ref.artifact_ref = None;
        }
        let expired_text = expired_projection.continuation_text(None);
        assert!(expired_text.contains("Retained background result content expired"));
        assert!(expired_text.contains(&artifact_digest));
        let continuation_record = store
            .load_run(&continuation.session_id, &continuation.run_id)
            .await
            .unwrap();
        assert_eq!(
            continuation_record.trigger_type.as_deref(),
            Some("async_subagent_result")
        );
        assert_eq!(
            continuation_record.parent_run_id.as_ref(),
            Some(&parent.run_id)
        );
        assert_eq!(
            continuation_record.restore_from_run_id.as_ref(),
            Some(&intervening.run_id)
        );
        assert_eq!(
            continuation_record
                .metadata
                .get("starweaver.async_subagent.attempt_id"),
            Some(&json!(attempt_id.as_str()))
        );
        assert_eq!(
            continuation_record
                .metadata
                .get("starweaver.async_subagent.child_run_id"),
            Some(&json!(child_run_id.as_str()))
        );
        assert_eq!(continuation_record.input.len(), 1);
        let InputPart::Text { text, .. } = &continuation_record.input[0] else {
            panic!("background continuation input must be text");
        };
        assert!(text.contains(&full_artifact_content), "{text}");
        assert!(!text.contains("durable child preview"), "{text}");

        assert!(
            Box::pin(coordinator.handle_background_completion(&attempt_id))
                .await
                .unwrap()
                .is_none()
        );
        let parent_after = store
            .load_run(&parent.session_id, &parent.run_id)
            .await
            .unwrap();
        assert_eq!(parent_after, parent_before);
        let continuations = store
            .list_runs(&parent.session_id)
            .await
            .unwrap()
            .into_iter()
            .filter(|run| run.trigger_type.as_deref() == Some("async_subagent_result"))
            .collect::<Vec<_>>();
        assert_eq!(continuations.len(), 1);
        assert_eq!(continuations[0].run_id, continuation.run_id);

        let cancelled_parent_id = RunId::from_string("cancelled-background-parent");
        let mut cancelled_parent = RunRecord::new(
            parent.session_id.clone(),
            cancelled_parent_id.clone(),
            ConversationId::new(),
        );
        cancelled_parent.status = RunStatus::Cancelled;
        cancelled_parent.profile = Some("default".to_string());
        cancelled_parent.input = vec![InputPart::text("cancelled parent")];
        store.append_run(cancelled_parent).await.unwrap();
        let pending_attempt = SubagentAttemptId::from_string("pending-after-parent-cancel");
        let pending_now = chrono::Utc::now();
        let mut pending = delivered.clone();
        pending.attempt_id = pending_attempt.clone();
        pending.agent_id = "pending-agent-after-cancel".to_string();
        pending.parent_run_id = parent.run_id.clone();
        pending.child_run_id = None;
        pending.execution_status = DurableBackgroundSubagentExecutionStatus::Accepted;
        pending.result_ref = None;
        pending.delivery_status = DurableBackgroundSubagentDeliveryStatus::Undelivered;
        pending.delivery_claim = None;
        pending.delivered_claim_id = None;
        pending.automatic_continuation_suppressed_by_run_id = None;
        pending.continuation_run_id = None;
        pending.retention_status = DurableBackgroundSubagentRetentionStatus::Inline;
        pending.retention_expires_at = None;
        pending.owner_lease.host_instance_id = (*coordinator.host_instance_id).clone();
        pending.owner_lease.heartbeat_at = pending_now;
        pending.owner_lease.lease_expires_at = pending_now + chrono::Duration::minutes(1);
        pending.accepted_at = pending_now;
        pending.updated_at = pending_now;
        pending.terminal_at = None;
        store
            .record_background_subagent_acceptance(pending.clone())
            .await
            .unwrap();
        pending.execution_status = DurableBackgroundSubagentExecutionStatus::Starting;
        pending.updated_at = pending_now + chrono::Duration::milliseconds(1);
        store
            .update_background_subagent_execution(pending.clone())
            .await
            .unwrap();
        pending.execution_status = DurableBackgroundSubagentExecutionStatus::Running;
        pending.child_run_id = Some(RunId::from_string("pending-child-after-cancel"));
        pending.updated_at = pending_now + chrono::Duration::milliseconds(2);
        store
            .update_background_subagent_execution(pending.clone())
            .await
            .unwrap();
        pending.execution_status = DurableBackgroundSubagentExecutionStatus::Completed;
        pending.result_ref = Some(DurableBackgroundSubagentResultRef {
            content: Some("pending result after cancelled parent".to_string()),
            size_bytes: 37,
            ..DurableBackgroundSubagentResultRef::default()
        });
        pending.updated_at = pending_now + chrono::Duration::milliseconds(3);
        pending.terminal_at = Some(pending.updated_at);
        pending.retention_expires_at = Some(pending.updated_at + chrono::Duration::hours(1));
        store
            .record_background_subagent_terminal(pending)
            .await
            .unwrap();

        let live_consumer_run_id = RunId::from_string("live-background-consumer");
        let live_consumer = RunRecord::new(
            parent.session_id.clone(),
            live_consumer_run_id.clone(),
            ConversationId::new(),
        );
        let live_consumer_admission = store
            .acquire_run_admission(AcquireRunAdmission {
                run: live_consumer,
                namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
                host_instance_id: (*coordinator.host_instance_id).clone(),
                admission_id: "live-background-consumer-admission".to_string(),
                lease_expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
                idempotency_key: "live-background-consumer".to_string(),
                command_fingerprint: "live-background-consumer-v1".to_string(),
            })
            .await
            .unwrap();
        let live_claim_id = "live-background-consumer-claim";
        store
            .claim_background_subagent_delivery(
                &pending_attempt,
                starweaver_session::DurableBackgroundSubagentDeliveryClaim {
                    claim_id: live_claim_id.to_string(),
                    continuation_run_id: Some(live_consumer_run_id.clone()),
                    deadline: chrono::Utc::now() + chrono::Duration::milliseconds(10),
                },
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            Box::pin(coordinator.handle_background_completion(&pending_attempt))
                .await
                .unwrap()
                .is_none(),
            "the completion handler must not steal an expired claim from a live admitted consumer"
        );
        let live_claimed = store
            .load_background_subagent(&pending_attempt)
            .await
            .unwrap();
        assert_eq!(
            live_claimed.delivery_status,
            DurableBackgroundSubagentDeliveryStatus::Claimed
        );
        assert_eq!(
            live_claimed
                .delivery_claim
                .as_ref()
                .map(|claim| claim.claim_id.as_str()),
            Some(live_claim_id)
        );
        store
            .release_background_subagent_delivery(
                &pending_attempt,
                live_claim_id,
                DurableBackgroundSubagentDeliveryRelease::Retryable,
            )
            .await
            .unwrap();
        store
            .update_run_status(
                &parent.session_id,
                &live_consumer_run_id,
                RunStatus::Cancelled,
                Some("live-claim fixture cleanup".to_string()),
            )
            .await
            .unwrap();
        store
            .release_run_admission(&live_consumer_admission.lease)
            .await
            .unwrap();

        let cancelled_parent_claim_id = "cancelled-parent-active-turn-claim";
        store
            .claim_background_subagent_delivery(
                &pending_attempt,
                starweaver_session::DurableBackgroundSubagentDeliveryClaim {
                    claim_id: cancelled_parent_claim_id.to_string(),
                    continuation_run_id: Some(cancelled_parent_id.clone()),
                    deadline: chrono::Utc::now() + chrono::Duration::minutes(1),
                },
            )
            .await
            .unwrap();
        let released = store
            .release_background_subagent_delivery(
                &pending_attempt,
                cancelled_parent_claim_id,
                DurableBackgroundSubagentDeliveryRelease::ConsumerTerminated {
                    run_id: cancelled_parent_id.clone(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            released.delivery_status,
            DurableBackgroundSubagentDeliveryStatus::Undelivered
        );
        assert_eq!(
            released
                .automatic_continuation_suppressed_by_run_id
                .as_ref(),
            Some(&cancelled_parent_id)
        );
        store
            .reconcile_background_subagents(LOCAL_SESSION_NAMESPACE, chrono::Utc::now())
            .await
            .unwrap();
        assert!(
            Box::pin(coordinator.handle_background_completion(&pending_attempt))
                .await
                .unwrap()
                .is_none(),
            "a cancelled consumer suppresses automatic redelivery even when the causal parent completed"
        );

        let explicit = coordinator
            .start(RpcRunRequest {
                durable_input: vec![InputPart::text("continue explicitly")],
                input: AgentInput::text("continue explicitly"),
                session_id: Some(parent.session_id.clone()),
                restore_from_run_id: Some(parent.run_id.clone()),
                profile: "default".to_string(),
                environment_attachments: Vec::new(),
                idempotency_key: "explicit-after-cancelled-parent".to_string(),
                command_fingerprint: "explicit-after-cancelled-parent-v1".to_string(),
                install_session_management: false,
            })
            .await
            .unwrap();
        coordinator
            .await_terminal(
                &explicit.session_id,
                &explicit.run_id,
                Some(Duration::from_secs(5)),
            )
            .await
            .unwrap();
        let consumed = store
            .load_background_subagent(&pending_attempt)
            .await
            .unwrap();
        assert_eq!(
            consumed.delivery_status,
            DurableBackgroundSubagentDeliveryStatus::Delivered
        );
        let claim_id = consumed
            .delivered_claim_id
            .as_deref()
            .expect("explicit run must own the durable delivery claim");
        assert!(claim_id.contains(explicit.run_id.as_str()), "{claim_id}");
        assert!(claim_id.contains(pending_attempt.as_str()), "{claim_id}");
    }

    #[tokio::test]
    async fn startup_reconciliation_preserves_unexpired_foreign_lease() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        let storage = SqliteStorage::open(&config.database_path).unwrap();
        let session = storage
            .create_session(Some("default".to_string()), None)
            .unwrap();
        let run = RunRecord::new(
            session.session_id.clone(),
            RunId::new(),
            ConversationId::new(),
        );
        let receipt = storage
            .session_store()
            .acquire_run_admission(AcquireRunAdmission {
                run,
                namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
                host_instance_id: "still-live-foreign-host".to_string(),
                admission_id: "foreign-admission".to_string(),
                lease_expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                idempotency_key: "foreign-start".to_string(),
                command_fingerprint: "foreign-command".to_string(),
            })
            .await
            .unwrap();
        let catalog = RpcAgentCatalog::new(config.clone()).unwrap();
        let coordinator = RpcRuntimeCoordinator::new(
            config,
            catalog,
            storage.clone(),
            EnvironmentAttachmentManager::new(),
        );
        assert!(coordinator.reconcile_startup().await.unwrap().is_empty());
        assert!(
            storage
                .session_store()
                .load_run_admission(&receipt.lease.target)
                .await
                .unwrap()
                .is_some()
        );
    }
}
