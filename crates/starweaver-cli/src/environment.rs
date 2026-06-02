//! CLI environment provider resolution.

use std::sync::Arc;

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
