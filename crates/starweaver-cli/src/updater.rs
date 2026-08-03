//! Component-aware GitHub Release discovery and native installation.

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    fs::OpenOptions,
    io::Cursor,
    path::{Component as PathComponent, Path, PathBuf},
    time::Duration,
};

use flate2::read::GzDecoder;
use fs2::FileExt as _;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{CliError, CliResult, build_info, error::io_error};

const DEFAULT_REPO: &str = "Wh1isper/starweaver";
const MANIFEST_NAME: &str = "starweaver-release.json";
const API_PAGE_SIZE: usize = 100;
const MAX_API_PAGES: usize = 10;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_ASSET_BYTES: u64 = 512 * 1024 * 1024;

/// Independently installable binary component.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum UpdateComponent {
    Cli,
    ComputerUse,
}

impl UpdateComponent {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::ComputerUse => "computer-use",
        }
    }

    const fn tag_prefix(self) -> &'static str {
        match self {
            Self::Cli => "cli-v",
            Self::ComputerUse => "computer-use-v",
        }
    }

    const fn binary_names(self) -> &'static [&'static str] {
        match self {
            Self::Cli => &["starweaver", "starweaver-cli", "sw", "starweaver-rpc"],
            Self::ComputerUse => &["starweaver-computer-use-mcp"],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ComponentRelease {
    pub(crate) component: UpdateComponent,
    pub(crate) tag: String,
    pub(crate) version: String,
    pub(crate) source_revision: String,
    pub(crate) asset: ReleaseAsset,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InstalledComponentState {
    schema_version: u32,
    version: String,
    tag: String,
    source_revision: String,
    target: String,
}

pub(crate) struct InstallLock {
    _file: fs::File,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseAsset {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) target: Option<String>,
    pub(crate) kind: String,
    pub(crate) size: u64,
    pub(crate) sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseManifest {
    schema_version: u32,
    release: ReleaseIdentity,
    components: BTreeMap<String, ReleaseComponentManifest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseIdentity {
    scope: String,
    version: String,
    tag: String,
    channel: String,
    source_revision: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseComponentManifest {
    version: String,
    #[serde(default)]
    registries: Vec<String>,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReleaseCandidate {
    tag: String,
    version: Version,
    component_specific: bool,
}

pub(crate) fn latest_component_release(
    component: UpdateComponent,
    channel: &str,
) -> CliResult<(String, String)> {
    validate_channel(channel)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::Run(error.to_string()))?;
    let repo = release_repo();
    let client = github_client()?;
    let candidate = runtime.block_on(async {
        let releases = fetch_releases(&client, &repo).await?;
        latest_candidate(component, channel, &releases)
    })?;
    let url = format!("https://github.com/{repo}/releases/tag/{}", candidate.tag);
    Ok((candidate.version.to_string(), url))
}

/// Resolve exact immutable releases for selected components.
pub(crate) fn resolve_releases(
    components: &[UpdateComponent],
    channel: &str,
    requested: Option<&str>,
) -> CliResult<Vec<ComponentRelease>> {
    validate_channel(channel)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::Run(error.to_string()))?;
    runtime.block_on(resolve_releases_async(components, channel, requested))
}

async fn resolve_releases_async(
    components: &[UpdateComponent],
    channel: &str,
    requested: Option<&str>,
) -> CliResult<Vec<ComponentRelease>> {
    let repo = release_repo();
    let client = github_client()?;
    let available = if requested.is_some() {
        Vec::new()
    } else {
        fetch_releases(&client, &repo).await?
    };
    let mut resolved = Vec::with_capacity(components.len());
    for component in components {
        let candidate = if let Some(requested) = requested {
            explicit_candidate(*component, requested)?
        } else {
            latest_candidate(*component, channel, &available)?
        };
        let manifest = fetch_manifest(&client, &repo, &candidate.tag).await?;
        resolved.push(select_component_asset(*component, &candidate, &manifest)?);
    }
    Ok(resolved)
}

fn github_client() -> CliResult<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("starweaver-cli")
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| CliError::Run(error.to_string()))
}

async fn fetch_releases(client: &reqwest::Client, repo: &str) -> CliResult<Vec<GitHubRelease>> {
    let mut releases = Vec::new();
    for page in 1..=MAX_API_PAGES {
        let url = format!(
            "https://api.github.com/repos/{repo}/releases?per_page={API_PAGE_SIZE}&page={page}"
        );
        let response = client
            .get(&url)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|error| CliError::Run(format!("failed to list GitHub Releases: {error}")))?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|error| CliError::Run(error.to_string()))?;
        if !status.is_success() {
            return Err(CliError::Run(format!(
                "GitHub Releases API returned {status}: {}",
                String::from_utf8_lossy(&body)
            )));
        }
        let page_releases: Vec<GitHubRelease> =
            serde_json::from_slice(&body).map_err(CliError::from)?;
        let complete = page_releases.len() < API_PAGE_SIZE;
        releases.extend(page_releases);
        if complete {
            return Ok(releases);
        }
    }
    Err(CliError::Run(format!(
        "GitHub Releases API exceeded the {MAX_API_PAGES}-page safety limit"
    )))
}

async fn fetch_manifest(
    client: &reqwest::Client,
    repo: &str,
    tag: &str,
) -> CliResult<ReleaseManifest> {
    let url = format!("https://github.com/{repo}/releases/download/{tag}/{MANIFEST_NAME}");
    let mut response = client
        .get(&url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|error| CliError::Run(format!("failed to download {url}: {error}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(CliError::Run(format!(
            "release {tag} has no usable {MANIFEST_NAME}: HTTP {status}"
        )));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_MANIFEST_BYTES as u64)
    {
        return Err(CliError::Run(format!(
            "release {tag} {MANIFEST_NAME} exceeds the {MAX_MANIFEST_BYTES}-byte size limit"
        )));
    }
    let mut body = Vec::with_capacity(
        response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or(0),
    );
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| CliError::Run(format!("failed to download {url}: {error}")))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_MANIFEST_BYTES {
            return Err(CliError::Run(format!(
                "release {tag} {MANIFEST_NAME} exceeds the {MAX_MANIFEST_BYTES}-byte size limit"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(CliError::from)
}

fn explicit_candidate(component: UpdateComponent, requested: &str) -> CliResult<ReleaseCandidate> {
    let requested = requested.trim();
    if requested.is_empty() || requested == "latest" {
        return Err(CliError::Usage(
            "explicit update version must not be empty or latest".to_string(),
        ));
    }
    if let Some(candidate) = candidate_from_tag(component, requested) {
        return Ok(candidate);
    }
    let version = Version::parse(requested.trim_start_matches('v')).map_err(|error| {
        CliError::Usage(format!("invalid requested release {requested}: {error}"))
    })?;
    Ok(ReleaseCandidate {
        tag: format!("v{version}"),
        version,
        component_specific: false,
    })
}

fn latest_candidate(
    component: UpdateComponent,
    channel: &str,
    releases: &[GitHubRelease],
) -> CliResult<ReleaseCandidate> {
    releases
        .iter()
        .filter(|release| !release.draft)
        .filter(|release| channel != "stable" || !release.prerelease)
        .filter_map(|release| {
            let candidate = candidate_from_tag(component, &release.tag_name)?;
            let tag_prerelease = !candidate.version.pre.is_empty();
            if release.prerelease != tag_prerelease {
                return None;
            }
            Some(candidate)
        })
        .max_by(|left, right| {
            left.version
                .cmp(&right.version)
                .then(left.component_specific.cmp(&right.component_specific))
        })
        .ok_or_else(|| {
            CliError::NotFound(format!(
                "no {channel} {} release was found",
                component.name()
            ))
        })
}

fn candidate_from_tag(component: UpdateComponent, tag: &str) -> Option<ReleaseCandidate> {
    let (version, component_specific) =
        if let Some(version) = tag.strip_prefix(component.tag_prefix()) {
            (version, true)
        } else {
            let version = tag.strip_prefix('v')?;
            (version, false)
        };
    let version = Version::parse(version).ok()?;
    if !version.build.is_empty() || version.to_string() != tag_version(tag, component)? {
        return None;
    }
    Some(ReleaseCandidate {
        tag: tag.to_string(),
        version,
        component_specific,
    })
}

fn tag_version(tag: &str, component: UpdateComponent) -> Option<String> {
    tag.strip_prefix(component.tag_prefix())
        .or_else(|| tag.strip_prefix('v'))
        .map(ToString::to_string)
}

fn select_component_asset(
    component: UpdateComponent,
    candidate: &ReleaseCandidate,
    manifest: &ReleaseManifest,
) -> CliResult<ComponentRelease> {
    validate_manifest_identity(manifest, candidate)?;
    let record = manifest.components.get(component.name()).ok_or_else(|| {
        CliError::Run(format!(
            "release {} does not contain component {}",
            candidate.tag,
            component.name()
        ))
    })?;
    if record.version != candidate.version.to_string() {
        return Err(CliError::Run(format!(
            "component {} version {} does not match release {}",
            component.name(),
            record.version,
            candidate.version
        )));
    }
    if !record.registries.is_empty() {
        return Err(CliError::Run(format!(
            "binary component {} unexpectedly declares registries",
            component.name()
        )));
    }
    let target = build_info::TARGET;
    let mut assets = record
        .assets
        .iter()
        .filter(|asset| asset.kind == "binary-archive" && asset.target.as_deref() == Some(target));
    let asset = assets.next().cloned().ok_or_else(|| {
        CliError::Unsupported(format!(
            "release {} has no {} binary for {target}",
            candidate.tag,
            component.name()
        ))
    })?;
    if assets.next().is_some() {
        return Err(CliError::Run(format!(
            "release {} contains duplicate {} assets for {target}",
            candidate.tag,
            component.name()
        )));
    }
    validate_asset_metadata(&asset)?;
    Ok(ComponentRelease {
        component,
        tag: candidate.tag.clone(),
        version: candidate.version.to_string(),
        source_revision: manifest.release.source_revision.clone(),
        asset,
    })
}

fn validate_manifest_identity(
    manifest: &ReleaseManifest,
    candidate: &ReleaseCandidate,
) -> CliResult<()> {
    if manifest.schema_version != 1 {
        return Err(CliError::Run(format!(
            "unsupported release manifest schema {}",
            manifest.schema_version
        )));
    }
    if manifest.release.tag != candidate.tag
        || manifest.release.version != candidate.version.to_string()
    {
        return Err(CliError::Run(
            "release manifest tag and version do not match the selected Release".to_string(),
        ));
    }
    let expected_scope = if candidate.component_specific {
        candidate
            .tag
            .strip_suffix(&format!("v{}", candidate.version))
            .unwrap_or_default()
            .trim_end_matches('-')
    } else {
        "full"
    };
    if manifest.release.scope != expected_scope {
        return Err(CliError::Run(format!(
            "release manifest scope {} does not match tag {}",
            manifest.release.scope, candidate.tag
        )));
    }
    let expected_channel = if candidate.version.pre.is_empty() {
        "stable"
    } else {
        "prerelease"
    };
    if manifest.release.channel != expected_channel {
        return Err(CliError::Run(
            "release manifest channel does not match version".to_string(),
        ));
    }
    if manifest.release.source_revision.len() != 40
        || !manifest
            .release
            .source_revision
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(CliError::Run(
            "release manifest contains an invalid source revision".to_string(),
        ));
    }
    Ok(())
}

fn validate_asset_metadata(asset: &ReleaseAsset) -> CliResult<()> {
    if asset.size == 0
        || asset.size > MAX_ASSET_BYTES
        || asset.sha256.len() != 64
        || !asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || asset.name.contains(['/', '\\'])
    {
        return Err(CliError::Run(format!(
            "release asset {} has invalid metadata",
            asset.name
        )));
    }
    Ok(())
}

fn validate_channel(channel: &str) -> CliResult<()> {
    match channel {
        "stable" | "beta" | "prerelease" => Ok(()),
        _ => Err(CliError::Config(format!(
            "unsupported update channel {channel}; expected stable or beta"
        ))),
    }
}

fn release_repo() -> String {
    env::var("STARWEAVER_GITHUB_REPO")
        .or_else(|_| env::var("STARWEAVER_REPO"))
        .unwrap_or_else(|_| DEFAULT_REPO.to_string())
}

/// Download and install all selected component releases.
pub(crate) fn install_releases(releases: &[ComponentRelease], install_dir: &Path) -> CliResult<()> {
    fs::create_dir_all(install_dir).map_err(|error| io_error(install_dir, error))?;
    let staging = tempfile::Builder::new()
        .prefix(".starweaver-update-")
        .tempdir_in(install_dir)
        .map_err(|error| io_error(install_dir, error))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::Run(error.to_string()))?;
    let client = github_client()?;
    let repo = release_repo();
    let mut prepared = Vec::with_capacity(releases.len());
    for release in releases {
        let bytes = runtime.block_on(download_asset(&client, &repo, release))?;
        let component_dir = staging.path().join(release.component.name());
        fs::create_dir_all(&component_dir).map_err(|error| io_error(&component_dir, error))?;
        extract_archive(&release.asset.name, &bytes, &component_dir)?;
        validate_extracted_component(release.component, &component_dir)?;
        prepared.push((release, component_dir));
    }
    install_prepared(&prepared, install_dir, staging.path())
}

async fn download_asset(
    client: &reqwest::Client,
    repo: &str,
    release: &ComponentRelease,
) -> CliResult<Vec<u8>> {
    let url = format!(
        "https://github.com/{repo}/releases/download/{}/{}",
        release.tag, release.asset.name
    );
    let mut response = client
        .get(&url)
        .timeout(Duration::from_mins(15))
        .send()
        .await
        .map_err(|error| CliError::Run(format!("failed to download {url}: {error}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(CliError::Run(format!(
            "release asset download returned {status}: {url}"
        )));
    }
    if response
        .content_length()
        .is_some_and(|length| length != release.asset.size)
    {
        return Err(CliError::Run(format!(
            "release asset size mismatch for {}",
            release.asset.name
        )));
    }
    let capacity = usize::try_from(release.asset.size)
        .map_err(|_| CliError::Run("release asset is too large for this platform".to_string()))?;
    let mut bytes = Vec::with_capacity(capacity);
    let mut hasher = Sha256::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| CliError::Run(format!("failed to download {url}: {error}")))?
    {
        if bytes.len().saturating_add(chunk.len()) > capacity {
            return Err(CliError::Run(format!(
                "release asset size mismatch for {}",
                release.asset.name
            )));
        }
        hasher.update(&chunk);
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() != capacity {
        return Err(CliError::Run(format!(
            "release asset size mismatch for {}",
            release.asset.name
        )));
    }
    let digest = format!("{:x}", hasher.finalize());
    if !digest.eq_ignore_ascii_case(&release.asset.sha256) {
        return Err(CliError::Run(format!(
            "release asset checksum mismatch for {}",
            release.asset.name
        )));
    }
    Ok(bytes)
}

fn extract_archive(name: &str, bytes: &[u8], output: &Path) -> CliResult<()> {
    if name.ends_with(".tar.gz") {
        let mut archive = tar::Archive::new(GzDecoder::new(Cursor::new(bytes)));
        let entries = archive
            .entries()
            .map_err(|error| CliError::Run(format!("invalid tar archive {name}: {error}")))?;
        for entry in entries {
            let mut entry = entry
                .map_err(|error| CliError::Run(format!("invalid tar archive {name}: {error}")))?;
            let path = entry
                .path()
                .map_err(|error| CliError::Run(format!("invalid tar path: {error}")))?;
            validate_archive_path(&path)?;
            if !(entry.header().entry_type().is_file() || entry.header().entry_type().is_dir()) {
                return Err(CliError::Run(format!(
                    "archive {name} contains a non-file entry"
                )));
            }
            entry
                .unpack_in(output)
                .map_err(|error| CliError::Run(format!("failed to extract {name}: {error}")))?;
        }
        return Ok(());
    }
    if Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|error| CliError::Run(format!("invalid zip archive {name}: {error}")))?;
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|error| CliError::Run(format!("invalid zip archive {name}: {error}")))?;
            let path = entry
                .enclosed_name()
                .ok_or_else(|| CliError::Run(format!("archive {name} contains an unsafe path")))?;
            validate_archive_path(&path)?;
            let destination = output.join(&path);
            if entry.is_dir() {
                fs::create_dir_all(&destination).map_err(|error| io_error(&destination, error))?;
                continue;
            }
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
            }
            let mut file =
                fs::File::create(&destination).map_err(|error| io_error(&destination, error))?;
            std::io::copy(&mut entry, &mut file).map_err(|error| io_error(&destination, error))?;
        }
        return Ok(());
    }
    Err(CliError::Run(format!("unsupported release archive {name}")))
}

fn validate_archive_path(path: &Path) -> CliResult<()> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, PathComponent::Normal(_) | PathComponent::CurDir))
    {
        return Err(CliError::Run(format!(
            "release archive contains unsafe path {}",
            path.display()
        )));
    }
    Ok(())
}

fn validate_extracted_component(component: UpdateComponent, root: &Path) -> CliResult<()> {
    let expected: BTreeSet<_> = component
        .binary_names()
        .iter()
        .map(|name| platform_binary_name(name))
        .collect();
    let actual: BTreeSet<_> = fs::read_dir(root)
        .map_err(|error| io_error(root, error))?
        .map(|entry| {
            let entry = entry.map_err(|error| io_error(root, error))?;
            if !entry
                .file_type()
                .map_err(|error| io_error(entry.path(), error))?
                .is_file()
            {
                return Err(CliError::Run(format!(
                    "release archive contains unexpected non-file entry {}",
                    entry.path().display()
                )));
            }
            entry.file_name().into_string().map_err(|_| {
                CliError::Run("release archive contains a non-UTF-8 file name".to_string())
            })
        })
        .collect::<CliResult<_>>()?;
    if actual != expected {
        return Err(CliError::Run(format!(
            "{} archive inventory mismatch: expected {expected:?}, got {actual:?}",
            component.name()
        )));
    }
    Ok(())
}

fn component_state_update(
    release: &ComponentRelease,
    install_dir: &Path,
) -> CliResult<(PathBuf, Vec<u8>)> {
    let state = InstalledComponentState {
        schema_version: 1,
        version: release.version.clone(),
        tag: release.tag.clone(),
        source_revision: release.source_revision.clone(),
        target: build_info::TARGET.to_string(),
    };
    let mut payload = serde_json::to_vec_pretty(&state).map_err(CliError::from)?;
    payload.push(b'\n');
    Ok((
        install_dir.join(format!(".starweaver-{}.version", release.component.name())),
        payload,
    ))
}

fn install_prepared(
    prepared: &[(&ComponentRelease, PathBuf)],
    install_dir: &Path,
    staging: &Path,
) -> CliResult<()> {
    let backup_dir = staging.join("backups");
    fs::create_dir_all(&backup_dir).map_err(|error| io_error(&backup_dir, error))?;
    let current_exe = env::current_exe()
        .ok()
        .and_then(|path| path.canonicalize().ok());
    let mut replacements = Vec::new();
    let mut state_updates = Vec::new();
    for (release, extracted) in prepared {
        for base in release.component.binary_names() {
            let name = platform_binary_name(base);
            replacements.push((extracted.join(&name), install_dir.join(&name), name));
        }
        state_updates.push(component_state_update(release, install_dir)?);
    }
    let pending_path = install_dir.join(".starweaver-update.pending");
    write_atomic(&pending_path, b"update in progress\n")?;
    let mut managed_paths = BTreeSet::new();
    let mut backups = BTreeMap::new();
    for (_, destination, name) in &replacements {
        managed_paths.insert(destination.clone());
        if destination.exists() {
            let backup = backup_dir.join(name);
            fs::copy(destination, &backup).map_err(|error| io_error(destination, error))?;
            backups.insert(destination.clone(), backup);
        }
    }
    for (state, _) in &state_updates {
        managed_paths.insert(state.clone());
        if state.exists() {
            let name = state.file_name().ok_or_else(|| {
                CliError::Run("component version state has no file name".to_string())
            })?;
            let backup = backup_dir.join(name);
            fs::copy(state, &backup).map_err(|error| io_error(state, error))?;
            backups.insert(state.clone(), backup);
        }
    }

    let self_index = replacements.iter().position(|(_, destination, _)| {
        current_exe.as_ref().is_some_and(|current| {
            destination
                .canonicalize()
                .ok()
                .as_ref()
                .is_some_and(|path| path == current)
        })
    });
    let result = (|| {
        for (index, (source, destination, name)) in replacements.iter().enumerate() {
            if Some(index) == self_index {
                continue;
            }
            replace_file(source, destination, name)?;
        }
        if let Some(index) = self_index {
            self_replace::self_replace(&replacements[index].0).map_err(|error| {
                CliError::Run(format!("failed to replace running binary: {error}"))
            })?;
        }
        for (state, value) in &state_updates {
            write_atomic(state, value)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut rollback_errors = Vec::new();
        for destination in managed_paths.iter().rev() {
            let rollback = backups.get(destination).map_or_else(
                || {
                    if destination.exists() {
                        fs::remove_file(destination)
                    } else {
                        Ok(())
                    }
                },
                |backup| fs::copy(backup, destination).map(|_| ()),
            );
            if let Err(rollback_error) = rollback {
                rollback_errors.push(format!("{}: {rollback_error}", destination.display()));
            }
        }
        if rollback_errors.is_empty() {
            return Err(error);
        }
        return Err(CliError::Run(format!(
            "{error}; rollback failed for {}",
            rollback_errors.join(", ")
        )));
    }
    fs::remove_file(&pending_path).map_err(|error| io_error(&pending_path, error))?;
    Ok(())
}

fn replace_file(source: &Path, destination: &Path, name: &str) -> CliResult<()> {
    let staged = destination.with_file_name(format!(".{name}.update.{}", std::process::id()));
    if staged.exists() {
        fs::remove_file(&staged).map_err(|error| io_error(&staged, error))?;
    }
    let result = (|| {
        fs::copy(source, &staged).map_err(|error| io_error(&staged, error))?;
        set_executable(&staged)?;
        if destination.exists() {
            fs::remove_file(destination).map_err(|error| io_error(destination, error))?;
        }
        fs::rename(&staged, destination).map_err(|error| io_error(destination, error))
    })();
    if result.is_err() && staged.exists() {
        let _ = fs::remove_file(&staged);
    }
    result
}

fn write_atomic(path: &Path, bytes: &[u8]) -> CliResult<()> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CliError::Run("update state path has no UTF-8 file name".to_string()))?;
    let staged = path.with_file_name(format!(".{name}.update.{}", std::process::id()));
    let result = (|| {
        fs::write(&staged, bytes).map_err(|error| io_error(&staged, error))?;
        if path.exists() {
            fs::remove_file(path).map_err(|error| io_error(path, error))?;
        }
        fs::rename(&staged, path).map_err(|error| io_error(path, error))
    })();
    if result.is_err() && staged.exists() {
        let _ = fs::remove_file(&staged);
    }
    result
}

#[cfg(unix)]
fn set_executable(path: &Path) -> CliResult<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .map_err(|error| io_error(path, error))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> CliResult<()> {
    Ok(())
}

fn platform_binary_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

pub(crate) fn acquire_install_lock(install_dir: &Path) -> CliResult<InstallLock> {
    fs::create_dir_all(install_dir).map_err(|error| io_error(install_dir, error))?;
    let path = install_dir.join(".starweaver-update.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| io_error(&path, error))?;
    file.lock_exclusive()
        .map_err(|error| io_error(&path, error))?;
    Ok(InstallLock { _file: file })
}

fn installed_state(
    component: UpdateComponent,
    install_dir: &Path,
) -> Option<InstalledComponentState> {
    let path = install_dir.join(format!(".starweaver-{}.version", component.name()));
    let payload = fs::read_to_string(path).ok()?;
    serde_json::from_str(&payload).ok()
}

pub(crate) fn installed_version(component: UpdateComponent, install_dir: &Path) -> Option<String> {
    let path = install_dir.join(format!(".starweaver-{}.version", component.name()));
    fs::read_to_string(path)
        .ok()
        .and_then(|payload| {
            serde_json::from_str::<InstalledComponentState>(&payload)
                .ok()
                .map(|state| state.version)
                .or_else(|| {
                    let version = payload.trim();
                    (!version.is_empty()).then(|| version.to_string())
                })
        })
        .or_else(|| (component == UpdateComponent::Cli).then(|| build_info::VERSION.to_string()))
}

pub(crate) fn installed_release_matches(release: &ComponentRelease, install_dir: &Path) -> bool {
    if install_dir.join(".starweaver-update.pending").exists() {
        return false;
    }
    installed_state(release.component, install_dir).is_some_and(|state| {
        state.schema_version == 1
            && state.version == release.version
            && state.tag == release.tag
            && state.source_revision == release.source_revision
            && state.target == build_info::TARGET
    })
}

pub(crate) fn component_is_complete(component: UpdateComponent, install_dir: &Path) -> bool {
    component
        .binary_names()
        .iter()
        .map(|name| install_dir.join(platform_binary_name(name)))
        .all(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn github_release(tag: &str, prerelease: bool) -> GitHubRelease {
        GitHubRelease {
            tag_name: tag.to_string(),
            draft: false,
            prerelease,
        }
    }

    #[test]
    fn component_discovery_uses_full_and_component_tags() {
        let releases = vec![
            github_release("v1.0.0", false),
            github_release("computer-use-v1.0.2", false),
            github_release("cli-v1.0.1", false),
            github_release("python-v9.0.0", false),
            github_release("cli-v1.1.0-beta.1", true),
        ];
        let cli = latest_candidate(UpdateComponent::Cli, "stable", &releases).unwrap();
        assert_eq!(cli.tag, "cli-v1.0.1");
        let computer_use =
            latest_candidate(UpdateComponent::ComputerUse, "stable", &releases).unwrap();
        assert_eq!(computer_use.tag, "computer-use-v1.0.2");
        let beta = latest_candidate(UpdateComponent::Cli, "beta", &releases).unwrap();
        assert_eq!(beta.tag, "cli-v1.1.0-beta.1");
    }

    #[test]
    fn component_discovery_rejects_noncanonical_and_mislabeled_tags() {
        let releases = vec![
            github_release("cli/v2.0.0", false),
            github_release("cli-v2.0.0+local", false),
            github_release("cli-v2.0.0-beta.1", false),
        ];
        assert!(latest_candidate(UpdateComponent::Cli, "stable", &releases).is_err());
    }

    #[test]
    fn explicit_versions_accept_full_or_matching_component_tags() {
        assert_eq!(
            explicit_candidate(UpdateComponent::Cli, "1.2.3")
                .unwrap()
                .tag,
            "v1.2.3"
        );
        assert_eq!(
            explicit_candidate(UpdateComponent::Cli, "cli-v1.2.4")
                .unwrap()
                .tag,
            "cli-v1.2.4"
        );
        assert!(explicit_candidate(UpdateComponent::ComputerUse, "cli-v1.2.4").is_err());
    }

    #[test]
    fn archive_paths_reject_traversal() {
        assert!(validate_archive_path(Path::new("starweaver")).is_ok());
        assert!(validate_archive_path(Path::new("./starweaver")).is_ok());
        assert!(validate_archive_path(Path::new("../starweaver")).is_err());
        assert!(validate_archive_path(Path::new("nested/starweaver")).is_ok());
    }

    #[test]
    fn manifest_identity_accepts_full_and_component_scopes() {
        for (component, tag, scope) in [
            (UpdateComponent::Cli, "v1.2.3", "full"),
            (UpdateComponent::Cli, "cli-v1.2.3", "cli"),
            (
                UpdateComponent::ComputerUse,
                "computer-use-v1.2.3",
                "computer-use",
            ),
        ] {
            let candidate = candidate_from_tag(component, tag).unwrap();
            let manifest = ReleaseManifest {
                schema_version: 1,
                release: ReleaseIdentity {
                    scope: scope.to_string(),
                    version: "1.2.3".to_string(),
                    tag: tag.to_string(),
                    channel: "stable".to_string(),
                    source_revision: "a".repeat(40),
                },
                components: BTreeMap::new(),
            };
            validate_manifest_identity(&manifest, &candidate).unwrap();
        }
    }

    #[test]
    fn prepared_component_install_records_version_and_inventory() {
        let temp = tempfile::tempdir().unwrap();
        let extracted = temp.path().join("extracted");
        let install = temp.path().join("install");
        fs::create_dir_all(&extracted).unwrap();
        fs::create_dir_all(&install).unwrap();
        for binary in UpdateComponent::ComputerUse.binary_names() {
            fs::write(extracted.join(platform_binary_name(binary)), b"new").unwrap();
        }
        let release = ComponentRelease {
            component: UpdateComponent::ComputerUse,
            tag: "computer-use-v1.2.3".to_string(),
            version: "1.2.3".to_string(),
            source_revision: "a".repeat(40),
            asset: ReleaseAsset {
                name: "asset.tar.gz".to_string(),
                target: Some(build_info::TARGET.to_string()),
                kind: "binary-archive".to_string(),
                size: 1,
                sha256: "0".repeat(64),
            },
        };
        install_prepared(&[(&release, extracted)], &install, temp.path()).unwrap();
        assert!(component_is_complete(
            UpdateComponent::ComputerUse,
            &install
        ));
        assert_eq!(
            installed_version(UpdateComponent::ComputerUse, &install).as_deref(),
            Some("1.2.3")
        );
        assert!(installed_release_matches(&release, &install));
        assert!(!install.join(".starweaver-update.pending").exists());
    }

    #[test]
    fn failed_component_install_restores_existing_files_and_removes_new_files() {
        let temp = tempfile::tempdir().unwrap();
        let extracted = temp.path().join("extracted");
        let install = temp.path().join("install");
        fs::create_dir_all(&extracted).unwrap();
        fs::create_dir_all(&install).unwrap();
        let first = platform_binary_name("starweaver");
        fs::write(extracted.join(&first), b"new").unwrap();
        fs::write(install.join(&first), b"old").unwrap();
        let release = ComponentRelease {
            component: UpdateComponent::Cli,
            tag: "cli-v1.2.3".to_string(),
            version: "1.2.3".to_string(),
            source_revision: "a".repeat(40),
            asset: ReleaseAsset {
                name: "asset.tar.gz".to_string(),
                target: Some(build_info::TARGET.to_string()),
                kind: "binary-archive".to_string(),
                size: 1,
                sha256: "0".repeat(64),
            },
        };

        assert!(install_prepared(&[(&release, extracted)], &install, temp.path()).is_err());
        assert_eq!(fs::read(install.join(first)).unwrap(), b"old");
        assert!(
            !install
                .join(platform_binary_name("starweaver-cli"))
                .exists()
        );
        assert!(!install.join(".starweaver-cli.version").exists());
        assert!(install.join(".starweaver-update.pending").is_file());
    }

    #[test]
    fn legacy_version_state_does_not_match_an_immutable_release() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join(".starweaver-computer-use.version"),
            "1.2.3\n",
        )
        .unwrap();
        let release = ComponentRelease {
            component: UpdateComponent::ComputerUse,
            tag: "computer-use-v1.2.3".to_string(),
            version: "1.2.3".to_string(),
            source_revision: "a".repeat(40),
            asset: ReleaseAsset {
                name: "asset.tar.gz".to_string(),
                target: Some(build_info::TARGET.to_string()),
                kind: "binary-archive".to_string(),
                size: 1,
                sha256: "0".repeat(64),
            },
        };
        assert_eq!(
            installed_version(UpdateComponent::ComputerUse, temp.path()).as_deref(),
            Some("1.2.3")
        );
        assert!(!installed_release_matches(&release, temp.path()));
        for name in UpdateComponent::ComputerUse.binary_names() {
            fs::create_dir(temp.path().join(platform_binary_name(name))).unwrap();
        }
        assert!(!component_is_complete(
            UpdateComponent::ComputerUse,
            temp.path()
        ));
    }
}
