use std::{collections::BTreeMap, process::Command};

use serde_json::Value;

use crate::common::root;

const DESKTOP_PACKAGE: &str = "starweaver-desktop";
const CLI_PACKAGE: &str = "starweaver-cli";
const RPC_PACKAGE: &str = "starweaver-rpc";
const AGENT_PACKAGE: &str = "starweaver-agent";
const SESSION_PACKAGE: &str = "starweaver-session";
const STORAGE_PACKAGE: &str = "starweaver-storage";
const STREAM_PACKAGE: &str = "starweaver-stream";
const RUNTIME_PACKAGE: &str = "starweaver-runtime";
const CONTEXT_PACKAGE: &str = "starweaver-context";
const ENVIRONMENT_PACKAGE: &str = "starweaver-environment";

pub fn check_boundaries() -> Result<(), String> {
    let root = root()?;
    let output = Command::new("cargo")
        .current_dir(&root)
        .args(["metadata", "--format-version", "1", "--no-deps", "--locked"])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let metadata: Value =
        serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
    let packages = metadata
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| "cargo metadata did not contain packages".to_string())?;
    let workspace_names = packages
        .iter()
        .filter_map(|package| package.get("name").and_then(Value::as_str))
        .collect::<std::collections::BTreeSet<_>>();
    ensure_workspace_packages(
        &workspace_names,
        &[
            DESKTOP_PACKAGE,
            CLI_PACKAGE,
            RPC_PACKAGE,
            AGENT_PACKAGE,
            SESSION_PACKAGE,
            STORAGE_PACKAGE,
            STREAM_PACKAGE,
            RUNTIME_PACKAGE,
            CONTEXT_PACKAGE,
            ENVIRONMENT_PACKAGE,
        ],
    )?;
    let mut graph = BTreeMap::<String, Vec<String>>::new();
    let mut normal_dependency_graph = BTreeMap::<String, Vec<String>>::new();
    for package in packages {
        let name = package
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "cargo metadata package did not contain a name".to_string())?;
        let dependencies = package
            .get("dependencies")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("cargo metadata package {name} omitted dependencies"))?;
        let mut workspace_dependencies = Vec::new();
        let mut normal_workspace_dependencies = Vec::new();
        for dependency in dependencies {
            let dependency_name = dependency
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("cargo metadata dependency for {name} omitted its name"))?;
            if name == CLI_PACKAGE && dependency_name == "rusqlite" {
                return Err(
                    "starweaver-cli must use starweaver-storage and cannot depend directly on rusqlite"
                        .to_string(),
                );
            }
            if name == SESSION_PACKAGE && dependency_name == ENVIRONMENT_PACKAGE {
                return Err(
                    "starweaver-session owns durable references and cannot depend on environment implementations"
                        .to_string(),
                );
            }
            if name == STREAM_PACKAGE && dependency_name == CONTEXT_PACKAGE {
                return Err(
                    "starweaver-stream owns display/replay contracts and cannot depend directly on mutable agent context"
                        .to_string(),
                );
            }
            if workspace_names.contains(dependency_name) {
                workspace_dependencies.push(dependency_name.to_string());
                if dependency.get("kind").is_none_or(Value::is_null) {
                    normal_workspace_dependencies.push(dependency_name.to_string());
                }
            }
        }
        graph.insert(name.to_string(), workspace_dependencies);
        normal_dependency_graph.insert(name.to_string(), normal_workspace_dependencies);
    }
    ensure_no_path(&graph, CLI_PACKAGE, RPC_PACKAGE)?;
    ensure_no_path(&graph, RPC_PACKAGE, CLI_PACKAGE)?;
    for forbidden in [
        CLI_PACKAGE,
        RPC_PACKAGE,
        AGENT_PACKAGE,
        RUNTIME_PACKAGE,
        STORAGE_PACKAGE,
    ] {
        ensure_no_path(&graph, DESKTOP_PACKAGE, forbidden)?;
    }
    ensure_no_direct_dependency(&graph, SESSION_PACKAGE, RUNTIME_PACKAGE)?;
    ensure_no_path(&normal_dependency_graph, SESSION_PACKAGE, RUNTIME_PACKAGE)?;
    ensure_no_path(&normal_dependency_graph, STORAGE_PACKAGE, RUNTIME_PACKAGE)?;
    ensure_direct_dependency(&normal_dependency_graph, RUNTIME_PACKAGE, STREAM_PACKAGE)?;
    ensure_no_path(&graph, STREAM_PACKAGE, RUNTIME_PACKAGE)?;
    println!(
        "architecture boundaries passed: CLI and RPC are independent; Desktop does not link CLI, RPC host, agent, runtime, or storage implementations; CLI has no direct rusqlite dependency; session has no normal dependency path or direct dependency of any kind to runtime and no direct environment implementation dependency; storage has no normal dependency path to runtime; runtime directly consumes stream-owned protocol contracts; stream has no direct mutable context dependency and no dependency path to runtime"
    );
    Ok(())
}

fn ensure_workspace_packages(
    workspace_names: &std::collections::BTreeSet<&str>,
    required: &[&str],
) -> Result<(), String> {
    for package in required {
        if !workspace_names.contains(package) {
            return Err(format!(
                "architecture check requires workspace package {package}, but cargo metadata omitted it"
            ));
        }
    }
    Ok(())
}

fn ensure_no_direct_dependency(
    graph: &BTreeMap<String, Vec<String>>,
    source: &str,
    target: &str,
) -> Result<(), String> {
    if graph
        .get(source)
        .is_some_and(|dependencies| dependencies.iter().any(|dependency| dependency == target))
    {
        return Err(format!("forbidden direct dependency: {source} -> {target}"));
    }
    Ok(())
}

fn ensure_direct_dependency(
    graph: &BTreeMap<String, Vec<String>>,
    source: &str,
    target: &str,
) -> Result<(), String> {
    if graph
        .get(source)
        .is_some_and(|dependencies| dependencies.iter().any(|dependency| dependency == target))
    {
        return Ok(());
    }
    Err(format!(
        "required direct normal dependency is missing: {source} -> {target}"
    ))
}

fn ensure_no_path(
    graph: &BTreeMap<String, Vec<String>>,
    source: &str,
    target: &str,
) -> Result<(), String> {
    let mut pending = vec![(source.to_string(), vec![source.to_string()])];
    let mut visited = std::collections::BTreeSet::new();
    while let Some((current, path)) = pending.pop() {
        if !visited.insert(current.clone()) {
            continue;
        }
        for dependency in graph.get(&current).into_iter().flatten() {
            let mut dependency_path = path.clone();
            dependency_path.push(dependency.clone());
            if dependency == target {
                return Err(format!(
                    "forbidden product dependency path: {}",
                    dependency_path.join(" -> ")
                ));
            }
            pending.push((dependency.clone(), dependency_path));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_missing_required_workspace_package() {
        let packages = std::collections::BTreeSet::from([CLI_PACKAGE]);
        let Err(error) = ensure_workspace_packages(&packages, &[CLI_PACKAGE, RPC_PACKAGE]) else {
            panic!("missing required package must fail the architecture gate");
        };
        assert!(error.contains(RPC_PACKAGE));
    }

    #[test]
    fn rejects_session_to_runtime_normal_dependency_path() {
        let normal_graph = BTreeMap::from([
            (SESSION_PACKAGE.to_string(), vec!["shared".to_string()]),
            ("shared".to_string(), vec![RUNTIME_PACKAGE.to_string()]),
            (RUNTIME_PACKAGE.to_string(), Vec::new()),
        ]);
        let Err(error) = ensure_no_path(&normal_graph, SESSION_PACKAGE, RUNTIME_PACKAGE) else {
            panic!("session-to-runtime normal dependency path must be rejected");
        };
        assert!(error.contains("starweaver-session -> shared -> starweaver-runtime"));
    }

    #[test]
    fn allows_transitive_owner_dev_dependency_outside_normal_graph() {
        let all_kinds_graph = BTreeMap::from([
            (
                SESSION_PACKAGE.to_string(),
                vec![CONTEXT_PACKAGE.to_string()],
            ),
            (
                CONTEXT_PACKAGE.to_string(),
                vec![RUNTIME_PACKAGE.to_string()],
            ),
        ]);
        let normal_graph = BTreeMap::from([
            (
                SESSION_PACKAGE.to_string(),
                vec![CONTEXT_PACKAGE.to_string()],
            ),
            (CONTEXT_PACKAGE.to_string(), Vec::new()),
        ]);

        assert!(
            ensure_no_direct_dependency(&all_kinds_graph, SESSION_PACKAGE, RUNTIME_PACKAGE).is_ok()
        );
        assert!(ensure_no_path(&normal_graph, SESSION_PACKAGE, RUNTIME_PACKAGE).is_ok());
    }

    #[test]
    fn rejects_direct_session_dev_or_build_dependency_on_runtime() {
        let all_kinds_graph = BTreeMap::from([(
            SESSION_PACKAGE.to_string(),
            vec![RUNTIME_PACKAGE.to_string()],
        )]);
        let Err(error) =
            ensure_no_direct_dependency(&all_kinds_graph, SESSION_PACKAGE, RUNTIME_PACKAGE)
        else {
            panic!("direct session-to-runtime dependency of any kind must be rejected");
        };
        assert!(error.contains("starweaver-session -> starweaver-runtime"));
    }

    #[test]
    fn rejects_storage_to_runtime_normal_dependency_path() {
        let graph = BTreeMap::from([
            (STORAGE_PACKAGE.to_string(), vec!["shared".to_string()]),
            ("shared".to_string(), vec![RUNTIME_PACKAGE.to_string()]),
            (RUNTIME_PACKAGE.to_string(), Vec::new()),
        ]);
        let Err(error) = ensure_no_path(&graph, STORAGE_PACKAGE, RUNTIME_PACKAGE) else {
            panic!("storage-to-runtime normal dependency path must be rejected");
        };
        assert!(error.contains("starweaver-storage -> shared -> starweaver-runtime"));
    }

    #[test]
    fn rejects_missing_runtime_to_stream_dependency() {
        let graph = BTreeMap::from([
            (RUNTIME_PACKAGE.to_string(), vec!["shared".to_string()]),
            (STREAM_PACKAGE.to_string(), Vec::new()),
        ]);
        let Err(error) = ensure_direct_dependency(&graph, RUNTIME_PACKAGE, STREAM_PACKAGE) else {
            panic!("missing runtime-to-stream dependency must fail the architecture gate");
        };
        assert!(error.contains("starweaver-runtime -> starweaver-stream"));
    }

    #[test]
    fn accepts_direct_runtime_to_stream_dependency() {
        let graph = BTreeMap::from([(
            RUNTIME_PACKAGE.to_string(),
            vec![STREAM_PACKAGE.to_string()],
        )]);
        assert!(ensure_direct_dependency(&graph, RUNTIME_PACKAGE, STREAM_PACKAGE).is_ok());
    }

    #[test]
    fn rejects_stream_to_runtime_dependency_path() {
        let graph = BTreeMap::from([
            (STREAM_PACKAGE.to_string(), vec!["shared".to_string()]),
            ("shared".to_string(), vec![RUNTIME_PACKAGE.to_string()]),
            (RUNTIME_PACKAGE.to_string(), Vec::new()),
        ]);
        let Err(error) = ensure_no_path(&graph, STREAM_PACKAGE, RUNTIME_PACKAGE) else {
            panic!("stream-to-runtime dependency path must be rejected");
        };
        assert!(error.contains("starweaver-stream -> shared -> starweaver-runtime"));
    }

    #[test]
    fn rejects_indirect_product_dependency() {
        let graph = BTreeMap::from([
            (CLI_PACKAGE.to_string(), vec!["shared".to_string()]),
            ("shared".to_string(), vec![RPC_PACKAGE.to_string()]),
            (RPC_PACKAGE.to_string(), Vec::new()),
        ]);
        let Err(error) = ensure_no_path(&graph, CLI_PACKAGE, RPC_PACKAGE) else {
            panic!("indirect product dependency must be rejected");
        };
        assert!(error.contains("starweaver-cli -> shared -> starweaver-rpc"));
    }
}
