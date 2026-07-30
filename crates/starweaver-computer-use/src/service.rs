use std::{
    collections::{HashMap, VecDeque},
    io::Cursor,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Instant,
};

use async_trait::async_trait;
use image::{ImageFormat, ImageReader, Limits};
use sha2::{Digest, Sha256};
use starweaver_core::CancellationToken;
use tokio::{
    sync::{Mutex, MutexGuard},
    task::JoinHandle,
    time::Instant as TokioInstant,
};

use crate::{
    ActiveSessionStatus, BackendProbe, CloseReason, CloseReceipt, ComputerAction,
    ComputerActionReceipt, ComputerActionRequest, ComputerActionResult, ComputerObservation,
    ComputerSessionId, ComputerSessionState, ComputerStatus, ComputerUseContractVersion,
    ComputerUseError, ComputerUseErrorCode, ComputerUseFailure, ComputerUsePolicy,
    DynNativeDesktopBackend, EffectEpoch, EffectStatus, EffectiveComputerCapabilities,
    EncodedDesktopImage, FrameGeneration, FrameRedactionStatus, GeometrySnapshot,
    InputCleanupStatus, ObservationId, ObservationRef, ObserveRequest, OperationId,
    OperationSequence, PauseReason, PauseReceipt, PermissionCapabilityStatus, PermissionRequest,
    PermissionRequestOutcome, ProcessInstanceId, RetryClassification, ShutdownReceipt,
    StabilityCheckStatus, TakeoverEpoch, UserPresenceStatus,
};

pub type DynComputerUseService = Arc<dyn ComputerUseService>;
pub type DynComputerSession = Arc<dyn ComputerSession>;

#[async_trait]
pub trait ComputerUseService: Send + Sync {
    fn contract_version(&self) -> ComputerUseContractVersion;
    fn process_instance_id(&self) -> ProcessInstanceId;
    fn policy(&self) -> &ComputerUsePolicy;

    async fn status(&self, cancel: CancellationToken) -> Result<ComputerStatus, ComputerUseError>;

    /// Request attended native permissions from a trusted host path.
    ///
    /// This operation is intentionally absent from the model-visible tool
    /// catalog. Its result is an immediate status observation, not proof that
    /// the user accepted a prompt.
    async fn request_permissions(
        &self,
        _request: PermissionRequest,
        cancel: CancellationToken,
    ) -> Result<PermissionRequestOutcome, ComputerUseError> {
        if cancel.is_cancelled() {
            return Err(ComputerUseError::cancelled());
        }
        Err(ComputerUseError::new(
            ComputerUseErrorCode::UnsupportedCapability,
            "this Computer Use service does not support attended permission requests",
            RetryClassification::Never,
        ))
    }

    #[doc(hidden)]
    async fn status_with_queue_deadline(
        &self,
        cancel: CancellationToken,
        _queue_deadline: TokioInstant,
    ) -> Result<ComputerStatus, ComputerUseError> {
        self.status(cancel).await
    }

    async fn open_current_desktop(
        &self,
        cancel: CancellationToken,
    ) -> Result<DynComputerSession, ComputerUseError>;

    #[doc(hidden)]
    async fn open_current_desktop_with_queue_deadline(
        &self,
        cancel: CancellationToken,
        _queue_deadline: TokioInstant,
    ) -> Result<DynComputerSession, ComputerUseError> {
        self.open_current_desktop(cancel).await
    }

    async fn shutdown(&self, reason: CloseReason) -> Result<ShutdownReceipt, ComputerUseError>;
}

#[async_trait]
pub trait ComputerSession: Send + Sync {
    fn id(&self) -> ComputerSessionId;
    fn process_instance_id(&self) -> ProcessInstanceId;
    fn capabilities(&self) -> EffectiveComputerCapabilities;

    async fn status(&self, cancel: CancellationToken) -> Result<ComputerStatus, ComputerUseError>;

    #[doc(hidden)]
    async fn status_with_queue_deadline(
        &self,
        cancel: CancellationToken,
        _queue_deadline: TokioInstant,
    ) -> Result<ComputerStatus, ComputerUseError> {
        self.status(cancel).await
    }

    async fn observe(
        &self,
        request: ObserveRequest,
        cancel: CancellationToken,
    ) -> Result<ComputerObservation, ComputerUseError>;

    #[doc(hidden)]
    async fn observe_with_queue_deadline(
        &self,
        request: ObserveRequest,
        cancel: CancellationToken,
        _queue_deadline: TokioInstant,
    ) -> Result<ComputerObservation, ComputerUseError> {
        self.observe(request, cancel).await
    }

    async fn act(
        &self,
        request: ComputerActionRequest,
        cancel: CancellationToken,
    ) -> Result<ComputerActionResult, ComputerUseFailure>;

    #[doc(hidden)]
    async fn act_with_queue_deadline(
        &self,
        request: ComputerActionRequest,
        cancel: CancellationToken,
        _queue_deadline: TokioInstant,
    ) -> Result<ComputerActionResult, ComputerUseFailure> {
        self.act(request, cancel).await
    }

    async fn pause(&self, reason: PauseReason) -> Result<PauseReceipt, ComputerUseError>;

    async fn close(&self, reason: CloseReason) -> Result<CloseReceipt, ComputerUseError>;
}

const BACKEND_HEALTHY: u8 = 0;
const BACKEND_POISONED: u8 = 1;
const BACKEND_CLOSED: u8 = 2;

#[derive(Default)]
struct BackendLifecycle {
    state: AtomicU8,
    gate: Arc<Mutex<()>>,
    unavailable: CancellationToken,
}

impl BackendLifecycle {
    fn ensure_healthy(&self) -> Result<(), ComputerUseError> {
        match self.state.load(Ordering::Acquire) {
            BACKEND_HEALTHY => Ok(()),
            BACKEND_POISONED => Err(backend_poisoned()),
            _ => Err(session_closed()),
        }
    }

    fn is_poisoned(&self) -> bool {
        self.state.load(Ordering::Acquire) == BACKEND_POISONED
    }

    fn is_closed(&self) -> bool {
        self.state.load(Ordering::Acquire) == BACKEND_CLOSED
    }

    fn poison(&self) {
        // Publish the terminal state before waking waiters or cancelling the
        // current native operation. A cancellation-cooperative operation may
        // release the backend gate immediately, so queued work must observe
        // POISONED before it can acquire that gate.
        let _ = self.state.compare_exchange(
            BACKEND_HEALTHY,
            BACKEND_POISONED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        self.unavailable.cancel();
    }

    fn mark_closed(&self) {
        let _ = self.state.compare_exchange(
            BACKEND_HEALTHY,
            BACKEND_CLOSED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        self.unavailable.cancel();
    }
}

#[derive(Clone)]
struct SessionPoisonProjection {
    capabilities_bits: Arc<AtomicU8>,
    state: Arc<Mutex<SessionData>>,
}

impl SessionPoisonProjection {
    fn poison(&self) {
        self.capabilities_bits.store(0, Ordering::Release);
        if let Ok(mut data) = self.state.try_lock() {
            mark_session_unavailable(&self.capabilities_bits, &mut data);
            return;
        }
        let state = self.state.clone();
        let capabilities_bits = self.capabilities_bits.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let mut data = state.lock().await;
                mark_session_unavailable(&capabilities_bits, &mut data);
            });
        }
    }
}

struct BackendOperationGuard {
    lifecycle: Arc<BackendLifecycle>,
    operation_cancel: CancellationToken,
    projection: Option<SessionPoisonProjection>,
    armed: bool,
}

impl BackendOperationGuard {
    const fn new(
        lifecycle: Arc<BackendLifecycle>,
        operation_cancel: CancellationToken,
        projection: Option<SessionPoisonProjection>,
    ) -> Self {
        Self {
            lifecycle,
            operation_cancel,
            projection,
            armed: true,
        }
    }

    fn poison(&self) {
        self.lifecycle.poison();
        self.operation_cancel.cancel();
        if let Some(projection) = &self.projection {
            projection.poison();
        }
    }

    const fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for BackendOperationGuard {
    fn drop(&mut self) {
        if self.armed {
            self.poison();
        }
    }
}

pub struct LocalComputerUseService {
    process_instance_id: ProcessInstanceId,
    policy: Arc<ComputerUsePolicy>,
    backend: DynNativeDesktopBackend,
    lifecycle: Arc<BackendLifecycle>,
    started_at: Instant,
    session_slot: Mutex<Option<Arc<LocalComputerSession>>>,
}

impl LocalComputerUseService {
    #[must_use]
    pub fn new(mut policy: ComputerUsePolicy, backend: DynNativeDesktopBackend) -> Self {
        policy.idempotency.max_entries = policy.idempotency.max_entries.max(1);
        Self {
            process_instance_id: ProcessInstanceId::new(),
            policy: Arc::new(policy),
            backend,
            lifecycle: Arc::new(BackendLifecycle::default()),
            started_at: Instant::now(),
            session_slot: Mutex::new(None),
        }
    }
}

#[async_trait]
impl ComputerUseService for LocalComputerUseService {
    fn contract_version(&self) -> ComputerUseContractVersion {
        ComputerUseContractVersion::V1
    }

    fn process_instance_id(&self) -> ProcessInstanceId {
        self.process_instance_id.clone()
    }

    fn policy(&self) -> &ComputerUsePolicy {
        &self.policy
    }

    async fn status(&self, cancel: CancellationToken) -> Result<ComputerStatus, ComputerUseError> {
        self.status_with_queue_deadline(
            cancel,
            TokioInstant::now() + self.policy.queue_wait_timeout,
        )
        .await
    }

    async fn request_permissions(
        &self,
        request: PermissionRequest,
        cancel: CancellationToken,
    ) -> Result<PermissionRequestOutcome, ComputerUseError> {
        if !request.screen_recording && !request.accessibility {
            return Err(ComputerUseError::invalid(
                "at least one permission must be requested",
            ));
        }
        self.lifecycle.ensure_healthy()?;
        let operation_cancel = CancellationToken::new();
        let operation_guard =
            BackendOperationGuard::new(self.lifecycle.clone(), operation_cancel.clone(), None);
        let backend = self.backend.clone();
        let task_cancel = operation_cancel.clone();
        let task_lifecycle = self.lifecycle.clone();
        let task = tokio::spawn(async move {
            let _backend_guard = cancellable_backend_gate(&task_lifecycle, &task_cancel).await?;
            task_lifecycle.ensure_healthy()?;
            backend.request_permissions(request, task_cancel).await
        });
        let probe = cancellable(
            self.policy.operation_timeout,
            self.policy.cancellation_cleanup_timeout,
            cancel,
            operation_cancel,
            operation_guard,
            task,
        )
        .await?;
        Ok(PermissionRequestOutcome {
            requested: request,
            permissions: probe.permissions,
            effective_capabilities: probe
                .capabilities
                .intersect(self.policy.allowed_capabilities),
            diagnostics_code: probe.diagnostics_code,
        })
    }

    async fn status_with_queue_deadline(
        &self,
        cancel: CancellationToken,
        queue_deadline: TokioInstant,
    ) -> Result<ComputerStatus, ComputerUseError> {
        let existing = {
            let slot = cancellable_lock_until(&self.session_slot, queue_deadline, &cancel).await?;
            slot.as_ref().cloned()
        };
        if let Some(session) = existing {
            return session
                .status_with_queue_deadline(cancel, queue_deadline)
                .await;
        }
        self.lifecycle.ensure_healthy()?;
        let operation_cancel = CancellationToken::new();
        let operation_guard =
            BackendOperationGuard::new(self.lifecycle.clone(), operation_cancel.clone(), None);
        let backend = self.backend.clone();
        let task_cancel = operation_cancel.clone();
        let task_lifecycle = self.lifecycle.clone();
        let task = tokio::spawn(async move {
            let _backend_guard = cancellable_backend_gate(&task_lifecycle, &task_cancel).await?;
            task_lifecycle.ensure_healthy()?;
            backend.probe(task_cancel).await
        });
        let probe = cancellable(
            self.policy.operation_timeout,
            self.policy.cancellation_cleanup_timeout,
            cancel,
            operation_cancel,
            operation_guard,
            task,
        )
        .await?;
        Ok(status_from_probe(
            &probe,
            self.process_instance_id.clone(),
            None,
            ComputerSessionState::Created,
            self.policy.desktop_scope,
            None,
            None,
            probe
                .capabilities
                .intersect(self.policy.allowed_capabilities),
        ))
    }

    #[allow(clippy::collapsible_if, clippy::significant_drop_tightening)]
    async fn open_current_desktop(
        &self,
        cancel: CancellationToken,
    ) -> Result<DynComputerSession, ComputerUseError> {
        self.open_current_desktop_with_queue_deadline(
            cancel,
            TokioInstant::now() + self.policy.queue_wait_timeout,
        )
        .await
    }

    async fn open_current_desktop_with_queue_deadline(
        &self,
        cancel: CancellationToken,
        queue_deadline: TokioInstant,
    ) -> Result<DynComputerSession, ComputerUseError> {
        let mut slot = cancellable_lock_until(&self.session_slot, queue_deadline, &cancel).await?;
        if let Some(session) = slot.as_ref() {
            let session_state = cancellable_lock_until(&session.state, queue_deadline, &cancel)
                .await?
                .state;
            match session_state {
                ComputerSessionState::Closed => {}
                ComputerSessionState::SessionUnavailable => return Err(backend_poisoned()),
                _ => return Ok(session.clone()),
            }
        }
        self.lifecycle.ensure_healthy()?;
        let operation_cancel = CancellationToken::new();
        let operation_guard =
            BackendOperationGuard::new(self.lifecycle.clone(), operation_cancel.clone(), None);
        let backend = self.backend.clone();
        let task_cancel = operation_cancel.clone();
        let task_lifecycle = self.lifecycle.clone();
        let task = tokio::spawn(async move {
            let _backend_guard = cancellable_backend_gate(&task_lifecycle, &task_cancel).await?;
            task_lifecycle.ensure_healthy()?;
            backend.open(task_cancel).await
        });
        let probe = cancellable(
            self.policy.operation_timeout,
            self.policy.cancellation_cleanup_timeout,
            cancel,
            operation_cancel,
            operation_guard,
            task,
        )
        .await?;
        if !probe
            .capabilities
            .intersect(self.policy.allowed_capabilities)
            .observe
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::UnsupportedCapability,
                "current desktop observation is not available under effective policy",
                RetryClassification::AfterPermissionChange,
            ));
        }
        let session = Arc::new(LocalComputerSession::new(
            self.process_instance_id.clone(),
            self.policy.clone(),
            self.backend.clone(),
            self.lifecycle.clone(),
            self.started_at,
            probe,
        ));
        *slot = Some(session.clone());
        drop(slot);
        Ok(session)
    }

    #[allow(clippy::significant_drop_tightening)]
    async fn shutdown(&self, reason: CloseReason) -> Result<ShutdownReceipt, ComputerUseError> {
        // Keep the service slot fenced until close reaches its bounded result,
        // so a concurrent open cannot race ahead of lifecycle closure.
        let mut slot = self.session_slot.lock().await;
        let session = slot.take();
        if let Some(session) = session {
            let result = session.close(reason).await;
            drop(slot);
            result
        } else {
            // Serialize shutdown with pre-session probes. A dropped call
            // poisons synchronously, so shutdown must not wait for that
            // quarantined task's still-owned gate. Confirmed closure is
            // idempotent and must not be confused with the shared wake token.
            if self.lifecycle.is_closed() {
                return Ok(closed_without_cleanup());
            }
            if self.lifecycle.is_poisoned() {
                return Err(backend_poisoned());
            }
            let _backend_guard = tokio::select! {
                biased;
                () = self.lifecycle.unavailable.cancelled() => {
                    if self.lifecycle.is_closed() {
                        return Ok(closed_without_cleanup());
                    }
                    return Err(backend_poisoned());
                }
                guard = self.lifecycle.gate.lock() => guard,
            };
            if self.lifecycle.is_closed() {
                return Ok(closed_without_cleanup());
            }
            if self.lifecycle.is_poisoned() {
                return Err(backend_poisoned());
            }
            self.lifecycle.mark_closed();
            drop(slot);
            Ok(closed_without_cleanup())
        }
    }
}

struct ObservationRecord {
    process_instance_id: ProcessInstanceId,
    session_id: ComputerSessionId,
    observation_id: ObservationId,
    target_generation: crate::TargetGeneration,
    layout_generation: crate::LayoutGeneration,
    effect_epoch: EffectEpoch,
    presence_epoch: TakeoverEpoch,
    geometry: GeometrySnapshot,
    image_sha256: [u8; 32],
    captured_at: Instant,
}

struct IdempotencyEntry {
    digest: [u8; 32],
    receipt: ComputerActionReceipt,
}

struct SessionData {
    state: ComputerSessionState,
    probe: BackendProbe,
    frame_generation: FrameGeneration,
    effect_epoch: EffectEpoch,
    sequence: OperationSequence,
    takeover_epoch: TakeoverEpoch,
    observations: HashMap<ObservationId, ObservationRecord>,
    observation_order: VecDeque<ObservationId>,
    current_layout_generation: Option<crate::LayoutGeneration>,
    cleanup_blocked: bool,
    idempotency: HashMap<OperationId, IdempotencyEntry>,
    idempotency_order: VecDeque<OperationId>,
}

pub struct LocalComputerSession {
    process_instance_id: ProcessInstanceId,
    session_id: ComputerSessionId,
    policy: Arc<ComputerUsePolicy>,
    backend: DynNativeDesktopBackend,
    lifecycle: Arc<BackendLifecycle>,
    started_at: Instant,
    capabilities_bits: Arc<AtomicU8>,
    state: Arc<Mutex<SessionData>>,
}

impl LocalComputerSession {
    fn new(
        process_instance_id: ProcessInstanceId,
        policy: Arc<ComputerUsePolicy>,
        backend: DynNativeDesktopBackend,
        lifecycle: Arc<BackendLifecycle>,
        started_at: Instant,
        probe: BackendProbe,
    ) -> Self {
        let capabilities = probe.capabilities.intersect(policy.allowed_capabilities);
        let state = if capabilities.pointer || capabilities.keyboard {
            ComputerSessionState::ReadyControl
        } else {
            ComputerSessionState::ReadyObserveOnly
        };
        Self {
            process_instance_id,
            session_id: ComputerSessionId::new(),
            policy,
            backend,
            lifecycle,
            started_at,
            capabilities_bits: Arc::new(AtomicU8::new(capabilities_to_bits(capabilities))),
            state: Arc::new(Mutex::new(SessionData {
                state,
                probe,
                frame_generation: FrameGeneration(0),
                effect_epoch: EffectEpoch(0),
                sequence: OperationSequence(0),
                takeover_epoch: TakeoverEpoch(0),
                observations: HashMap::new(),
                observation_order: VecDeque::new(),
                current_layout_generation: None,
                cleanup_blocked: false,
                idempotency: HashMap::new(),
                idempotency_order: VecDeque::new(),
            })),
        }
    }

    fn monotonic_ms(&self) -> u64 {
        u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn mark_unavailable(&self, data: &mut SessionData) {
        mark_session_unavailable(&self.capabilities_bits, data);
    }

    fn poison_projection(&self) -> SessionPoisonProjection {
        SessionPoisonProjection {
            capabilities_bits: self.capabilities_bits.clone(),
            state: self.state.clone(),
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn capture_locked(
        &self,
        data: &mut SessionData,
        include_accessibility: bool,
        cancel: CancellationToken,
    ) -> Result<ComputerObservation, ComputerUseError> {
        if cancel.is_cancelled() {
            return Err(ComputerUseError::cancelled());
        }
        ensure_session_operable(data.state)?;
        if !self.capabilities().observe {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::UnsupportedCapability,
                "desktop observation is not granted",
                RetryClassification::AfterPermissionChange,
            ));
        }
        if include_accessibility
            && (!self.policy.allowed_capabilities.observe
                || !self.policy.allowed_capabilities.accessibility_snapshot)
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::PolicyDenied,
                "accessibility observation is disabled by host policy",
                RetryClassification::Never,
            ));
        }
        self.lifecycle.ensure_healthy()?;
        if cancel.is_cancelled() {
            return Err(ComputerUseError::cancelled());
        }
        let previous_state = data.state;
        data.state = ComputerSessionState::Operating;
        let operation_cancel = CancellationToken::new();
        let operation_guard = BackendOperationGuard::new(
            self.lifecycle.clone(),
            operation_cancel.clone(),
            Some(self.poison_projection()),
        );
        let backend = self.backend.clone();
        let task_cancel = operation_cancel.clone();
        let task_lifecycle = self.lifecycle.clone();
        let task = tokio::spawn(async move {
            let _backend_guard = cancellable_backend_gate(&task_lifecycle, &task_cancel).await?;
            task_lifecycle.ensure_healthy()?;
            let observation = backend
                .observe(include_accessibility, task_cancel.clone())
                .await;
            let permission_probe = if include_accessibility
                && observation.as_ref().is_err_and(|error| {
                    matches!(
                        error.code,
                        ComputerUseErrorCode::PermissionRequired
                            | ComputerUseErrorCode::PermissionDenied
                            | ComputerUseErrorCode::PermissionRestartRequired
                    )
                }) {
                Some(backend.probe(task_cancel).await)
            } else {
                None
            };
            Ok((observation, permission_probe))
        });
        let outcome = cancellable(
            self.policy.operation_timeout,
            self.policy.cancellation_cleanup_timeout,
            cancel,
            operation_cancel,
            operation_guard,
            task,
        )
        .await;
        if self.lifecycle.is_poisoned() {
            self.mark_unavailable(data);
        }
        let mut native = match outcome {
            Ok((Ok(native), _)) => {
                data.state = previous_state;
                native
            }
            Ok((Err(error), permission_probe)) => {
                // A generic permission error is not enough to infer an
                // Accessibility-only failure: Screen Recording can fail with
                // the same code. Preserve the pixel
                // session only when a fresh passive probe, taken under the
                // same backend gate, proves pixel authority and target
                // identity remain valid while Accessibility remains absent.
                let fresh_accessibility_wait =
                    permission_probe.and_then(Result::ok).filter(|probe| {
                        include_accessibility
                            && probe.permissions.active_session == ActiveSessionStatus::Active
                            && probe.capabilities.observe
                            && probe.permissions.accessibility
                                != PermissionCapabilityStatus::Granted
                            && probe.target_generation == data.probe.target_generation
                    });
                if let Some(probe) = fresh_accessibility_wait {
                    clear_observations(data);
                    data.probe = probe;
                    let effective = data
                        .probe
                        .capabilities
                        .intersect(self.policy.allowed_capabilities);
                    self.capabilities_bits
                        .store(capabilities_to_bits(effective), Ordering::Release);
                    data.state = previous_state;
                } else if invalidates_session(&error) {
                    self.mark_unavailable(data);
                } else if !self.lifecycle.is_poisoned() {
                    data.state = previous_state;
                }
                return Err(error);
            }
            Err(error) => {
                if invalidates_session(&error) {
                    self.mark_unavailable(data);
                } else if !self.lifecycle.is_poisoned() {
                    data.state = previous_state;
                }
                return Err(error);
            }
        };
        let post_capture_accessibility = native
            .post_capture_probe
            .as_ref()
            .map(|probe| probe.capabilities.accessibility_snapshot);
        if let Some(post_capture_probe) = native.post_capture_probe.take() {
            if post_capture_probe.target_generation != data.probe.target_generation
                || !post_capture_probe.capabilities.observe
            {
                self.mark_unavailable(data);
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::StaleTarget,
                    "the active desktop authority changed during capture",
                    RetryClassification::AfterFreshObservation,
                ));
            }
            data.probe = post_capture_probe;
            let effective = data
                .probe
                .capabilities
                .intersect(self.policy.allowed_capabilities);
            self.capabilities_bits
                .store(capabilities_to_bits(effective), Ordering::Release);
        }
        native.geometry.validate().map_err(|message| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidTransform,
                message,
                RetryClassification::AfterFreshObservation,
            )
        })?;
        match (include_accessibility, native.accessibility.as_ref()) {
            (true, Some(snapshot)) => {
                if post_capture_accessibility == Some(false) {
                    self.mark_unavailable(data);
                    return Err(ComputerUseError::new(
                        ComputerUseErrorCode::PermissionRequired,
                        "Accessibility authority changed during capture",
                        RetryClassification::AfterPermissionChange,
                    ));
                }
                validate_accessibility_snapshot(&self.policy, &native.geometry, snapshot)?;
                data.probe.capabilities.accessibility_snapshot = true;
                data.probe.permissions.accessibility = PermissionCapabilityStatus::Granted;
                let effective = data
                    .probe
                    .capabilities
                    .intersect(self.policy.allowed_capabilities);
                self.capabilities_bits
                    .store(capabilities_to_bits(effective), Ordering::Release);
            }
            (true, None) => {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::BackendUnavailable,
                    "the backend omitted a requested accessibility snapshot",
                    RetryClassification::AfterPermissionChange,
                ));
            }
            (false, Some(_)) => {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::Internal,
                    "the backend returned accessibility content without a request",
                    RetryClassification::Never,
                ));
            }
            (false, None) => {}
        }
        validate_image_policy(&self.policy, &native)?;
        if matches!(
            native.redaction,
            FrameRedactionStatus::Protected | FrameRedactionStatus::Redacted
        ) {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::ProtectedOrRedactedFrame,
                "the desktop frame is protected or redacted",
                RetryClassification::AfterFreshObservation,
            ));
        }
        if native.geometry.target_generation != data.probe.target_generation {
            self.mark_unavailable(data);
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleTarget,
                "the active desktop target changed during capture",
                RetryClassification::AfterFreshObservation,
            ));
        }
        if data
            .current_layout_generation
            .is_some_and(|generation| generation != native.geometry.layout_generation)
        {
            clear_observations(data);
        }
        data.current_layout_generation = Some(native.geometry.layout_generation);
        evict_expired_observations(data, self.policy.observation_max_age);
        let digest: [u8; 32] = Sha256::digest(&native.image_bytes).into();
        data.frame_generation.0 = data.frame_generation.0.saturating_add(1);
        let observation_id = ObservationId::new();
        let captured_at = Instant::now();
        let capabilities = self.capabilities();
        let observation = ComputerObservation {
            process_instance_id: self.process_instance_id.clone(),
            session_id: self.session_id.clone(),
            observation_id: observation_id.clone(),
            target_generation: native.geometry.target_generation,
            layout_generation: native.geometry.layout_generation,
            frame_generation: data.frame_generation,
            effect_epoch: data.effect_epoch,
            captured_at_monotonic_ms: self.monotonic_ms(),
            geometry: native.geometry.clone(),
            image: EncodedDesktopImage {
                mime_type: native.mime_type,
                bytes: native.image_bytes,
                size_px: native.geometry.model_size_px,
                sha256: digest,
                color_space: native.color_space,
                redaction: native.redaction,
            },
            accessibility: native.accessibility,
            capabilities,
            session_state: previous_state,
        };
        data.observation_order.push_back(observation_id.clone());
        data.observations.insert(
            observation_id.clone(),
            ObservationRecord {
                process_instance_id: self.process_instance_id.clone(),
                session_id: self.session_id.clone(),
                observation_id,
                target_generation: observation.target_generation,
                layout_generation: observation.layout_generation,
                effect_epoch: observation.effect_epoch,
                presence_epoch: data.takeover_epoch,
                geometry: observation.geometry.clone(),
                image_sha256: digest,
                captured_at,
            },
        );
        while data.observations.len() > self.policy.max_observations {
            let Some(oldest) = data.observation_order.pop_front() else {
                break;
            };
            data.observations.remove(&oldest);
        }
        Ok(observation)
    }

    fn validate_action(
        &self,
        action: &ComputerAction,
        geometry: &GeometrySnapshot,
    ) -> Result<(), ComputerUseError> {
        let action_policy = &self.policy.action;
        let require_point = |point| {
            if geometry.model_size_px.contains(point) {
                Ok(())
            } else {
                Err(ComputerUseError::new(
                    ComputerUseErrorCode::InvalidCoordinate,
                    "action coordinate is outside the cited observation",
                    RetryClassification::AfterFreshObservation,
                ))
            }
        };
        match action {
            ComputerAction::Click(value) => {
                require_point(value.point)?;
                if value.click_count == 0 || value.click_count > action_policy.max_click_count {
                    return Err(ComputerUseError::invalid("click_count exceeds policy"));
                }
                validate_modifiers(&value.modifiers, action_policy.max_modifiers)?;
            }
            ComputerAction::MovePointer(value) => {
                require_point(value.point)?;
                validate_duration(value.duration_ms, action_policy.max_duration_ms)?;
            }
            ComputerAction::Drag(value) => {
                if value.path.len() < 2 || value.path.len() > action_policy.max_path_points {
                    return Err(ComputerUseError::invalid("drag path length exceeds policy"));
                }
                for point in &value.path {
                    require_point(*point)?;
                }
                validate_duration(value.duration_ms, action_policy.max_duration_ms)?;
                validate_modifiers(&value.modifiers, action_policy.max_modifiers)?;
            }
            ComputerAction::Scroll(value) => {
                require_point(value.anchor)?;
                if value.delta_x_model_px.unsigned_abs()
                    > action_policy.max_scroll_abs.unsigned_abs()
                    || value.delta_y_model_px.unsigned_abs()
                        > action_policy.max_scroll_abs.unsigned_abs()
                {
                    return Err(ComputerUseError::invalid("scroll magnitude exceeds policy"));
                }
                validate_modifiers(&value.modifiers, action_policy.max_modifiers)?;
            }
            ComputerAction::TypeText(value) => {
                if value.text.len() > action_policy.max_text_bytes
                    || value.text.chars().count() > action_policy.max_text_scalars
                {
                    return Err(ComputerUseError::invalid("text length exceeds policy"));
                }
            }
            ComputerAction::PressKeys(value) => {
                if value.keys.is_empty() || value.keys.len() > action_policy.max_keys {
                    return Err(ComputerUseError::invalid("key count exceeds policy"));
                }
            }
        }
        Ok(())
    }

    fn remember_idempotency(
        &self,
        data: &mut SessionData,
        operation_id: OperationId,
        digest: [u8; 32],
        receipt: ComputerActionReceipt,
    ) {
        let is_new = !data.idempotency.contains_key(&operation_id);
        data.idempotency
            .insert(operation_id.clone(), IdempotencyEntry { digest, receipt });
        if is_new {
            data.idempotency_order.push_back(operation_id);
        }
        while data.idempotency.len() > self.policy.idempotency.max_entries {
            if let Some(oldest) = data.idempotency_order.pop_front() {
                data.idempotency.remove(&oldest);
            } else {
                break;
            }
        }
    }
}

#[allow(clippy::significant_drop_tightening, clippy::too_many_lines)]
#[async_trait]
impl ComputerSession for LocalComputerSession {
    fn id(&self) -> ComputerSessionId {
        self.session_id.clone()
    }

    fn process_instance_id(&self) -> ProcessInstanceId {
        self.process_instance_id.clone()
    }

    fn capabilities(&self) -> EffectiveComputerCapabilities {
        if self.lifecycle.is_poisoned() {
            return EffectiveComputerCapabilities::default();
        }
        bits_to_capabilities(self.capabilities_bits.load(Ordering::Acquire))
    }

    async fn status(&self, cancel: CancellationToken) -> Result<ComputerStatus, ComputerUseError> {
        self.status_with_queue_deadline(
            cancel,
            TokioInstant::now() + self.policy.queue_wait_timeout,
        )
        .await
    }

    async fn status_with_queue_deadline(
        &self,
        cancel: CancellationToken,
        queue_deadline: TokioInstant,
    ) -> Result<ComputerStatus, ComputerUseError> {
        let mut data = cancellable_lock_until(&self.state, queue_deadline, &cancel).await?;
        if cancel.is_cancelled() {
            return Err(ComputerUseError::cancelled());
        }
        if data.state == ComputerSessionState::Closed {
            return Err(session_closed());
        }
        let recovering = data.state == ComputerSessionState::SessionUnavailable;
        self.lifecycle.ensure_healthy()?;
        let operation_cancel = CancellationToken::new();
        let operation_guard = BackendOperationGuard::new(
            self.lifecycle.clone(),
            operation_cancel.clone(),
            Some(self.poison_projection()),
        );
        let backend = self.backend.clone();
        let task_cancel = operation_cancel.clone();
        let task_lifecycle = self.lifecycle.clone();
        let task = tokio::spawn(async move {
            let _backend_guard = cancellable_backend_gate(&task_lifecycle, &task_cancel).await?;
            task_lifecycle.ensure_healthy()?;
            backend.probe(task_cancel).await
        });
        let probe = cancellable(
            self.policy.operation_timeout,
            self.policy.cancellation_cleanup_timeout,
            cancel,
            operation_cancel,
            operation_guard,
            task,
        )
        .await;
        if self.lifecycle.is_poisoned() {
            self.mark_unavailable(&mut data);
        }
        let probe = probe?;
        let effective = probe
            .capabilities
            .intersect(self.policy.allowed_capabilities);
        let invalidated = probe_invalidates_session(&data.probe, &probe, effective);
        data.probe = probe.clone();
        if invalidated {
            self.mark_unavailable(&mut data);
        } else {
            self.capabilities_bits
                .store(capabilities_to_bits(effective), Ordering::Release);
            if recovering {
                data.state = if data.cleanup_blocked {
                    ComputerSessionState::Paused
                } else {
                    ready_state(effective)
                };
            }
        }
        Ok(status_from_probe(
            &probe,
            self.process_instance_id.clone(),
            Some(self.session_id.clone()),
            data.state,
            self.policy.desktop_scope,
            Some(data.effect_epoch),
            data.current_layout_generation,
            if invalidated {
                EffectiveComputerCapabilities::default()
            } else {
                effective
            },
        ))
    }

    async fn observe(
        &self,
        request: ObserveRequest,
        cancel: CancellationToken,
    ) -> Result<ComputerObservation, ComputerUseError> {
        self.observe_with_queue_deadline(
            request,
            cancel,
            TokioInstant::now() + self.policy.queue_wait_timeout,
        )
        .await
    }

    async fn observe_with_queue_deadline(
        &self,
        request: ObserveRequest,
        cancel: CancellationToken,
        queue_deadline: TokioInstant,
    ) -> Result<ComputerObservation, ComputerUseError> {
        if cancel.is_cancelled() {
            return Err(ComputerUseError::cancelled());
        }
        let mut data = cancellable_lock_until(&self.state, queue_deadline, &cancel).await?;
        self.capture_locked(&mut data, request.include_accessibility, cancel)
            .await
    }

    #[allow(clippy::too_many_lines)]
    async fn act(
        &self,
        request: ComputerActionRequest,
        cancel: CancellationToken,
    ) -> Result<ComputerActionResult, ComputerUseFailure> {
        self.act_with_queue_deadline(
            request,
            cancel,
            TokioInstant::now() + self.policy.queue_wait_timeout,
        )
        .await
    }

    async fn act_with_queue_deadline(
        &self,
        request: ComputerActionRequest,
        cancel: CancellationToken,
        queue_deadline: TokioInstant,
    ) -> Result<ComputerActionResult, ComputerUseFailure> {
        if cancel.is_cancelled() {
            return Err(ComputerUseFailure::not_executed(
                ComputerUseError::cancelled(),
            ));
        }
        let digest = action_digest(&request).map_err(ComputerUseFailure::not_executed)?;
        let mut data = cancellable_lock_until(&self.state, queue_deadline, &cancel)
            .await
            .map_err(ComputerUseFailure::not_executed)?;
        if cancel.is_cancelled() {
            return Err(ComputerUseFailure::not_executed(
                ComputerUseError::cancelled(),
            ));
        }
        if let Some(entry) = data.idempotency.get(&request.operation_id) {
            if entry.digest != digest {
                return Err(ComputerUseFailure::not_executed(ComputerUseError::new(
                    ComputerUseErrorCode::IdempotencyConflict,
                    "operation identity was reused with different arguments",
                    RetryClassification::Never,
                )));
            }
            return Err(ComputerUseFailure {
                error: ComputerUseError::new(
                    ComputerUseErrorCode::DuplicateResultEvicted,
                    "the effect already completed; its image result is no longer retained",
                    RetryClassification::Never,
                ),
                effect_status: entry.receipt.effect_status,
                receipt: Some(entry.receipt.clone()),
            });
        }

        ensure_session_operable(data.state).map_err(ComputerUseFailure::not_executed)?;
        self.lifecycle
            .ensure_healthy()
            .map_err(ComputerUseFailure::not_executed)?;

        let record = data
            .observations
            .get(&request.observation.observation_id)
            .ok_or_else(|| {
                ComputerUseFailure::not_executed(ComputerUseError::new(
                    ComputerUseErrorCode::StaleObservation,
                    "observation is unknown, evicted, or stale",
                    RetryClassification::AfterFreshObservation,
                ))
            })?;
        validate_basis(
            record,
            &self.process_instance_id,
            &self.session_id,
            data.probe.target_generation,
            data.effect_epoch,
            data.takeover_epoch,
            self.policy.observation_max_age,
        )
        .map_err(ComputerUseFailure::not_executed)?;
        let geometry = record.geometry.clone();
        let basis_target = record.target_generation;
        let basis_layout = record.layout_generation;
        let basis_epoch = record.effect_epoch;
        let basis_id = record.observation_id.clone();
        let _basis_digest = record.image_sha256;
        self.validate_action(&request.action, &geometry)
            .map_err(ComputerUseFailure::not_executed)?;

        let capabilities = self.capabilities();
        let granted = match request.action {
            ComputerAction::Click(_)
            | ComputerAction::MovePointer(_)
            | ComputerAction::Drag(_)
            | ComputerAction::Scroll(_) => capabilities.pointer,
            ComputerAction::TypeText(_) | ComputerAction::PressKeys(_) => capabilities.keyboard,
        };
        if !granted {
            return Err(ComputerUseFailure::not_executed(ComputerUseError::new(
                ComputerUseErrorCode::UnsupportedCapability,
                "the requested input capability is not granted",
                RetryClassification::Never,
            )));
        }
        if cancel.is_cancelled() {
            return Err(ComputerUseFailure::not_executed(
                ComputerUseError::cancelled(),
            ));
        }

        data.sequence.0 = data.sequence.0.saturating_add(1);
        let sequence = data.sequence;
        data.effect_epoch.0 = data.effect_epoch.0.saturating_add(1);
        let resulting_epoch = data.effect_epoch;
        clear_observations(&mut data);
        self.lifecycle
            .ensure_healthy()
            .map_err(ComputerUseFailure::not_executed)?;
        if cancel.is_cancelled() {
            return Err(ComputerUseFailure::not_executed(
                ComputerUseError::cancelled(),
            ));
        }
        data.state = ComputerSessionState::Operating;
        let started = self.monotonic_ms();
        let reservation_native = uncertain_native_receipt();
        let reservation = build_receipt(
            &request,
            sequence,
            digest,
            &self.process_instance_id,
            &self.session_id,
            basis_target,
            basis_id.clone(),
            basis_layout,
            basis_epoch,
            resulting_epoch,
            started,
            started,
            reservation_native,
        );
        self.remember_idempotency(&mut data, request.operation_id.clone(), digest, reservation);

        let operation_cancel = CancellationToken::new();
        let operation_guard = BackendOperationGuard::new(
            self.lifecycle.clone(),
            operation_cancel.clone(),
            Some(self.poison_projection()),
        );
        let backend = self.backend.clone();
        let action = request.action.clone();
        let task_geometry = geometry.clone();
        let task_cancel = operation_cancel.clone();
        let task_lifecycle = self.lifecycle.clone();
        let task = tokio::spawn(async move {
            let _backend_guard = match cancellable_backend_gate(&task_lifecycle, &task_cancel).await
            {
                Ok(guard) => guard,
                Err(error) => {
                    return Err(crate::NativeActionFailure {
                        error,
                        effect_status: EffectStatus::NotExecuted,
                        receipt: None,
                        cleanup: InputCleanupStatus::NotRequired,
                    });
                }
            };
            if let Err(error) = task_lifecycle.ensure_healthy() {
                return Err(crate::NativeActionFailure {
                    error,
                    effect_status: EffectStatus::NotExecuted,
                    receipt: None,
                    cleanup: InputCleanupStatus::NotRequired,
                });
            }
            backend.execute(&action, &task_geometry, task_cancel).await
        });
        let native_result = cancellable_action(
            self.policy.operation_timeout,
            self.policy.cancellation_cleanup_timeout,
            cancel.clone(),
            operation_cancel,
            operation_guard,
            task,
        )
        .await;
        if self.lifecycle.is_poisoned() {
            self.mark_unavailable(&mut data);
        } else {
            data.state = if capabilities.pointer || capabilities.keyboard {
                ComputerSessionState::ReadyControl
            } else {
                ComputerSessionState::ReadyObserveOnly
            };
        }

        let native = match native_result {
            Ok(receipt) => receipt,
            Err(failure) => {
                let session_invalidated = invalidates_session(&failure.error);
                if session_invalidated {
                    self.mark_unavailable(&mut data);
                }
                let native = failure.receipt.unwrap_or(crate::NativeActionReceipt {
                    effect_status: failure.effect_status,
                    native_event_count: 0,
                    transformed_points: Vec::new(),
                    cleanup: failure.cleanup,
                    stability_check: StabilityCheckStatus::NotPerformed,
                });
                let receipt = build_receipt(
                    &request,
                    sequence,
                    digest,
                    &self.process_instance_id,
                    &self.session_id,
                    basis_target,
                    basis_id,
                    basis_layout,
                    basis_epoch,
                    resulting_epoch,
                    started,
                    self.monotonic_ms(),
                    native,
                );
                self.remember_idempotency(&mut data, request.operation_id, digest, receipt.clone());
                let cleanup_error =
                    if !cleanup_confirmed(receipt.cleanup) && !self.lifecycle.is_poisoned() {
                        data.cleanup_blocked = true;
                        data.takeover_epoch.0 = data.takeover_epoch.0.saturating_add(1);
                        if !session_invalidated {
                            data.state = ComputerSessionState::Paused;
                        }
                        Some(input_cleanup_failed(
                            "native input completed without confirmed held-input cleanup",
                        ))
                    } else {
                        None
                    };
                return Err(ComputerUseFailure {
                    error: cleanup_error.unwrap_or(failure.error),
                    effect_status: receipt.effect_status,
                    receipt: Some(receipt),
                });
            }
        };

        let receipt = build_receipt(
            &request,
            sequence,
            digest,
            &self.process_instance_id,
            &self.session_id,
            basis_target,
            basis_id,
            basis_layout,
            basis_epoch,
            resulting_epoch,
            started,
            self.monotonic_ms(),
            native,
        );
        if !cleanup_confirmed(receipt.cleanup) {
            self.remember_idempotency(
                &mut data,
                request.operation_id.clone(),
                digest,
                receipt.clone(),
            );
            data.state = ComputerSessionState::Paused;
            data.cleanup_blocked = true;
            data.takeover_epoch.0 = data.takeover_epoch.0.saturating_add(1);
            clear_observations(&mut data);
            return Err(ComputerUseFailure {
                error: input_cleanup_failed(
                    "native input completed without confirmed held-input cleanup",
                ),
                effect_status: receipt.effect_status,
                receipt: Some(receipt),
            });
        }
        if receipt.effect_status != EffectStatus::Executed {
            self.remember_idempotency(&mut data, request.operation_id, digest, receipt.clone());
            return Err(ComputerUseFailure {
                error: ComputerUseError::new(
                    ComputerUseErrorCode::InputDeliveryUncertain,
                    "native input did not complete with a known executed result",
                    RetryClassification::EffectStatusDependent,
                ),
                effect_status: receipt.effect_status,
                receipt: Some(receipt),
            });
        }

        if !self.policy.post_action_settle.is_zero() {
            let settle_error = tokio::select! {
                biased;
                () = cancel.cancelled() => Some(ComputerUseError::cancelled()),
                () = tokio::time::sleep(self.policy.post_action_settle) => None,
                () = tokio::time::sleep(self.policy.operation_timeout) => Some(ComputerUseError::new(
                    ComputerUseErrorCode::TimedOut,
                    "post-action settle timed out",
                    RetryClassification::EffectStatusDependent,
                )),
            };
            if let Some(error) = settle_error {
                self.remember_idempotency(&mut data, request.operation_id, digest, receipt.clone());
                return Err(ComputerUseFailure {
                    error,
                    effect_status: EffectStatus::Executed,
                    receipt: Some(receipt),
                });
            }
        }
        let observation = self.capture_locked(&mut data, false, cancel).await;
        self.remember_idempotency(&mut data, request.operation_id, digest, receipt.clone());
        match observation {
            Ok(observation) => Ok(ComputerActionResult {
                receipt,
                observation,
            }),
            Err(error) => Err(ComputerUseFailure {
                error,
                effect_status: EffectStatus::Executed,
                receipt: Some(receipt),
            }),
        }
    }

    async fn pause(&self, _reason: PauseReason) -> Result<PauseReceipt, ComputerUseError> {
        let mut data = self.state.lock().await;
        ensure_session_operable(data.state)?;
        data.state = ComputerSessionState::Paused;
        data.takeover_epoch.0 = data.takeover_epoch.0.saturating_add(1);
        clear_observations(&mut data);
        Ok(PauseReceipt {
            state: data.state,
            takeover_epoch: data.takeover_epoch,
        })
    }

    async fn close(&self, reason: CloseReason) -> Result<CloseReceipt, ComputerUseError> {
        let mut data = self.state.lock().await;
        if data.state == ComputerSessionState::Closed {
            return Ok(CloseReceipt {
                state: ComputerSessionState::Closed,
                cleanup: InputCleanupStatus::NotRequired,
            });
        }
        if self.lifecycle.is_poisoned() {
            self.mark_unavailable(&mut data);
            return Err(backend_poisoned());
        }
        self.lifecycle.ensure_healthy()?;
        data.state = ComputerSessionState::Closing;
        clear_observations(&mut data);
        let operation_cancel = CancellationToken::new();
        let operation_guard = BackendOperationGuard::new(
            self.lifecycle.clone(),
            operation_cancel,
            Some(self.poison_projection()),
        );
        let backend = self.backend.clone();
        let task_lifecycle = self.lifecycle.clone();
        let task = tokio::spawn(async move {
            let _backend_guard = task_lifecycle.gate.lock().await;
            task_lifecycle.ensure_healthy()?;
            backend.close(reason).await
        });
        let cleanup = bounded_close(self.policy.operation_timeout, operation_guard, task).await;
        match cleanup {
            Ok(cleanup) => {
                data.state = ComputerSessionState::Closed;
                self.capabilities_bits.store(0, Ordering::Release);
                self.lifecycle.mark_closed();
                Ok(CloseReceipt {
                    state: data.state,
                    cleanup,
                })
            }
            Err(error) => {
                self.mark_unavailable(&mut data);
                Err(error)
            }
        }
    }
}

async fn cancellable_backend_gate<'a>(
    lifecycle: &'a BackendLifecycle,
    cancel: &CancellationToken,
) -> Result<MutexGuard<'a, ()>, ComputerUseError> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(ComputerUseError::cancelled()),
        guard = lifecycle.gate.lock() => Ok(guard),
    }
}

pub async fn cancellable_lock_until<'a, T>(
    mutex: &'a Mutex<T>,
    deadline: TokioInstant,
    cancel: &CancellationToken,
) -> Result<MutexGuard<'a, T>, ComputerUseError> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(ComputerUseError::cancelled()),
        result = tokio::time::timeout_at(deadline, mutex.lock()) => result.map_err(|_| {
            ComputerUseError::new(
                ComputerUseErrorCode::TimedOut,
                "computer operation queue wait timed out",
                RetryClassification::Never,
            )
        }),
    }
}

async fn cancellable<T>(
    timeout: std::time::Duration,
    cleanup_timeout: std::time::Duration,
    cancel: CancellationToken,
    operation_cancel: CancellationToken,
    mut operation_guard: BackendOperationGuard,
    mut task: JoinHandle<Result<T, ComputerUseError>>,
) -> Result<T, ComputerUseError>
where
    T: Send + 'static,
{
    let terminal_error = tokio::select! {
        biased;
        result = &mut task => {
            let Ok(result) = result else {
                operation_guard.poison();
                operation_guard.disarm();
                return Err(backend_task_failed());
            };
            operation_guard.disarm();
            return result;
        },
        () = cancel.cancelled() => ComputerUseError::cancelled(),
        () = tokio::time::sleep(timeout) => ComputerUseError::new(
            ComputerUseErrorCode::TimedOut,
            "computer operation timed out",
            RetryClassification::Never,
        ),
    };

    operation_cancel.cancel();
    let cleanup = tokio::time::timeout(cleanup_timeout, &mut task).await;
    if cleanup.is_ok_and(|result| result.is_ok()) {
        operation_guard.disarm();
        return Err(terminal_error);
    }
    // The owned task remains detached when the join handle is dropped.
    // Permanently poison the shared lifecycle before the serialized backend
    // gate is released, so no later call or close can overlap it.
    operation_guard.poison();
    operation_guard.disarm();
    Err(terminal_error)
}

async fn cancellable_action(
    timeout: std::time::Duration,
    cleanup_timeout: std::time::Duration,
    cancel: CancellationToken,
    operation_cancel: CancellationToken,
    mut operation_guard: BackendOperationGuard,
    mut task: JoinHandle<Result<crate::NativeActionReceipt, crate::NativeActionFailure>>,
) -> Result<crate::NativeActionReceipt, crate::NativeActionFailure> {
    let terminal_error = tokio::select! {
        biased;
        result = &mut task => {
            let Ok(result) = result else {
                operation_guard.poison();
                operation_guard.disarm();
                return Err(uncertain_action_failure());
            };
            operation_guard.disarm();
            return result;
        },
        () = cancel.cancelled() => ComputerUseError::cancelled(),
        () = tokio::time::sleep(timeout) => ComputerUseError::new(
            ComputerUseErrorCode::TimedOut,
            "native input operation timed out",
            RetryClassification::EffectStatusDependent,
        ),
    };

    operation_cancel.cancel();
    match tokio::time::timeout(cleanup_timeout, &mut task).await {
        Ok(Ok(Ok(receipt))) => {
            operation_guard.disarm();
            Err(crate::NativeActionFailure {
                error: terminal_error,
                effect_status: receipt.effect_status,
                cleanup: receipt.cleanup,
                receipt: Some(receipt),
            })
        }
        Ok(Ok(Err(mut failure))) => {
            operation_guard.disarm();
            failure.error = terminal_error;
            Err(failure)
        }
        Ok(Err(_)) | Err(_) => {
            operation_guard.poison();
            operation_guard.disarm();
            Err(uncertain_action_failure())
        }
    }
}

async fn bounded_close(
    timeout: std::time::Duration,
    mut operation_guard: BackendOperationGuard,
    mut task: JoinHandle<Result<InputCleanupStatus, ComputerUseError>>,
) -> Result<InputCleanupStatus, ComputerUseError> {
    match tokio::time::timeout(timeout, &mut task).await {
        Ok(Ok(Ok(cleanup))) if cleanup_confirmed(cleanup) => {
            operation_guard.disarm();
            Ok(cleanup)
        }
        Ok(Ok(Ok(_))) => {
            operation_guard.poison();
            operation_guard.disarm();
            Err(input_cleanup_failed(
                "computer backend close did not confirm cleanup",
            ))
        }
        Ok(Ok(Err(error))) => {
            operation_guard.poison();
            operation_guard.disarm();
            Err(error)
        }
        Ok(Err(_)) => {
            operation_guard.poison();
            operation_guard.disarm();
            Err(backend_task_failed())
        }
        Err(_) => {
            operation_guard.poison();
            operation_guard.disarm();
            Err(ComputerUseError::new(
                ComputerUseErrorCode::TimedOut,
                "computer backend close timed out; lifecycle is permanently unavailable",
                RetryClassification::Never,
            ))
        }
    }
}

const fn uncertain_native_receipt() -> crate::NativeActionReceipt {
    crate::NativeActionReceipt {
        effect_status: EffectStatus::DeliveryUncertain,
        native_event_count: 0,
        transformed_points: Vec::new(),
        cleanup: InputCleanupStatus::Failed,
        stability_check: StabilityCheckStatus::NotPerformed,
    }
}

fn uncertain_action_failure() -> crate::NativeActionFailure {
    let receipt = uncertain_native_receipt();
    crate::NativeActionFailure {
        error: ComputerUseError::new(
            ComputerUseErrorCode::InputDeliveryUncertain,
            "native input did not reach a terminal state; the backend lifecycle is permanently unavailable",
            RetryClassification::EffectStatusDependent,
        ),
        effect_status: receipt.effect_status,
        cleanup: receipt.cleanup,
        receipt: Some(receipt),
    }
}

fn backend_task_failed() -> ComputerUseError {
    ComputerUseError::new(
        ComputerUseErrorCode::BackendUnavailable,
        "computer backend task terminated unexpectedly; lifecycle is permanently unavailable",
        RetryClassification::Never,
    )
}

const fn cleanup_confirmed(cleanup: InputCleanupStatus) -> bool {
    matches!(
        cleanup,
        InputCleanupStatus::NotRequired | InputCleanupStatus::Complete
    )
}

fn input_cleanup_failed(message: &'static str) -> ComputerUseError {
    ComputerUseError::new(
        ComputerUseErrorCode::InputCleanupFailed,
        message,
        RetryClassification::AfterExplicitResume,
    )
}

#[allow(clippy::too_many_arguments)]
fn status_from_probe(
    probe: &BackendProbe,
    process_instance_id: ProcessInstanceId,
    session_id: Option<ComputerSessionId>,
    state: ComputerSessionState,
    scope: crate::DesktopSurfaceScope,
    effect_epoch: Option<EffectEpoch>,
    layout_generation: Option<crate::LayoutGeneration>,
    effective_capabilities: EffectiveComputerCapabilities,
) -> ComputerStatus {
    ComputerStatus {
        contract_version: ComputerUseContractVersion::V1,
        process_instance_id,
        session_id,
        state,
        platform: probe.platform,
        backend: probe.backend,
        desktop_scope: scope,
        active_session: probe.permissions.active_session,
        permissions: probe.permissions.clone(),
        effective_capabilities,
        target_generation: Some(probe.target_generation),
        layout_generation,
        effect_epoch,
        user_presence: probe.user_presence,
        diagnostics_code: probe.diagnostics_code.clone(),
    }
}

const fn ready_state(capabilities: EffectiveComputerCapabilities) -> ComputerSessionState {
    if capabilities.pointer || capabilities.keyboard {
        ComputerSessionState::ReadyControl
    } else {
        ComputerSessionState::ReadyObserveOnly
    }
}

fn mark_session_unavailable(capabilities_bits: &AtomicU8, data: &mut SessionData) {
    capabilities_bits.store(0, Ordering::Release);
    clear_observations(data);
    data.current_layout_generation = None;
    if data.state != ComputerSessionState::SessionUnavailable {
        data.effect_epoch.0 = data.effect_epoch.0.saturating_add(1);
        data.takeover_epoch.0 = data.takeover_epoch.0.saturating_add(1);
    }
    data.state = ComputerSessionState::SessionUnavailable;
}

fn probe_invalidates_session(
    previous: &BackendProbe,
    current: &BackendProbe,
    effective: EffectiveComputerCapabilities,
) -> bool {
    current.permissions.active_session != ActiveSessionStatus::Active
        || !effective.observe
        || current.target_generation != previous.target_generation
        || current.user_presence == UserPresenceStatus::Revoked
}

const fn invalidates_session(error: &ComputerUseError) -> bool {
    matches!(
        error.code,
        ComputerUseErrorCode::PermissionRequired
            | ComputerUseErrorCode::PermissionDenied
            | ComputerUseErrorCode::PermissionRestartRequired
            | ComputerUseErrorCode::SessionInactive
            | ComputerUseErrorCode::SessionLocked
            | ComputerUseErrorCode::SessionChanged
            | ComputerUseErrorCode::SecureDesktopUnavailable
            | ComputerUseErrorCode::UserPresenceRevoked
            | ComputerUseErrorCode::StaleTarget
            | ComputerUseErrorCode::DisplayTopologyChanged
    )
}

fn clear_observations(data: &mut SessionData) {
    data.observations.clear();
    data.observation_order.clear();
}

fn evict_expired_observations(data: &mut SessionData, max_age: std::time::Duration) {
    while let Some(oldest) = data.observation_order.front() {
        let expired = data
            .observations
            .get(oldest)
            .is_none_or(|record| record.captured_at.elapsed() > max_age);
        if !expired {
            break;
        }
        let Some(oldest) = data.observation_order.pop_front() else {
            break;
        };
        data.observations.remove(&oldest);
    }
}

fn ensure_session_operable(state: ComputerSessionState) -> Result<(), ComputerUseError> {
    match state {
        ComputerSessionState::ReadyObserveOnly | ComputerSessionState::ReadyControl => Ok(()),
        ComputerSessionState::Paused => Err(ComputerUseError::new(
            ComputerUseErrorCode::UserPresenceRevoked,
            "computer session is paused and requires attended recovery",
            RetryClassification::AfterExplicitResume,
        )),
        ComputerSessionState::Closed | ComputerSessionState::Closing => Err(session_closed()),
        ComputerSessionState::SessionUnavailable => Err(backend_poisoned()),
        _ => Err(ComputerUseError::new(
            ComputerUseErrorCode::SessionInactive,
            "computer session is not ready",
            RetryClassification::AfterExplicitResume,
        )),
    }
}

fn session_closed() -> ComputerUseError {
    ComputerUseError::new(
        ComputerUseErrorCode::SessionClosed,
        "computer session is closed",
        RetryClassification::NewSessionRequired,
    )
}

const fn closed_without_cleanup() -> CloseReceipt {
    CloseReceipt {
        state: ComputerSessionState::Closed,
        cleanup: InputCleanupStatus::NotRequired,
    }
}

fn backend_poisoned() -> ComputerUseError {
    ComputerUseError::new(
        ComputerUseErrorCode::BackendUnavailable,
        "computer backend lifecycle is permanently unavailable; restart the host process",
        RetryClassification::Never,
    )
}

#[allow(clippy::too_many_lines)]
fn validate_accessibility_snapshot(
    policy: &ComputerUsePolicy,
    geometry: &GeometrySnapshot,
    snapshot: &crate::AccessibilitySnapshot,
) -> Result<(), ComputerUseError> {
    let accessibility = &policy.accessibility;
    if snapshot.nodes.len() > accessibility.max_nodes {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            "accessibility snapshot exceeds the configured node budget",
            RetryClassification::Never,
        ));
    }
    if snapshot.truncated == snapshot.truncation_reasons.is_empty() {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            "accessibility truncation metadata is inconsistent",
            RetryClassification::Never,
        ));
    }

    let mut node_states = HashMap::<u64, (usize, bool)>::with_capacity(snapshot.nodes.len());
    let mut child_counts = HashMap::<u64, usize>::new();
    let mut total_string_bytes = 0_usize;
    for node in &snapshot.nodes {
        if node.role.is_empty() || node.role.len() > accessibility.max_string_bytes {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "accessibility node role violates the configured string budget",
                RetryClassification::Never,
            ));
        }
        for value in [node.name.as_ref(), node.value_summary.as_ref()]
            .into_iter()
            .flatten()
        {
            if value.len() > accessibility.max_string_bytes {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::BackendUnavailable,
                    "accessibility node string exceeds the configured budget",
                    RetryClassification::Never,
                ));
            }
        }
        total_string_bytes = total_string_bytes
            .checked_add(node.role.len())
            .and_then(|value| value.checked_add(node.name.as_ref().map_or(0, String::len)))
            .and_then(|value| value.checked_add(node.value_summary.as_ref().map_or(0, String::len)))
            .ok_or_else(|| {
                ComputerUseError::new(
                    ComputerUseErrorCode::BackendUnavailable,
                    "accessibility string accounting overflowed",
                    RetryClassification::Never,
                )
            })?;
        if total_string_bytes > accessibility.max_total_string_bytes {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "accessibility snapshot exceeds the total string budget",
                RetryClassification::Never,
            ));
        }
        let (depth, inherited_protection) = match node.parent_local_id {
            Some(parent) => {
                let (parent_depth, parent_protected) =
                    node_states.get(&parent).copied().ok_or_else(|| {
                        ComputerUseError::new(
                            ComputerUseErrorCode::BackendUnavailable,
                            "accessibility parent must precede its child",
                            RetryClassification::Never,
                        )
                    })?;
                let child_count = child_counts.entry(parent).or_default();
                *child_count = child_count.saturating_add(1);
                if *child_count > accessibility.max_children_per_node {
                    return Err(ComputerUseError::new(
                        ComputerUseErrorCode::BackendUnavailable,
                        "accessibility snapshot exceeds the per-node child budget",
                        RetryClassification::Never,
                    ));
                }
                (parent_depth + 1, parent_protected)
            }
            None => (0, false),
        };
        let effective_protection = inherited_protection || node.state.protected == Some(true);
        if effective_protection && node.value_summary.is_some() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "protected accessibility subtrees must not expose value summaries",
                RetryClassification::Never,
            ));
        }
        if depth > accessibility.max_depth
            || node_states
                .insert(node.local_id, (depth, effective_protection))
                .is_some()
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "accessibility snapshot contains invalid identity or depth",
                RetryClassification::Never,
            ));
        }
        if let Some(bounds) = node.model_bounds {
            let right = bounds.x.checked_add(bounds.width);
            let bottom = bounds.y.checked_add(bounds.height);
            if bounds.width == 0
                || bounds.height == 0
                || right.is_none_or(|value| value > geometry.model_size_px.width)
                || bottom.is_none_or(|value| value > geometry.model_size_px.height)
            {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::BackendUnavailable,
                    "accessibility bounds fall outside the captured model space",
                    RetryClassification::Never,
                ));
            }
        }
    }
    Ok(())
}

fn validate_image_policy(
    policy: &ComputerUsePolicy,
    native: &crate::NativeObservation,
) -> Result<(), ComputerUseError> {
    let size = native.geometry.model_size_px;
    let pixels = size.pixels().ok_or_else(|| {
        ComputerUseError::new(
            ComputerUseErrorCode::ImageLimitExceeded,
            "image dimensions overflow policy accounting",
            RetryClassification::Never,
        )
    })?;
    if size.width > policy.screenshot.max_width
        || size.height > policy.screenshot.max_height
        || pixels > policy.screenshot.max_pixels
        || u64::try_from(native.image_bytes.len()).unwrap_or(u64::MAX)
            > policy.screenshot.max_encoded_bytes
        || native.image_bytes.is_empty()
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::ImageLimitExceeded,
            "desktop image exceeds configured limits",
            RetryClassification::Never,
        ));
    }

    let expected_format = match native.mime_type {
        crate::DesktopImageMime::ImagePng => ImageFormat::Png,
        crate::DesktopImageMime::ImageJpeg => ImageFormat::Jpeg,
    };
    let actual_format = image::guess_format(&native.image_bytes)
        .map_err(|_| invalid_desktop_image("desktop image has no recognized encoded format"))?;
    if actual_format != expected_format {
        return Err(invalid_desktop_image(
            "desktop image MIME does not match its encoded format",
        ));
    }

    let mut reader = ImageReader::with_format(Cursor::new(&native.image_bytes), actual_format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(policy.screenshot.max_width);
    limits.max_image_height = Some(policy.screenshot.max_height);
    limits.max_alloc = Some(
        policy
            .screenshot
            .max_pixels
            .saturating_mul(16)
            .saturating_add(policy.screenshot.max_encoded_bytes),
    );
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|_| invalid_desktop_image("desktop image failed bounded decode validation"))?;
    if decoded.width() != size.width || decoded.height() != size.height {
        return Err(invalid_desktop_image(
            "desktop image dimensions do not match observation geometry",
        ));
    }
    Ok(())
}

fn invalid_desktop_image(message: &'static str) -> ComputerUseError {
    ComputerUseError::new(
        ComputerUseErrorCode::CaptureInterrupted,
        message,
        RetryClassification::AfterFreshObservation,
    )
}

fn validate_basis(
    record: &ObservationRecord,
    process: &ProcessInstanceId,
    session: &ComputerSessionId,
    target: crate::TargetGeneration,
    effect_epoch: EffectEpoch,
    takeover_epoch: TakeoverEpoch,
    max_age: std::time::Duration,
) -> Result<(), ComputerUseError> {
    if &record.process_instance_id != process {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleProcess,
            "observation belongs to another process instance",
            RetryClassification::NewSessionRequired,
        ));
    }
    if &record.session_id != session {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleSession,
            "observation belongs to another computer session",
            RetryClassification::NewSessionRequired,
        ));
    }
    if record.target_generation != target {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleTarget,
            "active desktop target changed",
            RetryClassification::AfterFreshObservation,
        ));
    }
    if record.effect_epoch != effect_epoch {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleObservation,
            "another input effect invalidated this observation",
            RetryClassification::AfterFreshObservation,
        ));
    }
    if record.presence_epoch != takeover_epoch {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::UserPresenceRevoked,
            "user takeover invalidated this observation",
            RetryClassification::AfterExplicitResume,
        ));
    }
    if record.captured_at.elapsed() > max_age {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::ObservationExpired,
            "observation exceeded its maximum age",
            RetryClassification::AfterFreshObservation,
        ));
    }
    if record.geometry.layout_generation != record.layout_generation {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleLayout,
            "observation geometry record is inconsistent",
            RetryClassification::AfterFreshObservation,
        ));
    }
    Ok(())
}

fn validate_modifiers(
    modifiers: &[crate::ModifierKey],
    max: usize,
) -> Result<(), ComputerUseError> {
    if modifiers.len() > max {
        Err(ComputerUseError::invalid("modifier count exceeds policy"))
    } else {
        Ok(())
    }
}

fn validate_duration(value: u32, max: u32) -> Result<(), ComputerUseError> {
    if value > max {
        Err(ComputerUseError::invalid("action duration exceeds policy"))
    } else {
        Ok(())
    }
}

fn action_digest(request: &ComputerActionRequest) -> Result<[u8; 32], ComputerUseError> {
    #[derive(serde::Serialize)]
    struct DigestInput<'a> {
        observation: &'a ObservationRef,
        action: &'a ComputerAction,
    }
    let bytes = serde_json::to_vec(&DigestInput {
        observation: &request.observation,
        action: &request.action,
    })
    .map_err(|_| {
        ComputerUseError::new(
            ComputerUseErrorCode::Internal,
            "failed to canonicalize action request",
            RetryClassification::Never,
        )
    })?;
    Ok(Sha256::digest(bytes).into())
}

#[allow(clippy::too_many_arguments)]
fn build_receipt(
    request: &ComputerActionRequest,
    sequence: OperationSequence,
    digest: [u8; 32],
    process: &ProcessInstanceId,
    session: &ComputerSessionId,
    target: crate::TargetGeneration,
    basis_id: ObservationId,
    layout: crate::LayoutGeneration,
    basis_epoch: EffectEpoch,
    resulting_epoch: EffectEpoch,
    started: u64,
    completed: u64,
    native: crate::NativeActionReceipt,
) -> ComputerActionReceipt {
    ComputerActionReceipt {
        operation_id: request.operation_id.clone(),
        sequence,
        request_digest_hex: hex_digest(digest),
        effect_status: native.effect_status,
        action_kind: request.action.kind(),
        process_instance_id: process.clone(),
        session_id: session.clone(),
        target_generation: target,
        basis_observation_id: basis_id,
        basis_layout_generation: layout,
        basis_effect_epoch: basis_epoch,
        resulting_effect_epoch: resulting_epoch,
        native_event_count: native.native_event_count,
        transformed_points: native.transformed_points,
        cleanup: native.cleanup,
        stability_check: native.stability_check,
        started_at_monotonic_ms: started,
        completed_at_monotonic_ms: completed,
    }
}

fn hex_digest(digest: [u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn capabilities_to_bits(value: EffectiveComputerCapabilities) -> u8 {
    u8::from(value.observe)
        | (u8::from(value.pointer) << 1)
        | (u8::from(value.keyboard) << 2)
        | (u8::from(value.accessibility_snapshot) << 3)
}

const fn bits_to_capabilities(value: u8) -> EffectiveComputerCapabilities {
    EffectiveComputerCapabilities {
        observe: value & 1 != 0,
        pointer: value & 2 != 0,
        keyboard: value & 4 != 0,
        accessibility_snapshot: value & 8 != 0,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    use async_trait::async_trait;
    use starweaver_core::CancellationToken;
    use tokio::sync::Notify;

    use super::{
        BackendLifecycle, BackendOperationGuard, LocalComputerSession, cancellable,
        validate_accessibility_snapshot, validate_image_policy,
    };
    use crate::{
        ClickAction, CloseReason, ComputerAction, ComputerActionRequest, ComputerCapabilityGrant,
        ComputerSession, ComputerSessionState, ComputerUseError, ComputerUseErrorCode,
        ComputerUsePolicy, ComputerUseService, DesktopImageMime, EffectStatus,
        FakeComputerUseConfig, FakeNativeDesktopBackend, InputCleanupStatus,
        LocalComputerUseService, ModelPoint, NativeDesktopBackend, ObservationRef, ObserveRequest,
        OperationId, PermissionRequest, PointerButton, ProcessInstanceId, RetryClassification,
    };

    #[tokio::test]
    async fn protected_accessibility_value_is_rejected_at_service_boundary() {
        let mut policy = ComputerUsePolicy::default();
        policy.allowed_capabilities.accessibility_snapshot = true;
        let mut config = FakeComputerUseConfig::default();
        config.capabilities.accessibility_snapshot = true;
        let backend = FakeNativeDesktopBackend::new(config);
        let mut native = backend
            .observe(true, CancellationToken::new())
            .await
            .expect("fake Accessibility observation should succeed");
        let snapshot = native
            .accessibility
            .as_mut()
            .expect("fake Accessibility snapshot should be present");
        snapshot.nodes[0].state.protected = Some(true);
        snapshot.nodes[0].value_summary = Some("must not escape".into());

        let error = validate_accessibility_snapshot(&policy, &native.geometry, snapshot)
            .expect_err("protected values must be rejected");
        assert_eq!(error.code, ComputerUseErrorCode::BackendUnavailable);

        snapshot.nodes[0].value_summary = None;
        let mut child = snapshot.nodes[0].clone();
        child.local_id = 2;
        child.parent_local_id = Some(1);
        child.state.protected = Some(false);
        child.value_summary = Some("must not escape through a child".into());
        snapshot.nodes.push(child);
        let error = validate_accessibility_snapshot(&policy, &native.geometry, snapshot)
            .expect_err("ancestor protection must be inherited by the validator");
        assert_eq!(error.code, ComputerUseErrorCode::BackendUnavailable);
    }

    #[tokio::test]
    async fn queued_permission_cancellation_does_not_poison_backend_lifecycle() {
        let backend = Arc::new(FakeNativeDesktopBackend::new(
            FakeComputerUseConfig::default(),
        ));
        let service = Arc::new(LocalComputerUseService::new(
            short_timeout_policy(),
            backend,
        ));
        let gate = service.lifecycle.gate.clone();
        let held_gate = gate.lock().await;
        let cancel = CancellationToken::new();
        let request = {
            let service = service.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move {
                service
                    .request_permissions(
                        PermissionRequest {
                            screen_recording: true,
                            accessibility: true,
                        },
                        cancel,
                    )
                    .await
            })
        };
        tokio::task::yield_now().await;
        cancel.cancel();
        let error = tokio::time::timeout(Duration::from_millis(100), request)
            .await
            .expect("queued permission request should cancel promptly")
            .expect("permission task should not panic")
            .expect_err("queued permission request should be cancelled");
        assert_eq!(error.code, ComputerUseErrorCode::Cancelled);
        assert!(!service.lifecycle.is_poisoned());

        drop(held_gate);
        service
            .status(CancellationToken::new())
            .await
            .expect("backend should remain usable after queued cancellation");
    }

    const WEDGE_NONE: u8 = 0;
    const WEDGE_OBSERVE: u8 = 1;
    const WEDGE_ACTION: u8 = 2;
    const WEDGE_CLOSE: u8 = 3;
    const WEDGE_PROBE: u8 = 4;
    const CANCELLABLE_PROBE: u8 = 5;

    struct WedgeBackend {
        inner: FakeNativeDesktopBackend,
        wedge: AtomicU8,
        probe_calls: AtomicUsize,
        open_calls: AtomicUsize,
        observe_calls: AtomicUsize,
        action_calls: AtomicUsize,
        close_calls: AtomicUsize,
        action_cleanup_failed: AtomicBool,
        close_cleanup_failed: AtomicBool,
    }

    impl WedgeBackend {
        fn new() -> Self {
            Self {
                inner: FakeNativeDesktopBackend::new(FakeComputerUseConfig::default()),
                wedge: AtomicU8::new(WEDGE_NONE),
                probe_calls: AtomicUsize::new(0),
                open_calls: AtomicUsize::new(0),
                observe_calls: AtomicUsize::new(0),
                action_calls: AtomicUsize::new(0),
                close_calls: AtomicUsize::new(0),
                action_cleanup_failed: AtomicBool::new(false),
                close_cleanup_failed: AtomicBool::new(false),
            }
        }

        fn wedge(&self, operation: u8) {
            self.wedge.store(operation, Ordering::Release);
        }

        fn fail_action_cleanup(&self) {
            self.action_cleanup_failed.store(true, Ordering::Release);
        }

        fn fail_close_cleanup(&self) {
            self.close_cleanup_failed.store(true, Ordering::Release);
        }
    }

    #[async_trait]
    impl crate::NativeDesktopBackend for WedgeBackend {
        fn platform(&self) -> crate::NativeDesktopPlatform {
            self.inner.platform()
        }

        fn kind(&self) -> crate::NativeBackendKind {
            self.inner.kind()
        }

        async fn probe(
            &self,
            cancel: CancellationToken,
        ) -> Result<crate::BackendProbe, ComputerUseError> {
            self.probe_calls.fetch_add(1, Ordering::AcqRel);
            match self.wedge.load(Ordering::Acquire) {
                WEDGE_PROBE => return std::future::pending().await,
                CANCELLABLE_PROBE => {
                    cancel.cancelled().await;
                    return Err(ComputerUseError::cancelled());
                }
                _ => {}
            }
            crate::NativeDesktopBackend::probe(&self.inner, cancel).await
        }

        async fn open(
            &self,
            cancel: CancellationToken,
        ) -> Result<crate::BackendProbe, ComputerUseError> {
            self.open_calls.fetch_add(1, Ordering::AcqRel);
            crate::NativeDesktopBackend::open(&self.inner, cancel).await
        }

        async fn observe(
            &self,
            include_accessibility: bool,
            cancel: CancellationToken,
        ) -> Result<crate::NativeObservation, ComputerUseError> {
            self.observe_calls.fetch_add(1, Ordering::AcqRel);
            if self.wedge.load(Ordering::Acquire) == WEDGE_OBSERVE {
                return std::future::pending().await;
            }
            crate::NativeDesktopBackend::observe(&self.inner, include_accessibility, cancel).await
        }

        async fn execute(
            &self,
            action: &ComputerAction,
            geometry: &crate::GeometrySnapshot,
            cancel: CancellationToken,
        ) -> Result<crate::NativeActionReceipt, crate::NativeActionFailure> {
            self.action_calls.fetch_add(1, Ordering::AcqRel);
            if self.wedge.load(Ordering::Acquire) == WEDGE_ACTION {
                return std::future::pending().await;
            }
            let mut receipt =
                crate::NativeDesktopBackend::execute(&self.inner, action, geometry, cancel).await?;
            if self.action_cleanup_failed.load(Ordering::Acquire) {
                receipt.cleanup = InputCleanupStatus::Failed;
            }
            Ok(receipt)
        }

        async fn close(
            &self,
            reason: crate::CloseReason,
        ) -> Result<crate::InputCleanupStatus, ComputerUseError> {
            self.close_calls.fetch_add(1, Ordering::AcqRel);
            if self.wedge.load(Ordering::Acquire) == WEDGE_CLOSE {
                return std::future::pending().await;
            }
            if self.close_cleanup_failed.load(Ordering::Acquire) {
                return Ok(InputCleanupStatus::Failed);
            }
            crate::NativeDesktopBackend::close(&self.inner, reason).await
        }
    }

    fn short_timeout_policy() -> ComputerUsePolicy {
        ComputerUsePolicy {
            allowed_capabilities: ComputerCapabilityGrant {
                observe: true,
                pointer: true,
                keyboard: true,
                accessibility_snapshot: false,
            },
            operation_timeout: Duration::from_millis(30),
            cancellation_cleanup_timeout: Duration::from_millis(20),
            post_action_settle: Duration::ZERO,
            ..ComputerUsePolicy::default()
        }
    }

    async fn local_fake_session() -> (Arc<LocalComputerSession>, Arc<FakeNativeDesktopBackend>) {
        let backend = Arc::new(FakeNativeDesktopBackend::new(
            FakeComputerUseConfig::default(),
        ));
        let probe = crate::NativeDesktopBackend::open(backend.as_ref(), CancellationToken::new())
            .await
            .expect("fake backend should open");
        let policy = ComputerUsePolicy {
            allowed_capabilities: ComputerCapabilityGrant {
                observe: true,
                pointer: true,
                keyboard: true,
                accessibility_snapshot: false,
            },
            post_action_settle: Duration::ZERO,
            ..ComputerUsePolicy::default()
        };
        let backend_dyn: crate::DynNativeDesktopBackend = backend.clone();
        (
            Arc::new(LocalComputerSession::new(
                ProcessInstanceId::new(),
                Arc::new(policy),
                backend_dyn,
                Arc::new(BackendLifecycle::default()),
                Instant::now(),
                probe,
            )),
            backend,
        )
    }

    #[tokio::test]
    async fn geometry_bound_image_validation_rejects_malformed_mismatched_bytes() {
        let backend = FakeNativeDesktopBackend::new(FakeComputerUseConfig::default());
        let native =
            crate::NativeDesktopBackend::observe(&backend, false, CancellationToken::new())
                .await
                .expect("fake image should encode");
        let policy = ComputerUsePolicy::default();
        validate_image_policy(&policy, &native).expect("valid fake PNG should pass");

        let mut wrong_mime = native.clone();
        wrong_mime.mime_type = DesktopImageMime::ImageJpeg;
        assert_eq!(
            validate_image_policy(&policy, &wrong_mime)
                .expect_err("encoded format and MIME must agree")
                .code,
            ComputerUseErrorCode::CaptureInterrupted
        );

        let mut malformed = native.clone();
        malformed.image_bytes = vec![1, 2, 3, 4];
        assert_eq!(
            validate_image_policy(&policy, &malformed)
                .expect_err("malformed image bytes must fail")
                .code,
            ComputerUseErrorCode::CaptureInterrupted
        );

        let mut wrong_dimensions = native;
        wrong_dimensions.geometry.model_size_px.width = wrong_dimensions
            .geometry
            .model_size_px
            .width
            .saturating_sub(1);
        assert_eq!(
            validate_image_policy(&policy, &wrong_dimensions)
                .expect_err("decoded dimensions and geometry must agree")
                .code,
            ComputerUseErrorCode::CaptureInterrupted
        );
    }

    #[tokio::test]
    async fn queued_observe_cancelled_before_fence_entry_never_reaches_backend() {
        let (session, backend) = local_fake_session().await;
        backend
            .fail_next_observe(ComputerUseError::new(
                ComputerUseErrorCode::CaptureInterrupted,
                "unconsumed backend marker",
                RetryClassification::Never,
            ))
            .await;
        let guard = session.state.lock().await;
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let started = Arc::new(Notify::new());
        let task_started = started.clone();
        let task_session = session.clone();
        let task = tokio::spawn(async move {
            task_started.notify_one();
            task_session
                .observe(
                    ObserveRequest {
                        operation_id: OperationId::new(),
                        include_accessibility: false,
                    },
                    task_cancel,
                )
                .await
        });
        started.notified().await;
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        cancel.cancel();
        let error = tokio::time::timeout(Duration::from_millis(100), task)
            .await
            .expect("queued cancellation must return without releasing the fence")
            .expect("queued observation task should join")
            .expect_err("queued cancelled observation should fail");
        drop(guard);
        assert_eq!(error.code, ComputerUseErrorCode::Cancelled);
        let marker = session
            .observe(
                ObserveRequest {
                    operation_id: OperationId::new(),
                    include_accessibility: false,
                },
                CancellationToken::new(),
            )
            .await
            .expect_err("cancelled queue entry must not consume the backend marker");
        assert_eq!(marker.code, ComputerUseErrorCode::CaptureInterrupted);
    }

    #[tokio::test]
    async fn queued_action_cancelled_before_fence_entry_has_no_receipt_or_native_effect() {
        let (session, backend) = local_fake_session().await;
        let observation = session
            .observe(
                ObserveRequest {
                    operation_id: OperationId::new(),
                    include_accessibility: false,
                },
                CancellationToken::new(),
            )
            .await
            .expect("basis observation should succeed");
        let guard = session.state.lock().await;
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let started = Arc::new(Notify::new());
        let task_started = started.clone();
        let task_session = session.clone();
        let task = tokio::spawn(async move {
            task_started.notify_one();
            task_session
                .act(
                    ComputerActionRequest {
                        operation_id: OperationId::new(),
                        observation: ObservationRef {
                            observation_id: observation.observation_id,
                        },
                        action: ComputerAction::Click(ClickAction {
                            point: ModelPoint { x: 10, y: 20 },
                            button: PointerButton::Left,
                            click_count: 1,
                            modifiers: Vec::new(),
                        }),
                    },
                    task_cancel,
                )
                .await
        });
        started.notified().await;
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        cancel.cancel();
        let failure = tokio::time::timeout(Duration::from_millis(100), task)
            .await
            .expect("queued cancellation must return without releasing the fence")
            .expect("queued action task should join")
            .expect_err("queued cancelled action should fail");
        drop(guard);
        assert_eq!(failure.error.code, ComputerUseErrorCode::Cancelled);
        assert_eq!(failure.effect_status, EffectStatus::NotExecuted);
        assert!(failure.receipt.is_none());
        assert!(backend.recorded_actions().await.is_empty());
    }

    #[tokio::test]
    async fn poison_is_published_before_cancellation_releases_backend_gate() {
        let backend = Arc::new(WedgeBackend::new());
        backend.wedge(CANCELLABLE_PROBE);
        let backend_dyn: crate::DynNativeDesktopBackend = backend.clone();
        let service = Arc::new(LocalComputerUseService::new(
            short_timeout_policy(),
            backend_dyn,
        ));

        let status_service = service.clone();
        let status_task =
            tokio::spawn(async move { status_service.status(CancellationToken::new()).await });
        while backend.probe_calls.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }

        let open_service = service.clone();
        let open_task = tokio::spawn(async move {
            open_service
                .open_current_desktop(CancellationToken::new())
                .await
        });
        tokio::task::yield_now().await;
        assert!(!open_task.is_finished());

        status_task.abort();
        assert!(
            status_task
                .await
                .expect_err("aborted probe should not complete")
                .is_cancelled()
        );
        let result = tokio::time::timeout(Duration::from_millis(100), open_task)
            .await
            .expect("queued open must observe poison promptly")
            .expect("queued open task should join");
        let Err(error) = result else {
            panic!("queued open must not enter the backend after poison");
        };
        assert_eq!(error.code, ComputerUseErrorCode::BackendUnavailable);
        assert_eq!(backend.probe_calls.load(Ordering::Acquire), 1);
        assert_eq!(backend.open_calls.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn service_status_uses_one_absolute_queue_deadline() {
        let backend = Arc::new(FakeNativeDesktopBackend::new(
            FakeComputerUseConfig::default(),
        ));
        let mut policy = short_timeout_policy();
        policy.queue_wait_timeout = Duration::from_millis(120);
        let backend_dyn: crate::DynNativeDesktopBackend = backend;
        let service = Arc::new(LocalComputerUseService::new(policy, backend_dyn));
        service
            .open_current_desktop(CancellationToken::new())
            .await
            .expect("fake desktop should open");
        let session = service
            .session_slot
            .lock()
            .await
            .as_ref()
            .cloned()
            .expect("service should retain the session");
        let state_guard = session.state.lock().await;
        let slot_guard = service.session_slot.lock().await;

        let started = Instant::now();
        let status_service = service.clone();
        let status_task =
            tokio::spawn(async move { status_service.status(CancellationToken::new()).await });
        tokio::time::sleep(Duration::from_millis(80)).await;
        drop(slot_guard);

        let error = tokio::time::timeout(Duration::from_millis(90), status_task)
            .await
            .expect("slot and state waits must share one queue deadline")
            .expect("status task should join")
            .expect_err("held session state should exhaust the queue deadline");
        drop(state_guard);
        assert_eq!(error.code, ComputerUseErrorCode::TimedOut);
        assert!(started.elapsed() < Duration::from_millis(170));
    }

    #[tokio::test]
    async fn volatile_capability_loss_invalidates_observations_and_session() {
        let (session, backend) = local_fake_session().await;
        let observation = session
            .observe(
                ObserveRequest {
                    operation_id: OperationId::new(),
                    include_accessibility: false,
                },
                CancellationToken::new(),
            )
            .await
            .expect("initial observation should succeed");
        assert!(
            session
                .state
                .lock()
                .await
                .observations
                .contains_key(&observation.observation_id)
        );

        backend
            .set_capabilities(crate::EffectiveComputerCapabilities::default())
            .await;
        let status = session
            .status(CancellationToken::new())
            .await
            .expect("status should report the recoverable unavailable state");
        assert_eq!(status.state, ComputerSessionState::SessionUnavailable);
        assert_eq!(
            status.effective_capabilities,
            crate::EffectiveComputerCapabilities::default()
        );
        let data = session.state.lock().await;
        assert!(data.observations.is_empty());
        assert!(data.current_layout_generation.is_none());
        drop(data);
        assert_eq!(
            session
                .observe(
                    ObserveRequest {
                        operation_id: OperationId::new(),
                        include_accessibility: false,
                    },
                    CancellationToken::new(),
                )
                .await
                .expect_err("authority recovery requires an explicit successful probe")
                .code,
            ComputerUseErrorCode::BackendUnavailable
        );

        backend
            .set_capabilities(FakeComputerUseConfig::default().capabilities)
            .await;
        let recovered = session
            .status(CancellationToken::new())
            .await
            .expect("explicit status should recover restored volatile authority");
        assert_eq!(recovered.state, ComputerSessionState::ReadyControl);
        assert!(session.state.lock().await.observations.is_empty());
        let fresh = session
            .observe(
                ObserveRequest {
                    operation_id: OperationId::new(),
                    include_accessibility: false,
                },
                CancellationToken::new(),
            )
            .await
            .expect("recovered authority should require and accept a fresh observation");
        assert_ne!(fresh.observation_id, observation.observation_id);
    }

    #[tokio::test]
    async fn volatile_observe_error_invalidates_existing_observations() {
        let (session, backend) = local_fake_session().await;
        session
            .observe(
                ObserveRequest {
                    operation_id: OperationId::new(),
                    include_accessibility: false,
                },
                CancellationToken::new(),
            )
            .await
            .expect("initial observation should succeed");
        backend
            .fail_next_observe(ComputerUseError::new(
                ComputerUseErrorCode::SessionLocked,
                "session locked",
                RetryClassification::AfterExplicitResume,
            ))
            .await;

        let error = session
            .observe(
                ObserveRequest {
                    operation_id: OperationId::new(),
                    include_accessibility: false,
                },
                CancellationToken::new(),
            )
            .await
            .expect_err("session lock should reject capture");
        assert_eq!(error.code, ComputerUseErrorCode::SessionLocked);
        let data = session.state.lock().await;
        assert_eq!(data.state, ComputerSessionState::SessionUnavailable);
        assert!(data.observations.is_empty());
        assert!(data.current_layout_generation.is_none());
        drop(data);
    }

    #[tokio::test]
    async fn volatile_action_errors_invalidate_authority_until_status_probe_recovers() {
        let (session, backend) = local_fake_session().await;
        let invalidating_errors = [
            ComputerUseErrorCode::StaleTarget,
            ComputerUseErrorCode::DisplayTopologyChanged,
            ComputerUseErrorCode::PermissionDenied,
            ComputerUseErrorCode::SessionLocked,
        ];

        for code in invalidating_errors {
            let observation = session
                .observe(
                    ObserveRequest {
                        operation_id: OperationId::new(),
                        include_accessibility: false,
                    },
                    CancellationToken::new(),
                )
                .await
                .expect("fresh basis observation should succeed");
            backend
                .fail_next_action(crate::NativeActionFailure {
                    error: ComputerUseError::new(
                        code,
                        "volatile authority changed during native input",
                        RetryClassification::AfterFreshObservation,
                    ),
                    effect_status: EffectStatus::DeliveryUncertain,
                    receipt: None,
                    cleanup: InputCleanupStatus::Complete,
                })
                .await;

            let failure = session
                .act(
                    ComputerActionRequest {
                        operation_id: OperationId::new(),
                        observation: ObservationRef {
                            observation_id: observation.observation_id,
                        },
                        action: ComputerAction::Click(ClickAction {
                            point: ModelPoint { x: 10, y: 20 },
                            button: PointerButton::Left,
                            click_count: 1,
                            modifiers: Vec::new(),
                        }),
                    },
                    CancellationToken::new(),
                )
                .await
                .expect_err("volatile native action error should be preserved");
            assert_eq!(failure.error.code, code);
            assert_eq!(failure.effect_status, EffectStatus::DeliveryUncertain);
            let receipt = failure
                .receipt
                .expect("volatile native action failure should retain its receipt");
            assert_eq!(receipt.effect_status, EffectStatus::DeliveryUncertain);
            assert_eq!(receipt.cleanup, InputCleanupStatus::Complete);

            let data = session.state.lock().await;
            assert_eq!(data.state, ComputerSessionState::SessionUnavailable);
            assert!(data.observations.is_empty());
            assert!(data.current_layout_generation.is_none());
            drop(data);
            assert_eq!(
                session.capabilities(),
                crate::EffectiveComputerCapabilities::default()
            );
            assert_eq!(
                session
                    .observe(
                        ObserveRequest {
                            operation_id: OperationId::new(),
                            include_accessibility: false,
                        },
                        CancellationToken::new(),
                    )
                    .await
                    .expect_err("an explicit status probe is required before recovery")
                    .code,
                ComputerUseErrorCode::BackendUnavailable
            );

            let recovered = session
                .status(CancellationToken::new())
                .await
                .expect("an explicit successful status probe should recover authority");
            assert_eq!(recovered.state, ComputerSessionState::ReadyControl);
            assert!(recovered.effective_capabilities.pointer);
            session
                .observe(
                    ObserveRequest {
                        operation_id: OperationId::new(),
                        include_accessibility: false,
                    },
                    CancellationToken::new(),
                )
                .await
                .expect("recovered authority should accept a fresh observation");
        }
    }

    #[tokio::test]
    async fn invalidating_action_with_unconfirmed_cleanup_cannot_rearm_control() {
        let (session, backend) = local_fake_session().await;
        let observation = session
            .observe(
                ObserveRequest {
                    operation_id: OperationId::new(),
                    include_accessibility: false,
                },
                CancellationToken::new(),
            )
            .await
            .expect("basis observation should succeed");
        backend
            .fail_next_action(crate::NativeActionFailure {
                error: ComputerUseError::new(
                    ComputerUseErrorCode::SessionLocked,
                    "session locked during native input",
                    RetryClassification::AfterExplicitResume,
                ),
                effect_status: EffectStatus::DeliveryUncertain,
                receipt: None,
                cleanup: InputCleanupStatus::Failed,
            })
            .await;

        let failure = session
            .act(
                ComputerActionRequest {
                    operation_id: OperationId::new(),
                    observation: ObservationRef {
                        observation_id: observation.observation_id,
                    },
                    action: ComputerAction::Click(ClickAction {
                        point: ModelPoint { x: 10, y: 20 },
                        button: PointerButton::Left,
                        click_count: 1,
                        modifiers: Vec::new(),
                    }),
                },
                CancellationToken::new(),
            )
            .await
            .expect_err("unconfirmed cleanup must fail and block control");
        assert_eq!(failure.error.code, ComputerUseErrorCode::InputCleanupFailed);
        assert_eq!(failure.effect_status, EffectStatus::DeliveryUncertain);
        let receipt = failure
            .receipt
            .expect("cleanup failure must preserve action evidence");
        assert_eq!(receipt.effect_status, EffectStatus::DeliveryUncertain);
        assert_eq!(receipt.cleanup, InputCleanupStatus::Failed);

        let data = session.state.lock().await;
        assert_eq!(data.state, ComputerSessionState::SessionUnavailable);
        assert!(data.cleanup_blocked);
        assert_eq!(data.takeover_epoch, crate::TakeoverEpoch(2));
        assert!(data.observations.is_empty());
        assert!(data.current_layout_generation.is_none());
        drop(data);

        let recovered = session
            .status(CancellationToken::new())
            .await
            .expect("status probe may recover volatile authority but not cleanup safety");
        assert_eq!(recovered.state, ComputerSessionState::Paused);
        assert!(recovered.effective_capabilities.pointer);
        assert_eq!(
            session
                .observe(
                    ObserveRequest {
                        operation_id: OperationId::new(),
                        include_accessibility: false,
                    },
                    CancellationToken::new(),
                )
                .await
                .expect_err("cleanup-blocked session must not resume through status")
                .code,
            ComputerUseErrorCode::UserPresenceRevoked
        );
    }

    #[tokio::test]
    async fn successful_service_shutdown_is_idempotent() {
        let backend = Arc::new(WedgeBackend::new());
        let backend_dyn: crate::DynNativeDesktopBackend = backend.clone();
        let service = LocalComputerUseService::new(short_timeout_policy(), backend_dyn);
        service
            .open_current_desktop(CancellationToken::new())
            .await
            .expect("fake desktop should open");

        let first = service
            .shutdown(CloseReason::HostShutdown)
            .await
            .expect("first shutdown should close the backend");
        let second = service
            .shutdown(CloseReason::HostShutdown)
            .await
            .expect("confirmed shutdown should be idempotent");
        assert_eq!(first.state, ComputerSessionState::Closed);
        assert_eq!(second.state, ComputerSessionState::Closed);
        assert_eq!(second.cleanup, InputCleanupStatus::NotRequired);
        assert_eq!(backend.close_calls.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn shutdown_serializes_with_pre_session_probe_and_observes_poison() {
        let backend = Arc::new(WedgeBackend::new());
        backend.wedge(WEDGE_PROBE);
        let backend_dyn: crate::DynNativeDesktopBackend = backend.clone();
        let service = Arc::new(LocalComputerUseService::new(
            short_timeout_policy(),
            backend_dyn,
        ));
        let status_service = service.clone();
        let status_task =
            tokio::spawn(async move { status_service.status(CancellationToken::new()).await });
        while backend.probe_calls.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
        let shutdown_service = service.clone();
        let shutdown_task =
            tokio::spawn(async move { shutdown_service.shutdown(CloseReason::HostShutdown).await });
        tokio::task::yield_now().await;
        assert!(!shutdown_task.is_finished());

        assert_eq!(
            status_task
                .await
                .expect("status task should join")
                .expect_err("wedged probe must time out")
                .code,
            ComputerUseErrorCode::TimedOut
        );
        assert_eq!(
            shutdown_task
                .await
                .expect("shutdown task should join")
                .expect_err("shutdown must report probe poison")
                .code,
            ComputerUseErrorCode::BackendUnavailable
        );
        assert_eq!(backend.probe_calls.load(Ordering::Acquire), 1);
        assert_eq!(backend.close_calls.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn dropped_observe_future_poison_is_immediate_and_forbids_reentry() {
        let backend = Arc::new(WedgeBackend::new());
        let backend_dyn: crate::DynNativeDesktopBackend = backend.clone();
        let service = LocalComputerUseService::new(short_timeout_policy(), backend_dyn);
        let session = service
            .open_current_desktop(CancellationToken::new())
            .await
            .expect("fake desktop should open");
        backend.wedge(WEDGE_OBSERVE);
        let task_session = session.clone();
        let task = tokio::spawn(async move {
            task_session
                .observe(
                    ObserveRequest {
                        operation_id: OperationId::new(),
                        include_accessibility: false,
                    },
                    CancellationToken::new(),
                )
                .await
        });
        while backend.observe_calls.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
        task.abort();
        assert!(
            task.await
                .expect_err("aborted observation task must not complete")
                .is_cancelled()
        );
        assert_eq!(
            session.capabilities(),
            crate::EffectiveComputerCapabilities::default()
        );
        assert_eq!(
            session
                .status(CancellationToken::new())
                .await
                .expect_err("abandoned observation must poison the session")
                .code,
            ComputerUseErrorCode::BackendUnavailable
        );
        assert_eq!(backend.observe_calls.load(Ordering::Acquire), 1);
        assert_eq!(
            service
                .shutdown(CloseReason::HostShutdown)
                .await
                .expect_err("poisoned shutdown must skip close")
                .code,
            ComputerUseErrorCode::BackendUnavailable
        );
        assert_eq!(backend.close_calls.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn dropped_action_future_retains_uncertain_idempotency_evidence() {
        let backend = Arc::new(WedgeBackend::new());
        let backend_dyn: crate::DynNativeDesktopBackend = backend.clone();
        let service = LocalComputerUseService::new(short_timeout_policy(), backend_dyn);
        let session = service
            .open_current_desktop(CancellationToken::new())
            .await
            .expect("fake desktop should open");
        let observation = session
            .observe(
                ObserveRequest {
                    operation_id: OperationId::new(),
                    include_accessibility: false,
                },
                CancellationToken::new(),
            )
            .await
            .expect("basis observation should succeed");
        let request = ComputerActionRequest {
            operation_id: OperationId::new(),
            observation: ObservationRef {
                observation_id: observation.observation_id,
            },
            action: ComputerAction::Click(ClickAction {
                point: ModelPoint { x: 10, y: 20 },
                button: PointerButton::Left,
                click_count: 1,
                modifiers: Vec::new(),
            }),
        };
        backend.wedge(WEDGE_ACTION);
        let task_session = session.clone();
        let task_request = request.clone();
        let task = tokio::spawn(async move {
            task_session
                .act(task_request, CancellationToken::new())
                .await
        });
        while backend.action_calls.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
        task.abort();
        assert!(
            task.await
                .expect_err("aborted action task must not complete")
                .is_cancelled()
        );

        let duplicate = session
            .act(request, CancellationToken::new())
            .await
            .expect_err("abandoned action identity must remain reserved");
        assert_eq!(
            duplicate.error.code,
            ComputerUseErrorCode::DuplicateResultEvicted
        );
        assert_eq!(duplicate.effect_status, EffectStatus::DeliveryUncertain);
        let receipt = duplicate
            .receipt
            .expect("abandoned action reservation must retain a receipt");
        assert_eq!(receipt.effect_status, EffectStatus::DeliveryUncertain);
        assert_eq!(receipt.cleanup, InputCleanupStatus::Failed);
        assert_eq!(backend.action_calls.load(Ordering::Acquire), 1);
        assert_eq!(backend.close_calls.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn hanging_observe_poison_is_bounded_and_forbids_reentry_or_close() {
        let backend = Arc::new(WedgeBackend::new());
        let backend_dyn: crate::DynNativeDesktopBackend = backend.clone();
        let service = LocalComputerUseService::new(short_timeout_policy(), backend_dyn);
        let session = service
            .open_current_desktop(CancellationToken::new())
            .await
            .expect("fake desktop should open");
        backend.wedge(WEDGE_OBSERVE);

        let error = tokio::time::timeout(
            Duration::from_millis(250),
            session.observe(
                ObserveRequest {
                    operation_id: OperationId::new(),
                    include_accessibility: false,
                },
                CancellationToken::new(),
            ),
        )
        .await
        .expect("wedged observation must return within the operation and cleanup budgets")
        .expect_err("wedged observation must fail");
        assert_eq!(error.code, ComputerUseErrorCode::TimedOut);
        assert_eq!(
            session.capabilities(),
            crate::EffectiveComputerCapabilities::default()
        );
        assert_eq!(
            service
                .session_slot
                .lock()
                .await
                .as_ref()
                .expect("session should remain recorded")
                .state
                .lock()
                .await
                .state,
            ComputerSessionState::SessionUnavailable
        );

        let second = session
            .observe(
                ObserveRequest {
                    operation_id: OperationId::new(),
                    include_accessibility: false,
                },
                CancellationToken::new(),
            )
            .await
            .expect_err("poisoned session must reject later observations");
        assert_eq!(second.code, ComputerUseErrorCode::BackendUnavailable);
        assert_eq!(backend.observe_calls.load(Ordering::Acquire), 1);
        assert_eq!(
            service
                .shutdown(CloseReason::HostShutdown)
                .await
                .expect_err("poisoned shutdown must report unconfirmed cleanup")
                .code,
            ComputerUseErrorCode::BackendUnavailable
        );
        assert_eq!(backend.close_calls.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn hanging_action_returns_uncertain_receipt_and_remembers_idempotency() {
        let backend = Arc::new(WedgeBackend::new());
        let backend_dyn: crate::DynNativeDesktopBackend = backend.clone();
        let service = LocalComputerUseService::new(short_timeout_policy(), backend_dyn);
        let session = service
            .open_current_desktop(CancellationToken::new())
            .await
            .expect("fake desktop should open");
        let observation = session
            .observe(
                ObserveRequest {
                    operation_id: OperationId::new(),
                    include_accessibility: false,
                },
                CancellationToken::new(),
            )
            .await
            .expect("basis observation should succeed");
        let request = ComputerActionRequest {
            operation_id: OperationId::new(),
            observation: ObservationRef {
                observation_id: observation.observation_id,
            },
            action: ComputerAction::Click(ClickAction {
                point: ModelPoint { x: 10, y: 20 },
                button: PointerButton::Left,
                click_count: 1,
                modifiers: Vec::new(),
            }),
        };
        backend.wedge(WEDGE_ACTION);

        let failure = tokio::time::timeout(
            Duration::from_millis(250),
            session.act(request.clone(), CancellationToken::new()),
        )
        .await
        .expect("wedged action must return within the operation and cleanup budgets")
        .expect_err("wedged action must fail");
        assert_eq!(
            failure.error.code,
            ComputerUseErrorCode::InputDeliveryUncertain
        );
        assert_eq!(failure.effect_status, EffectStatus::DeliveryUncertain);
        let receipt = failure
            .receipt
            .expect("uncertain action must include a receipt");
        assert_eq!(receipt.effect_status, EffectStatus::DeliveryUncertain);
        assert_eq!(receipt.cleanup, InputCleanupStatus::Failed);
        assert_eq!(
            session.capabilities(),
            crate::EffectiveComputerCapabilities::default()
        );

        let duplicate = session
            .act(request, CancellationToken::new())
            .await
            .expect_err("uncertain operation identity must remain recorded");
        assert_eq!(
            duplicate.error.code,
            ComputerUseErrorCode::DuplicateResultEvicted
        );
        assert_eq!(duplicate.effect_status, EffectStatus::DeliveryUncertain);
        assert_eq!(backend.action_calls.load(Ordering::Acquire), 1);
        assert_eq!(
            service
                .shutdown(CloseReason::HostShutdown)
                .await
                .expect_err("poisoned shutdown must report unconfirmed cleanup")
                .code,
            ComputerUseErrorCode::BackendUnavailable
        );
        assert_eq!(backend.close_calls.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn dropped_pre_session_probe_poison_makes_shutdown_bounded() {
        let backend = Arc::new(WedgeBackend::new());
        backend.wedge(WEDGE_PROBE);
        let backend_dyn: crate::DynNativeDesktopBackend = backend.clone();
        let service = Arc::new(LocalComputerUseService::new(
            short_timeout_policy(),
            backend_dyn,
        ));
        let task_service = service.clone();
        let task = tokio::spawn(async move { task_service.status(CancellationToken::new()).await });
        while backend.probe_calls.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
        task.abort();
        assert!(
            task.await
                .expect_err("aborted probe task must not complete")
                .is_cancelled()
        );
        assert_eq!(
            tokio::time::timeout(
                Duration::from_millis(100),
                service.shutdown(CloseReason::HostShutdown),
            )
            .await
            .expect("poisoned shutdown must not wait on the quarantined gate")
            .expect_err("abandoned probe must make cleanup unconfirmed")
            .code,
            ComputerUseErrorCode::BackendUnavailable
        );
        assert_eq!(backend.probe_calls.load(Ordering::Acquire), 1);
        assert_eq!(backend.close_calls.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn dropped_close_future_poison_forbids_backend_retry() {
        let backend = Arc::new(WedgeBackend::new());
        let backend_dyn: crate::DynNativeDesktopBackend = backend.clone();
        let service = LocalComputerUseService::new(short_timeout_policy(), backend_dyn);
        let session = service
            .open_current_desktop(CancellationToken::new())
            .await
            .expect("fake desktop should open");
        backend.wedge(WEDGE_CLOSE);
        let task_session = session.clone();
        let task = tokio::spawn(async move { task_session.close(CloseReason::HostShutdown).await });
        while backend.close_calls.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
        task.abort();
        assert!(
            task.await
                .expect_err("aborted close task must not complete")
                .is_cancelled()
        );
        assert_eq!(
            session
                .close(CloseReason::HostShutdown)
                .await
                .expect_err("abandoned close must poison the lifecycle")
                .code,
            ComputerUseErrorCode::BackendUnavailable
        );
        assert_eq!(backend.close_calls.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn action_success_with_failed_cleanup_pauses_and_returns_evidence() {
        let backend = Arc::new(WedgeBackend::new());
        let backend_dyn: crate::DynNativeDesktopBackend = backend.clone();
        let service = LocalComputerUseService::new(short_timeout_policy(), backend_dyn);
        let session = service
            .open_current_desktop(CancellationToken::new())
            .await
            .expect("fake desktop should open");
        let observation = session
            .observe(
                ObserveRequest {
                    operation_id: OperationId::new(),
                    include_accessibility: false,
                },
                CancellationToken::new(),
            )
            .await
            .expect("basis observation should succeed");
        backend.fail_action_cleanup();
        let failure = session
            .act(
                ComputerActionRequest {
                    operation_id: OperationId::new(),
                    observation: ObservationRef {
                        observation_id: observation.observation_id,
                    },
                    action: ComputerAction::Click(ClickAction {
                        point: ModelPoint { x: 10, y: 20 },
                        button: PointerButton::Left,
                        click_count: 1,
                        modifiers: Vec::new(),
                    }),
                },
                CancellationToken::new(),
            )
            .await
            .expect_err("failed cleanup must not be projected as action success");
        assert_eq!(failure.error.code, ComputerUseErrorCode::InputCleanupFailed);
        assert_eq!(failure.effect_status, EffectStatus::Executed);
        let receipt = failure
            .receipt
            .expect("cleanup failure must retain receipt");
        assert_eq!(receipt.cleanup, InputCleanupStatus::Failed);
        assert_eq!(
            service
                .session_slot
                .lock()
                .await
                .as_ref()
                .expect("session should remain recorded")
                .state
                .lock()
                .await
                .state,
            ComputerSessionState::Paused
        );
    }

    #[tokio::test]
    async fn close_success_with_failed_cleanup_poison_is_reported() {
        let backend = Arc::new(WedgeBackend::new());
        let backend_dyn: crate::DynNativeDesktopBackend = backend.clone();
        let service = LocalComputerUseService::new(short_timeout_policy(), backend_dyn);
        let session = service
            .open_current_desktop(CancellationToken::new())
            .await
            .expect("fake desktop should open");
        backend.fail_close_cleanup();
        let error = service
            .shutdown(CloseReason::HostShutdown)
            .await
            .expect_err("unconfirmed close cleanup must fail shutdown");
        assert_eq!(error.code, ComputerUseErrorCode::InputCleanupFailed);
        assert_eq!(backend.close_calls.load(Ordering::Acquire), 1);
        assert_eq!(
            session.capabilities(),
            crate::EffectiveComputerCapabilities::default()
        );
        assert_eq!(
            session
                .close(CloseReason::HostShutdown)
                .await
                .expect_err("poisoned close must not call backend again")
                .code,
            ComputerUseErrorCode::BackendUnavailable
        );
        assert_eq!(backend.close_calls.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn zero_idempotency_capacity_is_normalized_to_one() {
        let backend = Arc::new(WedgeBackend::new());
        let backend_dyn: crate::DynNativeDesktopBackend = backend;
        let mut policy = short_timeout_policy();
        policy.idempotency.max_entries = 0;
        let service = LocalComputerUseService::new(policy, backend_dyn);
        assert_eq!(service.policy().idempotency.max_entries, 1);
    }

    #[tokio::test]
    async fn hanging_close_is_bounded_and_never_retried_after_poison() {
        let backend = Arc::new(WedgeBackend::new());
        let backend_dyn: crate::DynNativeDesktopBackend = backend.clone();
        let service = LocalComputerUseService::new(short_timeout_policy(), backend_dyn);
        let session = service
            .open_current_desktop(CancellationToken::new())
            .await
            .expect("fake desktop should open");
        backend.wedge(WEDGE_CLOSE);

        let service = Arc::new(service);
        let shutdown_service = service.clone();
        let shutdown_task =
            tokio::spawn(async move { shutdown_service.shutdown(CloseReason::HostShutdown).await });
        while backend.close_calls.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
        let open_service = service.clone();
        let open_task = tokio::spawn(async move {
            open_service
                .open_current_desktop(CancellationToken::new())
                .await
        });
        tokio::task::yield_now().await;
        assert!(!open_task.is_finished());

        let error = tokio::time::timeout(Duration::from_millis(250), shutdown_task)
            .await
            .expect("wedged close must return within its operation budget")
            .expect("shutdown task should join")
            .expect_err("wedged close must fail");
        assert_eq!(error.code, ComputerUseErrorCode::TimedOut);
        assert_eq!(backend.close_calls.load(Ordering::Acquire), 1);
        let Err(open_error) = open_task.await.expect("concurrent open task should join") else {
            panic!("concurrent open must observe the poisoned lifecycle");
        };
        assert_eq!(open_error.code, ComputerUseErrorCode::BackendUnavailable);
        assert_eq!(backend.open_calls.load(Ordering::Acquire), 1);
        assert_eq!(
            session.capabilities(),
            crate::EffectiveComputerCapabilities::default()
        );

        assert_eq!(
            session
                .close(CloseReason::HostShutdown)
                .await
                .expect_err("poisoned close must not call the backend again")
                .code,
            ComputerUseErrorCode::BackendUnavailable
        );
        assert_eq!(backend.close_calls.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn cancellation_cleanup_budget_exhaustion_poison_is_bounded() {
        let cancel = CancellationToken::new();
        let operation_cancel = CancellationToken::new();
        let lifecycle = Arc::new(BackendLifecycle::default());
        let backend_task = tokio::spawn(async move {
            std::future::pending::<Result<(), crate::ComputerUseError>>().await
        });
        cancel.cancel();
        let started = Instant::now();
        let operation_guard =
            BackendOperationGuard::new(lifecycle.clone(), operation_cancel.clone(), None);
        let error = cancellable(
            Duration::from_secs(1),
            Duration::from_millis(20),
            cancel,
            operation_cancel,
            operation_guard,
            backend_task,
        )
        .await
        .expect_err("cancelled wedged task must fail");
        assert_eq!(error.code, ComputerUseErrorCode::Cancelled);
        assert!(started.elapsed() < Duration::from_millis(250));
        assert!(lifecycle.is_poisoned());
    }

    #[tokio::test]
    async fn cancellation_waits_for_backend_terminal_state_within_cleanup_budget() {
        let cancel = CancellationToken::new();
        let operation_cancel = CancellationToken::new();
        let completed = Arc::new(AtomicBool::new(false));
        let completed_in_future = completed.clone();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            let lifecycle = Arc::new(BackendLifecycle::default());
            let backend_task = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                completed_in_future.store(true, Ordering::Release);
                Ok::<_, crate::ComputerUseError>(())
            });
            let operation_guard =
                BackendOperationGuard::new(lifecycle, operation_cancel.clone(), None);
            cancellable(
                Duration::from_secs(1),
                Duration::from_secs(1),
                task_cancel,
                operation_cancel,
                operation_guard,
                backend_task,
            )
            .await
        });

        tokio::time::sleep(Duration::from_millis(5)).await;
        cancel.cancel();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!task.is_finished());
        let error = task
            .await
            .expect("cancellable task should join")
            .expect_err("cancelled operation should fail");
        assert_eq!(error.code, crate::ComputerUseErrorCode::Cancelled);
        assert!(completed.load(Ordering::Acquire));
    }
}
