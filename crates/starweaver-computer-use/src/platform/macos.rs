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
use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};
use sha2::{Digest, Sha256};
use starweaver_core::CancellationToken;
use xcap::Monitor;

use crate::{
    AccessibilityGeneration, AccessibilityPolicy, ActiveSessionStatus, AffineTransform2D,
    BackendProbe, CloseReason, ComputerAction, ComputerUseError, ComputerUseErrorCode,
    ComputerUsePolicy, DesktopImageMime, DesktopSurfaceScope, DisplayGeometry, EffectStatus,
    EffectiveComputerCapabilities, FrameRedactionStatus, GeometrySnapshot, InputCleanupStatus,
    LayoutGeneration, ModelRect, NativeActionFailure, NativeActionReceipt, NativeBackendKind,
    NativeDesktopBackend, NativeDesktopPlatform, NativeObservation, NativeRect,
    PermissionCapabilityStatus, PermissionPromptPolicy, PermissionReport, PermissionRequest,
    PixelSize, RetryClassification, TargetGeneration, UserPresenceStatus,
};

use super::macos_accessibility;

struct BackendState {
    topology_digest: Option<[u8; 32]>,
    layout_generation: LayoutGeneration,
    accessibility_generation: AccessibilityGeneration,
    closed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CaptureFence {
    active_session: ActiveSessionStatus,
    capture_granted: bool,
    topology_digest: Option<[u8; 32]>,
    layout_generation: LayoutGeneration,
}

pub struct MacosDesktopBackend {
    scope: DesktopSurfaceScope,
    accessibility_policy: AccessibilityPolicy,
    permission_prompts: PermissionPromptPolicy,
    capture_prompt_attempted: AtomicBool,
    accessibility_prompt_attempted: AtomicBool,
    started_at: Instant,
    state: Mutex<BackendState>,
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
            started_at: Instant::now(),
            state: Mutex::new(BackendState {
                topology_digest: None,
                layout_generation: LayoutGeneration(1),
                accessibility_generation: AccessibilityGeneration(0),
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
        let accessibility_granted = macos_accessibility::is_trusted();
        let (monitors, layout_generation, current_topology_digest) =
            if active_session == ActiveSessionStatus::Active {
                let monitors = tokio::task::spawn_blocking(Monitor::all)
                    .await
                    .map_err(|_| backend_error("macOS display probe task failed"))?
                    .map_err(|_| backend_error("macOS display topology is unavailable"))?;
                if monitors.is_empty() {
                    return Err(backend_error("macOS reported no active display"));
                }
                let topology_digest = topology_digest(&monitors)?;
                let layout_generation = {
                    let mut state = self
                        .state
                        .lock()
                        .map_err(|_| backend_error("macOS backend state is unavailable"))?;
                    ensure_backend_open(&state)?;
                    if state
                        .topology_digest
                        .is_some_and(|value| value != topology_digest)
                    {
                        state.layout_generation.0 = state.layout_generation.0.saturating_add(1);
                    }
                    state.topology_digest = Some(topology_digest);
                    state.layout_generation
                };
                (monitors, layout_generation, Some(topology_digest))
            } else {
                let state = self
                    .state
                    .lock()
                    .map_err(|_| backend_error("macOS backend state is unavailable"))?;
                ensure_backend_open(&state)?;
                (Vec::new(), state.layout_generation, None)
            };
        let capture = if capture_granted {
            PermissionCapabilityStatus::Granted
        } else {
            PermissionCapabilityStatus::Required
        };
        let capabilities = EffectiveComputerCapabilities {
            observe: capture_granted && active_session == ActiveSessionStatus::Active,
            // Input remains unavailable until a production same-process
            // presence indicator, takeover detector, emergency stop, and
            // locally initiated resume path are implemented and accepted.
            pointer: false,
            keyboard: false,
            accessibility_snapshot: accessibility_granted
                && active_session == ActiveSessionStatus::Active,
        };
        let diagnostics_code = if !console.owned_by_process_user {
            "macos_console_user_mismatch"
        } else if console.locked {
            "macos_session_locked"
        } else if !capture_granted {
            "macos_screen_recording_permission_required"
        } else if !accessibility_granted {
            "macos_observe_ready_accessibility_permission_required"
        } else {
            "macos_observe_accessibility_ready_input_presence_guard_unavailable"
        };
        let permissions = PermissionReport {
            platform: NativeDesktopPlatform::Macos,
            backend: NativeBackendKind::MacosCoreGraphics,
            active_session,
            capture,
            pointer_input: PermissionCapabilityStatus::Unavailable,
            keyboard_input: PermissionCapabilityStatus::Unavailable,
            accessibility: if accessibility_granted {
                PermissionCapabilityStatus::Granted
            } else {
                PermissionCapabilityStatus::Required
            },
            user_presence: PermissionCapabilityStatus::Unavailable,
            restart_required: false,
            remediation: permission_remediation(&console, capture_granted, accessibility_granted),
            diagnostics_code: diagnostics_code.into(),
        };
        Ok((
            BackendProbe {
                platform: NativeDesktopPlatform::Macos,
                backend: NativeBackendKind::MacosCoreGraphics,
                permissions,
                capabilities,
                target_generation: TargetGeneration(1),
                user_presence: UserPresenceStatus::Unavailable,
                diagnostics_code: diagnostics_code.into(),
            },
            monitors,
            CaptureFence {
                active_session,
                capture_granted,
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
        if request.accessibility && !macos_accessibility::is_trusted() {
            if cancel.is_cancelled() {
                return Err(ComputerUseError::cancelled());
            }
            // The Screen Recording request above can itself display UI or
            // block. Re-establish attended foreground-session evidence before
            // causing a second native permission side effect.
            let (accessibility_fence, _, _) = self.inspect().await?;
            if accessibility_fence.permissions.active_session != ActiveSessionStatus::Active {
                return Ok(accessibility_fence);
            }
            if cancel.is_cancelled() {
                return Err(ComputerUseError::cancelled());
            }
            self.accessibility_prompt_attempted
                .store(true, Ordering::Release);
            let _immediate_result = macos_accessibility::request_trust();
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
            && !probe.capabilities.accessibility_snapshot
            && self.permission_prompts.accessibility_on_observe
        {
            if cancel.is_cancelled() {
                return Err(ComputerUseError::cancelled());
            }
            if !self
                .accessibility_prompt_attempted
                .swap(true, Ordering::AcqRel)
            {
                let _immediate_result = macos_accessibility::request_trust();
            }
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
            capture_monitors(scope, monitors, capture_fence.layout_generation)
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
        _action: &ComputerAction,
        _geometry: &GeometrySnapshot,
        _cancel: CancellationToken,
    ) -> Result<NativeActionReceipt, NativeActionFailure> {
        Err(NativeActionFailure {
            error: ComputerUseError::new(
                ComputerUseErrorCode::UserPresenceRequired,
                "macOS input is disabled until an accepted production same-process UserPresenceGuard is available",
                RetryClassification::Never,
            ),
            effect_status: EffectStatus::NotExecuted,
            receipt: None,
            cleanup: InputCleanupStatus::NotRequired,
        })
    }

    async fn close(&self, _reason: CloseReason) -> Result<InputCleanupStatus, ComputerUseError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| backend_error("macOS backend state is unavailable"))?;
        state.closed = true;
        Ok(InputCleanupStatus::NotRequired)
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

struct ConsoleSession {
    locked: bool,
    owned_by_process_user: bool,
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
    Ok(ConsoleSession {
        locked,
        owned_by_process_user: console_uid == process_uid,
    })
}

fn permission_remediation(
    console: &ConsoleSession,
    capture_granted: bool,
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
    if !accessibility_granted {
        remediation.push("Grant Accessibility permission to this exact executable identity in System Settings, then retry the observation.".into());
    }
    remediation.push("Pointer and keyboard input are disabled because no accepted production macOS UserPresenceGuard is installed.".into());
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
            target_generation: TargetGeneration(1),
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
    use super::{CaptureFence, validate_capture_fence, validate_session_uids};
    use crate::{ActiveSessionStatus, ComputerUseErrorCode, LayoutGeneration, RetryClassification};

    const fn fence(digest: [u8; 32]) -> CaptureFence {
        CaptureFence {
            active_session: ActiveSessionStatus::Active,
            capture_granted: true,
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
    fn capture_fence_discards_frame_after_topology_change() {
        let error = validate_capture_fence(fence([1; 32]), fence([2; 32]))
            .expect_err("changed topology must invalidate captured bytes");
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
}
