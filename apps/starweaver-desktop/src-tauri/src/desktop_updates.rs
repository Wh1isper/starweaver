//! Privileged Tauri updater integration for the native Desktop shell.

use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use minisign_verify::{PublicKey, Signature};
use reqwest::{Client, Url, redirect::Policy};
use serde::Serialize;
use serde_json::Value;
use tauri::AppHandle;
#[cfg(target_os = "linux")]
use tauri::utils::{config::BundleType, platform::bundle_type};
use tauri_plugin_updater::{Update, UpdaterExt as _};
use tokio::sync::Mutex;

const UPDATE_ENDPOINT: &str =
    "https://github.com/Wh1isper/starweaver/releases/latest/download/latest.json";
const RELEASE_REPOSITORY_PREFIX: &str = "/Wh1isper/starweaver/releases/download/";
const UPDATE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_DESKTOP_METADATA_BYTES: usize = 256 * 1024;
const MAX_DESKTOP_UPDATE_BYTES: usize = 1024 * 1024 * 1024;
const MAX_RELEASE_NOTES_CHARS: usize = 2_000;
const MAX_SIGNATURE_CHARS: usize = 16 * 1024;

/// Safe projection of one Tauri-signature-verifiable Desktop update.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopUpdateCandidate {
    /// Newer Desktop semantic version.
    pub version: String,
    /// Bounded plain-text release notes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Optional RFC 3339 publication timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    /// OS publisher signing is intentionally not configured for this release channel.
    pub platform_publisher_signed: bool,
}

/// Safe Tauri Desktop updater state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopUpdateSnapshot {
    /// Running Desktop version.
    pub current_version: String,
    /// Whether this binary embeds a project updater public key.
    pub configured: bool,
    /// Backend-retained candidate, if a newer release was found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate: Option<DesktopUpdateCandidate>,
}

/// Stable safe Desktop updater failure categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopUpdateErrorCode {
    /// This binary has no project updater trust key.
    Unavailable,
    /// The fixed update source or artifact could not be reached.
    Network,
    /// Update metadata or the mandatory Tauri artifact signature was rejected.
    Verification,
    /// The exact retained candidate is no longer available.
    StaleCandidate,
    /// The native installer could not be applied.
    Installation,
    /// The user cancelled the native confirmation.
    Cancelled,
}

/// Sanitized Desktop updater error returned through privileged IPC.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopUpdateError {
    /// Stable category.
    pub code: DesktopUpdateErrorCode,
    /// Fixed summary without URLs, signatures, or server-controlled bodies.
    pub message: String,
}

impl DesktopUpdateError {
    fn new(code: DesktopUpdateErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }

    pub(crate) fn unavailable() -> Self {
        Self::new(
            DesktopUpdateErrorCode::Unavailable,
            "Desktop updates are not configured in this build",
        )
    }

    pub(crate) fn cancelled() -> Self {
        Self::new(
            DesktopUpdateErrorCode::Cancelled,
            "Desktop update installation was cancelled",
        )
    }

    fn network() -> Self {
        Self::new(
            DesktopUpdateErrorCode::Network,
            "the fixed Starweaver Desktop update source is unavailable",
        )
    }

    fn verification() -> Self {
        Self::new(
            DesktopUpdateErrorCode::Verification,
            "the Desktop update did not pass Tauri signature verification",
        )
    }

    pub(crate) fn installation() -> Self {
        Self::new(
            DesktopUpdateErrorCode::Installation,
            "the verified Desktop update could not be installed",
        )
    }
}

/// Process-owned exact Tauri update candidate.
#[derive(Default)]
pub struct DesktopUpdateManager {
    candidate: Mutex<Option<Update>>,
}

impl DesktopUpdateManager {
    /// Return the safe state without contacting the release source.
    pub async fn snapshot(&self, current_version: &str) -> DesktopUpdateSnapshot {
        let candidate = self
            .candidate
            .lock()
            .await
            .as_ref()
            .map(candidate_projection);
        DesktopUpdateSnapshot {
            current_version: current_version.to_string(),
            configured: is_configured(),
            candidate,
        }
    }

    /// Check the fixed Tauri endpoint and retain the exact newer candidate in native state.
    pub async fn check(
        &self,
        app: &AppHandle,
    ) -> Result<DesktopUpdateSnapshot, DesktopUpdateError> {
        let public_key = update_public_key()?;
        let expected_metadata = fetch_and_validate_metadata().await?;
        let endpoint = UPDATE_ENDPOINT
            .parse()
            .map_err(|_| DesktopUpdateError::unavailable())?;
        let updater = app
            .updater_builder()
            .pubkey(public_key)
            .endpoints(vec![endpoint])
            .map_err(|_| DesktopUpdateError::unavailable())?
            .timeout(UPDATE_TIMEOUT)
            .build()
            .map_err(|_| DesktopUpdateError::unavailable())?;
        let candidate = updater
            .check()
            .await
            .map_err(|_| DesktopUpdateError::network())?;
        if let Some(candidate) = candidate.as_ref() {
            validate_candidate(candidate)?;
            if candidate.raw_json != expected_metadata {
                return Err(DesktopUpdateError::verification());
            }
        }
        *self.candidate.lock().await = candidate;
        Ok(self.snapshot(&app.package_info().version.to_string()).await)
    }

    /// Download and Tauri-verify the retained candidate before returning installer bytes.
    pub async fn download(
        &self,
        expected_version: &str,
    ) -> Result<(Update, Vec<u8>), DesktopUpdateError> {
        let candidate = self
            .candidate
            .lock()
            .await
            .take()
            .filter(|candidate| candidate.version == expected_version)
            .ok_or_else(|| {
                DesktopUpdateError::new(
                    DesktopUpdateErrorCode::StaleCandidate,
                    "the checked Desktop candidate expired; check again",
                )
            })?;
        validate_candidate(&candidate)?;
        let public_key = update_public_key()?;
        let bytes = tokio::time::timeout(
            UPDATE_TIMEOUT,
            download_and_verify_candidate(&candidate, &public_key),
        )
        .await
        .map_err(|_| DesktopUpdateError::network())??;
        Ok((candidate, bytes))
    }

    /// Apply already downloaded and signature-verified installer bytes.
    pub fn install(candidate: &Update, bytes: &[u8]) -> Result<(), DesktopUpdateError> {
        candidate
            .install(bytes)
            .map_err(|_| DesktopUpdateError::installation())
    }
}

pub fn is_configured() -> bool {
    update_public_key().is_ok()
}

fn update_public_key() -> Result<String, DesktopUpdateError> {
    crate::update_trust::embedded_public_key()
        .map(str::to_string)
        .ok_or_else(DesktopUpdateError::unavailable)
}

async fn fetch_and_validate_metadata() -> Result<Value, DesktopUpdateError> {
    let client = update_client()?;
    let url = Url::parse(UPDATE_ENDPOINT).map_err(|_| DesktopUpdateError::unavailable())?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| DesktopUpdateError::network())?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|length| length > MAX_DESKTOP_METADATA_BYTES as u64)
    {
        return Err(DesktopUpdateError::network());
    }
    let mut bytes = Vec::with_capacity(
        response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or(0)
            .min(MAX_DESKTOP_METADATA_BYTES),
    );
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| DesktopUpdateError::network())?
    {
        if chunk.len() > MAX_DESKTOP_METADATA_BYTES.saturating_sub(bytes.len()) {
            return Err(DesktopUpdateError::verification());
        }
        bytes.extend_from_slice(&chunk);
    }
    let metadata: Value =
        serde_json::from_slice(&bytes).map_err(|_| DesktopUpdateError::verification())?;
    let platforms = metadata
        .get("platforms")
        .and_then(Value::as_object)
        .ok_or_else(DesktopUpdateError::verification)?;
    if platforms.is_empty()
        || platforms.len() > 16
        || platforms.values().any(|entry| {
            entry
                .get("signature")
                .and_then(Value::as_str)
                .is_none_or(|signature| {
                    signature.is_empty() || signature.len() > MAX_SIGNATURE_CHARS
                })
        })
    {
        return Err(DesktopUpdateError::verification());
    }
    Ok(metadata)
}

fn update_client() -> Result<Client, DesktopUpdateError> {
    Client::builder()
        .https_only(true)
        .redirect(Policy::limited(5))
        .timeout(UPDATE_TIMEOUT)
        .user_agent(concat!("starweaver-desktop/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| DesktopUpdateError::unavailable())
}

async fn download_and_verify_candidate(
    candidate: &Update,
    configured_public_key: &str,
) -> Result<Vec<u8>, DesktopUpdateError> {
    let client = update_client()?;
    let mut response = client
        .get(candidate.download_url.clone())
        .send()
        .await
        .map_err(|_| DesktopUpdateError::network())?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|length| length > MAX_DESKTOP_UPDATE_BYTES as u64)
    {
        return Err(DesktopUpdateError::network());
    }
    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0)
        .min(MAX_DESKTOP_UPDATE_BYTES);
    let mut bytes = Vec::with_capacity(capacity);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| DesktopUpdateError::network())?
    {
        if chunk.len() > MAX_DESKTOP_UPDATE_BYTES.saturating_sub(bytes.len()) {
            return Err(DesktopUpdateError::verification());
        }
        bytes.extend_from_slice(&chunk);
    }
    let public_key = parse_public_key(configured_public_key)?;
    let signature = parse_signature(&candidate.signature)?;
    public_key
        .verify(&bytes, &signature, false)
        .map_err(|_| DesktopUpdateError::verification())?;
    Ok(bytes)
}

fn parse_public_key(configured: &str) -> Result<PublicKey, DesktopUpdateError> {
    crate::update_trust::parse_public_key(configured)
        .map_err(|()| DesktopUpdateError::unavailable())
}

fn parse_signature(signature: &str) -> Result<Signature, DesktopUpdateError> {
    let signature = signature.trim();
    if signature.starts_with("untrusted comment:") {
        return Signature::decode(signature).map_err(|_| DesktopUpdateError::verification());
    }
    let decoded = BASE64
        .decode(signature)
        .map_err(|_| DesktopUpdateError::verification())?;
    let text = std::str::from_utf8(&decoded).map_err(|_| DesktopUpdateError::verification())?;
    Signature::decode(text).map_err(|_| DesktopUpdateError::verification())
}

fn validate_candidate(candidate: &Update) -> Result<(), DesktopUpdateError> {
    let (updater_target, asset_name) = match env!("STARWEAVER_TARGET_TRIPLE") {
        "aarch64-apple-darwin" => (
            "darwin",
            format!(
                "starweaver-desktop-v{}-aarch64-apple-darwin.app.tar.gz",
                candidate.version
            ),
        ),
        "x86_64-apple-darwin" => (
            "darwin",
            format!(
                "starweaver-desktop-v{}-x86_64-apple-darwin.app.tar.gz",
                candidate.version
            ),
        ),
        "x86_64-unknown-linux-gnu" => ("linux", linux_update_asset_name(&candidate.version)?),
        "x86_64-pc-windows-msvc" => (
            "windows",
            format!(
                "starweaver-desktop-v{}-x86_64-pc-windows-msvc-setup.exe",
                candidate.version
            ),
        ),
        _ => return Err(DesktopUpdateError::verification()),
    };
    let expected_path = format!(
        "{RELEASE_REPOSITORY_PREFIX}v{}/{}",
        candidate.version, asset_name
    );
    let url = &candidate.download_url;
    if candidate.target != updater_target
        || url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || url.path() != expected_path
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || candidate.signature.trim().is_empty()
        || candidate.signature.len() > MAX_SIGNATURE_CHARS
    {
        return Err(DesktopUpdateError::verification());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn linux_update_asset_name(version: &str) -> Result<String, DesktopUpdateError> {
    let extension = match bundle_type() {
        Some(BundleType::AppImage) => "AppImage",
        Some(BundleType::Deb) => "deb",
        _ => return Err(DesktopUpdateError::verification()),
    };
    Ok(format!(
        "starweaver-desktop-v{version}-x86_64-unknown-linux-gnu.{extension}"
    ))
}

#[cfg(not(target_os = "linux"))]
fn linux_update_asset_name(_version: &str) -> Result<String, DesktopUpdateError> {
    Err(DesktopUpdateError::verification())
}

fn candidate_projection(candidate: &Update) -> DesktopUpdateCandidate {
    DesktopUpdateCandidate {
        version: candidate.version.clone(),
        notes: candidate
            .body
            .as_deref()
            .map(|notes| notes.chars().take(MAX_RELEASE_NOTES_CHARS).collect()),
        published_at: candidate.date.map(|date| date.to_string()),
        platform_publisher_signed: false,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn verifies_tauri_encoded_desktop_artifact_signature() {
        const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEY3NjkyNTVFNjYyRDFEOEIKUldTTEhTMW1YaVZwOS9SZkVNeWtlYUREWjFKTy9mSmFGZXV1SHFNVUJaRUxHQnJlTGFSeERjYmoK";
        const SIGNATURE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTTEhTMW1YaVZwOXphZCt0aTY4ZldUOElERUpxcXQ4ck0xakNOVFdubEJyeTRmNGRhdE9XZU5aMEJKdmJhRXZlNytGMVI5M1RzK0FxTld1WjRYV1BNU1c0Mk1GWWFnV1FZPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzg1MTMwNDUzCWZpbGU6c2lnbmF0dXJlLWZpeHR1cmUudHh0CnY3eENHNHQ5cS9pdE95TEppMzNuN1RvS3V2Y1UxN2w3YlVxamNlSGNuNW9EaGtGa0xPalNndUNDbFFBWDIzaGJMMlRRYmx5ZzhXNW0wTHluYlNSUURnPT0K";
        let key = parse_public_key(PUBLIC_KEY).expect("Tauri public key");
        let signature = parse_signature(SIGNATURE).expect("Tauri signature");

        key.verify(
            b"starweaver runtime update signature fixture\n",
            &signature,
            false,
        )
        .expect("matching artifact signature");
        assert!(
            key.verify(b"tampered artifact\n", &signature, false)
                .is_err()
        );
    }

    #[test]
    fn update_endpoint_is_fixed_https_release_metadata() {
        let endpoint = reqwest::Url::parse(UPDATE_ENDPOINT).expect("valid fixed endpoint");
        assert_eq!(endpoint.scheme(), "https");
        assert_eq!(endpoint.host_str(), Some("github.com"));
        assert_eq!(
            endpoint.path(),
            "/Wh1isper/starweaver/releases/latest/download/latest.json"
        );
    }
}
