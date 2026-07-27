//! Tauri build-time capability generation.

fn main() {
    let Ok(target) = std::env::var("TARGET") else {
        panic!("Cargo must provide the Desktop target triple");
    };
    println!("cargo:rustc-env=STARWEAVER_TARGET_TRIPLE={target}");
    println!("cargo:rerun-if-env-changed=STARWEAVER_BUILD_REVISION");
    println!("cargo:rerun-if-env-changed=STARWEAVER_UPDATE_PUBLIC_KEY");

    let attributes =
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
            "get_desktop_status",
            "retry_managed_runtime",
            "get_runtime_update_status",
            "check_runtime_update",
            "install_runtime_update",
            "rollback_runtime_update",
            "get_desktop_update_status",
            "check_desktop_update",
            "install_desktop_update",
            "get_desktop_preferences",
            "update_desktop_preferences",
            "reload_desktop_preferences",
            "subscribe_desktop_activation",
            "unsubscribe_desktop_activation",
            "get_desktop_window_route",
            "open_conversation_window",
        ]));
    if let Err(error) = tauri_build::try_build(attributes) {
        panic!("failed to build Starweaver Desktop: {error}");
    }
}
