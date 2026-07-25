//! Privileged Starweaver Desktop shell and process-owned application state.

mod app_state;
mod commands;
/// Generated renderer-safe host protocol bindings.
pub mod generated;
mod managed_runtime;
mod platform;
mod preferences;
mod single_instance;
pub mod supervisor;

use app_state::DesktopState;
use preferences::{DesktopPreferencesStore, WindowCloseBehavior};
use tauri::Manager as _;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MainWindowCloseAction {
    Hide,
    RequestExit,
}

const fn main_window_close_action(behavior: WindowCloseBehavior) -> MainWindowCloseAction {
    match behavior {
        WindowCloseBehavior::KeepRunning => MainWindowCloseAction::Hide,
        WindowCloseBehavior::Quit => MainWindowCloseAction::RequestExit,
    }
}

fn begin_coordinated_exit(app_handle: tauri::AppHandle) {
    let state = app_handle.state::<DesktopState>();
    if state.exit_shutdown_completed() {
        app_handle.exit(0);
        return;
    }
    if state.begin_exit_shutdown() {
        tauri::async_runtime::spawn(async move {
            let state = app_handle.state::<DesktopState>();
            let _ = state.shutdown_managed_runtime().await;
            state.complete_exit_shutdown();
            app_handle.exit(0);
        });
    }
}

/// Runs the native Desktop application until its event loop exits.
///
/// # Errors
///
/// Returns a Tauri error when setup or the native event loop cannot start or complete.
pub fn run() -> tauri::Result<()> {
    let app = tauri::Builder::default()
        // The single-instance plugin must remain the first registered plugin.
        .plugin(single_instance::plugin())
        .plugin(tauri_plugin_dialog::init())
        .manage(DesktopState::default())
        .manage(DesktopPreferencesStore::default())
        .setup(|app| {
            let app_data_root = app.path().app_local_data_dir()?;
            std::fs::create_dir_all(&app_data_root)?;
            let app_data_metadata = std::fs::symlink_metadata(&app_data_root)?;
            if !app_data_metadata.file_type().is_dir() || app_data_metadata.file_type().is_symlink()
            {
                return Err(std::io::Error::other(
                    "Desktop application data root is not a private directory",
                )
                .into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&app_data_root, std::fs::Permissions::from_mode(0o700))?;
            }
            app.state::<DesktopPreferencesStore>()
                .configure(&app_data_root.join("preferences"))
                .map_err(|error| std::io::Error::other(error.message))?;
            app.state::<DesktopState>()
                .configure_supervisor_storage(app_data_root.join("supervisor"))
                .map_err(|error| std::io::Error::other(error.message))?;
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let app_for_prepare = app_handle.clone();
                let state = app_handle.state::<DesktopState>();
                let _ = state
                    .prepare_and_start_managed_runtime(move || {
                        managed_runtime::prepare(&app_for_prepare)
                    })
                    .await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_desktop_status,
            commands::retry_managed_runtime,
            commands::get_desktop_preferences,
            commands::update_desktop_preferences,
            commands::reload_desktop_preferences,
            commands::subscribe_desktop_activation,
            commands::unsubscribe_desktop_activation,
            commands::get_desktop_window_route,
            commands::open_conversation_window,
            commands::execute_host_operation,
            commands::list_pending_host_operations,
            commands::acknowledge_host_operation,
            commands::acknowledge_host_event,
            commands::subscribe_host_events,
            commands::unsubscribe_host_events,
        ])
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } if window.label() == "main" => {
                let preferences = window.state::<DesktopPreferencesStore>();
                api.prevent_close();
                match main_window_close_action(preferences.window_close_behavior()) {
                    MainWindowCloseAction::Hide => {
                        if let Err(error) = window.hide() {
                            eprintln!("failed to hide the Desktop window: {error}");
                        }
                    }
                    MainWindowCloseAction::RequestExit => {
                        begin_coordinated_exit(window.app_handle().clone());
                    }
                }
            }
            tauri::WindowEvent::Destroyed => {
                window
                    .state::<DesktopState>()
                    .release_window(window.label());
            }
            _ => {}
        })
        .build(tauri::generate_context!())?;
    app.run(|app_handle, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            let state = app_handle.state::<DesktopState>();
            if state.exit_shutdown_completed() {
                return;
            }
            api.prevent_exit();
            begin_coordinated_exit(app_handle.clone());
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_window_quit_requests_process_exit() {
        assert_eq!(
            main_window_close_action(WindowCloseBehavior::KeepRunning),
            MainWindowCloseAction::Hide
        );
        assert_eq!(
            main_window_close_action(WindowCloseBehavior::Quit),
            MainWindowCloseAction::RequestExit
        );
    }
}
