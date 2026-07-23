//! CLI environment provider resolution.

use std::{path::PathBuf, sync::Arc};

use serde::{Deserialize, Serialize};
use starweaver_envd::LocalEnvd;
use starweaver_envd_client::EnvdRpcClient;
use starweaver_envd_core::DEFAULT_ENVIRONMENT_ID;
use starweaver_environment::{
    CompositeEnvironmentProvider, DynEnvironmentProvider, DynProcessShellProvider,
    EnvdEnvironmentProvider, EnvironmentMount, EnvironmentMountMode, EnvironmentPolicy, FilePolicy,
    LocalEnvironmentProvider, ShellPolicy, VirtualEnvironmentProvider,
};

use crate::{CliConfig, CliError, CliResult};

pub(crate) const LOCAL_ENVIRONMENT_ATTACHMENT_ID: &str = "local";
pub(crate) const LOCAL_ENVIRONMENT_ATTACHMENT_KIND: &str = "local";

/// CLI-private access mode for one configured environment mount.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentAttachmentAccessMode {
    /// Permit reads but reject writes and process execution.
    ReadOnly,
    /// Permit reads, writes, and process execution allowed by the provider policy.
    #[default]
    ReadWrite,
}

/// CLI-private provider material used while building one run environment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EnvironmentAttachmentRef {
    pub id: String,
    #[serde(default = "default_environment_attachment_kind")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<EnvironmentAttachmentAccessMode>,
    #[serde(default, rename = "default")]
    pub is_default: bool,
    #[serde(default, rename = "defaultForShell")]
    pub is_default_for_shell: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
    #[serde(default, skip_serializing)]
    pub auth_token: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

impl EnvironmentAttachmentRef {
    pub(crate) fn resolved_mode(&self) -> EnvironmentAttachmentAccessMode {
        self.mode.unwrap_or_default()
    }

    pub(crate) fn requested_environment_id(&self) -> Option<&str> {
        self.environment_id.as_deref()
    }

    pub(crate) fn requested_endpoint_ref(&self) -> Option<&str> {
        self.endpoint_ref.as_deref()
    }

    pub(crate) fn requested_auth_token(&self) -> Option<&str> {
        self.auth_token.as_deref()
    }
}

pub(crate) fn is_valid_environment_attachment_id(id: &str) -> bool {
    !id.is_empty()
        && !matches!(id, "." | ".." | "environment")
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn default_environment_attachment_kind() -> String {
    LOCAL_ENVIRONMENT_ATTACHMENT_KIND.to_string()
}

/// Resolved environment provider for one CLI run.
#[derive(Clone)]
pub struct ResolvedEnvironment {
    /// Provider handle attached to `AgentSession`.
    pub provider: DynEnvironmentProvider,
    /// Optional process-capable provider override for background shell tools.
    pub process_provider: Option<DynProcessShellProvider>,
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
    resolve_environment_target_for_session_with_attachments(config, session_id, attachments)
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
                .with_context_file_tree_roots([config.workspace_root.clone()])
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
    let client = envd_client_for_attachment(attachment)
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
        attachments: vec![attachment.clone()],
    })
}

pub fn envd_client_for_attachment(
    attachment: &EnvironmentAttachmentRef,
) -> Result<EnvdRpcClient, String> {
    let endpoint = attachment
        .requested_endpoint_ref()
        .ok_or_else(|| "envd attachment requires endpointRef".to_string())?;
    EnvdRpcClient::from_local_endpoint_ref(endpoint, attachment.requested_auth_token())
        .map_err(|error| error.to_string())
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
    // Include the system temp dir so CLI/TUI agents can access user-specified
    // temporary files, while LocalEnvironmentProvider still appends its
    // provider-managed session temp dir separately.
    push_allowed_path(&mut paths, std::env::temp_dir());
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

#[cfg(test)]
fn display_local_test_path(path: &std::path::Path) -> String {
    #[cfg(windows)]
    {
        let path = path.to_string_lossy();
        let normalized = path.replace('/', "\\");
        let stripped = if let Some(stripped) = normalized.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{stripped}")
        } else if let Some(stripped) = normalized.strip_prefix(r"\\?\") {
            stripped.to_string()
        } else {
            normalized
        };
        stripped.replace('\\', "/")
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy().replace('\\', "/")
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
    use crate::{ConfigResolver, args, profiles::list_skills};

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
        assert!(
            config
                .skill_dirs
                .iter()
                .any(|path| path.ends_with("shared-agents/skills"))
        );

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
    async fn cli_local_environment_tmp_outputs_and_system_tmp_are_readable() {
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

        let system_tmp_file = std::env::temp_dir().join(format!(
            "starweaver-cli-system-tmp-read-{}",
            std::process::id()
        ));
        let system_tmp_output = std::env::temp_dir().join(format!(
            "starweaver-cli-system-tmp-write-{}",
            std::process::id()
        ));
        std::fs::write(&system_tmp_file, "user temp").unwrap();
        assert_eq!(
            environment
                .provider
                .read_text(&system_tmp_file.display().to_string())
                .await
                .unwrap(),
            "user temp"
        );
        environment
            .provider
            .write_text(&system_tmp_output.display().to_string(), "agent temp")
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&system_tmp_output).unwrap(),
            "agent temp"
        );
        let _ = std::fs::remove_file(system_tmp_file);
        let _ = std::fs::remove_file(system_tmp_output);
    }

    #[tokio::test]
    async fn cli_local_environment_allows_system_tmp_before_config_paths() {
        let temp = tempfile::tempdir().unwrap();
        let cli = args::parse(["starweaver-cli".to_string()]).unwrap();
        let config = ConfigResolver::for_tests(temp.path())
            .resolve(&cli)
            .unwrap();

        let allowed_paths = local_allowed_paths(&config);
        let system_tmp_dir = std::env::temp_dir()
            .canonicalize()
            .unwrap_or_else(|_| std::env::temp_dir());

        assert_eq!(allowed_paths.first(), Some(&system_tmp_dir));
        assert!(
            allowed_paths.contains(
                &config
                    .global_dir
                    .canonicalize()
                    .unwrap_or_else(|_| config.global_dir.clone())
            )
        );
        assert!(
            allowed_paths.contains(
                &config
                    .workspace_root
                    .canonicalize()
                    .unwrap_or_else(|_| config.workspace_root.clone())
            )
        );
        assert!(
            allowed_paths.contains(
                &config
                    .project_dir
                    .canonicalize()
                    .unwrap_or_else(|_| config.project_dir.clone())
            )
        );
    }

    #[tokio::test]
    async fn cli_local_environment_file_tree_context_uses_workspace_only() {
        let temp = tempfile::tempdir().unwrap();
        let cli = args::parse(["starweaver-cli".to_string()]).unwrap();
        let config = ConfigResolver::for_tests(temp.path())
            .resolve(&cli)
            .unwrap();
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        std::fs::create_dir_all(&config.global_dir).unwrap();
        std::fs::write(config.workspace_root.join("app.rs"), "fn main() {}").unwrap();
        std::fs::write(config.global_dir.join("config-marker.txt"), "config").unwrap();

        let environment = resolve_environment_for_session(&config, "session_123").unwrap();
        let context = environment
            .provider
            .render_environment_context()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(context.matches("<directory path=").count(), 1);
        assert!(context.contains(&format!(
            "<directory path=\"{}\">",
            display_local_test_path(
                &config
                    .workspace_root
                    .canonicalize()
                    .unwrap_or_else(|_| config.workspace_root.clone())
            )
        )));
        assert!(context.contains("app.rs"));
        assert!(!context.contains("config-marker.txt"));
        assert!(!context.contains(&format!(
            "<directory path=\"{}\">",
            display_local_test_path(
                &config
                    .global_dir
                    .canonicalize()
                    .unwrap_or_else(|_| config.global_dir.clone())
            )
        )));

        assert_eq!(
            environment
                .provider
                .read_text(
                    &config
                        .global_dir
                        .join("config-marker.txt")
                        .display()
                        .to_string(),
                )
                .await
                .unwrap(),
            "config"
        );
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
            endpoint_ref: None,
            environment_id: None,
            auth_token: None,
            metadata: serde_json::Map::new(),
        }];

        let environment =
            resolve_environment_for_session_with_attachments(&config, "session_123", &attachments)
                .unwrap();

        assert_eq!(environment.provider.id(), "composite");
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
                endpoint_ref: None,
                environment_id: None,
                auth_token: None,
                metadata: serde_json::Map::new(),
            },
        ];

        let environment =
            resolve_environment_for_session_with_attachments(&config, "session_123", &attachments)
                .unwrap();

        assert_eq!(environment.provider.id(), "composite");
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
