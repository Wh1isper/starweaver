//! CLI environment provider resolution.

use std::{path::PathBuf, sync::Arc};

use starweaver_envd::LocalEnvd;
use starweaver_envd_client::EnvdRpcClient;
use starweaver_envd_core::DEFAULT_ENVIRONMENT_ID;
use starweaver_environment::{
    CompositeEnvironmentProvider, DynEnvironmentProvider, DynProcessShellProvider,
    EnvdEnvironmentProvider, EnvironmentMount, EnvironmentMountMode, EnvironmentPolicy, FilePolicy,
    LocalEnvironmentProvider, ShellPolicy, SwitchableEnvironmentProvider,
    SwitchableEnvironmentTarget, VirtualEnvironmentProvider,
};
use starweaver_rpc_core::{
    EnvironmentAttachmentAccessMode, EnvironmentAttachmentRef, LOCAL_ENVIRONMENT_ATTACHMENT_ID,
    LOCAL_ENVIRONMENT_ATTACHMENT_KIND,
};

use crate::{CliConfig, CliError, CliResult};

/// Resolved environment provider for one CLI run.
#[derive(Clone)]
pub struct ResolvedEnvironment {
    /// Provider handle attached to `AgentSession`.
    pub provider: DynEnvironmentProvider,
    /// Optional process-capable provider override for background shell tools.
    pub process_provider: Option<DynProcessShellProvider>,
    /// Switchable provider handle used by active-run host mutations.
    pub switchable: Option<Arc<SwitchableEnvironmentProvider>>,
    /// Effective run-local attachment refs backing this environment.
    pub attachments: Vec<EnvironmentAttachmentRef>,
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

/// Build an environment provider for a session, optionally using host RPC attachments.
pub fn resolve_environment_for_session_with_attachments(
    config: &CliConfig,
    session_id: &str,
    attachments: &[EnvironmentAttachmentRef],
) -> CliResult<ResolvedEnvironment> {
    let target =
        resolve_environment_target_for_session_with_attachments(config, session_id, attachments)?;
    let switchable = Arc::new(SwitchableEnvironmentProvider::new(
        "cli-active-environment",
        SwitchableEnvironmentTarget::new(target.provider.clone(), target.process_provider.clone()),
    ));
    let provider: DynEnvironmentProvider = switchable.clone();
    let process_provider = target
        .process_provider
        .is_some()
        .then(|| switchable.clone() as DynProcessShellProvider);
    Ok(ResolvedEnvironment {
        provider,
        process_provider,
        switchable: Some(switchable),
        attachments: target.attachments,
    })
}

/// Build a non-switchable target for a session and attachment list.
pub fn resolve_environment_target_for_session_with_attachments(
    config: &CliConfig,
    session_id: &str,
    attachments: &[EnvironmentAttachmentRef],
) -> CliResult<ResolvedEnvironment> {
    if attachments.is_empty() {
        let mut resolved = resolve_environment_for_session(config, session_id)?;
        resolved.attachments = vec![default_local_attachment()];
        return Ok(resolved);
    }
    let default_id = default_attachment_id(attachments);
    let default_shell_id = default_shell_attachment_id(attachments, default_id);
    let mut effective_attachments = attachments.to_vec();
    for attachment in &mut effective_attachments {
        attachment.is_default = default_id == Some(attachment.id.as_str());
        attachment.is_default_for_shell = default_shell_id == Some(attachment.id.as_str());
    }
    let mut mounts = Vec::new();
    for attachment in &effective_attachments {
        let resolved = resolve_environment_attachment(config, Some(session_id), attachment)?;
        mounts.push(
            EnvironmentMount::new(&attachment.id, resolved.provider)
                .map_err(|error| CliError::Config(error.to_string()))?
                .with_mode(environment_mount_mode(attachment.resolved_mode()))
                .with_default(attachment.is_default)
                .with_default_for_shell(attachment.is_default_for_shell),
        );
    }
    let provider: DynEnvironmentProvider = Arc::new(
        CompositeEnvironmentProvider::new(mounts)
            .map_err(|error| CliError::Config(error.to_string()))?,
    );
    let process_provider = provider.clone().process_shell_provider();
    Ok(ResolvedEnvironment {
        provider,
        process_provider,
        switchable: None,
        attachments: effective_attachments,
    })
}

fn default_attachment_id(attachments: &[EnvironmentAttachmentRef]) -> Option<&str> {
    if attachments.len() == 1 {
        return Some(attachments[0].id.as_str());
    }
    attachments
        .iter()
        .find(|attachment| attachment.is_default)
        .map(|attachment| attachment.id.as_str())
}

fn default_shell_attachment_id<'a>(
    attachments: &'a [EnvironmentAttachmentRef],
    default_id: Option<&'a str>,
) -> Option<&'a str> {
    if let Some(explicit) = attachments
        .iter()
        .find(|attachment| attachment.is_default_for_shell)
        .map(|attachment| attachment.id.as_str())
    {
        return Some(explicit);
    }
    let default_id = default_id?;
    attachments
        .iter()
        .find(|attachment| attachment.id == default_id && attachment_supports_shell(attachment))
        .map(|attachment| attachment.id.as_str())
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
            envd_backed_provider(Arc::new(provider), "cli-local")
        }
        "virtual" => {
            let mut provider = VirtualEnvironmentProvider::new("cli-virtual")
                .with_policy(policy)
                .with_file("README.md", "Virtual Starweaver CLI workspace");
            if let Some(namespace) = tmp_namespace {
                provider = provider.with_tmp_namespace(namespace);
            }
            envd_backed_provider(Arc::new(provider), "cli-virtual")
        }
        other => {
            unreachable!("environment provider should be validated before resolution: {other}")
        }
    };
    let process_provider = provider.clone().process_shell_provider();
    Ok(ResolvedEnvironment {
        provider,
        process_provider,
        switchable: None,
        attachments: vec![default_local_attachment()],
    })
}

fn default_local_attachment() -> EnvironmentAttachmentRef {
    EnvironmentAttachmentRef {
        id: LOCAL_ENVIRONMENT_ATTACHMENT_ID.to_string(),
        kind: LOCAL_ENVIRONMENT_ATTACHMENT_KIND.to_string(),
        mode: Some(EnvironmentAttachmentAccessMode::ReadWrite),
        is_default: true,
        is_default_for_shell: true,
        attachment_lease_id: None,
        endpoint_ref: None,
        environment_id: None,
        auth_token: None,
        metadata: serde_json::Map::new(),
    }
}

fn resolve_environment_attachment(
    config: &CliConfig,
    tmp_namespace: Option<&str>,
    attachment: &EnvironmentAttachmentRef,
) -> CliResult<ResolvedEnvironment> {
    if attachment.id == LOCAL_ENVIRONMENT_ATTACHMENT_ID
        && attachment.kind != LOCAL_ENVIRONMENT_ATTACHMENT_KIND
    {
        return Err(CliError::Config(
            "reserved environment attachment id local requires kind local".to_string(),
        ));
    }
    match attachment.kind.as_str() {
        "local" => resolve_environment_with_tmp_namespace(config, tmp_namespace),
        "envd" => resolve_envd_attachment(attachment),
        other => Err(CliError::Config(format!(
            "unsupported environment attachment kind: {other}"
        ))),
    }
}

fn resolve_envd_attachment(
    attachment: &EnvironmentAttachmentRef,
) -> CliResult<ResolvedEnvironment> {
    let endpoint = attachment
        .requested_endpoint_ref()
        .ok_or_else(|| CliError::Config("envd attachment requires endpointRef".to_string()))?;
    let auth_token = attachment
        .requested_auth_token()
        .ok_or_else(|| CliError::Config("envd attachment requires authToken".to_string()))?;
    let client = EnvdRpcClient::http_with_token(endpoint, auth_token)
        .map_err(|error| CliError::Config(format!("invalid envd endpoint: {error}")))?;
    let environment_id = attachment
        .requested_environment_id()
        .unwrap_or(DEFAULT_ENVIRONMENT_ID)
        .to_string();
    let provider: DynEnvironmentProvider = Arc::new(
        EnvdEnvironmentProvider::new(Arc::new(client), environment_id).with_id(&attachment.id),
    );
    let process_provider = provider.clone().process_shell_provider();
    Ok(ResolvedEnvironment {
        provider,
        process_provider,
        switchable: None,
        attachments: vec![attachment.clone()],
    })
}

const fn environment_mount_mode(mode: EnvironmentAttachmentAccessMode) -> EnvironmentMountMode {
    match mode {
        EnvironmentAttachmentAccessMode::ReadOnly => EnvironmentMountMode::ReadOnly,
        EnvironmentAttachmentAccessMode::ReadWrite => EnvironmentMountMode::ReadWrite,
    }
}

fn attachment_supports_shell(attachment: &EnvironmentAttachmentRef) -> bool {
    matches!(
        attachment.resolved_mode(),
        EnvironmentAttachmentAccessMode::ReadWrite
    )
}

fn envd_backed_provider(provider: DynEnvironmentProvider, id: &str) -> DynEnvironmentProvider {
    let shell_review_context = provider.shell_review_context();
    let envd = Arc::new(LocalEnvd::new(provider));
    let environment_id = envd.environment_id().to_string();
    Arc::new(
        EnvdEnvironmentProvider::new(envd, environment_id)
            .with_id(id)
            .with_shell_review_context(shell_review_context),
    )
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
        assert_eq!(environment.provider.id(), "cli-local");
        let state = environment.provider.export_state().await.unwrap();
        assert_eq!(
            state.metadata["envd_environment_id"],
            serde_json::json!("env_cli_default")
        );
        assert_eq!(state.metadata["envd_store"], serde_json::json!("ephemeral"));
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
        let state = environment.provider.export_state().await.unwrap();
        assert_eq!(
            state.metadata["envd_operation_ids"]
                .as_array()
                .unwrap()
                .len(),
            1
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

    #[tokio::test]
    async fn cli_resolves_single_environment_attachment_as_composite_provider() {
        let temp = tempfile::tempdir().unwrap();
        let cli = args::parse(["starweaver-cli".to_string()]).unwrap();
        let config = ConfigResolver::for_tests(temp.path())
            .resolve(&cli)
            .unwrap();
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        std::fs::write(config.workspace_root.join("README.md"), "local workspace").unwrap();
        let attachments = vec![EnvironmentAttachmentRef {
            id: "workspace".to_string(),
            kind: "local".to_string(),
            mode: Some(EnvironmentAttachmentAccessMode::ReadOnly),
            is_default: false,
            is_default_for_shell: false,
            attachment_lease_id: None,
            endpoint_ref: None,
            environment_id: None,
            auth_token: None,
            metadata: serde_json::Map::new(),
        }];

        let environment =
            resolve_environment_for_session_with_attachments(&config, "session_123", &attachments)
                .unwrap();

        assert_eq!(environment.provider.id(), "cli-active-environment");
        assert!(environment.switchable.is_some());
        assert_eq!(environment.attachments[0].id, "workspace");
        assert!(environment.attachments[0].is_default);
        assert!(!environment.attachments[0].is_default_for_shell);
        assert_eq!(
            environment.provider.read_text("README.md").await.unwrap(),
            "local workspace"
        );
        assert_eq!(
            environment
                .provider
                .read_text("/environment/workspace/README.md")
                .await
                .unwrap(),
            "local workspace"
        );
        assert!(matches!(
            environment.provider.write_text("new.txt", "blocked").await,
            Err(starweaver_environment::EnvironmentError::AccessDenied(_))
        ));
        assert!(matches!(
            environment
                .provider
                .write_text("/environment/workspace/new.txt", "blocked")
                .await,
            Err(starweaver_environment::EnvironmentError::AccessDenied(_))
        ));
    }

    #[tokio::test]
    async fn cli_resolves_multiple_environment_attachments_as_composite_provider() {
        let temp = tempfile::tempdir().unwrap();
        let cli = args::parse(["starweaver-cli".to_string()]).unwrap();
        let config = ConfigResolver::for_tests(temp.path())
            .resolve(&cli)
            .unwrap();
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        std::fs::write(config.workspace_root.join("README.md"), "local workspace").unwrap();
        let attachments = vec![
            EnvironmentAttachmentRef {
                id: "workspace".to_string(),
                kind: "local".to_string(),
                mode: Some(EnvironmentAttachmentAccessMode::ReadWrite),
                is_default: true,
                is_default_for_shell: true,
                attachment_lease_id: None,
                endpoint_ref: None,
                environment_id: None,
                auth_token: None,
                metadata: serde_json::Map::new(),
            },
            EnvironmentAttachmentRef {
                id: "tools".to_string(),
                kind: "local".to_string(),
                mode: Some(EnvironmentAttachmentAccessMode::ReadOnly),
                is_default: false,
                is_default_for_shell: false,
                attachment_lease_id: None,
                endpoint_ref: None,
                environment_id: None,
                auth_token: None,
                metadata: serde_json::Map::new(),
            },
        ];

        let environment =
            resolve_environment_for_session_with_attachments(&config, "session_123", &attachments)
                .unwrap();

        assert_eq!(environment.provider.id(), "cli-active-environment");
        assert!(environment.switchable.is_some());
        assert!(environment.attachments[0].is_default);
        assert!(environment.attachments[0].is_default_for_shell);
        assert!(!environment.attachments[1].is_default);
        assert!(!environment.attachments[1].is_default_for_shell);
        assert_eq!(
            environment.provider.read_text("README.md").await.unwrap(),
            "local workspace"
        );
        assert_eq!(
            environment
                .provider
                .read_text("/environment/tools/README.md")
                .await
                .unwrap(),
            "local workspace"
        );
        assert!(matches!(
            environment
                .provider
                .write_text("/environment/tools/new.txt", "blocked")
                .await,
            Err(starweaver_environment::EnvironmentError::AccessDenied(_))
        ));
        assert!(environment.process_provider.is_some());
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
