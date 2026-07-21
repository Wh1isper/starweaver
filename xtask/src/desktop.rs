use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::Value;

use crate::common::{read_json, root};

const TARGET_REGISTRY: &str = "apps/starweaver-desktop/targets.toml";
const DESKTOP_WORKFLOW: &str = ".github/workflows/desktop-ci.yml";
const DESKTOP_ROOT: &str = "apps/starweaver-desktop";

#[derive(Debug, Deserialize)]
struct TargetRegistry {
    schema_version: u32,
    targets: Vec<DesktopTarget>,
}

#[derive(Debug, Deserialize)]
struct DesktopTarget {
    id: String,
    os: String,
    architecture: String,
    rust_target: String,
    runner: String,
    desktop_bundles: Vec<String>,
    runtime_archive: String,
    native_test: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ExpectedTarget {
    id: &'static str,
    os: &'static str,
    architecture: &'static str,
    rust_target: &'static str,
    runner: &'static str,
    bundles: &'static [&'static str],
    runtime_archive: &'static str,
    native_test: bool,
}

const EXPECTED_TARGETS: &[ExpectedTarget] = &[
    ExpectedTarget {
        id: "linux-x86_64",
        os: "linux",
        architecture: "x86_64",
        rust_target: "x86_64-unknown-linux-gnu",
        runner: "ubuntu-latest",
        bundles: &["appimage", "deb"],
        runtime_archive: "tar.gz",
        native_test: true,
    },
    ExpectedTarget {
        id: "macos-x86_64",
        os: "macos",
        architecture: "x86_64",
        rust_target: "x86_64-apple-darwin",
        runner: "macos-latest",
        bundles: &["dmg"],
        runtime_archive: "tar.gz",
        native_test: false,
    },
    ExpectedTarget {
        id: "macos-aarch64",
        os: "macos",
        architecture: "aarch64",
        rust_target: "aarch64-apple-darwin",
        runner: "macos-latest",
        bundles: &["dmg"],
        runtime_archive: "tar.gz",
        native_test: true,
    },
    ExpectedTarget {
        id: "windows-x86_64",
        os: "windows",
        architecture: "x86_64",
        rust_target: "x86_64-pc-windows-msvc",
        runner: "windows-latest",
        bundles: &["nsis"],
        runtime_archive: "zip",
        native_test: true,
    },
];

#[derive(Clone, Debug, Eq, PartialEq)]
struct WorkflowTarget {
    runner: String,
    native_test: bool,
}

pub fn check() -> Result<(), String> {
    let repository = root()?;
    let registry = read_registry(&repository.join(TARGET_REGISTRY))?;
    check_target_registry(&registry)?;
    check_workflow_matrix(
        &registry,
        &fs::read_to_string(repository.join(DESKTOP_WORKFLOW))
            .map_err(|error| error.to_string())?,
    )?;
    let desktop_root = repository.join(DESKTOP_ROOT);
    check_renderer_boundary(&desktop_root)?;
    check_security_configuration(&desktop_root)?;
    check_version_alignment(&repository, &desktop_root)?;
    println!(
        "desktop boundaries passed: four supported native targets match CI; renderer IPC is confined to the typed bridge; Tauri capabilities and CSP are least-authority"
    );
    Ok(())
}

fn read_registry(path: &Path) -> Result<TargetRegistry, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

fn check_target_registry(registry: &TargetRegistry) -> Result<(), String> {
    if registry.schema_version != 1 {
        return Err(format!(
            "Desktop target registry schema must be 1, found {}",
            registry.schema_version
        ));
    }
    if registry.targets.len() != EXPECTED_TARGETS.len() {
        return Err(format!(
            "Desktop target registry must contain exactly {} targets, found {}",
            EXPECTED_TARGETS.len(),
            registry.targets.len()
        ));
    }

    let mut seen_ids = BTreeSet::new();
    let mut seen_targets = BTreeSet::new();
    for target in &registry.targets {
        if !seen_ids.insert(target.id.as_str()) {
            return Err(format!("duplicate Desktop target id: {}", target.id));
        }
        if !seen_targets.insert(target.rust_target.as_str()) {
            return Err(format!(
                "duplicate Desktop Rust target: {}",
                target.rust_target
            ));
        }
        let expected = EXPECTED_TARGETS
            .iter()
            .find(|expected| expected.rust_target == target.rust_target)
            .ok_or_else(|| format!("unsupported Desktop Rust target: {}", target.rust_target))?;
        let bundles = target
            .desktop_bundles
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        if target.id != expected.id
            || target.os != expected.os
            || target.architecture != expected.architecture
            || target.runner != expected.runner
            || bundles != expected.bundles
            || target.runtime_archive != expected.runtime_archive
            || target.native_test != expected.native_test
        {
            return Err(format!(
                "Desktop target {} does not match the reviewed target contract",
                target.rust_target
            ));
        }
    }
    Ok(())
}

fn check_workflow_matrix(registry: &TargetRegistry, workflow: &str) -> Result<(), String> {
    for required in [
        "Smoke test macOS single-instance activation",
        "if: matrix.target == 'aarch64-apple-darwin'",
        "apps/starweaver-desktop/scripts/smoke-single-instance-macos.sh target/${{ matrix.target }}/release/starweaver-desktop",
    ] {
        if !workflow.contains(required) {
            return Err(format!(
                "Desktop CI must retain the native single-instance smoke contract: {required}"
            ));
        }
    }
    let workflow_targets = parse_workflow_targets(workflow)?;
    let registry_targets = registry
        .targets
        .iter()
        .map(|target| target.rust_target.as_str())
        .collect::<BTreeSet<_>>();
    let workflow_target_names = workflow_targets
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if workflow_target_names != registry_targets {
        return Err(format!(
            "Desktop CI targets do not match {TARGET_REGISTRY}: registry={registry_targets:?}, workflow={workflow_target_names:?}"
        ));
    }
    for target in &registry.targets {
        let workflow_target = &workflow_targets[&target.rust_target];
        if workflow_target.runner != target.runner
            || workflow_target.native_test != target.native_test
        {
            return Err(format!(
                "Desktop CI entry for {} disagrees with its runner or native-test policy",
                target.rust_target
            ));
        }
    }
    Ok(())
}

fn parse_workflow_targets(workflow: &str) -> Result<BTreeMap<String, WorkflowTarget>, String> {
    let mut targets = BTreeMap::new();
    let mut runner: Option<String> = None;
    let mut target: Option<String> = None;

    for line in workflow.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("- os: ") {
            insert_workflow_target(&mut targets, &mut runner, &mut target, None)?;
            runner = Some(value.trim_matches(['\'', '"']).to_string());
        } else if let Some(value) = trimmed.strip_prefix("target: ") {
            if !value.contains("${{") {
                target = Some(value.trim_matches(['\'', '"']).to_string());
            }
        } else if let Some(value) = trimmed.strip_prefix("run_tests: ") {
            let native_test = value
                .parse::<bool>()
                .map_err(|error| format!("invalid Desktop run_tests value {value}: {error}"))?;
            insert_workflow_target(&mut targets, &mut runner, &mut target, Some(native_test))?;
        }
    }
    insert_workflow_target(&mut targets, &mut runner, &mut target, None)?;
    Ok(targets)
}

fn insert_workflow_target(
    targets: &mut BTreeMap<String, WorkflowTarget>,
    runner: &mut Option<String>,
    target: &mut Option<String>,
    native_test: Option<bool>,
) -> Result<(), String> {
    let Some(target_name) = target.take() else {
        return Ok(());
    };
    let runner_name = runner
        .take()
        .ok_or_else(|| format!("Desktop CI target {target_name} has no runner"))?;
    let native_test = native_test.ok_or_else(|| {
        format!("Desktop CI target {target_name} must declare its run_tests policy")
    })?;
    if targets
        .insert(
            target_name.clone(),
            WorkflowTarget {
                runner: runner_name,
                native_test,
            },
        )
        .is_some()
    {
        return Err(format!("duplicate Desktop CI target: {target_name}"));
    }
    Ok(())
}

fn check_renderer_boundary(desktop_root: &Path) -> Result<(), String> {
    let source_root = desktop_root.join("src");
    let allowed_bridge = source_root.join("bridge/desktop.ts");
    let allowed_bridge_test = source_root.join("bridge/desktop.test.ts");
    let mut renderer_sources = source_files(&source_root)?;
    renderer_sources.push(desktop_root.join("index.html"));
    for path in renderer_sources {
        let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        if text.contains("@tauri-apps/api") && path != allowed_bridge && path != allowed_bridge_test
        {
            return Err(format!(
                "renderer Tauri API import must stay in src/bridge/desktop.ts: {}",
                path.display()
            ));
        }
        for forbidden in [
            "__TAURI_INTERNALS__",
            "window.__TAURI__",
            "globalThis.__TAURI__",
        ] {
            if text.contains(forbidden) {
                return Err(format!(
                    "renderer source must not access raw Tauri internals ({forbidden}): {}",
                    path.display()
                ));
            }
        }
    }

    let bridge_test =
        fs::read_to_string(&allowed_bridge_test).map_err(|error| error.to_string())?;
    if bridge_test.matches("@tauri-apps/api").count() != 1
        || !bridge_test.contains("vi.mock(\"@tauri-apps/api/core\"")
    {
        return Err("Desktop bridge test may only mock the reviewed Tauri core module".to_string());
    }

    let bridge = fs::read_to_string(&allowed_bridge).map_err(|error| error.to_string())?;
    if bridge.matches("@tauri-apps/api").count() != 1
        || !bridge.contains("from \"@tauri-apps/api/core\"")
        || bridge.matches("invoke<").count() != 3
        || bridge.contains("invoke(")
        || bridge.matches("new Channel<DesktopActivation>").count() != 1
    {
        return Err(
            "Desktop bridge must use only the reviewed core invoke/channel surface".to_string(),
        );
    }
    for (constant, command) in [
        ("GET_DESKTOP_STATUS_COMMAND", "get_desktop_status"),
        (
            "SUBSCRIBE_DESKTOP_ACTIVATION_COMMAND",
            "subscribe_desktop_activation",
        ),
        (
            "UNSUBSCRIBE_DESKTOP_ACTIVATION_COMMAND",
            "unsubscribe_desktop_activation",
        ),
    ] {
        let declaration = format!("const {constant} = \"{command}\";");
        if bridge.matches(&declaration).count() != 1 {
            return Err(format!(
                "Desktop bridge must declare exactly the reviewed command {command}"
            ));
        }
    }

    let package = read_json(&desktop_root.join("package.json"))?;
    for section in ["dependencies", "devDependencies"] {
        for name in package
            .get(section)
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|dependencies| dependencies.keys())
        {
            if name.starts_with("@tauri-apps/plugin-") {
                return Err(format!(
                    "renderer package {name} is forbidden until a scoped capability is specified"
                ));
            }
        }
    }
    Ok(())
}

fn source_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.is_dir() {
                pending.push(path);
            } else if matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("ts" | "tsx" | "js" | "jsx" | "html" | "rs")
            ) {
                files.push(path);
            }
        }
    }
    Ok(files)
}

fn check_security_configuration(desktop_root: &Path) -> Result<(), String> {
    let capability = read_json(&desktop_root.join("src-tauri/capabilities/default.json"))?;
    let permissions = string_set(&capability, "permissions")?;
    let expected_permissions = BTreeSet::from([
        "allow-get-desktop-status",
        "allow-subscribe-desktop-activation",
        "allow-unsubscribe-desktop-activation",
    ]);
    if permissions != expected_permissions {
        return Err(format!(
            "Desktop main capability must contain only reviewed permissions: {permissions:?}"
        ));
    }
    let platforms = string_set(&capability, "platforms")?;
    if platforms != BTreeSet::from(["linux", "macOS", "windows"]) {
        return Err(format!(
            "Desktop capability platforms do not cover exactly Linux, macOS, and Windows: {platforms:?}"
        ));
    }

    let config = read_json(&desktop_root.join("src-tauri/tauri.conf.json"))?;
    let security = config
        .pointer("/app/security")
        .and_then(Value::as_object)
        .ok_or_else(|| "Desktop Tauri configuration omitted app.security".to_string())?;
    if security.get("freezePrototype") != Some(&Value::Bool(true)) {
        return Err("Desktop Tauri configuration must freeze the IPC prototype".to_string());
    }
    check_csp(
        security,
        "csp",
        &[
            ("connect-src", "'self' ipc: http://ipc.localhost"),
            ("default-src", "'self' customprotocol: asset:"),
            ("font-src", "'self'"),
            (
                "img-src",
                "'self' asset: http://asset.localhost data: blob:",
            ),
            ("style-src", "'self'"),
        ],
    )?;
    check_csp(
        security,
        "devCsp",
        &[
            (
                "connect-src",
                "'self' ipc: http://ipc.localhost http://localhost:1420 ws://localhost:1421",
            ),
            ("default-src", "'self' http://localhost:1420"),
            ("font-src", "'self' http://localhost:1420"),
            ("img-src", "'self' data: blob: http://localhost:1420"),
            ("script-src", "'self' http://localhost:1420"),
            ("style-src", "'self' 'unsafe-inline' http://localhost:1420"),
        ],
    )?;
    if config
        .pointer("/app/withGlobalTauri")
        .is_some_and(|value| value != &Value::Bool(false))
    {
        return Err("Desktop must not expose the global Tauri API".to_string());
    }
    if security.get("capabilities") != Some(&Value::Array(vec![Value::String("main".to_string())]))
    {
        return Err("Desktop must explicitly select only the main capability".to_string());
    }

    let build_script = fs::read_to_string(desktop_root.join("src-tauri/build.rs"))
        .map_err(|error| error.to_string())?;
    for command in [
        "get_desktop_status",
        "subscribe_desktop_activation",
        "unsubscribe_desktop_activation",
    ] {
        if build_script.matches(&format!("\"{command}\"")).count() != 1 {
            return Err(format!(
                "Desktop build manifest must generate exactly one permission for {command}"
            ));
        }
    }

    let cargo_manifest = fs::read_to_string(desktop_root.join("src-tauri/Cargo.toml"))
        .map_err(|error| error.to_string())?;
    if cargo_manifest.contains("tauri-plugin-single-instance") {
        return Err(
            "Desktop must not use a single-instance transport that forwards argv or cwd"
                .to_string(),
        );
    }
    let mut single_instance_sources =
        source_files(&desktop_root.join("src-tauri/src/single_instance"))?;
    single_instance_sources.push(desktop_root.join("src-tauri/src/single_instance.rs"));
    for path in single_instance_sources {
        let source = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        for forbidden in ["std::env::args", "std::env::current_dir"] {
            if source.contains(forbidden) {
                return Err(format!(
                    "Desktop single-instance transport must not read process context ({forbidden}): {}",
                    path.display()
                ));
            }
        }
    }
    for (platform, required) in [
        (
            "linux.rs",
            &["replace_existing_names(false)", "Activate", "&()"][..],
        ),
        (
            "macos.rs",
            &["try_lock()", "getpeereid", "ACTIVATION_ACK"][..],
        ),
        (
            "windows.rs",
            &[
                "app_local_data_dir",
                "Uuid::new_v4",
                "try_lock()",
                "process_user_id",
                "peer_creds()",
                "recv_timeout",
                "set_nonblocking(true)",
                "ENDPOINT_SECURITY",
            ][..],
        ),
    ] {
        let path = desktop_root
            .join("src-tauri/src/single_instance")
            .join(platform);
        let source = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        for invariant in required {
            if !source.contains(invariant) {
                return Err(format!(
                    "Desktop {platform} single-instance transport omitted reviewed invariant {invariant}"
                ));
            }
        }
    }
    Ok(())
}

fn check_csp(
    security: &serde_json::Map<String, Value>,
    key: &str,
    expected: &[(&str, &str)],
) -> Result<(), String> {
    let directives = security
        .get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("Desktop {key} must be an explicit directive map"))?;
    let actual = directives
        .iter()
        .map(|(directive, value)| {
            value
                .as_str()
                .map(|value| (directive.as_str(), value))
                .ok_or_else(|| format!("Desktop {key}.{directive} must be a string"))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let expected = expected.iter().copied().collect::<BTreeMap<_, _>>();
    if actual != expected {
        return Err(format!(
            "Desktop {key} must match the reviewed least-authority policy: {actual:?}"
        ));
    }
    Ok(())
}

fn check_version_alignment(repository: &Path, desktop_root: &Path) -> Result<(), String> {
    let workspace_manifest = fs::read_to_string(repository.join("Cargo.toml"))
        .map_err(|error| error.to_string())?
        .parse::<toml::Value>()
        .map_err(|error| error.to_string())?;
    let workspace_version = workspace_manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| "workspace.package.version must be a string".to_string())?;

    for path in [
        desktop_root.join("package.json"),
        desktop_root.join("src-tauri/tauri.conf.json"),
    ] {
        let version = read_json(&path)?
            .get("version")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{} version must be a string", path.display()))?
            .to_string();
        if version != workspace_version {
            return Err(format!(
                "{} version {version} does not match workspace version {workspace_version}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn string_set<'a>(value: &'a Value, key: &str) -> Result<BTreeSet<&'a str>, String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("Desktop JSON field {key} must be an array"))?
        .iter()
        .map(|item| {
            item.as_str()
                .ok_or_else(|| format!("Desktop JSON field {key} must contain only strings"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reviewed_workflow_entries() {
        let workflow = r"
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            run_tests: true
          - os: macos-latest
            target: x86_64-apple-darwin
            run_tests: false
        run: cargo build --target ${{ matrix.target }}
";
        let Ok(targets) = parse_workflow_targets(workflow) else {
            panic!("reviewed matrix must parse");
        };

        assert_eq!(targets.len(), 2);
        assert_eq!(targets["x86_64-unknown-linux-gnu"].runner, "ubuntu-latest");
        assert!(targets["x86_64-unknown-linux-gnu"].native_test);
        assert!(!targets["x86_64-apple-darwin"].native_test);
    }

    #[test]
    fn rejects_workflow_entry_without_test_policy() {
        let workflow = "- os: ubuntu-latest\n  target: x86_64-unknown-linux-gnu\n";

        assert!(parse_workflow_targets(workflow).is_err());
    }
}
