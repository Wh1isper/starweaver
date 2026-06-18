//! System clipboard helpers for CLI/TUI media paste handling.

use std::env;

#[cfg(target_os = "linux")]
use std::{process::Command, time::Duration};

#[cfg(target_os = "linux")]
use starweaver_model::{detect_media_kind, MediaKind};

#[cfg(target_os = "linux")]
use crate::CliError;
use crate::{prompt_input::PromptAttachment, CliResult};

#[cfg(target_os = "linux")]
const SUPPORTED_IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];
#[cfg(target_os = "linux")]
const CLIPBOARD_COMMAND_TIMEOUT_SECONDS: u64 = 2;

/// Clipboard image read outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipboardImageReadResult {
    /// Attachment when an image was read.
    pub image: Option<PromptAttachment>,
    /// User-facing diagnostic when no image was available.
    pub error: Option<String>,
}

impl ClipboardImageReadResult {
    #[cfg(target_os = "linux")]
    fn image(index: usize, bytes: Vec<u8>, media_type: impl Into<String>) -> Self {
        Self {
            image: Some(PromptAttachment::image(index, bytes, media_type)),
            error: None,
        }
    }

    fn error(error: impl Into<String>) -> Self {
        Self {
            image: None,
            error: Some(error.into()),
        }
    }

    #[cfg(target_os = "linux")]
    const fn empty() -> Self {
        Self {
            image: None,
            error: None,
        }
    }
}

/// Read an image from the system clipboard when available.
#[allow(clippy::unnecessary_wraps)]
pub fn read_clipboard_image(index: usize) -> CliResult<ClipboardImageReadResult> {
    #[cfg(target_os = "linux")]
    {
        read_linux_clipboard_image(index)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = index;
        Ok(ClipboardImageReadResult::error(format!(
            "Clipboard image paste is not supported on platform: {}.",
            env::consts::OS
        )))
    }
}

#[cfg(target_os = "linux")]
fn read_linux_clipboard_image(index: usize) -> CliResult<ClipboardImageReadResult> {
    let mut errors = Vec::new();
    let mut attempted = false;
    if env::var_os("WAYLAND_DISPLAY").is_some() {
        attempted = true;
        let result = read_wayland_clipboard_image(index)?;
        if result.image.is_some() {
            return Ok(result);
        }
        if let Some(error) = result.error {
            errors.push(error);
        }
    }
    if env::var_os("DISPLAY").is_some() {
        attempted = true;
        let result = read_x11_clipboard_image(index)?;
        if result.image.is_some() {
            return Ok(result);
        }
        if let Some(error) = result.error {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        return Ok(if attempted {
            ClipboardImageReadResult::empty()
        } else {
            ClipboardImageReadResult::error(
                "Clipboard image paste requires wl-paste on Wayland or xclip on X11.",
            )
        });
    }
    Ok(ClipboardImageReadResult::error(errors.join(" ")))
}

#[cfg(target_os = "linux")]
fn read_wayland_clipboard_image(index: usize) -> CliResult<ClipboardImageReadResult> {
    if !command_exists("wl-paste") {
        return Ok(ClipboardImageReadResult::error(
            "Clipboard image paste requires wl-paste on Wayland.",
        ));
    }
    let output = run_clipboard_command(&["wl-paste", "--list-types"])?;
    if !output.status.success() {
        return Ok(ClipboardImageReadResult::empty());
    }
    let available = String::from_utf8_lossy(&output.stdout);
    for media_type in SUPPORTED_IMAGE_TYPES {
        if !available.lines().any(|line| line.trim() == media_type) {
            continue;
        }
        let image_output =
            run_clipboard_command(&["wl-paste", "--no-newline", "--type", media_type])?;
        if image_output.status.success() && !image_output.stdout.is_empty() {
            let detected = detected_image_media_type(&image_output.stdout).unwrap_or(media_type);
            return Ok(ClipboardImageReadResult::image(
                index,
                image_output.stdout,
                detected,
            ));
        }
    }
    Ok(ClipboardImageReadResult::empty())
}

#[cfg(target_os = "linux")]
fn read_x11_clipboard_image(index: usize) -> CliResult<ClipboardImageReadResult> {
    if !command_exists("xclip") {
        return Ok(ClipboardImageReadResult::error(
            "Clipboard image paste requires xclip on X11.",
        ));
    }
    let output =
        run_clipboard_command(&["xclip", "-selection", "clipboard", "-t", "TARGETS", "-o"])?;
    if !output.status.success() {
        return Ok(ClipboardImageReadResult::empty());
    }
    let available = String::from_utf8_lossy(&output.stdout);
    for media_type in SUPPORTED_IMAGE_TYPES {
        if !available.lines().any(|line| line.trim() == media_type) {
            continue;
        }
        let image_output =
            run_clipboard_command(&["xclip", "-selection", "clipboard", "-t", media_type, "-o"])?;
        if image_output.status.success() && !image_output.stdout.is_empty() {
            let detected = detected_image_media_type(&image_output.stdout).unwrap_or(media_type);
            return Ok(ClipboardImageReadResult::image(
                index,
                image_output.stdout,
                detected,
            ));
        }
    }
    Ok(ClipboardImageReadResult::empty())
}

#[cfg(target_os = "linux")]
fn command_exists(command: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {command} >/dev/null 2>&1"))
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(target_os = "linux")]
fn run_clipboard_command(args: &[&str]) -> CliResult<std::process::Output> {
    if args.is_empty() {
        return Err(CliError::Run(
            "clipboard command cannot be empty".to_string(),
        ));
    }
    let mut child = Command::new(args[0])
        .args(&args[1..])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| CliError::Run(error.to_string()))?;
    let start = std::time::Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|error| CliError::Run(error.to_string()))?
            .is_some()
        {
            return child
                .wait_with_output()
                .map_err(|error| CliError::Run(error.to_string()));
        }
        if start.elapsed() >= Duration::from_secs(CLIPBOARD_COMMAND_TIMEOUT_SECONDS) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CliError::Run("clipboard command timed out".to_string()));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(target_os = "linux")]
fn detected_image_media_type(bytes: &[u8]) -> Option<&'static str> {
    let kind = detect_media_kind(bytes);
    match kind {
        MediaKind::Png | MediaKind::Jpeg | MediaKind::Gif | MediaKind::Webp => kind.media_type(),
        _ => None,
    }
}
