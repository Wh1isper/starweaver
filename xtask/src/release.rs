use std::{collections::BTreeSet, env, fs, process::Command, time::Duration};

use crate::common::{root, run_capture, run_command};

const WORKSPACE_DEPENDENCIES: [&str; 13] = [
    "starweaver-agent",
    "starweaver-context",
    "starweaver-core",
    "starweaver-environment",
    "starweaver-model",
    "starweaver-oauth",
    "starweaver-oauth-provider",
    "starweaver-runtime",
    "starweaver-session",
    "starweaver-storage",
    "starweaver-stream",
    "starweaver-tools",
    "starweaver-usage",
];
const DRY_RUN_PACKAGES: [&str; 3] = ["starweaver-core", "starweaver-usage", "starweaver-oauth"];
const PUBLISH_PACKAGES: [&str; 14] = [
    "starweaver-core",
    "starweaver-usage",
    "starweaver-oauth",
    "starweaver-model",
    "starweaver-context",
    "starweaver-tools",
    "starweaver-runtime",
    "starweaver-environment",
    "starweaver-session",
    "starweaver-stream",
    "starweaver-oauth-provider",
    "starweaver-agent",
    "starweaver-storage",
    "starweaver-cli",
];

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
    run_command(
        Command::new("cargo")
            .arg("metadata")
            .arg("--format-version")
            .arg("1")
            .current_dir(&root),
    )?;
    println!("Updated workspace version to {version}");
    Ok(())
}

fn valid_version(version: &str) -> bool {
    let mut parts = version.splitn(2, ['-', '+']);
    let core = parts.next().unwrap_or_default();
    let nums: Vec<_> = core.split('.').collect();
    let suffix_ok = parts.next().map_or(true, |suffix| {
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
    let needle = format!("{krate} = {{ path = \"crates/{krate}\", version = \"");
    let start = text
        .find(&needle)
        .ok_or_else(|| format!("missing workspace dependency {krate}"))?
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

pub fn workspace_version(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("workspace-version takes no arguments".to_string());
    }
    let root = root()?;
    let manifest = root.join("Cargo.toml");
    let text = fs::read_to_string(&manifest).map_err(|error| error.to_string())?;
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
    println!("{}", &text[value_start..value_end]);
    Ok(())
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
    for package in PUBLISH_PACKAGES {
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
                    println!("Waiting for crates.io index before retrying {package} ({attempt}/{retries})");
                    attempt += 1;
                    std::thread::sleep(Duration::from_secs(delay));
                }
            }
        }
    }
    Ok(())
}

fn validate_release_package_lists(root: &std::path::Path) -> Result<(), String> {
    ensure_unique("workspace dependency", &WORKSPACE_DEPENDENCIES)?;
    ensure_unique("dry-run package", &DRY_RUN_PACKAGES)?;
    ensure_unique("publish package", &PUBLISH_PACKAGES)?;

    let publish_packages: BTreeSet<_> = PUBLISH_PACKAGES.iter().copied().collect();
    let workspace_dependencies: BTreeSet<_> = WORKSPACE_DEPENDENCIES.iter().copied().collect();
    let expected_workspace_dependencies: BTreeSet<_> = publish_packages
        .iter()
        .copied()
        .filter(|package| *package != "starweaver-cli")
        .collect();
    if workspace_dependencies != expected_workspace_dependencies {
        return Err(format!(
            "workspace dependency list must match publish packages except starweaver-cli: expected {expected_workspace_dependencies:?}, got {workspace_dependencies:?}"
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
    let workspace_crates = workspace_crates_from_manifest(&manifest_value)?;
    if workspace_crates != publish_packages {
        return Err(format!(
            "publish package list must match crates/* workspace members: expected {workspace_crates:?}, got {publish_packages:?}"
        ));
    }
    for krate in WORKSPACE_DEPENDENCIES {
        let needle = format!("{krate} = {{ path = \"crates/{krate}\", version = \"");
        if !manifest_text.contains(&needle) {
            return Err(format!(
                "workspace dependency {krate} must use a path plus version entry"
            ));
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
