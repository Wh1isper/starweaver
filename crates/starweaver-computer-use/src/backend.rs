use async_trait::async_trait;
use starweaver_core::CancellationToken;

use crate::{
    AccessibilitySnapshot, CloseReason, ComputerAction, ComputerUseError, DesktopImageMime,
    EffectStatus, EffectiveComputerCapabilities, FrameRedactionStatus, GeometrySnapshot,
    InputCleanupStatus, NativeBackendKind, NativeDesktopPlatform, NativePoint, PermissionReport,
    PermissionRequest, StabilityCheckStatus, TargetGeneration, UserPresenceStatus,
};

#[derive(Clone, Debug)]
pub struct BackendProbe {
    pub platform: NativeDesktopPlatform,
    pub backend: NativeBackendKind,
    pub permissions: PermissionReport,
    pub capabilities: EffectiveComputerCapabilities,
    pub target_generation: TargetGeneration,
    pub user_presence: UserPresenceStatus,
    pub diagnostics_code: String,
}

#[derive(Clone, Debug)]
pub struct NativeObservation {
    pub geometry: GeometrySnapshot,
    pub mime_type: DesktopImageMime,
    pub image_bytes: Vec<u8>,
    pub color_space: Option<String>,
    pub redaction: FrameRedactionStatus,
    pub accessibility: Option<AccessibilitySnapshot>,
    /// A passive probe captured by the same backend fence as this observation.
    ///
    /// Backends that can cheaply provide this should use it to refresh volatile
    /// permissions without requiring the caller to request richer content.
    pub post_capture_probe: Option<BackendProbe>,
}

#[derive(Clone, Debug)]
pub struct NativeActionReceipt {
    pub effect_status: EffectStatus,
    pub native_event_count: u32,
    pub transformed_points: Vec<NativePoint>,
    pub cleanup: InputCleanupStatus,
    pub stability_check: StabilityCheckStatus,
}

#[derive(Clone, Debug)]
pub struct NativeActionFailure {
    pub error: ComputerUseError,
    pub effect_status: EffectStatus,
    pub receipt: Option<NativeActionReceipt>,
    pub cleanup: InputCleanupStatus,
}

#[async_trait]
pub trait NativeDesktopBackend: Send + Sync {
    fn platform(&self) -> NativeDesktopPlatform;

    fn kind(&self) -> NativeBackendKind;

    async fn probe(&self, cancel: CancellationToken) -> Result<BackendProbe, ComputerUseError>;

    /// Request attended OS permissions and return the immediate authoritative probe.
    ///
    /// Prompt presentation is not evidence that a permission was granted. The
    /// returned probe always reflects the state observed immediately after the
    /// native request APIs return.
    async fn request_permissions(
        &self,
        _request: PermissionRequest,
        cancel: CancellationToken,
    ) -> Result<BackendProbe, ComputerUseError> {
        self.probe(cancel).await
    }

    async fn open(&self, cancel: CancellationToken) -> Result<BackendProbe, ComputerUseError>;

    async fn observe(
        &self,
        include_accessibility: bool,
        cancel: CancellationToken,
    ) -> Result<NativeObservation, ComputerUseError>;

    async fn execute(
        &self,
        action: &ComputerAction,
        geometry: &GeometrySnapshot,
        cancel: CancellationToken,
    ) -> Result<NativeActionReceipt, NativeActionFailure>;

    async fn close(&self, reason: CloseReason) -> Result<InputCleanupStatus, ComputerUseError>;
}

pub type DynNativeDesktopBackend = std::sync::Arc<dyn NativeDesktopBackend>;
