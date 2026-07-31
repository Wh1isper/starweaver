use std::{
    io::Cursor,
    os::unix::fs::MetadataExt as _,
    process::Command,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use async_trait::async_trait;
use image::{DynamicImage, ImageFormat, RgbaImage, imageops::FilterType};
use objc2_core_graphics::{
    CGPreflightPostEventAccess, CGPreflightScreenCaptureAccess, CGRequestPostEventAccess,
    CGRequestScreenCaptureAccess,
};
use sha2::{Digest, Sha256};
use starweaver_core::CancellationToken;
use xcap::Monitor;

use crate::{
    AccessibilityGeneration, AccessibilityPolicy, ActiveSessionStatus, AffineTransform2D,
    BackendProbe, CanonicalKey, CloseReason, ComputerAction, ComputerUseError,
    ComputerUseErrorCode, ComputerUsePolicy, DesktopImageMime, DesktopSurfaceScope,
    DisplayGeometry, EffectStatus, EffectiveComputerCapabilities, FrameRedactionStatus,
    GeometrySnapshot, InputCleanupStatus, KeyMode, LayoutGeneration, ModelPoint, ModelRect,
    NativeActionFailure, NativeActionReceipt, NativeBackendKind, NativeDesktopBackend,
    NativeDesktopPlatform, NativeObservation, NativePoint, NativeRect, PermissionCapabilityStatus,
    PermissionPromptPolicy, PermissionReport, PermissionRequest, PixelSize, RetryClassification,
    StabilityCheckStatus, TargetGeneration, UserPresenceStatus,
};

use super::{macos_accessibility, macos_input, macos_session};

const MAX_NATIVE_PATH_POINTS: usize = 4_096;
const MAX_NATIVE_POINTER_SAMPLES: u32 = 4_096;
const MAX_NATIVE_DURATION_MS: u32 = 10_000;
const MAX_NATIVE_CLICK_COUNT: u8 = 3;
const MAX_NATIVE_TEXT_BYTES: usize = 16_384;
const MAX_NATIVE_TEXT_SCALARS: usize = 8_192;
const MAX_NATIVE_KEYS: usize = 32;
const MAX_NATIVE_MODIFIERS: usize = 4;
const MAX_NATIVE_SCROLL_ABS: u32 = 100_000;
const INPUT_FENCE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
const MAX_TEXT_INPUT_PART_DELAY_MS: u64 = 20;
const MAX_TEXT_INPUT_PACING_BUDGET_MS: u64 = 5_000;

struct BackendState {
    session_marker: Option<[u8; 32]>,
    session_transition_epoch: Option<u64>,
    active_session: Option<ActiveSessionStatus>,
    target_generation: TargetGeneration,
    topology_digest: Option<[u8; 32]>,
    layout_generation: LayoutGeneration,
    accessibility_generation: AccessibilityGeneration,
    active_input: bool,
    pending_input: Option<macos_input::NativeInput>,
    closed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CaptureFence {
    active_session: ActiveSessionStatus,
    capture_granted: bool,
    target_generation: TargetGeneration,
    topology_digest: Option<[u8; 32]>,
    layout_generation: LayoutGeneration,
}

pub struct MacosDesktopBackend {
    scope: DesktopSurfaceScope,
    accessibility_policy: AccessibilityPolicy,
    permission_prompts: PermissionPromptPolicy,
    capture_prompt_attempted: AtomicBool,
    accessibility_prompt_attempted: AtomicBool,
    session_transitions: macos_session::SessionTransitionMonitor,
    started_at: Instant,
    state: Mutex<BackendState>,
}

struct InputExecutionGuard<'a> {
    input: Option<macos_input::NativeInput>,
    state: &'a Mutex<BackendState>,
}

impl<'a> InputExecutionGuard<'a> {
    fn new(
        state: &'a Mutex<BackendState>,
        input: macos_input::NativeInput,
    ) -> Result<Self, ComputerUseError> {
        let mut backend_state = state
            .lock()
            .map_err(|_| backend_error("macOS backend state is unavailable"))?;
        reserve_input(&mut backend_state)?;
        drop(backend_state);
        Ok(Self {
            input: Some(input),
            state,
        })
    }

    fn input_mut(&mut self) -> Result<&mut macos_input::NativeInput, ComputerUseError> {
        self.input.as_mut().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::Internal,
                "macOS input execution guard is not armed",
                RetryClassification::Never,
            )
        })
    }

    fn finish(&mut self) -> (macos_input::CleanupResult, u32, Option<ComputerUseError>) {
        let Some(input) = self.input.as_mut() else {
            return (
                macos_input::CleanupResult::default(),
                0,
                Some(ComputerUseError::new(
                    ComputerUseErrorCode::Internal,
                    "macOS input execution guard is not armed",
                    RetryClassification::Never,
                )),
            );
        };
        let cleanup = input.cleanup();
        let event_count = input.event_count();
        let retention_error = if cleanup.complete {
            let result = self.release_active();
            self.input = None;
            result.err()
        } else {
            self.retain_pending().err()
        };
        (cleanup, event_count, retention_error)
    }

    fn retain_pending(&mut self) -> Result<(), ComputerUseError> {
        if self.input.is_none() {
            return Ok(());
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| backend_error("macOS backend state is unavailable"))?;
        if state.pending_input.is_some() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InputCleanupFailed,
                "macOS input cleanup state collided with an existing pending action",
                RetryClassification::Never,
            ));
        }
        let input = self.input.take().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::Internal,
                "macOS input execution guard lost its pending input",
                RetryClassification::Never,
            )
        })?;
        state.active_input = false;
        state.pending_input = Some(input);
        drop(state);
        Ok(())
    }

    fn release_active(&self) -> Result<(), ComputerUseError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| backend_error("macOS backend state is unavailable"))?;
        state.active_input = false;
        drop(state);
        Ok(())
    }
}

impl Drop for InputExecutionGuard<'_> {
    fn drop(&mut self) {
        let Some(input) = self.input.as_mut() else {
            return;
        };
        let cleanup = input.cleanup();
        if cleanup.complete {
            let _ = self.release_active();
            self.input = None;
        } else {
            let _ = self.retain_pending();
        }
    }
}

impl MacosDesktopBackend {
    #[must_use]
    pub fn new(policy: &ComputerUsePolicy) -> Self {
        Self {
            scope: policy.desktop_scope,
            accessibility_policy: policy.accessibility.clone(),
            permission_prompts: policy.permission_prompts,
            capture_prompt_attempted: AtomicBool::new(false),
            accessibility_prompt_attempted: AtomicBool::new(false),
            session_transitions: macos_session::SessionTransitionMonitor::new(),
            started_at: Instant::now(),
            state: Mutex::new(BackendState {
                session_marker: None,
                session_transition_epoch: None,
                active_session: None,
                target_generation: TargetGeneration(1),
                topology_digest: None,
                layout_generation: LayoutGeneration(1),
                accessibility_generation: AccessibilityGeneration(0),
                active_input: false,
                pending_input: None,
                closed: false,
            }),
        }
    }

    fn next_accessibility_generation(&self) -> Result<AccessibilityGeneration, ComputerUseError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| backend_error("macOS backend state is unavailable"))?;
        ensure_backend_open(&state)?;
        state.accessibility_generation.0 = state.accessibility_generation.0.saturating_add(1);
        Ok(state.accessibility_generation)
    }

    fn monotonic_ms(&self) -> u64 {
        u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    #[allow(clippy::too_many_lines)]
    async fn inspect(
        &self,
    ) -> Result<(BackendProbe, Vec<Monitor>, CaptureFence), ComputerUseError> {
        let session_transition_epoch = self
            .session_transitions
            .poll_epoch()
            .map_err(|_| backend_error("macOS session transition monitor is unavailable"))?;
        let console = tokio::task::spawn_blocking(console_session)
            .await
            .map_err(|_| backend_error("macOS session probe task failed"))??;
        let active_session = if !console.owned_by_process_user {
            ActiveSessionStatus::Inactive
        } else if console.locked {
            ActiveSessionStatus::Locked
        } else {
            ActiveSessionStatus::Active
        };
        let capture_granted = CGPreflightScreenCaptureAccess();
        let input_granted = CGPreflightPostEventAccess();
        let accessibility_granted = macos_accessibility::is_trusted();
        let (monitors, current_topology_digest) = if active_session == ActiveSessionStatus::Active {
            let monitors = tokio::task::spawn_blocking(Monitor::all)
                .await
                .map_err(|_| backend_error("macOS display probe task failed"))?
                .map_err(|_| backend_error("macOS display topology is unavailable"))?;
            if monitors.is_empty() {
                return Err(backend_error("macOS reported no active display"));
            }
            let topology_digest = topology_digest(&monitors)?;
            (monitors, Some(topology_digest))
        } else {
            (Vec::new(), None)
        };
        let (target_generation, layout_generation) = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| backend_error("macOS backend state is unavailable"))?;
            ensure_backend_open(&state)?;
            update_backend_generations(
                &mut state,
                console.session_marker,
                session_transition_epoch,
                active_session,
                current_topology_digest,
            )
        };
        let capture = if capture_granted {
            PermissionCapabilityStatus::Granted
        } else {
            PermissionCapabilityStatus::Required
        };
        let active = active_session == ActiveSessionStatus::Active;
        let capabilities = EffectiveComputerCapabilities {
            observe: capture_granted && active,
            pointer: input_granted && active,
            keyboard: input_granted && active,
            accessibility_snapshot: accessibility_granted && active,
        };
        let diagnostics_code = if !console.owned_by_process_user {
            "macos_console_user_mismatch"
        } else if console.locked {
            "macos_session_locked"
        } else if !capture_granted {
            "macos_screen_recording_permission_required"
        } else if !input_granted {
            "macos_post_event_permission_required"
        } else if !accessibility_granted {
            "macos_input_ready_accessibility_permission_required"
        } else {
            "macos_observe_input_accessibility_ready"
        };
        let permissions = PermissionReport {
            platform: NativeDesktopPlatform::Macos,
            backend: NativeBackendKind::MacosCoreGraphics,
            active_session,
            capture,
            pointer_input: if input_granted {
                PermissionCapabilityStatus::Granted
            } else {
                PermissionCapabilityStatus::Required
            },
            keyboard_input: if input_granted {
                PermissionCapabilityStatus::Granted
            } else {
                PermissionCapabilityStatus::Required
            },
            accessibility: if accessibility_granted {
                PermissionCapabilityStatus::Granted
            } else {
                PermissionCapabilityStatus::Required
            },
            user_presence: PermissionCapabilityStatus::Unavailable,
            restart_required: false,
            remediation: permission_remediation(
                &console,
                capture_granted,
                input_granted,
                accessibility_granted,
            ),
            diagnostics_code: diagnostics_code.into(),
        };
        Ok((
            BackendProbe {
                platform: NativeDesktopPlatform::Macos,
                backend: NativeBackendKind::MacosCoreGraphics,
                permissions,
                capabilities,
                target_generation,
                user_presence: UserPresenceStatus::Unavailable,
                diagnostics_code: diagnostics_code.into(),
            },
            monitors,
            CaptureFence {
                active_session,
                capture_granted,
                target_generation,
                topology_digest: current_topology_digest,
                layout_generation,
            },
        ))
    }
}

#[allow(clippy::significant_drop_tightening)]
#[async_trait]
impl NativeDesktopBackend for MacosDesktopBackend {
    fn platform(&self) -> NativeDesktopPlatform {
        NativeDesktopPlatform::Macos
    }

    fn kind(&self) -> NativeBackendKind {
        NativeBackendKind::MacosCoreGraphics
    }

    async fn probe(&self, cancel: CancellationToken) -> Result<BackendProbe, ComputerUseError> {
        if cancel.is_cancelled() {
            return Err(ComputerUseError::cancelled());
        }
        let (probe, _, _) = self.inspect().await?;
        Ok(probe)
    }

    async fn request_permissions(
        &self,
        request: PermissionRequest,
        cancel: CancellationToken,
    ) -> Result<BackendProbe, ComputerUseError> {
        if cancel.is_cancelled() {
            return Err(ComputerUseError::cancelled());
        }
        // Permission prompts are attended side effects. Verify the foreground
        // console-session fence before invoking either native request API.
        let (initial_probe, _, _) = self.inspect().await?;
        if initial_probe.permissions.active_session != ActiveSessionStatus::Active {
            return Ok(initial_probe);
        }
        if request.screen_recording && !CGPreflightScreenCaptureAccess() {
            if cancel.is_cancelled() {
                return Err(ComputerUseError::cancelled());
            }
            self.capture_prompt_attempted.store(true, Ordering::Release);
            let _immediate_result = CGRequestScreenCaptureAccess();
        }
        if request.accessibility && !CGPreflightPostEventAccess() {
            if cancel.is_cancelled() {
                return Err(ComputerUseError::cancelled());
            }
            // The preceding request may display UI or block. Re-establish the
            // foreground-session fence before each independent TCC request.
            let (post_event_fence, _, _) = self.inspect().await?;
            if post_event_fence.permissions.active_session != ActiveSessionStatus::Active {
                return Ok(post_event_fence);
            }
            let _immediate_post_event_result = CGRequestPostEventAccess();
        }
        if request.accessibility && !macos_accessibility::is_trusted() {
            if cancel.is_cancelled() {
                return Err(ComputerUseError::cancelled());
            }
            let (accessibility_fence, _, _) = self.inspect().await?;
            if accessibility_fence.permissions.active_session != ActiveSessionStatus::Active {
                return Ok(accessibility_fence);
            }
            if cancel.is_cancelled() {
                return Err(ComputerUseError::cancelled());
            }
            self.accessibility_prompt_attempted
                .store(true, Ordering::Release);
            let _immediate_accessibility_result = macos_accessibility::request_trust();
        }
        if cancel.is_cancelled() {
            return Err(ComputerUseError::cancelled());
        }
        let (probe, _, _) = self.inspect().await?;
        Ok(probe)
    }

    async fn open(&self, cancel: CancellationToken) -> Result<BackendProbe, ComputerUseError> {
        if cancel.is_cancelled() {
            return Err(ComputerUseError::cancelled());
        }
        let (mut probe, _, _) = self.inspect().await?;
        if probe.permissions.active_session == ActiveSessionStatus::Active
            && !probe.capabilities.observe
            && self.permission_prompts.capture_on_open
        {
            if cancel.is_cancelled() {
                return Err(ComputerUseError::cancelled());
            }
            if !self.capture_prompt_attempted.swap(true, Ordering::AcqRel) {
                let _immediate_result = CGRequestScreenCaptureAccess();
                (probe, _, _) = self.inspect().await?;
            }
        }
        match probe.permissions.active_session {
            ActiveSessionStatus::Locked => Err(ComputerUseError::new(
                ComputerUseErrorCode::SessionLocked,
                "the macOS console session is locked",
                RetryClassification::AfterExplicitResume,
            )),
            ActiveSessionStatus::Active if !probe.capabilities.observe => {
                Err(ComputerUseError::new(
                    ComputerUseErrorCode::PermissionRequired,
                    "Screen Recording permission is required for this executable identity",
                    RetryClassification::AfterPermissionChange,
                ))
            }
            ActiveSessionStatus::Active => Ok(probe),
            _ => Err(ComputerUseError::new(
                ComputerUseErrorCode::SessionInactive,
                "the macOS console session is not active",
                RetryClassification::AfterExplicitResume,
            )),
        }
    }

    async fn observe(
        &self,
        include_accessibility: bool,
        cancel: CancellationToken,
    ) -> Result<NativeObservation, ComputerUseError> {
        if cancel.is_cancelled() {
            return Err(ComputerUseError::cancelled());
        }
        let (probe, monitors, capture_fence) = self.inspect().await?;
        if !probe.capabilities.observe {
            return Err(
                if probe.permissions.active_session == ActiveSessionStatus::Locked {
                    ComputerUseError::new(
                        ComputerUseErrorCode::SessionLocked,
                        "the macOS console session is locked",
                        RetryClassification::AfterExplicitResume,
                    )
                } else {
                    ComputerUseError::new(
                        ComputerUseErrorCode::PermissionRequired,
                        "Screen Recording permission is required for this executable identity",
                        RetryClassification::AfterPermissionChange,
                    )
                },
            );
        }
        if include_accessibility
            && (!probe.capabilities.accessibility_snapshot
                || !probe.capabilities.pointer
                || !probe.capabilities.keyboard)
            && self.permission_prompts.accessibility_on_observe
            && !self
                .accessibility_prompt_attempted
                .swap(true, Ordering::AcqRel)
        {
            let _immediate_probe = self
                .request_permissions(
                    PermissionRequest {
                        screen_recording: false,
                        accessibility: true,
                    },
                    cancel.clone(),
                )
                .await?;
        }
        if include_accessibility && !macos_accessibility::is_trusted() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::PermissionRequired,
                "Accessibility permission is required for this executable identity",
                RetryClassification::AfterPermissionChange,
            ));
        }
        let scope = self.scope;
        let mut native = tokio::task::spawn_blocking(move || {
            capture_monitors(
                scope,
                monitors,
                capture_fence.target_generation,
                capture_fence.layout_generation,
            )
        })
        .await
        .map_err(|_| backend_error("macOS capture task failed"))??;
        if include_accessibility {
            let policy = self.accessibility_policy.clone();
            let geometry = native.geometry.clone();
            let generation = self.next_accessibility_generation()?;
            let captured_at_monotonic_ms = self.monotonic_ms();
            native.accessibility = Some(
                tokio::task::spawn_blocking(move || {
                    macos_accessibility::capture(
                        &policy,
                        &geometry,
                        generation,
                        captured_at_monotonic_ms,
                    )
                })
                .await
                .map_err(|_| backend_error("macOS Accessibility capture task failed"))??,
            );
        }
        let (post_capture_probe, _, post_capture_fence) = self.inspect().await?;
        validate_capture_fence(capture_fence, post_capture_fence)?;
        if include_accessibility && !post_capture_probe.capabilities.accessibility_snapshot {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::PermissionRequired,
                "Accessibility permission changed during capture; semantic content was discarded",
                RetryClassification::AfterPermissionChange,
            ));
        }
        if cancel.is_cancelled() {
            return Err(ComputerUseError::cancelled());
        }
        native.post_capture_probe = Some(post_capture_probe);
        Ok(native)
    }

    async fn execute(
        &self,
        action: &ComputerAction,
        geometry: &GeometrySnapshot,
        cancel: CancellationToken,
    ) -> Result<NativeActionReceipt, NativeActionFailure> {
        if cancel.is_cancelled() {
            return Err(action_failure(
                ComputerUseError::cancelled(),
                0,
                Vec::new(),
                InputCleanupStatus::NotRequired,
            ));
        }

        // Re-establish the active same-user unlocked console and display-layout
        // fence immediately before the first native input event.
        let (probe, _, fence) = self.inspect().await.map_err(|error| {
            action_failure(error, 0, Vec::new(), InputCleanupStatus::NotRequired)
        })?;
        require_input_fence(&probe, fence, geometry).map_err(|error| {
            action_failure(error, 0, Vec::new(), InputCleanupStatus::NotRequired)
        })?;
        let transformed_points = transformed_action_points(action, geometry).map_err(|error| {
            action_failure(error, 0, Vec::new(), InputCleanupStatus::NotRequired)
        })?;
        if cancel.is_cancelled() {
            return Err(action_failure(
                ComputerUseError::cancelled(),
                0,
                transformed_points,
                InputCleanupStatus::NotRequired,
            ));
        }
        let input = macos_input::NativeInput::new().map_err(|error| {
            action_failure(
                native_input_error(error),
                0,
                transformed_points.clone(),
                InputCleanupStatus::NotRequired,
            )
        })?;
        let mut guard = InputExecutionGuard::new(&self.state, input).map_err(|error| {
            action_failure(
                error,
                0,
                transformed_points.clone(),
                InputCleanupStatus::NotRequired,
            )
        })?;
        let guarded_input = guard.input_mut().map_err(|error| {
            action_failure(
                error,
                0,
                transformed_points.clone(),
                InputCleanupStatus::NotRequired,
            )
        })?;
        let outcome = tokio::select! {
            biased;
            result = execute_action(action, &transformed_points, guarded_input, &cancel) => result,
            result = self.monitor_input_fence(geometry, &cancel) => result,
        };
        let (cleanup, event_count, retention_error) = guard.finish();
        let cleanup_status = cleanup_status(cleanup);
        let outcome = retention_error.map_or(outcome, Err);
        match outcome {
            Ok(()) if cleanup.complete => Ok(NativeActionReceipt {
                effect_status: EffectStatus::Executed,
                native_event_count: event_count,
                transformed_points,
                cleanup: cleanup_status,
                stability_check: StabilityCheckStatus::NotPerformed,
            }),
            Ok(()) => Err(action_failure(
                ComputerUseError::new(
                    ComputerUseErrorCode::InputCleanupFailed,
                    "macOS input completed but held-input cleanup could not be confirmed",
                    RetryClassification::AfterExplicitResume,
                ),
                event_count,
                transformed_points,
                cleanup_status,
            )),
            Err(error) => Err(action_failure(
                error,
                event_count,
                transformed_points,
                cleanup_status,
            )),
        }
    }

    async fn close(&self, _reason: CloseReason) -> Result<InputCleanupStatus, ComputerUseError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| backend_error("macOS backend state is unavailable"))?;
        if state.active_input {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InputCleanupFailed,
                "cannot close the macOS backend while native input is active",
                RetryClassification::AfterExplicitResume,
            ));
        }
        let cleanup = state.pending_input.as_mut().map_or_else(
            macos_input::CleanupResult::default,
            macos_input::NativeInput::cleanup,
        );
        if cleanup.complete {
            state.pending_input = None;
        }
        state.closed = true;
        Ok(cleanup_status(cleanup))
    }
}

impl MacosDesktopBackend {
    async fn monitor_input_fence(
        &self,
        geometry: &GeometrySnapshot,
        cancel: &CancellationToken,
    ) -> Result<(), ComputerUseError> {
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(ComputerUseError::cancelled()),
                () = tokio::time::sleep(INPUT_FENCE_POLL_INTERVAL) => {}
            }
            let (probe, _, fence) = self.inspect().await?;
            require_input_fence(&probe, fence, geometry)?;
        }
    }
}

fn require_input_fence(
    probe: &BackendProbe,
    fence: CaptureFence,
    geometry: &GeometrySnapshot,
) -> Result<(), ComputerUseError> {
    match probe.permissions.active_session {
        ActiveSessionStatus::Active => {}
        ActiveSessionStatus::Locked => {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::SessionLocked,
                "the macOS console session is locked",
                RetryClassification::AfterExplicitResume,
            ));
        }
        ActiveSessionStatus::Inactive | ActiveSessionStatus::Unknown => {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::SessionInactive,
                "the macOS console session is not active for input",
                RetryClassification::AfterExplicitResume,
            ));
        }
    }
    if probe.permissions.capture != PermissionCapabilityStatus::Granted
        || !probe.capabilities.observe
        || !fence.capture_granted
        || !CGPreflightScreenCaptureAccess()
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::PermissionRequired,
            "macOS Screen Recording permission is required before input delivery",
            RetryClassification::AfterPermissionChange,
        ));
    }
    if probe.permissions.pointer_input != PermissionCapabilityStatus::Granted
        || probe.permissions.keyboard_input != PermissionCapabilityStatus::Granted
        || !CGPreflightPostEventAccess()
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::PermissionRequired,
            "macOS post-event permission is required for this executable identity",
            RetryClassification::AfterPermissionChange,
        ));
    }
    geometry.validate().map_err(|message| {
        ComputerUseError::new(
            ComputerUseErrorCode::InvalidTransform,
            message,
            RetryClassification::AfterFreshObservation,
        )
    })?;
    if geometry.target_generation != probe.target_generation
        || geometry.target_generation != fence.target_generation
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleTarget,
            "the macOS desktop target changed after the basis observation",
            RetryClassification::AfterFreshObservation,
        ));
    }
    if geometry.layout_generation != fence.layout_generation {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::StaleLayout,
            "the macOS display layout changed after the basis observation",
            RetryClassification::AfterFreshObservation,
        ));
    }
    validate_native_rect(geometry.native_desktop_rect)?;
    if geometry.displays.is_empty() {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidTransform,
            "the basis observation has no display geometry",
            RetryClassification::AfterFreshObservation,
        ));
    }
    for display in &geometry.displays {
        validate_display_geometry(geometry, display)?;
    }
    Ok(())
}

fn transformed_action_points(
    action: &ComputerAction,
    geometry: &GeometrySnapshot,
) -> Result<Vec<NativePoint>, ComputerUseError> {
    validate_native_action_shape(action)?;
    action_model_points(action)
        .into_iter()
        .map(|point| transform_point(geometry, point))
        .collect()
}

fn validate_native_action_shape(action: &ComputerAction) -> Result<(), ComputerUseError> {
    match action {
        ComputerAction::Click(value)
            if value.click_count == 0
                || value.click_count > MAX_NATIVE_CLICK_COUNT
                || value.modifiers.len() > MAX_NATIVE_MODIFIERS =>
        {
            Err(ComputerUseError::invalid(
                "the macOS click action exceeds native defensive bounds",
            ))
        }
        ComputerAction::MovePointer(value) if value.duration_ms > MAX_NATIVE_DURATION_MS => Err(
            ComputerUseError::invalid("the macOS pointer duration exceeds the native bound"),
        ),
        ComputerAction::Drag(value)
            if value.path.len() < 2
                || value.path.len() > MAX_NATIVE_PATH_POINTS
                || value.duration_ms > MAX_NATIVE_DURATION_MS
                || value.modifiers.len() > MAX_NATIVE_MODIFIERS =>
        {
            Err(ComputerUseError::invalid(
                "the macOS drag action exceeds native defensive bounds",
            ))
        }
        ComputerAction::Scroll(value)
            if value.delta_x_model_px.unsigned_abs() > MAX_NATIVE_SCROLL_ABS
                || value.delta_y_model_px.unsigned_abs() > MAX_NATIVE_SCROLL_ABS
                || value.modifiers.len() > MAX_NATIVE_MODIFIERS =>
        {
            Err(ComputerUseError::invalid(
                "the macOS scroll action exceeds native defensive bounds",
            ))
        }
        ComputerAction::TypeText(value)
            if value.text.is_empty()
                || value.text.len() > MAX_NATIVE_TEXT_BYTES
                || value.text.chars().count() > MAX_NATIVE_TEXT_SCALARS =>
        {
            Err(ComputerUseError::invalid(
                "the macOS text action exceeds native defensive bounds",
            ))
        }
        ComputerAction::PressKeys(value)
            if value.keys.is_empty() || value.keys.len() > MAX_NATIVE_KEYS =>
        {
            Err(ComputerUseError::invalid(
                "the macOS key action exceeds native defensive bounds",
            ))
        }
        _ => Ok(()),
    }
}

fn action_model_points(action: &ComputerAction) -> Vec<ModelPoint> {
    match action {
        ComputerAction::Click(value) => vec![value.point],
        ComputerAction::MovePointer(value) => vec![value.point],
        ComputerAction::Drag(value) => value.path.clone(),
        ComputerAction::Scroll(value) => vec![value.anchor],
        ComputerAction::TypeText(_) | ComputerAction::PressKeys(_) => Vec::new(),
    }
}

fn transform_point(
    geometry: &GeometrySnapshot,
    point: ModelPoint,
) -> Result<NativePoint, ComputerUseError> {
    if !geometry.model_size_px.contains(point) {
        return Err(invalid_coordinate(
            "the model point is outside the basis observation",
        ));
    }
    let display = geometry
        .displays
        .iter()
        .find(|display| model_rect_contains(display.model_rect, point))
        .ok_or_else(|| {
            invalid_coordinate("the model point falls in a gap between active displays")
        })?;
    validate_native_rect(display.native_rect)?;
    let native = geometry.model_to_native.apply(point);
    if !native.x.is_finite() || !native.y.is_finite() {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidTransform,
            "the basis transform produced a non-finite native point",
            RetryClassification::AfterFreshObservation,
        ));
    }
    if !native_rect_contains(display.native_rect, native)
        || !native_rect_contains(geometry.native_desktop_rect, native)
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidTransform,
            "the transformed point is outside its native display geometry",
            RetryClassification::AfterFreshObservation,
        ));
    }
    Ok(native)
}

const fn model_rect_contains(rect: ModelRect, point: ModelPoint) -> bool {
    point.x >= rect.x
        && point.y >= rect.y
        && point.x < rect.x.saturating_add(rect.width)
        && point.y < rect.y.saturating_add(rect.height)
}

fn native_rect_contains(rect: NativeRect, point: NativePoint) -> bool {
    point.x >= rect.x
        && point.y >= rect.y
        && point.x < rect.x + rect.width
        && point.y < rect.y + rect.height
}

fn validate_display_geometry(
    geometry: &GeometrySnapshot,
    display: &DisplayGeometry,
) -> Result<(), ComputerUseError> {
    let model_right = display
        .model_rect
        .x
        .checked_add(display.model_rect.width)
        .ok_or_else(|| invalid_transform("display model bounds overflow"))?;
    let model_bottom = display
        .model_rect
        .y
        .checked_add(display.model_rect.height)
        .ok_or_else(|| invalid_transform("display model bounds overflow"))?;
    if display.model_rect.width == 0
        || display.model_rect.height == 0
        || model_right > geometry.model_size_px.width
        || model_bottom > geometry.model_size_px.height
        || !display.scale_factor.is_finite()
        || display.scale_factor <= 0.0
        || !matches!(display.rotation_degrees, 0 | 90 | 180 | 270)
    {
        return Err(invalid_transform(
            "the basis observation contains invalid display geometry",
        ));
    }
    validate_native_rect(display.native_rect)?;
    let first = geometry.model_to_native.apply(ModelPoint {
        x: display.model_rect.x,
        y: display.model_rect.y,
    });
    let last = geometry.model_to_native.apply(ModelPoint {
        x: model_right - 1,
        y: model_bottom - 1,
    });
    if !native_rect_contains(display.native_rect, first)
        || !native_rect_contains(display.native_rect, last)
        || !native_rect_contains(geometry.native_desktop_rect, first)
        || !native_rect_contains(geometry.native_desktop_rect, last)
    {
        return Err(invalid_transform(
            "display model bounds do not match the native transform",
        ));
    }
    Ok(())
}

fn validate_native_rect(rect: NativeRect) -> Result<(), ComputerUseError> {
    if !rect.x.is_finite()
        || !rect.y.is_finite()
        || !rect.width.is_finite()
        || !rect.height.is_finite()
        || rect.width <= 0.0
        || rect.height <= 0.0
        || !(rect.x + rect.width).is_finite()
        || !(rect.y + rect.height).is_finite()
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidTransform,
            "the basis observation contains invalid native display bounds",
            RetryClassification::AfterFreshObservation,
        ));
    }
    Ok(())
}

fn invalid_coordinate(message: &str) -> ComputerUseError {
    ComputerUseError::new(
        ComputerUseErrorCode::InvalidCoordinate,
        message,
        RetryClassification::AfterFreshObservation,
    )
}

fn invalid_transform(message: &str) -> ComputerUseError {
    ComputerUseError::new(
        ComputerUseErrorCode::InvalidTransform,
        message,
        RetryClassification::AfterFreshObservation,
    )
}

fn key_press_order(keys: &[CanonicalKey], mode: KeyMode) -> Vec<CanonicalKey> {
    match mode {
        KeyMode::Chord => keys
            .iter()
            .copied()
            .filter(|key| is_modifier_key(*key))
            .chain(keys.iter().copied().filter(|key| !is_modifier_key(*key)))
            .collect(),
        KeyMode::Sequence => keys.to_vec(),
    }
}

const fn is_modifier_key(key: CanonicalKey) -> bool {
    matches!(
        key,
        CanonicalKey::Shift | CanonicalKey::Control | CanonicalKey::Alt | CanonicalKey::Meta
    )
}

#[allow(clippy::too_many_lines)]
async fn execute_action(
    action: &ComputerAction,
    points: &[NativePoint],
    input: &mut macos_input::NativeInput,
    cancel: &CancellationToken,
) -> Result<(), ComputerUseError> {
    match action {
        ComputerAction::Click(value) => {
            let point = points[0];
            for click_state in 1..=value.click_count {
                check_cancel(cancel)?;
                input
                    .mouse_down(value.button, point, &value.modifiers, click_state)
                    .map_err(native_input_error)?;
                check_cancel(cancel)?;
                input
                    .mouse_up(value.button, point, &value.modifiers, click_state)
                    .map_err(native_input_error)?;
            }
        }
        ComputerAction::MovePointer(value) => {
            let start = macos_input::NativeInput::pointer_location().map_err(native_input_error)?;
            let samples = movement_samples(start, points[0], value.duration_ms);
            run_timed_pointer_samples(input, &samples, value.duration_ms, cancel, false, None)
                .await?;
        }
        ComputerAction::Drag(value) => {
            let first = points[0];
            check_cancel(cancel)?;
            input.move_pointer(first).map_err(native_input_error)?;
            check_cancel(cancel)?;
            input
                .mouse_down(value.button, first, &value.modifiers, 1)
                .map_err(native_input_error)?;
            let samples = drag_samples(points, value.duration_ms);
            run_timed_pointer_samples(
                input,
                &samples,
                value.duration_ms,
                cancel,
                true,
                Some((value.button, value.modifiers.as_slice())),
            )
            .await?;
            check_cancel(cancel)?;
            input
                .mouse_up(
                    value.button,
                    *points.last().unwrap_or(&first),
                    &value.modifiers,
                    1,
                )
                .map_err(native_input_error)?;
        }
        ComputerAction::Scroll(value) => {
            check_cancel(cancel)?;
            let (wheel_x, wheel_y) =
                core_graphics_scroll_deltas(value.delta_x_model_px, value.delta_y_model_px);
            input
                .scroll(points[0], wheel_x, wheel_y, &value.modifiers)
                .map_err(native_input_error)?;
        }
        ComputerAction::TypeText(value) => {
            let parts = macos_input::text_input_parts(&value.text);
            let part_count = parts.len();
            let part_delay = text_input_part_delay(part_count);
            for (index, part) in parts.into_iter().enumerate() {
                check_cancel(cancel)?;
                match part {
                    macos_input::TextInputPart::Unicode(chunk) => input
                        .type_unicode_chunk(&chunk)
                        .map_err(native_input_error)?,
                    macos_input::TextInputPart::Key(key) => {
                        input.key_down(key).map_err(native_input_error)?;
                        check_cancel(cancel)?;
                        input.key_up(key).map_err(native_input_error)?;
                    }
                }
                if index + 1 < part_count {
                    wait_until(tokio::time::Instant::now() + part_delay, cancel).await?;
                }
            }
        }
        ComputerAction::PressKeys(value) => match value.mode {
            KeyMode::Chord => {
                let ordered = key_press_order(&value.keys, value.mode);
                for key in &ordered {
                    check_cancel(cancel)?;
                    input.key_down(*key).map_err(native_input_error)?;
                }
                for key in ordered.iter().rev() {
                    check_cancel(cancel)?;
                    input.key_up(*key).map_err(native_input_error)?;
                }
            }
            KeyMode::Sequence => {
                for key in &value.keys {
                    check_cancel(cancel)?;
                    input.key_down(*key).map_err(native_input_error)?;
                    check_cancel(cancel)?;
                    input.key_up(*key).map_err(native_input_error)?;
                }
            }
        },
    }
    Ok(())
}

async fn run_timed_pointer_samples(
    input: &mut macos_input::NativeInput,
    samples: &[NativePoint],
    duration_ms: u32,
    cancel: &CancellationToken,
    dragging: bool,
    drag_state: Option<(crate::PointerButton, &[crate::ModifierKey])>,
) -> Result<(), ComputerUseError> {
    let started = tokio::time::Instant::now();
    let count = u32::try_from(samples.len()).unwrap_or(u32::MAX).max(1);
    for (index, point) in samples.iter().enumerate() {
        if duration_ms != 0 {
            let numerator =
                u64::from(duration_ms).saturating_mul(u64::try_from(index + 1).unwrap_or(u64::MAX));
            let offset_ms = numerator / u64::from(count);
            wait_until(
                started + std::time::Duration::from_millis(offset_ms),
                cancel,
            )
            .await?;
        } else {
            check_cancel(cancel)?;
        }
        if dragging {
            let (button, modifiers) = drag_state.ok_or_else(|| {
                ComputerUseError::new(
                    ComputerUseErrorCode::Internal,
                    "macOS drag state is unavailable",
                    RetryClassification::Never,
                )
            })?;
            input
                .mouse_drag(button, *point, modifiers)
                .map_err(native_input_error)?;
        } else {
            input.move_pointer(*point).map_err(native_input_error)?;
        }
    }
    Ok(())
}

async fn wait_until(
    deadline: tokio::time::Instant,
    cancel: &CancellationToken,
) -> Result<(), ComputerUseError> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(ComputerUseError::cancelled()),
        () = tokio::time::sleep_until(deadline) => Ok(()),
    }
}

fn text_input_part_delay(part_count: usize) -> std::time::Duration {
    let interval_count = u64::try_from(part_count.saturating_sub(1)).unwrap_or(u64::MAX);
    if interval_count == 0 {
        return std::time::Duration::ZERO;
    }
    let delay_ms =
        (MAX_TEXT_INPUT_PACING_BUDGET_MS / interval_count).clamp(1, MAX_TEXT_INPUT_PART_DELAY_MS);
    std::time::Duration::from_millis(delay_ms)
}

fn check_cancel(cancel: &CancellationToken) -> Result<(), ComputerUseError> {
    if cancel.is_cancelled() {
        Err(ComputerUseError::cancelled())
    } else {
        Ok(())
    }
}

/// Convert model-space scroll deltas (positive right/down) to CoreGraphics
/// wheel deltas (positive left/up). Saturation keeps direct backend calls with
/// `i32::MIN` deterministic even though the service normally applies bounds.
const fn core_graphics_scroll_deltas(delta_x: i32, delta_y: i32) -> (i32, i32) {
    (delta_x.saturating_neg(), delta_y.saturating_neg())
}

fn movement_samples(start: NativePoint, end: NativePoint, duration_ms: u32) -> Vec<NativePoint> {
    let steps = duration_ms
        .div_ceil(16)
        .clamp(1, MAX_NATIVE_POINTER_SAMPLES);
    (1..=steps)
        .map(|step| interpolate(start, end, f64::from(step) / f64::from(steps)))
        .collect()
}

fn drag_samples(path: &[NativePoint], duration_ms: u32) -> Vec<NativePoint> {
    if path.len() < 2 {
        return Vec::new();
    }
    let segment_count = u32::try_from(path.len() - 1).unwrap_or(MAX_NATIVE_POINTER_SAMPLES);
    let requested_steps = duration_ms
        .div_ceil(16)
        .max(segment_count)
        .min(MAX_NATIVE_POINTER_SAMPLES);
    let base_steps = requested_steps / segment_count;
    let extra_steps = requested_steps % segment_count;
    let mut samples =
        Vec::with_capacity(usize::try_from(requested_steps).unwrap_or(MAX_NATIVE_PATH_POINTS));
    for (index, segment) in path.windows(2).enumerate() {
        let segment_steps =
            base_steps + u32::from(u32::try_from(index).unwrap_or(u32::MAX) < extra_steps);
        for step in 1..=segment_steps {
            samples.push(interpolate(
                segment[0],
                segment[1],
                f64::from(step) / f64::from(segment_steps),
            ));
        }
    }
    samples
}

fn interpolate(start: NativePoint, end: NativePoint, progress: f64) -> NativePoint {
    NativePoint {
        x: (end.x - start.x).mul_add(progress, start.x),
        y: (end.y - start.y).mul_add(progress, start.y),
    }
}

fn native_input_error(error: macos_input::InputError) -> ComputerUseError {
    match error {
        macos_input::InputError::PermissionRequired => ComputerUseError::new(
            ComputerUseErrorCode::PermissionRequired,
            "macOS post-event permission changed before input delivery completed",
            RetryClassification::AfterPermissionChange,
        ),
        macos_input::InputError::EventCreationFailed => ComputerUseError::new(
            ComputerUseErrorCode::InputRejected,
            "CoreGraphics could not create a native input event",
            RetryClassification::AfterFreshObservation,
        ),
    }
}

const fn cleanup_status(cleanup: macos_input::CleanupResult) -> InputCleanupStatus {
    if !cleanup.required {
        InputCleanupStatus::NotRequired
    } else if cleanup.complete {
        InputCleanupStatus::Complete
    } else if cleanup.event_count == 0 {
        InputCleanupStatus::Failed
    } else {
        InputCleanupStatus::BestEffort
    }
}

const fn action_failure(
    error: ComputerUseError,
    native_event_count: u32,
    transformed_points: Vec<NativePoint>,
    cleanup: InputCleanupStatus,
) -> NativeActionFailure {
    let effect_status = if native_event_count == 0 {
        EffectStatus::NotExecuted
    } else {
        EffectStatus::PartiallyExecuted
    };
    NativeActionFailure {
        error,
        effect_status,
        receipt: Some(NativeActionReceipt {
            effect_status,
            native_event_count,
            transformed_points,
            cleanup,
            stability_check: StabilityCheckStatus::NotPerformed,
        }),
        cleanup,
    }
}

fn validate_capture_fence(
    before: CaptureFence,
    after: CaptureFence,
) -> Result<(), ComputerUseError> {
    match after.active_session {
        ActiveSessionStatus::Locked => {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::SessionLocked,
                "the macOS console session locked during capture; captured bytes were discarded",
                RetryClassification::AfterExplicitResume,
            ));
        }
        ActiveSessionStatus::Active => {}
        ActiveSessionStatus::Inactive | ActiveSessionStatus::Unknown => {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::SessionInactive,
                "the macOS foreground console session changed during capture; captured bytes were discarded",
                RetryClassification::AfterExplicitResume,
            ));
        }
    }
    if !after.capture_granted {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::PermissionRequired,
            "Screen Recording permission changed during capture; captured bytes were discarded",
            RetryClassification::AfterPermissionChange,
        ));
    }
    if before.active_session != after.active_session
        || before.target_generation != after.target_generation
        || before.topology_digest != after.topology_digest
        || before.layout_generation != after.layout_generation
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::DisplayTopologyChanged,
            "the macOS display topology changed during capture; captured bytes were discarded",
            RetryClassification::AfterFreshObservation,
        ));
    }
    Ok(())
}

fn ensure_backend_open(state: &BackendState) -> Result<(), ComputerUseError> {
    if state.closed {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::SessionClosed,
            "macOS desktop backend is closed",
            RetryClassification::NewSessionRequired,
        ));
    }
    Ok(())
}

fn reserve_input(state: &mut BackendState) -> Result<(), ComputerUseError> {
    ensure_backend_open(state)?;
    if state.active_input || state.pending_input.is_some() {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InputCleanupFailed,
            "another macOS input action or cleanup is still active",
            RetryClassification::AfterExplicitResume,
        ));
    }
    state.active_input = true;
    Ok(())
}

fn update_backend_generations(
    state: &mut BackendState,
    session_marker: [u8; 32],
    session_transition_epoch: u64,
    active_session: ActiveSessionStatus,
    topology_digest: Option<[u8; 32]>,
) -> (TargetGeneration, LayoutGeneration) {
    let session_changed = state
        .session_marker
        .is_some_and(|value| value != session_marker)
        || state
            .session_transition_epoch
            .is_some_and(|value| value != session_transition_epoch)
        || state
            .active_session
            .is_some_and(|value| value != active_session);
    if session_changed {
        state.target_generation.0 = state.target_generation.0.saturating_add(1);
    }
    state.session_marker = Some(session_marker);
    state.session_transition_epoch = Some(session_transition_epoch);
    state.active_session = Some(active_session);
    if let Some(topology_digest) = topology_digest {
        if state
            .topology_digest
            .is_some_and(|value| value != topology_digest)
        {
            state.layout_generation.0 = state.layout_generation.0.saturating_add(1);
        }
        state.topology_digest = Some(topology_digest);
    }
    (state.target_generation, state.layout_generation)
}

struct ConsoleSession {
    locked: bool,
    owned_by_process_user: bool,
    session_marker: [u8; 32],
}

fn console_session() -> Result<ConsoleSession, ComputerUseError> {
    let console_uid = std::fs::metadata("/dev/console")
        .map_err(|_| backend_error("failed to inspect the macOS foreground console owner"))?
        .uid();
    let process_uid = nix::unistd::geteuid().as_raw();
    validate_session_uids(console_uid, process_uid)?;

    let output = Command::new("/usr/sbin/ioreg")
        .args(["-n", "Root", "-d1", "-a"])
        .output()
        .map_err(|_| backend_error("failed to query macOS console lock state"))?;
    if !output.status.success() || output.stdout.len() > 4 * 1024 * 1024 {
        return Err(backend_error("macOS console lock-state query failed"));
    }
    let compact: Vec<u8> = output
        .stdout
        .into_iter()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    let locked_key = b"<key>IOConsoleLocked</key>";
    let Some(position) = compact
        .windows(locked_key.len())
        .position(|part| part == locked_key)
    else {
        return Err(backend_error("macOS console lock state is unavailable"));
    };
    let tail = &compact[position + locked_key.len()..];
    let locked = if tail.starts_with(b"<true/>") {
        true
    } else if tail.starts_with(b"<false/>") {
        false
    } else {
        return Err(backend_error("macOS console lock state is malformed"));
    };
    let session_marker = console_session_marker(&compact, console_uid)?;
    Ok(ConsoleSession {
        locked,
        owned_by_process_user: console_uid == process_uid,
        session_marker,
    })
}

fn console_session_marker(compact: &[u8], console_uid: u32) -> Result<[u8; 32], ComputerUseError> {
    let users_key = b"<key>IOConsoleUsers</key><array>";
    let users_start = find_bytes(compact, users_key)
        .map(|index| index + users_key.len())
        .ok_or_else(|| backend_error("macOS console session list is unavailable"))?;
    let users = &compact[users_start..];
    let users_end = find_bytes(users, b"</array>")
        .ok_or_else(|| backend_error("macOS console session list is malformed"))?;
    let users = &users[..users_end];
    let uid_marker = format!("<key>kCGSSessionUserIDKey</key><integer>{console_uid}</integer>");
    let on_console_marker = b"<key>kCGSSessionOnConsoleKey</key><true/>";
    let mut remaining = users;
    while let Some(dict_start) = find_bytes(remaining, b"<dict>") {
        let dict_tail = &remaining[dict_start + b"<dict>".len()..];
        let Some(dict_end) = find_bytes(dict_tail, b"</dict>") else {
            break;
        };
        let dict = &dict_tail[..dict_end];
        remaining = &dict_tail[dict_end + b"</dict>".len()..];
        if find_bytes(dict, uid_marker.as_bytes()).is_none()
            || find_bytes(dict, on_console_marker).is_none()
        {
            continue;
        }
        let uuid = xml_value(dict, "CGSSessionUniqueSessionUUID", "string")
            .ok_or_else(|| backend_error("macOS console session UUID is unavailable"))?;
        let audit_id = xml_value(dict, "kCGSSessionAuditIDKey", "integer")
            .ok_or_else(|| backend_error("macOS console session audit ID is unavailable"))?;
        let locked_at =
            xml_value(dict, "CGSSessionScreenLockedTime", "integer").unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(console_uid.to_be_bytes());
        hasher.update([0]);
        hasher.update(uuid);
        hasher.update([0]);
        hasher.update(audit_id);
        hasher.update([0]);
        hasher.update(locked_at);
        return Ok(hasher.finalize().into());
    }
    Err(backend_error(
        "the active macOS console session identity is unavailable",
    ))
}

fn xml_value<'a>(dict: &'a [u8], key: &str, tag: &str) -> Option<&'a [u8]> {
    let prefix = format!("<key>{key}</key><{tag}>");
    let start = find_bytes(dict, prefix.as_bytes())? + prefix.len();
    let suffix = format!("</{tag}>");
    let end = find_bytes(&dict[start..], suffix.as_bytes())?;
    Some(&dict[start..start + end])
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    (!needle.is_empty())
        .then(|| {
            haystack
                .windows(needle.len())
                .position(|part| part == needle)
        })
        .flatten()
}

fn permission_remediation(
    console: &ConsoleSession,
    capture_granted: bool,
    input_granted: bool,
    accessibility_granted: bool,
) -> Vec<String> {
    if !console.owned_by_process_user {
        return vec![
            "Run Computer Use as the user who owns the current foreground macOS console session."
                .into(),
        ];
    }
    if console.locked {
        return vec![
            "Unlock the current macOS console session and retry from the same foreground user session."
                .into(),
        ];
    }
    let mut remediation = Vec::new();
    if !capture_granted {
        remediation.push("Grant Screen Recording permission to this exact executable identity in System Settings, then restart it if macOS requires a restart.".into());
    }
    if !input_granted {
        remediation.push("Grant macOS Accessibility/post-event permission to this exact executable identity in System Settings, then retry input from the same unlocked user session.".into());
    }
    if !accessibility_granted {
        remediation.push("Grant Accessibility permission to this exact executable identity in System Settings, then retry the observation.".into());
    }
    remediation
}

fn validate_session_uids(console_uid: u32, process_uid: u32) -> Result<(), ComputerUseError> {
    if process_uid == 0 {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::SessionInactive,
            "Computer Use refuses to capture from a root process",
            RetryClassification::Never,
        ));
    }
    if console_uid == 0 {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::SessionInactive,
            "no non-root user owns the active macOS graphical console",
            RetryClassification::AfterExplicitResume,
        ));
    }
    Ok(())
}

fn topology_digest(monitors: &[Monitor]) -> Result<[u8; 32], ComputerUseError> {
    let mut value = String::new();
    for monitor in monitors {
        use std::fmt::Write as _;
        write!(
            value,
            "{}:{}:{}:{}:{}:{}:{};",
            monitor
                .id()
                .map_err(|_| backend_error("failed to read macOS display id"))?,
            monitor
                .x()
                .map_err(|_| backend_error("failed to read macOS display x"))?,
            monitor
                .y()
                .map_err(|_| backend_error("failed to read macOS display y"))?,
            monitor
                .width()
                .map_err(|_| backend_error("failed to read macOS display width"))?,
            monitor
                .height()
                .map_err(|_| backend_error("failed to read macOS display height"))?,
            monitor
                .rotation()
                .map_err(|_| backend_error("failed to read macOS display rotation"))?,
            monitor
                .scale_factor()
                .map_err(|_| backend_error("failed to read macOS display scale"))?,
        )
        .map_err(|_| backend_error("failed to fingerprint macOS display topology"))?;
    }
    Ok(Sha256::digest(value.as_bytes()).into())
}

#[allow(clippy::too_many_lines)]
fn capture_monitors(
    scope: DesktopSurfaceScope,
    mut monitors: Vec<Monitor>,
    target_generation: TargetGeneration,
    layout_generation: LayoutGeneration,
) -> Result<NativeObservation, ComputerUseError> {
    if scope == DesktopSurfaceScope::PrimaryDisplay {
        let selected = monitors
            .iter()
            .position(|monitor| monitor.is_primary().unwrap_or(false))
            .unwrap_or(0);
        monitors = vec![monitors.swap_remove(selected)];
    }
    let mut metadata = Vec::with_capacity(monitors.len());
    for monitor in &monitors {
        let x = monitor
            .x()
            .map_err(|_| backend_error("failed to read macOS display x"))?;
        let y = monitor
            .y()
            .map_err(|_| backend_error("failed to read macOS display y"))?;
        let width = monitor
            .width()
            .map_err(|_| backend_error("failed to read macOS display width"))?;
        let height = monitor
            .height()
            .map_err(|_| backend_error("failed to read macOS display height"))?;
        metadata.push((
            x,
            y,
            width,
            height,
            monitor.is_primary().unwrap_or(false),
            monitor.scale_factor().unwrap_or(1.0),
            monitor.rotation().unwrap_or(0.0),
        ));
    }
    let min_x = metadata
        .iter()
        .map(|value| value.0)
        .min()
        .ok_or_else(|| backend_error("macOS reported no active display"))?;
    let min_y = metadata
        .iter()
        .map(|value| value.1)
        .min()
        .ok_or_else(|| backend_error("macOS reported no active display"))?;
    let max_x = metadata
        .iter()
        .map(|value| i64::from(value.0) + i64::from(value.2))
        .max()
        .ok_or_else(|| backend_error("macOS display width is unavailable"))?;
    let max_y = metadata
        .iter()
        .map(|value| i64::from(value.1) + i64::from(value.3))
        .max()
        .ok_or_else(|| backend_error("macOS display height is unavailable"))?;
    let width = u32::try_from(max_x - i64::from(min_x))
        .map_err(|_| backend_error("macOS desktop width exceeds V1 limits"))?;
    let height = u32::try_from(max_y - i64::from(min_y))
        .map_err(|_| backend_error("macOS desktop height exceeds V1 limits"))?;
    if width == 0 || height == 0 {
        return Err(backend_error("macOS desktop geometry is empty"));
    }
    let allocation = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| backend_error("macOS desktop allocation overflows"))?;
    if allocation > 256 * 1024 * 1024 {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::ImageLimitExceeded,
            "macOS desktop capture exceeds the native allocation bound",
            RetryClassification::Never,
        ));
    }
    let mut composite = RgbaImage::new(width, height);
    let mut displays = Vec::with_capacity(monitors.len());
    for (monitor, (x, y, logical_width, logical_height, primary, scale, rotation)) in
        monitors.iter().zip(metadata)
    {
        let captured = monitor.capture_image().map_err(|_| {
            if CGPreflightScreenCaptureAccess() {
                ComputerUseError::new(
                    ComputerUseErrorCode::CaptureInterrupted,
                    "macOS desktop capture was interrupted",
                    RetryClassification::AfterFreshObservation,
                )
            } else {
                ComputerUseError::new(
                    ComputerUseErrorCode::PermissionRequired,
                    "Screen Recording permission is required for this executable identity",
                    RetryClassification::AfterPermissionChange,
                )
            }
        })?;
        let normalized = if captured.width() == logical_width && captured.height() == logical_height
        {
            captured
        } else {
            image::imageops::resize(
                &captured,
                logical_width,
                logical_height,
                FilterType::Lanczos3,
            )
        };
        let offset_x = i64::from(x) - i64::from(min_x);
        let offset_y = i64::from(y) - i64::from(min_y);
        image::imageops::overlay(&mut composite, &normalized, offset_x, offset_y);
        displays.push(DisplayGeometry {
            model_rect: ModelRect {
                x: u32::try_from(offset_x)
                    .map_err(|_| backend_error("macOS display x transform is invalid"))?,
                y: u32::try_from(offset_y)
                    .map_err(|_| backend_error("macOS display y transform is invalid"))?,
                width: logical_width,
                height: logical_height,
            },
            native_rect: NativeRect {
                x: f64::from(x),
                y: f64::from(y),
                width: f64::from(logical_width),
                height: f64::from(logical_height),
            },
            scale_factor: f64::from(scale),
            rotation_degrees: canonical_rotation(rotation)?,
            primary,
        });
    }
    let model_to_native = AffineTransform2D::checked([
        1.0,
        0.0,
        f64::from(min_x),
        0.0,
        1.0,
        f64::from(min_y),
        0.0,
        0.0,
        1.0,
    ])
    .map_err(|message| {
        ComputerUseError::new(
            ComputerUseErrorCode::InvalidTransform,
            message,
            RetryClassification::Never,
        )
    })?;
    let native_to_model = model_to_native.inverse().map_err(|message| {
        ComputerUseError::new(
            ComputerUseErrorCode::InvalidTransform,
            message,
            RetryClassification::Never,
        )
    })?;
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(composite)
        .write_to(&mut encoded, ImageFormat::Png)
        .map_err(|_| backend_error("failed to encode macOS desktop capture"))?;
    Ok(NativeObservation {
        geometry: GeometrySnapshot {
            target_generation,
            layout_generation,
            model_size_px: PixelSize { width, height },
            native_desktop_rect: NativeRect {
                x: f64::from(min_x),
                y: f64::from(min_y),
                width: f64::from(width),
                height: f64::from(height),
            },
            model_to_native,
            native_to_model,
            displays,
            cursor_embedded: false,
        },
        mime_type: DesktopImageMime::ImagePng,
        image_bytes: encoded.into_inner(),
        color_space: Some("srgb".into()),
        // CoreGraphics does not provide a complete protected-content signal.
        // The uncertainty is explicit rather than claiming a complete frame.
        redaction: FrameRedactionStatus::Uncertain,
        accessibility: None,
        post_capture_probe: None,
    })
}

fn canonical_rotation(rotation: f32) -> Result<u16, ComputerUseError> {
    const ROTATIONS: [(f32, u16); 4] = [(0.0, 0), (90.0, 90), (180.0, 180), (270.0, 270)];
    ROTATIONS
        .into_iter()
        .find_map(|(expected, value)| ((rotation - expected).abs() <= 0.5).then_some(value))
        .ok_or_else(|| backend_error("macOS display rotation is unsupported"))
}

fn backend_error(message: &str) -> ComputerUseError {
    ComputerUseError::new(
        ComputerUseErrorCode::BackendUnavailable,
        message,
        RetryClassification::AfterFreshObservation,
    )
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{
        BackendState, CaptureFence, InputExecutionGuard, MacosDesktopBackend, cleanup_status,
        console_session_marker, core_graphics_scroll_deltas, drag_samples, key_press_order,
        macos_input::NativeInput, movement_samples, reserve_input, text_input_part_delay,
        transform_point, update_backend_generations, validate_capture_fence,
        validate_native_action_shape, validate_session_uids,
    };
    use crate::{
        AccessibilityGeneration, ActiveSessionStatus, AffineTransform2D, CanonicalKey, CloseReason,
        ComputerAction, ComputerUseErrorCode, ComputerUsePolicy, DisplayGeometry, DragAction,
        GeometrySnapshot, InputCleanupStatus, LayoutGeneration, ModelPoint, ModelRect,
        NativeDesktopBackend, NativePoint, NativeRect, PixelSize, PointerButton,
        RetryClassification, TargetGeneration,
    };

    const fn fence(digest: [u8; 32]) -> CaptureFence {
        CaptureFence {
            active_session: ActiveSessionStatus::Active,
            capture_granted: true,
            target_generation: TargetGeneration(1),
            topology_digest: Some(digest),
            layout_generation: LayoutGeneration(1),
        }
    }

    #[test]
    fn session_uid_gate_rejects_root_and_loginwindow() {
        let root_process = validate_session_uids(501, 0)
            .expect_err("root process must be rejected before capture");
        assert_eq!(root_process.code, ComputerUseErrorCode::SessionInactive);
        assert_eq!(root_process.retry, RetryClassification::Never);

        let loginwindow = validate_session_uids(0, 501)
            .expect_err("root-owned console must be rejected before capture");
        assert_eq!(loginwindow.code, ComputerUseErrorCode::SessionInactive);
        assert_eq!(loginwindow.retry, RetryClassification::AfterExplicitResume);
    }

    #[test]
    fn console_session_marker_tracks_session_and_lock_transitions() {
        let unlocked = br"<plist><dict><key>IOConsoleUsers</key><array><dict><key>kCGSSessionOnConsoleKey</key><true/><key>kCGSSessionUserIDKey</key><integer>501</integer><key>CGSSessionUniqueSessionUUID</key><string>session-a</string><key>kCGSSessionAuditIDKey</key><integer>100003</integer><key>CGSSessionScreenLockedTime</key><integer>10</integer></dict></array></dict></plist>";
        let relocked = br"<plist><dict><key>IOConsoleUsers</key><array><dict><key>kCGSSessionOnConsoleKey</key><true/><key>kCGSSessionUserIDKey</key><integer>501</integer><key>CGSSessionUniqueSessionUUID</key><string>session-a</string><key>kCGSSessionAuditIDKey</key><integer>100003</integer><key>CGSSessionScreenLockedTime</key><integer>11</integer></dict></array></dict></plist>";
        let first = console_session_marker(unlocked, 501).expect("fixture session is valid");
        let second = console_session_marker(relocked, 501).expect("fixture session is valid");
        assert_ne!(first, second);
    }

    #[test]
    fn target_generation_advances_across_observed_session_transitions() {
        let mut state = BackendState {
            session_marker: None,
            session_transition_epoch: None,
            active_session: None,
            target_generation: TargetGeneration(1),
            topology_digest: None,
            layout_generation: LayoutGeneration(1),
            accessibility_generation: AccessibilityGeneration(0),
            active_input: false,
            pending_input: None,
            closed: false,
        };
        let marker = [1; 32];
        assert_eq!(
            update_backend_generations(
                &mut state,
                marker,
                0,
                ActiveSessionStatus::Active,
                Some([2; 32]),
            ),
            (TargetGeneration(1), LayoutGeneration(1))
        );
        assert_eq!(
            update_backend_generations(&mut state, marker, 1, ActiveSessionStatus::Locked, None,).0,
            TargetGeneration(2)
        );
        assert_eq!(
            update_backend_generations(
                &mut state,
                marker,
                2,
                ActiveSessionStatus::Active,
                Some([3; 32]),
            ),
            (TargetGeneration(3), LayoutGeneration(2))
        );
        assert_eq!(
            update_backend_generations(
                &mut state,
                [4; 32],
                2,
                ActiveSessionStatus::Active,
                Some([3; 32]),
            )
            .0,
            TargetGeneration(4)
        );
    }

    #[test]
    fn sampled_session_epoch_invalidates_the_same_unlocked_session() {
        let mut state = BackendState {
            session_marker: None,
            session_transition_epoch: None,
            active_session: None,
            target_generation: TargetGeneration(1),
            topology_digest: None,
            layout_generation: LayoutGeneration(1),
            accessibility_generation: AccessibilityGeneration(0),
            active_input: false,
            pending_input: None,
            closed: false,
        };
        let marker = [1; 32];
        let first = update_backend_generations(
            &mut state,
            marker,
            0,
            ActiveSessionStatus::Active,
            Some([2; 32]),
        );
        let after_unobserved_round_trip = update_backend_generations(
            &mut state,
            marker,
            2,
            ActiveSessionStatus::Active,
            Some([2; 32]),
        );
        assert_eq!(first.0, TargetGeneration(1));
        assert_eq!(after_unobserved_round_trip.0, TargetGeneration(2));
    }

    #[test]
    fn native_input_reservation_is_atomic_within_backend_state() {
        let mut state = BackendState {
            session_marker: None,
            session_transition_epoch: None,
            active_session: None,
            target_generation: TargetGeneration(1),
            topology_digest: None,
            layout_generation: LayoutGeneration(1),
            accessibility_generation: AccessibilityGeneration(0),
            active_input: false,
            pending_input: None,
            closed: false,
        };
        assert!(reserve_input(&mut state).is_ok());
        let error = reserve_input(&mut state)
            .expect_err("a second direct action must not overlap active native input");
        assert_eq!(error.code, ComputerUseErrorCode::InputCleanupFailed);
    }

    #[test]
    fn input_guard_finish_releases_complete_cleanup_ownership() {
        let backend = MacosDesktopBackend::new(&ComputerUsePolicy::default());
        let mut guard =
            InputExecutionGuard::new(&backend.state, NativeInput::with_forced_cleanup(true))
                .expect("first guard reserves native input");
        assert!(
            backend
                .state
                .lock()
                .expect("backend state is available")
                .active_input
        );

        let (cleanup, _event_count, retention_error) = guard.finish();
        assert!(cleanup.complete);
        assert!(retention_error.is_none());
        let state = backend.state.lock().expect("backend state is available");
        assert!(!state.active_input);
        assert!(state.pending_input.is_none());
        drop(state);
    }

    #[test]
    fn input_guard_finish_retains_incomplete_cleanup_and_blocks_reentry() {
        let backend = MacosDesktopBackend::new(&ComputerUsePolicy::default());
        let mut guard =
            InputExecutionGuard::new(&backend.state, NativeInput::with_forced_cleanup(false))
                .expect("first guard reserves native input");

        let (cleanup, _event_count, retention_error) = guard.finish();
        assert!(!cleanup.complete);
        assert!(retention_error.is_none());
        {
            let state = backend.state.lock().expect("backend state is available");
            assert!(!state.active_input);
            assert!(state.pending_input.is_some());
            drop(state);
        }
        let error =
            InputExecutionGuard::new(&backend.state, NativeInput::with_forced_cleanup(true))
                .err()
                .expect("pending cleanup must block another direct action");
        assert_eq!(error.code, ComputerUseErrorCode::InputCleanupFailed);
    }

    #[test]
    fn input_guard_drop_releases_or_retains_ownership_by_cleanup_result() {
        let complete_backend = MacosDesktopBackend::new(&ComputerUsePolicy::default());
        {
            let _guard = InputExecutionGuard::new(
                &complete_backend.state,
                NativeInput::with_forced_cleanup(true),
            )
            .expect("complete guard reserves native input");
        }
        {
            let state = complete_backend
                .state
                .lock()
                .expect("backend state is available");
            assert!(!state.active_input);
            assert!(state.pending_input.is_none());
            drop(state);
        }

        let incomplete_backend = MacosDesktopBackend::new(&ComputerUsePolicy::default());
        {
            let _guard = InputExecutionGuard::new(
                &incomplete_backend.state,
                NativeInput::with_forced_cleanup(false),
            )
            .expect("incomplete guard reserves native input");
        }
        let state = incomplete_backend
            .state
            .lock()
            .expect("backend state is available");
        assert!(!state.active_input);
        assert!(state.pending_input.is_some());
        drop(state);
    }

    #[tokio::test]
    async fn direct_close_rejects_an_active_native_action() {
        let backend = MacosDesktopBackend::new(&ComputerUsePolicy::default());
        {
            let mut state = backend.state.lock().expect("backend state is available");
            state.active_input = true;
        }
        let error = backend
            .close(CloseReason::HostShutdown)
            .await
            .expect_err("close must not report success over active input");
        assert_eq!(error.code, ComputerUseErrorCode::InputCleanupFailed);
    }

    #[test]
    fn capture_fence_discards_frame_after_topology_change() {
        let error = validate_capture_fence(fence([1; 32]), fence([2; 32]))
            .expect_err("changed topology must invalidate captured bytes");
        assert_eq!(error.code, ComputerUseErrorCode::DisplayTopologyChanged);
        assert_eq!(error.retry, RetryClassification::AfterFreshObservation);
    }

    #[test]
    fn capture_fence_discards_frame_after_target_generation_change() {
        let before = fence([1; 32]);
        let mut after = before;
        after.target_generation = TargetGeneration(2);
        let error = validate_capture_fence(before, after)
            .expect_err("session generation change must invalidate captured bytes");
        assert_eq!(error.code, ComputerUseErrorCode::DisplayTopologyChanged);
        assert_eq!(error.retry, RetryClassification::AfterFreshObservation);
    }

    #[test]
    fn capture_fence_discards_frame_after_permission_revocation() {
        let before = fence([1; 32]);
        let mut after = before;
        after.capture_granted = false;
        let error = validate_capture_fence(before, after)
            .expect_err("permission revocation must invalidate captured bytes");
        assert_eq!(error.code, ComputerUseErrorCode::PermissionRequired);
        assert_eq!(error.retry, RetryClassification::AfterPermissionChange);
    }

    #[test]
    fn capture_fence_discards_frame_after_session_lock() {
        let before = fence([1; 32]);
        let mut after = before;
        after.active_session = ActiveSessionStatus::Locked;
        let error = validate_capture_fence(before, after)
            .expect_err("session lock must invalidate captured bytes");
        assert_eq!(error.code, ComputerUseErrorCode::SessionLocked);
        assert_eq!(error.retry, RetryClassification::AfterExplicitResume);
    }

    #[test]
    fn movement_sampling_is_bounded_and_reaches_the_exact_destination() {
        let start = NativePoint { x: 0.0, y: 10.0 };
        let end = NativePoint { x: 32.0, y: 42.0 };
        let samples = movement_samples(start, end, 32);
        assert_eq!(
            samples,
            [
                NativePoint { x: 16.0, y: 26.0 },
                NativePoint { x: 32.0, y: 42.0 },
            ]
        );

        let bounded = movement_samples(start, end, u32::MAX);
        assert_eq!(bounded.len(), 4_096);
        assert_eq!(bounded.last(), Some(&end));
    }

    #[test]
    fn drag_sampling_preserves_waypoints_and_is_smooth_for_duration() {
        let path = [
            NativePoint { x: 0.0, y: 0.0 },
            NativePoint { x: 10.0, y: 0.0 },
            NativePoint { x: 10.0, y: 10.0 },
        ];
        let samples = drag_samples(&path, 64);
        assert_eq!(
            samples,
            [
                NativePoint { x: 5.0, y: 0.0 },
                NativePoint { x: 10.0, y: 0.0 },
                NativePoint { x: 10.0, y: 5.0 },
                NativePoint { x: 10.0, y: 10.0 },
            ]
        );
    }

    #[test]
    fn drag_sampling_respects_the_absolute_event_bound() {
        let path = vec![NativePoint { x: 1.0, y: 1.0 }; 4_096];
        let samples = drag_samples(&path, u32::MAX);
        assert_eq!(samples.len(), 4_096);
        assert_eq!(samples.last(), path.last());
    }

    #[test]
    fn malformed_direct_drag_is_rejected_before_sampling() {
        let empty = ComputerAction::Drag(DragAction {
            path: Vec::new(),
            button: PointerButton::Left,
            duration_ms: 1,
            modifiers: Vec::new(),
        });
        let error = validate_native_action_shape(&empty)
            .expect_err("direct backend drag without a path must not reach indexing");
        assert_eq!(error.code, ComputerUseErrorCode::InvalidRequest);

        let oversized = ComputerAction::Drag(DragAction {
            path: vec![ModelPoint { x: 0, y: 0 }; 4_097],
            button: PointerButton::Left,
            duration_ms: u32::MAX,
            modifiers: Vec::new(),
        });
        let error = validate_native_action_shape(&oversized)
            .expect_err("direct backend drag must obey the defensive native bound");
        assert_eq!(error.code, ComputerUseErrorCode::InvalidRequest);
    }

    #[test]
    fn transform_rejects_display_gaps_and_preserves_native_offset() {
        let geometry = gapped_geometry();
        let native = transform_point(&geometry, ModelPoint { x: 220, y: 20 })
            .expect("point on the second display must transform");
        assert_eq!(native, NativePoint { x: 170.0, y: 30.0 });

        let error = transform_point(&geometry, ModelPoint { x: 150, y: 20 })
            .expect_err("point in the composite display gap must be rejected");
        assert_eq!(error.code, ComputerUseErrorCode::InvalidCoordinate);
    }

    #[test]
    fn model_scroll_signs_are_converted_to_core_graphics_wheel_signs() {
        assert_eq!(core_graphics_scroll_deltas(12, 34), (-12, -34));
        assert_eq!(
            core_graphics_scroll_deltas(i32::MIN, i32::MIN),
            (i32::MAX, i32::MAX)
        );
    }

    #[test]
    fn chord_order_stably_moves_modifiers_before_ordinary_keys() {
        let caller_order = [
            CanonicalKey::A,
            CanonicalKey::Meta,
            CanonicalKey::B,
            CanonicalKey::Shift,
            CanonicalKey::Control,
            CanonicalKey::C,
        ];
        let press_order = key_press_order(&caller_order, crate::KeyMode::Chord);
        assert_eq!(
            press_order,
            [
                CanonicalKey::Meta,
                CanonicalKey::Shift,
                CanonicalKey::Control,
                CanonicalKey::A,
                CanonicalKey::B,
                CanonicalKey::C,
            ]
        );
        assert_eq!(
            press_order.iter().rev().copied().collect::<Vec<_>>(),
            [
                CanonicalKey::C,
                CanonicalKey::B,
                CanonicalKey::A,
                CanonicalKey::Control,
                CanonicalKey::Shift,
                CanonicalKey::Meta,
            ]
        );
        assert_eq!(
            key_press_order(&caller_order, crate::KeyMode::Sequence),
            caller_order
        );
    }

    #[test]
    fn text_input_pacing_is_bounded_and_preserves_the_normal_delay() {
        assert_eq!(text_input_part_delay(1), std::time::Duration::ZERO);
        assert_eq!(
            text_input_part_delay(2),
            std::time::Duration::from_millis(20)
        );
        assert_eq!(
            text_input_part_delay(251),
            std::time::Duration::from_millis(20)
        );
        assert_eq!(
            text_input_part_delay(401),
            std::time::Duration::from_millis(12)
        );
        assert_eq!(
            text_input_part_delay(8_193),
            std::time::Duration::from_millis(1)
        );
    }

    #[test]
    fn cleanup_status_reflects_required_release_attempts() {
        use super::macos_input::CleanupResult;

        assert_eq!(
            cleanup_status(CleanupResult {
                event_count: 0,
                required: false,
                complete: true,
            }),
            InputCleanupStatus::NotRequired
        );
        assert_eq!(
            cleanup_status(CleanupResult {
                event_count: 1,
                required: true,
                complete: true,
            }),
            InputCleanupStatus::Complete
        );
        assert_eq!(
            cleanup_status(CleanupResult {
                event_count: 1,
                required: true,
                complete: false,
            }),
            InputCleanupStatus::BestEffort
        );
        assert_eq!(
            cleanup_status(CleanupResult {
                event_count: 0,
                required: true,
                complete: false,
            }),
            InputCleanupStatus::Failed
        );
    }

    fn gapped_geometry() -> GeometrySnapshot {
        let model_to_native =
            AffineTransform2D::checked([1.0, 0.0, -50.0, 0.0, 1.0, 10.0, 0.0, 0.0, 1.0])
                .expect("fixture transform is valid");
        GeometrySnapshot {
            target_generation: TargetGeneration(1),
            layout_generation: LayoutGeneration(1),
            model_size_px: PixelSize {
                width: 300,
                height: 100,
            },
            native_desktop_rect: NativeRect {
                x: -50.0,
                y: 10.0,
                width: 300.0,
                height: 100.0,
            },
            model_to_native,
            native_to_model: model_to_native.inverse().expect("fixture inverse is valid"),
            displays: vec![
                DisplayGeometry {
                    model_rect: ModelRect {
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 100,
                    },
                    native_rect: NativeRect {
                        x: -50.0,
                        y: 10.0,
                        width: 100.0,
                        height: 100.0,
                    },
                    scale_factor: 1.0,
                    rotation_degrees: 0,
                    primary: true,
                },
                DisplayGeometry {
                    model_rect: ModelRect {
                        x: 200,
                        y: 0,
                        width: 100,
                        height: 100,
                    },
                    native_rect: NativeRect {
                        x: 150.0,
                        y: 10.0,
                        width: 100.0,
                        height: 100.0,
                    },
                    scale_factor: 1.0,
                    rotation_degrees: 0,
                    primary: false,
                },
            ],
            cursor_embedded: false,
        }
    }
}
