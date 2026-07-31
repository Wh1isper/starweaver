use std::sync::Arc;

use crate::{
    ComputerToolGrant, ComputerUsePolicy, DynComputerUseService, DynNativeDesktopBackend,
    LocalComputerUseService,
};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_accessibility;
#[cfg(target_os = "macos")]
mod macos_input;
#[cfg(target_os = "macos")]
mod macos_session;
#[cfg(not(target_os = "macos"))]
mod unsupported;

/// Construct the target-gated backend for the current process platform.
#[must_use]
pub fn current_desktop_backend(policy: &ComputerUsePolicy) -> DynNativeDesktopBackend {
    #[cfg(target_os = "macos")]
    {
        Arc::new(macos::MacosDesktopBackend::new(policy))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = policy;
        Arc::new(unsupported::UnsupportedDesktopBackend)
    }
}

/// Construct a process-local service using the target-gated native backend.
#[must_use]
pub fn current_desktop_service(policy: ComputerUsePolicy) -> DynComputerUseService {
    let backend = current_desktop_backend(&policy);
    Arc::new(LocalComputerUseService::new(policy, backend))
}

/// Intersect an external-harness launch grant with native platform support.
///
/// macOS supports the complete observe, pointer, and keyboard product grant.
/// Other platforms remain unsupported.
#[must_use]
pub const fn current_desktop_tool_grant(requested: ComputerToolGrant) -> ComputerToolGrant {
    #[cfg(target_os = "macos")]
    {
        requested
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = requested;
        ComputerToolGrant {
            observe: false,
            pointer: false,
            keyboard: false,
        }
    }
}
