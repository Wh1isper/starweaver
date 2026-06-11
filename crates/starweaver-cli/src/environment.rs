//! CLI environment provider resolution.

use std::{path::PathBuf, sync::Arc};

use starweaver_environment::{
    DynEnvironmentProvider, DynProcessShellProvider, EnvironmentPolicy, FilePolicy,
    LocalEnvironmentProvider, ShellPolicy, VirtualEnvironmentProvider,
};

use crate::{CliConfig, CliError, CliResult};

/// Resolved environment provider for one CLI run.
#[derive(Clone)]
pub struct ResolvedEnvironment {
    /// Provider handle attached to `AgentSession`.
    pub provider: DynEnvironmentProvider,
    /// Optional process-capable provider override for background shell tools.
    pub process_provider: Option<DynProcessShellProvider>,
}

/// Validate environment configuration before creating run/session records.
pub fn validate_environment_config(config: &CliConfig) -> CliResult<()> {
    let _policy = environment_policy(config)?;
    validate_environment_provider(config.environment_provider.as_str())
}

/// Build an environment provider from resolved CLI config.
#[cfg(test)]
pub fn resolve_environment(config: &CliConfig) -> CliResult<ResolvedEnvironment> {
    resolve_environment_with_tmp_namespace(config, None)
}

/// Build an environment provider with a session-scoped temporary file namespace.
pub fn resolve_environment_for_session(
    config: &CliConfig,
    session_id: &str,
) -> CliResult<ResolvedEnvironment> {
    resolve_environment_with_tmp_namespace(config, Some(session_id))
}

fn resolve_environment_with_tmp_namespace(
    config: &CliConfig,
    tmp_namespace: Option<&str>,
) -> CliResult<ResolvedEnvironment> {
    validate_environment_config(config)?;
    let policy = environment_policy(config)?;
    let provider: DynEnvironmentProvider = match config.environment_provider.as_str() {
        "local" => {
            let mut provider = LocalEnvironmentProvider::new(config.workspace_root.clone())
                .with_id("cli-local")
                .with_allowed_paths(local_allowed_paths(config))
                .with_policy(policy);
            if let Some(namespace) = tmp_namespace {
                provider = provider.with_tmp_namespace(namespace);
            }
            Arc::new(provider)
        }
        "virtual" => {
            let mut provider = VirtualEnvironmentProvider::new("cli-virtual")
                .with_policy(policy)
                .with_file("README.md", "Virtual Starweaver CLI workspace");
            if let Some(namespace) = tmp_namespace {
                provider = provider.with_tmp_namespace(namespace);
            }
            Arc::new(provider)
        }
        other => {
            unreachable!("environment provider should be validated before resolution: {other}")
        }
    };
    let process_provider = provider.clone().process_shell_provider();
    Ok(ResolvedEnvironment {
        provider,
        process_provider,
    })
}

fn local_allowed_paths(config: &CliConfig) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    push_allowed_path(&mut paths, config.global_dir.clone());
    if let Some(home) = std::env::var_os("HOME") {
        push_allowed_path(&mut paths, PathBuf::from(home).join(".agents"));
    }
    push_allowed_path(&mut paths, config.workspace_root.clone());
    push_allowed_path(&mut paths, config.project_dir.clone());
    for dir in config.skill_dirs.iter().chain(config.subagent_dirs.iter()) {
        push_allowed_path(&mut paths, dir.clone());
    }
    paths
}

fn push_allowed_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    let path = path.canonicalize().unwrap_or(path);
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn validate_environment_provider(provider: &str) -> CliResult<()> {
    match provider {
        "local" | "virtual" => Ok(()),
        other => Err(CliError::Config(format!(
            "unknown environment provider: {other}"
        ))),
    }
}

fn environment_policy(config: &CliConfig) -> CliResult<EnvironmentPolicy> {
    let files = match config.files_policy.as_str() {
        "read_only" | "read-only" => FilePolicy::read_only(),
        "read_write" | "read-write" => FilePolicy::read_write(),
        "none" | "disabled" => FilePolicy::default(),
        other => return Err(CliError::Config(format!("unknown files policy: {other}"))),
    };
    let shell = if config.shell_enabled {
        ShellPolicy::allow_all()
    } else {
        ShellPolicy::default()
    };
    Ok(EnvironmentPolicy { files, shell })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::path::Path;

    use super::*;
    use crate::{args, profiles::list_skills, ConfigResolver};

    #[tokio::test]
    async fn cli_local_environment_can_read_configured_skill_package_paths() {
        let temp = tempfile::tempdir().unwrap();
        let global_dir = temp.path().join("global");
        std::fs::create_dir_all(&global_dir).unwrap();
        std::fs::write(
            global_dir.join("config.toml"),
            r#"
[skills]
additional_dirs = ["../custom-skills"]
"#,
        )
        .unwrap();

        let cli = args::parse(["starweaver-cli".to_string()]).unwrap();
        let config = ConfigResolver::for_tests(temp.path())
            .resolve(&cli)
            .unwrap();
        write_skill(
            &config.global_dir.join("skills/global-skill/SKILL.md"),
            "global-skill",
            "Global config skill",
        );
        write_skill(
            &temp
                .path()
                .join("shared-agents/skills/shared-skill/SKILL.md"),
            "shared-skill",
            "Shared agent skill",
        );
        write_skill(
            &config.project_dir.join("skills/project-skill/SKILL.md"),
            "project-skill",
            "Project config skill",
        );
        write_skill(
            &temp.path().join("custom-skills/custom-skill/SKILL.md"),
            "custom-skill",
            "Custom skill dir",
        );

        let packages = list_skills(&config);
        assert_eq!(packages.len(), 4);
        assert!(config
            .skill_dirs
            .iter()
            .any(|path| path.ends_with("shared-agents/skills")));

        let environment = resolve_environment(&config).unwrap();
        for package in packages {
            let content = environment.provider.read_text(&package.path).await.unwrap();
            assert!(content.contains(&format!("name: {}", package.name)));
        }
    }

    #[tokio::test]
    async fn cli_local_environment_tmp_outputs_are_readable_without_allowing_all_tmp() {
        let temp = tempfile::tempdir().unwrap();
        let cli = args::parse(["starweaver-cli".to_string()]).unwrap();
        let config = ConfigResolver::for_tests(temp.path())
            .resolve(&cli)
            .unwrap();
        let environment = resolve_environment_for_session(&config, "session_123").unwrap();

        let tmp_path = environment
            .provider
            .write_tmp_file("stdout.log", b"captured output")
            .await
            .unwrap();
        assert!(Path::new(&tmp_path).is_absolute());
        assert!(tmp_path.contains("session_123"));
        assert_eq!(
            Path::new(&tmp_path).file_name().unwrap().to_string_lossy(),
            "stdout.log"
        );
        assert_eq!(
            environment.provider.read_text(&tmp_path).await.unwrap(),
            "captured output"
        );

        let unrelated_tmp =
            std::env::temp_dir().join(format!("starweaver-cli-unrelated-{}", std::process::id()));
        std::fs::write(&unrelated_tmp, "secret").unwrap();
        assert!(matches!(
            environment
                .provider
                .read_text(&unrelated_tmp.display().to_string())
                .await,
            Err(starweaver_environment::EnvironmentError::AccessDenied(_))
        ));
        let _ = std::fs::remove_file(unrelated_tmp);
    }

    fn write_skill(path: &Path, name: &str, description: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            format!(
                r"---
name: {name}
description: {description}
---
Use this skill.
"
            ),
        )
        .unwrap();
    }
}
