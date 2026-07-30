use std::sync::Arc;

use crate::{
    ComputerToolGrant, ComputerUsePolicy, DynComputerUseService, DynNativeDesktopBackend,
    LocalComputerUseService,
};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod unsupported;

/// Construct the target-gated backend for the current process platform.
#[must_use]
pub fn current_desktop_backend(policy: &ComputerUsePolicy) -> DynNativeDesktopBackend {
    #[cfg(target_os = "macos")]
    {
        Arc::new(macos::MacosDesktopBackend::new(policy.desktop_scope))
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

/// Intersect an external-harness launch grant with release-ready native support.
///
/// The current macOS release backend is observe-only until its production
/// user-presence guard and input delivery gates are complete. Other platforms
/// remain unsupported.
#[must_use]
pub const fn current_desktop_tool_grant(requested: ComputerToolGrant) -> ComputerToolGrant {
    #[cfg(target_os = "macos")]
    {
        ComputerToolGrant {
            observe: requested.observe,
            pointer: false,
            keyboard: false,
        }
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
