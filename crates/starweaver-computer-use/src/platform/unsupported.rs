use async_trait::async_trait;
use starweaver_core::CancellationToken;

use crate::{
    BackendProbe, CloseReason, ComputerAction, ComputerUseError, EffectStatus, InputCleanupStatus,
    NativeActionFailure, NativeActionReceipt, NativeBackendKind, NativeDesktopBackend,
    NativeDesktopPlatform, NativeObservation, PermissionReport,
};

pub struct UnsupportedDesktopBackend;

#[async_trait]
impl NativeDesktopBackend for UnsupportedDesktopBackend {
    fn platform(&self) -> NativeDesktopPlatform {
        if cfg!(target_os = "windows") {
            NativeDesktopPlatform::Windows
        } else if cfg!(target_os = "linux") {
            NativeDesktopPlatform::Linux
        } else {
            NativeDesktopPlatform::Unsupported
        }
    }

    fn kind(&self) -> NativeBackendKind {
        NativeBackendKind::Unsupported
    }

    async fn probe(&self, cancel: CancellationToken) -> Result<BackendProbe, ComputerUseError> {
        if cancel.is_cancelled() {
            return Err(ComputerUseError::cancelled());
        }
        let platform = self.platform();
        Ok(BackendProbe {
            platform,
            backend: NativeBackendKind::Unsupported,
            permissions: PermissionReport::unsupported(platform),
            capabilities: crate::EffectiveComputerCapabilities::default(),
            target_generation: crate::TargetGeneration(0),
            user_presence: crate::UserPresenceStatus::Unavailable,
            diagnostics_code: "unsupported_platform".into(),
        })
    }

    async fn open(&self, _cancel: CancellationToken) -> Result<BackendProbe, ComputerUseError> {
        Err(ComputerUseError::unsupported_platform())
    }

    async fn observe(
        &self,
        _include_accessibility: bool,
        _cancel: CancellationToken,
    ) -> Result<NativeObservation, ComputerUseError> {
        Err(ComputerUseError::unsupported_platform())
    }

    async fn execute(
        &self,
        _action: &ComputerAction,
        _geometry: &crate::GeometrySnapshot,
        _cancel: CancellationToken,
    ) -> Result<NativeActionReceipt, NativeActionFailure> {
        Err(NativeActionFailure {
            error: ComputerUseError::unsupported_platform(),
            effect_status: EffectStatus::NotExecuted,
            receipt: None,
            cleanup: InputCleanupStatus::NotRequired,
        })
    }

    async fn close(&self, _reason: CloseReason) -> Result<InputCleanupStatus, ComputerUseError> {
        Ok(InputCleanupStatus::NotRequired)
    }
}
