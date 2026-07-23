//! Privileged Starweaver Desktop shell and process-owned application state.

mod app_state;
mod commands;
/// Generated renderer-safe host protocol bindings.
pub mod generated;
mod platform;
mod single_instance;
pub mod supervisor;

use app_state::DesktopState;
use tauri::Manager as _;

/// Runs the native Desktop application until its event loop exits.
///
/// # Errors
///
/// Returns a Tauri error when setup or the native event loop cannot start or complete.
pub fn run() -> tauri::Result<()> {
    let app = tauri::Builder::default()
        // The single-instance plugin must remain the first registered plugin.
        .plugin(single_instance::plugin())
        .manage(DesktopState::default())
        .setup(|app| {
            let root = app.path().app_local_data_dir()?.join("supervisor");
            std::fs::create_dir_all(&root)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
            }
            app.state::<DesktopState>()
                .configure_supervisor_storage(root)
                .map_err(|error| std::io::Error::other(error.message))?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_desktop_status,
            commands::subscribe_desktop_activation,
            commands::unsubscribe_desktop_activation,
            commands::execute_host_operation,
            commands::list_pending_host_operations,
            commands::acknowledge_host_operation,
            commands::acknowledge_host_event,
            commands::subscribe_host_events,
            commands::unsubscribe_host_events,
        ])
        .build(tauri::generate_context!())?;
    app.run(|app_handle, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            let state = app_handle.state::<DesktopState>();
            if state.exit_shutdown_completed() {
                return;
            }
            api.prevent_exit();
            if state.begin_exit_shutdown() {
                let app_handle = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    let state = app_handle.state::<DesktopState>();
                    let _ = state.supervisor().shutdown().await;
                    state.complete_exit_shutdown();
                    app_handle.exit(0);
                });
            }
        }
    });
    Ok(())
}
