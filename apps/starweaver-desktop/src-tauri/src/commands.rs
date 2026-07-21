use serde::Serialize;
use tauri::{AppHandle, State, ipc::Channel};

use crate::{
    app_state::{DesktopActivation, DesktopState},
    platform::PlatformInfo,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RuntimeAvailability {
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RuntimeUnavailableReason {
    NotConfigured,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeStatus {
    state: RuntimeAvailability,
    reason: RuntimeUnavailableReason,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopStatus {
    app_version: String,
    platform: crate::platform::DesktopPlatform,
    architecture: String,
    launch_generation: u64,
    single_instance: bool,
    runtime: RuntimeStatus,
}

fn build_desktop_status(app_version: &str, state: &DesktopState) -> DesktopStatus {
    let platform = PlatformInfo::current();
    DesktopStatus {
        app_version: app_version.to_string(),
        platform: platform.platform,
        architecture: platform.architecture,
        launch_generation: state.launch_generation(),
        single_instance: true,
        runtime: RuntimeStatus {
            state: RuntimeAvailability::Unavailable,
            reason: RuntimeUnavailableReason::NotConfigured,
        },
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn get_desktop_status(app: AppHandle, state: State<'_, DesktopState>) -> DesktopStatus {
    build_desktop_status(&app.package_info().version.to_string(), state.inner())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn subscribe_desktop_activation(
    state: State<'_, DesktopState>,
    on_activation: Channel<DesktopActivation>,
) -> u64 {
    state.subscribe_to_activations(on_activation)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn unsubscribe_desktop_activation(state: State<'_, DesktopState>, subscription_token: u64) {
    state.unsubscribe_from_activations(subscription_token);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_safe_and_reports_unconfigured_runtime() {
        let state = DesktopState::default();
        let status = build_desktop_status("0.9.0", &state);

        assert_eq!(status.app_version, "0.9.0");
        assert_eq!(status.launch_generation, 1);
        assert!(status.single_instance);
        assert_eq!(status.runtime.state, RuntimeAvailability::Unavailable);
        assert_eq!(
            status.runtime.reason,
            RuntimeUnavailableReason::NotConfigured
        );
    }
}
