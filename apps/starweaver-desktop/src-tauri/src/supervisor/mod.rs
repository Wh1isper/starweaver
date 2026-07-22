//! Verified local stdio host supervision owned by the privileged Desktop backend.

use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    ffi::OsString,
    fs::File,
    future::Future,
    io::{Read as _, Seek as _, Write as _},
    path::{Path, PathBuf},
    pin::Pin,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
use process_wrap::tokio::JobObject;
#[cfg(unix)]
use process_wrap::tokio::ProcessGroup;
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use starweaver_rpc_core::generated as host;
use tokio::{
    io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader},
    process::{ChildStdin, ChildStdout, Command},
    sync::{Mutex as AsyncMutex, broadcast, mpsc, oneshot},
    time::{Instant, sleep_until, timeout},
};

use crate::generated::host as bridge;

const MAX_HOST_FRAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 64 * 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const HOST_SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_PAGE_TOKENS: usize = 1_024;
const MAX_RETIRED_SUBSCRIPTIONS: usize = 128;
const MAX_UNCERTAIN_OPERATIONS: usize = 1_024;
const MAX_DURABLE_OPERATION_RECORD_BYTES: usize = MAX_HOST_FRAME_BYTES + 16 * 1024;
const MAX_DURABLE_OPERATION_LEDGER_BYTES: usize = 64 * 1024 * 1024;
// Retired identities provide a bounded conflict/idempotency horizon without allowing successful
// renderer acknowledgements to consume the durable ledger forever.
const MAX_RETIRED_OPERATION_TOMBSTONES: usize = 512;
const MAX_RETIRED_OPERATION_TOMBSTONE_BYTES: usize = 2 * 1024 * 1024;
const CRASH_BUDGET: usize = 3;
const CRASH_BUDGET_WINDOW: Duration = Duration::from_secs(30);
#[cfg(all(test, unix))]
const PROCESS_REAP_TIMEOUT: Duration = Duration::from_secs(5);
const DESKTOP_STORAGE_GENERATION: u64 = 1;

const DESKTOP_METHODS: &[host::Method] = &[
    host::Method::ApprovalDecide,
    host::Method::ApprovalList,
    host::Method::ApprovalShow,
    host::Method::CatalogList,
    host::Method::ClarificationResolve,
    host::Method::DeferredComplete,
    host::Method::DeferredFail,
    host::Method::DeferredList,
    host::Method::DeferredShow,
    host::Method::EnvironmentDetach,
    host::Method::EnvironmentHealth,
    host::Method::EnvironmentList,
    host::Method::EventsReplay,
    host::Method::EventsSubscribe,
    host::Method::EventsUnsubscribe,
    host::Method::ModelSelect,
    host::Method::ModelSelectionGet,
    host::Method::ProfileGet,
    host::Method::RunInterrupt,
    host::Method::RunResume,
    host::Method::RunStart,
    host::Method::RunStatus,
    host::Method::RunSteer,
    host::Method::SessionCreate,
    host::Method::SessionDelete,
    host::Method::SessionFork,
    host::Method::SessionGet,
    host::Method::SessionList,
    host::Method::SessionSearch,
];

const DESKTOP_EVENT_CLASSES: &[host::EventClass] = &[
    host::EventClass::ApprovalChanged,
    host::EventClass::ClarificationChanged,
    host::EventClass::DeferredChanged,
    host::EventClass::OutputAvailable,
    host::EventClass::RunChanged,
];

/// Immutable verified runtime and launch selection supplied by the Desktop update/config owner.
#[derive(Clone, Debug)]
pub struct LocalLaunchSpec {
    /// Absolute managed `starweaver-rpc` executable path.
    pub runtime_path: PathBuf,
    /// Expected lowercase SHA-256 digest prefixed by `sha256:`.
    pub runtime_digest: String,
    /// Exact runtime package version expected during initialize.
    pub runtime_version: String,
    /// Exact immutable build revision expected during initialize.
    pub build_revision: String,
    /// Exact target identity expected during initialize.
    pub target: String,
    /// Absolute Desktop-owned public launch-envelope path.
    pub launch_envelope_path: PathBuf,
    /// Expected digest of the immutable public launch envelope.
    pub launch_envelope_digest: String,
    /// Expected monotonic launch configuration generation.
    pub configuration_generation: u64,
    /// Expected stable execution-domain identity.
    pub execution_domain_id: String,
    /// Expected stable workspace identity.
    pub workspace_identity: String,
}

/// Public process-state projection. Paths, process IDs, diagnostics, and wire metadata are omitted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostChildState {
    /// No managed runtime selection has been installed.
    #[default]
    Unconfigured,
    /// A verified child spawn is in progress.
    Starting,
    /// Initialize is the only admitted request.
    Handshaking,
    /// User operations are admitted.
    Ready,
    /// New operations are denied while shutdown completes.
    Draining,
    /// An unexpected transport failure is being fenced.
    Recovering,
    /// The child stopped cleanly.
    Stopped,
    /// Compatibility checks failed and automatic restart is forbidden.
    Incompatible,
    /// Startup or transport failed.
    Failed,
}

/// Safe supervisor status retained across renderer reloads.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSupervisorStatus {
    /// Current state.
    pub state: HostChildState,
    /// Monotonic child generation.
    pub generation: u64,
    /// Whether bounded private stderr diagnostics exist.
    pub diagnostics_available: bool,
}

/// Stable safe error categories returned across privileged IPC.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorErrorCode {
    /// No compatible initialized child currently admits operations.
    NotReady,
    /// Desktop-owned configuration or routing failed validation.
    InvalidConfiguration,
    /// Runtime, launch, protocol, or storage compatibility failed.
    Incompatible,
    /// Framing, process, timeout, or correlation failed.
    Transport,
    /// The host returned one of its declared public operation errors.
    Remote,
    /// A local invariant or safe projection failed.
    Internal,
}

/// Sanitized supervisor failure. It never contains a path, provider body, SQL, or debug chain.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupervisorError {
    /// Stable category.
    pub code: SupervisorErrorCode,
    /// Fixed user-safe summary.
    pub message: String,
    /// Declared JSON-RPC error code for safe remote failures.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_code: Option<i64>,
    /// Declared public error discriminator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_kind: Option<String>,
    /// Whether the host declares the operation retryable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    /// Whether the host requires reconciliation before retry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconciliation_required: Option<bool>,
    /// Safe resource category, when declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_kind: Option<String>,
    /// Backend-issued result acknowledgement for a conclusively rejected mutation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_acknowledgement_token: Option<String>,
}

impl SupervisorError {
    fn new(code: SupervisorErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
            remote_code: None,
            remote_kind: None,
            retryable: None,
            reconciliation_required: None,
            resource_kind: None,
            operation_acknowledgement_token: None,
        }
    }

    pub(crate) fn not_ready() -> Self {
        Self::new(SupervisorErrorCode::NotReady, "host runtime is not ready")
    }

    pub(crate) fn transport() -> Self {
        Self::new(
            SupervisorErrorCode::Transport,
            "host transport failed; reconciliation is required",
        )
    }

    pub(crate) fn invalid_configuration(message: &'static str) -> Self {
        Self::new(SupervisorErrorCode::InvalidConfiguration, message)
    }

    fn incompatible() -> Self {
        Self::new(
            SupervisorErrorCode::Incompatible,
            "managed host runtime is incompatible with this Desktop build",
        )
    }

    fn remote(error: &host::HostError) -> Self {
        let data = serde_json::to_value(&error.data).ok();
        let field = |name: &str| data.as_ref().and_then(|value| value.get(name));
        Self {
            code: SupervisorErrorCode::Remote,
            message: "host operation was rejected".to_string(),
            remote_code: Some(error.code),
            remote_kind: field("kind")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            retryable: field("retryable").and_then(serde_json::Value::as_bool),
            reconciliation_required: field("reconciliationRequired")
                .and_then(serde_json::Value::as_bool),
            resource_kind: field("resourceKind")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            operation_acknowledgement_token: None,
        }
    }
}

#[derive(Clone, Debug)]
struct VerifiedLaunch {
    runtime_path: PathBuf,
    runtime_digest: String,
    launch_envelope_path: PathBuf,
    envelope: host::LaunchEnvelope,
    envelope_digest: String,
    credential_environment: Vec<String>,
    runtime_identity: FileIdentity,
    envelope_identity: FileIdentity,
    runtime_version: String,
    build_revision: String,
    target: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    canonical_path: PathBuf,
    length: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

struct SupervisorState {
    state: HostChildState,
    generation: u64,
    execution_domain: Option<String>,
}

impl Default for SupervisorState {
    fn default() -> Self {
        Self {
            state: HostChildState::Unconfigured,
            generation: 0,
            execution_domain: None,
        }
    }
}

#[derive(Default)]
struct BoundedDiagnostics {
    bytes: VecDeque<u8>,
}

impl BoundedDiagnostics {
    fn append(&mut self, chunk: &[u8]) {
        for byte in chunk {
            if self.bytes.len() == MAX_STDERR_BYTES {
                self.bytes.pop_front();
            }
            self.bytes.push_back(*byte);
        }
    }

    fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SubscriptionPhase {
    Active,
    AwaitingClose,
    Closed,
}

#[derive(Clone, Debug)]
struct SubscriptionState {
    generation: u64,
    next_delivery_sequence: u64,
    phase: SubscriptionPhase,
}

#[derive(Default)]
struct SubscriptionLedger {
    records: HashMap<String, SubscriptionState>,
    retired_order: VecDeque<String>,
}

impl SubscriptionLedger {
    fn retire(&mut self, subscription_id: &str, phase: SubscriptionPhase) {
        let Some(state) = self.records.get_mut(subscription_id) else {
            return;
        };
        if state.phase == SubscriptionPhase::Active {
            self.retired_order.push_back(subscription_id.to_string());
        }
        state.phase = phase;
        while self.retired_order.len() > MAX_RETIRED_SUBSCRIPTIONS {
            let Some(oldest) = self.retired_order.pop_front() else {
                break;
            };
            if self
                .records
                .get(&oldest)
                .is_some_and(|record| record.phase != SubscriptionPhase::Active)
            {
                self.records.remove(&oldest);
            }
        }
    }
}

enum NotificationAdmission {
    Deliver,
    Ignore,
    Reject,
}

#[derive(Clone, Debug)]
struct PageTokenRecord {
    generation: u64,
    execution_domain: String,
    operation_fingerprint: String,
    wire_cursor: String,
}

#[derive(Default)]
struct PageTokenLedger {
    records: HashMap<String, PageTokenRecord>,
    order: VecDeque<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DurableOperationState {
    Pending,
    Retired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DurableOperationRecord {
    schema_version: u32,
    state: DurableOperationState,
    operation_id: String,
    execution_domain: String,
    fingerprint: String,
    operation: Option<serde_json::Value>,
    idempotency_key: String,
    acknowledgement_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retired_at_unix_ms: Option<u64>,
}

enum ShutdownAdmission {
    Terminal,
    Wait {
        generation: u64,
    },
    Actor {
        generation: u64,
        execution_domain: String,
    },
}

struct Shared {
    state: Mutex<SupervisorState>,
    diagnostics: Mutex<BoundedDiagnostics>,
    subscriptions: Mutex<SubscriptionLedger>,
    durable_operations: Mutex<HashMap<String, DurableOperationRecord>>,
    page_tokens: Mutex<PageTokenLedger>,
    recovery_spec: Mutex<Option<LocalLaunchSpec>>,
    recent_crashes: Mutex<VecDeque<std::time::Instant>>,
    recovery_cancelled: AtomicBool,
    storage_root: Mutex<Option<PathBuf>>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            state: Mutex::new(SupervisorState::default()),
            diagnostics: Mutex::new(BoundedDiagnostics::default()),
            subscriptions: Mutex::new(SubscriptionLedger::default()),
            durable_operations: Mutex::new(HashMap::new()),
            page_tokens: Mutex::new(PageTokenLedger::default()),
            recovery_spec: Mutex::new(None),
            recent_crashes: Mutex::new(VecDeque::new()),
            recovery_cancelled: AtomicBool::new(false),
            storage_root: Mutex::new(None),
        }
    }
}

impl Shared {
    fn configure_storage_root(&self, root: PathBuf) -> Result<(), SupervisorError> {
        let mut configured = self.storage_root.lock().map_err(|_| {
            SupervisorError::new(
                SupervisorErrorCode::Internal,
                "supervisor storage unavailable",
            )
        })?;
        if let Some(existing) = configured.as_ref() {
            return if existing == &root {
                Ok(())
            } else {
                Err(SupervisorError::new(
                    SupervisorErrorCode::InvalidConfiguration,
                    "supervisor storage is already configured",
                ))
            };
        }
        let operations = load_durable_operations(&root)?;
        *self.durable_operations.lock().map_err(|_| {
            SupervisorError::new(
                SupervisorErrorCode::Internal,
                "operation ledger unavailable",
            )
        })? = operations;
        *configured = Some(root);
        drop(configured);
        Ok(())
    }

    fn storage_root(&self) -> Result<PathBuf, SupervisorError> {
        self.storage_root
            .lock()
            .map_err(|_| {
                SupervisorError::new(
                    SupervisorErrorCode::Internal,
                    "supervisor storage unavailable",
                )
            })?
            .clone()
            .ok_or_else(|| {
                SupervisorError::new(
                    SupervisorErrorCode::NotReady,
                    "supervisor storage is not configured",
                )
            })
    }

    fn begin_start(&self) -> Result<u64, SupervisorError> {
        let mut state = self.state.lock().map_err(|_| {
            SupervisorError::new(SupervisorErrorCode::Internal, "host state unavailable")
        })?;
        if !matches!(
            state.state,
            HostChildState::Unconfigured | HostChildState::Stopped | HostChildState::Failed
        ) {
            return Err(SupervisorError::not_ready());
        }
        self.recovery_cancelled.store(false, Ordering::Release);
        state.generation = state.generation.saturating_add(1);
        state.state = HostChildState::Starting;
        state.execution_domain = None;
        let generation = state.generation;
        drop(state);
        self.clear_generation_scoped_state()?;
        if let Ok(mut crashes) = self.recent_crashes.lock() {
            crashes.clear();
        }
        Ok(generation)
    }

    fn remember_recovery_spec(&self, spec: LocalLaunchSpec) -> Result<(), SupervisorError> {
        *self.recovery_spec.lock().map_err(|_| {
            SupervisorError::new(SupervisorErrorCode::Internal, "recovery state unavailable")
        })? = Some(spec);
        Ok(())
    }

    fn recovery_spec(&self) -> Option<LocalLaunchSpec> {
        self.recovery_spec.lock().ok().and_then(|spec| spec.clone())
    }

    fn begin_recovery_start(&self) -> Result<u64, SupervisorError> {
        if self.recovery_cancelled.load(Ordering::Acquire) {
            return Err(SupervisorError::not_ready());
        }
        let mut state = self.state.lock().map_err(|_| {
            SupervisorError::new(SupervisorErrorCode::Internal, "host state unavailable")
        })?;
        if !matches!(
            state.state,
            HostChildState::Recovering | HostChildState::Failed
        ) {
            return Err(SupervisorError::not_ready());
        }
        state.generation = state.generation.saturating_add(1);
        state.state = HostChildState::Starting;
        state.execution_domain = None;
        let generation = state.generation;
        drop(state);
        self.clear_generation_scoped_state()?;
        Ok(generation)
    }

    fn transition(&self, generation: u64, next: HostChildState) -> Result<(), SupervisorError> {
        let mut state = self.state.lock().map_err(|_| {
            SupervisorError::new(SupervisorErrorCode::Internal, "host state unavailable")
        })?;
        if state.generation != generation || !valid_transition(state.state, next) {
            return Err(SupervisorError::new(
                SupervisorErrorCode::Internal,
                "invalid host lifecycle transition",
            ));
        }
        state.state = next;
        drop(state);
        Ok(())
    }

    fn request_shutdown(&self) -> Result<ShutdownAdmission, SupervisorError> {
        self.recovery_cancelled.store(true, Ordering::Release);
        let mut state = self.state.lock().map_err(|_| {
            SupervisorError::new(SupervisorErrorCode::Internal, "host state unavailable")
        })?;
        let admission = match state.state {
            HostChildState::Ready => {
                let execution_domain = state
                    .execution_domain
                    .clone()
                    .ok_or_else(SupervisorError::not_ready)?;
                state.state = HostChildState::Draining;
                ShutdownAdmission::Actor {
                    generation: state.generation,
                    execution_domain,
                }
            }
            HostChildState::Starting | HostChildState::Handshaking => {
                state.state = HostChildState::Draining;
                ShutdownAdmission::Wait {
                    generation: state.generation,
                }
            }
            HostChildState::Draining => ShutdownAdmission::Wait {
                generation: state.generation,
            },
            HostChildState::Recovering => {
                state.state = HostChildState::Stopped;
                ShutdownAdmission::Terminal
            }
            HostChildState::Unconfigured
            | HostChildState::Stopped
            | HostChildState::Failed
            | HostChildState::Incompatible => ShutdownAdmission::Terminal,
        };
        drop(state);
        Ok(admission)
    }

    fn finish_startup_shutdown(&self, generation: u64) -> Result<bool, SupervisorError> {
        let mut state = self.state.lock().map_err(|_| {
            SupervisorError::new(SupervisorErrorCode::Internal, "host state unavailable")
        })?;
        let completed = state.generation == generation && state.state == HostChildState::Draining;
        if completed {
            state.state = HostChildState::Stopped;
            state.execution_domain = None;
        }
        drop(state);
        Ok(completed)
    }

    fn startup_is_draining(&self, generation: u64) -> bool {
        self.state.lock().is_ok_and(|state| {
            state.generation == generation && state.state == HostChildState::Draining
        })
    }

    fn finish_actor(
        &self,
        generation: u64,
        shutdown_completed: bool,
        crash_budget_allows_recovery: bool,
    ) -> Result<bool, SupervisorError> {
        let mut state = self.state.lock().map_err(|_| {
            SupervisorError::new(SupervisorErrorCode::Internal, "host state unavailable")
        })?;
        if state.generation != generation {
            return Ok(false);
        }
        let recover = match state.state {
            HostChildState::Draining => {
                state.state = if shutdown_completed {
                    HostChildState::Stopped
                } else {
                    HostChildState::Failed
                };
                false
            }
            HostChildState::Ready if crash_budget_allows_recovery => {
                state.state = HostChildState::Recovering;
                true
            }
            _ => {
                state.state = HostChildState::Failed;
                false
            }
        };
        drop(state);
        Ok(recover)
    }

    fn set_ready_domain(&self, generation: u64, domain: String) -> Result<(), SupervisorError> {
        let mut state = self.state.lock().map_err(|_| {
            SupervisorError::new(SupervisorErrorCode::Internal, "host state unavailable")
        })?;
        if state.generation != generation || state.state != HostChildState::Handshaking {
            return Err(SupervisorError::new(
                SupervisorErrorCode::Internal,
                "invalid host lifecycle transition",
            ));
        }
        state.execution_domain = Some(domain);
        state.state = HostChildState::Ready;
        drop(state);
        Ok(())
    }

    fn status(&self) -> HostSupervisorStatus {
        let (state, generation) = self
            .state
            .lock()
            .map_or((HostChildState::Failed, 0), |state| {
                (state.state, state.generation)
            });
        let diagnostics_available = self
            .diagnostics
            .lock()
            .is_ok_and(|diagnostics| !diagnostics.is_empty());
        HostSupervisorStatus {
            state,
            generation,
            diagnostics_available,
        }
    }

    fn clear_generation_scoped_state(&self) -> Result<(), SupervisorError> {
        let mut subscriptions = self.subscriptions.lock().map_err(|_| {
            SupervisorError::new(
                SupervisorErrorCode::Internal,
                "subscription state unavailable",
            )
        })?;
        subscriptions.records.clear();
        subscriptions.retired_order.clear();
        drop(subscriptions);
        let mut page_tokens = self.page_tokens.lock().map_err(|_| {
            SupervisorError::new(
                SupervisorErrorCode::Internal,
                "page token state unavailable",
            )
        })?;
        page_tokens.records.clear();
        page_tokens.order.clear();
        drop(page_tokens);
        Ok(())
    }

    fn register_subscription(
        &self,
        generation: u64,
        id: &host::SubscriptionId,
    ) -> Result<(), SupervisorError> {
        let mut subscriptions = self.subscriptions.lock().map_err(|_| {
            SupervisorError::new(
                SupervisorErrorCode::Internal,
                "subscription state unavailable",
            )
        })?;
        if subscriptions
            .records
            .insert(
                id.as_str().to_string(),
                SubscriptionState {
                    generation,
                    next_delivery_sequence: 1,
                    phase: SubscriptionPhase::Active,
                },
            )
            .is_some()
        {
            return Err(SupervisorError::transport());
        }
        drop(subscriptions);
        Ok(())
    }

    fn complete_unsubscribe(
        &self,
        generation: u64,
        result: &host::EventsUnsubscribeResult,
    ) -> Result<(), SupervisorError> {
        let mut subscriptions = self.subscriptions.lock().map_err(|_| {
            SupervisorError::new(
                SupervisorErrorCode::Internal,
                "subscription state unavailable",
            )
        })?;
        let Some(state) = subscriptions.records.get(result.subscription_id.as_str()) else {
            return Err(SupervisorError::transport());
        };
        if state.generation != generation {
            return Err(SupervisorError::transport());
        }
        if state.phase != SubscriptionPhase::Closed {
            // A terminal close may cross this response in either direction. Keep a bounded
            // correlation tombstone until the close arrives instead of escalating that legal
            // race into a shared transport failure.
            subscriptions.retire(
                result.subscription_id.as_str(),
                SubscriptionPhase::AwaitingClose,
            );
        }
        drop(subscriptions);
        Ok(())
    }

    fn admit_notification(
        &self,
        generation: u64,
        notification: &host::HostNotification,
    ) -> NotificationAdmission {
        let Ok(mut subscriptions) = self.subscriptions.lock() else {
            return NotificationAdmission::Reject;
        };
        match &notification.params {
            host::HostNotificationParams::HostEvent(params) => {
                let Some(expected) = subscriptions
                    .records
                    .get_mut(params.subscription_id.as_str())
                else {
                    return NotificationAdmission::Reject;
                };
                if expected.generation != generation
                    || expected.phase != SubscriptionPhase::Active
                    || params.delivery_sequence.get() != expected.next_delivery_sequence
                {
                    return NotificationAdmission::Reject;
                }
                let Some(next) = expected.next_delivery_sequence.checked_add(1) else {
                    return NotificationAdmission::Reject;
                };
                expected.next_delivery_sequence = next;
                NotificationAdmission::Deliver
            }
            host::HostNotificationParams::SubscriptionClosed(params) => {
                let Some(expected) = subscriptions.records.get(params.subscription_id.as_str())
                else {
                    return NotificationAdmission::Reject;
                };
                if expected.generation != generation {
                    return NotificationAdmission::Reject;
                }
                if expected.phase == SubscriptionPhase::Closed {
                    return NotificationAdmission::Ignore;
                }
                subscriptions.retire(params.subscription_id.as_str(), SubscriptionPhase::Closed);
                NotificationAdmission::Deliver
            }
        }
    }

    fn admit_operation(
        &self,
        operation_id: &str,
        execution_domain: &str,
        fingerprint: &str,
        operation: &bridge::DesktopHostOperation,
    ) -> Result<((String, String), Option<DurableOperationRecord>), SupervisorError> {
        validate_operation_id(operation_id)?;
        let mut operations = self.durable_operations.lock().map_err(|_| {
            SupervisorError::new(
                SupervisorErrorCode::Internal,
                "operation ledger unavailable",
            )
        })?;
        if let Some(existing) = operations.get(operation_id) {
            if existing.execution_domain != execution_domain || existing.fingerprint != fingerprint
            {
                return Err(SupervisorError::new(
                    SupervisorErrorCode::InvalidConfiguration,
                    "logical operation identity does not match its original operation",
                ));
            }
            return Ok((
                (
                    existing.idempotency_key.clone(),
                    existing.acknowledgement_token.clone(),
                ),
                None,
            ));
        }
        if operations
            .values()
            .filter(|record| record.state == DurableOperationState::Pending)
            .count()
            >= MAX_UNCERTAIN_OPERATIONS
        {
            return Err(SupervisorError::new(
                SupervisorErrorCode::NotReady,
                "operation reconciliation is required before accepting more mutations",
            ));
        }
        let record = DurableOperationRecord {
            schema_version: 1,
            state: DurableOperationState::Pending,
            operation_id: operation_id.to_string(),
            execution_domain: execution_domain.to_string(),
            fingerprint: fingerprint.to_string(),
            operation: Some(serde_json::to_value(operation).map_err(|_| {
                SupervisorError::new(
                    SupervisorErrorCode::Internal,
                    "logical operation could not be persisted",
                )
            })?),
            idempotency_key: format!("desktop-{}", uuid::Uuid::new_v4()),
            acknowledgement_token: format!("desktop-operation-ack-v1-{}", uuid::Uuid::new_v4()),
            retired_at_unix_ms: None,
        };
        let record_bytes = serde_json::to_vec(&record).map_err(|_| {
            SupervisorError::new(
                SupervisorErrorCode::Internal,
                "logical operation could not be persisted",
            )
        })?;
        let ledger_bytes = operations.values().try_fold(0_usize, |total, existing| {
            let bytes = serde_json::to_vec(existing).map_err(|_| {
                SupervisorError::new(
                    SupervisorErrorCode::Internal,
                    "operation ledger unavailable",
                )
            })?;
            total.checked_add(bytes.len()).ok_or_else(|| {
                SupervisorError::new(
                    SupervisorErrorCode::NotReady,
                    "operation reconciliation is required before accepting more mutations",
                )
            })
        })?;
        if record_bytes.len() > MAX_DURABLE_OPERATION_RECORD_BYTES
            || ledger_bytes
                .checked_add(record_bytes.len())
                .is_none_or(|total| total > MAX_DURABLE_OPERATION_LEDGER_BYTES)
        {
            return Err(SupervisorError::new(
                SupervisorErrorCode::NotReady,
                "operation reconciliation is required before accepting more mutations",
            ));
        }
        let identity = (
            record.idempotency_key.clone(),
            record.acknowledgement_token.clone(),
        );
        operations.insert(operation_id.to_string(), record.clone());
        drop(operations);
        Ok((identity, Some(record)))
    }

    fn rollback_operation(&self, record: &DurableOperationRecord) {
        if let Ok(mut operations) = self.durable_operations.lock()
            && operations.get(&record.operation_id) == Some(record)
        {
            operations.remove(&record.operation_id);
        }
    }

    fn pending_operations(&self) -> Result<Vec<bridge::DesktopHostInvocation>, SupervisorError> {
        let operations = self.durable_operations.lock().map_err(|_| {
            SupervisorError::new(
                SupervisorErrorCode::Internal,
                "operation ledger unavailable",
            )
        })?;
        let mut records = operations
            .values()
            .filter(|record| record.state == DurableOperationState::Pending)
            .cloned()
            .collect::<Vec<_>>();
        drop(operations);
        records.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
        records
            .into_iter()
            .map(|record| {
                let operation = serde_json::from_value(
                    record.operation.ok_or_else(SupervisorError::transport)?,
                )
                .map_err(|_| SupervisorError::transport())?;
                Ok(bridge::DesktopHostInvocation {
                    operation_id: bridge::DesktopOperationId(record.operation_id),
                    operation,
                })
            })
            .collect()
    }

    fn operation_for_acknowledgement(
        &self,
        acknowledgement_token: &str,
    ) -> Result<Option<DurableOperationRecord>, SupervisorError> {
        validate_operation_acknowledgement_token(acknowledgement_token)?;
        let operations = self.durable_operations.lock().map_err(|_| {
            SupervisorError::new(
                SupervisorErrorCode::Internal,
                "operation ledger unavailable",
            )
        })?;
        Ok(operations
            .values()
            .find(|record| record.acknowledgement_token == acknowledgement_token)
            .cloned())
    }

    fn next_retired_at_unix_ms(&self) -> Result<u64, SupervisorError> {
        let previous = {
            let operations = self.durable_operations.lock().map_err(|_| {
                SupervisorError::new(
                    SupervisorErrorCode::Internal,
                    "operation ledger unavailable",
                )
            })?;
            operations
                .values()
                .filter_map(|record| record.retired_at_unix_ms)
                .max()
                .unwrap_or(0)
        };
        let next = previous
            .checked_add(1)
            .ok_or_else(SupervisorError::transport)?;
        Ok(current_unix_time_ms()?.max(next))
    }

    fn retired_operation_ids_after(
        &self,
        retired: &DurableOperationRecord,
    ) -> Result<Vec<String>, SupervisorError> {
        let mut prospective = {
            let operations = self.durable_operations.lock().map_err(|_| {
                SupervisorError::new(
                    SupervisorErrorCode::Internal,
                    "operation ledger unavailable",
                )
            })?;
            operations.clone()
        };
        prospective.insert(retired.operation_id.clone(), retired.clone());
        retired_operation_ids_to_prune(&prospective)
    }

    fn retire_operation(
        &self,
        pending: &DurableOperationRecord,
        retired: DurableOperationRecord,
        pruned_operation_ids: &[String],
    ) -> Result<(), SupervisorError> {
        let mut operations = self.durable_operations.lock().map_err(|_| {
            SupervisorError::new(
                SupervisorErrorCode::Internal,
                "operation ledger unavailable",
            )
        })?;
        if operations.get(&pending.operation_id) != Some(pending) {
            return Ok(());
        }
        operations.insert(pending.operation_id.clone(), retired);
        for operation_id in pruned_operation_ids {
            operations.remove(operation_id);
        }
        drop(operations);
        Ok(())
    }

    fn resolve_page_cursor(
        &self,
        token: Option<&bridge::DesktopPageToken>,
        generation: u64,
        execution_domain: &str,
        operation_fingerprint: &str,
    ) -> Result<Option<String>, SupervisorError> {
        let Some(token) = token else {
            return Ok(None);
        };
        let ledger = self.page_tokens.lock().map_err(|_| {
            SupervisorError::new(
                SupervisorErrorCode::Internal,
                "page token state unavailable",
            )
        })?;
        let record = ledger.records.get(&token.0).cloned().ok_or_else(|| {
            SupervisorError::new(
                SupervisorErrorCode::InvalidConfiguration,
                "Desktop page token is invalid or expired",
            )
        })?;
        drop(ledger);
        if record.generation != generation
            || record.execution_domain != execution_domain
            || record.operation_fingerprint != operation_fingerprint
        {
            return Err(SupervisorError::new(
                SupervisorErrorCode::InvalidConfiguration,
                "Desktop page token does not match this operation",
            ));
        }
        Ok(Some(record.wire_cursor))
    }

    fn issue_page_token(
        &self,
        generation: u64,
        execution_domain: &str,
        operation_fingerprint: &str,
        wire_cursor: String,
    ) -> Result<bridge::DesktopPageToken, SupervisorError> {
        let value = format!("desktop-page-{}", uuid::Uuid::new_v4());
        let mut ledger = self.page_tokens.lock().map_err(|_| {
            SupervisorError::new(
                SupervisorErrorCode::Internal,
                "page token state unavailable",
            )
        })?;
        while ledger.records.len() >= MAX_PAGE_TOKENS {
            if let Some(expired) = ledger.order.pop_front() {
                ledger.records.remove(&expired);
            } else {
                break;
            }
        }
        ledger.order.push_back(value.clone());
        ledger.records.insert(
            value.clone(),
            PageTokenRecord {
                generation,
                execution_domain: execution_domain.to_string(),
                operation_fingerprint: operation_fingerprint.to_string(),
                wire_cursor,
            },
        );
        drop(ledger);
        Ok(bridge::DesktopPageToken(value))
    }

    fn is_recovery_candidate(&self, generation: u64) -> bool {
        self.state.lock().is_ok_and(|state| {
            state.generation == generation && state.state == HostChildState::Ready
        })
    }

    fn allow_recovery_after_crash(&self) -> bool {
        let Ok(mut crashes) = self.recent_crashes.lock() else {
            return false;
        };
        let now = std::time::Instant::now();
        while crashes
            .front()
            .is_some_and(|crash| now.duration_since(*crash) > CRASH_BUDGET_WINDOW)
        {
            crashes.pop_front();
        }
        crashes.push_back(now);
        crashes.len() <= CRASH_BUDGET
    }

    fn ready_domain(&self) -> Result<(u64, String), SupervisorError> {
        let state = self.state.lock().map_err(|_| {
            SupervisorError::new(SupervisorErrorCode::Internal, "host state unavailable")
        })?;
        if state.state != HostChildState::Ready {
            return Err(SupervisorError::not_ready());
        }
        let domain = state
            .execution_domain
            .clone()
            .ok_or_else(SupervisorError::not_ready)?;
        Ok((state.generation, domain))
    }
}

const fn valid_transition(current: HostChildState, next: HostChildState) -> bool {
    matches!(
        (current, next),
        (
            HostChildState::Starting,
            HostChildState::Handshaking
                | HostChildState::Draining
                | HostChildState::Failed
                | HostChildState::Incompatible
        ) | (
            HostChildState::Handshaking,
            HostChildState::Draining | HostChildState::Failed | HostChildState::Incompatible
        ) | (
            HostChildState::Ready,
            HostChildState::Draining | HostChildState::Recovering | HostChildState::Failed
        ) | (
            HostChildState::Draining,
            HostChildState::Stopped | HostChildState::Failed
        ) | (
            HostChildState::Recovering,
            HostChildState::Draining | HostChildState::Failed | HostChildState::Starting
        )
    )
}

pub(crate) struct RunEventTail {
    pub(crate) subscription_id: host::SubscriptionId,
    pub(crate) generation: u64,
    pub(crate) execution_domain: String,
}

pub(crate) struct BackendHostEvent {
    pub(crate) event: bridge::SafeHostEvent,
    pub(crate) cursor: String,
    pub(crate) event_id: String,
}

pub(crate) struct BackendHostEventPage {
    pub(crate) deliveries: Vec<BackendHostEvent>,
    pub(crate) next_cursor: String,
    pub(crate) has_more: bool,
    pub(crate) generation: u64,
    pub(crate) execution_domain: String,
}

struct PendingRequest {
    method: host::Method,
    expected_unsubscribe_subscription_id: Option<host::SubscriptionId>,
    response: oneshot::Sender<Result<host::HostResult, SupervisorError>>,
    shutdown: bool,
    deadline: Instant,
}

async fn wait_for_deadline(deadline: Option<Instant>) {
    if let Some(deadline) = deadline {
        sleep_until(deadline).await;
    } else {
        std::future::pending::<()>().await;
    }
}

enum ActorCommand {
    Execute {
        expected_generation: u64,
        expected_domain: String,
        request: host::HostRequest,
        response: oneshot::Sender<Result<host::HostResult, SupervisorError>>,
    },
    Shutdown {
        expected_generation: u64,
        expected_domain: String,
        deadline: Instant,
        request: host::HostRequest,
        response: oneshot::Sender<Result<host::HostResult, SupervisorError>>,
    },
}

/// One process-owned local host supervisor shared by all renderer windows.
pub struct LocalHostSupervisor {
    shared: Arc<Shared>,
    actor: Arc<AsyncMutex<Option<mpsc::Sender<ActorCommand>>>>,
    mutation_gate: AsyncMutex<()>,
    notifications: broadcast::Sender<host::HostNotification>,
    next_request: AtomicU64,
}

impl Default for LocalHostSupervisor {
    fn default() -> Self {
        let (notifications, _) = broadcast::channel(256);
        Self {
            shared: Arc::new(Shared::default()),
            actor: Arc::new(AsyncMutex::new(None)),
            mutation_gate: AsyncMutex::new(()),
            notifications,
            next_request: AtomicU64::new(1),
        }
    }
}

impl LocalHostSupervisor {
    /// Configure the private Desktop-owned persistence and executable staging root.
    ///
    /// # Errors
    ///
    /// Returns an error when a different root was already configured.
    pub fn configure_storage_root(&self, root: PathBuf) -> Result<(), SupervisorError> {
        self.shared.configure_storage_root(root)
    }

    /// Return the current safe status.
    #[must_use]
    pub fn status(&self) -> HostSupervisorStatus {
        self.shared.status()
    }

    /// Subscribe to typed host notifications. Transport-owned recovery remains in this backend.
    pub fn subscribe_notifications(&self) -> broadcast::Receiver<host::HostNotification> {
        self.notifications.subscribe()
    }

    /// Verify, spawn, initialize, and compatibility-gate one exact managed runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when verification, process startup, initialization, or compatibility gating fails.
    #[allow(clippy::too_many_lines)]
    pub async fn start(&self, spec: LocalLaunchSpec) -> Result<(), SupervisorError> {
        let generation = self.shared.begin_start()?;
        self.shared.remember_recovery_spec(spec.clone())?;
        let verification = tokio::task::spawn_blocking(move || verify_launch(spec))
            .await
            .map_err(|_| {
                SupervisorError::new(SupervisorErrorCode::Internal, "runtime verification failed")
            });
        if self.shared.finish_startup_shutdown(generation)? {
            return Err(SupervisorError::not_ready());
        }
        let verified = match verification {
            Ok(Ok(verified)) => verified,
            Ok(Err(error)) => {
                if self.shared.finish_startup_shutdown(generation)? {
                    return Err(SupervisorError::not_ready());
                }
                let _ = self.shared.transition(
                    generation,
                    if error.code == SupervisorErrorCode::Incompatible {
                        HostChildState::Incompatible
                    } else {
                        HostChildState::Failed
                    },
                );
                return Err(error);
            }
            Err(error) => {
                if self.shared.finish_startup_shutdown(generation)? {
                    return Err(SupervisorError::not_ready());
                }
                let _ = self.shared.transition(generation, HostChildState::Failed);
                return Err(error);
            }
        };
        if let Err(error) = self
            .shared
            .transition(generation, HostChildState::Handshaking)
        {
            if self.shared.finish_startup_shutdown(generation)? {
                return Err(SupervisorError::not_ready());
            }
            return Err(error);
        }
        let spawned = spawn_verified_child(&verified, &self.shared).await;
        if self.shared.startup_is_draining(generation) {
            if let Ok(mut process) = spawned {
                terminate_and_reap(&mut process).await;
            }
            let _ = self.shared.finish_startup_shutdown(generation);
            return Err(SupervisorError::not_ready());
        }
        let mut process = match spawned {
            Ok(process) => process,
            Err(error) => {
                let _ = self.shared.transition(
                    generation,
                    if error.code == SupervisorErrorCode::Incompatible {
                        HostChildState::Incompatible
                    } else {
                        HostChildState::Failed
                    },
                );
                return Err(error);
            }
        };
        let initialization = initialize_child(&mut process, &verified, generation).await;
        if self.shared.startup_is_draining(generation) {
            terminate_and_reap(&mut process).await;
            let _ = self.shared.finish_startup_shutdown(generation);
            return Err(SupervisorError::not_ready());
        }
        if let Err(error) = initialization {
            terminate_and_reap(&mut process).await;
            let _ = self.shared.transition(
                generation,
                if error.code == SupervisorErrorCode::Incompatible {
                    HostChildState::Incompatible
                } else {
                    HostChildState::Failed
                },
            );
            return Err(error);
        }
        let (sender, receiver) = mpsc::channel(64);
        *self.actor.lock().await = Some(sender);
        if let Err(error) = self
            .shared
            .set_ready_domain(generation, verified.envelope.execution_domain_id.clone())
        {
            self.actor.lock().await.take();
            terminate_and_reap(&mut process).await;
            if self.shared.finish_startup_shutdown(generation)? {
                return Err(SupervisorError::not_ready());
            }
            let _ = self.shared.transition(generation, HostChildState::Failed);
            return Err(error);
        }
        let shared = Arc::clone(&self.shared);
        let actor = Arc::clone(&self.actor);
        let notifications = self.notifications.clone();
        tokio::spawn(async move {
            run_actor(
                process,
                receiver,
                notifications,
                Arc::clone(&shared),
                actor,
                generation,
                verified.envelope.execution_domain_id,
            )
            .await;
        });
        Ok(())
    }

    /// Execute one manifest-filtered renderer intent after supervisor-owned field construction.
    ///
    /// # Errors
    ///
    /// Returns an error when the supervisor is unavailable, request construction fails, or the host rejects the operation.
    #[allow(clippy::too_many_lines)]
    pub async fn execute_renderer_operation(
        &self,
        invocation: bridge::DesktopHostInvocation,
    ) -> Result<bridge::DesktopHostOperationDelivery, SupervisorError> {
        let bridge::DesktopHostInvocation {
            operation_id,
            operation,
        } = invocation;
        validate_operation_id(&operation_id.0)?;
        let (generation, execution_domain) = self.shared.ready_domain()?;
        let fingerprint = operation_fingerprint(&execution_domain, &operation)?;
        let wire_cursor = self.shared.resolve_page_cursor(
            operation.page_token(),
            generation,
            &execution_domain,
            &fingerprint,
        )?;
        let mutation = operation.requires_idempotency();
        let _mutation_guard = if mutation {
            Some(self.mutation_gate.lock().await)
        } else {
            None
        };
        let admission = if mutation {
            Some(self.shared.admit_operation(
                &operation_id.0,
                &execution_domain,
                &fingerprint,
                &operation,
            )?)
        } else {
            None
        };
        let idempotency_key = admission.as_ref().map_or_else(
            || "desktop-read-only".to_string(),
            |(identity, _)| identity.0.clone(),
        );
        let prepared = (|| {
            let supervisor = bridge::build_supervisor_fields(
                &operation,
                &idempotency_key,
                wire_cursor.as_deref(),
            )
            .map_err(|_| {
                SupervisorError::new(
                    SupervisorErrorCode::Internal,
                    "host request construction failed",
                )
            })?;
            let complete = bridge::build_complete_host_request(
                operation,
                supervisor,
                bridge::SupervisorRequestContext {
                    request_id: self.next_request_id(generation),
                    execution_domain: execution_domain.clone(),
                },
            )
            .map_err(|_| {
                SupervisorError::new(
                    SupervisorErrorCode::InvalidConfiguration,
                    "host operation is invalid",
                )
            })?;
            if complete.execution_domain != execution_domain {
                return Err(SupervisorError::new(
                    SupervisorErrorCode::InvalidConfiguration,
                    "workspace routing changed before request admission",
                ));
            }
            Ok(complete)
        })();
        let complete = match prepared {
            Ok(complete) => complete,
            Err(error) => {
                if let Some((_, Some(record))) = &admission {
                    self.shared.rollback_operation(record);
                }
                return Err(error);
            }
        };
        let acknowledgement_token = if let Some((identity, new_record)) = admission {
            let (_, token) = self
                .persist_admitted_operation(identity, new_record)
                .await?;
            Some(bridge::DesktopHostOperationAcknowledgementToken(token))
        } else {
            None
        };
        let result = match self
            .execute_fenced(complete.request, generation, &execution_domain)
            .await
        {
            Ok(result) => result,
            Err(mut error) => {
                if error.code == SupervisorErrorCode::Remote
                    && error.retryable == Some(false)
                    && error.reconciliation_required == Some(false)
                {
                    error.operation_acknowledgement_token =
                        acknowledgement_token.as_ref().map(|token| token.0.clone());
                }
                return Err(error);
            }
        };
        let continuation = pagination_continuation(&result)?;
        let next_page_token = match continuation {
            Some(cursor) => Some(self.shared.issue_page_token(
                generation,
                &execution_domain,
                &fingerprint,
                cursor,
            )?),
            None => None,
        };
        let result = bridge::project_host_result(result, next_page_token).map_err(|_| {
            SupervisorError::new(
                SupervisorErrorCode::Internal,
                "host result projection failed",
            )
        })?;
        Ok(bridge::DesktopHostOperationDelivery {
            acknowledgement_token,
            result,
        })
    }

    /// Return unresolved mutation handles for explicit retry/reconciliation after restart.
    ///
    /// # Errors
    ///
    /// Returns an error when the durable ledger is unavailable or invalid.
    pub fn pending_renderer_operations(
        &self,
    ) -> Result<Vec<bridge::DesktopHostInvocation>, SupervisorError> {
        self.shared.pending_operations()
    }

    /// Retire one mutation identity only after the renderer application confirms its result.
    ///
    /// # Errors
    ///
    /// Returns an error when the token is invalid or durable removal cannot be completed.
    pub async fn acknowledge_renderer_operation(
        &self,
        acknowledgement_token: &bridge::DesktopHostOperationAcknowledgementToken,
    ) -> Result<(), SupervisorError> {
        let _guard = self.mutation_gate.lock().await;
        let Some(record) = self
            .shared
            .operation_for_acknowledgement(&acknowledgement_token.0)?
        else {
            // A repeated acknowledgement after a lost IPC response is idempotent.
            return Ok(());
        };
        if record.state == DurableOperationState::Retired {
            return Ok(());
        }
        let root = self.shared.storage_root()?;
        let mut retired = record.clone();
        retired.state = DurableOperationState::Retired;
        retired.operation = None;
        retired.retired_at_unix_ms = Some(self.shared.next_retired_at_unix_ms()?);
        let pruned_operation_ids = self.shared.retired_operation_ids_after(&retired)?;
        let persisted = retired.clone();
        let persisted_pruned_operation_ids = pruned_operation_ids.clone();
        tokio::task::spawn_blocking(move || {
            replace_durable_operation(&root, &persisted)?;
            remove_durable_operations(&root, &persisted_pruned_operation_ids)
        })
        .await
        .map_err(|_| SupervisorError::transport())??;
        self.shared
            .retire_operation(&record, retired, &pruned_operation_ids)
    }

    #[cfg(test)]
    async fn durable_operation_identity(
        &self,
        operation_id: &str,
        execution_domain: &str,
        fingerprint: &str,
        operation: &bridge::DesktopHostOperation,
    ) -> Result<(String, String), SupervisorError> {
        let (identity, new_record) =
            self.shared
                .admit_operation(operation_id, execution_domain, fingerprint, operation)?;
        self.persist_admitted_operation(identity, new_record).await
    }

    async fn persist_admitted_operation(
        &self,
        identity: (String, String),
        new_record: Option<DurableOperationRecord>,
    ) -> Result<(String, String), SupervisorError> {
        let Some(record) = new_record else {
            return Ok(identity);
        };
        let root = match self.shared.storage_root() {
            Ok(root) => root,
            Err(error) => {
                self.shared.rollback_operation(&record);
                return Err(error);
            }
        };
        let persisted = tokio::task::spawn_blocking({
            let record = record.clone();
            move || persist_durable_operation(&root, &record)
        })
        .await;
        match persisted {
            Ok(Ok(())) => Ok(identity),
            Ok(Err(error)) => {
                self.shared.rollback_operation(&record);
                Err(error)
            }
            Err(_) => {
                self.shared.rollback_operation(&record);
                Err(SupervisorError::transport())
            }
        }
    }

    pub(crate) fn event_origin(&self) -> Result<String, SupervisorError> {
        self.shared.ready_domain().map(|(_, domain)| domain)
    }

    pub(crate) async fn replay_run_event_page(
        &self,
        scope: &bridge::DesktopHostEventScope,
        cursor: Option<String>,
    ) -> Result<BackendHostEventPage, SupervisorError> {
        let (generation, execution_domain) = self.shared.ready_domain()?;
        let view = desktop_event_view(scope)?;
        let wire_cursor = cursor
            .as_deref()
            .map(host::HostEventCursor::new)
            .transpose()
            .map_err(|_| SupervisorError::invalid_configuration("event cursor is invalid"))?;
        let request = host::HostRequest {
            id: host::RequestId::new(self.next_request_id(generation)).map_err(|_| {
                SupervisorError::new(SupervisorErrorCode::Internal, "request identity failed")
            })?,
            call: host::HostCall::EventsReplay(host::EventsReplayParams {
                cursor: wire_cursor,
                limit: 500,
                view,
            }),
        };
        let host::HostResult::EventsReplay(result) = self
            .send_actor_fenced(request, generation, &execution_domain)
            .await?
        else {
            return Err(SupervisorError::transport());
        };
        if result.has_more && cursor.as_deref() == Some(result.next_cursor.as_str()) {
            return Err(SupervisorError::transport());
        }
        let mut deliveries = Vec::with_capacity(result.deliveries.len());
        for (index, delivery) in result.deliveries.into_iter().enumerate() {
            let sequence = u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1);
            deliveries.push(backend_event_from_delivery(delivery, sequence)?);
        }
        Ok(BackendHostEventPage {
            deliveries,
            next_cursor: result.next_cursor.as_str().to_string(),
            has_more: result.has_more,
            generation,
            execution_domain,
        })
    }

    pub(crate) async fn open_run_event_tail(
        &self,
        scope: &bridge::DesktopHostEventScope,
        cursor: Option<String>,
    ) -> Result<RunEventTail, SupervisorError> {
        let (generation, execution_domain) = self.shared.ready_domain()?;
        let wire_cursor = cursor
            .as_deref()
            .map(host::HostEventCursor::new)
            .transpose()
            .map_err(|_| SupervisorError::invalid_configuration("event cursor is invalid"))?;
        let request = host::HostRequest {
            id: host::RequestId::new(self.next_request_id(generation)).map_err(|_| {
                SupervisorError::new(SupervisorErrorCode::Internal, "request identity failed")
            })?,
            call: host::HostCall::EventsSubscribe(host::EventsSubscribeParams {
                cursor: wire_cursor,
                view: desktop_event_view(scope)?,
            }),
        };
        let host::HostResult::EventsSubscribe(result) = self
            .send_actor_fenced(request, generation, &execution_domain)
            .await?
        else {
            return Err(SupervisorError::transport());
        };
        Ok(RunEventTail {
            subscription_id: result.subscription_id,
            generation,
            execution_domain,
        })
    }

    pub(crate) async fn close_event_tail(
        &self,
        subscription_id: host::SubscriptionId,
        generation: u64,
        execution_domain: &str,
    ) -> Result<(), SupervisorError> {
        if self.status().state != HostChildState::Ready {
            return Ok(());
        }
        let request = host::HostRequest {
            id: host::RequestId::new(self.next_request_id(generation)).map_err(|_| {
                SupervisorError::new(SupervisorErrorCode::Internal, "request identity failed")
            })?,
            call: host::HostCall::EventsUnsubscribe(host::EventsUnsubscribeParams {
                subscription_id,
            }),
        };
        let host::HostResult::EventsUnsubscribe(_) = self
            .send_actor_fenced(request, generation, execution_domain)
            .await?
        else {
            return Err(SupervisorError::transport());
        };
        Ok(())
    }

    /// Send a coordinated generated shutdown request and wait for the actor barrier.
    ///
    /// # Errors
    ///
    /// Returns an error when the host is not ready or the shutdown barrier cannot be completed.
    pub async fn shutdown(&self) -> Result<(), SupervisorError> {
        let (generation, execution_domain) = match self.shared.request_shutdown()? {
            ShutdownAdmission::Terminal => return Ok(()),
            ShutdownAdmission::Wait { generation } => {
                self.wait_for_generation_exit(generation).await;
                return matches!(self.status().state, HostChildState::Stopped)
                    .then_some(())
                    .ok_or_else(SupervisorError::transport);
            }
            ShutdownAdmission::Actor {
                generation,
                execution_domain,
            } => (generation, execution_domain),
        };
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        let request = host::HostRequest {
            id: host::RequestId::new(self.next_request_id(generation)).map_err(|_| {
                SupervisorError::new(SupervisorErrorCode::Internal, "request identity failed")
            })?,
            call: host::HostCall::Shutdown(host::ShutdownParams {
                deadline_ms: u32::try_from(HOST_SHUTDOWN_DRAIN_TIMEOUT.as_millis())
                    .unwrap_or(u32::MAX),
            }),
        };
        let sender = self.actor.lock().await.take();
        let Some(sender) = sender else {
            self.wait_for_generation_exit(generation).await;
            return matches!(self.status().state, HostChildState::Stopped)
                .then_some(())
                .ok_or_else(SupervisorError::transport);
        };
        let (response_tx, response_rx) = oneshot::channel();
        let enqueue = timeout(
            deadline.saturating_duration_since(Instant::now()),
            sender.send(ActorCommand::Shutdown {
                expected_generation: generation,
                expected_domain: execution_domain,
                deadline,
                request,
                response: response_tx,
            }),
        )
        .await;
        if !matches!(enqueue, Ok(Ok(()))) {
            drop(sender);
            self.wait_for_generation_exit(generation).await;
            return Err(SupervisorError::transport());
        }
        let response = response_rx.await;
        drop(sender);
        let result = response.map_err(|_| SupervisorError::transport())??;
        if !matches!(result, host::HostResult::Shutdown(_)) {
            return Err(SupervisorError::transport());
        }
        Ok(())
    }

    async fn execute_fenced(
        &self,
        request: host::HostRequest,
        expected_generation: u64,
        expected_domain: &str,
    ) -> Result<host::HostResult, SupervisorError> {
        if matches!(
            request.call.method(),
            host::Method::Initialize | host::Method::Shutdown
        ) {
            return Err(SupervisorError::new(
                SupervisorErrorCode::InvalidConfiguration,
                "renderer cannot execute host lifecycle methods",
            ));
        }
        self.send_actor_fenced(request, expected_generation, expected_domain)
            .await
    }

    #[cfg(test)]
    async fn send_actor(
        &self,
        request: host::HostRequest,
    ) -> Result<host::HostResult, SupervisorError> {
        let (expected_generation, expected_domain) = self.shared.ready_domain()?;
        self.send_actor_fenced(request, expected_generation, &expected_domain)
            .await
    }

    async fn send_actor_fenced(
        &self,
        request: host::HostRequest,
        expected_generation: u64,
        expected_domain: &str,
    ) -> Result<host::HostResult, SupervisorError> {
        let sender = self
            .actor
            .lock()
            .await
            .clone()
            .ok_or_else(SupervisorError::not_ready)?;
        let (response_tx, response_rx) = oneshot::channel();
        let send = sender.send(ActorCommand::Execute {
            expected_generation,
            expected_domain: expected_domain.to_string(),
            request,
            response: response_tx,
        });
        timeout(REQUEST_TIMEOUT, send)
            .await
            .map_err(|_| SupervisorError::transport())?
            .map_err(|_| SupervisorError::transport())?;
        response_rx
            .await
            .map_err(|_| SupervisorError::transport())?
    }

    async fn wait_for_generation_exit(&self, generation: u64) {
        loop {
            let status = self.status();
            if status.generation != generation
                || matches!(
                    status.state,
                    HostChildState::Stopped | HostChildState::Failed | HostChildState::Incompatible
                )
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn next_request_id(&self, generation: u64) -> String {
        let sequence = self.next_request.fetch_add(1, Ordering::Relaxed);
        format!("desktop-{generation}-{sequence}")
    }
}

fn desktop_event_view(
    scope: &bridge::DesktopHostEventScope,
) -> Result<host::EventViewRequest, SupervisorError> {
    let session_id = host::SessionId::new(scope.session_id.0.clone()).map_err(|_| {
        SupervisorError::new(
            SupervisorErrorCode::InvalidConfiguration,
            "invalid event scope",
        )
    })?;
    let run_id = host::RunId::new(scope.run_id.0.clone()).map_err(|_| {
        SupervisorError::new(
            SupervisorErrorCode::InvalidConfiguration,
            "invalid event scope",
        )
    })?;
    Ok(host::EventViewRequest {
        optional_features: vec![
            "clarifications".to_string(),
            "hitl".to_string(),
            "runs".to_string(),
        ],
        profile: host::EventProfile::DesktopConversationV1,
        scope: host::ResourceScope::RunResourceScope(host::RunResourceScope {
            kind: host::RunResourceScopeKind::Value,
            run_id,
            session_id,
        }),
    })
}

fn backend_event_from_delivery(
    delivery: host::EventDelivery,
    sequence: u64,
) -> Result<BackendHostEvent, SupervisorError> {
    let cursor = delivery.cursor.as_str().to_string();
    let event_id = delivery.record.event_id.as_str().to_string();
    let notification = host::HostNotification {
        params: host::HostNotificationParams::HostEvent(Box::new(
            host::HostEventNotificationParams {
                delivery,
                delivery_sequence: host::DecimalU64::new(sequence),
                subscription_id: host::SubscriptionId::new("desktop-projection")
                    .map_err(|_| SupervisorError::transport())?,
            },
        )),
    };
    let event = bridge::project_host_notification(notification).map_err(|_| {
        SupervisorError::new(
            SupervisorErrorCode::Internal,
            "host event projection failed",
        )
    })?;
    Ok(BackendHostEvent {
        event,
        cursor,
        event_id,
    })
}

pub(crate) fn backend_event_from_notification(
    notification: host::HostNotification,
) -> Result<BackendHostEvent, SupervisorError> {
    let host::HostNotificationParams::HostEvent(params) = notification.params else {
        return Err(SupervisorError::transport());
    };
    backend_event_from_delivery(params.delivery, params.delivery_sequence.get())
}

fn validate_operation_id(operation_id: &str) -> Result<uuid::Uuid, SupervisorError> {
    let value = operation_id.strip_prefix("desktop-op-v1-").ok_or_else(|| {
        SupervisorError::new(
            SupervisorErrorCode::InvalidConfiguration,
            "logical operation identity is invalid",
        )
    })?;
    let parsed = uuid::Uuid::parse_str(value).map_err(|_| {
        SupervisorError::new(
            SupervisorErrorCode::InvalidConfiguration,
            "logical operation identity is invalid",
        )
    })?;
    if parsed.get_version() != Some(uuid::Version::Random) || parsed.to_string() != value {
        return Err(SupervisorError::new(
            SupervisorErrorCode::InvalidConfiguration,
            "logical operation identity is invalid",
        ));
    }
    Ok(parsed)
}

fn validate_operation_acknowledgement_token(token: &str) -> Result<uuid::Uuid, SupervisorError> {
    let value = token
        .strip_prefix("desktop-operation-ack-v1-")
        .ok_or_else(|| {
            SupervisorError::new(
                SupervisorErrorCode::InvalidConfiguration,
                "operation acknowledgement token is invalid",
            )
        })?;
    let parsed = uuid::Uuid::parse_str(value).map_err(|_| {
        SupervisorError::new(
            SupervisorErrorCode::InvalidConfiguration,
            "operation acknowledgement token is invalid",
        )
    })?;
    if parsed.get_version() != Some(uuid::Version::Random) || parsed.to_string() != value {
        return Err(SupervisorError::new(
            SupervisorErrorCode::InvalidConfiguration,
            "operation acknowledgement token is invalid",
        ));
    }
    Ok(parsed)
}

fn operation_records_path(root: &Path) -> PathBuf {
    root.join("operations-v1")
}

fn current_unix_time_ms() -> Result<u64, SupervisorError> {
    let milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| SupervisorError::transport())?
        .as_millis();
    u64::try_from(milliseconds).map_err(|_| SupervisorError::transport())
}

fn retired_operation_ids_to_prune(
    records: &HashMap<String, DurableOperationRecord>,
) -> Result<Vec<String>, SupervisorError> {
    let mut retired = records
        .values()
        .filter(|record| record.state == DurableOperationState::Retired)
        .map(|record| {
            let bytes = serde_json::to_vec(record)
                .map_err(|_| SupervisorError::transport())?
                .len();
            Ok((
                record.retired_at_unix_ms.unwrap_or(0),
                record.operation_id.clone(),
                bytes,
            ))
        })
        .collect::<Result<Vec<_>, SupervisorError>>()?;
    retired.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
    let mut retained_count = retired.len();
    let mut retained_bytes = retired.iter().try_fold(0_usize, |total, entry| {
        total
            .checked_add(entry.2)
            .ok_or_else(SupervisorError::transport)
    })?;
    let mut pruned = Vec::new();
    for (_, operation_id, bytes) in retired {
        if retained_count <= MAX_RETIRED_OPERATION_TOMBSTONES
            && retained_bytes <= MAX_RETIRED_OPERATION_TOMBSTONE_BYTES
        {
            break;
        }
        retained_count -= 1;
        retained_bytes -= bytes;
        pruned.push(operation_id);
    }
    Ok(pruned)
}

fn remove_durable_operations(root: &Path, operation_ids: &[String]) -> Result<(), SupervisorError> {
    if operation_ids.is_empty() {
        return Ok(());
    }
    let records_path = operation_records_path(root);
    for operation_id in operation_ids {
        validate_operation_id(operation_id)?;
        let path = records_path.join(format!("{operation_id}.json"));
        if let Err(error) = std::fs::remove_file(path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(SupervisorError::transport());
        }
    }
    #[cfg(unix)]
    File::open(&records_path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| SupervisorError::transport())?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn load_durable_operations(
    root: &Path,
) -> Result<HashMap<String, DurableOperationRecord>, SupervisorError> {
    std::fs::create_dir_all(root).map_err(|_| SupervisorError::transport())?;
    let records_path = operation_records_path(root);
    std::fs::create_dir_all(&records_path).map_err(|_| SupervisorError::transport())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| SupervisorError::transport())?;
        std::fs::set_permissions(&records_path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| SupervisorError::transport())?;
        let metadata =
            std::fs::symlink_metadata(&records_path).map_err(|_| SupervisorError::transport())?;
        if !metadata.file_type().is_dir() || metadata.mode() & 0o077 != 0 {
            return Err(SupervisorError::new(
                SupervisorErrorCode::InvalidConfiguration,
                "operation ledger permissions are unsafe",
            ));
        }
    }
    let mut records = HashMap::new();
    let mut ledger_bytes = 0_usize;
    let mut removed_unpublished = false;
    for entry in std::fs::read_dir(&records_path).map_err(|_| SupervisorError::transport())? {
        let entry = entry.map_err(|_| SupervisorError::transport())?;
        let file_type = entry
            .file_type()
            .map_err(|_| SupervisorError::transport())?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(".pending-"))
        {
            if !file_type.is_file() {
                return Err(SupervisorError::new(
                    SupervisorErrorCode::InvalidConfiguration,
                    "operation ledger contains an invalid unpublished entry",
                ));
            }
            std::fs::remove_file(entry.path()).map_err(|_| SupervisorError::transport())?;
            removed_unpublished = true;
            continue;
        }
        if !file_type.is_file() {
            return Err(SupervisorError::new(
                SupervisorErrorCode::InvalidConfiguration,
                "operation ledger contains an invalid entry",
            ));
        }
        let metadata = entry.metadata().map_err(|_| SupervisorError::transport())?;
        let file_bytes = usize::try_from(metadata.len()).map_err(|_| {
            SupervisorError::new(
                SupervisorErrorCode::InvalidConfiguration,
                "operation ledger entry is invalid",
            )
        })?;
        ledger_bytes = ledger_bytes.checked_add(file_bytes).ok_or_else(|| {
            SupervisorError::new(
                SupervisorErrorCode::InvalidConfiguration,
                "operation ledger is invalid",
            )
        })?;
        if file_bytes > MAX_DURABLE_OPERATION_RECORD_BYTES
            || ledger_bytes > MAX_DURABLE_OPERATION_LEDGER_BYTES
        {
            return Err(SupervisorError::new(
                SupervisorErrorCode::InvalidConfiguration,
                "operation ledger contains an invalid entry",
            ));
        }
        let bytes = std::fs::read(entry.path()).map_err(|_| SupervisorError::transport())?;
        if bytes.len() != file_bytes {
            return Err(SupervisorError::new(
                SupervisorErrorCode::InvalidConfiguration,
                "operation ledger entry changed while loading",
            ));
        }
        let record: DurableOperationRecord = serde_json::from_slice(&bytes).map_err(|_| {
            SupervisorError::new(
                SupervisorErrorCode::InvalidConfiguration,
                "operation ledger entry is invalid",
            )
        })?;
        validate_operation_id(&record.operation_id)?;
        let operation_valid = match record.state {
            DurableOperationState::Pending => {
                record.retired_at_unix_ms.is_none()
                    && record
                        .operation
                        .clone()
                        .and_then(|operation| {
                            serde_json::from_value::<bridge::DesktopHostOperation>(operation).ok()
                        })
                        .filter(bridge::DesktopHostOperation::requires_idempotency)
                        .and_then(|operation| {
                            operation_fingerprint(&record.execution_domain, &operation).ok()
                        })
                        .as_deref()
                        == Some(record.fingerprint.as_str())
            }
            DurableOperationState::Retired => record.operation.is_none(),
        };
        if record.schema_version != 1
            || entry.file_name() != OsString::from(format!("{}.json", record.operation_id))
            || !record.fingerprint.starts_with("sha256:")
            || record.fingerprint.len() != 71
            || record.execution_domain.is_empty()
            || record.idempotency_key.is_empty()
            || !operation_valid
            || validate_operation_acknowledgement_token(&record.acknowledgement_token).is_err()
            || records
                .insert(record.operation_id.clone(), record)
                .is_some()
        {
            return Err(SupervisorError::new(
                SupervisorErrorCode::InvalidConfiguration,
                "operation ledger entry is invalid",
            ));
        }
        if records
            .values()
            .filter(|record| record.state == DurableOperationState::Pending)
            .count()
            > MAX_UNCERTAIN_OPERATIONS
        {
            return Err(SupervisorError::new(
                SupervisorErrorCode::NotReady,
                "operation reconciliation is required before accepting more mutations",
            ));
        }
    }
    #[cfg(unix)]
    if removed_unpublished {
        File::open(&records_path)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| SupervisorError::transport())?;
    }
    #[cfg(not(unix))]
    let _ = removed_unpublished;
    let pruned_operation_ids = retired_operation_ids_to_prune(&records)?;
    remove_durable_operations(root, &pruned_operation_ids)?;
    for operation_id in pruned_operation_ids {
        records.remove(&operation_id);
    }
    Ok(records)
}

fn persist_durable_operation(
    root: &Path,
    record: &DurableOperationRecord,
) -> Result<(), SupervisorError> {
    publish_durable_operation(root, record, false)
}

fn replace_durable_operation(
    root: &Path,
    record: &DurableOperationRecord,
) -> Result<(), SupervisorError> {
    publish_durable_operation(root, record, true)
}

#[cfg(windows)]
fn persist_windows_tempfile_write_through(
    temporary: tempfile::NamedTempFile,
    destination: &Path,
    replace: bool,
) -> std::io::Result<()> {
    // `keep` normalizes tempfile's FILE_ATTRIBUTE_TEMPORARY before publication. Atomicwrites owns
    // the audited unsafe Windows boundary and adds MOVEFILE_WRITE_THROUGH to the atomic rename.
    let (temporary_file, temporary_path) = temporary.keep().map_err(|error| error.error)?;
    let result = if replace {
        atomicwrites::replace_atomic(&temporary_path, destination)
    } else {
        atomicwrites::move_atomic(&temporary_path, destination)
    };
    drop(temporary_file);
    if result.is_err() {
        let _ = std::fs::remove_file(temporary_path);
    }
    result
}

fn publish_durable_operation(
    root: &Path,
    record: &DurableOperationRecord,
    replace: bool,
) -> Result<(), SupervisorError> {
    validate_operation_id(&record.operation_id)?;
    let records_path = operation_records_path(root);
    let path = records_path.join(format!("{}.json", record.operation_id));
    let bytes = serde_json::to_vec(record).map_err(|_| SupervisorError::transport())?;
    if bytes.len() > MAX_DURABLE_OPERATION_RECORD_BYTES {
        return Err(SupervisorError::new(
            SupervisorErrorCode::NotReady,
            "operation reconciliation is required before accepting more mutations",
        ));
    }
    let mut temporary = tempfile::Builder::new()
        .prefix(".pending-")
        .tempfile_in(&records_path)
        .map_err(|_| SupervisorError::transport())?;
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
    #[cfg(windows)]
    persist_windows_tempfile_write_through(temporary, &path, replace).map_err(|error| {
        if !replace && error.kind() == std::io::ErrorKind::AlreadyExists {
            SupervisorError::new(
                SupervisorErrorCode::InvalidConfiguration,
                "logical operation identity already exists on disk",
            )
        } else {
            SupervisorError::transport()
        }
    })?;
    #[cfg(not(windows))]
    if replace {
        temporary
            .persist(&path)
            .map_err(|_| SupervisorError::transport())?;
    } else {
        temporary.persist_noclobber(&path).map_err(|error| {
            if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                SupervisorError::new(
                    SupervisorErrorCode::InvalidConfiguration,
                    "logical operation identity already exists on disk",
                )
            } else {
                SupervisorError::transport()
            }
        })?;
    }
    #[cfg(unix)]
    File::open(&records_path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| SupervisorError::transport())?;
    Ok(())
}

fn operation_fingerprint(
    execution_domain: &str,
    operation: &bridge::DesktopHostOperation,
) -> Result<String, SupervisorError> {
    let mut value = serde_json::to_value(operation).map_err(|_| {
        SupervisorError::new(
            SupervisorErrorCode::Internal,
            "host operation identity could not be encoded",
        )
    })?;
    if let Some(input) = value
        .get_mut("input")
        .and_then(serde_json::Value::as_object_mut)
    {
        input.remove("pageToken");
    }
    let canonical = serde_json::to_vec(&value).map_err(|_| {
        SupervisorError::new(
            SupervisorErrorCode::Internal,
            "host operation identity could not be encoded",
        )
    })?;
    let mut digest = Sha256::new();
    digest.update(b"starweaver.desktop.operation.v1\0");
    digest.update(execution_domain.as_bytes());
    digest.update(b"\0");
    digest.update(canonical);
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn pagination_continuation(result: &host::HostResult) -> Result<Option<String>, SupervisorError> {
    match result {
        host::HostResult::ApprovalList(value) => pagination_from_value(value),
        host::HostResult::DeferredList(value) => pagination_from_value(value),
        host::HostResult::EnvironmentList(value) => pagination_from_value(value),
        host::HostResult::SessionList(value) => pagination_from_value(value),
        host::HostResult::SessionSearch(value) => pagination_from_value(value),
        _ => Ok(None),
    }
}

fn pagination_from_value<T: Serialize>(value: &T) -> Result<Option<String>, SupervisorError> {
    let value = serde_json::to_value(value).map_err(|_| SupervisorError::transport())?;
    let page = value
        .get("page")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(SupervisorError::transport)?;
    let has_more = page
        .get("hasMore")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(SupervisorError::transport)?;
    let cursor = page
        .get("nextCursor")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    match (has_more, cursor) {
        (false, _) => Ok(None),
        (true, Some(cursor)) => Ok(Some(cursor)),
        (true, None) => Err(SupervisorError::transport()),
    }
}

struct ChildProcess {
    child: Box<dyn ChildWrapper>,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    _staging: Option<StagedLaunch>,
}

fn verify_launch(spec: LocalLaunchSpec) -> Result<VerifiedLaunch, SupervisorError> {
    if !spec.runtime_path.is_absolute() || !spec.launch_envelope_path.is_absolute() {
        return Err(SupervisorError::new(
            SupervisorErrorCode::InvalidConfiguration,
            "managed runtime and launch envelope must use absolute paths",
        ));
    }
    let runtime_path = std::fs::canonicalize(&spec.runtime_path).map_err(|_| {
        SupervisorError::new(
            SupervisorErrorCode::InvalidConfiguration,
            "managed runtime is unavailable",
        )
    })?;
    if !runtime_path.is_file() || sha256_file(&runtime_path)? != spec.runtime_digest {
        return Err(SupervisorError::incompatible());
    }
    let runtime_identity = file_identity(&runtime_path)?;
    let launch_envelope_path = std::fs::canonicalize(&spec.launch_envelope_path).map_err(|_| {
        SupervisorError::new(
            SupervisorErrorCode::InvalidConfiguration,
            "managed launch envelope is unavailable",
        )
    })?;
    let bytes = std::fs::read(&launch_envelope_path).map_err(|_| {
        SupervisorError::new(
            SupervisorErrorCode::InvalidConfiguration,
            "managed launch envelope is unavailable",
        )
    })?;
    if bytes.len() > 1024 * 1024 {
        return Err(SupervisorError::new(
            SupervisorErrorCode::InvalidConfiguration,
            "managed launch envelope is too large",
        ));
    }
    let envelope_digest = sha256_bytes(&bytes);
    if envelope_digest != spec.launch_envelope_digest {
        return Err(SupervisorError::incompatible());
    }
    let envelope_identity = file_identity(&launch_envelope_path)?;
    let envelope = host::decode_launch_envelope(&bytes).map_err(|_| {
        SupervisorError::new(
            SupervisorErrorCode::InvalidConfiguration,
            "managed launch envelope is invalid",
        )
    })?;
    if envelope.capability_caps.native_local_shell
        || envelope.schema.version != host::LAUNCH_SCHEMA_VERSION
        || envelope.configuration_generation.get() != spec.configuration_generation
        || envelope.execution_domain_id != spec.execution_domain_id
        || envelope.workspace.identity != spec.workspace_identity
    {
        return Err(SupervisorError::incompatible());
    }
    for path in [
        &envelope.database.path,
        &envelope.workspace.root,
        &envelope.state_directory,
    ] {
        if !Path::new(path).is_absolute() {
            return Err(SupervisorError::new(
                SupervisorErrorCode::InvalidConfiguration,
                "managed launch envelope contains a relative authority path",
            ));
        }
    }
    let credential_environment = envelope
        .providers
        .iter()
        .filter(|provider| provider.enabled)
        .filter_map(|provider| provider.credential_env.clone())
        .collect::<Vec<_>>();
    if credential_environment
        .iter()
        .any(|name| !allowed_credential_environment(name))
    {
        return Err(SupervisorError::new(
            SupervisorErrorCode::InvalidConfiguration,
            "managed launch envelope requests a reserved process environment variable",
        ));
    }
    Ok(VerifiedLaunch {
        runtime_path,
        runtime_digest: spec.runtime_digest,
        launch_envelope_path,
        envelope,
        envelope_digest,
        credential_environment,
        runtime_identity,
        envelope_identity,
        runtime_version: spec.runtime_version,
        build_revision: spec.build_revision,
        target: spec.target,
    })
}

fn file_identity(path: &Path) -> Result<FileIdentity, SupervisorError> {
    let canonical_path = std::fs::canonicalize(path).map_err(|_| {
        SupervisorError::new(
            SupervisorErrorCode::InvalidConfiguration,
            "managed file identity is unavailable",
        )
    })?;
    let metadata = std::fs::metadata(&canonical_path).map_err(|_| {
        SupervisorError::new(
            SupervisorErrorCode::InvalidConfiguration,
            "managed file identity is unavailable",
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let parent = canonical_path
            .parent()
            .ok_or_else(SupervisorError::incompatible)?;
        let parent_metadata =
            std::fs::metadata(parent).map_err(|_| SupervisorError::incompatible())?;
        if metadata.mode() & 0o022 != 0
            || metadata.nlink() != 1
            || parent_metadata.mode() & 0o022 != 0
            || parent_metadata.uid() != metadata.uid()
        {
            return Err(SupervisorError::new(
                SupervisorErrorCode::InvalidConfiguration,
                "managed file or parent directory permissions are unsafe",
            ));
        }
        Ok(FileIdentity {
            canonical_path,
            length: metadata.len(),
            modified: metadata
                .modified()
                .map_err(|_| SupervisorError::incompatible())?,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        Ok(FileIdentity {
            canonical_path,
            length: metadata.len(),
            modified: metadata
                .modified()
                .map_err(|_| SupervisorError::incompatible())?,
        })
    }
}

fn sha256_file(path: &Path) -> Result<String, SupervisorError> {
    let mut file = File::open(path).map_err(|_| {
        SupervisorError::new(
            SupervisorErrorCode::InvalidConfiguration,
            "managed runtime is unavailable",
        )
    })?;
    sha256_open_file(&mut file)
}

fn sha256_open_file(file: &mut File) -> Result<String, SupervisorError> {
    file.rewind().map_err(|_| SupervisorError::incompatible())?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|_| {
            SupervisorError::new(
                SupervisorErrorCode::InvalidConfiguration,
                "managed runtime could not be verified",
            )
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    file.rewind().map_err(|_| SupervisorError::incompatible())?;
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

struct StagedLaunch {
    directory: tempfile::TempDir,
    runtime_path: PathBuf,
    envelope_path: PathBuf,
    _runtime_identity: FileIdentity,
    _envelope_identity: FileIdentity,
}

#[cfg(unix)]
impl Drop for StagedLaunch {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(
            self.directory.path(),
            std::fs::Permissions::from_mode(0o700),
        );
    }
}

fn stage_verified_launch(
    verified: &VerifiedLaunch,
    storage_root: &Path,
) -> Result<StagedLaunch, SupervisorError> {
    let staging_root = storage_root.join("runtime-staging");
    std::fs::create_dir_all(&staging_root).map_err(|_| SupervisorError::transport())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&staging_root, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| SupervisorError::transport())?;
    }
    let directory = tempfile::Builder::new()
        .prefix("child-")
        .tempdir_in(&staging_root)
        .map_err(|_| SupervisorError::transport())?;
    let runtime_name = if cfg!(windows) {
        "starweaver-rpc.exe"
    } else {
        "starweaver-rpc"
    };
    let runtime_path = directory.path().join(runtime_name);
    let envelope_path = directory.path().join("launch.json");
    std::fs::copy(&verified.runtime_path, &runtime_path)
        .map_err(|_| SupervisorError::transport())?;
    std::fs::copy(&verified.launch_envelope_path, &envelope_path)
        .map_err(|_| SupervisorError::transport())?;
    File::open(&runtime_path)
        .and_then(|file| file.sync_all())
        .map_err(|_| SupervisorError::transport())?;
    File::open(&envelope_path)
        .and_then(|file| file.sync_all())
        .map_err(|_| SupervisorError::transport())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&runtime_path, std::fs::Permissions::from_mode(0o500))
            .map_err(|_| SupervisorError::transport())?;
        std::fs::set_permissions(&envelope_path, std::fs::Permissions::from_mode(0o400))
            .map_err(|_| SupervisorError::transport())?;
    }
    if sha256_file(&runtime_path)? != verified.runtime_digest
        || std::fs::read(&envelope_path).map_or(true, |bytes| {
            bytes.len() > 1024 * 1024 || sha256_bytes(&bytes) != verified.envelope_digest
        })
    {
        return Err(SupervisorError::incompatible());
    }
    let runtime_identity = file_identity(&runtime_path)?;
    let envelope_identity = file_identity(&envelope_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o500))
            .map_err(|_| SupervisorError::transport())?;
    }
    Ok(StagedLaunch {
        directory,
        runtime_path,
        envelope_path,
        _runtime_identity: runtime_identity,
        _envelope_identity: envelope_identity,
    })
}

fn prepare_verified_launch(
    verified: &VerifiedLaunch,
    storage_root: &Path,
) -> Result<StagedLaunch, SupervisorError> {
    if file_identity(&verified.runtime_path)? != verified.runtime_identity
        || sha256_file(&verified.runtime_path)? != verified.runtime_digest
    {
        return Err(SupervisorError::incompatible());
    }
    let envelope_bytes = std::fs::read(&verified.launch_envelope_path)
        .map_err(|_| SupervisorError::incompatible())?;
    if file_identity(&verified.launch_envelope_path)? != verified.envelope_identity
        || envelope_bytes.len() > 1024 * 1024
        || sha256_bytes(&envelope_bytes) != verified.envelope_digest
    {
        return Err(SupervisorError::incompatible());
    }
    stage_verified_launch(verified, storage_root)
}

async fn spawn_verified_child(
    verified: &VerifiedLaunch,
    shared: &Arc<Shared>,
) -> Result<ChildProcess, SupervisorError> {
    let storage_root = shared.storage_root()?;
    let candidate = verified.clone();
    let staging =
        tokio::task::spawn_blocking(move || prepare_verified_launch(&candidate, &storage_root))
            .await
            .map_err(|_| SupervisorError::transport())??;
    let mut command = Command::new(&staging.runtime_path);
    command
        .arg("--launch-envelope")
        .arg(&staging.envelope_path)
        .arg("stdio")
        .current_dir(&verified.envelope.workspace.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env_clear();
    let mut credential_values = Vec::new();
    for name in safe_environment_names(&verified.credential_environment) {
        if let Some(value) = std::env::var_os(&name) {
            if verified
                .credential_environment
                .iter()
                .any(|credential| OsString::from(credential) == name)
            {
                let bytes = credential_bytes(&value);
                if !bytes.is_empty() {
                    credential_values.push(bytes);
                }
            }
            command.env(name, value);
        }
    }
    let mut command = CommandWrap::from(command);
    #[cfg(unix)]
    command.wrap(ProcessGroup::leader());
    #[cfg(windows)]
    command.wrap(JobObject);
    command.wrap(KillOnDrop);
    let mut child = command.spawn().map_err(|_| SupervisorError::transport())?;
    let Some(stdin) = child.stdin().take() else {
        terminate_wrapped_child(child.as_mut()).await;
        return Err(SupervisorError::transport());
    };
    let Some(stdout) = child.stdout().take() else {
        terminate_wrapped_child(child.as_mut()).await;
        return Err(SupervisorError::transport());
    };
    let Some(stderr) = child.stderr().take() else {
        terminate_wrapped_child(child.as_mut()).await;
        return Err(SupervisorError::transport());
    };
    let diagnostic_sink = Arc::clone(shared);
    tokio::spawn(async move {
        let mut stderr = stderr;
        let mut chunk = [0_u8; 4096];
        let mut redactor = StreamingSecretRedactor::new(credential_values);
        loop {
            match stderr.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let redacted = redactor.push(&chunk[..read]);
                    if let Ok(mut diagnostics) = diagnostic_sink.diagnostics.lock() {
                        diagnostics.append(&redacted);
                    }
                }
            }
        }
        let redacted = redactor.finish();
        if let Ok(mut diagnostics) = diagnostic_sink.diagnostics.lock() {
            diagnostics.append(&redacted);
        }
    });
    Ok(ChildProcess {
        child,
        stdin,
        stdout: BufReader::new(stdout),
        _staging: Some(staging),
    })
}

async fn terminate_wrapped_child(child: &mut dyn ChildWrapper) {
    let _ = child.start_kill();
    // process-wrap aggregate waits are not cancellation-safe after the leader exits.
    // Poll this one future to completion so the process-group / Job Object barrier remains whole.
    let _ = child.wait().await;
}

async fn terminate_and_reap(process: &mut ChildProcess) {
    terminate_wrapped_child(process.child.as_mut()).await;
}

struct StreamingSecretRedactor {
    pending: Vec<u8>,
    secrets: Vec<Vec<u8>>,
    overlap: usize,
}

impl StreamingSecretRedactor {
    fn new(mut secrets: Vec<Vec<u8>>) -> Self {
        secrets.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
        secrets.dedup();
        let overlap = secrets
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(1)
            .saturating_sub(1);
        Self {
            pending: Vec::new(),
            secrets,
            overlap,
        }
    }

    fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(chunk);
        redact_diagnostic_bytes(&mut self.pending, &self.secrets);
        let emit = self.pending.len().saturating_sub(self.overlap);
        self.pending.drain(..emit).collect()
    }

    fn finish(mut self) -> Vec<u8> {
        redact_diagnostic_bytes(&mut self.pending, &self.secrets);
        self.pending
    }
}

fn redact_diagnostic_bytes(output: &mut [u8], secrets: &[Vec<u8>]) {
    for secret in secrets {
        if secret.is_empty() || secret.len() > output.len() {
            continue;
        }
        let mut offset = 0;
        while let Some(index) = output[offset..]
            .windows(secret.len())
            .position(|window| window == secret)
        {
            let start = offset + index;
            output[start..start + secret.len()].fill(b'*');
            offset = start + secret.len();
        }
    }
}

#[cfg(unix)]
fn credential_bytes(value: &std::ffi::OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt as _;
    value.as_bytes().to_vec()
}

#[cfg(not(unix))]
fn credential_bytes(value: &std::ffi::OsStr) -> Vec<u8> {
    value.to_string_lossy().into_owned().into_bytes()
}

fn allowed_credential_environment(name: &str) -> bool {
    let canonical = name.to_ascii_uppercase();
    !canonical.starts_with("LD_")
        && !canonical.starts_with("DYLD_")
        && !canonical.starts_with("STARWEAVER_")
        && !matches!(canonical.as_str(), "PATH" | "COMSPEC")
}

fn safe_environment_names(credentials: &[String]) -> Vec<OsString> {
    let mut names = BTreeSet::<OsString>::new();
    for name in [
        "HOME",
        "USERPROFILE",
        "TMPDIR",
        "TEMP",
        "TMP",
        "SYSTEMROOT",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ] {
        names.insert(OsString::from(name));
    }
    names.extend(
        credentials
            .iter()
            .filter(|name| allowed_credential_environment(name))
            .map(OsString::from),
    );
    names.into_iter().collect()
}

async fn initialize_child(
    process: &mut ChildProcess,
    verified: &VerifiedLaunch,
    generation: u64,
) -> Result<host::InitializeResult, SupervisorError> {
    let features = desktop_features();
    let params = host::InitializeParams {
        client_info: host::ClientInfo {
            name: "starweaver-desktop".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        protocol: host::ProtocolIdentity {
            major: host::PROTOCOL_MAJOR,
            name: host::ProtocolIdentityName::Value,
            revision: host::PROTOCOL_REVISION.to_string(),
            schema_digest: host::SchemaDigest::new(host::SCHEMA_DIGEST)
                .map_err(|_| SupervisorError::incompatible())?,
        },
        required_features: features.clone(),
        supported_features: features,
    };
    let request = host::HostRequest {
        id: host::RequestId::new(format!("desktop-initialize-{generation}"))
            .map_err(|_| SupervisorError::incompatible())?,
        call: host::HostCall::Initialize(params),
    };
    let encoded =
        host::encode_request_frame(&request).map_err(|_| SupervisorError::incompatible())?;
    write_frame(&mut process.stdin, &encoded.bytes).await?;
    let frame = timeout(HANDSHAKE_TIMEOUT, read_frame(&mut process.stdout))
        .await
        .map_err(|_| SupervisorError::transport())??;
    let decoded = host::decode_server_frame(&frame, |id| {
        (id == &encoded.correlation.id).then_some(encoded.correlation.method)
    })
    .map_err(|_| SupervisorError::incompatible())?;
    let host::HostServerFrame::Response(response) = decoded else {
        return Err(SupervisorError::incompatible());
    };
    match response.result {
        Ok(host::HostResult::Initialize(result)) => {
            verify_initialize(&result, verified)?;
            Ok(result)
        }
        Ok(_) | Err(_) => Err(SupervisorError::incompatible()),
    }
}

fn verify_initialize(
    result: &host::InitializeResult,
    verified: &VerifiedLaunch,
) -> Result<(), SupervisorError> {
    let expected_features = desktop_features()
        .into_iter()
        .map(host::FeatureId::into_string)
        .collect::<BTreeSet<_>>();
    let negotiated = result
        .negotiated_features
        .iter()
        .map(|feature| feature.as_str().to_string())
        .collect::<BTreeSet<_>>();
    let supported = result
        .supported_features
        .iter()
        .map(|feature| feature.as_str().to_string())
        .collect::<BTreeSet<_>>();
    let storage_generation = DESKTOP_STORAGE_GENERATION;
    let compatible = result.protocol.major == host::PROTOCOL_MAJOR
        && result.protocol.revision == host::PROTOCOL_REVISION
        && result.protocol.schema_digest.as_str() == host::SCHEMA_DIGEST
        && result.runtime_status == "ready"
        && result.runtime_build.version == verified.runtime_version
        && result.runtime_build.build_revision == verified.build_revision
        && result.runtime_build.target == verified.target
        && result.launch.effective_schema.version == host::LAUNCH_SCHEMA_VERSION
        && result.launch.accepted_minimum_version <= host::LAUNCH_SCHEMA_VERSION
        && result.launch.accepted_maximum_version >= host::LAUNCH_SCHEMA_VERSION
        && result.launch.envelope_digest.as_str() == verified.envelope_digest
        && result.launch.configuration_generation.get()
            == verified.envelope.configuration_generation.get()
        && result.launch.mode == "workspace_execution"
        && result.workspace.execution_domain_id == verified.envelope.execution_domain_id
        && result.workspace.workspace_identity == verified.envelope.workspace.identity
        && result.storage.current_generation.get() == storage_generation
        && result.storage.maintenance_barrier_generation.get() == 0
        && result.storage.minimum_readable_generation.get() <= storage_generation
        && result.storage.maximum_readable_generation.get() >= storage_generation
        && result.storage.minimum_writable_generation.get() <= storage_generation
        && result.storage.maximum_writable_generation.get() >= storage_generation
        && negotiated == expected_features
        && expected_features.is_subset(&supported);
    compatible
        .then_some(())
        .ok_or_else(SupervisorError::incompatible)
}

fn desktop_features() -> Vec<host::FeatureId> {
    let features = DESKTOP_METHODS
        .iter()
        .flat_map(|method| method.metadata().features.iter().copied())
        .chain(
            DESKTOP_EVENT_CLASSES
                .iter()
                .filter_map(|class| class.metadata().feature),
        )
        .chain(host::Method::Shutdown.metadata().features.iter().copied())
        .collect::<BTreeSet<_>>();
    features
        .iter()
        .filter_map(|feature| host::FeatureId::new(*feature).ok())
        .collect()
}

#[allow(clippy::too_many_lines)]
fn run_actor(
    mut process: ChildProcess,
    mut receiver: mpsc::Receiver<ActorCommand>,
    notifications: broadcast::Sender<host::HostNotification>,
    shared: Arc<Shared>,
    actor: Arc<AsyncMutex<Option<mpsc::Sender<ActorCommand>>>>,
    generation: u64,
    execution_domain: String,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(async move {
        let mut pending = HashMap::<String, PendingRequest>::new();
        let mut expected_shutdown = false;
        let mut transport_failed = false;
        let mut shutdown_deadline = None;
        let mut completed_shutdown = None;
        let mut deferred_responses = Vec::new();
        loop {
            let pending_deadline = pending.values().map(|request| request.deadline).min();
            tokio::select! {
                command = receiver.recv(), if shutdown_deadline.is_none() => {
                    let Some(command) = command else { break; };
                    let (expected_generation, expected_domain, request, response, shutdown, command_deadline) = match command {
                        ActorCommand::Execute { expected_generation, expected_domain, request, response } => {
                            (expected_generation, expected_domain, request, response, false, None)
                        }
                        ActorCommand::Shutdown { expected_generation, expected_domain, deadline, request, response } => {
                            (expected_generation, expected_domain, request, response, true, Some(deadline))
                        }
                    };
                    if expected_generation != generation || expected_domain != execution_domain {
                        let _ = response.send(Err(SupervisorError::not_ready()));
                        continue;
                    }
                    if shutdown {
                        shutdown_deadline = command_deadline;
                    }
                    let expected_unsubscribe_subscription_id = match &request.call {
                        host::HostCall::EventsUnsubscribe(params) => {
                            Some(params.subscription_id.clone())
                        }
                        _ => None,
                    };
                    let Ok(encoded) = host::encode_request_frame(&request) else {
                        let error = Err(SupervisorError::new(
                            SupervisorErrorCode::Internal,
                            "host request encoding failed",
                        ));
                        if shutdown {
                            deferred_responses.push((response, error));
                            transport_failed = true;
                            break;
                        }
                        let _ = response.send(error);
                        continue;
                    };
                    let key = encoded.correlation.id.as_str().to_string();
                    if pending.contains_key(&key) {
                        deferred_responses.push((response, Err(SupervisorError::transport())));
                        transport_failed = true;
                        break;
                    }
                    let write_deadline = command_deadline.unwrap_or_else(|| Instant::now() + REQUEST_TIMEOUT);
                    pending.insert(key.clone(), PendingRequest {
                        method: encoded.correlation.method,
                        expected_unsubscribe_subscription_id,
                        response,
                        shutdown,
                        deadline: write_deadline,
                    });
                    if timeout(
                        write_deadline.saturating_duration_since(Instant::now()),
                        write_frame(&mut process.stdin, &encoded.bytes),
                    )
                    .await
                    .map_or(true, |result| result.is_err())
                    {
                        if let Some(pending) = pending.remove(&key) {
                            deferred_responses
                                .push((pending.response, Err(SupervisorError::transport())));
                        }
                        transport_failed = true;
                        break;
                    }
                    if shutdown {
                        receiver.close();
                        while let Ok(command) = receiver.try_recv() {
                            let response = match command {
                                ActorCommand::Execute { response, .. }
                                | ActorCommand::Shutdown { response, .. } => response,
                            };
                            deferred_responses
                                .push((response, Err(SupervisorError::transport())));
                        }
                    }
                }
                frame = read_frame(&mut process.stdout) => {
                    let Ok(frame) = frame else {
                        transport_failed = true;
                        break;
                    };
                    let decoded = host::decode_server_frame(&frame, |id| {
                        pending.get(id.as_str()).map(|pending| pending.method)
                    });
                    match decoded {
                        Ok(host::HostServerFrame::Notification(notification)) => {
                            match shared.admit_notification(generation, &notification) {
                                NotificationAdmission::Deliver => {
                                    let _ = notifications.send(notification);
                                }
                                NotificationAdmission::Ignore => {}
                                NotificationAdmission::Reject => {
                                    transport_failed = true;
                                    break;
                                }
                            }
                        }
                        Ok(host::HostServerFrame::Response(response)) => {
                            let key = response.correlation.id.as_str().to_string();
                            let Some(pending_request) = pending.remove(&key) else {
                                transport_failed = true;
                                break;
                            };
                            let (reply, state_failed) = response.result.map_or_else(
                                |error| (Err(SupervisorError::remote(&error)), false),
                                |result| {
                                    let state_result = match &result {
                                        host::HostResult::EventsSubscribe(result) => {
                                            shared.register_subscription(generation, &result.subscription_id)
                                        }
                                        host::HostResult::EventsUnsubscribe(result) => pending_request
                                            .expected_unsubscribe_subscription_id
                                            .as_ref()
                                            .filter(|expected| {
                                                expected.as_str() == result.subscription_id.as_str()
                                            })
                                            .ok_or_else(SupervisorError::transport)
                                            .and_then(|_| {
                                                shared.complete_unsubscribe(generation, result)
                                            }),
                                        _ => Ok(()),
                                    };
                                    match state_result {
                                        Ok(()) => (Ok(result), false),
                                        Err(error) => (Err(error), true),
                                    }
                                },
                            );
                            let shutdown = pending_request.shutdown;
                            let reply_failed = reply.is_err();
                            if state_failed || (shutdown && reply_failed) {
                                deferred_responses.push((pending_request.response, reply));
                                transport_failed = true;
                                break;
                            }
                            if shutdown {
                                completed_shutdown = Some((pending_request.response, reply));
                                expected_shutdown = true;
                                break;
                            }
                            let _ = pending_request.response.send(reply);
                        }
                        Err(_) => { transport_failed = true; break; }
                    }
                }
                () = wait_for_deadline(shutdown_deadline), if shutdown_deadline.is_some() => {
                    transport_failed = true;
                    break;
                }
                () = wait_for_deadline(pending_deadline), if pending_deadline.is_some() => {
                    transport_failed = true;
                    break;
                }
            }
        }
        for (_, pending) in pending.drain() {
            deferred_responses.push((pending.response, Err(SupervisorError::transport())));
        }
        // A shutdown response transfers termination authority back to the supervisor. Kill any
        // process that remains and await exactly one aggregate process-tree future. process-wrap
        // aggregate waits must never be cancelled and restarted after the leader is reaped.
        let _ = process.child.start_kill();
        let tree_reaped = process.child.wait().await.is_ok();
        let shutdown_completed = expected_shutdown && !transport_failed && tree_reaped;
        let crash_budget_allows_recovery =
            shared.is_recovery_candidate(generation) && shared.allow_recovery_after_crash();
        actor.lock().await.take();
        let recovery_allowed = shared
            .finish_actor(generation, shutdown_completed, crash_budget_allows_recovery)
            .unwrap_or(false);
        if let Some((response, result)) = completed_shutdown {
            let _ = response.send(if shutdown_completed {
                result
            } else {
                Err(SupervisorError::transport())
            });
        }
        for (response, result) in deferred_responses {
            let _ = response.send(result);
        }
        if recovery_allowed {
            tokio::spawn(Box::pin(recover_local_host(shared, actor, notifications)));
        }
    })
}

#[allow(clippy::too_many_lines)]
async fn recover_local_host(
    shared: Arc<Shared>,
    actor: Arc<AsyncMutex<Option<mpsc::Sender<ActorCommand>>>>,
    notifications: broadcast::Sender<host::HostNotification>,
) {
    let Some(spec) = shared.recovery_spec() else {
        let generation = shared.status().generation;
        let _ = shared.transition(generation, HostChildState::Failed);
        return;
    };
    for attempt in 0_u64..3 {
        tokio::time::sleep(Duration::from_millis(100 * (1 << attempt))).await;
        let Ok(generation) = shared.begin_recovery_start() else {
            return;
        };
        let candidate = spec.clone();
        let verification = tokio::task::spawn_blocking(move || verify_launch(candidate)).await;
        if shared.finish_startup_shutdown(generation).unwrap_or(false) {
            return;
        }
        let verified = match verification {
            Ok(Ok(verified)) => verified,
            Ok(Err(error)) => {
                let next = if error.code == SupervisorErrorCode::Incompatible {
                    HostChildState::Incompatible
                } else {
                    HostChildState::Failed
                };
                let _ = shared.transition(generation, next);
                if next == HostChildState::Incompatible {
                    return;
                }
                continue;
            }
            Err(_) => {
                let _ = shared.transition(generation, HostChildState::Failed);
                continue;
            }
        };
        if shared
            .transition(generation, HostChildState::Handshaking)
            .is_err()
        {
            let _ = shared.finish_startup_shutdown(generation);
            return;
        }
        let spawned = spawn_verified_child(&verified, &shared).await;
        if shared.startup_is_draining(generation) {
            if let Ok(mut process) = spawned {
                terminate_and_reap(&mut process).await;
            }
            let _ = shared.finish_startup_shutdown(generation);
            return;
        }
        let mut process = match spawned {
            Ok(process) => process,
            Err(error) => {
                let next = if error.code == SupervisorErrorCode::Incompatible {
                    HostChildState::Incompatible
                } else {
                    HostChildState::Failed
                };
                let _ = shared.transition(generation, next);
                if next == HostChildState::Incompatible {
                    return;
                }
                continue;
            }
        };
        let initialization = initialize_child(&mut process, &verified, generation).await;
        if shared.startup_is_draining(generation) {
            terminate_and_reap(&mut process).await;
            let _ = shared.finish_startup_shutdown(generation);
            return;
        }
        if let Err(error) = initialization {
            terminate_and_reap(&mut process).await;
            let next = if error.code == SupervisorErrorCode::Incompatible {
                HostChildState::Incompatible
            } else {
                HostChildState::Failed
            };
            let _ = shared.transition(generation, next);
            if next == HostChildState::Incompatible {
                return;
            }
            continue;
        }
        let execution_domain = verified.envelope.execution_domain_id.clone();
        let (sender, receiver) = mpsc::channel(64);
        *actor.lock().await = Some(sender);
        if shared
            .set_ready_domain(generation, execution_domain.clone())
            .is_err()
        {
            actor.lock().await.take();
            terminate_and_reap(&mut process).await;
            if !shared.finish_startup_shutdown(generation).unwrap_or(false) {
                let _ = shared.transition(generation, HostChildState::Failed);
            }
            return;
        }
        let next_shared = Arc::clone(&shared);
        let next_actor = Arc::clone(&actor);
        let next_notifications = notifications.clone();
        tokio::spawn(Box::pin(run_actor(
            process,
            receiver,
            next_notifications,
            next_shared,
            next_actor,
            generation,
            execution_domain,
        )));
        return;
    }
}

async fn write_frame(stdin: &mut ChildStdin, frame: &[u8]) -> Result<(), SupervisorError> {
    if frame.len() > MAX_HOST_FRAME_BYTES {
        return Err(SupervisorError::transport());
    }
    stdin
        .write_all(frame)
        .await
        .map_err(|_| SupervisorError::transport())?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|_| SupervisorError::transport())?;
    stdin
        .flush()
        .await
        .map_err(|_| SupervisorError::transport())
}

async fn read_frame(stdout: &mut BufReader<ChildStdout>) -> Result<Vec<u8>, SupervisorError> {
    let mut frame = Vec::new();
    let read = stdout
        .take(u64::try_from(MAX_HOST_FRAME_BYTES).unwrap_or(u64::MAX) + 2)
        .read_until(b'\n', &mut frame)
        .await
        .map_err(|_| SupervisorError::transport())?;
    if read == 0 || frame.len() > MAX_HOST_FRAME_BYTES + 1 || frame.last() != Some(&b'\n') {
        return Err(SupervisorError::transport());
    }
    frame.pop();
    if frame.last() == Some(&b'\r') {
        frame.pop();
    }
    if frame.is_empty() || frame.len() > MAX_HOST_FRAME_BYTES {
        return Err(SupervisorError::transport());
    }
    Ok(frame)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::expect_used,
        clippy::too_many_lines,
        clippy::uninlined_format_args
    )]

    use super::*;

    #[test]
    fn state_machine_rejects_skips_and_stale_generations() {
        let shared = Shared::default();
        let generation = shared.begin_start().expect("start generation");
        assert!(
            shared
                .transition(generation, HostChildState::Ready)
                .is_err()
        );
        assert!(
            shared
                .transition(generation + 1, HostChildState::Handshaking)
                .is_err()
        );
        shared
            .transition(generation, HostChildState::Handshaking)
            .expect("handshaking transition");
        shared
            .set_ready_domain(generation, "local-user".to_string())
            .expect("ready transition");
        assert_eq!(shared.status().state, HostChildState::Ready);
    }

    #[test]
    fn environment_allowlist_excludes_path_and_unreviewed_secrets() {
        let names = safe_environment_names(&[
            "MODEL_API_KEY".to_string(),
            "PATH".to_string(),
            "STARWEAVER_RPC_CONFIG".to_string(),
            "Path".to_string(),
            "Starweaver_RPC_CONFIG".to_string(),
            "LD_PRELOAD".to_string(),
        ]);
        assert!(names.contains(&OsString::from("HOME")));
        assert!(names.contains(&OsString::from("MODEL_API_KEY")));
        assert!(!names.contains(&OsString::from("PATH")));
        assert!(!names.contains(&OsString::from("STARWEAVER_RPC_CONFIG")));
        assert!(!names.contains(&OsString::from("Path")));
        assert!(!names.contains(&OsString::from("Starweaver_RPC_CONFIG")));
        assert!(!names.contains(&OsString::from("LD_PRELOAD")));
    }

    #[test]
    fn desktop_features_are_canonical_unique_and_cover_shutdown() {
        let features = desktop_features();
        let values = features
            .iter()
            .map(host::FeatureId::as_str)
            .collect::<Vec<_>>();
        assert!(
            values
                .windows(2)
                .all(|pair| pair[0].as_bytes() < pair[1].as_bytes())
        );
        assert!(values.contains(&"events.replay"));
        assert!(values.contains(&"events.subscribe"));
        assert!(values.contains(&"host.shutdown"));
        assert!(values.contains(&"runs"));
        assert!(values.contains(&"sessions"));
    }

    #[test]
    fn diagnostics_are_bounded() {
        let mut diagnostics = BoundedDiagnostics::default();
        diagnostics.append(&vec![b'x'; MAX_STDERR_BYTES + 100]);
        assert_eq!(diagnostics.bytes.len(), MAX_STDERR_BYTES);
    }

    #[test]
    fn renderer_request_builder_owns_wire_and_idempotency_fields() {
        let operation: bridge::DesktopHostOperation = serde_json::from_value(serde_json::json!({
            "kind": "run.steer",
            "input": {
                "runId": "run-test",
                "sessionId": "session-test",
                "text": "continue"
            }
        }))
        .expect("renderer-safe intent");
        let supervisor = bridge::build_supervisor_fields(&operation, "desktop-idempotency", None)
            .expect("supervisor fields");
        let complete = bridge::build_complete_host_request(
            operation,
            supervisor,
            bridge::SupervisorRequestContext {
                request_id: "desktop-request".to_string(),
                execution_domain: "local-test".to_string(),
            },
        )
        .expect("generated host request");
        assert_eq!(complete.request.id.as_str(), "desktop-request");
        let host::HostCall::RunSteer(params) = complete.request.call else {
            panic!("run.steer call expected");
        };
        assert_eq!(params.idempotency_key.as_str(), "desktop-idempotency");
        assert_eq!(params.text, "continue");
        let injected = serde_json::json!({
            "kind": "run.steer",
            "input": {
                "runId": "run-test",
                "sessionId": "session-test",
                "text": "continue",
                "idempotencyKey": "renderer-controlled"
            }
        });
        assert!(serde_json::from_value::<bridge::DesktopHostOperation>(injected).is_err());
    }

    #[test]
    fn renderer_run_start_accepts_text_only_and_rejects_raw_resources() {
        let text = serde_json::json!({
            "kind": "run.start",
            "input": {
                "continuationMode": "preserve",
                "input": [{"kind": "text", "text": "hello"}],
                "profile": null,
                "restoreFromRunId": null,
                "sessionId": null
            }
        });
        assert!(serde_json::from_value::<bridge::DesktopHostOperation>(text).is_ok());

        for uri in [
            "file:///etc/passwd",
            "/etc/passwd",
            "https://example.test/private",
        ] {
            let resource = serde_json::json!({
                "kind": "run.start",
                "input": {
                    "continuationMode": "preserve",
                    "input": [{
                        "kind": "resource",
                        "mediaType": "text/plain",
                        "name": "private",
                        "uri": uri
                    }],
                    "profile": null,
                    "restoreFromRunId": null,
                    "sessionId": null
                }
            });
            assert!(
                serde_json::from_value::<bridge::DesktopHostOperation>(resource).is_err(),
                "renderer resource URI must be outside the generated Desktop authority: {uri}"
            );
        }
    }

    #[test]
    fn result_projection_removes_supervisor_owned_receipt_evidence() {
        let result = host::HostResult::RunSteer(host::RunSteerResult {
            accepted: true,
            receipt: host::MutationReceipt {
                fingerprint: host::SchemaDigest::new(
                    "sha256:0000000000000000000000000000000000000000000000000000000000000000",
                )
                .expect("fingerprint"),
                idempotency_key: host::IdempotencyKey::new("secret-operation-key")
                    .expect("idempotency key"),
                operation: "run.steer".to_string(),
                receipt_id: host::ReceiptId::new("receipt-test").expect("receipt id"),
                reconciliation_required: false,
                replayed: false,
                state: "committed".to_string(),
                target_ref: "run-test".to_string(),
            },
        });
        let projected = bridge::project_host_result(result, None).expect("safe projection");
        let value = serde_json::to_value(projected).expect("serialize projection");
        let encoded = value.to_string();
        assert!(!encoded.contains("idempotencyKey"));
        assert!(!encoded.contains("secret-operation-key"));
        assert!(!encoded.contains("fingerprint"));
        assert!(!encoded.contains("\"kind\""));
        assert!(!encoded.contains("\"result\""));
        assert!(encoded.contains("receipt-test"));
        assert!(encoded.contains("accepted"));
    }

    #[test]
    fn durable_operation_ledger_separates_instances_and_survives_restart() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let operation: bridge::DesktopHostOperation = serde_json::from_value(serde_json::json!({
            "kind": "run.steer",
            "input": {
                "runId": "run",
                "sessionId": "session",
                "text": "x".repeat(16_384)
            }
        }))
        .expect("maximum-size mutation operation");
        let fingerprint = operation_fingerprint("domain", &operation).expect("fingerprint");
        let first_id = format!("desktop-op-v1-{}", uuid::Uuid::new_v4());
        let second_id = format!("desktop-op-v1-{}", uuid::Uuid::new_v4());
        let shared = Shared::default();
        shared
            .configure_storage_root(temp.path().to_path_buf())
            .expect("configure storage");
        let (first, record) = shared
            .admit_operation(&first_id, "domain", &fingerprint, &operation)
            .expect("admit first operation");
        let record = record.expect("new durable record");
        persist_durable_operation(temp.path(), &record).expect("persist operation");
        let (retry, retry_record) = shared
            .admit_operation(&first_id, "domain", &fingerprint, &operation)
            .expect("retry operation");
        assert_eq!(first, retry);
        assert!(retry_record.is_none());
        let (second, _) = shared
            .admit_operation(&second_id, "domain", &fingerprint, &operation)
            .expect("independent operation");
        assert_ne!(first, second);
        assert!(
            shared
                .admit_operation(&first_id, "other-domain", &fingerprint, &operation)
                .is_err()
        );

        let restarted = Shared::default();
        restarted
            .configure_storage_root(temp.path().to_path_buf())
            .expect("reload storage");
        let pending = restarted
            .pending_operations()
            .expect("discover pending operation");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].operation_id.0, first_id);
        assert_eq!(
            serde_json::to_value(&pending[0].operation).expect("serialize pending operation"),
            serde_json::to_value(&operation).expect("serialize original operation")
        );
        let (after_restart, new_record) = restarted
            .admit_operation(&first_id, "domain", &fingerprint, &operation)
            .expect("retry after restart");
        assert_eq!(first, after_restart);
        assert!(new_record.is_none());
    }

    #[test]
    fn loader_cleans_unpublished_atomic_operation_files() {
        let temp = tempfile::tempdir().expect("temporary directory");
        load_durable_operations(temp.path()).expect("initialize operation ledger");
        let unpublished = operation_records_path(temp.path()).join(".pending-interrupted");
        std::fs::write(&unpublished, b"{\"partial\":").expect("write interrupted temporary");

        let loaded = load_durable_operations(temp.path()).expect("reload operation ledger");

        assert!(loaded.is_empty());
        assert!(!unpublished.exists());
    }

    #[tokio::test]
    async fn retired_operation_binding_survives_acknowledgement_response_loss_and_restart() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let operation: bridge::DesktopHostOperation = serde_json::from_value(serde_json::json!({
            "kind": "run.steer",
            "input": {"runId": "run", "sessionId": "session", "text": "continue"}
        }))
        .expect("mutation operation");
        let fingerprint = operation_fingerprint("domain", &operation).expect("fingerprint");
        let operation_id = format!("desktop-op-v1-{}", uuid::Uuid::new_v4());
        let supervisor = LocalHostSupervisor::default();
        supervisor
            .configure_storage_root(temp.path().to_path_buf())
            .expect("configure storage");
        let (identity, token) = supervisor
            .durable_operation_identity(&operation_id, "domain", &fingerprint, &operation)
            .await
            .expect("admit operation");
        supervisor
            .acknowledge_renderer_operation(&bridge::DesktopHostOperationAcknowledgementToken(
                token.clone(),
            ))
            .await
            .expect("retire operation");
        assert!(
            supervisor
                .pending_renderer_operations()
                .expect("pending operations")
                .is_empty()
        );
        drop(supervisor);

        let restarted = LocalHostSupervisor::default();
        restarted
            .configure_storage_root(temp.path().to_path_buf())
            .expect("reload storage");
        let (retried_identity, retried_token) = restarted
            .durable_operation_identity(&operation_id, "domain", &fingerprint, &operation)
            .await
            .expect("retry retired operation");
        assert_eq!(retried_identity, identity);
        assert_eq!(retried_token, token);
        assert!(
            restarted
                .pending_renderer_operations()
                .expect("pending operations")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn invalid_mutations_are_validated_before_durable_admission() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let supervisor = LocalHostSupervisor::default();
        supervisor
            .configure_storage_root(temp.path().to_path_buf())
            .expect("configure storage");
        let generation = supervisor.shared.begin_start().expect("start generation");
        supervisor
            .shared
            .transition(generation, HostChildState::Handshaking)
            .expect("handshaking transition");
        supervisor
            .shared
            .set_ready_domain(generation, "domain".to_string())
            .expect("ready transition");
        let invalid: bridge::DesktopHostOperation = serde_json::from_value(serde_json::json!({
            "kind": "run.start",
            "input": {
                "continuationMode": "preserve",
                "input": [],
                "profile": null,
                "restoreFromRunId": null,
                "sessionId": null
            }
        }))
        .expect("renderer intent permits canonical validation to reject empty input");

        for _ in 0..=MAX_UNCERTAIN_OPERATIONS {
            let invocation = bridge::DesktopHostInvocation {
                operation_id: bridge::DesktopOperationId(format!(
                    "desktop-op-v1-{}",
                    uuid::Uuid::new_v4()
                )),
                operation: invalid.clone(),
            };
            let error = supervisor
                .execute_renderer_operation(invocation)
                .await
                .expect_err("empty run input must be rejected before dispatch");
            assert_eq!(error.code, SupervisorErrorCode::InvalidConfiguration);
        }
        assert!(
            supervisor
                .pending_renderer_operations()
                .expect("pending operations")
                .is_empty()
        );
        drop(supervisor);

        let restarted = LocalHostSupervisor::default();
        restarted
            .configure_storage_root(temp.path().to_path_buf())
            .expect("reload storage");
        assert!(
            restarted
                .pending_renderer_operations()
                .expect("pending operations after restart")
                .is_empty()
        );
        let valid: bridge::DesktopHostOperation = serde_json::from_value(serde_json::json!({
            "kind": "run.steer",
            "input": {"runId": "run", "sessionId": "session", "text": "continue"}
        }))
        .expect("valid mutation operation");
        let fingerprint = operation_fingerprint("domain", &valid).expect("fingerprint");
        restarted
            .durable_operation_identity(
                &format!("desktop-op-v1-{}", uuid::Uuid::new_v4()),
                "domain",
                &fingerprint,
                &valid,
            )
            .await
            .expect("capacity remains available for a valid mutation");
        assert_eq!(
            restarted
                .pending_renderer_operations()
                .expect("valid pending operation")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn acknowledged_operations_are_durably_retired_and_do_not_exhaust_capacity() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let supervisor = LocalHostSupervisor::default();
        supervisor
            .configure_storage_root(temp.path().to_path_buf())
            .expect("configure storage");
        let operation: bridge::DesktopHostOperation = serde_json::from_value(serde_json::json!({
            "kind": "run.steer",
            "input": {"runId": "run", "sessionId": "session", "text": "continue"}
        }))
        .expect("mutation operation");
        let fingerprint = operation_fingerprint("domain", &operation).expect("fingerprint");

        let mut newest = None;
        for _ in 0..MAX_RETIRED_OPERATION_TOMBSTONES + 64 {
            let operation_id = format!("desktop-op-v1-{}", uuid::Uuid::new_v4());
            let (identity, token) = supervisor
                .durable_operation_identity(&operation_id, "domain", &fingerprint, &operation)
                .await
                .expect("admit durable operation");
            let acknowledgement = bridge::DesktopHostOperationAcknowledgementToken(token);
            supervisor
                .acknowledge_renderer_operation(&acknowledgement)
                .await
                .expect("acknowledge operation");
            supervisor
                .acknowledge_renderer_operation(&acknowledgement)
                .await
                .expect("repeat acknowledgement");
            let (retired_identity, retired_token) = supervisor
                .durable_operation_identity(&operation_id, "domain", &fingerprint, &operation)
                .await
                .expect("retry retired logical operation");
            assert_eq!(retired_identity, identity);
            assert_eq!(retired_token, acknowledgement.0);
            newest = Some((operation_id, identity, acknowledgement.0));
        }

        assert!(
            supervisor
                .pending_renderer_operations()
                .expect("pending operations")
                .is_empty()
        );
        {
            let operations = supervisor
                .shared
                .durable_operations
                .lock()
                .expect("operation ledger");
            assert_eq!(operations.len(), MAX_RETIRED_OPERATION_TOMBSTONES);
            assert!(
                operations
                    .values()
                    .all(|record| record.state == DurableOperationState::Retired)
            );
            let retained_bytes = operations.values().try_fold(0_usize, |total, record| {
                total.checked_add(
                    serde_json::to_vec(record)
                        .expect("serialize tombstone")
                        .len(),
                )
            });
            drop(operations);
            assert!(
                retained_bytes.expect("retained byte count")
                    <= MAX_RETIRED_OPERATION_TOMBSTONE_BYTES
            );
        }
        assert_eq!(
            std::fs::read_dir(operation_records_path(temp.path()))
                .expect("operation ledger directory")
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension() == Some(std::ffi::OsStr::new("json")))
                .count(),
            MAX_RETIRED_OPERATION_TOMBSTONES
        );
        drop(supervisor);

        let restarted = LocalHostSupervisor::default();
        restarted
            .configure_storage_root(temp.path().to_path_buf())
            .expect("reload compacted storage");
        assert!(
            restarted
                .pending_renderer_operations()
                .expect("pending operations after restart")
                .is_empty()
        );
        let (operation_id, identity, token) = newest.expect("newest operation");
        let (restarted_identity, restarted_token) = restarted
            .durable_operation_identity(&operation_id, "domain", &fingerprint, &operation)
            .await
            .expect("newest tombstone survives restart");
        assert_eq!(restarted_identity, identity);
        assert_eq!(restarted_token, token);
    }

    #[test]
    fn shutdown_admission_covers_startup_and_recovery_without_an_actor() {
        let shared = Shared::default();
        let generation = shared.begin_start().expect("start generation");
        assert!(matches!(
            shared.request_shutdown().expect("startup shutdown"),
            ShutdownAdmission::Wait { generation: admitted } if admitted == generation
        ));
        assert!(
            shared
                .finish_startup_shutdown(generation)
                .expect("finish startup shutdown")
        );
        assert_eq!(shared.status().state, HostChildState::Stopped);

        let generation = shared.begin_start().expect("second start generation");
        shared
            .transition(generation, HostChildState::Handshaking)
            .expect("handshaking");
        shared
            .set_ready_domain(generation, "domain".to_string())
            .expect("ready");
        assert!(
            shared
                .finish_actor(generation, false, true)
                .expect("enter recovery")
        );
        assert_eq!(shared.status().state, HostChildState::Recovering);
        assert!(matches!(
            shared.request_shutdown().expect("recovery shutdown"),
            ShutdownAdmission::Terminal
        ));
        assert_eq!(shared.status().state, HostChildState::Stopped);
        assert!(shared.begin_recovery_start().is_err());
    }

    #[test]
    fn diagnostic_redaction_removes_inherited_secret_values() {
        let mut redactor = StreamingSecretRedactor::new(vec![b"private-token".to_vec()]);
        let mut redacted = redactor.push(b"Authorization: Bearer private-");
        redacted.extend(redactor.push(b"token"));
        redacted.extend(redactor.finish());
        assert!(!String::from_utf8_lossy(&redacted).contains("private-token"));
    }

    #[test]
    fn pagination_tokens_are_opaque_and_bound_to_the_admitted_query() {
        let shared = Shared::default();
        let first: bridge::DesktopHostOperation = serde_json::from_value(serde_json::json!({
            "kind": "session.search",
            "input": {
                "mode": "literal",
                "profile": null,
                "query": "rust",
                "status": null
            }
        }))
        .expect("first page intent");
        let fingerprint = operation_fingerprint("local-test", &first).expect("fingerprint");
        let token = shared
            .issue_page_token(
                7,
                "local-test",
                &fingerprint,
                "host-private-cursor".to_string(),
            )
            .expect("page token");
        assert!(!token.0.contains("host-private-cursor"));

        let next: bridge::DesktopHostOperation = serde_json::from_value(serde_json::json!({
            "kind": "session.search",
            "input": {
                "mode": "literal",
                "pageToken": token.0,
                "profile": null,
                "query": "rust",
                "status": null
            }
        }))
        .expect("next page intent");
        assert_eq!(
            operation_fingerprint("local-test", &next).expect("next fingerprint"),
            fingerprint
        );
        assert_eq!(
            shared
                .resolve_page_cursor(next.page_token(), 7, "local-test", &fingerprint,)
                .expect("resolved cursor")
                .as_deref(),
            Some("host-private-cursor")
        );
        assert!(
            shared
                .resolve_page_cursor(next.page_token(), 8, "local-test", &fingerprint,)
                .is_err()
        );
    }

    #[test]
    fn projected_pagination_replaces_the_host_cursor_with_a_desktop_token() {
        let result = host::HostResult::SessionList(host::SessionListResult {
            page: host::PageInfo {
                has_more: true,
                next_cursor: Some("host-private-cursor".to_string()),
            },
            sessions: Vec::new(),
        });
        let projected = bridge::project_host_result(
            result,
            Some(bridge::DesktopPageToken("desktop-page-safe".to_string())),
        )
        .expect("safe pagination projection");
        let encoded = serde_json::to_string(&projected).expect("serialize projection");
        assert!(encoded.contains("desktop-page-safe"));
        assert!(encoded.contains("nextPageToken"));
        assert!(!encoded.contains("host-private-cursor"));
        assert!(!encoded.contains("nextCursor"));
    }

    #[test]
    fn crash_budget_survives_successful_actor_generations() {
        let shared = Shared::default();
        assert!(shared.allow_recovery_after_crash());
        assert!(shared.allow_recovery_after_crash());
        assert!(shared.allow_recovery_after_crash());
        assert!(!shared.allow_recovery_after_crash());
    }

    #[test]
    fn shutdown_intent_wins_actor_transport_failure_race() {
        let shared = Shared::default();
        let generation = shared.begin_start().expect("start generation");
        shared
            .transition(generation, HostChildState::Handshaking)
            .expect("handshaking");
        shared
            .set_ready_domain(generation, "domain".to_string())
            .expect("ready");
        assert!(matches!(
            shared.request_shutdown().expect("shutdown intent"),
            ShutdownAdmission::Actor {
                generation: admitted,
                execution_domain
            } if admitted == generation && execution_domain == "domain"
        ));

        assert!(!shared.is_recovery_candidate(generation));
        assert!(
            !shared
                .finish_actor(generation, false, true)
                .expect("finish actor")
        );
        assert_eq!(shared.status().state, HostChildState::Failed);
        assert!(shared.begin_recovery_start().is_err());
    }

    #[cfg(unix)]
    fn process_is_executing(pid: i32) -> bool {
        match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
            Err(nix::errno::Errno::ESRCH) => false,
            Err(_) => true,
            Ok(()) => {
                let output = std::process::Command::new("ps")
                    .args(["-o", "stat=", "-p", &pid.to_string()])
                    .output();
                match output {
                    Ok(output) if output.status.success() => {
                        let state = String::from_utf8_lossy(&output.stdout);
                        let state = state.trim();
                        !state.is_empty() && !state.starts_with('Z')
                    }
                    Ok(_) => false,
                    Err(_) => true,
                }
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_tree_barrier_terminates_descendants() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let pid_path = temp.path().join("descendant.pid");
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("sleep 30 & echo $! > \"$1\"; wait")
            .arg("fixture")
            .arg(&pid_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut command = CommandWrap::from(command);
        command.wrap(ProcessGroup::leader()).wrap(KillOnDrop);
        let mut child = command.spawn().expect("spawn process tree");
        let descendant = timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(value) = std::fs::read_to_string(&pid_path)
                    && let Ok(pid) = value.trim().parse::<i32>()
                {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("descendant pid");
        child.start_kill().expect("terminate process group");
        timeout(PROCESS_REAP_TIMEOUT, child.wait())
            .await
            .expect("process tree reap timeout")
            .expect("process tree wait");
        timeout(PROCESS_REAP_TIMEOUT, async {
            while process_is_executing(descendant) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("descendant termination timeout");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_transport_failure_is_reported_only_after_child_reap() {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("IFS= read -r _request; sleep 0.2")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut command = CommandWrap::from(command);
        command.wrap(ProcessGroup::leader()).wrap(KillOnDrop);
        let mut child = command.spawn().expect("spawn fixture");
        let stdin = child.stdin().take().expect("fixture stdin");
        let stdout = child.stdout().take().expect("fixture stdout");
        let process = ChildProcess {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            _staging: None,
        };
        let shared = Arc::new(Shared::default());
        let generation = shared.begin_start().expect("start generation");
        shared
            .transition(generation, HostChildState::Handshaking)
            .expect("handshaking");
        shared
            .set_ready_domain(generation, "local-test".to_string())
            .expect("ready");
        shared
            .transition(generation, HostChildState::Draining)
            .expect("draining");
        let (sender, receiver) = mpsc::channel(1);
        let actor = Arc::new(AsyncMutex::new(Some(sender.clone())));
        let (notifications, _) = broadcast::channel(8);
        let task = tokio::spawn(run_actor(
            process,
            receiver,
            notifications,
            Arc::clone(&shared),
            Arc::clone(&actor),
            generation,
            "local-test".to_string(),
        ));
        let (response, completed) = oneshot::channel();
        let started = Instant::now();
        sender
            .send(ActorCommand::Shutdown {
                expected_generation: generation,
                expected_domain: "local-test".to_string(),
                deadline: Instant::now() + Duration::from_secs(2),
                request: host::HostRequest {
                    id: host::RequestId::new("shutdown-failure-test").expect("request id"),
                    call: host::HostCall::Shutdown(host::ShutdownParams { deadline_ms: 500 }),
                },
                response,
            })
            .await
            .expect("enqueue shutdown");
        assert!(completed.await.expect("shutdown response sender").is_err());
        assert!(started.elapsed() >= Duration::from_millis(150));
        assert_eq!(shared.status().state, HostChildState::Failed);
        task.await.expect("actor task");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mismatched_unsubscribe_result_identity_fails_the_shared_transport() {
        let expected_subscription_id =
            host::SubscriptionId::new("subscription-expected").expect("expected subscription id");
        let returned_subscription_id =
            host::SubscriptionId::new("subscription-returned").expect("returned subscription id");
        let response_frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "unsubscribe-mismatch-test",
            "result": {
                "closed": false,
                "subscriptionId": returned_subscription_id,
            },
        });
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(format!(
                "IFS= read -r _request; printf '%s\\n' '{}'",
                response_frame
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut command = CommandWrap::from(command);
        command.wrap(ProcessGroup::leader()).wrap(KillOnDrop);
        let mut child = command.spawn().expect("spawn fixture");
        let stdin = child.stdin().take().expect("fixture stdin");
        let stdout = child.stdout().take().expect("fixture stdout");
        let process = ChildProcess {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            _staging: None,
        };
        let shared = Arc::new(Shared::default());
        let generation = shared.begin_start().expect("start generation");
        shared
            .transition(generation, HostChildState::Handshaking)
            .expect("handshaking");
        shared
            .set_ready_domain(generation, "local-test".to_string())
            .expect("ready");
        shared
            .register_subscription(generation, &expected_subscription_id)
            .expect("register expected subscription");
        shared
            .register_subscription(generation, &returned_subscription_id)
            .expect("register returned subscription");
        let (sender, receiver) = mpsc::channel(1);
        let actor = Arc::new(AsyncMutex::new(Some(sender.clone())));
        let (notifications, _) = broadcast::channel(8);
        let task = tokio::spawn(run_actor(
            process,
            receiver,
            notifications,
            Arc::clone(&shared),
            Arc::clone(&actor),
            generation,
            "local-test".to_string(),
        ));
        let (response, completed) = oneshot::channel();
        sender
            .send(ActorCommand::Execute {
                expected_generation: generation,
                expected_domain: "local-test".to_string(),
                request: host::HostRequest {
                    id: host::RequestId::new("unsubscribe-mismatch-test").expect("request id"),
                    call: host::HostCall::EventsUnsubscribe(host::EventsUnsubscribeParams {
                        subscription_id: expected_subscription_id,
                    }),
                },
                response,
            })
            .await
            .expect("enqueue unsubscribe");
        let error = completed
            .await
            .expect("unsubscribe response sender")
            .expect_err("mismatched result identity must fail closed");
        assert_eq!(error.code, SupervisorErrorCode::Transport);
        task.await.expect("actor task");
        assert_eq!(shared.status().state, HostChildState::Failed);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn subscription_close_interleavings_keep_verified_subprocess_ready() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("temporary directory");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace directory");
        let envelope_path = temp.path().join("launch.json");
        let envelope = serde_json::json!({
            "schema": {"name": "starweaver.rpc.launch", "version": 1},
            "mode": "workspace_execution",
            "database": {"identity": "database-test", "path": temp.path().join("sessions.sqlite")},
            "workspace": {"identity": "workspace-test", "root": workspace},
            "stateDirectory": temp.path().join("state"),
            "executionDomainId": "local-test",
            "configurationGeneration": "3",
            "defaultProfile": "test",
            "profiles": [{"name":"test","modelId":"local_echo","instructions":[],"toolsets":[]}],
            "providers": [],
            "capabilityCaps": {"hitl":true,"clarifyingQuestions":true,"nativeLocalShell":false}
        });
        let envelope_bytes = serde_json::to_vec(&envelope).expect("launch JSON");
        std::fs::write(&envelope_path, &envelope_bytes).expect("write launch envelope");
        let features = desktop_features();
        let initialize = host::InitializeResult {
            launch: host::LaunchCompatibility {
                accepted_maximum_version: 1,
                accepted_minimum_version: 1,
                configuration_generation: host::DecimalU64::new(3),
                effective_schema: host::LaunchSchemaIdentity {
                    name: host::LaunchSchemaIdentityName::Value,
                    version: 1,
                },
                envelope_digest: host::SchemaDigest::new(sha256_bytes(&envelope_bytes))
                    .expect("envelope digest"),
                mode: "workspace_execution".to_string(),
            },
            negotiated_features: features.clone(),
            protocol: host::ProtocolIdentity {
                major: host::PROTOCOL_MAJOR,
                name: host::ProtocolIdentityName::Value,
                revision: host::PROTOCOL_REVISION.to_string(),
                schema_digest: host::SchemaDigest::new(host::SCHEMA_DIGEST).expect("schema digest"),
            },
            runtime_build: host::RuntimeBuildIdentity {
                build_revision: "fixture".to_string(),
                target: "fixture-target".to_string(),
                version: "fixture-version".to_string(),
            },
            runtime_status: "ready".to_string(),
            server_info: host::ServerInfo {
                name: "fixture".to_string(),
                version: "fixture-version".to_string(),
            },
            startup_reconciliation: host::StartupReconciliation {
                changed_run_state: false,
                repaired_runs: host::DecimalU64::new(0),
            },
            storage: host::StorageCompatibility {
                current_generation: host::DecimalU64::new(1),
                maintenance_barrier_generation: host::DecimalU64::new(0),
                maximum_readable_generation: host::DecimalU64::new(1),
                maximum_writable_generation: host::DecimalU64::new(1),
                minimum_readable_generation: host::DecimalU64::new(1),
                minimum_writable_generation: host::DecimalU64::new(1),
            },
            supported_features: features,
            workspace: host::WorkspaceCompatibility {
                execution_domain_id: "local-test".to_string(),
                workspace_identity: "workspace-test".to_string(),
            },
        };
        let initialize_frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "desktop-initialize-1",
            "result": initialize,
        });
        let remote_error_frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "desktop-1-1",
            "error": {
                "code": -32010,
                "message": "session not found",
                "data": {
                    "kind": "not_found",
                    "reconciliationRequired": false,
                    "resourceKind": "session",
                    "retryable": false
                }
            },
        });
        let subscribe_frame = |request_id: &str, subscription_id: &str| {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "acceptedCursor": "cursor-accepted",
                    "fenceCursor": "cursor-fence",
                    "nextDeliverySequence": "1",
                    "subscriptionId": subscription_id
                },
            })
        };
        let unsubscribe_frame = |request_id: &str, subscription_id: &str, closed: bool| {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {"closed":closed,"subscriptionId":subscription_id},
            })
        };
        let closed_notification = |subscription_id: &str, reason: &str| {
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "subscription.closed",
                "params": {
                    "lastFlushedCursor": "cursor-fence",
                    "lastFlushedDeliverySequence": "0",
                    "reason": reason,
                    "subscriptionId": subscription_id
                },
            })
        };
        let subscription_ordinary = "subscription-ordinary";
        let subscription_close_first = "subscription-close-first";
        let subscription_response_first = "subscription-response-first";
        let subscribe_ordinary = subscribe_frame("desktop-1-2", subscription_ordinary);
        let unsubscribe_ordinary = unsubscribe_frame("desktop-1-3", subscription_ordinary, true);
        let close_ordinary = closed_notification(subscription_ordinary, "unsubscribed");
        let subscribe_close_first = subscribe_frame("desktop-1-4", subscription_close_first);
        let unsubscribe_close_first =
            unsubscribe_frame("desktop-1-5", subscription_close_first, true);
        let close_first = closed_notification(subscription_close_first, "terminal");
        let duplicate_close = closed_notification(subscription_close_first, "unsubscribed");
        let subscribe_response_first = subscribe_frame("desktop-1-6", subscription_response_first);
        let unsubscribe_response_first =
            unsubscribe_frame("desktop-1-7", subscription_response_first, false);
        let close_after_response = closed_notification(subscription_response_first, "terminal");
        let shutdown_frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "desktop-1-8",
            "result": {"status":"shutdown"},
        });
        let runtime = temp.path().join("fixture-host");
        let script = format!(
            concat!(
                "#!/bin/sh\n",
                "IFS= read -r _request\nprintf '%s\\n' '{}'\n",
                "IFS= read -r _request\nprintf '%s\\n' '{}'\n",
                "IFS= read -r _request\nprintf '%s\\n' '{}'\n",
                "IFS= read -r _request\nprintf '%s\\n%s\\n' '{}' '{}'\n",
                "IFS= read -r _request\nprintf '%s\\n' '{}'\n",
                "IFS= read -r _request\nprintf '%s\\n%s\\n%s\\n' '{}' '{}' '{}'\n",
                "IFS= read -r _request\nprintf '%s\\n' '{}'\n",
                "IFS= read -r _request\nprintf '%s\\n%s\\n' '{}' '{}'\n",
                "IFS= read -r _request\nprintf '%s\\n' '{}'\n",
            ),
            initialize_frame,
            remote_error_frame,
            subscribe_ordinary,
            unsubscribe_ordinary,
            close_ordinary,
            subscribe_close_first,
            close_first,
            unsubscribe_close_first,
            duplicate_close,
            subscribe_response_first,
            unsubscribe_response_first,
            close_after_response,
            shutdown_frame,
        );
        std::fs::write(&runtime, script).expect("write fixture host");
        let mut permissions = std::fs::metadata(&runtime)
            .expect("fixture metadata")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&runtime, permissions).expect("fixture executable");
        let runtime_digest = sha256_file(&runtime).expect("fixture digest");
        let supervisor = LocalHostSupervisor::default();
        supervisor
            .configure_storage_root(temp.path().join("desktop-supervisor"))
            .expect("configure supervisor storage");
        supervisor
            .start(LocalLaunchSpec {
                runtime_path: runtime,
                runtime_digest,
                runtime_version: "fixture-version".to_string(),
                build_revision: "fixture".to_string(),
                target: "fixture-target".to_string(),
                launch_envelope_path: envelope_path,
                launch_envelope_digest: sha256_bytes(&envelope_bytes),
                configuration_generation: 3,
                execution_domain_id: "local-test".to_string(),
                workspace_identity: "workspace-test".to_string(),
            })
            .await
            .expect("verified fixture starts");
        assert_eq!(supervisor.status().state, HostChildState::Ready);
        let error = supervisor
            .send_actor(host::HostRequest {
                id: host::RequestId::new(supervisor.next_request_id(1)).expect("request id"),
                call: host::HostCall::SessionGet(host::SessionGetParams {
                    run_limit: 1,
                    session_id: host::SessionId::new("missing-session").expect("session id"),
                }),
            })
            .await
            .expect_err("ordinary remote error");
        assert_eq!(error.code, SupervisorErrorCode::Remote);
        assert_eq!(supervisor.status().state, HostChildState::Ready);
        let scope = bridge::DesktopHostEventScope {
            session_id: bridge::SessionId("session-fixture".to_string()),
            run_id: bridge::RunId("run-fixture".to_string()),
        };
        for (expected_subscription_id, interleaving) in [
            (subscription_ordinary, "ordinary response-before-close"),
            (subscription_close_first, "terminal-close-before-response"),
            (
                subscription_response_first,
                "response-before-terminal-close",
            ),
        ] {
            let tail = supervisor
                .open_run_event_tail(&scope, None)
                .await
                .expect("open fixture event tail");
            assert_eq!(tail.subscription_id.as_str(), expected_subscription_id);
            supervisor
                .close_event_tail(
                    tail.subscription_id,
                    tail.generation,
                    &tail.execution_domain,
                )
                .await
                .expect("close fixture event tail");
            tokio::time::sleep(Duration::from_millis(25)).await;
            assert_eq!(
                supervisor.status().state,
                HostChildState::Ready,
                "{interleaving} must not fail the shared transport"
            );
        }
        supervisor.shutdown().await.expect("shutdown barrier");
        assert_eq!(supervisor.status().state, HostChildState::Stopped);
    }
}
