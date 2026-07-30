use std::{io::Cursor, sync::Arc};

use async_trait::async_trait;
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use starweaver_core::CancellationToken;
use tokio::sync::Mutex;

use crate::{
    AccessibilityGeneration, AccessibilityNode, AccessibilitySnapshot, AccessibilityState,
    AffineTransform2D, BackendProbe, CloseReason, ComputerAction, ComputerUseError,
    ComputerUsePolicy, ComputerUseService, DesktopImageMime, DisplayGeometry, DynComputerSession,
    EffectStatus, EffectiveComputerCapabilities, FrameRedactionStatus, GeometrySnapshot,
    InputCleanupStatus, LayoutGeneration, LocalComputerUseService, ModelPoint, ModelRect,
    NativeActionFailure, NativeActionReceipt, NativeBackendKind, NativeDesktopBackend,
    NativeDesktopPlatform, NativeObservation, NativeRect, PermissionCapabilityStatus,
    PermissionReport, PermissionRequest, PermissionRequestOutcome, PixelSize, StabilityCheckStatus,
    TargetGeneration, UserPresenceStatus,
};

#[derive(Clone, Debug)]
pub struct FakeComputerUseConfig {
    pub size: PixelSize,
    pub rgba: [u8; 4],
    pub capabilities: EffectiveComputerCapabilities,
    pub close_cleanup: InputCleanupStatus,
}

impl Default for FakeComputerUseConfig {
    fn default() -> Self {
        Self {
            size: PixelSize {
                width: 320,
                height: 200,
            },
            rgba: [32, 64, 96, 255],
            capabilities: EffectiveComputerCapabilities {
                observe: true,
                pointer: true,
                keyboard: true,
                accessibility_snapshot: false,
            },
            close_cleanup: InputCleanupStatus::Complete,
        }
    }
}

struct FakeState {
    config: FakeComputerUseConfig,
    target_generation: TargetGeneration,
    layout_generation: LayoutGeneration,
    accessibility_generation: AccessibilityGeneration,
    actions: Vec<ComputerAction>,
    next_observe_error: Option<ComputerUseError>,
    next_action_failure: Option<NativeActionFailure>,
    closed: bool,
}

pub struct FakeNativeDesktopBackend {
    state: Mutex<FakeState>,
}

impl FakeNativeDesktopBackend {
    #[must_use]
    pub fn new(config: FakeComputerUseConfig) -> Self {
        Self {
            state: Mutex::new(FakeState {
                config,
                target_generation: TargetGeneration(1),
                layout_generation: LayoutGeneration(1),
                accessibility_generation: AccessibilityGeneration(0),
                actions: Vec::new(),
                next_observe_error: None,
                next_action_failure: None,
                closed: false,
            }),
        }
    }

    pub async fn recorded_actions(&self) -> Vec<ComputerAction> {
        self.state.lock().await.actions.clone()
    }

    pub async fn set_frame_color(&self, rgba: [u8; 4]) {
        self.state.lock().await.config.rgba = rgba;
    }

    pub async fn set_capabilities(&self, capabilities: EffectiveComputerCapabilities) {
        self.state.lock().await.config.capabilities = capabilities;
    }

    pub async fn change_layout(&self, size: PixelSize) {
        let mut state = self.state.lock().await;
        state.config.size = size;
        state.layout_generation.0 = state.layout_generation.0.saturating_add(1);
    }

    pub async fn change_target(&self) {
        let mut state = self.state.lock().await;
        state.target_generation.0 = state.target_generation.0.saturating_add(1);
        state.layout_generation.0 = state.layout_generation.0.saturating_add(1);
    }

    pub async fn fail_next_observe(&self, error: ComputerUseError) {
        self.state.lock().await.next_observe_error = Some(error);
    }

    pub async fn fail_next_action(&self, failure: NativeActionFailure) {
        self.state.lock().await.next_action_failure = Some(failure);
    }

    fn probe_from_state(state: &FakeState) -> BackendProbe {
        let granted = PermissionCapabilityStatus::Granted;
        BackendProbe {
            platform: NativeDesktopPlatform::Unsupported,
            backend: NativeBackendKind::Fake,
            permissions: PermissionReport {
                platform: NativeDesktopPlatform::Unsupported,
                backend: NativeBackendKind::Fake,
                active_session: crate::ActiveSessionStatus::Active,
                capture: granted,
                pointer_input: granted,
                keyboard_input: granted,
                accessibility: if state.config.capabilities.accessibility_snapshot {
                    granted
                } else {
                    PermissionCapabilityStatus::Unavailable
                },
                user_presence: granted,
                restart_required: false,
                remediation: Vec::new(),
                diagnostics_code: "fake_ready".into(),
            },
            capabilities: state.config.capabilities,
            target_generation: state.target_generation,
            user_presence: UserPresenceStatus::Armed,
            diagnostics_code: "fake_ready".into(),
        }
    }
}

#[allow(clippy::significant_drop_tightening)]
#[async_trait]
impl NativeDesktopBackend for FakeNativeDesktopBackend {
    fn platform(&self) -> NativeDesktopPlatform {
        NativeDesktopPlatform::Unsupported
    }

    fn kind(&self) -> NativeBackendKind {
        NativeBackendKind::Fake
    }

    async fn probe(&self, cancel: CancellationToken) -> Result<BackendProbe, ComputerUseError> {
        if cancel.is_cancelled() {
            return Err(ComputerUseError::cancelled());
        }
        let state = self.state.lock().await;
        if state.closed {
            return Err(ComputerUseError::new(
                crate::ComputerUseErrorCode::SessionClosed,
                "fake desktop is closed",
                crate::RetryClassification::NewSessionRequired,
            ));
        }
        Ok(Self::probe_from_state(&state))
    }

    async fn open(&self, cancel: CancellationToken) -> Result<BackendProbe, ComputerUseError> {
        self.probe(cancel).await
    }

    async fn observe(
        &self,
        include_accessibility: bool,
        cancel: CancellationToken,
    ) -> Result<NativeObservation, ComputerUseError> {
        if cancel.is_cancelled() {
            return Err(ComputerUseError::cancelled());
        }
        let mut state = self.state.lock().await;
        if let Some(error) = state.next_observe_error.take() {
            return Err(error);
        }
        if include_accessibility && !state.config.capabilities.accessibility_snapshot {
            return Err(ComputerUseError::new(
                crate::ComputerUseErrorCode::UnsupportedCapability,
                "fake accessibility is disabled",
                crate::RetryClassification::Never,
            ));
        }
        let image = RgbaImage::from_pixel(
            state.config.size.width,
            state.config.size.height,
            Rgba(state.config.rgba),
        );
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut encoded, ImageFormat::Png)
            .map_err(|_| {
                ComputerUseError::new(
                    crate::ComputerUseErrorCode::Internal,
                    "failed to encode deterministic fake frame",
                    crate::RetryClassification::Never,
                )
            })?;
        let size = state.config.size;
        let geometry = fake_geometry(size, state.target_generation, state.layout_generation);
        let accessibility = if include_accessibility {
            state.accessibility_generation.0 = state.accessibility_generation.0.saturating_add(1);
            Some(AccessibilitySnapshot {
                generation: state.accessibility_generation,
                captured_at_monotonic_ms: 0,
                nodes: vec![AccessibilityNode {
                    local_id: 1,
                    parent_local_id: None,
                    role: "AXApplication".into(),
                    name: Some("Deterministic fake application".into()),
                    value_summary: None,
                    state: AccessibilityState {
                        enabled: Some(true),
                        focused: Some(true),
                        selected: None,
                        protected: Some(false),
                    },
                    model_bounds: Some(ModelRect {
                        x: 0,
                        y: 0,
                        width: size.width,
                        height: size.height,
                    }),
                }],
                truncated: false,
                truncation_reasons: Vec::new(),
            })
        } else {
            None
        };
        let post_capture_probe = Some(Self::probe_from_state(&state));
        Ok(NativeObservation {
            geometry,
            mime_type: DesktopImageMime::ImagePng,
            image_bytes: encoded.into_inner(),
            color_space: Some("srgb".into()),
            redaction: FrameRedactionStatus::Complete,
            accessibility,
            post_capture_probe,
        })
    }

    async fn execute(
        &self,
        action: &ComputerAction,
        geometry: &GeometrySnapshot,
        cancel: CancellationToken,
    ) -> Result<NativeActionReceipt, NativeActionFailure> {
        if cancel.is_cancelled() {
            return Err(NativeActionFailure {
                error: ComputerUseError::cancelled(),
                effect_status: EffectStatus::NotExecuted,
                receipt: None,
                cleanup: InputCleanupStatus::NotRequired,
            });
        }
        let mut state = self.state.lock().await;
        if let Some(failure) = state.next_action_failure.take() {
            return Err(failure);
        }
        if geometry.target_generation != state.target_generation
            || geometry.layout_generation != state.layout_generation
        {
            return Err(NativeActionFailure {
                error: ComputerUseError::new(
                    crate::ComputerUseErrorCode::DisplayTopologyChanged,
                    "fake geometry changed before input",
                    crate::RetryClassification::AfterFreshObservation,
                ),
                effect_status: EffectStatus::NotExecuted,
                receipt: None,
                cleanup: InputCleanupStatus::NotRequired,
            });
        }
        let points = action_points(action)
            .into_iter()
            .map(|point| geometry.model_to_native.apply(point))
            .collect();
        let event_count = native_event_count(action);
        state.actions.push(action.clone());
        Ok(NativeActionReceipt {
            effect_status: EffectStatus::Executed,
            native_event_count: event_count,
            transformed_points: points,
            cleanup: InputCleanupStatus::Complete,
            stability_check: StabilityCheckStatus::NotPerformed,
        })
    }

    async fn close(&self, _reason: CloseReason) -> Result<InputCleanupStatus, ComputerUseError> {
        let mut state = self.state.lock().await;
        state.closed = true;
        Ok(state.config.close_cleanup)
    }
}

pub struct FakeComputerUseService {
    inner: Arc<LocalComputerUseService>,
    backend: Arc<FakeNativeDesktopBackend>,
}

impl FakeComputerUseService {
    #[must_use]
    pub fn new(policy: ComputerUsePolicy, config: FakeComputerUseConfig) -> Self {
        let backend = Arc::new(FakeNativeDesktopBackend::new(config));
        let inner = Arc::new(LocalComputerUseService::new(policy, backend.clone()));
        Self { inner, backend }
    }

    #[must_use]
    pub fn backend(&self) -> Arc<FakeNativeDesktopBackend> {
        self.backend.clone()
    }
}

#[async_trait]
impl ComputerUseService for FakeComputerUseService {
    fn contract_version(&self) -> crate::ComputerUseContractVersion {
        self.inner.contract_version()
    }

    fn process_instance_id(&self) -> crate::ProcessInstanceId {
        self.inner.process_instance_id()
    }

    fn policy(&self) -> &ComputerUsePolicy {
        self.inner.policy()
    }

    async fn status(
        &self,
        cancel: CancellationToken,
    ) -> Result<crate::ComputerStatus, ComputerUseError> {
        self.inner.status(cancel).await
    }

    async fn request_permissions(
        &self,
        request: PermissionRequest,
        cancel: CancellationToken,
    ) -> Result<PermissionRequestOutcome, ComputerUseError> {
        self.inner.request_permissions(request, cancel).await
    }

    async fn status_with_queue_deadline(
        &self,
        cancel: CancellationToken,
        queue_deadline: tokio::time::Instant,
    ) -> Result<crate::ComputerStatus, ComputerUseError> {
        self.inner
            .status_with_queue_deadline(cancel, queue_deadline)
            .await
    }

    async fn open_current_desktop(
        &self,
        cancel: CancellationToken,
    ) -> Result<DynComputerSession, ComputerUseError> {
        self.inner.open_current_desktop(cancel).await
    }

    async fn open_current_desktop_with_queue_deadline(
        &self,
        cancel: CancellationToken,
        queue_deadline: tokio::time::Instant,
    ) -> Result<DynComputerSession, ComputerUseError> {
        self.inner
            .open_current_desktop_with_queue_deadline(cancel, queue_deadline)
            .await
    }

    async fn shutdown(
        &self,
        reason: CloseReason,
    ) -> Result<crate::ShutdownReceipt, ComputerUseError> {
        self.inner.shutdown(reason).await
    }
}

fn fake_geometry(
    size: PixelSize,
    target: TargetGeneration,
    layout: LayoutGeneration,
) -> GeometrySnapshot {
    let native_rect = NativeRect {
        x: 0.0,
        y: 0.0,
        width: f64::from(size.width),
        height: f64::from(size.height),
    };
    GeometrySnapshot {
        target_generation: target,
        layout_generation: layout,
        model_size_px: size,
        native_desktop_rect: native_rect,
        model_to_native: AffineTransform2D::IDENTITY,
        native_to_model: AffineTransform2D::IDENTITY,
        displays: vec![DisplayGeometry {
            model_rect: ModelRect {
                x: 0,
                y: 0,
                width: size.width,
                height: size.height,
            },
            native_rect,
            scale_factor: 1.0,
            rotation_degrees: 0,
            primary: true,
        }],
        cursor_embedded: false,
    }
}

fn action_points(action: &ComputerAction) -> Vec<ModelPoint> {
    match action {
        ComputerAction::Click(value) => vec![value.point],
        ComputerAction::MovePointer(value) => vec![value.point],
        ComputerAction::Drag(value) => value.path.clone(),
        ComputerAction::Scroll(value) => vec![value.anchor],
        ComputerAction::TypeText(_) | ComputerAction::PressKeys(_) => Vec::new(),
    }
}

fn native_event_count(action: &ComputerAction) -> u32 {
    match action {
        ComputerAction::Click(value) => u32::from(value.click_count).saturating_mul(2),
        ComputerAction::MovePointer(_) | ComputerAction::Scroll(_) => 1,
        ComputerAction::Drag(value) => u32::try_from(value.path.len()).unwrap_or(u32::MAX) + 2,
        ComputerAction::TypeText(value) => u32::try_from(value.text.chars().count())
            .unwrap_or(u32::MAX)
            .saturating_mul(2),
        ComputerAction::PressKeys(value) => u32::try_from(value.keys.len())
            .unwrap_or(u32::MAX)
            .saturating_mul(2),
    }
}
