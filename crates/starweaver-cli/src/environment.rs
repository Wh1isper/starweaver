//! CLI environment provider resolution.

use std::{path::PathBuf, sync::Arc};

use starweaver_environment::{
    DynEnvironmentProvider, EnvironmentPolicy, FilePolicy, LocalEnvironmentProvider, ShellPolicy,
    VirtualEnvironmentProvider,
};

use crate::{CliConfig, CliError, CliResult};

/// Resolved environment provider for one CLI run.
#[derive(Clone)]
pub struct ResolvedEnvironment {
    /// Provider handle attached to `AgentSession`.
    pub provider: DynEnvironmentProvider,
}

/// Build an environment provider from resolved CLI config.
pub fn resolve_environment(config: &CliConfig) -> CliResult<ResolvedEnvironment> {
    let policy = environment_policy(config)?;
    let provider: DynEnvironmentProvider = match config.environment_provider.as_str() {
        "local" => Arc::new(
            LocalEnvironmentProvider::new(config.workspace_root.clone())
                .with_id("cli-local")
                .with_allowed_paths(local_allowed_paths(config))
                .with_policy(policy),
        ),
        "virtual" => Arc::new(
            VirtualEnvironmentProvider::new("cli-virtual")
                .with_policy(policy)
                .with_file("README.md", "Virtual Starweaver CLI workspace"),
        ),
        other => {
            return Err(CliError::Config(format!(
                "unknown environment provider: {other}"
            )))
        }
    };
    Ok(ResolvedEnvironment { provider })
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
