//! Tauri build-time capability generation.

fn main() {
    let attributes =
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
            "get_desktop_status",
            "subscribe_desktop_activation",
            "unsubscribe_desktop_activation",
        ]));
    if let Err(error) = tauri_build::try_build(attributes) {
        panic!("failed to build Starweaver Desktop: {error}");
    }
}
