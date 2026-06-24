use std::{
    collections::BTreeSet,
    env, fs,
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

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
    "starweaver-runtime",
    "starweaver-envd-core",
    "starweaver-environment",
    "starweaver-envd-client",
    "starweaver-envd",
    "starweaver-session",
    "starweaver-stream",
    "starweaver-rpc-core",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_version_parser_reads_workspace_package_version() {
        let text = r#"
[workspace]
members = ["crates/example"]

[workspace.package]
edition = "2021"
version = "1.2.3"
"#;
        let parsed = match workspace_version_from_manifest_text(text) {
            Ok(version) => version,
            Err(error) => panic!("workspace version should parse: {error}"),
        };
        assert_eq!(parsed, "1.2.3");
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
