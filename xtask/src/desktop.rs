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
const RELEASE_WORKFLOW: &str = ".github/workflows/release.yml";
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
    check_release_workflow(
        &registry,
        &fs::read_to_string(repository.join(RELEASE_WORKFLOW))
            .map_err(|error| error.to_string())?,
    )?;
    let desktop_root = repository.join(DESKTOP_ROOT);
    check_renderer_boundary(&desktop_root)?;
    check_security_configuration(&desktop_root)?;
    check_version_alignment(&repository, &desktop_root)?;
    println!(
        "desktop boundaries passed: four supported native targets match CI and release packaging; renderer IPC is confined to the typed bridge; Tauri capabilities and CSP are least-authority"
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
        "Build updater-ready native installers with exact RPC sidecar",
        "tauri signer generate",
        "--config src-tauri/tauri.updater.conf.json",
        "tauri-updater-config.mjs",
        "package-runtime-update.mjs",
        "collect-desktop-artifacts.mjs",
        "verify-update-signature",
        "verify-packaged-sidecar.mjs",
        "tests/protocol-client/client.py",
        "Smoke test macOS single-instance activation",
        "if: matrix.target == 'aarch64-apple-darwin'",
        "apps/starweaver-desktop/scripts/smoke-single-instance-macos.sh target/${{ matrix.target }}/release/starweaver-desktop",
    ] {
        if !workflow.contains(required) {
            return Err(format!(
                "Desktop CI must retain native package, update, and single-instance validation: {required}"
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

fn check_release_workflow(registry: &TargetRegistry, workflow: &str) -> Result<(), String> {
    for required in [
        "build-runtime-update-artifacts:",
        "build-desktop-artifacts:",
        "tauri.updater.conf.json",
        "tauri-updater-config.mjs",
        "package-runtime-update.mjs",
        "collect-desktop-artifacts.mjs",
        "finalize-update-metadata.mjs",
        "verify-update-signature",
        "actions/attest-build-provenance@v3",
        "STARWEAVER_UPDATE_PUBLIC_KEY",
        "TAURI_SIGNING_PRIVATE_KEY",
        "upload-core-assets:",
        "needs: [build-binaries, build-protocol-artifacts, build-python-sdist, build-python-wheels]",
        "upload-runtime-assets:",
        "needs: build-runtime-update-artifacts",
        "runtime-checksums.txt",
        "upload-desktop-assets:",
        "needs: build-desktop-artifacts",
        "desktop-checksums.txt",
        "needs: upload-core-assets",
        "Upload immutable core assets to release",
        "Upload immutable runtime update assets to release",
        "Upload immutable Desktop assets to release",
        "gh release view",
    ] {
        if !workflow.contains(required) {
            return Err(format!(
                "Desktop release packaging must retain the reviewed update and provenance contract: {required}"
            ));
        }
    }
    let runtime_manifest_path = concat!(
        "manifest=\"$GITHUB_WORKSPACE/dist/runtime/starweaver-runtime-$",
        "{target}.manifest.json\""
    );
    if !workflow.contains(runtime_manifest_path) {
        return Err(
            "Runtime release signing must use the workspace-absolute manifest path".to_string(),
        );
    }
    if workflow.contains("--clobber") {
        return Err("Release automation must not replace immutable published assets".to_string());
    }

    let job_dependencies = parse_release_job_dependencies(workflow)?;
    for job in [
        "upload-core-assets",
        "upload-runtime-assets",
        "publish-crates",
        "publish-python",
    ] {
        if release_job_depends_on(
            &job_dependencies,
            job,
            "build-desktop-artifacts",
            &mut BTreeSet::new(),
        ) {
            return Err(format!(
                "Desktop release failure must not block publication job {job}"
            ));
        }
    }
    for (job, dependency) in [
        ("build-runtime-update-artifacts", "build-binaries"),
        ("upload-runtime-assets", "build-runtime-update-artifacts"),
        ("upload-desktop-assets", "build-desktop-artifacts"),
        ("publish-crates", "upload-core-assets"),
        ("publish-python", "upload-core-assets"),
    ] {
        if !release_job_depends_on(&job_dependencies, job, dependency, &mut BTreeSet::new()) {
            return Err(format!(
                "Release job {job} must retain dependency path to {dependency}"
            ));
        }
    }

    for target in &registry.targets {
        let target_marker = format!("target: {}", target.rust_target);
        let bundle_marker = format!("bundles: {}", target.desktop_bundles.join(","));
        if !workflow.contains(&target_marker) || !workflow.contains(&bundle_marker) {
            return Err(format!(
                "Desktop release workflow does not package the reviewed target and bundles for {}",
                target.rust_target
            ));
        }
    }
    Ok(())
}

fn parse_release_job_dependencies(workflow: &str) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut jobs = BTreeMap::new();
    let mut current_job: Option<String> = None;
    let mut in_jobs = false;

    for line in workflow.lines() {
        if line == "jobs:" {
            in_jobs = true;
            continue;
        }
        if !in_jobs {
            continue;
        }
        if line.starts_with("  ") && !line.starts_with("   ") && line.ends_with(':') {
            let job = line.trim().trim_end_matches(':').to_string();
            jobs.entry(job.clone()).or_insert_with(Vec::new);
            current_job = Some(job);
            continue;
        }
        let Some(job) = current_job.as_ref() else {
            continue;
        };
        let Some(value) = line.strip_prefix("    needs:") else {
            continue;
        };
        let value = value.trim();
        let dependencies = if let Some(inner) = value
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
        {
            inner
                .split(',')
                .map(|dependency| dependency.trim().trim_matches(['\'', '"']).to_string())
                .filter(|dependency| !dependency.is_empty())
                .collect::<Vec<_>>()
        } else if value.is_empty() {
            return Err(format!(
                "Release job {job} must keep needs on one reviewed line"
            ));
        } else {
            vec![value.trim_matches(['\'', '"']).to_string()]
        };
        jobs.insert(job.clone(), dependencies);
    }

    for (job, dependencies) in &jobs {
        for dependency in dependencies {
            if !jobs.contains_key(dependency) {
                return Err(format!(
                    "Release job {job} references unknown dependency {dependency}"
                ));
            }
        }
    }
    Ok(jobs)
}

fn release_job_depends_on(
    jobs: &BTreeMap<String, Vec<String>>,
    job: &str,
    dependency: &str,
    seen: &mut BTreeSet<String>,
) -> bool {
    if !seen.insert(job.to_string()) {
        return false;
    }
    jobs.get(job).is_some_and(|dependencies| {
        dependencies.iter().any(|candidate| {
            candidate == dependency || release_job_depends_on(jobs, candidate, dependency, seen)
        })
    })
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
    let allowed_generated_host_client = source_root.join("generated/host/client.ts");
    let mut renderer_sources = source_files(&source_root)?;
    renderer_sources.push(desktop_root.join("index.html"));
    for path in renderer_sources {
        let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        if text.contains("@starweaver/host-protocol") {
            return Err(format!(
                "renderer must not import the complete host protocol package: {}",
                path.display()
            ));
        }
        if text.contains("@tauri-apps/api")
            && path != allowed_bridge
            && path != allowed_bridge_test
            && path != allowed_generated_host_client
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
        || bridge.matches("invoke<").count() != 16
        || bridge.matches("invoke(").count() != 3
        || bridge.matches("new Channel<DesktopActivation>").count() != 1
    {
        return Err(
            "Desktop bridge must use only the reviewed core invoke/channel surface".to_string(),
        );
    }
    for (constant, expected_uses) in [
        ("EXECUTE_HOST_OPERATION_COMMAND", 2),
        ("ACKNOWLEDGE_HOST_OPERATION_COMMAND", 3),
    ] {
        if bridge.matches(constant).count() != expected_uses {
            return Err(format!(
                "Desktop workspace grant bridge must use only the generated {constant} command"
            ));
        }
    }
    for (constant, command) in [
        ("GET_DESKTOP_STATUS_COMMAND", "get_desktop_status"),
        ("RETRY_MANAGED_RUNTIME_COMMAND", "retry_managed_runtime"),
        (
            "GET_RUNTIME_UPDATE_STATUS_COMMAND",
            "get_runtime_update_status",
        ),
        ("CHECK_RUNTIME_UPDATE_COMMAND", "check_runtime_update"),
        ("INSTALL_RUNTIME_UPDATE_COMMAND", "install_runtime_update"),
        ("ROLLBACK_RUNTIME_UPDATE_COMMAND", "rollback_runtime_update"),
        (
            "GET_DESKTOP_UPDATE_STATUS_COMMAND",
            "get_desktop_update_status",
        ),
        ("CHECK_DESKTOP_UPDATE_COMMAND", "check_desktop_update"),
        ("INSTALL_DESKTOP_UPDATE_COMMAND", "install_desktop_update"),
        ("GET_DESKTOP_PREFERENCES_COMMAND", "get_desktop_preferences"),
        (
            "UPDATE_DESKTOP_PREFERENCES_COMMAND",
            "update_desktop_preferences",
        ),
        (
            "RELOAD_DESKTOP_PREFERENCES_COMMAND",
            "reload_desktop_preferences",
        ),
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

    check_generated_host_surface(desktop_root, &allowed_generated_host_client)?;

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

fn check_generated_host_surface(desktop_root: &Path, client_path: &Path) -> Result<(), String> {
    let manifest = read_json(&desktop_root.join("host-bridge/manifest.yaml"))?;
    let operations = manifest
        .get("operations")
        .and_then(Value::as_object)
        .ok_or("Desktop host surface operations must be an object")?;
    if operations.len() != 38 {
        return Err(format!(
            "Desktop generated host surface must expose exactly 38 renderer user intents, found {}",
            operations.len()
        ));
    }
    for backend_owned in [
        "initialize",
        "shutdown",
        "events.replay",
        "events.subscribe",
        "events.unsubscribe",
        "environment.attach",
    ] {
        if operations.contains_key(backend_owned) {
            return Err(format!(
                "Desktop renderer surface must not expose backend-owned operation {backend_owned}"
            ));
        }
    }

    let client = fs::read_to_string(client_path).map_err(|error| error.to_string())?;
    if client.matches("@tauri-apps/api/core").count() != 1
        || client
            .matches("return this.execute(this.prepare({ kind:")
            .count()
            != operations.len()
        || client.matches("invoke(").count() != 6
        || client.contains("invoke(\"")
        || !client.contains("operationAcknowledgementToken")
        || !client.contains("acknowledgeOperation")
        || client.contains("@starweaver/host-protocol")
    {
        return Err(
            "generated Desktop host client must use only its closed operation union and six fixed commands"
                .to_string(),
        );
    }
    for (constant, command) in [
        ("ACKNOWLEDGE_HOST_EVENT_COMMAND", "acknowledge_host_event"),
        (
            "ACKNOWLEDGE_HOST_OPERATION_COMMAND",
            "acknowledge_host_operation",
        ),
        ("EXECUTE_HOST_OPERATION_COMMAND", "execute_host_operation"),
        (
            "LIST_PENDING_HOST_OPERATIONS_COMMAND",
            "list_pending_host_operations",
        ),
        ("SUBSCRIBE_HOST_EVENTS_COMMAND", "subscribe_host_events"),
        ("UNSUBSCRIBE_HOST_EVENTS_COMMAND", "unsubscribe_host_events"),
    ] {
        if !client.contains(&format!(
            "export const {constant} = \"{command}\" as const;"
        )) {
            return Err(format!(
                "generated Desktop host client omitted fixed command {command}"
            ));
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
        "allow-retry-managed-runtime",
        "allow-get-runtime-update-status",
        "allow-check-runtime-update",
        "allow-install-runtime-update",
        "allow-rollback-runtime-update",
        "allow-get-desktop-update-status",
        "allow-check-desktop-update",
        "allow-install-desktop-update",
        "allow-get-desktop-preferences",
        "allow-update-desktop-preferences",
        "allow-reload-desktop-preferences",
        "allow-subscribe-desktop-activation",
        "allow-unsubscribe-desktop-activation",
        "allow-get-desktop-window-route",
        "allow-open-conversation-window",
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

    let conversation_capability =
        read_json(&desktop_root.join("src-tauri/capabilities/conversation.json"))?;
    if string_set(&conversation_capability, "permissions")?
        != BTreeSet::from([
            "allow-get-desktop-status",
            "allow-get-desktop-preferences",
            "allow-get-desktop-window-route",
        ])
        || string_set(&conversation_capability, "windows")? != BTreeSet::from(["conversation-*"])
        || string_set(&conversation_capability, "platforms")?
            != BTreeSet::from(["linux", "macOS", "windows"])
    {
        return Err(
            "Desktop conversation capability must remain backend-routed and least-authority"
                .to_string(),
        );
    }

    let generated_capability =
        read_json(&desktop_root.join("src-tauri/capabilities/generated-host.json"))?;
    let generated_permissions = string_set(&generated_capability, "permissions")?;
    let expected_generated_permissions = BTreeSet::from([
        "allow-acknowledge-host-event",
        "allow-acknowledge-host-operation",
        "allow-execute-host-operation",
        "allow-list-pending-host-operations",
        "allow-subscribe-host-events",
        "allow-unsubscribe-host-events",
    ]);
    if generated_permissions != expected_generated_permissions
        || string_set(&generated_capability, "windows")?
            != BTreeSet::from(["main", "conversation-*"])
    {
        return Err(format!(
            "generated Desktop host capability has unexpected permissions or window roles: {generated_permissions:?}"
        ));
    }
    for (file, identifier, command) in [
        (
            "acknowledge_host_event.toml",
            "allow-acknowledge-host-event",
            "acknowledge_host_event",
        ),
        (
            "acknowledge_host_operation.toml",
            "allow-acknowledge-host-operation",
            "acknowledge_host_operation",
        ),
        (
            "execute_host_operation.toml",
            "allow-execute-host-operation",
            "execute_host_operation",
        ),
        (
            "list_pending_host_operations.toml",
            "allow-list-pending-host-operations",
            "list_pending_host_operations",
        ),
        (
            "subscribe_host_events.toml",
            "allow-subscribe-host-events",
            "subscribe_host_events",
        ),
        (
            "unsubscribe_host_events.toml",
            "allow-unsubscribe-host-events",
            "unsubscribe_host_events",
        ),
    ] {
        let permission = fs::read_to_string(
            desktop_root
                .join("src-tauri/permissions/autogenerated")
                .join(file),
        )
        .map_err(|error| error.to_string())?;
        if !permission.contains(&format!("identifier = \"{identifier}\""))
            || !permission.contains(&format!("commands.allow = [\"{command}\"]"))
        {
            return Err(format!(
                "generated Desktop permission {file} does not match {command}"
            ));
        }
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
    if security.get("capabilities")
        != Some(&Value::Array(vec![
            Value::String("main".to_string()),
            Value::String("conversation".to_string()),
            Value::String("generated-host".to_string()),
        ]))
    {
        return Err(
            "Desktop must explicitly select only the reviewed shell and generated host capabilities"
                .to_string(),
        );
    }

    let build_script = fs::read_to_string(desktop_root.join("src-tauri/build.rs"))
        .map_err(|error| error.to_string())?;
    for command in [
        "get_desktop_status",
        "retry_managed_runtime",
        "get_runtime_update_status",
        "check_runtime_update",
        "install_runtime_update",
        "rollback_runtime_update",
        "get_desktop_update_status",
        "check_desktop_update",
        "install_desktop_update",
        "subscribe_desktop_activation",
        "unsubscribe_desktop_activation",
        "get_desktop_window_route",
        "open_conversation_window",
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

    #[test]
    fn parses_release_dependencies_and_detects_transitive_desktop_blocking() {
        let workflow = r"
jobs:
  build-binaries:
    runs-on: ubuntu-latest
  build-desktop-artifacts:
    runs-on: ubuntu-latest
  upload-core-assets:
    needs: [build-binaries]
  publish-crates:
    needs: upload-core-assets
";
        let Ok(jobs) = parse_release_job_dependencies(workflow) else {
            panic!("release jobs must parse");
        };
        assert!(release_job_depends_on(
            &jobs,
            "publish-crates",
            "build-binaries",
            &mut BTreeSet::new(),
        ));
        assert!(!release_job_depends_on(
            &jobs,
            "publish-crates",
            "build-desktop-artifacts",
            &mut BTreeSet::new(),
        ));

        let blocked = workflow.replace(
            "needs: [build-binaries]",
            "needs: [build-binaries, build-desktop-artifacts]",
        );
        let Ok(jobs) = parse_release_job_dependencies(&blocked) else {
            panic!("blocked release jobs must parse");
        };
        assert!(release_job_depends_on(
            &jobs,
            "publish-crates",
            "build-desktop-artifacts",
            &mut BTreeSet::new(),
        ));
    }

    #[test]
    fn rejects_unknown_release_dependency() {
        let workflow = "jobs:\n  publish-crates:\n    needs: missing-assets\n";

        assert!(parse_release_job_dependencies(workflow).is_err());
    }
}
