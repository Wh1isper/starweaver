//! RPC-owned active-run coordination over the public Agent SDK.

use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use serde::Serialize;
use serde_json::{Value, json};
use starweaver_agent::{
    AgentContext, AgentControlHandle, AgentDurabilityError, AgentHitlResults,
    AgentSessionControlHandle, AgentSessionQueryHandle, AgentStreamDropPolicy, AgentStreamError,
    AgentStreamOptions, BackgroundSubagentSupervisor, BackgroundSubagentTaskResult,
    ContinuationMaterialization, ContinuationMaterializationMode, ResolvedAgentMaterialization,
    SubagentDelegationMode, attach_agent_session_control, attach_agent_session_query,
    environment_binding_class,
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
    ContinuationEffectState, DurableBackgroundSubagentDeliveryStatus,
    DurableBackgroundSubagentRetentionStatus, DurableControlReceipt, HitlResumeAbortOutcome,
    HitlResumeClaim, InputPart, LOCAL_SESSION_NAMESPACE, ManagedRunTarget, PreparedContinuation,
    RunAdmissionLease, RunAdmissionReceipt, RunRecord, RunStatus, SessionDeletionFence,
    SessionResumeSnapshot, SessionStatus, SessionStore, SessionStoreError,
};
use starweaver_storage::{DurableReplaySource, SqliteStorage};
use starweaver_stream::{
    DefaultDisplayMessageProjector, DisplayMessageProjector, DisplayProjectionContext,
    EnvironmentLifecycleEvent, ReplayCursor, ReplayEvent, ReplayEventKind, ReplayEventLog,
    ReplayScope, StreamArchive, StreamTerminalMarker,
};
use tokio::{
    sync::{Mutex as AsyncMutex, watch},
    task::JoinHandle,
};

use crate::{
    RpcAgentCatalog, RpcConfig, RpcHostError, RpcHostResult,
    environment::{
        effective_rpc_environment_attachments, resolve_rpc_environment,
        resolve_rpc_environment_target, safe_rpc_environment_attachments,
    },
    environment_manager::EnvironmentAttachmentManager,
    session_management::{RpcAgentSessionAdapter, command_fingerprint},
};

const DURABLE_SESSION_ID_METADATA_KEY: &str = "starweaver.durable_session_id";
const DURABLE_RUN_ID_METADATA_KEY: &str = "starweaver.durable_run_id";
const RPC_PROFILE_METADATA_KEY: &str = "rpc.profile";
const RPC_ENVIRONMENT_ATTACHMENTS_METADATA_KEY: &str = "rpc.environment_attachments";
const ACTIVE_LEASE_TTL: Duration = Duration::from_secs(30);
const ACTIVE_LEASE_HEARTBEAT: Duration = Duration::from_secs(10);
const DURABLE_TERMINAL_POLL_INTERVAL: Duration = Duration::from_millis(100);
const TERMINAL_CACHE_LIMIT: usize = 64;
const ACTIVE_EVENT_CACHE_LIMIT: usize = 2_048;
const DEFAULT_REPLAY_PAGE_LIMIT: usize = 200;
const MAX_REPLAY_PAGE_LIMIT: usize = 1_000;
const BACKGROUND_COMPLETION_TASK_LIMIT: usize = 256;
const BACKGROUND_RECORD_SCAN_LIMIT: usize = 1_024;
const BACKGROUND_RETENTION_CLEANUP_LIMIT: usize = 256;
const BACKGROUND_CONTINUATION_LEASE_TTL: Duration = Duration::from_secs(30);

/// Maintains a newly admitted run while RPC is still resolving state, injecting HITL results, and
/// registering the worker. The worker takes over with its own heartbeat after registration.
struct RpcPreworkerAdmissionHeartbeat {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl RpcPreworkerAdmissionHeartbeat {
    fn start(session_store: Arc<dyn SessionStore>, lease: RunAdmissionLease) -> Self {
        let (stop, mut stopped) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(ACTIVE_LEASE_HEARTBEAT);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = &mut stopped => break,
                    _ = interval.tick() => {
                        let expires = chrono::Utc::now()
                            + chrono::Duration::from_std(ACTIVE_LEASE_TTL).unwrap_or_default();
                        if session_store.heartbeat_run_admission(&lease, expires).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Self { stop: Some(stop) }
    }
}

impl Drop for RpcPreworkerAdmissionHeartbeat {
    fn drop(&mut self) {
        let _ = self.stop.take();
    }
}

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
    /// Explicit materialization policy for a restored run.
    pub continuation_mode: ContinuationMaterializationMode,
    /// Install profile-granted session query/control handles for this run.
    pub install_session_management: bool,
}

/// RPC HITL continuation request after wire parameters and attachments are validated.
#[derive(Clone, Debug)]
pub struct RpcHitlResumeRequest {
    /// Durable session containing the waiting source run.
    pub session_id: SessionId,
    /// Waiting run whose decisions are ready to consume.
    pub source_run_id: RunId,
    /// Profile used to materialize the continuation runtime.
    pub profile: String,
    /// Materialized host environment attachments for this continuation.
    pub environment_attachments: Vec<EnvironmentAttachmentRef>,
    /// Stable resume idempotency key.
    pub idempotency_key: String,
    /// Normalized typed resume-command fingerprint.
    pub command_fingerprint: String,
    /// Explicit materialization policy for this continuation.
    pub continuation_mode: ContinuationMaterializationMode,
    /// Install profile-granted session query/control handles for this run.
    pub install_session_management: bool,
}

#[derive(Clone)]
struct HitlLaunch {
    snapshot: SessionResumeSnapshot,
    results: AgentHitlResults,
    claim_id: String,
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
    /// Fail-closed effect recovery projection when a started continuation lost its host.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation_effect: Option<ContinuationEffectState>,
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
    replay_publish_lock: Arc<AsyncMutex<()>>,
    next_display_sequence: usize,
    next_event_sequence: usize,
    terminal_replay_sequence: Arc<AtomicUsize>,
    replay_error: Option<String>,
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
}

/// RPC-owned active mount mutation outcome used by the service boundary.
#[derive(Debug)]
pub struct RpcActiveMountOutcome {
    /// Wire result.
    pub result: Value,
    /// Whether this request applied a new mutation rather than replaying an idempotent result.
    #[cfg(test)]
    pub applied: bool,
}

/// RPC-owned active unmount mutation outcome used by the service boundary.
#[derive(Debug)]
pub struct RpcActiveUnmountOutcome {
    /// Wire result.
    pub result: Value,
    /// Whether this request applied a new mutation rather than replaying an idempotent result.
    #[cfg(test)]
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

    fn materialization_plan(
        &self,
        profile: &str,
        attachments: &[EnvironmentAttachmentRef],
        source: Option<&RunRecord>,
        mode: ContinuationMaterializationMode,
    ) -> RpcHostResult<(
        ResolvedAgentMaterialization,
        Option<ContinuationMaterialization>,
    )> {
        let binding_class = environment_binding_class(attachments.iter().map(|attachment| {
            let mode = match attachment.resolved_mode() {
                starweaver_rpc_core::EnvironmentAttachmentAccessMode::ReadOnly => "read_only",
                starweaver_rpc_core::EnvironmentAttachmentAccessMode::ReadWrite => "read_write",
            };
            (attachment.kind.clone(), mode.to_string())
        }));
        let materialization = self.catalog.materialization(profile, binding_class)?;
        let continuation = source
            .map(|source| {
                let source_materialization =
                    ResolvedAgentMaterialization::from_metadata(&source.metadata)
                        .map_err(|error| RpcHostError::Invalid(error.to_string()))?;
                let assessment = ContinuationMaterialization::assess(
                    source_materialization.as_ref(),
                    &materialization,
                    mode,
                );
                if !assessment.allowed {
                    return Err(RpcHostError::Invalid(format!(
                        "continuation materialization mode {} rejected drift: {}",
                        assessment.mode.as_str(),
                        assessment.drift_summary()
                    )));
                }
                Ok(assessment)
            })
            .transpose()?;
        Ok((materialization, continuation))
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

    /// Resume a durable waiting run through the ordinary active-run pipeline.
    ///
    /// All materialization and HITL validation occurs before the exclusive claim is acquired.
    /// Waiting replacement admission atomically transitions that claim to `Admitted`; the runtime
    /// effect boundary later validates the live admission and advances it to `Started`. From that
    /// point failures are persisted as related-run evidence instead of releasing the claim.
    ///
    /// # Errors
    ///
    /// Returns validation, storage, or runtime materialization failures.
    #[must_use]
    pub fn resume_waiting(&self, request: RpcHitlResumeRequest) -> RpcBoxFuture<'_, RpcStartedRun> {
        Box::pin(self.resume_waiting_inner(request))
    }

    #[allow(clippy::too_many_lines)]
    async fn resume_waiting_inner(
        &self,
        mut request: RpcHitlResumeRequest,
    ) -> RpcHostResult<RpcStartedRun> {
        request.environment_attachments =
            effective_rpc_environment_attachments(&request.environment_attachments);
        if !self.accepting.load(Ordering::Acquire) {
            return Err(RpcHostError::Runtime(
                "RPC coordinator is shutting down and no longer accepts runs".to_string(),
            ));
        }
        self.reap_finished_tasks().await?;
        let identity = command_fingerprint(
            "rpc_hitl_resume_identity",
            &json!({
                "sessionId": request.session_id,
                "sourceRunId": request.source_run_id,
                "idempotencyKey": request.idempotency_key,
                "commandFingerprint": request.command_fingerprint,
            }),
        )
        .map_err(|error| RpcHostError::Invalid(error.to_string()))?;
        let identity_suffix = identity.rsplit(':').next().unwrap_or(identity.as_str());
        let continuation_run_id = RunId::from_string(format!("run_rpc_hitl_{identity_suffix}"));
        let claim_id = format!("rpc-hitl-claim-{identity_suffix}");
        let store = self.storage.session_store();
        // Read durable idempotency truth before mutable source evidence. This lookup is strictly
        // non-mutating, so a Preflight claim orphaned before admission cannot accidentally create
        // a queued continuation merely because a client retried.
        if let Some(replay) = store
            .load_run_admission_receipt(
                LOCAL_SESSION_NAMESPACE,
                &request.idempotency_key,
                &request.command_fingerprint,
            )
            .await?
        {
            let status = store
                .load_run(&request.session_id, &continuation_run_id)
                .await
                .map_or(replay.run.status, |run| run.status);
            let environment_attachments = recorded_environment_attachments(&replay.run);
            return Ok(RpcStartedRun {
                session_id: request.session_id,
                run_id: continuation_run_id,
                environment_attachments,
                admission_id: replay.lease.admission_id,
                fencing_generation: replay.lease.fencing_generation,
                status,
                idempotent_replay: true,
            });
        }
        let snapshot = store
            .resume_snapshot(&request.session_id, &request.source_run_id)
            .await?;
        if snapshot.run.status != RunStatus::Waiting {
            // An admission may have committed between the first receipt lookup and this snapshot.
            // Recheck without mutating; otherwise the source simply is not resumable.
            if let Some(replay) = store
                .load_run_admission_receipt(
                    LOCAL_SESSION_NAMESPACE,
                    &request.idempotency_key,
                    &request.command_fingerprint,
                )
                .await?
            {
                let status = store
                    .load_run(&request.session_id, &continuation_run_id)
                    .await
                    .map_or(replay.run.status, |run| run.status);
                let environment_attachments = recorded_environment_attachments(&replay.run);
                return Ok(RpcStartedRun {
                    session_id: request.session_id,
                    run_id: continuation_run_id,
                    environment_attachments,
                    admission_id: replay.lease.admission_id,
                    fencing_generation: replay.lease.fencing_generation,
                    status,
                    idempotent_replay: true,
                });
            }
            return Err(RpcHostError::Invalid(format!(
                "run {} is not waiting",
                request.source_run_id.as_str()
            )));
        }
        let prepared = PreparedContinuation::waiting_hitl(snapshot)
            .map_err(|error| RpcHostError::Invalid(error.to_string()))?;
        let checkpoint_state = prepared
            .waiting_state()
            .cloned()
            .ok_or_else(|| RpcHostError::Runtime("missing prepared HITL state".to_string()))?;
        let results = AgentHitlResults::from_prepared_continuation(&prepared);
        let snapshot = prepared.into_snapshot();

        // Resolve the complete runtime boundary before claiming. This may open provider handles,
        // but it does not preprocess user input, execute tools, or mutate durable claim state.
        let resolved_environment = resolve_rpc_environment(
            &self.config.workspace_root,
            request.session_id.as_str(),
            &request.environment_attachments,
        )?;
        let (materialization, continuation) = self.materialization_plan(
            &request.profile,
            &request.environment_attachments,
            Some(&snapshot.run),
            request.continuation_mode,
        )?;
        let preflight_context = AgentContext::from_state(snapshot.state.clone());
        let preflight_runtime = self
            .catalog
            .runtime_builder(&request.profile)?
            .context(preflight_context)
            .environment(resolved_environment.provider)
            .build();
        preflight_runtime
            .session()
            .validate_hitl_results_for_state(&checkpoint_state, &results)
            .map_err(|error| RpcHostError::Invalid(error.to_string()))?;

        let mut run = RunRecord::new(
            request.session_id.clone(),
            continuation_run_id,
            snapshot.run.conversation_id.clone(),
        );
        run.profile = Some(request.profile.clone());
        run.restore_from_run_id = Some(request.source_run_id.clone());
        run.trigger_type = Some("rpc_hitl_resume".to_string());
        run.status = RunStatus::Queued;
        run.metadata
            .insert(RPC_PROFILE_METADATA_KEY.to_string(), json!(request.profile));
        run.metadata.insert(
            RPC_ENVIRONMENT_ATTACHMENTS_METADATA_KEY.to_string(),
            json!(safe_rpc_environment_attachments(
                &request.environment_attachments
            )),
        );
        materialization
            .insert_into(&mut run.metadata)
            .map_err(|error| RpcHostError::Invalid(error.to_string()))?;
        let continuation = continuation.ok_or_else(|| {
            RpcHostError::Runtime("missing HITL continuation materialization plan".to_string())
        })?;
        continuation
            .insert_into(&mut run.metadata)
            .map_err(|error| RpcHostError::Invalid(error.to_string()))?;
        let admission_request = AcquireRunAdmission {
            run,
            namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
            host_instance_id: (*self.host_instance_id).clone(),
            admission_id: format!("admission_{identity_suffix}"),
            lease_expires_at: chrono::Utc::now()
                + chrono::Duration::from_std(ACTIVE_LEASE_TTL).unwrap_or_default(),
            idempotency_key: request.idempotency_key.clone(),
            command_fingerprint: request.command_fingerprint.clone(),
            replaces_waiting_run_id: Some(request.source_run_id.clone()),
            hitl_resume_claim_id: Some(claim_id.clone()),
        };
        let claim = HitlResumeClaim::new(
            claim_id.clone(),
            request.session_id.clone(),
            request.source_run_id.clone(),
            chrono::Utc::now(),
        );
        let admission = match store.claim_hitl_resume(claim).await {
            Ok(()) => match store.acquire_run_admission(admission_request.clone()).await {
                Ok(admission) => admission,
                Err(error) => {
                    let _ = store
                        .release_hitl_resume_claim(
                            &request.session_id,
                            &request.source_run_id,
                            &claim_id,
                        )
                        .await;
                    return Err(error.into());
                }
            },
            // A prior host may have durably written this deterministic Preflight claim and
            // stopped before admission. After the complete side-effect-free preflight above, the
            // store can safely consume that exact claim into one fenced admission. A concurrent
            // winner is returned as an idempotent receipt by the same call.
            Err(claim_error) => match store.acquire_run_admission(admission_request).await {
                Ok(admission) => admission,
                Err(_) => return Err(claim_error.into()),
            },
        };
        let launch = HitlLaunch {
            snapshot,
            results,
            claim_id,
        };
        Box::pin(self.start_preadmitted_hitl(
            RpcRunRequest {
                durable_input: Vec::new(),
                input: AgentInput::text(""),
                session_id: Some(request.session_id),
                restore_from_run_id: Some(request.source_run_id),
                profile: request.profile,
                environment_attachments: request.environment_attachments,
                idempotency_key: request.idempotency_key,
                command_fingerprint: request.command_fingerprint,
                continuation_mode: request.continuation_mode,
                install_session_management: request.install_session_management,
            },
            admission,
            launch,
        ))
        .await
    }

    /// Read an exact durable start receipt without probing external resources or changing state.
    ///
    /// # Errors
    ///
    /// Returns an idempotency conflict or storage error.
    pub async fn lookup_started_run(
        &self,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> RpcHostResult<Option<RpcStartedRun>> {
        let Some(receipt) = self
            .storage
            .session_store()
            .load_run_admission_receipt(
                LOCAL_SESSION_NAMESPACE,
                idempotency_key,
                command_fingerprint,
            )
            .await?
        else {
            return Ok(None);
        };
        let status = self
            .storage
            .session_store()
            .load_run(&receipt.run.session_id, &receipt.run.run_id)
            .await
            .map_or(receipt.run.status, |run| run.status);
        Ok(Some(RpcStartedRun {
            session_id: receipt.run.session_id.clone(),
            run_id: receipt.run.run_id.clone(),
            environment_attachments: recorded_environment_attachments(&receipt.run),
            admission_id: receipt.lease.admission_id,
            fencing_generation: receipt.lease.fencing_generation,
            status,
            idempotent_replay: true,
        }))
    }

    /// Start one live run directly through `AgentRuntime`.
    ///
    /// # Errors
    ///
    /// Returns storage or runtime construction failures.
    #[must_use]
    pub fn start(&self, request: RpcRunRequest) -> RpcBoxFuture<'_, RpcStartedRun> {
        Box::pin(self.start_inner(request, None, None))
    }

    async fn start_preadmitted_hitl(
        &self,
        request: RpcRunRequest,
        admission: RunAdmissionReceipt,
        launch: HitlLaunch,
    ) -> RpcHostResult<RpcStartedRun> {
        let result =
            Box::pin(self.start_inner(request, Some(admission.clone()), Some(launch.clone())))
                .await;
        let error = match result {
            Ok(started) => return Ok(started),
            Err(error) => error,
        };

        // From admission until the worker owns the lease, this wrapper is the single failure
        // owner. Inner runtime preparation may already have committed related-run evidence; in
        // that case durable terminal state is authoritative and must not be written a second time.
        let store = self.storage.session_store();
        let mut durable = store
            .load_run(&admission.run.session_id, &admission.run.run_id)
            .await?;
        if !durable.status.is_terminal() {
            // This phase-aware operation proves whether an approved effect can have run. An
            // admitted replacement is safely aborted without consuming its waiting source;
            // a started replacement must instead write fail-closed related-run evidence.
            match store
                .abort_admitted_hitl_resume(
                    &admission.lease,
                    &launch.snapshot.run.run_id,
                    &launch.claim_id,
                    "runtime preparation failed",
                )
                .await
            {
                Ok(HitlResumeAbortOutcome::AbortedBeforeEffect) => {}
                Ok(HitlResumeAbortOutcome::EffectStarted) => {
                    if let Err(persist_error) = self
                        .persist_started_hitl_launch_failure(
                            &admission,
                            &launch,
                            "runtime preparation failed",
                        )
                        .await
                    {
                        return Err(RpcHostError::Runtime(format!(
                            "{error}; failed to persist started continuation failure: {persist_error}"
                        )));
                    }
                }
                Err(abort_error) => {
                    return Err(RpcHostError::Runtime(format!(
                        "{error}; failed to reconcile admitted continuation failure: {abort_error}"
                    )));
                }
            }
            durable = store
                .load_run(&admission.run.session_id, &admission.run.run_id)
                .await?;
        }
        if let Err(finalize_error) = store
            .finalize_run_admission(
                &admission.lease,
                durable.status,
                durable.output_preview.clone(),
            )
            .await
        {
            return Err(RpcHostError::Runtime(format!(
                "{error}; terminal evidence committed but admission release requires reconciliation: {finalize_error}"
            )));
        }
        Err(error)
    }

    async fn persist_started_hitl_launch_failure(
        &self,
        admission: &RunAdmissionReceipt,
        launch: &HitlLaunch,
        message: &str,
    ) -> RpcHostResult<()> {
        let mut run = admission.run.clone();
        run.status = RunStatus::Failed;
        run.output_preview = Some(message.to_string());
        run.updated_at = chrono::Utc::now();
        let mut state = launch.snapshot.state.clone();
        state.session_id = Some(run.session_id.clone());
        state.run_id = Some(run.run_id.clone());
        state.metadata.insert(
            DURABLE_SESSION_ID_METADATA_KEY.to_string(),
            json!(run.session_id.as_str()),
        );
        state.metadata.insert(
            DURABLE_RUN_ID_METADATA_KEY.to_string(),
            json!(run.run_id.as_str()),
        );
        let mut source_update = starweaver_session::RelatedRunUpdate::new(
            launch.snapshot.run.run_id.clone(),
            RunStatus::Waiting,
            RunStatus::Failed,
        );
        source_update.resume_claim_id = Some(launch.claim_id.clone());
        source_update.output_preview = Some("continuation launch failed".to_string());
        source_update
            .approvals
            .clone_from(&launch.snapshot.approvals);
        source_update
            .deferred_tools
            .clone_from(&launch.snapshot.deferred_tools);
        let mut commit = starweaver_session::RunEvidenceCommit::new(run, state);
        commit.related_run_updates.push(source_update);
        self.storage
            .session_store()
            .commit_run_evidence_fenced(&admission.lease, commit)
            .await?;
        Ok(())
    }

    async fn finalize_preworker_failure(
        &self,
        admission: &RunAdmissionReceipt,
        fallback: &str,
    ) -> RpcHostResult<RunRecord> {
        let store = self.storage.session_store();
        let durable = store
            .load_run(&admission.run.session_id, &admission.run.run_id)
            .await?;
        let (status, output_preview) = if durable.status.is_terminal() {
            // Stream draining commits complete evidence before admission release. That durable
            // result is authoritative even when the surrounding startup path also failed.
            (durable.status, durable.output_preview)
        } else {
            (RunStatus::Failed, Some(fallback.to_string()))
        };
        Ok(store
            .finalize_run_admission(&admission.lease, status, output_preview)
            .await?)
    }

    async fn ensure_started_hitl_terminal_evidence(
        &self,
        admission: &RunAdmissionReceipt,
        launch: &HitlLaunch,
        fallback: &str,
    ) -> RpcHostResult<RunRecord> {
        let store = self.storage.session_store();
        let durable = store
            .load_run(&admission.run.session_id, &admission.run.run_id)
            .await?;
        if durable.status.is_terminal() {
            return Ok(durable);
        }
        self.persist_started_hitl_launch_failure(admission, launch, fallback)
            .await?;
        let durable = store
            .load_run(&admission.run.session_id, &admission.run.run_id)
            .await?;
        if !durable.status.is_terminal() {
            return Err(RpcHostError::Runtime(
                "started HITL continuation has no atomic terminal related-run evidence".to_string(),
            ));
        }
        Ok(durable)
    }

    #[allow(clippy::too_many_lines)]
    async fn start_inner(
        &self,
        mut request: RpcRunRequest,
        preadmitted: Option<RunAdmissionReceipt>,
        hitl_launch: Option<HitlLaunch>,
    ) -> RpcHostResult<RpcStartedRun> {
        request.environment_attachments =
            effective_rpc_environment_attachments(&request.environment_attachments);
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
        let materialization_plan = if preadmitted.is_none() {
            let source = match request.restore_from_run_id.as_ref() {
                Some(run_id) => Some(
                    self.storage
                        .session_store()
                        .load_run(&session_id, run_id)
                        .await?,
                ),
                None => None,
            };
            Some(self.materialization_plan(
                &request.profile,
                &request.environment_attachments,
                source.as_ref(),
                request.continuation_mode,
            )?)
        } else {
            None
        };
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
            run.metadata.insert(
                RPC_ENVIRONMENT_ATTACHMENTS_METADATA_KEY.to_string(),
                json!(safe_rpc_environment_attachments(
                    &request.environment_attachments
                )),
            );
            let (materialization, continuation) =
                materialization_plan.as_ref().ok_or_else(|| {
                    RpcHostError::Runtime(
                        "missing RPC materialization plan before admission".to_string(),
                    )
                })?;
            materialization
                .insert_into(&mut run.metadata)
                .map_err(|error| RpcHostError::Invalid(error.to_string()))?;
            if let Some(continuation) = continuation.as_ref() {
                continuation
                    .insert_into(&mut run.metadata)
                    .map_err(|error| RpcHostError::Invalid(error.to_string()))?;
            }
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
                    replaces_waiting_run_id: None,
                    hitl_resume_claim_id: None,
                })
                .await?
        };
        let run_id = admission.run.run_id.clone();
        let target = admission.lease.target.clone();
        if admission.idempotent_replay && hitl_launch.is_some() {
            let environment_attachments = recorded_environment_attachments(&admission.run);
            let status = self
                .storage
                .session_store()
                .load_run(&session_id, &run_id)
                .await
                .map_or(admission.run.status, |run| run.status);
            return Ok(RpcStartedRun {
                session_id,
                run_id,
                environment_attachments,
                admission_id: admission.lease.admission_id,
                fencing_generation: admission.lease.fencing_generation,
                status,
                idempotent_replay: true,
            });
        }
        if admission.idempotent_replay && !launch_preadmitted {
            let environment_attachments = recorded_environment_attachments(&admission.run);
            let status = self
                .storage
                .session_store()
                .load_run(&session_id, &run_id)
                .await
                .map_or(admission.run.status, |run| run.status);
            return Ok(RpcStartedRun {
                session_id,
                run_id,
                environment_attachments,
                admission_id: admission.lease.admission_id,
                fencing_generation: admission.lease.fencing_generation,
                status,
                idempotent_replay: true,
            });
        }
        // Start lease maintenance before any potentially slow durable result hydration or HITL
        // injection. Without this guard a 30-second admission can expire before the worker's
        // existing heartbeat is installed.
        let preworker_heartbeat = RpcPreworkerAdmissionHeartbeat::start(
            Arc::new(self.storage.session_store()),
            admission.lease.clone(),
        );
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

        let prepared: RpcHostResult<_> = Box::pin(async {
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
            let terminal_replay_sequence = Arc::new(AtomicUsize::new(0));
            let mut runtime = self
                .catalog
                .runtime_builder(&request.profile)?
                .subagent_delegation_mode(SubagentDelegationMode::Async)
                .background_subagent_supervisor(supervisor.clone())
                .context(context)
                .environment(resolved_environment.provider.clone())
                .durable_session_id(session_id.clone())
                .session_store(session_store)
                .admission_lease(admission.lease.clone())
                .stream_archive(stream_archive)
                .retain_terminal_replay_evidence()
                .terminal_replay_sequence(Arc::clone(&terminal_replay_sequence))
                .build();
            let input = request.input.clone();
            if let Some(launch) = hitl_launch.as_ref()
                && let Err(error) = runtime
                    .inject_prepared_hitl_results(&launch.snapshot.run.run_id, &launch.claim_id)
                    .await
            {
                let message = format!("HITL result injection failed: {error}");
                let persisted = runtime
                    .persist_hitl_injection_failure(
                        &launch.snapshot,
                        &launch.results,
                        launch.claim_id.clone(),
                        message.clone(),
                    )
                    .await;
                return Err(RpcHostError::Runtime(match persisted {
                    Ok(()) => message,
                    Err(persist_error) => {
                        format!("{message}; failed to persist started continuation failure: {persist_error}")
                    }
                }));
            }
            let handle = match runtime.try_stream_with_stream_options(
                input.clone(),
                AgentStreamOptions::new().drop_policy(AgentStreamDropPolicy::Backpressure),
            ) {
                Ok(handle) => handle,
                Err(error) => {
                    let message = error.to_string();
                    if let Some(launch) = hitl_launch.as_ref() {
                        let persisted = runtime
                            .persist_hitl_injection_failure(
                                &launch.snapshot,
                                &launch.results,
                                launch.claim_id.clone(),
                                message.clone(),
                            )
                            .await;
                        return Err(RpcHostError::Runtime(match persisted {
                            Ok(()) => message,
                            Err(persist_error) => format!(
                                "{message}; failed to persist started continuation failure: {persist_error}"
                            ),
                        }));
                    }
                    return Err(RpcHostError::Runtime(message));
                }
            };
            Ok((
                runtime,
                input,
                handle,
                resolved_environment,
                terminal_replay_sequence,
            ))
        })
        .await;
        let (mut runtime, input, mut handle, resolved_environment, terminal_replay_sequence) =
            match prepared {
                Ok(prepared) => prepared,
                Err(error) => {
                    if hitl_launch.is_some() {
                        // `start_preadmitted_hitl` owns exactly-once related-run failure evidence
                        // and admission release for every pre-worker error.
                        return Err(error);
                    }
                    let _ = self
                        .storage
                        .session_store()
                        .finalize_run_admission(
                            &admission.lease,
                            RunStatus::Failed,
                            Some("runtime preparation failed".to_string()),
                        )
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
            let hitl_completion_error = if let Some(launch) = hitl_launch.as_ref() {
                runtime
                    .finish_hitl_stream(
                        input,
                        handle,
                        &launch.snapshot,
                        &launch.results,
                        launch.claim_id.clone(),
                    )
                    .await
                    .err()
            } else {
                let _ = runtime.finish_stream(input, handle).await;
                None
            };
            supervisor.end_parent_run(&run_id);
            if let Some(hitl_error) = hitl_completion_error {
                return Err(RpcHostError::Runtime(format!(
                    "{}; HITL stream completion requires atomic reconciliation: {hitl_error}",
                    error.message
                )));
            }
            let cleanup = self
                .finalize_preworker_failure(&admission, "environment lease registration failed")
                .await;
            if let Err(cleanup_error) = cleanup {
                return Err(RpcHostError::Runtime(format!(
                    "{}; admission cleanup requires reconciliation: {cleanup_error}",
                    error.message
                )));
            }
            return Err(RpcHostError::Invalid(error.message));
        }
        let initial_status = RpcRunStatus {
            session_id: session_id.as_str().to_string(),
            run_id: run_id.as_str().to_string(),
            status: "running".to_string(),
            output_preview: None,
            error: None,
            continuation_effect: None,
        };
        let (status_tx, _status_rx) = watch::channel(initial_status);
        let active_run = ActiveRun {
            status_tx,
            control,
            lease: admission.lease.clone(),
            events: Vec::new(),
            replay_publish_lock: Arc::new(AsyncMutex::new(())),
            next_display_sequence: 0,
            next_event_sequence: 0,
            terminal_replay_sequence,
            replay_error: None,
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
            .update_run_status_fenced(&admission.lease, RunStatus::Running, None)
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
            let hitl_completion_error = if let Some(launch) = hitl_launch.as_ref() {
                runtime
                    .finish_hitl_stream(
                        input,
                        handle,
                        &launch.snapshot,
                        &launch.results,
                        launch.claim_id.clone(),
                    )
                    .await
                    .err()
            } else {
                let _ = runtime.finish_stream(input, handle).await;
                None
            };
            let _ = self.environment_manager.mark_run_finished(run_id.as_str());
            supervisor.end_parent_run(&run_id);
            if let Some(hitl_error) = hitl_completion_error {
                return Err(RpcHostError::Runtime(format!(
                    "{error}; HITL stream completion requires atomic reconciliation: {hitl_error}"
                )));
            }
            let cleanup = self
                .finalize_preworker_failure(&admission, "durable running transition failed")
                .await;
            let primary: RpcHostError = error.into();
            if let Err(cleanup_error) = cleanup {
                return Err(RpcHostError::Runtime(format!(
                    "{primary}; admission cleanup requires reconciliation: {cleanup_error}"
                )));
            }
            return Err(primary);
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
        let mut worker_lease = admission.lease.clone();
        let worker_admission = admission.clone();
        let worker_supervisor = supervisor.clone();
        let worker_hitl_launch = hitl_launch;
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
                            &store,
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
                            Ok(renewed) => {
                                worker_lease = renewed.clone();
                                if let Ok(mut registry) = active.lock()
                                    && let Some(run) = registry.get_mut(&worker_target)
                                {
                                    run.lease = renewed;
                                }
                            }
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
            let terminal_publish_lock = {
                let Ok(registry) = active.lock() else {
                    return;
                };
                let Some(active_run) = registry.get(&worker_target) else {
                    return;
                };
                Arc::clone(&active_run.replay_publish_lock)
            };
            // Terminal evidence, durable publication, cache visibility, and active-run removal
            // share the same barrier as display and environment lifecycle publication. Once this
            // guard is held no later mutation may reserve the terminal sequence or alter bindings.
            let _terminal_publish_guard = terminal_publish_lock.lock().await;
            let replay_error = active
                .lock()
                .ok()
                .and_then(|registry| registry.get(&worker_target)?.replay_error.clone());
            if let Some(replay_error) = replay_error {
                let hitl_evidence_error = if let Some(launch) = worker_hitl_launch.as_ref() {
                    let _ = runtime
                        .finish_hitl_stream(
                            input,
                            handle,
                            &launch.snapshot,
                            &launch.results,
                            launch.claim_id.clone(),
                        )
                        .await;
                    completion_coordinator
                        .ensure_started_hitl_terminal_evidence(
                            &worker_admission,
                            launch,
                            "live replay persistence failed",
                        )
                        .await
                        .err()
                } else {
                    let _ = handle.complete().await;
                    None
                };
                let message = format!("live replay persistence failed: {replay_error}");
                let terminal_durable = if hitl_evidence_error.is_some() {
                    false
                } else {
                    store
                        .finalize_run_admission(
                            &worker_lease,
                            RunStatus::Failed,
                            Some(message.clone()),
                        )
                        .await
                        .is_ok()
                };
                let mut final_status = RpcRunStatus {
                    session_id: worker_session_id.as_str().to_string(),
                    run_id: worker_run_id.as_str().to_string(),
                    status: "failed".to_string(),
                    output_preview: None,
                    error: Some(message.clone()),
                    continuation_effect: None,
                };
                if let Some(hitl_error) = hitl_evidence_error {
                    final_status.error = Some(format!(
                        "{message}; atomic HITL terminal evidence requires reconciliation: {hitl_error}"
                    ));
                } else if !terminal_durable {
                    final_status.error = Some(
                        "failed to persist replay failure as the terminal durable status"
                            .to_string(),
                    );
                }
                if let Err(delivery_error) =
                    finalize_parent_deliveries_with_retry(&worker_supervisor, &worker_run_id, false)
                        .await
                {
                    final_status.error.get_or_insert_with(|| {
                        format!("failed to roll back background result delivery: {delivery_error}")
                    });
                }
                worker_supervisor.end_parent_run(&worker_run_id);
                if let Ok(registry) = active.lock()
                    && let Some(active_run) = registry.get(&worker_target)
                {
                    let _ = active_run.status_tx.send(final_status.clone());
                }
                if let Err(cleanup) = environment_manager.mark_run_finished(worker_run_id.as_str())
                {
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
                return;
            }
            let completion = if let Some(launch) = worker_hitl_launch.as_ref() {
                runtime
                    .finish_hitl_stream(
                        input,
                        handle,
                        &launch.snapshot,
                        &launch.results,
                        launch.claim_id.clone(),
                    )
                    .await
            } else {
                runtime.finish_stream(input, handle).await
            };
            let hitl_evidence_error = if let Some(launch) = worker_hitl_launch.as_ref() {
                completion_coordinator
                    .ensure_started_hitl_terminal_evidence(
                        &worker_admission,
                        launch,
                        "continuation completion persistence failed",
                    )
                    .await
                    .err()
            } else {
                None
            };
            let (status, durable_status, output_preview, error) = match completion {
                Ok(result) => (
                    run_status_name(result.result.state.status).to_string(),
                    durable_status_from_runtime(result.result.state.status),
                    (!result.result.output.is_empty()).then_some(result.result.output),
                    None,
                ),
                Err(error) => {
                    let cancelled = durability_error_is_cancelled(&error);
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
            let mut final_status = RpcRunStatus {
                session_id: worker_session_id.as_str().to_string(),
                run_id: worker_run_id.as_str().to_string(),
                status,
                output_preview: output_preview.clone(),
                error,
                continuation_effect: None,
            };
            if let Some(hitl_error) = hitl_evidence_error.as_ref() {
                final_status.status = "failed".to_string();
                final_status.error = Some(format!(
                    "atomic HITL terminal evidence requires reconciliation: {hitl_error}"
                ));
            }
            let finalized_status = durable_status;
            let finalized_output = output_preview;
            if let Err(persist_error) =
                publish_committed_terminal_events(&active, &replay_log, &worker_target, marker)
                    .await
            {
                final_status.error.get_or_insert_with(|| {
                    format!("failed to load committed terminal replay events: {persist_error}")
                });
            }
            let terminal_durable = if hitl_evidence_error.is_some() {
                false
            } else {
                match store
                    .finalize_run_admission(&worker_lease, finalized_status, finalized_output)
                    .await
                {
                    Ok(_) => true,
                    Err(finalize_error) => {
                        // `finish_stream` commits terminal evidence before admission release. If that
                        // evidence is present, it remains authoritative; lease cleanup is recovered by
                        // reconciliation and must not rewrite a completed run as process-local failed.
                        match store.load_run(&worker_session_id, &worker_run_id).await {
                            Ok(run)
                                if run.status == finalized_status && run.status.is_terminal() =>
                            {
                                final_status.error.get_or_insert_with(|| {
                                format!(
                                    "terminal evidence committed but admission release requires reconciliation: {finalize_error}"
                                )
                            });
                                true
                            }
                            _ => {
                                final_status.status = "failed".to_string();
                                final_status.error.get_or_insert_with(|| {
                                format!(
                                    "failed to persist terminal durable run status: {finalize_error}"
                                )
                            });
                                false
                            }
                        }
                    }
                }
            };
            if let Err(delivery_error) = finalize_parent_deliveries_with_retry(
                &worker_supervisor,
                &worker_run_id,
                terminal_durable && finalized_status == RunStatus::Completed,
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
        // The worker owns its heartbeat from the spawned task onward.
        drop(preworker_heartbeat);
        self.tasks
            .lock()
            .map_err(active_registry_error)?
            .insert(target, task);

        Ok(RpcStartedRun {
            session_id,
            run_id,
            environment_attachments: safe_rpc_environment_attachments(
                &resolved_environment.attachments,
            ),
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
        // Admission/reconciliation can terminalize a durable run inline while a previous host
        // still has a local active or terminal-cache projection. Durable terminal evidence wins
        // so callers can observe a fail-closed continuation effect immediately.
        let durable = self
            .storage
            .session_store()
            .load_run(session_id, run_id)
            .await?;
        if durable.status.is_terminal() {
            return Ok(status_from_record(&durable));
        }
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
        Ok(status_from_record(&durable))
    }

    fn durable_terminal_status(
        durable: &RunRecord,
        local_terminal: Option<&RpcRunStatus>,
    ) -> RpcRunStatus {
        let mut status = status_from_record(durable);
        // The durable record is authoritative for the terminal outcome. When the matching local
        // worker supplied a diagnostic before its final transaction committed, retain that
        // diagnostic without allowing it to choose or override the durable terminal state.
        if let Some(local) = local_terminal
            && local.status == status.status
            && status.error.is_none()
        {
            status.error.clone_from(&local.error);
        }
        status
    }

    async fn await_durable_terminal(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        local_terminal: Option<RpcRunStatus>,
    ) -> RpcHostResult<RpcRunStatus> {
        loop {
            let durable = self
                .storage
                .session_store()
                .load_run(session_id, run_id)
                .await?;
            if durable.status.is_terminal() {
                return Ok(Self::durable_terminal_status(
                    &durable,
                    local_terminal.as_ref(),
                ));
            }
            tokio::time::sleep(DURABLE_TERMINAL_POLL_INTERVAL).await;
        }
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
                // A remote owner or a restarted host has no process-local watch channel. It must
                // still honor the `run.await` terminal-only contract, so read durable evidence
                // until terminal rather than returning a queued/running status.
                return self.await_durable_terminal(session_id, run_id, None).await;
            };
            // A foreign host or inline lease reconciliation can terminalize durable evidence
            // without publishing to this process-local watch channel. Poll independently from the
            // lease heartbeat so short `run.await` deadlines can still observe remote completion.
            let mut durable_poll = tokio::time::interval(DURABLE_TERMINAL_POLL_INTERVAL);
            durable_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                let status = receiver.borrow().clone();
                if status.terminal() {
                    // A worker can publish a process-local failure before its terminal evidence
                    // transaction commits. `run.await` is durable-terminal-only, so never expose
                    // that provisional projection as a completed await result.
                    let durable = self
                        .storage
                        .session_store()
                        .load_run(session_id, run_id)
                        .await?;
                    if durable.status.is_terminal() {
                        return Ok(Self::durable_terminal_status(&durable, Some(&status)));
                    }
                    return self
                        .await_durable_terminal(session_id, run_id, Some(status))
                        .await;
                }
                tokio::select! {
                    changed = receiver.changed() => {
                        if changed.is_err() {
                            // The local worker can remove its active entry before durable failure
                            // reconciliation commits. Once the sender closes, preserve await's
                            // terminal-only contract by following durable evidence instead of the
                            // general-purpose status projection.
                            return self.await_durable_terminal(session_id, run_id, None).await;
                        }
                    }
                    _ = durable_poll.tick() => {
                        let durable = self
                            .storage
                            .session_store()
                            .load_run(session_id, run_id)
                            .await?;
                        if durable.status.is_terminal() {
                            return Ok(status_from_record(&durable));
                        }
                    }
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
        let run = self
            .storage
            .session_store()
            .load_run(session_id, run_id)
            .await?;
        let target = Self::target(session_id, run_id);
        let scope = ReplayScope::run(run_id.as_str());
        // Persist the first evidence-family decision so a later canonical event cannot reinterpret
        // a public replay-event cursor that was projected from display-message sequences.
        let replay_source = self.storage.resolve_replay_source(
            &scope,
            matches!(
                run.trigger_type.as_deref(),
                Some("rpc" | "rpc_hitl_resume" | "async_subagent_result")
            ),
        )?;
        let canonical_source = replay_source == DurableReplaySource::ReplayEvents;
        let effective_limit = limit
            .unwrap_or(DEFAULT_REPLAY_PAGE_LIMIT)
            .clamp(1, MAX_REPLAY_PAGE_LIMIT);
        let mut events = if canonical_source {
            self.storage
                .replay_event_log()
                .replay_after(&scope, cursor.clone(), Some(effective_limit))
                .await?
        } else {
            let display_cursor = cursor
                .as_ref()
                .map(|cursor| ReplayCursor::display(scope.clone(), cursor.sequence));
            self.storage
                .stream_archive()
                .replay_display_after(&scope, display_cursor)
                .await?
                .into_iter()
                .map(|message| ReplayEvent::display(scope.clone(), message))
                .collect()
        };
        let live = if canonical_source {
            self.active
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
                .unwrap_or_default()
        } else {
            Vec::new()
        };
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
        events.truncate(effective_limit);
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
        let source_run_id = session.head_run_id.clone().ok_or_else(|| {
            RpcHostError::Invalid(
                "async subagent continuation requires a current session head".to_string(),
            )
        })?;
        let source = store
            .load_run(&background.parent_session_id, &source_run_id)
            .await?;
        self.catalog.profile(&background.profile)?;
        let recorded_attachments = recorded_environment_attachments(&source);
        let environment_attachments = self
            .environment_manager
            .materialize_run_attachments(
                recorded_attachments,
                Some(background.parent_session_id.as_str()),
                None,
            )
            .await
            .map_err(|error| RpcHostError::Invalid(error.message))?;
        let (materialization, continuation) = self.materialization_plan(
            &background.profile,
            &environment_attachments,
            Some(&source),
            ContinuationMaterializationMode::Preserve,
        )?;
        let continuation = continuation.ok_or_else(|| {
            RpcHostError::Runtime(
                "missing async subagent continuation materialization plan".to_string(),
            )
        })?;
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
        run.restore_from_run_id = Some(source_run_id.clone());
        run.parent_run_id = Some(background.parent_run_id.clone());
        run.trace_context = background.trace_context.clone().unwrap_or_default();
        run.trigger_type = Some("async_subagent_result".to_string());
        run.status = RunStatus::Queued;
        run.metadata.insert(
            RPC_PROFILE_METADATA_KEY.to_string(),
            json!(background.profile),
        );
        run.metadata.insert(
            RPC_ENVIRONMENT_ATTACHMENTS_METADATA_KEY.to_string(),
            json!(safe_rpc_environment_attachments(&environment_attachments)),
        );
        materialization
            .insert_into(&mut run.metadata)
            .map_err(|error| RpcHostError::Invalid(error.to_string()))?;
        continuation
            .insert_into(&mut run.metadata)
            .map_err(|error| RpcHostError::Invalid(error.to_string()))?;
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
                source_run_id.as_str(),
                safe_rpc_environment_attachments(&environment_attachments),
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
                    replaces_waiting_run_id: None,
                    hitl_resume_claim_id: None,
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
                environment_attachments,
                idempotency_key,
                command_fingerprint,
                continuation_mode: ContinuationMaterializationMode::Preserve,
                install_session_management: true,
            },
            Some(receipt.admission),
            None,
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
        let tasks = self
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
        for mut task in tasks {
            if tokio::time::timeout_at(deadline, &mut task).await.is_err() {
                timed_out = true;
                task.abort();
                let _ = task.await;
            }
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
        let coordinator = self.clone();
        let params = params.clone();
        let params_digest = params_digest.to_string();
        tokio::spawn(async move {
            coordinator
                .active_environment_mount_owned(params, attachment, params_digest)
                .await
        })
        .await
        .map_err(|error| {
            RpcError::new(RUN_CONFLICT, format!("active mount task failed: {error}"))
        })?
    }

    #[allow(clippy::significant_drop_tightening, clippy::too_many_lines)]
    async fn active_environment_mount_owned(
        &self,
        params: EnvironmentActiveMountParams,
        attachment: EnvironmentAttachmentRef,
        params_digest: String,
    ) -> Result<RpcActiveMountOutcome, RpcError> {
        let publish_lock = {
            let registry = self
                .active
                .lock()
                .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
            Arc::clone(&active_mutable_run(&registry, &params.run_id)?.replay_publish_lock)
        };
        let _publish_guard = publish_lock.lock().await;
        let mutation_key = params
            .idempotency_key
            .as_deref()
            .map(|key| format!("mount:{key}"));
        let (
            previous_target,
            previous_attachment,
            mounted,
            target,
            previous_binding_version,
            binding_version,
            event_sequence,
            session_id,
            run_id,
            lease,
            environment,
            control,
        ) = {
            let registry = self
                .active
                .lock()
                .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
            let run = active_mutable_run(&registry, &params.run_id)?;
            if let Some(key) = mutation_key.as_ref()
                && let Some(record) = run.environment_idempotency.get(key)
            {
                ensure_idempotency_digest(record, &params_digest)?;
                return Ok(RpcActiveMountOutcome {
                    result: record.result.clone(),
                    #[cfg(test)]
                    applied: false,
                });
            }
            ensure_expected_binding(
                run.environment_binding_version,
                params.expected_binding_version,
            )?;
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
            let previous_attachment =
                existing_index.map(|index| run.environment_attachments[index].clone());
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
            let previous_target = resolve_rpc_environment_target(
                &self.config.workspace_root,
                run.lease.target.session_id.as_str(),
                &run.environment_attachments,
            )
            .map_err(RpcError::from)?;
            let target = resolve_rpc_environment_target(
                &self.config.workspace_root,
                run.lease.target.session_id.as_str(),
                &updated,
            )
            .map_err(RpcError::from)?;
            (
                previous_target,
                previous_attachment,
                mounted,
                target,
                run.environment_binding_version,
                run.environment_binding_version.saturating_add(1),
                run.next_event_sequence,
                run.lease.target.session_id.clone(),
                run.lease.target.run_id.clone(),
                run.lease.clone(),
                Arc::clone(&run.environment),
                run.control.clone(),
            )
        };
        let operation_id = format!("envop_{}", uuid::Uuid::new_v4());
        let lifecycle_event = environment_lifecycle_event(
            event_sequence,
            &session_id,
            &run_id,
            binding_version,
            &target.attachments,
            &operation_id,
            "environment_mounted",
            &json!({"action": "mounted", "mount": mount_summary(&mounted, "ready")}),
        );
        let mut result = json!({
            "runId": params.run_id,
            "operationId": operation_id,
            "mountId": mounted.id,
            "replace": params.replace,
            "mount": mount_summary(&mounted, "ready"),
            "previousBindingVersion": previous_binding_version,
            "bindingVersion": binding_version,
            "environment": environment_summary(binding_version, &target.attachments),
            "eventCursor": cursor_value(&params.run_id, lifecycle_event.sequence),
            "contextInjectionRequested": params.inject_context,
        });

        self.environment_manager.replace_run_attachment(
            &params.run_id,
            previous_attachment.as_ref(),
            Some(&mounted),
        )?;
        let replacement = SwitchableEnvironmentTarget::new(
            target.provider.clone(),
            target.provider.clone().process_shell_provider(),
        );
        if let Err(error) = environment.replace_target(replacement) {
            let manager_rollback = self.environment_manager.replace_run_attachment(
                &params.run_id,
                Some(&mounted),
                previous_attachment.as_ref(),
            );
            if let Err(rollback) = manager_rollback {
                return Err(RpcError::new(
                    RUN_CONFLICT,
                    format!("{error}; active mount lease rollback failed: {rollback:?}"),
                ));
            }
            return Err(RpcError::new(RUN_CONFLICT, error.to_string()));
        }
        if let Err(error) = self
            .storage
            .session_store()
            .append_replay_events_fenced(&lease, vec![lifecycle_event.clone()])
            .await
        {
            let rollback_target = SwitchableEnvironmentTarget::new(
                previous_target.provider.clone(),
                previous_target.provider.clone().process_shell_provider(),
            );
            let provider_rollback = environment.replace_target(rollback_target);
            let lease_rollback = self.environment_manager.replace_run_attachment(
                &params.run_id,
                Some(&mounted),
                previous_attachment.as_ref(),
            );
            if provider_rollback.is_err() || lease_rollback.is_err() {
                return Err(RpcError::new(
                    RUN_CONFLICT,
                    format!(
                        "{error}; active mount rollback failed: provider={:?}, lease={:?}",
                        provider_rollback.err(),
                        lease_rollback.err().map(|error| error.message)
                    ),
                ));
            }
            return Err(RpcError::new(RUN_CONFLICT, error.to_string()));
        }
        {
            let mut registry = self
                .active
                .lock()
                .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
            let run = active_mutable_run_mut(&mut registry, &params.run_id)?;
            debug_assert_eq!(run.environment_binding_version, previous_binding_version);
            debug_assert_eq!(run.next_event_sequence, event_sequence);
            run.environment_attachments = target.attachments;
            run.environment_binding_version = binding_version;
            run.next_event_sequence = event_sequence.saturating_add(1);
            run.terminal_replay_sequence
                .store(run.next_event_sequence, Ordering::Release);
            push_cached_event(run, lifecycle_event);
        }
        if params.inject_context {
            let injected = control
                .steer(
                    operation_id,
                    format!(
                        "Environment mount {} is now available at /environment/{}.",
                        attachment_id_from_result(&result),
                        attachment_id_from_result(&result),
                    ),
                )
                .await
                .is_ok();
            if let Some(object) = result.as_object_mut() {
                object.insert("contextInjected".to_string(), json!(injected));
            }
        }
        if let Some(key) = mutation_key {
            let mut registry = self
                .active
                .lock()
                .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
            active_mutable_run_mut(&mut registry, &params.run_id)?
                .environment_idempotency
                .insert(
                    key,
                    EnvironmentMutationRecord {
                        params_digest,
                        result: result.clone(),
                    },
                );
        }
        Ok(RpcActiveMountOutcome {
            result,
            #[cfg(test)]
            applied: true,
        })
    }

    #[allow(clippy::significant_drop_tightening, clippy::too_many_lines)]
    pub(crate) async fn active_environment_unmount(
        &self,
        params: &EnvironmentActiveUnmountParams,
        params_digest: &str,
    ) -> Result<RpcActiveUnmountOutcome, RpcError> {
        let coordinator = self.clone();
        let params = params.clone();
        let params_digest = params_digest.to_string();
        tokio::spawn(async move {
            coordinator
                .active_environment_unmount_owned(params, params_digest)
                .await
        })
        .await
        .map_err(|error| {
            RpcError::new(RUN_CONFLICT, format!("active unmount task failed: {error}"))
        })?
    }

    #[allow(clippy::significant_drop_tightening, clippy::too_many_lines)]
    async fn active_environment_unmount_owned(
        &self,
        params: EnvironmentActiveUnmountParams,
        params_digest: String,
    ) -> Result<RpcActiveUnmountOutcome, RpcError> {
        let publish_lock = {
            let registry = self
                .active
                .lock()
                .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
            Arc::clone(&active_mutable_run(&registry, &params.run_id)?.replay_publish_lock)
        };
        let _publish_guard = publish_lock.lock().await;
        let mutation_key = params
            .idempotency_key
            .as_deref()
            .map(|key| format!("unmount:{key}"));
        let (
            removed,
            mut updated,
            previous_attachments,
            previous_binding_version,
            event_sequence,
            environment,
            session_id,
            run_id,
            lease,
            control,
        ) = {
            let registry = self
                .active
                .lock()
                .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
            let run = active_mutable_run(&registry, &params.run_id)?;
            if let Some(key) = mutation_key.as_ref()
                && let Some(record) = run.environment_idempotency.get(key)
            {
                ensure_idempotency_digest(record, &params_digest)?;
                return Ok(RpcActiveUnmountOutcome {
                    result: record.result.clone(),
                    #[cfg(test)]
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
            let previous_attachments = run.environment_attachments.clone();
            let mut updated = previous_attachments.clone();
            let removed = updated.remove(index);
            apply_unmount_defaults(&removed, &mut updated, &params)?;
            (
                removed,
                updated,
                previous_attachments,
                run.environment_binding_version,
                run.next_event_sequence,
                Arc::clone(&run.environment),
                run.lease.target.session_id.clone(),
                run.lease.target.run_id.clone(),
                run.lease.clone(),
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
        let previous_target = resolve_rpc_environment_target(
            &self.config.workspace_root,
            session_id.as_str(),
            &previous_attachments,
        )
        .map_err(RpcError::from)?;
        let target = resolve_rpc_environment_target(
            &self.config.workspace_root,
            session_id.as_str(),
            &updated,
        )
        .map_err(RpcError::from)?;
        let binding_version = previous_binding_version.saturating_add(1);
        let operation_id = format!("envop_{}", uuid::Uuid::new_v4());
        let lifecycle_event = environment_lifecycle_event(
            event_sequence,
            &session_id,
            &run_id,
            binding_version,
            &target.attachments,
            &operation_id,
            "environment_unmounted",
            &json!({
                "action": "unmounted",
                "removedMount": mount_summary(&removed, "detached"),
            }),
        );
        let mut result = json!({
            "runId": params.run_id,
            "operationId": operation_id,
            "mountId": removed.id,
            "removedMount": mount_summary(&removed, "detached"),
            "previousBindingVersion": previous_binding_version,
            "bindingVersion": binding_version,
            "environment": environment_summary(binding_version, &target.attachments),
            "eventCursor": cursor_value(&params.run_id, lifecycle_event.sequence),
            "contextInjectionRequested": params.inject_context,
        });

        self.environment_manager
            .replace_run_attachment(&params.run_id, Some(&removed), None)?;
        let replacement = SwitchableEnvironmentTarget::new(
            target.provider.clone(),
            target.provider.clone().process_shell_provider(),
        );
        if let Err(error) = environment.replace_target(replacement) {
            let manager_rollback = self.environment_manager.replace_run_attachment(
                &params.run_id,
                None,
                Some(&removed),
            );
            if let Err(rollback) = manager_rollback {
                return Err(RpcError::new(
                    RUN_CONFLICT,
                    format!("{error}; active unmount lease rollback failed: {rollback:?}"),
                ));
            }
            return Err(RpcError::new(RUN_CONFLICT, error.to_string()));
        }
        if let Err(error) = self
            .storage
            .session_store()
            .append_replay_events_fenced(&lease, vec![lifecycle_event.clone()])
            .await
        {
            let rollback_target = SwitchableEnvironmentTarget::new(
                previous_target.provider.clone(),
                previous_target.provider.clone().process_shell_provider(),
            );
            let provider_rollback = environment.replace_target(rollback_target);
            let manager_rollback = self.environment_manager.replace_run_attachment(
                &params.run_id,
                None,
                Some(&removed),
            );
            if provider_rollback.is_err() || manager_rollback.is_err() {
                return Err(RpcError::new(
                    RUN_CONFLICT,
                    format!(
                        "{error}; active unmount rollback failed: provider={:?}, lease={:?}",
                        provider_rollback.err(),
                        manager_rollback.err().map(|error| error.message)
                    ),
                ));
            }
            return Err(RpcError::new(RUN_CONFLICT, error.to_string()));
        }
        {
            let mut registry = self
                .active
                .lock()
                .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
            let run = active_mutable_run_mut(&mut registry, &params.run_id)?;
            debug_assert_eq!(run.environment_binding_version, previous_binding_version);
            debug_assert_eq!(run.next_event_sequence, event_sequence);
            run.environment_attachments = target.attachments;
            run.environment_binding_version = binding_version;
            run.next_event_sequence = event_sequence.saturating_add(1);
            run.terminal_replay_sequence
                .store(run.next_event_sequence, Ordering::Release);
            push_cached_event(run, lifecycle_event);
        }
        if params.inject_context {
            let injected = control
                .steer(
                    operation_id,
                    format!("Environment mount {} was removed.", params.mount_id),
                )
                .await
                .is_ok();
            if let Some(object) = result.as_object_mut() {
                object.insert("contextInjected".to_string(), json!(injected));
            }
        }
        if let Some(key) = mutation_key {
            let mut registry = self
                .active
                .lock()
                .map_err(|error| RpcError::new(RUN_CONFLICT, error.to_string()))?;
            active_mutable_run_mut(&mut registry, &params.run_id)?
                .environment_idempotency
                .insert(
                    key,
                    EnvironmentMutationRecord {
                        params_digest,
                        result: result.clone(),
                    },
                );
        }
        Ok(RpcActiveUnmountOutcome {
            result,
            #[cfg(test)]
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

#[allow(clippy::too_many_arguments)]
fn environment_lifecycle_event(
    sequence: usize,
    session_id: &SessionId,
    run_id: &RunId,
    binding_version: u64,
    attachments: &[EnvironmentAttachmentRef],
    operation_id: &str,
    operation_kind: &str,
    extra: &Value,
) -> ReplayEvent {
    let extra = extra.as_object().cloned().unwrap_or_default();
    let lifecycle = EnvironmentLifecycleEvent {
        operation_kind: operation_kind.to_string(),
        session_id: session_id.as_str().to_string(),
        run_id: run_id.as_str().to_string(),
        binding_version,
        environment: environment_summary(binding_version, attachments),
        operation_id: Some(operation_id.to_string()),
        extra,
    };
    ReplayEvent::new(
        ReplayScope::run(run_id.as_str()),
        sequence,
        ReplayEventKind::EnvironmentLifecycle(Box::new(lifecycle)),
    )
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
        // Arbitrary attachment metadata remains provider-private until a typed safe allowlist
        // exists; lifecycle replay must not turn extension values into durable diagnostics.
        "metadata": {},
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
    store: &starweaver_storage::SqliteSessionStore,
    target: &ManagedRunTarget,
    projection_context: &DisplayProjectionContext,
    record: &AgentStreamRecord,
) {
    let mut messages = DefaultDisplayMessageProjector
        .project(projection_context, record)
        .await;
    if let Some(terminal_index) = messages
        .iter()
        .position(starweaver_stream::DisplayMessage::is_terminal)
    {
        if messages[terminal_index..]
            .iter()
            .any(|message| !message.is_terminal())
        {
            if let Ok(mut registry) = active.lock()
                && let Some(active_run) = registry.get_mut(target)
            {
                let error =
                    "display projector emitted a non-terminal message after a terminal message"
                        .to_string();
                active_run.replay_error = Some(error.clone());
                let _ = active_run.control.interrupt(Some(error));
            }
            return;
        }
        messages.truncate(terminal_index);
    }
    if messages.is_empty() {
        return;
    }
    let publish_lock = {
        let Ok(registry) = active.lock() else {
            return;
        };
        let Some(active_run) = registry.get(target) else {
            return;
        };
        Arc::clone(&active_run.replay_publish_lock)
    };
    let _publish_guard = publish_lock.lock().await;
    let scope = ReplayScope::run(target.run_id.as_str());
    let (display_sequence, event_sequence, lease) = {
        let Ok(registry) = active.lock() else {
            return;
        };
        let Some(active_run) = registry.get(target) else {
            return;
        };
        (
            active_run.next_display_sequence,
            active_run.next_event_sequence,
            active_run.lease.clone(),
        )
    };
    let mut events = Vec::with_capacity(messages.len());
    for (offset, mut message) in messages.into_iter().enumerate() {
        message.sequence = display_sequence.saturating_add(offset);
        events.push(ReplayEvent::display_at(
            scope.clone(),
            event_sequence.saturating_add(offset),
            message,
        ));
    }
    if let Err(error) = store
        .append_replay_events_fenced(&lease, events.clone())
        .await
    {
        if let Ok(mut registry) = active.lock()
            && let Some(active_run) = registry.get_mut(target)
        {
            let message = format!("failed to persist replay event batch: {error}");
            active_run.replay_error = Some(message.clone());
            let mut status = active_run.status_tx.borrow().clone();
            status.error.get_or_insert_with(|| message.clone());
            let _ = active_run.status_tx.send(status);
            let _ = active_run.control.interrupt(Some(message));
        }
        return;
    }
    if let Ok(mut registry) = active.lock()
        && let Some(active_run) = registry.get_mut(target)
    {
        active_run.next_display_sequence = display_sequence.saturating_add(events.len());
        active_run.next_event_sequence = event_sequence.saturating_add(events.len());
        active_run
            .terminal_replay_sequence
            .store(active_run.next_event_sequence, Ordering::Release);
        for event in events {
            push_cached_event(active_run, event);
        }
    }
}

fn push_cached_event(run: &mut ActiveRun, event: ReplayEvent) {
    run.events.push(event);
    let overflow = run.events.len().saturating_sub(ACTIVE_EVENT_CACHE_LIMIT);
    if overflow > 0 {
        run.events.drain(..overflow);
    }
}

async fn publish_committed_terminal_events(
    active: &Arc<Mutex<HashMap<ManagedRunTarget, ActiveRun>>>,
    replay_log: &starweaver_storage::SqliteReplayEventLog,
    target: &ManagedRunTarget,
    expected_marker: StreamTerminalMarker,
) -> Result<(), String> {
    let sequence = {
        let registry = active.lock().map_err(|error| error.to_string())?;
        registry
            .get(target)
            .map(|run| run.next_event_sequence)
            .ok_or_else(|| "active run disappeared before terminal publication".to_string())?
    };
    let scope = ReplayScope::run(target.run_id.as_str());
    let cursor = sequence
        .checked_sub(1)
        .map(|previous| ReplayCursor::replay_event(scope.clone(), previous));
    let events = replay_log
        .replay_after(&scope, cursor, Some(2))
        .await
        .map_err(|error| error.to_string())?;
    if events.len() != 2
        || events[0].sequence != sequence
        || !matches!(&events[0].event, ReplayEventKind::DisplayMessage(message) if message.is_terminal())
        || events[1].sequence != sequence.saturating_add(1)
        || !matches!(&events[1].event, ReplayEventKind::Terminal { marker } if marker == &expected_marker)
    {
        return Err(format!(
            "durable replay does not contain the expected terminal pair at sequence {sequence}"
        ));
    }
    let mut registry = active.lock().map_err(|error| error.to_string())?;
    let run = registry
        .get_mut(target)
        .ok_or_else(|| "active run disappeared after terminal publication".to_string())?;
    run.next_display_sequence = run.next_display_sequence.saturating_add(1);
    run.next_event_sequence = sequence.saturating_add(events.len());
    for event in events {
        push_cached_event(run, event);
    }
    drop(registry);
    Ok(())
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

fn recorded_environment_attachments(run: &RunRecord) -> Vec<EnvironmentAttachmentRef> {
    let recorded = run
        .metadata
        .get(RPC_ENVIRONMENT_ATTACHMENTS_METADATA_KEY)
        .cloned()
        .and_then(|value| serde_json::from_value::<Vec<EnvironmentAttachmentRef>>(value).ok())
        // Receipts written before this evidence field existed are interpreted conservatively as
        // the RPC default binding. Never substitute attachments from a retrying request.
        .unwrap_or_else(|| effective_rpc_environment_attachments(&[]));
    // Sanitize older records that may predate the provider-private/durable projection split.
    safe_rpc_environment_attachments(&recorded)
}

fn status_from_record(run: &RunRecord) -> RpcRunStatus {
    RpcRunStatus {
        session_id: run.session_id.as_str().to_string(),
        run_id: run.run_id.as_str().to_string(),
        status: durable_run_status_name(run.status).to_string(),
        output_preview: run.output_preview.clone(),
        error: None,
        continuation_effect: ContinuationEffectState::from_metadata(&run.metadata)
            .ok()
            .flatten(),
    }
}

const fn durable_run_status_name(status: RunStatus) -> &'static str {
    status.as_str()
}

const fn durability_error_is_cancelled(error: &AgentDurabilityError) -> bool {
    matches!(
        error,
        AgentDurabilityError::Agent(starweaver_runtime::AgentError::Cancelled { .. })
            | AgentDurabilityError::Stream(
                AgentStreamError::Interrupted { .. }
                    | AgentStreamError::Agent(starweaver_runtime::AgentError::Cancelled { .. })
            )
    )
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
    #![allow(
        clippy::significant_drop_tightening,
        clippy::too_many_lines,
        clippy::unwrap_used
    )]

    use super::*;
    use rusqlite::Connection;
    use starweaver_agent::{
        AgentRuntimeBuilder, DynToolset, FunctionTool, StaticToolset, TestModel, ToolContext,
        ToolResult,
    };
    use starweaver_environment::EnvironmentProvider;
    use starweaver_model::{
        ModelResponse, ModelResponsePart, ProviderPartInfo, ToolCallPart, tool_call_response,
    };
    use starweaver_rpc_core::{
        EnvironmentAttachmentAccessMode, LOCAL_ENVIRONMENT_ATTACHMENT_ID,
        LOCAL_ENVIRONMENT_ATTACHMENT_KIND,
    };
    use starweaver_session::{ApprovalStatus, DurableBackgroundSubagentDeliveryRelease};
    use starweaver_stream::AgentStreamEvent;

    // File-backed SQLite has a long latency tail under parallel Windows CI load; production
    // run-await limits remain owned by the RPC service policy.
    const TEST_RUN_COMPLETION_TIMEOUT: Duration = Duration::from_secs(30);

    async fn await_run_worker_finalized(
        coordinator: &RpcRuntimeCoordinator,
        session_id: &SessionId,
        run_id: &RunId,
    ) {
        let target = RpcRuntimeCoordinator::target(session_id, run_id);
        let completed = tokio::time::timeout(TEST_RUN_COMPLETION_TIMEOUT, async {
            loop {
                coordinator.reap_finished_tasks().await.unwrap();
                if !coordinator.tasks.lock().unwrap().contains_key(&target) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await;
        assert!(completed.is_ok(), "run worker finalizer did not complete");
    }

    fn local_attachment(id: &str, is_default: bool) -> EnvironmentAttachmentRef {
        EnvironmentAttachmentRef {
            id: id.to_string(),
            kind: LOCAL_ENVIRONMENT_ATTACHMENT_KIND.to_string(),
            mode: Some(EnvironmentAttachmentAccessMode::ReadWrite),
            is_default,
            is_default_for_shell: is_default,
            attachment_lease_id: None,
            endpoint_ref: None,
            environment_id: None,
            auth_token: None,
            metadata: serde_json::Map::new(),
        }
    }

    async fn insert_active_environment_fixture(
        coordinator: &RpcRuntimeCoordinator,
        attachments: Vec<EnvironmentAttachmentRef>,
    ) -> (SessionId, RunId, starweaver_agent::AgentStreamHandle) {
        let session = coordinator
            .storage
            .create_session(Some("default".to_string()), None)
            .unwrap();
        let run_id = RunId::new();
        let mut record = RunRecord::new(
            session.session_id.clone(),
            run_id.clone(),
            ConversationId::new(),
        );
        record.status = RunStatus::Queued;
        let store = coordinator.storage.session_store();
        let admission = store
            .acquire_run_admission(AcquireRunAdmission {
                run: record,
                namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
                host_instance_id: (*coordinator.host_instance_id).clone(),
                admission_id: format!("environment-fixture-admission-{}", run_id.as_str()),
                lease_expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
                idempotency_key: format!("environment-fixture-{}", run_id.as_str()),
                command_fingerprint: "environment-fixture-v1".to_string(),
                replaces_waiting_run_id: None,
                hitl_resume_claim_id: None,
            })
            .await
            .unwrap();
        store
            .update_run_status_fenced(&admission.lease, RunStatus::Running, None)
            .await
            .unwrap();
        let resolved = resolve_rpc_environment(
            &coordinator.config.workspace_root,
            session.session_id.as_str(),
            &attachments,
        )
        .unwrap();
        let handle = starweaver_agent::AgentBuilder::new(Arc::new(
            starweaver_agent::TestModel::with_text("fixture"),
        ))
        .build_app()
        .stream("fixture");
        let control = handle.control_handle();
        let target = RpcRuntimeCoordinator::target(&session.session_id, &run_id);
        let initial_status = RpcRunStatus {
            session_id: session.session_id.as_str().to_string(),
            run_id: run_id.as_str().to_string(),
            status: "running".to_string(),
            output_preview: None,
            error: None,
            continuation_effect: None,
        };
        let (status_tx, _) = watch::channel(initial_status);
        coordinator.active.lock().unwrap().insert(
            target,
            ActiveRun {
                status_tx,
                control,
                lease: admission.lease,
                events: Vec::new(),
                replay_publish_lock: Arc::new(AsyncMutex::new(())),
                next_display_sequence: 0,
                next_event_sequence: 0,
                terminal_replay_sequence: Arc::new(AtomicUsize::new(0)),
                replay_error: None,
                environment: resolved.switchable,
                environment_attachments: resolved.attachments,
                environment_binding_version: 1,
                environment_idempotency: HashMap::new(),
            },
        );
        (session.session_id, run_id, handle)
    }

    async fn seed_waiting_approval(
        storage: &SqliteStorage,
        session_id: SessionId,
        executions: Arc<AtomicUsize>,
    ) -> (RunId, String) {
        let executions_for_tool = Arc::clone(&executions);
        let tool = FunctionTool::new(
            "effect_once",
            Some("Test effect requiring approval".to_string()),
            json!({"type": "object"}),
            move |_context: ToolContext, _arguments: Value| {
                let executions = Arc::clone(&executions_for_tool);
                async move {
                    executions.fetch_add(1, Ordering::SeqCst);
                    Ok(ToolResult::new(json!({"executed": true})))
                }
            },
        );
        let toolset: DynToolset =
            Arc::new(StaticToolset::new("hitl-effect-once").with_tool(Arc::new(tool)));
        let mut runtime = AgentRuntimeBuilder::new(Arc::new(TestModel::with_responses(vec![
            tool_call_response("effect-call", "effect_once", json!({})),
        ])))
        .durable_session_id(session_id.clone())
        .session_store(Arc::new(storage.session_store()))
        .approval_required_tools(["effect_once"])
        .toolset(&toolset)
        .build();
        let waiting = runtime.run("request approved effect").await.unwrap();
        assert_eq!(waiting.state.status, starweaver_runtime::RunStatus::Waiting);
        let approvals = storage
            .session_store()
            .load_approvals(&session_id, &waiting.state.run_id)
            .await
            .unwrap();
        assert_eq!(approvals.len(), 1);
        assert_eq!(approvals[0].status, ApprovalStatus::Pending);
        (waiting.state.run_id, approvals[0].approval_id.clone())
    }

    fn hitl_resume_request(
        session_id: SessionId,
        source_run_id: RunId,
        idempotency_key: &str,
    ) -> RpcHitlResumeRequest {
        RpcHitlResumeRequest {
            session_id,
            source_run_id,
            profile: "default".to_string(),
            environment_attachments: Vec::new(),
            idempotency_key: idempotency_key.to_string(),
            command_fingerprint: format!("hitl-resume:{idempotency_key}:v1"),
            continuation_mode: ContinuationMaterializationMode::Switch,
            install_session_management: false,
        }
    }

    #[tokio::test]
    async fn await_terminal_prefers_durable_terminal_state_over_stale_active_watch() {
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
        let (session_id, run_id, _handle) = insert_active_environment_fixture(
            &coordinator,
            vec![local_attachment(LOCAL_ENVIRONMENT_ATTACHMENT_ID, true)],
        )
        .await;
        let target = RpcRuntimeCoordinator::target(&session_id, &run_id);
        let lease = coordinator
            .active
            .lock()
            .unwrap()
            .get(&target)
            .unwrap()
            .lease
            .clone();
        storage
            .session_store()
            .finalize_run_admission(
                &lease,
                RunStatus::Cancelled,
                Some("reconciled by another host".to_string()),
            )
            .await
            .unwrap();

        let status = coordinator
            .await_terminal(&session_id, &run_id, Some(Duration::from_secs(1)))
            .await
            .unwrap();
        assert_eq!(status.status, "cancelled");
        assert_eq!(
            status.output_preview.as_deref(),
            Some("reconciled by another host")
        );
    }

    #[tokio::test]
    async fn await_terminal_without_local_active_waits_for_durable_terminal_evidence() {
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
        let admission = storage
            .session_store()
            .acquire_run_admission(AcquireRunAdmission {
                run,
                namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
                host_instance_id: "foreign-host".to_string(),
                admission_id: "foreign-admission".to_string(),
                lease_expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
                idempotency_key: "foreign-await".to_string(),
                command_fingerprint: "foreign-await-v1".to_string(),
                replaces_waiting_run_id: None,
                hitl_resume_claim_id: None,
            })
            .await
            .unwrap();
        let session_id = admission.lease.target.session_id.clone();
        let run_id = admission.lease.target.run_id.clone();
        let coordinator = Arc::new(RpcRuntimeCoordinator::new(
            config.clone(),
            RpcAgentCatalog::new(config).unwrap(),
            storage.clone(),
            EnvironmentAttachmentManager::new(),
        ));

        let timeout = match coordinator
            .await_terminal(&session_id, &run_id, Some(Duration::from_millis(20)))
            .await
        {
            Ok(status) => {
                panic!("a non-terminal foreign run must not be returned by run.await: {status:?}")
            }
            Err(error) => error,
        };
        assert!(timeout.to_string().contains("run.await timed out"));

        let awaiting = {
            let coordinator = Arc::clone(&coordinator);
            let session_id = session_id.clone();
            let run_id = run_id.clone();
            tokio::spawn(async move {
                coordinator
                    .await_terminal(&session_id, &run_id, Some(Duration::from_millis(500)))
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        storage
            .session_store()
            .finalize_run_admission(
                &admission.lease,
                RunStatus::Cancelled,
                Some("foreign host completed cancellation".to_string()),
            )
            .await
            .unwrap();
        let terminal = tokio::time::timeout(Duration::from_secs(1), awaiting)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(terminal.status, "cancelled");
        assert_eq!(
            terminal.output_preview.as_deref(),
            Some("foreign host completed cancellation")
        );
    }

    #[tokio::test]
    async fn await_terminal_after_local_watcher_closes_waits_for_durable_terminal_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        let storage = SqliteStorage::open(&config.database_path).unwrap();
        let coordinator = Arc::new(RpcRuntimeCoordinator::new(
            config.clone(),
            RpcAgentCatalog::new(config).unwrap(),
            storage,
            EnvironmentAttachmentManager::new(),
        ));
        let (session_id, run_id, _handle) = insert_active_environment_fixture(
            &coordinator,
            vec![local_attachment(LOCAL_ENVIRONMENT_ATTACHMENT_ID, true)],
        )
        .await;
        let target = RpcRuntimeCoordinator::target(&session_id, &run_id);
        let awaiting = {
            let coordinator = Arc::clone(&coordinator);
            let session_id = session_id.clone();
            let run_id = run_id.clone();
            tokio::spawn(async move {
                coordinator
                    .await_terminal(&session_id, &run_id, Some(Duration::from_millis(50)))
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let subscribed = coordinator
                    .active
                    .lock()
                    .unwrap()
                    .get(&target)
                    .is_some_and(|run| run.status_tx.receiver_count() > 0);
                if subscribed {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        coordinator.active.lock().unwrap().remove(&target);

        let timeout = match awaiting.await.unwrap() {
            Ok(status) => panic!(
                "a non-terminal run with a closed local watcher must not be returned by run.await: {status:?}"
            ),
            Err(error) => error,
        };
        assert!(timeout.to_string().contains("run.await timed out"));
    }

    #[tokio::test]
    async fn await_terminal_requires_durable_evidence_for_local_terminal_projection() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        let storage = SqliteStorage::open(&config.database_path).unwrap();
        let coordinator = RpcRuntimeCoordinator::new(
            config.clone(),
            RpcAgentCatalog::new(config).unwrap(),
            storage,
            EnvironmentAttachmentManager::new(),
        );
        let (session_id, run_id, _handle) = insert_active_environment_fixture(
            &coordinator,
            vec![local_attachment(LOCAL_ENVIRONMENT_ATTACHMENT_ID, true)],
        )
        .await;
        let target = RpcRuntimeCoordinator::target(&session_id, &run_id);
        let sender = coordinator
            .active
            .lock()
            .unwrap()
            .get(&target)
            .unwrap()
            .status_tx
            .clone();
        let mut provisional = sender.borrow().clone();
        provisional.status = "failed".to_string();
        provisional.error = Some("local finalizer has not committed durable evidence".to_string());
        sender.send_replace(provisional);

        let timeout = match coordinator
            .await_terminal(&session_id, &run_id, Some(Duration::from_millis(50)))
            .await
        {
            Ok(status) => panic!(
                "a process-local terminal projection without durable evidence must not complete run.await: {status:?}"
            ),
            Err(error) => error,
        };
        assert!(timeout.to_string().contains("run.await timed out"));
    }

    #[tokio::test]
    async fn hitl_preflight_does_not_claim_and_denied_resume_terminalizes_atomically() {
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
        let session_id = SessionId::from_string("rpc-hitl-denied-session");
        let executions = Arc::new(AtomicUsize::new(0));
        let (source_run_id, approval_id) = Box::pin(seed_waiting_approval(
            &storage,
            session_id.clone(),
            Arc::clone(&executions),
        ))
        .await;
        let request =
            hitl_resume_request(session_id.clone(), source_run_id.clone(), "rpc-hitl-denied");

        let mut preserve = request.clone();
        preserve.continuation_mode = ContinuationMaterializationMode::Preserve;
        let materialization_error = coordinator.resume_waiting(preserve).await.unwrap_err();
        assert!(
            materialization_error
                .to_string()
                .contains("missing terminal durable approval decision"),
            "{materialization_error}"
        );
        let store = storage.session_store();
        let preclaim_probe = "materialization-preflight-remained-unclaimed";
        store
            .claim_hitl_resume(HitlResumeClaim::new(
                preclaim_probe.to_string(),
                session_id.clone(),
                source_run_id.clone(),
                chrono::Utc::now(),
            ))
            .await
            .unwrap();
        store
            .release_hitl_resume_claim(&session_id, &source_run_id, preclaim_probe)
            .await
            .unwrap();

        let error = coordinator
            .resume_waiting(request.clone())
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("missing terminal durable approval decision"),
            "{error}"
        );
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        let store = storage.session_store();
        assert_eq!(store.list_runs(&session_id).await.unwrap().len(), 1);
        let probe_claim_id = "preflight-remained-unclaimed";
        store
            .claim_hitl_resume(HitlResumeClaim::new(
                probe_claim_id.to_string(),
                session_id.clone(),
                source_run_id.clone(),
                chrono::Utc::now(),
            ))
            .await
            .unwrap();
        store
            .release_hitl_resume_claim(&session_id, &source_run_id, probe_claim_id)
            .await
            .unwrap();

        storage
            .decide_approval(
                &approval_id,
                ApprovalStatus::Denied,
                Some("rpc-user".to_string()),
                Some("not authorized".to_string()),
            )
            .unwrap();
        let started = coordinator.resume_waiting(request.clone()).await.unwrap();
        let terminal = coordinator
            .await_terminal(
                &started.session_id,
                &started.run_id,
                Some(TEST_RUN_COMPLETION_TIMEOUT),
            )
            .await
            .unwrap();
        assert_eq!(terminal.status, "completed", "{terminal:?}");
        assert_eq!(executions.load(Ordering::SeqCst), 0);

        let source = store.load_run(&session_id, &source_run_id).await.unwrap();
        let continuation = store.load_run(&session_id, &started.run_id).await.unwrap();
        assert_eq!(source.status, RunStatus::Completed);
        assert_eq!(continuation.status, RunStatus::Completed);
        assert_eq!(
            continuation.restore_from_run_id.as_ref(),
            Some(&source_run_id)
        );
        let resolved = store
            .load_approvals(&session_id, &source_run_id)
            .await
            .unwrap();
        assert_eq!(resolved[0].status, ApprovalStatus::Denied);
        assert!(
            store
                .release_hitl_resume_claim(
                    &session_id,
                    &source_run_id,
                    "preflight-remained-unclaimed",
                )
                .await
                .is_ok(),
            "consumed claims are absent and cannot re-enable an effect"
        );

        let replay = coordinator.resume_waiting(request).await.unwrap();
        assert!(replay.idempotent_replay);
        assert_eq!(replay.run_id, started.run_id);
        assert_eq!(replay.status, RunStatus::Completed);
    }

    #[tokio::test]
    async fn admitted_hitl_preparation_failure_aborts_only_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        let storage = SqliteStorage::open(&config.database_path).unwrap();
        let session_id = SessionId::from_string("rpc-hitl-preparation-failure-session");
        let executions = Arc::new(AtomicUsize::new(0));
        let (source_run_id, approval_id) = Box::pin(seed_waiting_approval(
            &storage,
            session_id.clone(),
            Arc::clone(&executions),
        ))
        .await;
        storage
            .decide_approval(
                &approval_id,
                ApprovalStatus::Approved,
                Some("rpc-user".to_string()),
                None,
            )
            .unwrap();

        let materializations = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&materializations);
        let factory: Arc<crate::agent_catalog::TestRuntimeFactory> = Arc::new(move |_profile| {
            if calls.fetch_add(1, Ordering::SeqCst) > 0 {
                return Err(RpcHostError::Runtime(
                    "injected second materialization failure".to_string(),
                ));
            }
            let tool = FunctionTool::new(
                "effect_once",
                Some("Test effect requiring approval".to_string()),
                json!({"type": "object"}),
                |_context: ToolContext, _arguments: Value| async move {
                    Ok(ToolResult::new(json!({"executed": true})))
                },
            );
            let toolset: DynToolset =
                Arc::new(StaticToolset::new("hitl-effect-once").with_tool(Arc::new(tool)));
            Ok(
                AgentRuntimeBuilder::new(Arc::new(TestModel::with_text("unused")))
                    .approval_required_tools(["effect_once"])
                    .toolset(&toolset),
            )
        });
        let catalog = RpcAgentCatalog::new(config.clone())
            .unwrap()
            .with_test_runtime_factory(factory);
        let coordinator = RpcRuntimeCoordinator::new(
            config,
            catalog,
            storage.clone(),
            EnvironmentAttachmentManager::new(),
        );
        let request = hitl_resume_request(
            session_id.clone(),
            source_run_id.clone(),
            "rpc-hitl-preparation-failure",
        );
        let error = coordinator
            .resume_waiting(request.clone())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("second materialization failure"),
            "{error}"
        );
        assert!(!error.to_string().contains("failed to persist"), "{error}");
        assert_eq!(materializations.load(Ordering::SeqCst), 2);
        assert_eq!(executions.load(Ordering::SeqCst), 0);

        let receipt = storage
            .session_store()
            .load_run_admission_receipt(
                LOCAL_SESSION_NAMESPACE,
                &request.idempotency_key,
                &request.command_fingerprint,
            )
            .await
            .unwrap()
            .unwrap();
        let store = storage.session_store();
        assert_eq!(
            store
                .load_run(&session_id, &source_run_id)
                .await
                .unwrap()
                .status,
            RunStatus::Waiting
        );
        assert_eq!(
            store
                .load_run(&session_id, &receipt.run.run_id)
                .await
                .unwrap()
                .status,
            RunStatus::Failed
        );
        assert_eq!(
            store.load_session(&session_id).await.unwrap().active_run_id,
            None,
            "an aborted admitted replacement must not leave the session wedged on its failed run"
        );
        assert!(
            store
                .load_run_admission(&receipt.lease.target)
                .await
                .unwrap()
                .is_none(),
            "terminal failure evidence must release the active admission"
        );
    }

    #[tokio::test]
    async fn started_hitl_commit_failure_never_finalizes_continuation_without_source_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        let storage = SqliteStorage::open(&config.database_path).unwrap();
        let session_id = SessionId::from_string("rpc-hitl-atomic-failure-session");
        let executions = Arc::new(AtomicUsize::new(0));
        let (source_run_id, approval_id) = Box::pin(seed_waiting_approval(
            &storage,
            session_id.clone(),
            Arc::clone(&executions),
        ))
        .await;
        storage
            .decide_approval(
                &approval_id,
                ApprovalStatus::Approved,
                Some("rpc-user".to_string()),
                None,
            )
            .unwrap();

        let materializations = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&materializations);
        let factory: Arc<crate::agent_catalog::TestRuntimeFactory> = Arc::new(move |_profile| {
            if calls.fetch_add(1, Ordering::SeqCst) > 0 {
                return Err(RpcHostError::Runtime(
                    "injected post-admission materialization failure".to_string(),
                ));
            }
            let tool = FunctionTool::new(
                "effect_once",
                Some("Test effect requiring approval".to_string()),
                json!({"type": "object"}),
                |_context: ToolContext, _arguments: Value| async move {
                    Ok(ToolResult::new(json!({"executed": true})))
                },
            );
            let toolset: DynToolset =
                Arc::new(StaticToolset::new("hitl-atomic-failure").with_tool(Arc::new(tool)));
            Ok(
                AgentRuntimeBuilder::new(Arc::new(TestModel::with_text("unused")))
                    .approval_required_tools(["effect_once"])
                    .toolset(&toolset),
            )
        });
        let catalog = RpcAgentCatalog::new(config.clone())
            .unwrap()
            .with_test_runtime_factory(factory);
        let coordinator = RpcRuntimeCoordinator::new(
            config,
            catalog,
            storage.clone(),
            EnvironmentAttachmentManager::new(),
        );
        let request = hitl_resume_request(
            session_id.clone(),
            source_run_id.clone(),
            "rpc-hitl-atomic-failure",
        );
        let error = coordinator
            .resume_waiting(request.clone())
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("injected post-admission materialization failure"),
            "{error}"
        );
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        let store = storage.session_store();
        let receipt = store
            .load_run_admission_receipt(
                LOCAL_SESSION_NAMESPACE,
                &request.idempotency_key,
                &request.command_fingerprint,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            store
                .load_run(&session_id, &source_run_id)
                .await
                .unwrap()
                .status,
            RunStatus::Waiting,
            "an admitted failure must leave the source retryable"
        );
        assert_eq!(
            store
                .load_run(&session_id, &receipt.run.run_id)
                .await
                .unwrap()
                .status,
            RunStatus::Failed
        );
        assert!(
            store
                .load_run_admission(&receipt.lease.target)
                .await
                .unwrap()
                .is_none(),
            "the admitted-only failure must release its replacement lease"
        );
    }

    #[tokio::test]
    async fn exact_hitl_retry_survives_profile_removal_after_restart() {
        let temp = tempfile::tempdir().unwrap();
        let mut initial_config = RpcConfig::for_tests(temp.path());
        let mut retired = initial_config.profiles["default"].clone();
        retired.model_id = "test:retired-hitl".to_string();
        retired.test_response = Some("resumed".to_string());
        initial_config
            .profiles
            .insert("retired".to_string(), retired);
        let mut restarted_config = initial_config.clone();
        restarted_config.profiles.remove("retired");
        std::fs::create_dir_all(&initial_config.workspace_root).unwrap();
        let storage = SqliteStorage::open(&initial_config.database_path).unwrap();
        let session_id = SessionId::from_string("rpc-hitl-retired-profile-session");
        let executions = Arc::new(AtomicUsize::new(0));
        let (source_run_id, approval_id) = Box::pin(seed_waiting_approval(
            &storage,
            session_id.clone(),
            Arc::clone(&executions),
        ))
        .await;
        storage
            .decide_approval(
                &approval_id,
                ApprovalStatus::Approved,
                Some("rpc-user".to_string()),
                None,
            )
            .unwrap();

        let executions_for_factory = Arc::clone(&executions);
        let factory: Arc<crate::agent_catalog::TestRuntimeFactory> = Arc::new(move |_profile| {
            let executions_for_tool = Arc::clone(&executions_for_factory);
            let tool = FunctionTool::new(
                "effect_once",
                Some("Test effect requiring approval".to_string()),
                json!({"type": "object"}),
                move |_context: ToolContext, _arguments: Value| {
                    let executions = Arc::clone(&executions_for_tool);
                    async move {
                        executions.fetch_add(1, Ordering::SeqCst);
                        Ok(ToolResult::new(json!({"executed": true})))
                    }
                },
            );
            let toolset: DynToolset =
                Arc::new(StaticToolset::new("hitl-retired-profile").with_tool(Arc::new(tool)));
            Ok(
                AgentRuntimeBuilder::new(Arc::new(TestModel::with_text("resumed")))
                    .approval_required_tools(["effect_once"])
                    .toolset(&toolset),
            )
        });
        let catalog = RpcAgentCatalog::new(initial_config.clone())
            .unwrap()
            .with_test_runtime_factory(factory);
        let coordinator = RpcRuntimeCoordinator::new(
            initial_config,
            catalog,
            storage.clone(),
            EnvironmentAttachmentManager::new(),
        );
        let mut request = hitl_resume_request(
            session_id.clone(),
            source_run_id,
            "rpc-hitl-retired-profile",
        );
        request.profile = "retired".to_string();
        request.command_fingerprint = "hitl-resume:retired-profile:v1".to_string();
        let first = coordinator.resume_waiting(request.clone()).await.unwrap();
        let terminal = coordinator
            .await_terminal(
                &first.session_id,
                &first.run_id,
                Some(TEST_RUN_COMPLETION_TIMEOUT),
            )
            .await
            .unwrap();
        assert_eq!(terminal.status, "completed");
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        coordinator.shutdown(Duration::from_secs(5)).await.unwrap();
        drop(coordinator);

        let restarted_catalog = RpcAgentCatalog::new(restarted_config.clone()).unwrap();
        assert!(restarted_catalog.profile("retired").is_err());
        let restarted = RpcRuntimeCoordinator::new(
            restarted_config,
            restarted_catalog,
            storage,
            EnvironmentAttachmentManager::new(),
        );
        let replay = restarted.resume_waiting(request).await.unwrap();
        assert!(replay.idempotent_replay);
        assert_eq!(replay.run_id, first.run_id);
        assert_eq!(replay.status, RunStatus::Completed);
        assert_eq!(executions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concurrent_exact_hitl_resume_executes_approved_tool_once() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        let storage = SqliteStorage::open(&config.database_path).unwrap();
        let session_id = SessionId::from_string("rpc-hitl-effect-once-session");
        let executions = Arc::new(AtomicUsize::new(0));
        let (source_run_id, approval_id) = Box::pin(seed_waiting_approval(
            &storage,
            session_id.clone(),
            Arc::clone(&executions),
        ))
        .await;
        storage
            .decide_approval(
                &approval_id,
                ApprovalStatus::Approved,
                Some("rpc-user".to_string()),
                None,
            )
            .unwrap();

        let executions_for_factory = Arc::clone(&executions);
        let factory: Arc<crate::agent_catalog::TestRuntimeFactory> = Arc::new(move |_profile| {
            let executions_for_tool = Arc::clone(&executions_for_factory);
            let tool = FunctionTool::new(
                "effect_once",
                Some("Test effect requiring approval".to_string()),
                json!({"type": "object"}),
                move |_context: ToolContext, _arguments: Value| {
                    let executions = Arc::clone(&executions_for_tool);
                    async move {
                        executions.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        Ok(ToolResult::new(json!({"executed": true})))
                    }
                },
            );
            let toolset: DynToolset =
                Arc::new(StaticToolset::new("hitl-effect-once").with_tool(Arc::new(tool)));
            Ok(
                AgentRuntimeBuilder::new(Arc::new(TestModel::with_text("resumed")))
                    .approval_required_tools(["effect_once"])
                    .toolset(&toolset),
            )
        });
        let catalog = RpcAgentCatalog::new(config.clone())
            .unwrap()
            .with_test_runtime_factory(factory);
        let coordinator = RpcRuntimeCoordinator::new(
            config,
            catalog,
            storage.clone(),
            EnvironmentAttachmentManager::new(),
        );
        let request = hitl_resume_request(
            session_id.clone(),
            source_run_id.clone(),
            "rpc-hitl-effect-once",
        );
        // Simulate a host stopping after the durable Preflight claim commit but before admission.
        // Retrying must perform a read-only receipt lookup, complete the real preflight, and then
        // consume this exact claim into one continuation instead of admitting a probe run.
        let identity = command_fingerprint(
            "rpc_hitl_resume_identity",
            &json!({
                "sessionId": session_id,
                "sourceRunId": source_run_id,
                "idempotencyKey": request.idempotency_key,
                "commandFingerprint": request.command_fingerprint,
            }),
        )
        .unwrap();
        let identity_suffix = identity.rsplit(':').next().unwrap();
        storage
            .session_store()
            .claim_hitl_resume(HitlResumeClaim::new(
                format!("rpc-hitl-claim-{identity_suffix}"),
                session_id.clone(),
                source_run_id.clone(),
                chrono::Utc::now(),
            ))
            .await
            .unwrap();

        let mut drifted = request.clone();
        drifted.command_fingerprint = format!("{}-drifted", request.command_fingerprint);
        let drifted_result = coordinator.resume_waiting(drifted).await;
        assert!(
            drifted_result.is_err(),
            "changed command must not consume an orphaned preflight claim"
        );
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        assert_eq!(
            storage
                .session_store()
                .load_run(&session_id, &source_run_id)
                .await
                .unwrap()
                .status,
            RunStatus::Waiting
        );

        let (first, second) = tokio::join!(
            coordinator.resume_waiting(request.clone()),
            coordinator.resume_waiting(request.clone()),
        );
        let first = first.unwrap();
        let second = second.unwrap();
        assert_eq!(first.run_id, second.run_id);
        assert_eq!(first.admission_id, second.admission_id);
        assert_ne!(first.idempotent_replay, second.idempotent_replay);

        // A Desktop client can subscribe immediately after `run.resume` returns, before the
        // continuation publishes its first event. The run producer identity must select the
        // canonical replay family rather than pinning the empty run to display messages.
        coordinator
            .replay(&first.session_id, &first.run_id, None, Some(32))
            .await
            .unwrap();
        assert_eq!(
            coordinator
                .storage
                .resolve_replay_source(&ReplayScope::run(first.run_id.as_str()), false)
                .unwrap(),
            DurableReplaySource::ReplayEvents
        );

        let terminal = coordinator
            .await_terminal(
                &first.session_id,
                &first.run_id,
                Some(TEST_RUN_COMPLETION_TIMEOUT),
            )
            .await
            .unwrap();
        assert_eq!(terminal.status, "completed", "{terminal:?}");
        assert_eq!(executions.load(Ordering::SeqCst), 1);

        let replay = coordinator.resume_waiting(request).await.unwrap();
        assert!(replay.idempotent_replay);
        assert_eq!(replay.run_id, first.run_id);
        assert_eq!(replay.status, RunStatus::Completed);
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        let store = storage.session_store();
        assert_eq!(
            store
                .load_run(&session_id, &source_run_id)
                .await
                .unwrap()
                .status,
            RunStatus::Completed
        );
        assert_eq!(
            store
                .load_approvals(&session_id, &source_run_id)
                .await
                .unwrap()[0]
                .status,
            ApprovalStatus::Approved
        );
    }

    #[tokio::test]
    async fn run_start_persists_and_replays_only_safe_environment_attachments() {
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
        let private_program = std::env::current_exe().unwrap();
        let encoded_program = private_program
            .to_string_lossy()
            .bytes()
            .map(|byte| {
                if byte.is_ascii_alphanumeric()
                    || matches!(byte, b'/' | b':' | b'\\' | b'.' | b'-' | b'_')
                {
                    char::from(byte).to_string()
                } else {
                    format!("%{byte:02X}")
                }
            })
            .collect::<String>();
        let private_endpoint = format!("stdio://{encoded_program}?arg=--help&arg=private-value");
        let request = RpcRunRequest {
            durable_input: vec![InputPart::text("safe environment evidence")],
            input: AgentInput::text("safe environment evidence"),
            session_id: None,
            restore_from_run_id: None,
            profile: "default".to_string(),
            environment_attachments: vec![EnvironmentAttachmentRef {
                id: "workspace".to_string(),
                kind: "envd".to_string(),
                mode: Some(EnvironmentAttachmentAccessMode::ReadWrite),
                is_default: true,
                is_default_for_shell: true,
                attachment_lease_id: None,
                endpoint_ref: Some(private_endpoint),
                environment_id: Some("environment-safe-id".to_string()),
                auth_token: Some("private-bearer".to_string()),
                metadata: serde_json::Map::from_iter([(
                    "private".to_string(),
                    json!("private-metadata"),
                )]),
            }],
            idempotency_key: "safe-environment-start".to_string(),
            command_fingerprint: "safe-environment-start-v1".to_string(),
            continuation_mode: ContinuationMaterializationMode::Preserve,
            install_session_management: false,
        };

        let started = coordinator.start(request.clone()).await.unwrap();
        assert_eq!(
            started.environment_attachments[0].endpoint_ref.as_deref(),
            Some("stdio://<redacted>")
        );
        assert!(started.environment_attachments[0].auth_token.is_none());
        assert!(started.environment_attachments[0].metadata.is_empty());
        let durable = storage
            .session_store()
            .load_run(&started.session_id, &started.run_id)
            .await
            .unwrap();
        let recorded = durable
            .metadata
            .get(RPC_ENVIRONMENT_ATTACHMENTS_METADATA_KEY)
            .unwrap();
        let encoded = recorded.to_string();
        let private_program = private_program.to_string_lossy();
        for private in [
            private_program.as_ref(),
            "private-value",
            "private-bearer",
            "private-metadata",
        ] {
            assert!(
                !encoded.contains(private),
                "durable metadata leaked {private}"
            );
        }

        let mut retry = request;
        retry.environment_attachments = vec![local_attachment("retry-must-not-win", true)];
        let replay = coordinator.start(retry).await.unwrap();
        assert!(replay.idempotent_replay);
        assert_eq!(replay.run_id, started.run_id);
        assert_eq!(
            replay.environment_attachments,
            started.environment_attachments
        );
        assert!(
            !serde_json::to_string(&replay.environment_attachments)
                .unwrap()
                .contains("private")
        );
    }

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
                continuation_mode: ContinuationMaterializationMode::Preserve,
                install_session_management: false,
            })
            .await
            .unwrap();
        let status = coordinator
            .await_terminal(
                &started.session_id,
                &started.run_id,
                Some(TEST_RUN_COMPLETION_TIMEOUT),
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
            environment_attachments: vec![local_attachment("workspace", true)],
            idempotency_key: "same-start".to_string(),
            command_fingerprint: "same-fingerprint".to_string(),
            continuation_mode: ContinuationMaterializationMode::Preserve,
            install_session_management: false,
        };
        let first = coordinator.start(request.clone()).await.unwrap();
        coordinator
            .await_terminal(
                &first.session_id,
                &first.run_id,
                Some(TEST_RUN_COMPLETION_TIMEOUT),
            )
            .await
            .unwrap();
        let replay = coordinator
            .start(RpcRunRequest {
                // Even a caller that bypasses wire-level fingerprinting cannot replace facts in
                // an exact durable receipt with values from the retrying request.
                environment_attachments: vec![local_attachment("data", true)],
                ..request.clone()
            })
            .await
            .unwrap();
        assert_eq!(replay.session_id, first.session_id);
        assert_eq!(replay.run_id, first.run_id);
        assert_eq!(replay.admission_id, first.admission_id);
        assert!(replay.idempotent_replay);
        assert_eq!(replay.status, RunStatus::Completed);
        assert_eq!(
            replay.environment_attachments,
            first.environment_attachments
        );
        assert_eq!(replay.environment_attachments[0].id, "workspace");

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
                continuation_mode: ContinuationMaterializationMode::Preserve,
                install_session_management: false,
            })
            .await
            .unwrap();
        coordinator
            .await_terminal(
                &parent.session_id,
                &parent.run_id,
                Some(TEST_RUN_COMPLETION_TIMEOUT),
            )
            .await
            .unwrap();

        let store = storage.session_store();
        let parent_before = store
            .load_run(&parent.session_id, &parent.run_id)
            .await
            .unwrap();
        await_run_worker_finalized(&coordinator, &parent.session_id, &parent.run_id).await;
        let intervening = coordinator
            .start(RpcRunRequest {
                durable_input: vec![InputPart::text("intervening input")],
                input: AgentInput::text("intervening input"),
                session_id: Some(parent.session_id.clone()),
                restore_from_run_id: Some(parent.run_id.clone()),
                profile: "default".to_string(),
                environment_attachments: vec![local_attachment("workspace", true)],
                idempotency_key: "background-intervening".to_string(),
                command_fingerprint: "background-intervening-v1".to_string(),
                continuation_mode: ContinuationMaterializationMode::Preserve,
                install_session_management: false,
            })
            .await
            .unwrap();
        coordinator
            .await_terminal(
                &intervening.session_id,
                &intervening.run_id,
                Some(TEST_RUN_COMPLETION_TIMEOUT),
            )
            .await
            .unwrap();
        await_run_worker_finalized(&coordinator, &intervening.session_id, &intervening.run_id)
            .await;
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
                Some(TEST_RUN_COMPLETION_TIMEOUT),
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
        assert_eq!(continuation.environment_attachments[0].id, "workspace");
        assert_eq!(
            recorded_environment_attachments(&continuation_record)[0].id,
            "workspace"
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
                replaces_waiting_run_id: None,
                hitl_resume_claim_id: None,
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
                continuation_mode: ContinuationMaterializationMode::Preserve,
                install_session_management: false,
            })
            .await
            .unwrap();
        coordinator
            .await_terminal(
                &explicit.session_id,
                &explicit.run_id,
                Some(TEST_RUN_COMPLETION_TIMEOUT),
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
    async fn active_mount_replay_failure_leaves_binding_and_idempotency_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        std::fs::write(
            config.workspace_root.join("environment-fixture.txt"),
            "before",
        )
        .unwrap();
        let storage = SqliteStorage::open(&config.database_path).unwrap();
        let coordinator = RpcRuntimeCoordinator::new(
            config.clone(),
            RpcAgentCatalog::new(config.clone()).unwrap(),
            storage,
            EnvironmentAttachmentManager::new(),
        );
        let (_session_id, run_id, _handle) = insert_active_environment_fixture(
            &coordinator,
            vec![local_attachment(LOCAL_ENVIRONMENT_ATTACHMENT_ID, true)],
        )
        .await;
        let before = coordinator
            .active_environment_list(run_id.as_str())
            .unwrap();
        let connection = Connection::open(&config.database_path).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_environment_mount_replay
                 BEFORE INSERT ON replay_events
                 BEGIN
                   SELECT RAISE(ABORT, 'injected active mount replay failure');
                 END;",
            )
            .unwrap();
        drop(connection);

        let attachment = local_attachment("extra", false);
        let params = EnvironmentActiveMountParams {
            run_id: run_id.as_str().to_string(),
            attachment: attachment.clone(),
            replace: false,
            inject_context: false,
            expected_binding_version: Some(1),
            idempotency_key: Some("mount-once".to_string()),
        };
        let error = coordinator
            .active_environment_mount(&params, attachment.clone(), "mount-digest")
            .await
            .unwrap_err();
        assert!(
            error
                .message
                .contains("injected active mount replay failure")
        );
        assert_eq!(
            coordinator
                .active_environment_list(run_id.as_str())
                .unwrap(),
            before
        );
        let environment = {
            let registry = coordinator.active.lock().unwrap();
            let run = active_mutable_run(&registry, run_id.as_str()).unwrap();
            assert_eq!(run.environment_binding_version, 1);
            assert_eq!(run.next_event_sequence, 0);
            assert_eq!(run.terminal_replay_sequence.load(Ordering::Acquire), 0);
            assert!(run.events.is_empty());
            assert!(run.environment_idempotency.is_empty());
            Arc::clone(&run.environment)
        };
        assert_eq!(
            environment
                .read_text("/environment/local/environment-fixture.txt")
                .await
                .unwrap(),
            "before"
        );
        assert!(
            environment
                .read_text("/environment/extra/environment-fixture.txt")
                .await
                .is_err()
        );

        let connection = Connection::open(&config.database_path).unwrap();
        connection
            .execute_batch("DROP TRIGGER fail_environment_mount_replay;")
            .unwrap();
        drop(connection);
        let applied = coordinator
            .active_environment_mount(&params, attachment.clone(), "mount-digest")
            .await
            .unwrap();
        assert!(applied.applied);
        let replayed = coordinator
            .active_environment_mount(&params, attachment, "mount-digest")
            .await
            .unwrap();
        assert!(!replayed.applied);
        assert_eq!(replayed.result, applied.result);
    }

    #[tokio::test]
    async fn active_unmount_replay_failure_leaves_binding_and_idempotency_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        std::fs::write(
            config.workspace_root.join("environment-fixture.txt"),
            "before",
        )
        .unwrap();
        let storage = SqliteStorage::open(&config.database_path).unwrap();
        let coordinator = RpcRuntimeCoordinator::new(
            config.clone(),
            RpcAgentCatalog::new(config.clone()).unwrap(),
            storage,
            EnvironmentAttachmentManager::new(),
        );
        let (_session_id, run_id, _handle) = insert_active_environment_fixture(
            &coordinator,
            vec![
                local_attachment(LOCAL_ENVIRONMENT_ATTACHMENT_ID, true),
                local_attachment("extra", false),
            ],
        )
        .await;
        let before = coordinator
            .active_environment_list(run_id.as_str())
            .unwrap();
        let connection = Connection::open(&config.database_path).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_environment_unmount_replay
                 BEFORE INSERT ON replay_events
                 BEGIN
                   SELECT RAISE(ABORT, 'injected active unmount replay failure');
                 END;",
            )
            .unwrap();
        drop(connection);

        let params = EnvironmentActiveUnmountParams {
            run_id: run_id.as_str().to_string(),
            mount_id: "extra".to_string(),
            new_default_mount_id: None,
            new_default_shell_mount_id: None,
            inject_context: false,
            expected_binding_version: Some(1),
            idempotency_key: Some("unmount-once".to_string()),
        };
        let error = coordinator
            .active_environment_unmount(&params, "unmount-digest")
            .await
            .unwrap_err();
        assert!(
            error
                .message
                .contains("injected active unmount replay failure")
        );
        assert_eq!(
            coordinator
                .active_environment_list(run_id.as_str())
                .unwrap(),
            before
        );
        let environment = {
            let registry = coordinator.active.lock().unwrap();
            let run = active_mutable_run(&registry, run_id.as_str()).unwrap();
            assert_eq!(run.environment_binding_version, 1);
            assert_eq!(run.next_event_sequence, 0);
            assert_eq!(run.terminal_replay_sequence.load(Ordering::Acquire), 0);
            assert!(run.events.is_empty());
            assert!(run.environment_idempotency.is_empty());
            Arc::clone(&run.environment)
        };
        assert_eq!(
            environment
                .read_text("/environment/extra/environment-fixture.txt")
                .await
                .unwrap(),
            "before"
        );

        let connection = Connection::open(&config.database_path).unwrap();
        connection
            .execute_batch("DROP TRIGGER fail_environment_unmount_replay;")
            .unwrap();
        drop(connection);
        let applied = coordinator
            .active_environment_unmount(&params, "unmount-digest")
            .await
            .unwrap();
        assert!(applied.applied);
        let replayed = coordinator
            .active_environment_unmount(&params, "unmount-digest")
            .await
            .unwrap();
        assert!(!replayed.applied);
        assert_eq!(replayed.result, applied.result);
        let environment = {
            let registry = coordinator.active.lock().unwrap();
            let run = active_mutable_run(&registry, run_id.as_str()).unwrap();
            Arc::clone(&run.environment)
        };
        assert!(
            environment
                .read_text("/environment/extra/environment-fixture.txt")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn stale_environment_owner_cannot_publish_or_change_binding() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        std::fs::write(
            config.workspace_root.join("environment-fixture.txt"),
            "before",
        )
        .unwrap();
        let storage = SqliteStorage::open(&config.database_path).unwrap();
        let coordinator = RpcRuntimeCoordinator::new(
            config.clone(),
            RpcAgentCatalog::new(config.clone()).unwrap(),
            storage,
            EnvironmentAttachmentManager::new(),
        );
        let (session_id, run_id, _handle) = insert_active_environment_fixture(
            &coordinator,
            vec![local_attachment(LOCAL_ENVIRONMENT_ATTACHMENT_ID, true)],
        )
        .await;
        let old_lease = {
            let registry = coordinator.active.lock().unwrap();
            active_mutable_run(&registry, run_id.as_str())
                .unwrap()
                .lease
                .clone()
        };
        coordinator
            .storage
            .session_store()
            .reconcile_expired_run_admissions(
                LOCAL_SESSION_NAMESPACE,
                old_lease.lease_expires_at + chrono::Duration::seconds(1),
            )
            .await
            .unwrap();
        let replacement_run =
            RunRecord::new(session_id.clone(), RunId::new(), ConversationId::new());
        coordinator
            .storage
            .session_store()
            .acquire_run_admission(AcquireRunAdmission {
                run: replacement_run,
                namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
                host_instance_id: "replacement-host".to_string(),
                admission_id: "replacement-admission".to_string(),
                lease_expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
                idempotency_key: "replacement-start".to_string(),
                command_fingerprint: "replacement-start-v1".to_string(),
                replaces_waiting_run_id: None,
                hitl_resume_claim_id: None,
            })
            .await
            .unwrap();

        let attachment = local_attachment("extra", false);
        let params = EnvironmentActiveMountParams {
            run_id: run_id.as_str().to_string(),
            attachment: attachment.clone(),
            replace: false,
            inject_context: false,
            expected_binding_version: Some(1),
            idempotency_key: Some("stale-mount".to_string()),
        };
        let error = coordinator
            .active_environment_mount(&params, attachment, "stale-mount-digest")
            .await
            .unwrap_err();
        assert_eq!(error.code, RUN_CONFLICT);
        let (environment, binding_version, event_count) = {
            let registry = coordinator.active.lock().unwrap();
            let run = active_mutable_run(&registry, run_id.as_str()).unwrap();
            (
                Arc::clone(&run.environment),
                run.environment_binding_version,
                run.events.len(),
            )
        };
        assert_eq!(binding_version, 1);
        assert_eq!(event_count, 0);
        assert!(
            environment
                .read_text("/environment/extra/environment-fixture.txt")
                .await
                .is_err()
        );
        let durable = coordinator
            .storage
            .replay_event_log()
            .replay_after(&ReplayScope::run(run_id.as_str()), None, None)
            .await
            .unwrap();
        assert!(durable.is_empty());
    }

    #[tokio::test]
    async fn display_projection_batch_rolls_back_when_second_event_fails() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        let storage = SqliteStorage::open(&config.database_path).unwrap();
        let coordinator = RpcRuntimeCoordinator::new(
            config.clone(),
            RpcAgentCatalog::new(config.clone()).unwrap(),
            storage,
            EnvironmentAttachmentManager::new(),
        );
        let (session_id, run_id, _handle) = insert_active_environment_fixture(
            &coordinator,
            vec![local_attachment(LOCAL_ENVIRONMENT_ATTACHMENT_ID, true)],
        )
        .await;
        let connection = Connection::open(&config.database_path).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_second_projected_replay
                 BEFORE INSERT ON replay_events
                 WHEN NEW.sequence = 1
                 BEGIN
                   SELECT RAISE(ABORT, 'injected second replay failure');
                 END;",
            )
            .unwrap();
        drop(connection);
        let response = ModelResponse {
            parts: vec![
                ModelResponsePart::ProviderThinking {
                    text: "inspect".to_string(),
                    signature: None,
                    provider: ProviderPartInfo::new("test").with_id("thinking-1"),
                },
                ModelResponsePart::ProviderText {
                    text: "answer".to_string(),
                    provider: ProviderPartInfo::new("test").with_id("text-1"),
                },
                ModelResponsePart::ProviderToolCall {
                    call: ToolCallPart {
                        id: "call-1".to_string(),
                        name: "lookup".to_string(),
                        arguments: json!({"query": "value"}).into(),
                    },
                    provider: ProviderPartInfo::new("test").with_id("tool-1"),
                },
            ],
            usage: starweaver_agent::Usage::default(),
            model_name: None,
            provider: None,
            finish_reason: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: starweaver_core::Metadata::default(),
        };
        let record =
            AgentStreamRecord::new(0, AgentStreamEvent::ModelResponse { step: 1, response });
        let projection_context = DisplayProjectionContext::new(session_id, run_id.clone());
        assert!(
            DefaultDisplayMessageProjector
                .project(&projection_context, &record)
                .await
                .len()
                > 1
        );
        publish_record(
            &coordinator.active,
            &coordinator.storage.session_store(),
            &RpcRuntimeCoordinator::target(&projection_context.session_id, &run_id),
            &projection_context,
            &record,
        )
        .await;

        let durable = coordinator
            .storage
            .replay_event_log()
            .replay_after(&ReplayScope::run(run_id.as_str()), None, None)
            .await
            .unwrap();
        assert!(durable.is_empty(), "the first event must roll back");
        let registry = coordinator.active.lock().unwrap();
        let run = active_mutable_run(&registry, run_id.as_str()).unwrap();
        assert_eq!(run.next_display_sequence, 0);
        assert_eq!(run.next_event_sequence, 0);
        assert!(run.events.is_empty());
        assert!(run.replay_error.is_some());
    }

    #[tokio::test]
    async fn cancelled_active_mount_request_finishes_once_and_replays_exact_result() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        let storage = SqliteStorage::open(&config.database_path).unwrap();
        let coordinator = RpcRuntimeCoordinator::new(
            config.clone(),
            RpcAgentCatalog::new(config.clone()).unwrap(),
            storage,
            EnvironmentAttachmentManager::new(),
        );
        let (_session_id, run_id, _handle) = insert_active_environment_fixture(
            &coordinator,
            vec![local_attachment(LOCAL_ENVIRONMENT_ATTACHMENT_ID, true)],
        )
        .await;
        let publish_lock = {
            let registry = coordinator.active.lock().unwrap();
            Arc::clone(
                &active_mutable_run(&registry, run_id.as_str())
                    .unwrap()
                    .replay_publish_lock,
            )
        };
        let guard = publish_lock.lock().await;
        let attachment = local_attachment("extra", false);
        let params = EnvironmentActiveMountParams {
            run_id: run_id.as_str().to_string(),
            attachment: attachment.clone(),
            replace: false,
            inject_context: true,
            expected_binding_version: Some(1),
            idempotency_key: Some("lost-response".to_string()),
        };
        let request_coordinator = coordinator.clone();
        let request_params = params.clone();
        let request_attachment = attachment.clone();
        let request = tokio::spawn(async move {
            request_coordinator
                .active_environment_mount(
                    &request_params,
                    request_attachment,
                    "lost-response-digest",
                )
                .await
        });
        tokio::task::yield_now().await;
        request.abort();
        drop(guard);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let ready = coordinator
                    .active_environment_list(run_id.as_str())
                    .ok()
                    .and_then(|value| value["environment"]["bindingVersion"].as_u64())
                    == Some(2);
                if ready {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let replayed = coordinator
            .active_environment_mount(&params, attachment, "lost-response-digest")
            .await
            .unwrap();
        assert!(!replayed.applied);
        assert_eq!(replayed.result["bindingVersion"], 2);
        let registry = coordinator.active.lock().unwrap();
        let run = active_mutable_run(&registry, run_id.as_str()).unwrap();
        assert_eq!(run.environment_binding_version, 2);
        assert_eq!(run.next_event_sequence, 1);
        assert_eq!(run.events.len(), 1);
        assert_eq!(run.environment_idempotency.len(), 1);
    }

    #[tokio::test]
    async fn terminal_publication_barrier_rejects_waiting_environment_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        let storage = SqliteStorage::open(&config.database_path).unwrap();
        let coordinator = RpcRuntimeCoordinator::new(
            config,
            RpcAgentCatalog::new(RpcConfig::for_tests(temp.path())).unwrap(),
            storage,
            EnvironmentAttachmentManager::new(),
        );
        let (session_id, run_id, _handle) = insert_active_environment_fixture(
            &coordinator,
            vec![local_attachment(LOCAL_ENVIRONMENT_ATTACHMENT_ID, true)],
        )
        .await;
        let target = RpcRuntimeCoordinator::target(&session_id, &run_id);
        let (publish_lock, environment) = {
            let registry = coordinator.active.lock().unwrap();
            let run = registry.get(&target).unwrap();
            (
                Arc::clone(&run.replay_publish_lock),
                Arc::clone(&run.environment),
            )
        };
        let guard = publish_lock.lock().await;
        let attachment = local_attachment("extra", false);
        let params = EnvironmentActiveMountParams {
            run_id: run_id.as_str().to_string(),
            attachment: attachment.clone(),
            replace: false,
            inject_context: false,
            expected_binding_version: Some(1),
            idempotency_key: Some("terminal-race".to_string()),
        };
        let request_coordinator = coordinator.clone();
        let request = tokio::spawn(async move {
            request_coordinator
                .active_environment_mount(&params, attachment, "terminal-race-digest")
                .await
        });
        tokio::task::yield_now().await;
        coordinator.active.lock().unwrap().remove(&target).unwrap();
        drop(guard);
        let error = request.await.unwrap().unwrap_err();
        assert_eq!(error.code, RUN_CONFLICT);
        assert!(error.message.contains("active run not found"));
        assert!(
            environment
                .read_text("/environment/extra/environment-fixture.txt")
                .await
                .is_err()
        );
        let durable = coordinator
            .storage
            .replay_event_log()
            .replay_after(&ReplayScope::run(run_id.as_str()), None, None)
            .await
            .unwrap();
        assert!(durable.is_empty());
    }

    #[tokio::test]
    async fn replay_append_failure_publishes_no_cursor_and_cannot_complete_run() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        let storage = SqliteStorage::open(&config.database_path).unwrap();
        let connection = Connection::open(&config.database_path).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_rpc_replay_append
                 BEFORE INSERT ON replay_events
                 BEGIN
                   SELECT RAISE(ABORT, 'injected RPC replay append failure');
                 END;",
            )
            .unwrap();
        drop(connection);
        let catalog = RpcAgentCatalog::new(config.clone()).unwrap();
        let coordinator = RpcRuntimeCoordinator::new(
            config.clone(),
            catalog,
            storage.clone(),
            EnvironmentAttachmentManager::new(),
        );
        let started = coordinator
            .start(RpcRunRequest {
                durable_input: vec![InputPart::text("replay failure")],
                input: AgentInput::text("replay failure"),
                session_id: None,
                restore_from_run_id: None,
                profile: "default".to_string(),
                environment_attachments: Vec::new(),
                idempotency_key: "replay-failure".to_string(),
                command_fingerprint: "replay-failure-v1".to_string(),
                continuation_mode: ContinuationMaterializationMode::Preserve,
                install_session_management: false,
            })
            .await
            .unwrap();
        let status = coordinator
            .await_terminal(
                &started.session_id,
                &started.run_id,
                Some(TEST_RUN_COMPLETION_TIMEOUT),
            )
            .await
            .unwrap();
        assert_eq!(status.status, "failed", "{status:?}");
        assert!(
            status
                .error
                .as_deref()
                .is_some_and(|error| error.contains("replay")),
            "{status:?}"
        );
        assert!(
            coordinator
                .replay(&started.session_id, &started.run_id, None, None)
                .await
                .unwrap()
                .is_empty(),
            "a failed durable append must not leak an active-cache cursor"
        );
        let durable = storage
            .session_store()
            .load_run(&started.session_id, &started.run_id)
            .await
            .unwrap();
        assert_eq!(durable.status, RunStatus::Failed);

        coordinator.shutdown(Duration::from_secs(5)).await.unwrap();
        drop(coordinator);
        drop(storage);
        let connection = Connection::open(&config.database_path).unwrap();
        connection
            .execute_batch("DROP TRIGGER fail_rpc_replay_append;")
            .unwrap();
        drop(connection);
        let reopened_storage = SqliteStorage::open(&config.database_path).unwrap();
        let reopened = RpcRuntimeCoordinator::new(
            config.clone(),
            RpcAgentCatalog::new(config).unwrap(),
            reopened_storage,
            EnvironmentAttachmentManager::new(),
        );
        assert!(
            reopened
                .replay(&started.session_id, &started.run_id, None, None)
                .await
                .unwrap()
                .is_empty(),
            "restart must not reveal a cursor that was never durably appended"
        );
    }

    #[tokio::test]
    async fn every_published_cursor_replays_in_bounded_pages_after_restart() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        let storage = SqliteStorage::open(&config.database_path).unwrap();
        let coordinator = RpcRuntimeCoordinator::new(
            config.clone(),
            RpcAgentCatalog::new(config.clone()).unwrap(),
            storage,
            EnvironmentAttachmentManager::new(),
        );
        let started = coordinator
            .start(RpcRunRequest {
                durable_input: vec![InputPart::text("replay restart")],
                input: AgentInput::text("replay restart"),
                session_id: None,
                restore_from_run_id: None,
                profile: "default".to_string(),
                environment_attachments: Vec::new(),
                idempotency_key: "replay-restart".to_string(),
                command_fingerprint: "replay-restart-v1".to_string(),
                continuation_mode: ContinuationMaterializationMode::Preserve,
                install_session_management: false,
            })
            .await
            .unwrap();
        let status = coordinator
            .await_terminal(
                &started.session_id,
                &started.run_id,
                Some(TEST_RUN_COMPLETION_TIMEOUT),
            )
            .await
            .unwrap();
        assert_eq!(status.status, "completed", "{status:?}");
        let published = coordinator
            .replay(
                &started.session_id,
                &started.run_id,
                None,
                Some(MAX_REPLAY_PAGE_LIMIT),
            )
            .await
            .unwrap();
        assert!(published.len() >= 2, "{published:?}");
        assert!(
            published
                .iter()
                .enumerate()
                .all(|(sequence, event)| event.sequence == sequence),
            "published replay sequences must be contiguous: {published:?}"
        );
        assert!(matches!(
            published.last().map(|event| &event.event),
            Some(ReplayEventKind::Terminal {
                marker: StreamTerminalMarker::RunCompleted
            })
        ));
        coordinator.shutdown(Duration::from_secs(5)).await.unwrap();
        drop(coordinator);

        let reopened_storage = SqliteStorage::open(&config.database_path).unwrap();
        let reopened = RpcRuntimeCoordinator::new(
            config.clone(),
            RpcAgentCatalog::new(config).unwrap(),
            reopened_storage,
            EnvironmentAttachmentManager::new(),
        );
        let mut replayed = Vec::new();
        let mut cursor = None;
        loop {
            let page = reopened
                .replay(
                    &started.session_id,
                    &started.run_id,
                    cursor.clone(),
                    Some(1),
                )
                .await
                .unwrap();
            if page.is_empty() {
                break;
            }
            assert_eq!(page.len(), 1);
            cursor = Some(ReplayCursor::replay_event(
                ReplayScope::run(started.run_id.as_str()),
                page[0].sequence,
            ));
            replayed.extend(page);
        }
        assert_eq!(replayed, published);
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
                replaces_waiting_run_id: None,
                hitl_resume_claim_id: None,
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
