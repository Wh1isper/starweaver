use std::{collections::BTreeSet, fmt::Write as _, fs, path::Path, process::Command};

use serde::Deserialize;
use serde_json::Value;

use crate::common::root;

const SUPPORTED_SCHEMA_VERSION: u32 = 1;
const GENERATED_STATUS: &str = "spec/capability-status.md";
const STATUS_VIEW_REFERENCES: &[&str] = &[
    "spec/README.md",
    "spec/core/05-agent-foundation-feature-map.md",
    "spec/sdk/05-sdk-integration-map.md",
    "spec/envd/05-api-backlog.md",
];
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilityRegistry {
    schema_version: u32,
    last_verified_release: String,
    capability: Vec<Capability>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Capability {
    id: String,
    owner: String,
    stability: String,
    status: String,
    normative_spec: String,
    implementation: Vec<String>,
    contract_tests: Vec<String>,
}

pub fn update_verified_release(root: &Path, version: &str) -> Result<(), String> {
    let registry_path = root.join("spec/capabilities.toml");
    let source = fs::read_to_string(&registry_path)
        .map_err(|error| format!("{}: {error}", registry_path.display()))?;
    let updated = replace_verified_release(&source, version)?;
    fs::write(&registry_path, updated)
        .map_err(|error| format!("{}: {error}", registry_path.display()))?;
    Ok(())
}

pub fn check(args: &[String]) -> Result<(), String> {
    let bless = match args {
        [] => false,
        [arg] if arg == "--bless" => true,
        _ => return Err("usage: check-capabilities [--bless]".to_string()),
    };
    let root = root()?;
    let registry_path = root.join("spec/capabilities.toml");
    let source = fs::read_to_string(&registry_path)
        .map_err(|error| format!("{}: {error}", registry_path.display()))?;
    let registry: CapabilityRegistry =
        toml::from_str(&source).map_err(|error| format!("{}: {error}", registry_path.display()))?;
    let workspace_version = workspace_version(&root)?;
    let workspace_packages = workspace_packages(&root)?;
    validate_registry(&root, &registry, &workspace_version, &workspace_packages)?;
    validate_status_view_references(&root)?;
    validate_or_write_generated_status(&root, &registry, bless)?;
    println!(
        "capability registry passed: {} entries verified for release {}; generated status is current",
        registry.capability.len(),
        registry.last_verified_release
    );
    Ok(())
}

fn replace_verified_release(source: &str, version: &str) -> Result<String, String> {
    let registry: CapabilityRegistry = toml::from_str(source)
        .map_err(|error| format!("invalid spec/capabilities.toml: {error}"))?;
    let current = format!(
        "last_verified_release = \"{}\"",
        registry.last_verified_release
    );
    if source.matches(&current).count() != 1 {
        return Err(
            "spec/capabilities.toml must contain one canonical last_verified_release declaration"
                .to_string(),
        );
    }
    Ok(source.replacen(
        &current,
        &format!("last_verified_release = \"{version}\""),
        1,
    ))
}

fn validate_status_view_references(root: &Path) -> Result<(), String> {
    for relative in STATUS_VIEW_REFERENCES {
        let source = fs::read_to_string(root.join(relative))
            .map_err(|error| format!("failed to read {relative}: {error}"))?;
        if !source.contains("capability-status.md") {
            return Err(format!(
                "{relative} must defer current implementation status to {GENERATED_STATUS}"
            ));
        }
    }
    Ok(())
}

fn validate_or_write_generated_status(
    root: &Path,
    registry: &CapabilityRegistry,
    bless: bool,
) -> Result<(), String> {
    let rendered = render_status(registry)?;
    let path = root.join(GENERATED_STATUS);
    if bless {
        fs::write(&path, rendered)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
        println!("updated {GENERATED_STATUS}");
        return Ok(());
    }
    let committed = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {GENERATED_STATUS}: {error}"))?;
    if committed != rendered {
        return Err(format!(
            "{GENERATED_STATUS} is stale; review spec/capabilities.toml and run \
             `cargo run -p xtask -- check-capabilities --bless`"
        ));
    }
    Ok(())
}

fn render_status(registry: &CapabilityRegistry) -> Result<String, String> {
    let mut capabilities = registry.capability.iter().collect::<Vec<_>>();
    capabilities.sort_by(|left, right| left.id.cmp(&right.id));
    let mut output = String::new();
    writeln!(output, "# Capability Status").map_err(|error| error.to_string())?;
    writeln!(output).map_err(|error| error.to_string())?;
    writeln!(
        output,
        "<!-- Generated from spec/capabilities.toml by `cargo run -p xtask -- check-capabilities --bless`. Do not edit manually. -->"
    )
    .map_err(|error| error.to_string())?;
    writeln!(output).map_err(|error| error.to_string())?;
    writeln!(
        output,
        "This is the normative current implementation-status view for release `{}`. Feature maps, roadmaps, and backlogs describe design or future work and must defer to this generated view for current status.",
        registry.last_verified_release
    )
    .map_err(|error| error.to_string())?;
    writeln!(output).map_err(|error| error.to_string())?;
    writeln!(
        output,
        "| Capability | Owner | Stability | Status | Normative spec | Implementation evidence | Contract evidence |"
    )
    .map_err(|error| error.to_string())?;
    writeln!(output, "| --- | --- | --- | --- | --- | --- | --- |")
        .map_err(|error| error.to_string())?;
    for capability in capabilities {
        let implementation = markdown_links(&capability.implementation);
        let contract_tests = markdown_links(&capability.contract_tests);
        writeln!(
            output,
            "| `{}` | `{}` | {} | {} | [{}](../{}) | {} | {} |",
            capability.id,
            capability.owner,
            capability.stability,
            capability.status,
            capability
                .normative_spec
                .strip_prefix("spec/")
                .unwrap_or(&capability.normative_spec),
            capability.normative_spec,
            implementation,
            contract_tests,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(output)
}

fn markdown_links(paths: &[String]) -> String {
    if paths.is_empty() {
        return "—".to_string();
    }
    paths
        .iter()
        .map(|path| format!("[`{path}`](../{path})"))
        .collect::<Vec<_>>()
        .join("<br>")
}

fn workspace_version(root: &Path) -> Result<String, String> {
    let source = fs::read_to_string(root.join("Cargo.toml")).map_err(|error| error.to_string())?;
    let manifest: toml::Value = toml::from_str(&source).map_err(|error| error.to_string())?;
    manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "Cargo.toml omits workspace.package.version".to_string())
}

fn workspace_packages(root: &Path) -> Result<BTreeSet<String>, String> {
    let output = Command::new("cargo")
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--no-deps", "--locked"])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let metadata: Value =
        serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
    metadata
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| "cargo metadata did not contain packages".to_string())?
        .iter()
        .map(|package| {
            package
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| "cargo metadata package did not contain a name".to_string())
        })
        .collect()
}

fn validate_registry(
    root: &Path,
    registry: &CapabilityRegistry,
    workspace_version: &str,
    workspace_packages: &BTreeSet<String>,
) -> Result<(), String> {
    if registry.schema_version != SUPPORTED_SCHEMA_VERSION {
        return Err(format!(
            "unsupported capability registry schema {}; supported schema is {SUPPORTED_SCHEMA_VERSION}",
            registry.schema_version
        ));
    }
    if registry.last_verified_release != workspace_version {
        return Err(format!(
            "capability registry last_verified_release {} does not match workspace version {workspace_version}",
            registry.last_verified_release
        ));
    }

    let mut ids = BTreeSet::new();
    for capability in &registry.capability {
        validate_capability(root, capability, workspace_packages)?;
        if !ids.insert(capability.id.as_str()) {
            return Err(format!("duplicate capability id {}", capability.id));
        }
    }
    if ids.is_empty() {
        return Err("capability registry cannot be empty".to_string());
    }
    Ok(())
}

fn validate_capability(
    root: &Path,
    capability: &Capability,
    workspace_packages: &BTreeSet<String>,
) -> Result<(), String> {
    if !valid_id(&capability.id) {
        return Err(format!(
            "capability id {} must be a lowercase dotted identifier",
            capability.id
        ));
    }
    if !matches!(
        capability.stability.as_str(),
        "stable" | "provisional" | "planned"
    ) {
        return Err(format!(
            "capability {} has unsupported stability {}",
            capability.id, capability.stability
        ));
    }
    if !matches!(capability.status.as_str(), "implemented" | "planned") {
        return Err(format!(
            "capability {} has unsupported status {}",
            capability.id, capability.status
        ));
    }
    if capability.status == "implemented" {
        if capability.stability == "planned" {
            return Err(format!(
                "implemented capability {} cannot use planned stability",
                capability.id
            ));
        }
        if !workspace_packages.contains(&capability.owner) {
            return Err(format!(
                "implemented capability {} names non-workspace owner {}",
                capability.id, capability.owner
            ));
        }
        if capability.implementation.is_empty() || capability.contract_tests.is_empty() {
            return Err(format!(
                "implemented capability {} requires implementation and contract-test evidence",
                capability.id
            ));
        }
    } else if capability.stability != "planned"
        || !capability.implementation.is_empty()
        || !capability.contract_tests.is_empty()
    {
        return Err(format!(
            "planned capability {} must use planned stability and cannot claim implementation evidence",
            capability.id
        ));
    }

    validate_evidence_path(
        root,
        &capability.id,
        "normative_spec",
        &capability.normative_spec,
    )?;
    if !capability.normative_spec.starts_with("spec/")
        || !capability.normative_spec.ends_with(".md")
    {
        return Err(format!(
            "capability {} normative_spec must be a Markdown file under spec/",
            capability.id
        ));
    }
    for path in &capability.implementation {
        validate_evidence_path(root, &capability.id, "implementation", path)?;
        if !root.join(path).is_file() {
            return Err(format!(
                "capability {} implementation evidence must be a file: {path}",
                capability.id
            ));
        }
    }
    let mut has_runnable_test = false;
    for path in &capability.contract_tests {
        validate_evidence_path(root, &capability.id, "contract_tests", path)?;
        has_runnable_test |= validate_contract_evidence(root, &capability.id, path)?;
    }
    if capability.status == "implemented" && !has_runnable_test {
        return Err(format!(
            "implemented capability {} requires at least one runnable Rust contract-test file",
            capability.id
        ));
    }
    Ok(())
}

fn validate_contract_evidence(
    root: &Path,
    capability_id: &str,
    relative: &str,
) -> Result<bool, String> {
    let path = root.join(relative);
    if path.is_dir() {
        let mut entries =
            fs::read_dir(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        if entries.next().is_none() {
            return Err(format!(
                "capability {capability_id} contract fixture directory is empty: {relative}"
            ));
        }
        return Ok(false);
    }
    if path.extension().and_then(std::ffi::OsStr::to_str) != Some("rs") {
        return Err(format!(
            "capability {capability_id} contract evidence must be a Rust test file or non-empty fixture directory: {relative}"
        ));
    }
    let source =
        fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !source.contains("#[test]")
        && !source.contains("#[tokio::test]")
        && !source.contains("#[cfg(test)]")
    {
        return Err(format!(
            "capability {capability_id} contract evidence has no compiled test marker: {relative}"
        ));
    }
    Ok(true)
}

fn valid_id(id: &str) -> bool {
    id.contains('.')
        && !id.starts_with('.')
        && !id.ends_with('.')
        && !id.contains("..")
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'.'
        })
}

fn validate_evidence_path(
    root: &Path,
    capability_id: &str,
    field: &str,
    relative: &str,
) -> Result<(), String> {
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(format!(
            "capability {capability_id} {field} path must stay inside the repository: {relative}"
        ));
    }
    if relative.is_empty() || !root.join(path).exists() {
        return Err(format!(
            "capability {capability_id} {field} evidence does not exist: {relative}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{replace_verified_release, valid_id};

    #[test]
    fn replaces_verified_release_without_changing_capabilities() {
        let source = r#"schema_version = 1
last_verified_release = "0.7.0"
capability = []
"#;
        let updated = match replace_verified_release(source, "0.8.0") {
            Ok(updated) => updated,
            Err(error) => panic!("verified release should update: {error}"),
        };
        assert_eq!(
            updated,
            r#"schema_version = 1
last_verified_release = "0.8.0"
capability = []
"#
        );
    }

    #[test]
    fn validates_capability_ids() {
        assert!(valid_id("runtime.agent_loop"));
        assert!(!valid_id("Runtime.agent_loop"));
        assert!(!valid_id("runtime"));
        assert!(!valid_id("runtime..loop!"));
    }
}
