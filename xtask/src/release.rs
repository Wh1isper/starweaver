use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;

use crate::common::{root, run_capture, run_command};

const WORKSPACE_DEPENDENCIES: [&str; 18] = [
    "starweaver-agent",
    "starweaver-cli",
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
const PUBLISH_PACKAGES: [&str; 18] = [
    "starweaver-core",
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

pub fn upversion(args: &[String]) -> Result<(), String> {
    let version = args
        .first()
        .ok_or_else(|| "usage: upversion x.y.z".to_string())?;
    if args.len() != 1 || !valid_version(version) {
        return Err("usage: upversion x.y.z".to_string());
    }
    let root = root()?;
    validate_release_package_lists(&root)?;
    let manifest = root.join("Cargo.toml");
    let mut text = fs::read_to_string(&manifest).map_err(|error| error.to_string())?;
    text = replace_workspace_version(&text, version)?;
    for krate in WORKSPACE_DEPENDENCIES {
        text = replace_workspace_dependency_version(&text, krate, version)?;
    }
    fs::write(&manifest, text).map_err(|error| error.to_string())?;
    update_python_package_versions(&root, version)?;
    run_command(
        Command::new("cargo")
            .arg("metadata")
            .arg("--format-version")
            .arg("1")
            .current_dir(&root),
    )?;
    crate::capabilities::update_verified_release(&root, version)?;
    crate::capabilities::check_at(&root, true)?;
    println!("Updated workspace version to {version}");
    Ok(())
}

fn update_python_package_versions(root: &std::path::Path, version: &str) -> Result<(), String> {
    let pyproject = root.join("packages/starweaver-py/pyproject.toml");
    let cargo_manifest = root.join("packages/starweaver-py/Cargo.toml");
    if !pyproject.exists() && !cargo_manifest.exists() {
        return Ok(());
    }
    update_toml_table_version(&pyproject, "[project]\n", version)?;
    let mut cargo_text = fs::read_to_string(&cargo_manifest).map_err(|error| error.to_string())?;
    cargo_text = replace_toml_table_version(&cargo_text, "[package]\n", version)
        .map_err(|error| format!("{}: {error}", cargo_manifest.display()))?;
    for krate in python_package_workspace_dependencies(&cargo_text)? {
        cargo_text = replace_path_dependency_version(
            &cargo_text,
            &krate,
            &format!("../../crates/{krate}"),
            version,
        )?;
    }
    fs::write(&cargo_manifest, cargo_text).map_err(|error| error.to_string())?;
    run_command(
        Command::new("cargo")
            .arg("metadata")
            .arg("--manifest-path")
            .arg(&cargo_manifest)
            .arg("--format-version")
            .arg("1")
            .current_dir(root),
    )?;
    Ok(())
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

fn valid_version(version: &str) -> bool {
    let mut parts = version.splitn(2, ['-', '+']);
    let core = parts.next().unwrap_or_default();
    let nums: Vec<_> = core.split('.').collect();
    let suffix_ok = parts.next().is_none_or(|suffix| {
        !suffix.is_empty()
            && suffix
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-')
    });
    nums.len() == 3
        && nums
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
        && suffix_ok
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
