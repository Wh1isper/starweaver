use tauri::{AppHandle, Manager, Runtime};

use crate::app_state::DesktopState;

const MAIN_WINDOW_LABEL: &str = "main";

type ActivationCallback<R> = dyn Fn(&AppHandle<R>) + Send + Sync + 'static;

#[cfg(target_os = "linux")]
#[path = "single_instance/linux.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "single_instance/macos.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "single_instance/windows.rs"]
mod platform;

pub fn plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    platform::init(Box::new(route_secondary_launch))
}

fn route_secondary_launch<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<DesktopState>();
    let activation = state.record_secondary_launch();

    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        if let Err(error) = window.show() {
            eprintln!("failed to show the primary desktop window: {error}");
        }
        if let Err(error) = window.unminimize() {
            eprintln!("failed to restore the primary desktop window: {error}");
        }
        if let Err(error) = window.set_focus() {
            eprintln!("failed to focus the primary desktop window: {error}");
        }
    }

    state.publish_activation(activation);
}
