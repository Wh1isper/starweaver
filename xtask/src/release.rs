use std::{env, fs, process::Command, time::Duration};

use crate::common::{root, run_capture, run_command};

const WORKSPACE_CRATES: [&str; 10] = [
    "starweaver-agent",
    "starweaver-context",
    "starweaver-core",
    "starweaver-environment",
    "starweaver-model",
    "starweaver-runtime",
    "starweaver-session",
    "starweaver-storage",
    "starweaver-stream",
    "starweaver-tools",
];
const PUBLISH_PACKAGES: [&str; 11] = [
    "starweaver-core",
    "starweaver-model",
    "starweaver-context",
    "starweaver-tools",
    "starweaver-runtime",
    "starweaver-environment",
    "starweaver-session",
    "starweaver-stream",
    "starweaver-storage",
    "starweaver-agent",
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
    let manifest = root.join("Cargo.toml");
    let mut text = fs::read_to_string(&manifest).map_err(|error| error.to_string())?;
    text = replace_workspace_version(&text, version)?;
    for krate in WORKSPACE_CRATES {
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
    println!("Dry-run publishing starweaver-core");
    run_command(
        Command::new("cargo")
            .arg("publish")
            .arg("-p")
            .arg("starweaver-core")
            .arg("--locked")
            .arg("--dry-run")
            .arg("--allow-dirty")
            .current_dir(root),
    )
}

pub fn publish(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("publish takes no arguments".to_string());
    }
    let root = root()?;
    let retries = env::var("PUBLISH_RETRIES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(10);
    let delay = env::var("PUBLISH_RETRY_DELAY_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30);
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
