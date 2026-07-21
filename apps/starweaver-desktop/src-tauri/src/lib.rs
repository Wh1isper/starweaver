//! Privileged Starweaver Desktop shell and process-owned application state.

mod app_state;
mod commands;
mod platform;
mod single_instance;

use app_state::DesktopState;

/// Runs the native Desktop application until its event loop exits.
///
/// # Errors
///
/// Returns a Tauri error when setup or the native event loop cannot start or complete.
pub fn run() -> tauri::Result<()> {
    tauri::Builder::default()
        // The single-instance plugin must remain the first registered plugin.
        .plugin(single_instance::plugin())
        .manage(DesktopState::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_desktop_status,
            commands::subscribe_desktop_activation,
            commands::unsubscribe_desktop_activation,
        ])
        .run(tauri::generate_context!())
}
