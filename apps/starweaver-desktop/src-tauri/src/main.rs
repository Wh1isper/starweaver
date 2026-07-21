// Prevent an additional console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Starweaver Desktop executable entry point.

fn main() {
    if let Err(error) = starweaver_desktop_lib::run() {
        eprintln!("failed to run Starweaver Desktop: {error}");
        std::process::exit(1);
    }
}
