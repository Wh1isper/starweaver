//! CLI configuration resolution.

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use starweaver_model::MaxTokensParameter;
use starweaver_rpc_core::{
    is_valid_environment_attachment_id, EnvironmentAttachmentAccessMode,
    LOCAL_ENVIRONMENT_ATTACHMENT_ID,
};
use toml::Value;

use crate::{
    args::{Cli, CliCommand, ConfigCommand, HitlPolicy, OutputMode, SetupCommand},
    error::io_error,
    oauth::CODEX_BASE_URL,
    slash_commands::{normalize_command_name, valid_command_name, SlashCommandDefinition},
    CliError, CliResult,
};

mod env_overrides;
mod metadata;
mod state;
mod templates;
mod values;

use env_overrides::{apply_cli_overrides, apply_env, parse_hitl_policy, parse_output_mode};
pub use metadata::{mcp_servers, tool_need_approval};
use metadata::{merge_json_value, read_mcp_config, read_tools_config};
pub use state::{
    clear_current_session, ensure_config_dirs, read_current_session, write_current_session,
};
use templates::default_config_template;
pub use templates::{
    init_config_file, write_default_subagent_presets, DEFAULT_GLOBAL_GITIGNORE_TEMPLATE,
    DEFAULT_MCP_TEMPLATE, DEFAULT_PROJECT_GITIGNORE_TEMPLATE, DEFAULT_TOOLS_TEMPLATE,
};
pub use values::{get_config_value, set_config_value, ConfigScope};

/// Resolved CLI configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CliConfig {
    /// Global config root.
    pub global_dir: PathBuf,
    /// Project config root.
    pub project_dir: PathBuf,
    /// TUI client state root.
    pub tui_state_dir: PathBuf,
    /// Desktop client state root.
    pub desktop_state_dir: PathBuf,
    /// `SQLite` database path.
    pub database_path: PathBuf,
    /// Local file store path.
    pub file_store_path: PathBuf,
    /// Default profile.
    pub default_profile: String,
    /// Skill directory search paths.
    pub skill_dirs: Vec<PathBuf>,
    /// Subagent directory search paths.
    pub subagent_dirs: Vec<PathBuf>,
    /// Disabled subagent names from layered subagent config.
    pub disabled_subagents: Vec<String>,
    /// Workspace root for environment providers.
    pub workspace_root: PathBuf,
    /// Environment provider kind.
    pub environment_provider: String,
    /// Filesystem policy mode.
    pub files_policy: String,
    /// Whether shell execution is enabled for environment tools.
    pub shell_enabled: bool,
    /// Shell command review configuration.
    pub shell_review: CliShellReviewConfig,
    /// Default output mode.
    pub default_output: OutputMode,
    /// Default headless human-in-the-loop policy.
    pub default_hitl: HitlPolicy,
    /// Default maximum runtime goal retry iterations.
    pub max_goal_iterations: usize,
    /// Update channel metadata.
    pub update_channel: String,
    /// OAuth token refresh supervisor configuration.
    pub oauth_refresh: OAuthRefreshConfig,
    /// Default model from `[general] model` fields.
    pub default_model: Option<CliModelProfile>,
    /// Named model profiles from `[model_profiles.*]` fields.
    pub model_profiles: BTreeMap<String, CliModelProfile>,
    /// Named envd profiles from `[envd_profiles.*]` fields.
    pub envd_profiles: BTreeMap<String, CliEnvdProfile>,
    /// Environment variables loaded from config `[env]` sections.
    pub env_vars: BTreeMap<String, String>,
    /// Provider API configuration.
    pub providers: ProviderConfigs,
    /// Tool config metadata loaded from tools.toml.
    pub tools_config: serde_json::Value,
    /// MCP config metadata loaded from mcp.json.
    pub mcp_config: serde_json::Value,
    /// Unmapped config metadata preserved for configuration audits.
    pub unmapped_metadata: serde_json::Value,
    /// Custom slash commands loaded from `[commands.*]` config sections.
    pub slash_commands: BTreeMap<String, SlashCommandDefinition>,
    /// Automatic trim after a run.
    pub auto_trim: bool,
    /// Recent runs to keep for automatic trim.
    pub current_session_keep_recent_runs: usize,
    /// Retention horizon for future all-session maintenance.
    pub all_sessions_keep_days: u64,
}

/// Config resolver.
#[allow(clippy::struct_field_names)]
#[derive(Clone, Debug)]
pub struct ConfigResolver {
    global_dir: Option<PathBuf>,
    project_dir: Option<PathBuf>,
    shared_agents_dir: Option<PathBuf>,
    current_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileConfig {
    general: Option<GeneralConfig>,
    storage: Option<StorageConfig>,
    environment: Option<EnvironmentConfig>,
    security: Option<FileSecurityConfig>,
    update: Option<UpdateConfig>,
    providers: Option<FileProviderConfigs>,
    oauth_refresh: Option<FileOAuthRefreshConfig>,
    model_profiles: Option<BTreeMap<String, FileModelProfile>>,
    envd_profiles: Option<BTreeMap<String, FileEnvdProfile>>,
    env: Option<BTreeMap<String, String>>,
    skills: Option<SkillsConfig>,
    subagents: Option<SubagentsConfig>,
    commands: Option<BTreeMap<String, FileCommandDefinition>>,
    trim: Option<TrimConfig>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct GeneralConfig {
    default_profile: Option<String>,
    default_output: Option<OutputMode>,
    default_hitl: Option<HitlPolicy>,
    max_goal_iterations: Option<usize>,
    model: Option<String>,
    model_settings: Option<String>,
    model_cfg: Option<String>,
    max_requests: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileOAuthRefreshConfig {
    enabled: Option<bool>,
    interval_seconds: Option<u64>,
    failure_retry_seconds: Option<u64>,
    refresh_on_startup: Option<bool>,
}

/// OAuth token refresh supervisor configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OAuthRefreshConfig {
    /// Whether OAuth refresh supervisor should be started for OAuth-backed models.
    pub enabled: bool,
    /// Successful-refresh interval in seconds.
    pub interval_seconds: u64,
    /// Retry interval in seconds after the last refresh attempt failed.
    pub failure_retry_seconds: u64,
    /// Refresh immediately when the supervisor starts.
    pub refresh_on_startup: bool,
}

impl Default for OAuthRefreshConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_seconds: 30 * 60,
            failure_retry_seconds: 60,
            refresh_on_startup: true,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileModelProfile {
    label: Option<String>,
    model: Option<String>,
    model_settings: Option<String>,
    model_cfg: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileEnvdProfile {
    label: Option<String>,
    enabled: Option<bool>,
    #[serde(alias = "endpoint_ref", alias = "endpointRef")]
    endpoint: Option<String>,
    auth_token: Option<String>,
    auth_token_env: Option<String>,
    environment_id: Option<String>,
    mount_id: Option<String>,
    mode: Option<String>,
    #[serde(rename = "default")]
    is_default: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileCommandDefinition {
    prompt: Option<String>,
    description: Option<String>,
    aliases: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SkillsConfig {
    dirs: Option<Vec<String>>,
    additional_dirs: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SubagentsConfig {
    dirs: Option<Vec<String>>,
    additional_dirs: Option<Vec<String>>,
    disabled: Option<Vec<String>>,
    disabled_builtins: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct StorageConfig {
    database_path: Option<String>,
    file_store_path: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct EnvironmentConfig {
    workspace_root: Option<String>,
    provider: Option<String>,
    files_policy: Option<String>,
    shell_enabled: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileSecurityConfig {
    shell_review: Option<FileShellReviewConfig>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileShellReviewConfig {
    enabled: Option<bool>,
    model: Option<String>,
    model_settings: Option<String>,
    on_needs_approval: Option<String>,
    risk_threshold: Option<String>,
    system_prompt: Option<String>,
}

/// CLI shell command review configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliShellReviewConfig {
    /// Whether shell review is enabled.
    pub enabled: bool,
    /// Review model id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Review model settings preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_settings: Option<String>,
    /// Action when review reaches threshold: defer or deny.
    pub on_needs_approval: String,
    /// Risk threshold: low, medium, high, or `extra_high`.
    pub risk_threshold: String,
    /// Optional prompt override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

impl Default for CliShellReviewConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: None,
            model_settings: None,
            on_needs_approval: "defer".to_string(),
            risk_threshold: "high".to_string(),
            system_prompt: None,
        }
    }
}

impl CliShellReviewConfig {
    fn validate(&self) -> CliResult<()> {
        if self.enabled
            && self
                .model
                .as_deref()
                .is_none_or(|model| model.trim().is_empty())
        {
            return Err(CliError::Config(
                "security.shell_review.model is required when shell review is enabled".to_string(),
            ));
        }
        validate_shell_review_action(&self.on_needs_approval)?;
        validate_shell_review_risk(&self.risk_threshold)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct UpdateConfig {
    channel: Option<String>,
}

/// CLI model profile resolved from config.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliModelProfile {
    /// Human label for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Provider model id, such as `openai-responses:gpt-5` or `homelab@openai-responses:gpt-5`.
    pub model_id: String,
    /// Model config preset name from `model_cfg`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_cfg: Option<String>,
    /// Model settings preset name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_settings: Option<String>,
}

/// Envd profile resolved from config.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliEnvdProfile {
    /// Human label for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Whether this profile is active for TUI runs.
    pub enabled: bool,
    /// HTTP endpoint for the envd JSON-RPC transport.
    pub endpoint: String,
    /// Bearer token stored directly in config. This is intentionally
    /// request-only and must not be returned by config display surfaces.
    #[serde(default, skip_serializing)]
    pub auth_token: Option<String>,
    /// Environment variable that contains the bearer token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token_env: Option<String>,
    /// Concrete envd environment id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
    /// Agent-facing mount id.
    pub mount_id: String,
    /// Access mode for the mount.
    pub mode: EnvironmentAttachmentAccessMode,
    /// Whether this profile should be the default TUI environment.
    #[serde(rename = "default")]
    pub is_default: bool,
}

/// Provider API configuration.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ProviderConfigs {
    /// `OpenAI` provider config.
    pub openai: ProviderConfig,
    /// Anthropic provider config.
    pub anthropic: ProviderConfig,
    /// Gemini provider config.
    pub gemini: ProviderConfig,
    /// Google Cloud Gemini provider config.
    pub google_cloud: ProviderConfig,
    /// Codex OAuth provider config.
    pub codex: ProviderConfig,
    /// Named gateway provider configs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub gateways: BTreeMap<String, ProviderConfig>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileProviderConfigs {
    openai: Option<FileProviderConfig>,
    anthropic: Option<FileProviderConfig>,
    gemini: Option<FileProviderConfig>,
    google: Option<FileProviderConfig>,
    #[serde(rename = "google-gla")]
    google_gla: Option<FileProviderConfig>,
    #[serde(rename = "google-cloud", alias = "google_cloud")]
    google_cloud: Option<FileProviderConfig>,
    #[serde(rename = "google-vertex")]
    google_vertex: Option<FileProviderConfig>,
    codex: Option<FileProviderConfig>,
    #[serde(flatten)]
    gateways: BTreeMap<String, FileProviderConfig>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileProviderConfig {
    enabled: Option<bool>,
    api_key_env: Option<String>,
    auth_token_env: Option<String>,
    project: Option<String>,
    location: Option<String>,
    base_url: Option<String>,
    endpoint_path: Option<String>,
    max_tokens_parameter: Option<MaxTokensParameter>,
}

/// Single provider API configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProviderConfig {
    /// Enable provider-backed profile resolution for this provider.
    pub enabled: bool,
    /// Environment variable containing the provider API key.
    pub api_key_env: Option<String>,
    /// Environment variable containing a bearer access token.
    pub auth_token_env: Option<String>,
    /// Provider project identifier when required by the backend.
    pub project: Option<String>,
    /// Provider location or region when required by the backend.
    pub location: Option<String>,
    /// Provider or gateway base URL.
    pub base_url: Option<String>,
    /// Override endpoint path.
    pub endpoint_path: Option<String>,
    /// Provider or gateway max-token parameter mapping.
    pub max_tokens_parameter: MaxTokensParameter,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            api_key_env: None,
            auth_token_env: None,
            project: None,
            location: None,
            base_url: None,
            endpoint_path: None,
            max_tokens_parameter: MaxTokensParameter::Default,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct TrimConfig {
    auto_after_run: Option<bool>,
    current_session_keep_recent_runs: Option<usize>,
    all_sessions_keep_days: Option<u64>,
}

impl Default for ConfigResolver {
    fn default() -> Self {
        Self {
            global_dir: env::var_os("STARWEAVER_CONFIG_DIR").map(PathBuf::from),
            project_dir: env::var_os("STARWEAVER_PROJECT_DIR").map(PathBuf::from),
            shared_agents_dir: None,
            current_dir: None,
        }
    }
}

impl ConfigResolver {
    /// Build a resolver pinned to a root for deterministic tests.
    #[must_use]
    pub fn for_tests(root: &std::path::Path) -> Self {
        Self {
            global_dir: Some(root.join("global")),
            project_dir: Some(root.join("project/.starweaver")),
            shared_agents_dir: Some(root.join("shared-agents")),
            current_dir: Some(root.join("project")),
        }
    }

    /// Resolve final config.
    pub fn resolve(&self, cli: &Cli) -> CliResult<CliConfig> {
        let current_dir = self.current_dir.clone().unwrap_or_else(default_current_dir);
        let global_dir = self.global_dir.clone().unwrap_or_else(default_global_dir);
        let project_dir = self
            .project_dir
            .clone()
            .unwrap_or_else(|| default_project_dir(cli, &global_dir, &current_dir));
        let shared_agents_dir = self
            .shared_agents_dir
            .clone()
            .unwrap_or_else(default_shared_agents_dir);
        let mut config = CliConfig {
            global_dir: global_dir.clone(),
            project_dir: project_dir.clone(),
            tui_state_dir: global_dir.join("tui"),
            desktop_state_dir: global_dir.join("desktop"),
            database_path: project_dir.join("starweaver.sqlite"),
            file_store_path: project_dir.join("store"),
            default_profile: "general".to_string(),
            skill_dirs: default_skill_dirs(&global_dir, &shared_agents_dir, &project_dir),
            subagent_dirs: vec![global_dir.join("subagents"), project_dir.join("subagents")],
            disabled_subagents: Vec::new(),
            workspace_root: default_workspace_root(&project_dir, &global_dir, &current_dir),
            environment_provider: "local".to_string(),
            files_policy: "read_write".to_string(),
            shell_enabled: true,
            shell_review: CliShellReviewConfig::default(),
            default_output: OutputMode::AguiJsonl,
            default_hitl: HitlPolicy::Defer,
            max_goal_iterations: 10,
            update_channel: "stable".to_string(),
            oauth_refresh: OAuthRefreshConfig::default(),
            default_model: None,
            model_profiles: BTreeMap::new(),
            envd_profiles: BTreeMap::new(),
            env_vars: BTreeMap::new(),
            providers: default_provider_configs(),
            tools_config: serde_json::Value::Null,
            mcp_config: serde_json::Value::Null,
            unmapped_metadata: serde_json::json!({}),
            slash_commands: BTreeMap::new(),
            auto_trim: true,
            current_session_keep_recent_runs: 20,
            all_sessions_keep_days: 60,
        };
        bootstrap_global_config_dir(&global_dir)?;
        apply_file_config(&mut config, &global_dir.join("config.toml"))?;
        apply_file_config(&mut config, &project_dir.join("config.toml"))?;
        config.tools_config = read_tools_config(&global_dir, &project_dir)?;
        config.mcp_config = read_mcp_config(&global_dir, &project_dir)?;
        apply_env(&mut config);
        apply_cli_overrides(&mut config, cli, &project_dir);
        config.shell_review.validate()?;
        Ok(config)
    }
}

fn default_global_dir() -> PathBuf {
    env::var_os("HOME")
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join(".starweaver")
}

fn default_shared_agents_dir() -> PathBuf {
    env::var_os("HOME")
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join(".agents")
}

fn default_current_dir() -> PathBuf {
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn default_skill_dirs(
    global_dir: &std::path::Path,
    shared_agents_dir: &std::path::Path,
    project_dir: &std::path::Path,
) -> Vec<PathBuf> {
    vec![
        global_dir.join("skills"),
        shared_agents_dir.join("skills"),
        project_dir.join("skills"),
    ]
}

fn default_project_dir(cli: &Cli, global_dir: &Path, current_dir: &Path) -> PathBuf {
    if wants_project_config(cli) {
        return current_dir.join(".starweaver");
    }
    find_project_dir(current_dir, global_dir).unwrap_or_else(|| global_dir.to_path_buf())
}

fn default_workspace_root(project_dir: &Path, global_dir: &Path, current_dir: &Path) -> PathBuf {
    if paths_equivalent(project_dir, global_dir) {
        return current_dir.to_path_buf();
    }
    project_dir
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| current_dir.to_path_buf(), std::path::Path::to_path_buf)
}

const fn wants_project_config(cli: &Cli) -> bool {
    matches!(
        &cli.command,
        Some(
            CliCommand::Setup(SetupCommand {
                global: false,
                project: true,
                ..
            }) | CliCommand::Config {
                command: ConfigCommand::Init {
                    global: false,
                    project: true,
                    ..
                } | ConfigCommand::Set {
                    global: false,
                    project: true,
                    ..
                }
            }
        )
    )
}

fn find_project_dir(start: &Path, global_dir: &Path) -> Option<PathBuf> {
    let home_project_dir = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".starweaver"));
    let mut current = start.to_path_buf();
    loop {
        let candidate = current.join(".starweaver");
        let is_global_dir = paths_equivalent(&candidate, global_dir);
        let is_home_global_dir = home_project_dir
            .as_ref()
            .is_some_and(|home| paths_equivalent(&candidate, home));
        if candidate.join("config.toml").exists() && !is_global_dir && !is_home_global_dir {
            return Some(candidate);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    let normalized_left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let normalized_right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    normalized_left == normalized_right
}

fn bootstrap_global_config_dir(global_dir: &Path) -> CliResult<()> {
    fs::create_dir_all(global_dir).map_err(|error| io_error(global_dir, error))?;
    let config_path = global_dir.join("config.toml");
    if !config_path.exists() {
        fs::write(&config_path, default_config_template(ConfigScope::Global))
            .map_err(|error| io_error(&config_path, error))?;
    }
    for (path, content) in [
        (global_dir.join("tools.toml"), DEFAULT_TOOLS_TEMPLATE),
        (global_dir.join("mcp.json"), DEFAULT_MCP_TEMPLATE),
        (
            global_dir.join(".gitignore"),
            DEFAULT_GLOBAL_GITIGNORE_TEMPLATE,
        ),
    ] {
        if !path.exists() {
            fs::write(&path, content).map_err(|error| io_error(&path, error))?;
        }
    }
    for name in ["skills", "subagents", "tui", "desktop"] {
        let path = global_dir.join(name);
        fs::create_dir_all(&path).map_err(|error| io_error(path, error))?;
    }
    write_default_subagent_presets(global_dir, false)?;
    Ok(())
}

fn default_provider_configs() -> ProviderConfigs {
    ProviderConfigs {
        openai: ProviderConfig {
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            base_url: Some("https://api.openai.com/v1".to_string()),
            ..ProviderConfig::default()
        },
        anthropic: ProviderConfig {
            api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
            base_url: Some("https://api.anthropic.com/v1".to_string()),
            ..ProviderConfig::default()
        },
        gemini: ProviderConfig {
            api_key_env: Some("GEMINI_API_KEY".to_string()),
            base_url: Some("https://generativelanguage.googleapis.com/v1beta".to_string()),
            ..ProviderConfig::default()
        },
        google_cloud: ProviderConfig {
            api_key_env: Some("GOOGLE_API_KEY".to_string()),
            auth_token_env: Some("GOOGLE_CLOUD_ACCESS_TOKEN".to_string()),
            base_url: Some("https://aiplatform.googleapis.com".to_string()),
            ..ProviderConfig::default()
        },
        codex: ProviderConfig {
            base_url: Some(CODEX_BASE_URL.to_string()),
            max_tokens_parameter: MaxTokensParameter::Omit,
            ..ProviderConfig::default()
        },
        gateways: BTreeMap::new(),
    }
}

#[allow(clippy::too_many_lines)]
fn apply_file_config(config: &mut CliConfig, path: &PathBuf) -> CliResult<()> {
    if !path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(path).map_err(|error| io_error(path, error))?;
    let raw = content
        .parse::<Value>()
        .map_err(|error| CliError::Config(error.to_string()))?;
    merge_unmapped_metadata(config, &raw);
    let parsed = toml::from_str::<FileConfig>(&content)?;
    let base = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    if let Some(general) = parsed.general {
        let has_default_profile = general.default_profile.is_some();
        let model = general.model.clone();
        let model_settings = general.model_settings.clone();
        let model_cfg = general.model_cfg.clone();
        let max_requests = general.max_requests;
        if let Some(max_requests) = max_requests {
            merge_json_value(
                &mut config.unmapped_metadata,
                serde_json::json!({"general": {"max_requests": max_requests}}),
            );
        }
        if let Some(profile) = general.default_profile {
            config.default_profile = profile;
        }
        if let Some(output) = general.default_output {
            config.default_output = output;
        }
        if let Some(hitl) = general.default_hitl {
            config.default_hitl = hitl;
        }
        if let Some(max_goal_iterations) = general.max_goal_iterations {
            config.max_goal_iterations = max_goal_iterations.max(1);
        }
        if let Some(model_id) = model {
            config.default_model = Some(CliModelProfile {
                label: Some("Default".to_string()),
                model_id,
                model_cfg,
                model_settings,
            });
            if !has_default_profile {
                config.default_profile = "default_model".to_string();
            }
        }
    }
    if let Some(storage) = parsed.storage {
        if let Some(database_path) = storage.database_path {
            config.database_path = expand_path(&database_path, base);
        }
        if let Some(file_store_path) = storage.file_store_path {
            config.file_store_path = expand_path(&file_store_path, base);
        }
    }
    if let Some(environment) = parsed.environment {
        if let Some(workspace_root) = environment.workspace_root {
            config.workspace_root = expand_path(&workspace_root, base);
        }
        if let Some(provider) = environment.provider {
            config.environment_provider = provider;
        }
        if let Some(files_policy) = environment.files_policy {
            config.files_policy = files_policy;
        }
        if let Some(shell_enabled) = environment.shell_enabled {
            config.shell_enabled = shell_enabled;
        }
    }
    if let Some(security) = parsed.security {
        if let Some(shell_review) = security.shell_review {
            merge_shell_review_config(&mut config.shell_review, shell_review)?;
        }
    }
    if let Some(update) = parsed.update {
        if let Some(channel) = update.channel {
            config.update_channel = channel;
        }
    }
    if let Some(providers) = parsed.providers {
        merge_provider_configs(&mut config.providers, providers);
    }
    if let Some(oauth_refresh) = parsed.oauth_refresh {
        merge_oauth_refresh_config(&mut config.oauth_refresh, &oauth_refresh)?;
    }
    if let Some(env_vars) = parsed.env {
        config.env_vars.extend(env_vars);
    }
    if let Some(model_profiles) = parsed.model_profiles {
        for (name, profile) in model_profiles {
            if let Some(model_id) = profile.model {
                config.model_profiles.insert(
                    name,
                    CliModelProfile {
                        label: profile.label,
                        model_id,
                        model_cfg: profile.model_cfg,
                        model_settings: profile.model_settings,
                    },
                );
            }
        }
    }
    if let Some(envd_profiles) = parsed.envd_profiles {
        for (name, profile) in envd_profiles {
            let profile = resolve_envd_profile(&name, profile)?;
            config.envd_profiles.insert(name, profile);
        }
    }
    if let Some(skills) = parsed.skills {
        merge_skill_dirs(config, skills, base);
    }
    if let Some(subagents) = parsed.subagents {
        merge_subagent_config(config, subagents, base);
    }
    if let Some(commands) = parsed.commands {
        merge_slash_commands(config, commands);
    }
    if let Some(trim) = parsed.trim {
        if let Some(auto_after_run) = trim.auto_after_run {
            config.auto_trim = auto_after_run;
        }
        if let Some(keep) = trim.current_session_keep_recent_runs {
            config.current_session_keep_recent_runs = keep;
        }
        if let Some(days) = trim.all_sessions_keep_days {
            config.all_sessions_keep_days = days;
        }
    }
    Ok(())
}

fn merge_skill_dirs(config: &mut CliConfig, skills: SkillsConfig, base: &std::path::Path) {
    if let Some(dirs) = skills.dirs {
        config.skill_dirs = dirs.iter().map(|path| expand_path(path, base)).collect();
    }
    if let Some(additional_dirs) = skills.additional_dirs {
        config
            .skill_dirs
            .extend(additional_dirs.iter().map(|path| expand_path(path, base)));
    }
}

fn merge_subagent_config(
    config: &mut CliConfig,
    subagents: SubagentsConfig,
    base: &std::path::Path,
) {
    if let Some(dirs) = subagents.dirs {
        config.subagent_dirs = dirs.iter().map(|path| expand_path(path, base)).collect();
    }
    if let Some(additional_dirs) = subagents.additional_dirs {
        config
            .subagent_dirs
            .extend(additional_dirs.iter().map(|path| expand_path(path, base)));
    }
    if let Some(disabled) = subagents.disabled {
        config.disabled_subagents.extend(disabled);
    }
    if let Some(disabled_builtins) = subagents.disabled_builtins {
        config.disabled_subagents.extend(disabled_builtins);
    }
    config.disabled_subagents.sort();
    config.disabled_subagents.dedup();
}

fn merge_slash_commands(config: &mut CliConfig, commands: BTreeMap<String, FileCommandDefinition>) {
    for (name, command) in commands {
        let normalized = normalize_command_name(&name);
        if !valid_command_name(&normalized) || reserved_slash_command(&normalized) {
            continue;
        }
        let Some(prompt) = command.prompt.filter(|prompt| !prompt.trim().is_empty()) else {
            continue;
        };
        let aliases = command
            .aliases
            .unwrap_or_default()
            .into_iter()
            .map(|alias| normalize_command_name(&alias))
            .filter(|alias| valid_command_name(alias) && !reserved_slash_command(alias))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|alias| alias != &normalized)
            .collect::<Vec<_>>();
        let definition = SlashCommandDefinition {
            name: normalized,
            prompt,
            description: command.description,
            aliases,
        };
        upsert_slash_command(config, definition);
    }
}

fn resolve_envd_profile(name: &str, profile: FileEnvdProfile) -> CliResult<CliEnvdProfile> {
    let endpoint = required_trimmed_envd_field(name, "endpoint", profile.endpoint)?;
    let mount_id = profile.mount_id.unwrap_or_else(|| name.to_string());
    let mount_id = mount_id.trim().to_string();
    if !is_valid_environment_attachment_id(&mount_id) {
        return Err(CliError::Config(format!(
            "envd profile {name} has invalid mount_id: {mount_id}; expected an ASCII slug"
        )));
    }
    if mount_id == LOCAL_ENVIRONMENT_ATTACHMENT_ID {
        return Err(CliError::Config(format!(
            "envd profile {name} cannot use reserved mount_id: {mount_id}"
        )));
    }
    let auth_token = profile
        .auth_token
        .as_deref()
        .map(|token| validate_envd_token_field(name, "auth_token", token))
        .transpose()?;
    let auth_token_env = profile
        .auth_token_env
        .as_deref()
        .map(|env| validate_envd_token_env_field(name, env))
        .transpose()?;
    if auth_token.is_none() && auth_token_env.is_none() {
        return Err(CliError::Config(format!(
            "envd profile {name} requires auth_token or auth_token_env"
        )));
    }
    let mode = match profile.mode.as_deref().unwrap_or("read_write").trim() {
        "read_only" | "read-only" => EnvironmentAttachmentAccessMode::ReadOnly,
        "read_write" | "read-write" => EnvironmentAttachmentAccessMode::ReadWrite,
        other => {
            return Err(CliError::Config(format!(
                "envd profile {name} has invalid mode: {other}; expected read_only or read_write"
            )));
        }
    };
    Ok(CliEnvdProfile {
        label: profile.label,
        enabled: profile.enabled.unwrap_or(true),
        endpoint,
        auth_token,
        auth_token_env,
        environment_id: profile
            .environment_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        mount_id,
        mode,
        is_default: profile.is_default.unwrap_or(false),
    })
}

fn required_trimmed_envd_field(
    profile_name: &str,
    field: &str,
    value: Option<String>,
) -> CliResult<String> {
    let Some(value) = value else {
        return Err(CliError::Config(format!(
            "envd profile {profile_name} requires {field}"
        )));
    };
    let value = value.trim().to_string();
    if value.is_empty() {
        return Err(CliError::Config(format!(
            "envd profile {profile_name} {field} cannot be empty"
        )));
    }
    Ok(value)
}

fn validate_envd_token_field(profile_name: &str, field: &str, token: &str) -> CliResult<String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(CliError::Config(format!(
            "envd profile {profile_name} {field} cannot be empty"
        )));
    }
    if token.bytes().any(|byte| matches!(byte, b'\r' | b'\n')) {
        return Err(CliError::Config(format!(
            "envd profile {profile_name} {field} cannot contain newlines"
        )));
    }
    Ok(token)
}

fn validate_envd_token_env_field(profile_name: &str, env: &str) -> CliResult<String> {
    let env = env.trim().to_string();
    if env.is_empty() {
        return Err(CliError::Config(format!(
            "envd profile {profile_name} auth_token_env cannot be empty"
        )));
    }
    Ok(env)
}

fn upsert_slash_command(config: &mut CliConfig, mut definition: SlashCommandDefinition) {
    let canonical = definition.name.clone();
    let stale_aliases = config
        .slash_commands
        .iter()
        .filter(|(lookup, existing)| existing.name == canonical && *lookup != &canonical)
        .map(|(lookup, _)| lookup.clone())
        .collect::<Vec<_>>();
    for alias in stale_aliases {
        config.slash_commands.remove(&alias);
    }
    for existing in config.slash_commands.values_mut() {
        if existing.name != canonical {
            existing.aliases.retain(|alias| alias != &canonical);
        }
    }

    let requested_aliases = std::mem::take(&mut definition.aliases);
    let active_aliases = requested_aliases
        .into_iter()
        .filter(|alias| {
            config
                .slash_commands
                .get(alias)
                .is_none_or(|existing| existing.name == canonical)
        })
        .collect::<Vec<_>>();
    definition.aliases.clone_from(&active_aliases);
    config.slash_commands.insert(canonical, definition.clone());
    for alias in active_aliases {
        config.slash_commands.insert(alias, definition.clone());
    }
}

fn reserved_slash_command(name: &str) -> bool {
    matches!(
        name,
        "help"
            | "config"
            | "mode"
            | "act"
            | "plan"
            | "loop"
            | "tasks"
            | "session"
            | "dump"
            | "load"
            | "clear"
            | "cost"
            | "exit"
            | "model"
            | "paste-image"
            | "goal"
    )
}

fn merge_unmapped_metadata(config: &mut CliConfig, raw: &Value) {
    let Some(root) = raw.as_table() else {
        return;
    };
    let mut metadata = serde_json::Map::new();
    for key in ["display", "subagents", "security"] {
        if let Some(value) = root.get(key).cloned() {
            if let Ok(json) = serde_json::to_value(value) {
                metadata.insert(key.to_string(), json);
            }
        }
    }
    if !metadata.is_empty() {
        merge_json_value(
            &mut config.unmapped_metadata,
            serde_json::Value::Object(metadata),
        );
    }
}

fn merge_provider_configs(target: &mut ProviderConfigs, overlay: FileProviderConfigs) {
    if let Some(openai) = overlay.openai {
        merge_provider_config(&mut target.openai, openai);
    }
    if let Some(anthropic) = overlay.anthropic {
        merge_provider_config(&mut target.anthropic, anthropic);
    }
    if let Some(gemini) = overlay.gemini {
        merge_provider_config(&mut target.gemini, gemini);
    }
    if let Some(google) = overlay.google {
        merge_provider_config(&mut target.gemini, google);
    }
    if let Some(google_gla) = overlay.google_gla {
        merge_provider_config(&mut target.gemini, google_gla);
    }
    if let Some(google_cloud) = overlay.google_cloud {
        merge_provider_config(&mut target.google_cloud, google_cloud);
    }
    if let Some(google_vertex) = overlay.google_vertex {
        merge_provider_config(&mut target.google_cloud, google_vertex);
    }
    if let Some(codex) = overlay.codex {
        merge_provider_config(&mut target.codex, codex);
    }
    for (name, gateway) in overlay.gateways {
        merge_provider_config(target.gateways.entry(name).or_default(), gateway);
    }
}

fn merge_provider_config(target: &mut ProviderConfig, overlay: FileProviderConfig) {
    if let Some(enabled) = overlay.enabled {
        target.enabled = enabled;
    }
    if overlay.api_key_env.is_some() {
        target.api_key_env = overlay.api_key_env;
    }
    if overlay.auth_token_env.is_some() {
        target.auth_token_env = overlay.auth_token_env;
    }
    if overlay.project.is_some() {
        target.project = overlay.project;
    }
    if overlay.location.is_some() {
        target.location = overlay.location;
    }
    if overlay.base_url.is_some() {
        target.base_url = overlay.base_url;
    }
    if overlay.endpoint_path.is_some() {
        target.endpoint_path = overlay.endpoint_path;
    }
    if let Some(max_tokens_parameter) = overlay.max_tokens_parameter {
        target.max_tokens_parameter = max_tokens_parameter;
    }
}

fn merge_shell_review_config(
    target: &mut CliShellReviewConfig,
    overlay: FileShellReviewConfig,
) -> CliResult<()> {
    if let Some(enabled) = overlay.enabled {
        target.enabled = enabled;
    }
    if overlay.model.is_some() {
        target.model = overlay.model;
    }
    if overlay.model_settings.is_some() {
        target.model_settings = overlay.model_settings;
    }
    if let Some(action) = overlay.on_needs_approval {
        target.on_needs_approval = validate_shell_review_action(&action)?.to_string();
    }
    if let Some(threshold) = overlay.risk_threshold {
        target.risk_threshold = validate_shell_review_risk(&threshold)?.to_string();
    }
    if overlay.system_prompt.is_some() {
        target.system_prompt = overlay.system_prompt;
    }
    Ok(())
}

fn validate_shell_review_action(value: &str) -> CliResult<&'static str> {
    match value.trim() {
        "defer" => Ok("defer"),
        "deny" => Ok("deny"),
        other => Err(CliError::Usage(format!(
            "invalid security.shell_review.on_needs_approval: {other}; expected defer or deny"
        ))),
    }
}

fn validate_shell_review_risk(value: &str) -> CliResult<&'static str> {
    match value.trim() {
        "low" => Ok("low"),
        "medium" => Ok("medium"),
        "high" => Ok("high"),
        "extra_high" | "extra-high" => Ok("extra_high"),
        other => Err(CliError::Usage(format!(
            "invalid security.shell_review.risk_threshold: {other}; expected low, medium, high, or extra_high"
        ))),
    }
}

fn merge_oauth_refresh_config(
    target: &mut OAuthRefreshConfig,
    overlay: &FileOAuthRefreshConfig,
) -> CliResult<()> {
    if let Some(enabled) = overlay.enabled {
        target.enabled = enabled;
    }
    if let Some(interval_seconds) = overlay.interval_seconds {
        if interval_seconds == 0 {
            return Err(CliError::Usage(
                "invalid oauth_refresh.interval_seconds: value must be positive".to_string(),
            ));
        }
        target.interval_seconds = interval_seconds;
    }
    if let Some(failure_retry_seconds) = overlay.failure_retry_seconds {
        if failure_retry_seconds == 0 {
            return Err(CliError::Usage(
                "invalid oauth_refresh.failure_retry_seconds: value must be positive".to_string(),
            ));
        }
        target.failure_retry_seconds = failure_retry_seconds;
    }
    if let Some(refresh_on_startup) = overlay.refresh_on_startup {
        target.refresh_on_startup = refresh_on_startup;
    }
    Ok(())
}

fn expand_path(value: &str, base: &std::path::Path) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        return env::var_os("HOME")
            .map_or_else(|| PathBuf::from("."), PathBuf::from)
            .join(rest);
    }
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

#[cfg(test)]
mod tests;
