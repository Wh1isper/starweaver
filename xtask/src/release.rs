use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::Read as _,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::common::{root, run_capture, run_command};

pub const DEVELOPMENT_VERSION: &str = "0.0.0-dev.0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ReleaseScope {
    Full,
    Cli,
    ComputerUse,
    Sdk,
    Python,
}

impl ReleaseScope {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "full" => Ok(Self::Full),
            "cli" => Ok(Self::Cli),
            "computer-use" => Ok(Self::ComputerUse),
            "sdk" => Ok(Self::Sdk),
            "python" => Ok(Self::Python),
            _ => Err(format!(
                "unknown release scope {value}; expected full, cli, computer-use, sdk, or python"
            )),
        }
    }

    const fn tag_prefix(self) -> &'static str {
        match self {
            Self::Full => "v",
            Self::Cli => "cli-v",
            Self::ComputerUse => "computer-use-v",
            Self::Sdk => "sdk-v",
            Self::Python => "python-v",
        }
    }

    const fn updates_workspace(self) -> bool {
        matches!(self, Self::Full | Self::Sdk)
    }

    const fn updates_python(self) -> bool {
        matches!(self, Self::Full | Self::Python)
    }
}

#[derive(Debug, Serialize)]
struct ParsedReleaseTag<'a> {
    scope: ReleaseScope,
    version: &'a str,
    tag: &'a str,
    channel: &'static str,
}

#[derive(Debug, Serialize)]
struct SemverCheckPlan {
    baseline_version: String,
    release_version: String,
    release_type: &'static str,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReleaseManifest {
    schema_version: u32,
    release: ReleaseIdentity,
    components: BTreeMap<String, ReleaseComponentManifest>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReleaseIdentity {
    scope: ReleaseScope,
    version: String,
    tag: String,
    channel: String,
    source_revision: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReleaseComponentManifest {
    version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    registries: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReleaseAsset {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    kind: String,
    size: u64,
    sha256: String,
}

const WORKSPACE_DEPENDENCIES: [&str; 19] = [
    "starweaver-agent",
    "starweaver-cli",
    "starweaver-computer-use",
    "starweaver-context",
    "starweaver-core",
    "starweaver-environment",
    "starweaver-envd",
    "starweaver-envd-client",
    "starweaver-envd-core",
    "starweaver-model",
    "starweaver-oauth",
    "starweaver-oauth-provider",
    "starweaver-runtime",
    "starweaver-rpc-core",
    "starweaver-session",
    "starweaver-storage",
    "starweaver-stream",
    "starweaver-tools",
    "starweaver-usage",
];
const NON_PUBLISH_WORKSPACE_CRATES: [&str; 1] = ["starweaver-rpc"];
const DRY_RUN_PACKAGES: [&str; 3] = ["starweaver-core", "starweaver-usage", "starweaver-oauth"];
const PUBLISH_PACKAGES: [&str; 19] = [
    "starweaver-core",
    "starweaver-computer-use",
    "starweaver-usage",
    "starweaver-oauth",
    "starweaver-model",
    "starweaver-context",
    "starweaver-tools",
    "starweaver-stream",
    "starweaver-envd-core",
    "starweaver-environment",
    "starweaver-envd-client",
    "starweaver-envd",
    "starweaver-session",
    "starweaver-runtime",
    "starweaver-rpc-core",
    "starweaver-oauth-provider",
    "starweaver-agent",
    "starweaver-storage",
    "starweaver-cli",
];

#[derive(Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
}

#[derive(Deserialize)]
struct CargoPackage {
    name: String,
    dependencies: Vec<CargoDependency>,
}

#[derive(Deserialize)]
struct CargoDependency {
    name: String,
    path: Option<String>,
}

pub fn release_tag(args: &[String]) -> Result<(), String> {
    if args.len() != 1 {
        return Err("usage: release-tag <tag>".to_string());
    }
    let parsed = parse_release_tag(&args[0])?;
    println!(
        "{}",
        serde_json::to_string(&parsed).map_err(|error| error.to_string())?
    );
    Ok(())
}

pub fn semver_check_plan(args: &[String]) -> Result<(), String> {
    if args.len() != 1 {
        return Err("usage: semver-check-plan <tag>".to_string());
    }
    let parsed = parse_release_tag(&args[0])?;
    let release_version = parse_publish_version(parsed.version)?;
    let repository = root()?;
    let baseline_version = read_last_verified_release(&repository)?;
    let plan = SemverCheckPlan {
        release_type: classify_semver_release_type(&baseline_version, &release_version)?,
        baseline_version: baseline_version.to_string(),
        release_version: release_version.to_string(),
    };
    println!(
        "{}",
        serde_json::to_string(&plan).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn read_last_verified_release(repository: &Path) -> Result<Version, String> {
    let path = repository.join("spec/capabilities.toml");
    let source =
        fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let document = source
        .parse::<toml::Value>()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let value = document
        .get("last_verified_release")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| format!("{} has no last_verified_release", path.display()))?;
    Version::parse(value).map_err(|error| {
        format!(
            "{} has invalid last_verified_release {value}: {error}",
            path.display()
        )
    })
}

fn classify_semver_release_type(
    baseline: &Version,
    release: &Version,
) -> Result<&'static str, String> {
    if release <= baseline {
        return Err(format!(
            "release version {release} must be newer than semver baseline {baseline}"
        ));
    }
    if release.major > baseline.major {
        Ok("major")
    } else if release.minor > baseline.minor {
        Ok("minor")
    } else {
        Ok("patch")
    }
}

pub fn release_manifest(args: &[String]) -> Result<(), String> {
    if args.len() != 6 {
        return Err(
            "usage: release-manifest <scope> <version> <tag> <source-revision> <assets-dir> <output>"
                .to_string(),
        );
    }
    let scope = ReleaseScope::parse(&args[0])?;
    let version = parse_publish_version(&args[1])?;
    let parsed_tag = parse_release_tag(&args[2])?;
    if parsed_tag.scope != scope || parsed_tag.version != version.to_string() {
        return Err(format!(
            "release tag {} does not match scope {} and version {}",
            args[2], args[0], args[1]
        ));
    }
    validate_source_revision(&args[3])?;
    let assets_dir = Path::new(&args[4]);
    let output = Path::new(&args[5]);
    let manifest = build_release_manifest(
        scope,
        &args[1],
        &args[2],
        parsed_tag.channel,
        &args[3],
        assets_dir,
    )?;
    validate_release_manifest(&manifest, Some(assets_dir))?;
    let payload = serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
    fs::write(output, payload).map_err(|error| format!("{}: {error}", output.display()))?;
    println!("Generated {}", output.display());
    Ok(())
}

pub fn release_manifest_verify(args: &[String]) -> Result<(), String> {
    if !(1..=2).contains(&args.len()) {
        return Err("usage: release-manifest-verify <manifest> [assets-dir]".to_string());
    }
    let path = Path::new(&args[0]);
    let payload = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let manifest: ReleaseManifest =
        serde_json::from_slice(&payload).map_err(|error| format!("{}: {error}", path.display()))?;
    let assets_dir = args.get(1).map(Path::new);
    validate_release_manifest(&manifest, assets_dir)?;
    println!("Release manifest validated: {}", path.display());
    Ok(())
}

fn build_release_manifest(
    scope: ReleaseScope,
    version: &str,
    tag: &str,
    channel: &str,
    source_revision: &str,
    assets_dir: &Path,
) -> Result<ReleaseManifest, String> {
    let mut components = release_components(scope, version);
    let mut paths = fs::read_dir(assets_dir)
        .map_err(|error| format!("{}: {error}", assets_dir.display()))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    for path in paths {
        if !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("release asset name is not UTF-8: {}", path.display()))?;
        if matches!(name, "checksums.txt" | "starweaver-release.json") {
            continue;
        }
        let (component, target, kind) = classify_release_asset(name, version)?;
        let record = components.get_mut(component).ok_or_else(|| {
            format!(
                "asset {name} belongs to component {component}, which is not part of this release"
            )
        })?;
        record.assets.push(ReleaseAsset {
            name: name.to_string(),
            target,
            kind: kind.to_string(),
            size: fs::metadata(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?
                .len(),
            sha256: sha256_file(&path)?,
        });
    }
    for component in components.values_mut() {
        component
            .assets
            .sort_by(|left, right| left.name.cmp(&right.name));
    }
    Ok(ReleaseManifest {
        schema_version: 1,
        release: ReleaseIdentity {
            scope,
            version: version.to_string(),
            tag: tag.to_string(),
            channel: channel.to_string(),
            source_revision: source_revision.to_string(),
        },
        components,
    })
}

fn release_components(
    scope: ReleaseScope,
    version: &str,
) -> BTreeMap<String, ReleaseComponentManifest> {
    let names: &[(&str, &[&str])] = match scope {
        ReleaseScope::Full => &[
            ("cli", &[]),
            ("computer-use", &[]),
            ("sdk", &["crates.io"]),
            ("python", &["PyPI"]),
        ],
        ReleaseScope::Cli => &[("cli", &[])],
        ReleaseScope::ComputerUse => &[("computer-use", &[])],
        ReleaseScope::Sdk => &[("sdk", &["crates.io"])],
        ReleaseScope::Python => &[("python", &["PyPI"])],
    };
    names
        .iter()
        .map(|(name, registries)| {
            (
                (*name).to_string(),
                ReleaseComponentManifest {
                    version: version.to_string(),
                    registries: registries
                        .iter()
                        .map(|value| (*value).to_string())
                        .collect(),
                    assets: Vec::new(),
                },
            )
        })
        .collect()
}

fn classify_release_asset(
    name: &str,
    version: &str,
) -> Result<(&'static str, Option<String>, &'static str), String> {
    let cli_prefix = format!("starweaver-cli-v{version}-");
    if let Some(target) = archive_target(name, &cli_prefix) {
        return Ok(("cli", Some(target), "binary-archive"));
    }
    let computer_use_prefix = format!("starweaver-computer-use-mcp-v{version}-");
    if let Some(target) = archive_target(name, &computer_use_prefix) {
        return Ok(("computer-use", Some(target), "binary-archive"));
    }
    if name.starts_with(&format!("starweaver-host-{version}."))
        || name == format!("starweaver-host-{version}-schemas.tar.gz")
    {
        return Ok(("cli", None, "protocol"));
    }
    if name.ends_with(".whl") || (name.starts_with("starweaver-") && name.ends_with(".tar.gz")) {
        return Ok(("python", None, "python-distribution"));
    }
    Err(format!("unrecognized release asset {name}"))
}

fn archive_target(name: &str, prefix: &str) -> Option<String> {
    let suffix = name.strip_prefix(prefix)?;
    let target = suffix
        .strip_suffix(".tar.gz")
        .or_else(|| suffix.strip_suffix(".zip"))?;
    (!target.is_empty()).then(|| target.to_string())
}

fn validate_release_manifest(
    manifest: &ReleaseManifest,
    assets_dir: Option<&Path>,
) -> Result<(), String> {
    if manifest.schema_version != 1 {
        return Err(format!(
            "unsupported release manifest schema version {}",
            manifest.schema_version
        ));
    }
    let parsed = parse_release_tag(&manifest.release.tag)?;
    if parsed.scope != manifest.release.scope || parsed.version != manifest.release.version {
        return Err("release manifest tag, scope, and version do not match".to_string());
    }
    if parsed.channel != manifest.release.channel {
        return Err("release manifest channel does not match tag prerelease state".to_string());
    }
    validate_source_revision(&manifest.release.source_revision)?;
    let expected = release_components(manifest.release.scope, &manifest.release.version);
    if expected.keys().collect::<Vec<_>>() != manifest.components.keys().collect::<Vec<_>>() {
        return Err("release manifest component inventory does not match scope".to_string());
    }
    let mut names = BTreeSet::new();
    for (name, component) in &manifest.components {
        if component.version != manifest.release.version {
            return Err(format!(
                "component {name} version {} does not match release version {}",
                component.version, manifest.release.version
            ));
        }
        validate_component_asset_inventory(name, component)?;
        for asset in &component.assets {
            if !names.insert(asset.name.as_str()) {
                return Err(format!("duplicate release asset {}", asset.name));
            }
            if asset.size == 0 || asset.sha256.len() != 64 {
                return Err(format!("invalid release asset metadata for {}", asset.name));
            }
            if let Some(assets_dir) = assets_dir {
                let path = assets_dir.join(&asset.name);
                let metadata =
                    fs::metadata(&path).map_err(|error| format!("{}: {error}", path.display()))?;
                if metadata.len() != asset.size || sha256_file(&path)? != asset.sha256 {
                    return Err(format!(
                        "release asset does not match manifest: {}",
                        asset.name
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_component_asset_inventory(
    name: &str,
    component: &ReleaseComponentManifest,
) -> Result<(), String> {
    let targets: BTreeSet<_> = component
        .assets
        .iter()
        .filter(|asset| asset.kind == "binary-archive")
        .filter_map(|asset| asset.target.as_deref())
        .collect();
    match name {
        "cli" => {
            let expected = BTreeSet::from([
                "aarch64-apple-darwin",
                "x86_64-apple-darwin",
                "x86_64-pc-windows-msvc",
                "x86_64-unknown-linux-gnu",
            ]);
            if targets != expected {
                return Err(format!(
                    "CLI release targets must be {expected:?}, got {targets:?}"
                ));
            }
            if component
                .assets
                .iter()
                .filter(|asset| asset.kind == "protocol")
                .count()
                != 3
            {
                return Err("CLI release must include three host protocol assets".to_string());
            }
        }
        "computer-use" => {
            let expected = BTreeSet::from(["aarch64-apple-darwin", "x86_64-apple-darwin"]);
            if targets != expected {
                return Err(format!(
                    "Computer Use release targets must be {expected:?}, got {targets:?}"
                ));
            }
        }
        "python" => {
            let distributions: Vec<_> = component
                .assets
                .iter()
                .filter(|asset| asset.kind == "python-distribution")
                .collect();
            if !distributions
                .iter()
                .any(|asset| asset.name.ends_with(".tar.gz"))
                || !distributions
                    .iter()
                    .any(|asset| asset.name.ends_with(".whl"))
            {
                return Err(
                    "Python release must include an sdist and at least one wheel".to_string(),
                );
            }
        }
        "sdk" => {
            if !component.assets.is_empty() {
                return Err("SDK registry release must not include downloadable assets".to_string());
            }
        }
        _ => return Err(format!("unknown release component {name}")),
    }
    Ok(())
}

fn validate_source_revision(revision: &str) -> Result<(), String> {
    if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("source revision must be a 40-character Git object ID".to_string());
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn release_prepare(args: &[String]) -> Result<(), String> {
    if args.len() != 2 {
        return Err("usage: release-prepare <scope> <version>".to_string());
    }
    let scope = ReleaseScope::parse(&args[0])?;
    parse_publish_version(&args[1])?;
    let version = args[1].as_str();
    let root = root()?;
    validate_release_package_lists(&root)?;
    ensure_development_versions(&root, scope)?;

    if scope.updates_workspace() {
        update_workspace_release_version(&root, version)?;
        if scope == ReleaseScope::Full {
            update_python_workspace_dependency_versions(&root, version)?;
        }
        crate::capabilities::update_verified_release(&root, version)?;
        crate::capabilities::check_at(&root, true)?;
        run_cargo_metadata(&root, None)?;
    }
    if scope.updates_python() {
        update_python_package_version(&root, version)?;
        run_cargo_metadata(&root, Some(&root.join("packages/starweaver-py/Cargo.toml")))?;
    }

    println!(
        "Prepared {} release {version} from development version {DEVELOPMENT_VERSION}",
        args[0]
    );
    Ok(())
}

pub fn upversion(args: &[String]) -> Result<(), String> {
    if args.len() != 1 {
        return Err("usage: upversion x.y.z".to_string());
    }
    release_prepare(&["full".to_string(), args[0].clone()])
}

fn parse_release_tag(tag: &str) -> Result<ParsedReleaseTag<'_>, String> {
    let (scope, version) = [
        (ReleaseScope::ComputerUse, ReleaseScope::ComputerUse.tag_prefix()),
        (ReleaseScope::Python, ReleaseScope::Python.tag_prefix()),
        (ReleaseScope::Cli, ReleaseScope::Cli.tag_prefix()),
        (ReleaseScope::Sdk, ReleaseScope::Sdk.tag_prefix()),
        (ReleaseScope::Full, ReleaseScope::Full.tag_prefix()),
    ]
    .into_iter()
    .find_map(|(scope, prefix)| tag.strip_prefix(prefix).map(|version| (scope, version)))
    .ok_or_else(|| {
        format!(
            "unsupported release tag {tag}; expected vX.Y.Z, cli-vX.Y.Z, computer-use-vX.Y.Z, sdk-vX.Y.Z, or python-vX.Y.Z"
        )
    })?;
    let parsed = parse_publish_version(version)?;
    if matches!(scope, ReleaseScope::Full | ReleaseScope::Python) {
        validate_python_release_version(&parsed)?;
    }
    if tag != format!("{}{parsed}", scope.tag_prefix()) {
        return Err(format!("release tag {tag} is not canonical"));
    }
    Ok(ParsedReleaseTag {
        scope,
        version,
        tag,
        channel: if parsed.pre.is_empty() {
            "stable"
        } else {
            "prerelease"
        },
    })
}

fn validate_python_release_version(version: &Version) -> Result<(), String> {
    if version.pre.is_empty() {
        return Ok(());
    }
    let identifiers = version.pre.as_str().split('.').collect::<Vec<_>>();
    if identifiers.len() == 2
        && matches!(identifiers[0], "alpha" | "beta" | "rc" | "dev")
        && !identifiers[1].is_empty()
        && identifiers[1].bytes().all(|byte| byte.is_ascii_digit())
    {
        return Ok(());
    }
    Err(format!(
        "Python release version {version} must use a PEP 440-compatible prerelease such as beta.1, rc.1, or dev.1"
    ))
}

fn parse_publish_version(version: &str) -> Result<Version, String> {
    let parsed = Version::parse(version)
        .map_err(|error| format!("invalid release version {version}: {error}"))?;
    if !parsed.build.is_empty() {
        return Err(format!(
            "release version {version} must not contain build metadata"
        ));
    }
    if parsed == Version::parse(DEVELOPMENT_VERSION).map_err(|error| error.to_string())? {
        return Err(format!(
            "release version must not equal development version {DEVELOPMENT_VERSION}"
        ));
    }
    Ok(parsed)
}

fn ensure_development_versions(root: &std::path::Path, scope: ReleaseScope) -> Result<(), String> {
    let actual = workspace_version_from_manifest(root)?;
    if actual != DEVELOPMENT_VERSION {
        return Err(format!(
            "workspace version must be {DEVELOPMENT_VERSION} before release preparation, got {actual}"
        ));
    }
    if scope.updates_python() {
        let manifest = root.join("packages/starweaver-py/pyproject.toml");
        let text = fs::read_to_string(&manifest).map_err(|error| error.to_string())?;
        let actual = toml_table_version(&text, "[project]\n")?;
        if actual != DEVELOPMENT_VERSION {
            return Err(format!(
                "Python package version must be {DEVELOPMENT_VERSION} before release preparation, got {actual}"
            ));
        }
    }
    Ok(())
}

fn update_workspace_release_version(root: &std::path::Path, version: &str) -> Result<(), String> {
    let manifest = root.join("Cargo.toml");
    let mut text = fs::read_to_string(&manifest).map_err(|error| error.to_string())?;
    text = replace_workspace_version(&text, version)?;
    for krate in WORKSPACE_DEPENDENCIES {
        text = replace_workspace_dependency_version(&text, krate, version)?;
    }
    fs::write(&manifest, text).map_err(|error| error.to_string())?;
    Ok(())
}

fn update_python_package_version(root: &std::path::Path, version: &str) -> Result<(), String> {
    let pyproject = root.join("packages/starweaver-py/pyproject.toml");
    let cargo_manifest = root.join("packages/starweaver-py/Cargo.toml");
    if !pyproject.exists() && !cargo_manifest.exists() {
        return Ok(());
    }
    update_toml_table_version(&pyproject, "[project]\n", version)?;
    update_toml_table_version(&cargo_manifest, "[package]\n", version)
}

fn update_python_workspace_dependency_versions(
    root: &std::path::Path,
    version: &str,
) -> Result<(), String> {
    let cargo_manifest = root.join("packages/starweaver-py/Cargo.toml");
    if !cargo_manifest.exists() {
        return Ok(());
    }
    let mut cargo_text = fs::read_to_string(&cargo_manifest).map_err(|error| error.to_string())?;
    for krate in python_package_workspace_dependencies(&cargo_text)? {
        cargo_text = replace_path_dependency_version(
            &cargo_text,
            &krate,
            &format!("../../crates/{krate}"),
            version,
        )?;
    }
    fs::write(&cargo_manifest, cargo_text).map_err(|error| error.to_string())?;
    Ok(())
}

fn run_cargo_metadata(
    root: &std::path::Path,
    manifest: Option<&std::path::Path>,
) -> Result<(), String> {
    let mut command = Command::new("cargo");
    command.arg("metadata").arg("--format-version").arg("1");
    if let Some(manifest) = manifest {
        command.arg("--manifest-path").arg(manifest);
    }
    command.current_dir(root).stdout(Stdio::null());
    run_command(&mut command)
}

fn update_toml_table_version(
    manifest: &std::path::Path,
    marker: &str,
    version: &str,
) -> Result<(), String> {
    let text = fs::read_to_string(manifest).map_err(|error| error.to_string())?;
    let updated = replace_toml_table_version(&text, marker, version)
        .map_err(|error| format!("{}: {error}", manifest.display()))?;
    fs::write(manifest, updated).map_err(|error| error.to_string())?;
    Ok(())
}

fn replace_toml_table_version(text: &str, marker: &str, version: &str) -> Result<String, String> {
    let start = text
        .find(marker)
        .ok_or_else(|| format!("missing {marker:?}"))?
        + marker.len();
    let after = &text[start..];
    let line_start = after
        .find("version = \"")
        .ok_or_else(|| "missing package version".to_string())?
        + start;
    let value_start = line_start + "version = \"".len();
    let value_end = text[value_start..]
        .find('"')
        .ok_or_else(|| "unterminated version".to_string())?
        + value_start;
    let mut output = String::new();
    output.push_str(&text[..value_start]);
    output.push_str(version);
    output.push_str(&text[value_end..]);
    Ok(output)
}

fn toml_table_version<'a>(text: &'a str, marker: &str) -> Result<&'a str, String> {
    let start = text
        .find(marker)
        .ok_or_else(|| format!("missing {marker:?}"))?
        + marker.len();
    let line_start = text[start..]
        .find("version = \"")
        .ok_or_else(|| "missing package version".to_string())?
        + start;
    let value_start = line_start + "version = \"".len();
    let value_end = text[value_start..]
        .find('"')
        .ok_or_else(|| "unterminated version".to_string())?
        + value_start;
    Ok(&text[value_start..value_end])
}

fn replace_workspace_version(text: &str, version: &str) -> Result<String, String> {
    let marker = "[workspace.package]\n";
    let start = text
        .find(marker)
        .ok_or_else(|| "missing [workspace.package]".to_string())?
        + marker.len();
    let after = &text[start..];
    let line_start = after
        .find("version = \"")
        .ok_or_else(|| "missing workspace package version".to_string())?
        + start;
    let value_start = line_start + "version = \"".len();
    let value_end = text[value_start..]
        .find('"')
        .ok_or_else(|| "unterminated version".to_string())?
        + value_start;
    let mut output = String::new();
    output.push_str(&text[..value_start]);
    output.push_str(version);
    output.push_str(&text[value_end..]);
    Ok(output)
}

fn replace_workspace_dependency_version(
    text: &str,
    krate: &str,
    version: &str,
) -> Result<String, String> {
    replace_path_dependency_version(text, krate, &format!("crates/{krate}"), version)
}

fn replace_path_dependency_version(
    text: &str,
    krate: &str,
    path: &str,
    version: &str,
) -> Result<String, String> {
    let needle = format!("{krate} = {{ path = \"{path}\", version = \"");
    let start = text
        .find(&needle)
        .ok_or_else(|| format!("missing path dependency {krate} at {path}"))?
        + needle.len();
    let end = text[start..]
        .find('"')
        .ok_or_else(|| format!("unterminated dependency version for {krate}"))?
        + start;
    let mut output = String::new();
    output.push_str(&text[..start]);
    output.push_str(version);
    output.push_str(&text[end..]);
    Ok(output)
}

fn python_package_workspace_dependencies(text: &str) -> Result<BTreeSet<String>, String> {
    let manifest: toml::Value = text
        .parse()
        .map_err(|error| format!("invalid Python package Cargo.toml: {error}"))?;
    let dependencies = manifest
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "missing Python package [dependencies]".to_string())?;
    let mut crates = BTreeSet::new();
    for (name, dependency) in dependencies {
        let Some(table) = dependency.as_table() else {
            continue;
        };
        let Some(path) = table.get("path").and_then(toml::Value::as_str) else {
            continue;
        };
        let Some(crate_name) = path.strip_prefix("../../crates/") else {
            continue;
        };
        if crate_name != name {
            return Err(format!(
                "Python package dependency {name} path points at crate {crate_name}"
            ));
        }
        if !table.contains_key("version") {
            return Err(format!(
                "Python package workspace dependency {name} must include a version"
            ));
        }
        crates.insert(name.clone());
    }
    Ok(crates)
}

pub fn workspace_version(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("workspace-version takes no arguments".to_string());
    }
    let root = root()?;
    println!("{}", workspace_version_from_manifest(&root)?);
    Ok(())
}

fn workspace_version_from_manifest(root: &std::path::Path) -> Result<String, String> {
    let manifest = root.join("Cargo.toml");
    let text = fs::read_to_string(&manifest).map_err(|error| error.to_string())?;
    workspace_version_from_manifest_text(&text)
}

fn workspace_version_from_manifest_text(text: &str) -> Result<String, String> {
    let marker = "[workspace.package]\n";
    let start = text
        .find(marker)
        .ok_or_else(|| "missing [workspace.package]".to_string())?
        + marker.len();
    let after = &text[start..];
    let line_start = after
        .find("version = \"")
        .ok_or_else(|| "missing workspace package version".to_string())?
        + start;
    let value_start = line_start + "version = \"".len();
    let value_end = text[value_start..]
        .find('"')
        .ok_or_else(|| "unterminated version".to_string())?
        + value_start;
    Ok(text[value_start..value_end].to_string())
}

pub fn publish_dry_run() -> Result<(), String> {
    let root = root()?;
    validate_release_package_lists(&root)?;
    for package in DRY_RUN_PACKAGES {
        println!("Dry-run publishing {package}");
        run_command(
            Command::new("cargo")
                .arg("publish")
                .arg("-p")
                .arg(package)
                .arg("--locked")
                .arg("--dry-run")
                .arg("--allow-dirty")
                .current_dir(&root),
        )?;
    }
    Ok(())
}

pub fn publish(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("publish takes no arguments".to_string());
    }
    let root = root()?;
    validate_release_package_lists(&root)?;
    validate_ephemeral_publish_checkout(&root)?;
    let retries = env::var("PUBLISH_RETRIES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(60);
    let delay = env::var("PUBLISH_RETRY_DELAY_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(60);
    let max_delay = env::var("PUBLISH_RETRY_MAX_DELAY_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(900);
    let version = workspace_version_from_manifest(&root)?;
    for package in PUBLISH_PACKAGES {
        if published_version_exists(&root, package, &version) {
            println!("Skipping {package} {version} because this version is already published");
            continue;
        }
        println!("Publishing {package}");
        let mut attempt = 1;
        loop {
            match run_capture(
                Command::new("cargo")
                    .arg("publish")
                    .arg("-p")
                    .arg(package)
                    .arg("--locked")
                    .arg("--allow-dirty")
                    .current_dir(&root),
            ) {
                Ok(output) => {
                    print!("{output}");
                    break;
                }
                Err(output) => {
                    print!("{output}");
                    let lower = output.to_ascii_lowercase();
                    if lower.contains("already uploaded") || lower.contains("already exists") {
                        println!("Skipping {package} because this version is already published");
                        break;
                    }
                    if attempt >= retries {
                        return Err(format!(
                            "Publishing {package} failed after {attempt} attempts"
                        ));
                    }
                    let retry_delay = publish_retry_delay_seconds(&output, delay, max_delay);
                    println!(
                        "Waiting {retry_delay}s before retrying {package} ({attempt}/{retries})"
                    );
                    attempt += 1;
                    std::thread::sleep(Duration::from_secs(retry_delay));
                }
            }
        }
    }
    Ok(())
}

fn validate_ephemeral_publish_checkout(root: &std::path::Path) -> Result<(), String> {
    let version = workspace_version_from_manifest(root)?;
    if version == DEVELOPMENT_VERSION {
        return Err(format!(
            "refusing to publish the development workspace version {DEVELOPMENT_VERSION}"
        ));
    }
    let output = run_capture(
        Command::new("git")
            .arg("status")
            .arg("--porcelain=v1")
            .arg("--untracked-files=all")
            .current_dir(root),
    )?;
    let actual = parse_dirty_paths(&output)?;
    let expected = BTreeSet::from([
        "Cargo.lock".to_string(),
        "Cargo.toml".to_string(),
        "spec/capabilities.toml".to_string(),
        "spec/capability-status.md".to_string(),
    ]);
    if actual != expected {
        return Err(format!(
            "ephemeral SDK publish checkout must contain only the reviewed release metadata changes; expected {expected:?}, got {actual:?}"
        ));
    }
    Ok(())
}

fn parse_dirty_paths(output: &str) -> Result<BTreeSet<String>, String> {
    output
        .lines()
        .map(|line| {
            if line.len() < 4 || line.as_bytes().get(2) != Some(&b' ') {
                return Err(format!("unexpected git status entry: {line}"));
            }
            let path = &line[3..];
            if path.is_empty() || path.starts_with('"') || path.contains(" -> ") {
                return Err(format!(
                    "unsupported dirty path in release checkout: {line}"
                ));
            }
            Ok(path.to_string())
        })
        .collect()
}

fn published_version_exists(root: &std::path::Path, package: &str, version: &str) -> bool {
    match run_capture(
        Command::new("cargo")
            .arg("info")
            .arg(format!("{package}@{version}"))
            .arg("--registry")
            .arg("crates-io")
            .arg("--quiet")
            .current_dir(root),
    ) {
        Ok(_) => true,
        Err(output) => {
            let lower = output.to_ascii_lowercase();
            if lower.contains("could not find")
                || lower.contains("no matching package")
                || lower.contains("not found")
            {
                return false;
            }
            println!(
                "Could not preflight crates.io version for {package} {version}; continuing with publish"
            );
            false
        }
    }
}

fn publish_retry_delay_seconds(output: &str, default_delay: u64, max_delay: u64) -> u64 {
    publish_retry_delay_seconds_at(output, default_delay, max_delay, SystemTime::now())
}

fn publish_retry_delay_seconds_at(
    output: &str,
    default_delay: u64,
    max_delay: u64,
    now: SystemTime,
) -> u64 {
    let delay = retry_after_delay_seconds(output, now).unwrap_or(default_delay);
    delay.clamp(1, max_delay.max(1))
}

fn retry_after_delay_seconds(output: &str, now: SystemTime) -> Option<u64> {
    for line in output.lines() {
        let lower = line.to_ascii_lowercase();
        let Some(index) = lower.find("retry-after") else {
            continue;
        };
        let value = line[index + "retry-after".len()..]
            .trim_start_matches([':', ' ', '\t'])
            .trim();
        if let Ok(seconds) = value.parse::<u64>() {
            return Some(seconds);
        }
        if let Some(retry_at) = parse_http_date(value) {
            return Some(
                retry_at
                    .duration_since(now)
                    .map_or(1, |duration| duration.as_secs().max(1)),
            );
        }
    }
    None
}

fn parse_http_date(value: &str) -> Option<SystemTime> {
    let (_, rest) = value.trim().split_once(',')?;
    let mut parts = rest.split_whitespace();
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year = parts.next()?.parse::<i32>().ok()?;
    let mut time = parts.next()?.split(':');
    let hour = time.next()?.parse::<u32>().ok()?;
    let minute = time.next()?.parse::<u32>().ok()?;
    let second = time.next()?.parse::<u32>().ok()?;
    if parts.next()? != "GMT" || parts.next().is_some() {
        return None;
    }
    let days = days_from_civil(year, month, day)?;
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600)?
        .checked_add(i64::from(minute) * 60)?
        .checked_add(i64::from(second))?;
    let seconds = u64::try_from(seconds).ok()?;
    Some(UNIX_EPOCH + Duration::from_secs(seconds))
}

fn days_from_civil(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i32::try_from(month).ok()?;
    let day = i32::try_from(day).ok()?;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    if !(0..=365).contains(&day_of_year) {
        return None;
    }
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(i64::from(era * 146_097 + day_of_era - 719_468))
}

fn validate_release_package_lists(root: &std::path::Path) -> Result<(), String> {
    ensure_unique("workspace dependency", &WORKSPACE_DEPENDENCIES)?;
    ensure_unique("non-publish workspace crate", &NON_PUBLISH_WORKSPACE_CRATES)?;
    ensure_unique("dry-run package", &DRY_RUN_PACKAGES)?;
    ensure_unique("publish package", &PUBLISH_PACKAGES)?;

    let publish_packages: BTreeSet<_> = PUBLISH_PACKAGES.iter().copied().collect();
    let workspace_dependencies: BTreeSet<_> = WORKSPACE_DEPENDENCIES.iter().copied().collect();
    let expected_workspace_dependencies = publish_packages.clone();
    if workspace_dependencies != expected_workspace_dependencies {
        return Err(format!(
            "workspace dependency list must match publish packages: expected {expected_workspace_dependencies:?}, got {workspace_dependencies:?}"
        ));
    }

    let dry_run_packages: BTreeSet<_> = DRY_RUN_PACKAGES.iter().copied().collect();
    if !dry_run_packages.is_subset(&publish_packages) {
        return Err(format!(
            "dry-run packages must be publish packages: got {dry_run_packages:?}"
        ));
    }

    let manifest = root.join("Cargo.toml");
    let manifest_text = fs::read_to_string(&manifest).map_err(|error| error.to_string())?;
    let manifest_value: toml::Value = manifest_text
        .parse()
        .map_err(|error| format!("{}: {error}", manifest.display()))?;
    let mut workspace_crates = workspace_crates_from_manifest(&manifest_value)?;
    for krate in NON_PUBLISH_WORKSPACE_CRATES {
        workspace_crates.remove(krate);
    }
    if workspace_crates != publish_packages {
        return Err(format!(
            "publish package list must match publishable crates/* workspace members: expected {workspace_crates:?}, got {publish_packages:?}"
        ));
    }
    let publish_dependencies = workspace_publish_dependencies(root, &publish_packages)?;
    validate_publish_dependency_order(&PUBLISH_PACKAGES, &publish_dependencies)?;
    for krate in WORKSPACE_DEPENDENCIES {
        let needle = format!("{krate} = {{ path = \"crates/{krate}\", version = \"");
        if !manifest_text.contains(&needle) {
            return Err(format!(
                "workspace dependency {krate} must use a path plus version entry"
            ));
        }
    }
    let python_manifest = root.join("packages/starweaver-py/Cargo.toml");
    if python_manifest.exists() {
        let python_manifest_text =
            fs::read_to_string(&python_manifest).map_err(|error| error.to_string())?;
        for krate in python_package_workspace_dependencies(&python_manifest_text)? {
            if !workspace_dependencies.contains(krate.as_str()) {
                return Err(format!(
                    "Python package workspace dependency {krate} must be a publishable workspace dependency"
                ));
            }
            let needle = format!("{krate} = {{ path = \"../../crates/{krate}\", version = \"");
            if !python_manifest_text.contains(&needle) {
                return Err(format!(
                    "Python package workspace dependency {krate} must use a path plus version entry"
                ));
            }
        }
    }
    Ok(())
}

fn workspace_publish_dependencies(
    root: &std::path::Path,
    publish_packages: &BTreeSet<&str>,
) -> Result<BTreeMap<String, BTreeSet<String>>, String> {
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .arg("--no-deps")
        .current_dir(root)
        .output()
        .map_err(|error| format!("failed to run cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let metadata: CargoMetadata = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("cargo metadata returned invalid JSON: {error}"))?;
    publish_dependencies_from_metadata(metadata, publish_packages)
}

fn publish_dependencies_from_metadata(
    metadata: CargoMetadata,
    publish_packages: &BTreeSet<&str>,
) -> Result<BTreeMap<String, BTreeSet<String>>, String> {
    let mut dependencies = BTreeMap::new();

    for package in metadata.packages {
        if !publish_packages.contains(package.name.as_str()) {
            continue;
        }
        let mut package_dependencies = BTreeSet::new();
        for dependency in package.dependencies {
            if dependency.path.is_none() {
                continue;
            }
            if !publish_packages.contains(dependency.name.as_str()) {
                return Err(format!(
                    "publish package {} has local dependency {} that is not in the publish package list",
                    package.name, dependency.name
                ));
            }
            package_dependencies.insert(dependency.name);
        }
        dependencies.insert(package.name, package_dependencies);
    }

    for package in publish_packages {
        if !dependencies.contains_key(*package) {
            return Err(format!(
                "cargo metadata did not return publish package {package}"
            ));
        }
    }
    Ok(dependencies)
}

fn validate_publish_dependency_order(
    publish_packages: &[&str],
    dependencies: &BTreeMap<String, BTreeSet<String>>,
) -> Result<(), String> {
    let positions: BTreeMap<_, _> = publish_packages
        .iter()
        .enumerate()
        .map(|(position, package)| (*package, position))
        .collect();

    for (package, package_dependencies) in dependencies {
        let package_position = positions
            .get(package.as_str())
            .ok_or_else(|| format!("missing publish package {package}"))?;
        for dependency in package_dependencies {
            let dependency_position = positions.get(dependency.as_str()).ok_or_else(|| {
                format!("publish package {package} depends on missing package {dependency}")
            })?;
            if dependency_position >= package_position {
                return Err(format!(
                    "publish package {package} must come after its workspace dependency {dependency}"
                ));
            }
        }
    }
    Ok(())
}

fn ensure_unique(label: &str, values: &[&str]) -> Result<(), String> {
    let set: BTreeSet<_> = values.iter().copied().collect();
    if set.len() != values.len() {
        return Err(format!("{label} list contains duplicate entries"));
    }
    Ok(())
}

fn workspace_crates_from_manifest(manifest: &toml::Value) -> Result<BTreeSet<&str>, String> {
    let members = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "missing workspace.members".to_string())?;
    let mut crates = BTreeSet::new();
    for member in members {
        let member = member
            .as_str()
            .ok_or_else(|| "workspace.members entries must be strings".to_string())?;
        if let Some(name) = member.strip_prefix("crates/") {
            crates.insert(name);
        }
    }
    Ok(crates)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_tags_define_component_scope_and_channel() {
        let cli = parse_release_tag("cli-v1.2.3").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(cli.scope, ReleaseScope::Cli);
        assert_eq!(cli.version, "1.2.3");
        assert_eq!(cli.channel, "stable");

        let prerelease =
            parse_release_tag("v2.0.0-beta.1").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(prerelease.scope, ReleaseScope::Full);
        assert_eq!(prerelease.channel, "prerelease");

        for invalid in [
            "release-v1.2.3",
            "cli/v1.2.3",
            "cli-v01.2.3",
            "computer-use-v0.0.0-dev.0",
            "sdk-v1.2.3+local",
            "python-v1.2.3-foo.1",
            "python-v1.2.3-a.1",
            "v1.2.3-b.1",
            "v1.2.3-preview.1",
        ] {
            assert!(parse_release_tag(invalid).is_err(), "accepted {invalid}");
        }
        assert!(parse_release_tag("python-v1.2.3-rc.1").is_ok());
        assert!(parse_release_tag("sdk-v1.2.3-foo.1").is_ok());
    }

    #[test]
    fn semver_check_plan_classifies_the_candidate_against_the_public_baseline() {
        let baseline = Version::parse("0.13.0").unwrap_or_else(|error| panic!("{error}"));
        for (candidate, expected) in [
            ("1.0.0", "major"),
            ("0.14.0", "minor"),
            ("0.13.1", "patch"),
            ("0.13.1-rc.1", "patch"),
        ] {
            let candidate = Version::parse(candidate).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(
                classify_semver_release_type(&baseline, &candidate),
                Ok(expected)
            );
        }
        let same = Version::parse("0.13.0").unwrap_or_else(|error| panic!("{error}"));
        let older = Version::parse("0.12.9").unwrap_or_else(|error| panic!("{error}"));
        assert!(classify_semver_release_type(&baseline, &same).is_err());
        assert!(classify_semver_release_type(&baseline, &older).is_err());
    }

    #[test]
    fn dirty_publish_paths_require_the_exact_ephemeral_sdk_overlay() {
        let parsed = parse_dirty_paths(
            " M Cargo.toml\n M Cargo.lock\n M spec/capabilities.toml\n M spec/capability-status.md\n",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(parsed.len(), 4);
        assert!(parse_dirty_paths("?? unexpected.txt\n").is_ok());
        assert!(parse_dirty_paths("R  old -> new\n").is_err());
    }

    #[test]
    fn release_manifest_builds_and_verifies_component_inventory() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let version = "1.2.3";
        for name in [
            "starweaver-cli-v1.2.3-x86_64-unknown-linux-gnu.tar.gz",
            "starweaver-cli-v1.2.3-x86_64-apple-darwin.tar.gz",
            "starweaver-cli-v1.2.3-aarch64-apple-darwin.tar.gz",
            "starweaver-cli-v1.2.3-x86_64-pc-windows-msvc.zip",
            "starweaver-host-1.2.3.openrpc.json",
            "starweaver-host-1.2.3.manifest.json",
            "starweaver-host-1.2.3-schemas.tar.gz",
        ] {
            fs::write(temp.path().join(name), name.as_bytes())
                .unwrap_or_else(|error| panic!("{error}"));
        }
        let manifest = build_release_manifest(
            ReleaseScope::Cli,
            version,
            "cli-v1.2.3",
            "stable",
            "0123456789012345678901234567890123456789",
            temp.path(),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        validate_release_manifest(&manifest, Some(temp.path()))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(manifest.components["cli"].assets.len(), 7);
    }

    #[test]
    fn workspace_version_parser_reads_workspace_package_version() {
        let text = r#"
[workspace]
members = ["crates/example"]

[workspace.package]
edition = "2024"
version = "1.2.3"
"#;
        let parsed = match workspace_version_from_manifest_text(text) {
            Ok(version) => version,
            Err(error) => panic!("workspace version should parse: {error}"),
        };
        assert_eq!(parsed, "1.2.3");
    }

    #[test]
    fn toml_table_version_replacer_updates_selected_table_version() {
        let text = r#"
[project]
name = "example"
version = "1.2.3"

[tool.example]
version = "9.9.9"
"#;
        let updated = match replace_toml_table_version(text, "[project]\n", "2.0.0") {
            Ok(updated) => updated,
            Err(error) => panic!("project version should update: {error}"),
        };
        assert!(updated.contains("version = \"2.0.0\""));
        assert!(updated.contains("version = \"9.9.9\""));
    }

    #[test]
    fn path_dependency_replacer_updates_selected_dependency_version() {
        let text = r#"
[dependencies]
starweaver-agent = { path = "../../crates/starweaver-agent", version = "0.2.1" }
starweaver-core = { path = "../../crates/starweaver-core", version = "0.2.1" }
"#;
        let updated = match replace_path_dependency_version(
            text,
            "starweaver-agent",
            "../../crates/starweaver-agent",
            "0.3.0",
        ) {
            Ok(updated) => updated,
            Err(error) => panic!("path dependency version should update: {error}"),
        };
        assert!(updated.contains(
            "starweaver-agent = { path = \"../../crates/starweaver-agent\", version = \"0.3.0\" }"
        ));
        assert!(updated.contains(
            "starweaver-core = { path = \"../../crates/starweaver-core\", version = \"0.2.1\" }"
        ));
    }

    #[test]
    fn python_package_workspace_dependencies_parse_all_crate_paths() {
        let text = r#"
[dependencies]
serde = "1"
starweaver-agent = { path = "../../crates/starweaver-agent", version = "0.3.0" }
starweaver-context = { path = "../../crates/starweaver-context", version = "0.3.0" }
tokio = { version = "1", features = ["sync"] }
"#;
        let dependencies = match python_package_workspace_dependencies(text) {
            Ok(dependencies) => dependencies,
            Err(error) => panic!("Python package dependencies should parse: {error}"),
        };
        assert_eq!(
            dependencies,
            BTreeSet::from([
                "starweaver-agent".to_string(),
                "starweaver-context".to_string(),
            ])
        );
    }

    #[test]
    fn checked_in_publish_order_respects_workspace_dependencies() {
        let root = match root() {
            Ok(root) => root,
            Err(error) => panic!("workspace root should resolve: {error}"),
        };
        let publish_packages = BTreeSet::from(PUBLISH_PACKAGES);
        let dependencies = match workspace_publish_dependencies(&root, &publish_packages) {
            Ok(dependencies) => dependencies,
            Err(error) => panic!("workspace dependencies should load: {error}"),
        };
        assert!(dependencies["starweaver-runtime"].contains("starweaver-stream"));
        assert!(dependencies["starweaver-storage"].contains("starweaver-agent"));
        if let Err(error) = validate_release_package_lists(&root) {
            panic!("publish package list should be dependency ordered: {error}");
        }
    }

    #[test]
    fn metadata_dependencies_include_all_local_dependencies() {
        let metadata: CargoMetadata = match serde_json::from_str(
            r#"{
                "packages": [
                    {
                        "name": "starweaver-runtime",
                        "dependencies": [
                            {"name": "starweaver-stream", "kind": null, "path": "crates/starweaver-stream"},
                            {"name": "starweaver-build", "kind": "build", "path": "crates/starweaver-build"},
                            {"name": "starweaver-dev", "kind": "dev", "path": "crates/starweaver-dev"},
                            {"name": "serde", "kind": null, "path": null}
                        ]
                    },
                    {"name": "starweaver-stream", "dependencies": []},
                    {"name": "starweaver-build", "dependencies": []},
                    {"name": "starweaver-dev", "dependencies": []}
                ]
            }"#,
        ) {
            Ok(metadata) => metadata,
            Err(error) => panic!("metadata fixture should parse: {error}"),
        };
        let publish_packages = BTreeSet::from([
            "starweaver-runtime",
            "starweaver-stream",
            "starweaver-build",
            "starweaver-dev",
        ]);
        let dependencies = match publish_dependencies_from_metadata(metadata, &publish_packages) {
            Ok(dependencies) => dependencies,
            Err(error) => panic!("metadata dependencies should load: {error}"),
        };

        assert_eq!(
            dependencies["starweaver-runtime"],
            BTreeSet::from([
                "starweaver-build".to_string(),
                "starweaver-dev".to_string(),
                "starweaver-stream".to_string(),
            ])
        );
    }

    #[test]
    fn metadata_dependencies_reject_local_dependencies_outside_the_publish_list() {
        let metadata: CargoMetadata = match serde_json::from_str(
            r#"{
                "packages": [
                    {
                        "name": "starweaver-runtime",
                        "dependencies": [
                            {"name": "starweaver-rpc", "kind": null, "path": "crates/starweaver-rpc"}
                        ]
                    }
                ]
            }"#,
        ) {
            Ok(metadata) => metadata,
            Err(error) => panic!("metadata fixture should parse: {error}"),
        };
        let publish_packages = BTreeSet::from(["starweaver-runtime"]);

        let Err(error) = publish_dependencies_from_metadata(metadata, &publish_packages) else {
            panic!("local dependencies outside the publish list should be rejected");
        };
        assert_eq!(
            error,
            "publish package starweaver-runtime has local dependency starweaver-rpc that is not in the publish package list"
        );
    }

    #[test]
    fn publish_dependency_order_rejects_a_dependency_after_its_dependent() {
        let dependencies = BTreeMap::from([
            (
                "starweaver-agent".to_string(),
                BTreeSet::from(["starweaver-runtime".to_string()]),
            ),
            ("starweaver-runtime".to_string(), BTreeSet::new()),
        ]);

        let error = match validate_publish_dependency_order(
            &["starweaver-agent", "starweaver-runtime"],
            &dependencies,
        ) {
            Ok(()) => panic!("invalid publish order should be rejected"),
            Err(error) => error,
        };
        assert_eq!(
            error,
            "publish package starweaver-agent must come after its workspace dependency starweaver-runtime"
        );
    }

    #[test]
    fn publish_retry_delay_uses_numeric_retry_after() {
        let output = "error: 429 Too Many Requests\nretry-after: 123\n";
        let delay = publish_retry_delay_seconds_at(output, 60, 900, UNIX_EPOCH);
        assert_eq!(delay, 123);
    }

    #[test]
    fn publish_retry_delay_uses_http_date_retry_after() {
        let output = "headers:\nretry-after: Thu, 01 Jan 1970 00:02:00 GMT\n";
        let delay = publish_retry_delay_seconds_at(output, 60, 900, UNIX_EPOCH);
        assert_eq!(delay, 120);
    }

    #[test]
    fn publish_retry_delay_caps_and_defaults() {
        assert_eq!(
            publish_retry_delay_seconds_at("retry-after: 999", 60, 300, UNIX_EPOCH),
            300
        );
        assert_eq!(
            publish_retry_delay_seconds_at("no retry header", 60, 300, UNIX_EPOCH),
            60
        );
    }
}
