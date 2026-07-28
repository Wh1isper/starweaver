//! Compatibility-gated independent updates for the Desktop-managed RPC runtime.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use minisign_verify::{PublicKey, Signature};
use reqwest::{Client, Url, redirect::Policy};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::sync::Mutex as AsyncMutex;

use crate::{
    managed_runtime,
    supervisor::{LocalHostSupervisor, ReadyRuntimeIdentity, RuntimeLaunchSource, SupervisorError},
};

const MANIFEST_SCHEMA_VERSION: u32 = 1;
const POINTER_SCHEMA_VERSION: u32 = 1;
const SELECTION_SCHEMA_VERSION: u32 = 1;
const STORAGE_GENERATION: u64 = 1;
const MAX_MANIFEST_BYTES: usize = 128 * 1024;
const MAX_SIGNATURE_BYTES: usize = 16 * 1024;
const MAX_RUNTIME_BYTES: u64 = 256 * 1024 * 1024;
const NETWORK_TIMEOUT: Duration = Duration::from_secs(30);
const RELEASE_REPOSITORY_PREFIX: &str = "/Wh1isper/starweaver/releases/download/";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RuntimeProtocolIdentity {
    name: String,
    major: u32,
    revision: String,
    schema_digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RuntimeAsset {
    name: String,
    url: String,
    size: u64,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RuntimeUpdateManifest {
    schema_version: u32,
    version: String,
    build_revision: String,
    rust_target: String,
    desktop_version_requirement: String,
    protocol: RuntimeProtocolIdentity,
    launch_schema_version: u32,
    storage_generation: u64,
    asset: RuntimeAsset,
}

#[derive(Clone)]
struct VerifiedRuntimeCandidate {
    candidate_id: String,
    manifest: RuntimeUpdateManifest,
    manifest_bytes: Vec<u8>,
    signature: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RuntimePointer {
    schema_version: u32,
    installation_id: String,
    manifest_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RuntimeSelectionRecord {
    schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<RuntimePointer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous: Option<RuntimePointer>,
}

impl Default for RuntimeSelectionRecord {
    fn default() -> Self {
        Self {
            schema_version: SELECTION_SCHEMA_VERSION,
            current: None,
            previous: None,
        }
    }
}

/// Safe renderer projection of an independently published RPC candidate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeUpdateCandidate {
    /// Opaque identity bound to the verified manifest bytes.
    pub candidate_id: String,
    /// Runtime semantic version.
    pub version: String,
    /// Immutable source revision.
    pub build_revision: String,
    /// Exact Rust target triple.
    pub target: String,
    /// Download size in bytes.
    pub size: u64,
}

/// Safe current runtime selection and candidate projection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeUpdateSnapshot {
    /// Whether this binary embeds the project update trust key.
    pub configured: bool,
    /// Version reported by the currently ready host, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_version: Option<String>,
    /// Source of the currently ready host, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_source: Option<RuntimeSelectionSource>,
    /// Version selected for the next Desktop process start.
    pub selected_version: String,
    /// Whether the next start uses the bundled fallback or a managed version.
    pub selected_source: RuntimeSelectionSource,
    /// Verified newer candidate retained by the privileged backend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate: Option<RuntimeUpdateCandidate>,
    /// Whether changing the selection requires an application restart.
    pub restart_required: bool,
    #[serde(skip)]
    selected_digest: Option<String>,
}

impl RuntimeUpdateSnapshot {
    /// Bind the next-start selection projection to the currently ready runtime identity.
    #[must_use]
    pub(crate) fn with_active_runtime(mut self, active: Option<ReadyRuntimeIdentity>) -> Self {
        self.restart_required = active.as_ref().is_some_and(|active| {
            let active_source = selection_source(active.source);
            active.version != self.selected_version
                || active_source != self.selected_source
                || (self.selected_source == RuntimeSelectionSource::Managed
                    && self.selected_digest.as_deref() != Some(active.digest.as_str()))
        });
        self.active_version = active.as_ref().map(|active| active.version.clone());
        self.active_source = active.map(|active| selection_source(active.source));
        self
    }
}

const fn selection_source(source: RuntimeLaunchSource) -> RuntimeSelectionSource {
    match source {
        RuntimeLaunchSource::Bundled => RuntimeSelectionSource::Bundled,
        RuntimeLaunchSource::Managed => RuntimeSelectionSource::Managed,
    }
}

/// Runtime source selected for the next Desktop process start.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSelectionSource {
    /// Exact RPC sidecar shipped in the native Desktop package.
    Bundled,
    /// Independently installed and compatibility-gated RPC version.
    Managed,
}

/// Stable safe runtime-update failure categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeUpdateErrorCode {
    /// The release trust key or target is not configured in this build.
    Unavailable,
    /// The fixed release source could not be reached within policy.
    Network,
    /// Manifest, signature, digest, target, or compatibility validation failed.
    Verification,
    /// Private versioned runtime state could not be persisted.
    Storage,
    /// The requested candidate is no longer retained by the backend.
    StaleCandidate,
    /// The candidate failed its isolated RPC initialize probe.
    Probe,
}

/// Sanitized runtime-update failure returned through privileged IPC.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeUpdateError {
    /// Stable category.
    pub code: RuntimeUpdateErrorCode,
    /// Fixed user-safe summary without paths, URLs, or response bodies.
    pub message: String,
}

impl RuntimeUpdateError {
    fn new(code: RuntimeUpdateErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }

    fn unavailable() -> Self {
        Self::new(
            RuntimeUpdateErrorCode::Unavailable,
            "runtime updates are not configured in this Desktop build",
        )
    }

    fn network() -> Self {
        Self::new(
            RuntimeUpdateErrorCode::Network,
            "the fixed Starweaver runtime update source is unavailable",
        )
    }

    fn verification() -> Self {
        Self::new(
            RuntimeUpdateErrorCode::Verification,
            "the runtime update did not pass signature and compatibility verification",
        )
    }

    fn storage() -> Self {
        Self::new(
            RuntimeUpdateErrorCode::Storage,
            "the verified runtime update could not be stored privately",
        )
    }
}

/// Process-owned independent RPC update manager.
#[derive(Default)]
pub struct RuntimeUpdateManager {
    root: Mutex<Option<PathBuf>>,
    candidate: Mutex<Option<VerifiedRuntimeCandidate>>,
    operation_gate: AsyncMutex<()>,
}

impl RuntimeUpdateManager {
    /// Configure the private runtime root exactly once.
    pub fn configure(&self, root: PathBuf) -> Result<(), RuntimeUpdateError> {
        create_private_directory(&root)?;
        let mut configured = self
            .root
            .lock()
            .map_err(|_| RuntimeUpdateError::storage())?;
        if configured
            .as_ref()
            .is_some_and(|existing| existing != &root)
        {
            return Err(RuntimeUpdateError::storage());
        }
        *configured = Some(root);
        drop(configured);
        Ok(())
    }

    /// Return the next-start selection and any retained verified candidate.
    pub fn snapshot(&self, desktop_version: &str) -> RuntimeUpdateSnapshot {
        let root = self.root();
        let selected = root.as_deref().and_then(|root| {
            resolve_managed_runtime(root, desktop_version)
                .ok()
                .flatten()
        });
        let candidate = self
            .candidate
            .lock()
            .ok()
            .and_then(|candidate| candidate.as_ref().map(candidate_projection));
        RuntimeUpdateSnapshot {
            configured: update_public_key().is_ok(),
            active_version: None,
            active_source: None,
            selected_version: selected.as_ref().map_or_else(
                || desktop_version.to_string(),
                |selection| selection.version.clone(),
            ),
            selected_source: if selected.is_some() {
                RuntimeSelectionSource::Managed
            } else {
                RuntimeSelectionSource::Bundled
            },
            candidate,
            restart_required: false,
            selected_digest: selected.map(|selection| selection.runtime_digest),
        }
    }

    /// Check the one fixed target-specific manifest and retain a newer compatible candidate.
    pub async fn check(
        &self,
        desktop_version: &str,
    ) -> Result<RuntimeUpdateSnapshot, RuntimeUpdateError> {
        let _operation = self.operation_gate.lock().await;
        let public_key = update_public_key()?;
        let target = env!("STARWEAVER_TARGET_TRIPLE");
        let manifest_url = fixed_manifest_url(target)?;
        let signature_url = Url::parse(&format!("{manifest_url}.sig"))
            .map_err(|_| RuntimeUpdateError::unavailable())?;
        let client = update_client()?;
        let manifest_bytes = fetch_bounded(&client, manifest_url, MAX_MANIFEST_BYTES).await?;
        let signature_bytes = fetch_bounded(&client, signature_url, MAX_SIGNATURE_BYTES).await?;
        let signature = std::str::from_utf8(&signature_bytes)
            .map_err(|_| RuntimeUpdateError::verification())?
            .to_string();
        verify_signature(&public_key, &manifest_bytes, &signature)?;
        let manifest: RuntimeUpdateManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|_| RuntimeUpdateError::verification())?;
        validate_manifest(&manifest, desktop_version, target)?;

        let selected_version = self.snapshot(desktop_version).selected_version;
        let current =
            Version::parse(&selected_version).map_err(|_| RuntimeUpdateError::verification())?;
        let candidate_version =
            Version::parse(&manifest.version).map_err(|_| RuntimeUpdateError::verification())?;
        let retained = if candidate_version > current {
            Some(VerifiedRuntimeCandidate {
                candidate_id: sha256_bytes(&manifest_bytes),
                manifest,
                manifest_bytes,
                signature,
            })
        } else {
            None
        };
        *self
            .candidate
            .lock()
            .map_err(|_| RuntimeUpdateError::storage())? = retained;
        Ok(self.snapshot(desktop_version))
    }

    /// Download, verify, probe, and select one retained candidate for the next app start.
    pub async fn install(
        &self,
        candidate_id: &str,
        desktop_version: &str,
    ) -> Result<RuntimeUpdateSnapshot, RuntimeUpdateError> {
        let _operation = self.operation_gate.lock().await;
        let candidate = self
            .candidate
            .lock()
            .map_err(|_| RuntimeUpdateError::storage())?
            .as_ref()
            .filter(|candidate| candidate.candidate_id == candidate_id)
            .cloned()
            .ok_or_else(|| {
                RuntimeUpdateError::new(
                    RuntimeUpdateErrorCode::StaleCandidate,
                    "the checked runtime candidate expired; check again",
                )
            })?;
        let root = self.root().ok_or_else(RuntimeUpdateError::storage)?;
        validate_manifest(
            &candidate.manifest,
            desktop_version,
            env!("STARWEAVER_TARGET_TRIPLE"),
        )?;

        let staging_root = root.join("staging");
        create_private_directory(&staging_root)?;
        let staging = staging_root.join(format!("candidate-{}", uuid::Uuid::new_v4()));
        create_private_directory(&staging)?;
        let executable_name = runtime_executable_name();
        let staged_runtime = staging.join(executable_name);
        if let Err(error) = self.download_and_stage(&candidate, &staged_runtime).await {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        if let Err(error) = probe_candidate(&candidate.manifest, &staged_runtime).await {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }

        let installation_id = installation_id(&candidate.candidate_id)?;
        let versions = root.join("versions");
        create_private_directory(&versions)?;
        let installed = versions.join(&installation_id);
        if installed.exists() {
            let existing_runtime = installed.join(executable_name);
            if sha256_file_exact(&existing_runtime, candidate.manifest.asset.size)
                .ok()
                .as_deref()
                == Some(candidate.manifest.asset.sha256.as_str())
            {
                fs::remove_dir_all(&staging).map_err(|_| RuntimeUpdateError::storage())?;
            } else {
                fs::remove_dir_all(&installed).map_err(|_| RuntimeUpdateError::storage())?;
                fs::rename(&staging, &installed).map_err(|_| RuntimeUpdateError::storage())?;
                sync_parent(&versions)?;
            }
        } else {
            fs::rename(&staging, &installed).map_err(|_| RuntimeUpdateError::storage())?;
            sync_parent(&versions)?;
        }
        atomic_write(
            &installed.join("manifest.json"),
            &candidate.manifest_bytes,
            0o400,
        )?;
        atomic_write(
            &installed.join("manifest.sig"),
            candidate.signature.as_bytes(),
            0o400,
        )?;

        let pointer = RuntimePointer {
            schema_version: POINTER_SCHEMA_VERSION,
            installation_id,
            manifest_digest: candidate.candidate_id.clone(),
        };
        replace_runtime_pointer(&root, &pointer)?;
        *self
            .candidate
            .lock()
            .map_err(|_| RuntimeUpdateError::storage())? = None;
        let mut snapshot = self.snapshot(desktop_version);
        snapshot.restart_required = true;
        Ok(snapshot)
    }

    /// Select the previous verified runtime, or the bundled fallback, for the next app start.
    pub async fn rollback(
        &self,
        desktop_version: &str,
    ) -> Result<RuntimeUpdateSnapshot, RuntimeUpdateError> {
        let _operation = self.operation_gate.lock().await;
        let root = self.root().ok_or_else(RuntimeUpdateError::storage)?;
        let mut selection = read_selection_or_default(&root)?;
        if let Some(previous) = selection.previous.take() {
            selection.previous = selection.current.replace(previous);
        } else {
            selection.previous = selection.current.take();
        }
        atomic_json(&root.join("selection.json"), &selection)?;
        let mut snapshot = self.snapshot(desktop_version);
        snapshot.restart_required = true;
        Ok(snapshot)
    }

    async fn download_and_stage(
        &self,
        candidate: &VerifiedRuntimeCandidate,
        destination: &Path,
    ) -> Result<(), RuntimeUpdateError> {
        let url = validate_asset_url(&candidate.manifest.asset)?;
        let client = update_client()?;
        let mut response = client
            .get(url)
            .send()
            .await
            .map_err(|_| RuntimeUpdateError::network())?;
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|length| length != candidate.manifest.asset.size)
        {
            return Err(RuntimeUpdateError::network());
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o500);
        }
        let mut file = options
            .open(destination)
            .map_err(|_| RuntimeUpdateError::storage())?;
        let mut hasher = Sha256::new();
        let mut received = 0_u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| RuntimeUpdateError::network())?
        {
            received = received
                .checked_add(
                    u64::try_from(chunk.len()).map_err(|_| RuntimeUpdateError::verification())?,
                )
                .ok_or_else(RuntimeUpdateError::verification)?;
            if received > candidate.manifest.asset.size || received > MAX_RUNTIME_BYTES {
                return Err(RuntimeUpdateError::verification());
            }
            file.write_all(&chunk)
                .map_err(|_| RuntimeUpdateError::storage())?;
            hasher.update(&chunk);
        }
        file.sync_all().map_err(|_| RuntimeUpdateError::storage())?;
        if received != candidate.manifest.asset.size
            || format!("sha256:{:x}", hasher.finalize()) != candidate.manifest.asset.sha256
        {
            return Err(RuntimeUpdateError::verification());
        }
        #[cfg(unix)]
        fs::set_permissions(
            destination,
            std::os::unix::fs::PermissionsExt::from_mode(0o500),
        )
        .map_err(|_| RuntimeUpdateError::storage())?;
        Ok(())
    }

    fn root(&self) -> Option<PathBuf> {
        self.root.lock().ok().and_then(|root| root.clone())
    }
}

pub struct ManagedRuntimeSelection {
    pub path: PathBuf,
    pub version: String,
    pub build_revision: String,
    pub target: String,
    pub runtime_digest: String,
    pub runtime_size: u64,
}

pub fn resolve_managed_runtime(
    root: &Path,
    desktop_version: &str,
) -> Result<Option<ManagedRuntimeSelection>, RuntimeUpdateError> {
    let Ok(selection) = read_selection(&root.join("selection.json")) else {
        return Ok(None);
    };
    let Some(pointer) = selection.current else {
        return Ok(None);
    };
    let installed = root.join("versions").join(&pointer.installation_id);
    let manifest_bytes = read_bounded_file(&installed.join("manifest.json"), MAX_MANIFEST_BYTES)?;
    if sha256_bytes(&manifest_bytes) != pointer.manifest_digest {
        return Ok(None);
    }
    let signature_bytes = read_bounded_file(&installed.join("manifest.sig"), MAX_SIGNATURE_BYTES)?;
    let signature =
        std::str::from_utf8(&signature_bytes).map_err(|_| RuntimeUpdateError::verification())?;
    verify_signature(&update_public_key()?, &manifest_bytes, signature)?;
    let manifest: RuntimeUpdateManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|_| RuntimeUpdateError::verification())?;
    validate_manifest(&manifest, desktop_version, env!("STARWEAVER_TARGET_TRIPLE"))?;
    let runtime = installed.join(runtime_executable_name());
    if sha256_file_exact(&runtime, manifest.asset.size)? != manifest.asset.sha256 {
        return Ok(None);
    }
    Ok(Some(ManagedRuntimeSelection {
        path: runtime,
        version: manifest.version,
        build_revision: manifest.build_revision,
        target: manifest.rust_target,
        runtime_digest: manifest.asset.sha256,
        runtime_size: manifest.asset.size,
    }))
}

fn candidate_projection(candidate: &VerifiedRuntimeCandidate) -> RuntimeUpdateCandidate {
    RuntimeUpdateCandidate {
        candidate_id: candidate.candidate_id.clone(),
        version: candidate.manifest.version.clone(),
        build_revision: candidate.manifest.build_revision.clone(),
        target: candidate.manifest.rust_target.clone(),
        size: candidate.manifest.asset.size,
    }
}

fn update_public_key() -> Result<PublicKey, RuntimeUpdateError> {
    let configured =
        crate::update_trust::embedded_public_key().ok_or_else(RuntimeUpdateError::unavailable)?;
    parse_public_key(configured)
}

fn parse_public_key(configured: &str) -> Result<PublicKey, RuntimeUpdateError> {
    crate::update_trust::parse_public_key(configured)
        .map_err(|()| RuntimeUpdateError::unavailable())
}

fn fixed_manifest_url(target: &str) -> Result<Url, RuntimeUpdateError> {
    Url::parse(&format!(
        "https://github.com/Wh1isper/starweaver/releases/latest/download/starweaver-runtime-{target}.manifest.json"
    ))
    .map_err(|_| RuntimeUpdateError::unavailable())
}

fn update_client() -> Result<Client, RuntimeUpdateError> {
    Client::builder()
        .https_only(true)
        .redirect(Policy::limited(5))
        .timeout(NETWORK_TIMEOUT)
        .user_agent(concat!("starweaver-desktop/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| RuntimeUpdateError::unavailable())
}

async fn fetch_bounded(
    client: &Client,
    url: Url,
    maximum: usize,
) -> Result<Vec<u8>, RuntimeUpdateError> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| RuntimeUpdateError::network())?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|length| length > maximum as u64)
    {
        return Err(RuntimeUpdateError::network());
    }
    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0)
        .min(maximum);
    let mut bytes = Vec::with_capacity(capacity);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| RuntimeUpdateError::network())?
    {
        if chunk.len() > maximum.saturating_sub(bytes.len()) {
            return Err(RuntimeUpdateError::verification());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn verify_signature(
    public_key: &PublicKey,
    bytes: &[u8],
    signature: &str,
) -> Result<(), RuntimeUpdateError> {
    let signature = parse_signature(signature)?;
    public_key
        .verify(bytes, &signature, false)
        .map_err(|_| RuntimeUpdateError::verification())
}

fn parse_signature(signature: &str) -> Result<Signature, RuntimeUpdateError> {
    let signature = signature.trim();
    if signature.starts_with("untrusted comment:") {
        return Signature::decode(signature).map_err(|_| RuntimeUpdateError::verification());
    }
    let decoded = BASE64
        .decode(signature)
        .map_err(|_| RuntimeUpdateError::verification())?;
    let text = std::str::from_utf8(&decoded).map_err(|_| RuntimeUpdateError::verification())?;
    Signature::decode(text).map_err(|_| RuntimeUpdateError::verification())
}

fn validate_manifest(
    manifest: &RuntimeUpdateManifest,
    desktop_version: &str,
    target: &str,
) -> Result<(), RuntimeUpdateError> {
    let desktop =
        Version::parse(desktop_version).map_err(|_| RuntimeUpdateError::verification())?;
    let requirement = VersionReq::parse(&manifest.desktop_version_requirement)
        .map_err(|_| RuntimeUpdateError::verification())?;
    let version =
        Version::parse(&manifest.version).map_err(|_| RuntimeUpdateError::verification())?;
    let expected_name = format!(
        "starweaver-rpc-v{}-{target}{}",
        manifest.version,
        if target.contains("windows") {
            ".exe"
        } else {
            ""
        }
    );
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION
        || !requirement.matches(&desktop)
        || !version.pre.is_empty()
        || !valid_build_revision(&manifest.build_revision)
        || manifest.rust_target != target
        || manifest.protocol.name != starweaver_rpc_core::generated::PROTOCOL_NAME
        || manifest.protocol.major != starweaver_rpc_core::generated::PROTOCOL_MAJOR
        || manifest.protocol.revision != starweaver_rpc_core::generated::PROTOCOL_REVISION
        || manifest.protocol.schema_digest != starweaver_rpc_core::generated::SCHEMA_DIGEST
        || manifest.launch_schema_version != starweaver_rpc_core::generated::LAUNCH_SCHEMA_VERSION
        || manifest.storage_generation != STORAGE_GENERATION
        || manifest.asset.name != expected_name
        || manifest.asset.size == 0
        || manifest.asset.size > MAX_RUNTIME_BYTES
        || !valid_sha256(&manifest.asset.sha256)
        || validate_asset_url(&manifest.asset).is_err()
    {
        return Err(RuntimeUpdateError::verification());
    }
    Ok(())
}

fn valid_build_revision(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_asset_url(asset: &RuntimeAsset) -> Result<Url, RuntimeUpdateError> {
    let url = Url::parse(&asset.url).map_err(|_| RuntimeUpdateError::verification())?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.path().starts_with(RELEASE_REPOSITORY_PREFIX)
        || !url.path().ends_with(&format!("/{}", asset.name))
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(RuntimeUpdateError::verification());
    }
    Ok(url)
}

fn installation_id(candidate_id: &str) -> Result<String, RuntimeUpdateError> {
    candidate_id
        .strip_prefix("sha256:")
        .filter(|digest| digest.len() == 64 && valid_sha256(candidate_id))
        .map(|digest| format!("sha256-{digest}"))
        .ok_or_else(RuntimeUpdateError::verification)
}

const fn runtime_executable_name() -> &'static str {
    if cfg!(windows) {
        "starweaver-rpc.exe"
    } else {
        "starweaver-rpc"
    }
}

async fn probe_candidate(
    manifest: &RuntimeUpdateManifest,
    runtime_path: &Path,
) -> Result<(), RuntimeUpdateError> {
    let temp = tempfile::tempdir().map_err(|_| RuntimeUpdateError::storage())?;
    let spec = managed_runtime::prepare_candidate_from_paths(
        runtime_path,
        temp.path(),
        &temp.path().join("managed"),
        managed_runtime::RuntimeCandidateIdentity {
            version: &manifest.version,
            build_revision: &manifest.build_revision,
            target: &manifest.rust_target,
            digest: &manifest.asset.sha256,
            size: manifest.asset.size,
            source: RuntimeLaunchSource::Managed,
        },
    )
    .map_err(|_| {
        RuntimeUpdateError::new(
            RuntimeUpdateErrorCode::Probe,
            "the runtime update failed its isolated compatibility probe",
        )
    })?;
    let supervisor = LocalHostSupervisor::default();
    supervisor
        .configure_storage_root(temp.path().join("supervisor"))
        .map_err(|_| {
            RuntimeUpdateError::new(
                RuntimeUpdateErrorCode::Probe,
                "the runtime update failed its isolated compatibility probe",
            )
        })?;
    async {
        supervisor.start(spec).await?;
        supervisor.shutdown().await
    }
    .await
    .map_err(|_: SupervisorError| {
        RuntimeUpdateError::new(
            RuntimeUpdateErrorCode::Probe,
            "the runtime update failed its isolated compatibility probe",
        )
    })
}

fn replace_runtime_pointer(
    root: &Path,
    pointer: &RuntimePointer,
) -> Result<(), RuntimeUpdateError> {
    let mut selection = read_selection_or_default(root)?;
    selection.previous = selection.current.replace(pointer.clone());
    atomic_json(&root.join("selection.json"), &selection)
}

fn read_selection_or_default(root: &Path) -> Result<RuntimeSelectionRecord, RuntimeUpdateError> {
    let path = root.join("selection.json");
    if path.exists() {
        read_selection(&path)
    } else {
        Ok(RuntimeSelectionRecord::default())
    }
}

fn read_selection(path: &Path) -> Result<RuntimeSelectionRecord, RuntimeUpdateError> {
    let bytes = read_bounded_file(path, 16 * 1024)?;
    let selection: RuntimeSelectionRecord =
        serde_json::from_slice(&bytes).map_err(|_| RuntimeUpdateError::storage())?;
    if selection.schema_version != SELECTION_SCHEMA_VERSION
        || selection
            .current
            .as_ref()
            .is_some_and(|value| !valid_pointer(value))
        || selection
            .previous
            .as_ref()
            .is_some_and(|value| !valid_pointer(value))
    {
        return Err(RuntimeUpdateError::storage());
    }
    Ok(selection)
}

fn valid_pointer(pointer: &RuntimePointer) -> bool {
    let valid_installation_id =
        pointer
            .installation_id
            .strip_prefix("sha256-")
            .is_some_and(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
    pointer.schema_version == POINTER_SCHEMA_VERSION
        && valid_installation_id
        && valid_sha256(&pointer.manifest_digest)
}

fn read_bounded_file(path: &Path, maximum: usize) -> Result<Vec<u8>, RuntimeUpdateError> {
    let file = File::open(path).map_err(|_| RuntimeUpdateError::storage())?;
    let metadata = file.metadata().map_err(|_| RuntimeUpdateError::storage())?;
    if !metadata.is_file() || metadata.len() > maximum as u64 {
        return Err(RuntimeUpdateError::storage());
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| RuntimeUpdateError::storage())?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take((maximum as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| RuntimeUpdateError::storage())?;
    if bytes.len() > maximum {
        return Err(RuntimeUpdateError::storage());
    }
    Ok(bytes)
}

fn create_private_directory(path: &Path) -> Result<(), RuntimeUpdateError> {
    fs::create_dir_all(path).map_err(|_| RuntimeUpdateError::storage())?;
    #[cfg(unix)]
    fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .map_err(|_| RuntimeUpdateError::storage())?;
    Ok(())
}

fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), RuntimeUpdateError> {
    let bytes = serde_json::to_vec(value).map_err(|_| RuntimeUpdateError::storage())?;
    atomic_write(path, &bytes, 0o600)
}

fn atomic_write(path: &Path, bytes: &[u8], unix_mode: u32) -> Result<(), RuntimeUpdateError> {
    let parent = path.parent().ok_or_else(RuntimeUpdateError::storage)?;
    create_private_directory(parent)?;
    let temporary = parent.join(format!(".update-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(unix_mode);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        atomic_replace(&temporary, path)?;
        sync_parent_io(parent)?;
        Ok::<(), std::io::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|_| RuntimeUpdateError::storage())
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    atomicwrites::replace_atomic(source, destination)
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

fn sync_parent(parent: &Path) -> Result<(), RuntimeUpdateError> {
    sync_parent_io(parent).map_err(|_| RuntimeUpdateError::storage())
}

#[cfg(unix)]
fn sync_parent_io(parent: &Path) -> std::io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_io(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

fn sha256_file_exact(path: &Path, expected_size: u64) -> Result<String, RuntimeUpdateError> {
    if expected_size == 0 || expected_size > MAX_RUNTIME_BYTES {
        return Err(RuntimeUpdateError::verification());
    }
    let file = File::open(path).map_err(|_| RuntimeUpdateError::storage())?;
    let metadata = file.metadata().map_err(|_| RuntimeUpdateError::storage())?;
    if !metadata.is_file() || metadata.len() != expected_size {
        return Err(RuntimeUpdateError::verification());
    }
    let mut reader = file.take(expected_size.saturating_add(1));
    let mut received = 0_u64;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| RuntimeUpdateError::storage())?;
        if read == 0 {
            break;
        }
        received = received
            .checked_add(u64::try_from(read).map_err(|_| RuntimeUpdateError::verification())?)
            .ok_or_else(RuntimeUpdateError::verification)?;
        hasher.update(&buffer[..read]);
    }
    if received != expected_size {
        return Err(RuntimeUpdateError::verification());
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    fn manifest() -> RuntimeUpdateManifest {
        let target = env!("STARWEAVER_TARGET_TRIPLE");
        RuntimeUpdateManifest {
            schema_version: 1,
            version: "1.2.4".to_string(),
            build_revision: "0123456789abcdef0123456789abcdef01234567".to_string(),
            rust_target: target.to_string(),
            desktop_version_requirement: ">=1.2.3, <1.3.0".to_string(),
            protocol: RuntimeProtocolIdentity {
                name: starweaver_rpc_core::generated::PROTOCOL_NAME.to_string(),
                major: starweaver_rpc_core::generated::PROTOCOL_MAJOR,
                revision: starweaver_rpc_core::generated::PROTOCOL_REVISION.to_string(),
                schema_digest: starweaver_rpc_core::generated::SCHEMA_DIGEST.to_string(),
            },
            launch_schema_version: starweaver_rpc_core::generated::LAUNCH_SCHEMA_VERSION,
            storage_generation: 1,
            asset: RuntimeAsset {
                name: format!(
                    "starweaver-rpc-v1.2.4-{target}{}",
                    if cfg!(windows) { ".exe" } else { "" }
                ),
                url: format!(
                    "https://github.com/Wh1isper/starweaver/releases/download/v1.2.4/starweaver-rpc-v1.2.4-{target}{}",
                    if cfg!(windows) { ".exe" } else { "" }
                ),
                size: 42,
                sha256: format!("sha256:{}", "a".repeat(64)),
            },
        }
    }

    #[test]
    fn configured_tauri_public_key_decodes_for_runtime_manifests() {
        if option_env!("STARWEAVER_UPDATE_PUBLIC_KEY").is_some() {
            assert!(update_public_key().is_ok());
        }
    }

    #[test]
    fn verifies_tauri_encoded_runtime_manifest_signature() {
        const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEY3NjkyNTVFNjYyRDFEOEIKUldTTEhTMW1YaVZwOS9SZkVNeWtlYUREWjFKTy9mSmFGZXV1SHFNVUJaRUxHQnJlTGFSeERjYmoK";
        const SIGNATURE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTTEhTMW1YaVZwOXphZCt0aTY4ZldUOElERUpxcXQ4ck0xakNOVFdubEJyeTRmNGRhdE9XZU5aMEJKdmJhRXZlNytGMVI5M1RzK0FxTld1WjRYV1BNU1c0Mk1GWWFnV1FZPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzg1MTMwNDUzCWZpbGU6c2lnbmF0dXJlLWZpeHR1cmUudHh0CnY3eENHNHQ5cS9pdE95TEppMzNuN1RvS3V2Y1UxN2w3YlVxamNlSGNuNW9EaGtGa0xPalNndUNDbFFBWDIzaGJMMlRRYmx5ZzhXNW0wTHluYlNSUURnPT0K";
        let key = parse_public_key(PUBLIC_KEY).expect("Tauri public key");
        verify_signature(
            &key,
            b"starweaver runtime update signature fixture\n",
            SIGNATURE,
        )
        .expect("Tauri signature");
    }

    #[test]
    fn restart_requirement_is_derived_from_the_complete_ready_and_next_start_identity() {
        let selected_digest = format!("sha256:{}", "a".repeat(64));
        let snapshot = RuntimeUpdateSnapshot {
            configured: true,
            active_version: None,
            active_source: None,
            selected_version: "1.2.4".to_string(),
            selected_source: RuntimeSelectionSource::Managed,
            candidate: None,
            restart_required: true,
            selected_digest: Some(selected_digest.clone()),
        };
        let identity = |version: &str, source, digest: &str| ReadyRuntimeIdentity {
            version: version.to_string(),
            source,
            digest: digest.to_string(),
        };

        let same = snapshot.clone().with_active_runtime(Some(identity(
            "1.2.4",
            RuntimeLaunchSource::Managed,
            &selected_digest,
        )));
        assert_eq!(same.active_version.as_deref(), Some("1.2.4"));
        assert_eq!(same.active_source, Some(RuntimeSelectionSource::Managed));
        assert!(!same.restart_required);

        let changed_version = snapshot.clone().with_active_runtime(Some(identity(
            "1.2.3",
            RuntimeLaunchSource::Managed,
            &selected_digest,
        )));
        assert!(changed_version.restart_required);

        let changed_source = snapshot.clone().with_active_runtime(Some(identity(
            "1.2.4",
            RuntimeLaunchSource::Bundled,
            &selected_digest,
        )));
        assert!(changed_source.restart_required);

        let changed_digest = snapshot.clone().with_active_runtime(Some(identity(
            "1.2.4",
            RuntimeLaunchSource::Managed,
            &format!("sha256:{}", "b".repeat(64)),
        )));
        assert!(changed_digest.restart_required);

        let not_ready = snapshot.with_active_runtime(None);
        assert_eq!(not_ready.active_version, None);
        assert_eq!(not_ready.active_source, None);
        assert!(!not_ready.restart_required);
    }

    #[test]
    fn accepts_only_the_exact_no_schema_change_contract() {
        let manifest = manifest();
        assert!(validate_manifest(&manifest, "1.2.3", env!("STARWEAVER_TARGET_TRIPLE")).is_ok());

        let mut wrong_protocol = manifest.clone();
        wrong_protocol.protocol.revision = "ordered-looking-but-wrong".to_string();
        assert!(
            validate_manifest(&wrong_protocol, "1.2.3", env!("STARWEAVER_TARGET_TRIPLE")).is_err()
        );

        let mut wrong_storage = manifest;
        wrong_storage.storage_generation = 2;
        assert!(
            validate_manifest(&wrong_storage, "1.2.3", env!("STARWEAVER_TARGET_TRIPLE")).is_err()
        );
    }

    #[test]
    fn rejects_asset_authority_outside_the_fixed_repository() {
        let mut manifest = manifest();
        manifest.asset.url = "https://example.com/starweaver-rpc".to_string();
        assert!(validate_manifest(&manifest, "1.2.3", env!("STARWEAVER_TARGET_TRIPLE")).is_err());
    }

    fn pointer(fill: char) -> RuntimePointer {
        RuntimePointer {
            schema_version: 1,
            installation_id: format!("sha256-{}", fill.to_string().repeat(64)),
            manifest_digest: format!("sha256:{}", fill.to_string().repeat(64)),
        }
    }

    #[test]
    fn local_update_files_are_read_with_a_hard_size_bound() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let path = temp.path().join("bounded.bin");
        fs::write(&path, b"1234").expect("bounded fixture");
        assert_eq!(read_bounded_file(&path, 4).expect("bounded read"), b"1234");

        fs::write(&path, b"12345").expect("oversized fixture");
        assert!(read_bounded_file(&path, 4).is_err());
    }

    #[test]
    fn runtime_selection_is_one_bounded_path_free_record() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let selection = RuntimeSelectionRecord {
            schema_version: 1,
            current: Some(pointer('b')),
            previous: Some(pointer('c')),
        };
        atomic_json(&temp.path().join("selection.json"), &selection).expect("selection");
        assert_eq!(
            read_selection(&temp.path().join("selection.json")).expect("read"),
            selection
        );
    }

    #[tokio::test]
    async fn rollback_swaps_the_selection_pair_with_one_atomic_record() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let root = temp.path().join("runtime");
        let manager = RuntimeUpdateManager::default();
        manager.configure(root.clone()).expect("configure");
        let selection = RuntimeSelectionRecord {
            schema_version: 1,
            current: Some(pointer('b')),
            previous: Some(pointer('c')),
        };
        atomic_json(&root.join("selection.json"), &selection).expect("selection");
        manager.rollback("1.2.3").await.expect("rollback");
        let rolled_back = read_selection(&root.join("selection.json")).expect("read");
        assert_eq!(rolled_back.current, selection.previous);
        assert_eq!(rolled_back.previous, selection.current);
    }
}
