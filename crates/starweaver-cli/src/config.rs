//! CLI configuration resolution.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process, thread,
};

use serde::{Deserialize, Serialize};
use starweaver_model::MaxTokensParameter;
use toml::Value;

use crate::{
    args::{Cli, CliCommand, HitlPolicy, OutputMode},
    error::io_error,
    oauth::CODEX_BASE_URL,
    CliError, CliResult,
};

/// Resolved CLI configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CliConfig {
    /// Global config root.
    pub global_dir: PathBuf,
    /// Project config root.
    pub project_dir: PathBuf,
    /// `SQLite` database path.
    pub database_path: PathBuf,
    /// Local file store path.
    pub file_store_path: PathBuf,
    /// Default profile.
    pub default_profile: String,
    /// Profile search paths.
    pub profile_search_paths: Vec<PathBuf>,
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
    /// Default output mode.
    pub default_output: OutputMode,
    /// Default headless human-in-the-loop policy.
    pub default_hitl: HitlPolicy,
    /// Update channel metadata.
    pub update_channel: String,
    /// Default model from `[general] model` fields.
    pub default_model: Option<CliModelProfile>,
    /// Named model profiles from `[model_profiles.*]` fields.
    pub model_profiles: BTreeMap<String, CliModelProfile>,
    /// Environment variables loaded from config `[env]` sections.
    pub env_vars: BTreeMap<String, String>,
    /// Provider API configuration.
    pub providers: ProviderConfigs,
    /// Tool config metadata loaded from tools.toml.
    pub tools_config: serde_json::Value,
    /// MCP config metadata loaded from mcp.json.
    pub mcp_config: serde_json::Value,
    /// Compatibility metadata for config sections preserved for migration audits.
    pub compatibility_metadata: serde_json::Value,
    /// Automatic trim after a run.
    pub auto_trim: bool,
    /// Recent runs to keep for automatic trim.
    pub current_session_keep_recent_runs: usize,
    /// Retention horizon for future all-session maintenance.
    pub all_sessions_keep_days: u64,
}

/// Config resolver.
#[derive(Clone, Debug)]
pub struct ConfigResolver {
    global_dir: Option<PathBuf>,
    project_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileConfig {
    general: Option<GeneralConfig>,
    storage: Option<StorageConfig>,
    environment: Option<EnvironmentConfig>,
    update: Option<UpdateConfig>,
    providers: Option<FileProviderConfigs>,
    model_profiles: Option<BTreeMap<String, FileModelProfile>>,
    env: Option<BTreeMap<String, String>>,
    skills: Option<SkillsConfig>,
    subagents: Option<SubagentsConfig>,
    trim: Option<TrimConfig>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct GeneralConfig {
    default_profile: Option<String>,
    profile_search_paths: Option<Vec<String>>,
    default_output: Option<OutputMode>,
    default_hitl: Option<HitlPolicy>,
    model: Option<String>,
    model_settings: Option<String>,
    model_cfg: Option<String>,
    max_requests: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileModelProfile {
    label: Option<String>,
    model: Option<String>,
    model_settings: Option<String>,
    model_cfg: Option<String>,
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

/// Provider API configuration.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ProviderConfigs {
    /// `OpenAI` provider config.
    pub openai: ProviderConfig,
    /// Anthropic provider config.
    pub anthropic: ProviderConfig,
    /// Gemini provider config.
    pub gemini: ProviderConfig,
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
    codex: Option<FileProviderConfig>,
    #[serde(flatten)]
    gateways: BTreeMap<String, FileProviderConfig>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileProviderConfig {
    enabled: Option<bool>,
    api_key_env: Option<String>,
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
        }
    }

    /// Resolve final config.
    pub fn resolve(&self, cli: &Cli) -> CliResult<CliConfig> {
        let global_dir = self.global_dir.clone().unwrap_or_else(default_global_dir);
        let project_dir = self.project_dir.clone().unwrap_or_else(default_project_dir);
        let mut config = CliConfig {
            global_dir: global_dir.clone(),
            project_dir: project_dir.clone(),
            database_path: project_dir.join("starweaver.sqlite"),
            file_store_path: project_dir.join("store"),
            default_profile: "general".to_string(),
            profile_search_paths: vec![
                project_dir.join("profiles"),
                project_dir.join("agents"),
                global_dir.join("profiles"),
            ],
            skill_dirs: vec![global_dir.join("skills"), project_dir.join("skills")],
            subagent_dirs: vec![global_dir.join("subagents"), project_dir.join("subagents")],
            disabled_subagents: Vec::new(),
            workspace_root: project_dir
                .parent()
                .map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf),
            environment_provider: "local".to_string(),
            files_policy: "read_only".to_string(),
            shell_enabled: false,
            default_output: OutputMode::DisplayJsonl,
            default_hitl: HitlPolicy::Deny,
            update_channel: "stable".to_string(),
            default_model: None,
            model_profiles: BTreeMap::new(),
            env_vars: BTreeMap::new(),
            providers: default_provider_configs(),
            tools_config: serde_json::Value::Null,
            mcp_config: serde_json::Value::Null,
            compatibility_metadata: serde_json::json!({}),
            auto_trim: true,
            current_session_keep_recent_runs: 20,
            all_sessions_keep_days: 60,
        };
        apply_file_config(&mut config, &global_dir.join("config.toml"))?;
        apply_file_config(&mut config, &project_dir.join("config.toml"))?;
        config.tools_config = read_tools_config(&global_dir, &project_dir)?;
        config.mcp_config = read_mcp_config(&global_dir, &project_dir)?;
        apply_env(&mut config);
        apply_cli_overrides(&mut config, cli, &project_dir);
        Ok(config)
    }
}

fn default_global_dir() -> PathBuf {
    env::var_os("HOME")
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join(".starweaver")
}

fn default_project_dir() -> PathBuf {
    PathBuf::from(".starweaver")
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
    merge_compatibility_metadata(config, &raw);
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
                &mut config.compatibility_metadata,
                serde_json::json!({"general": {"max_requests": max_requests}}),
            );
        }
        if let Some(profile) = general.default_profile {
            config.default_profile = profile;
        }
        if let Some(paths) = general.profile_search_paths {
            config.profile_search_paths =
                paths.iter().map(|path| expand_path(path, base)).collect();
        }
        if let Some(output) = general.default_output {
            config.default_output = output;
        }
        if let Some(hitl) = general.default_hitl {
            config.default_hitl = hitl;
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
    if let Some(update) = parsed.update {
        if let Some(channel) = update.channel {
            config.update_channel = channel;
        }
    }
    if let Some(providers) = parsed.providers {
        merge_provider_configs(&mut config.providers, providers);
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
    if let Some(skills) = parsed.skills {
        merge_skill_dirs(config, skills, base);
    }
    if let Some(subagents) = parsed.subagents {
        merge_subagent_config(config, subagents, base);
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

fn merge_compatibility_metadata(config: &mut CliConfig, raw: &Value) {
    let Some(root) = raw.as_table() else {
        return;
    };
    let mut metadata = serde_json::Map::new();
    for key in ["display", "browser", "subagents", "commands", "security"] {
        if let Some(value) = root.get(key).cloned() {
            if let Ok(json) = serde_json::to_value(value) {
                metadata.insert(key.to_string(), json);
            }
        }
    }
    if !metadata.is_empty() {
        merge_json_value(
            &mut config.compatibility_metadata,
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

fn apply_env(config: &mut CliConfig) {
    for (key, value) in &config.env_vars {
        if env::var_os(key).is_none() {
            env::set_var(key, value);
        }
    }
    if let Some(value) = env::var_os("STARWEAVER_PROFILE") {
        config.default_profile = value.to_string_lossy().to_string();
    }
    if let Some(value) = env::var_os("STARWEAVER_PROFILE_PATHS") {
        config.profile_search_paths = env::split_paths(&value).collect();
    }
    if let Some(value) = env::var_os("STARWEAVER_SKILL_DIRS") {
        config.skill_dirs = env::split_paths(&value).collect();
    }
    if let Some(value) = env::var_os("STARWEAVER_SUBAGENT_DIRS") {
        config.subagent_dirs = env::split_paths(&value).collect();
    }
    if let Some(value) = env::var_os("STARWEAVER_DISABLED_SUBAGENTS") {
        config.disabled_subagents = value
            .to_string_lossy()
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();
    }
    if let Some(value) = env::var_os("STARWEAVER_SESSION_DB") {
        config.database_path = PathBuf::from(value);
    }
    if let Some(value) = env::var_os("STARWEAVER_FILE_STORE") {
        config.file_store_path = PathBuf::from(value);
    }
    if let Some(value) = env::var_os("STARWEAVER_WORKSPACE_ROOT") {
        config.workspace_root = PathBuf::from(value);
    }
    if let Some(value) = env::var_os("STARWEAVER_ENV_PROVIDER") {
        config.environment_provider = value.to_string_lossy().to_string();
    }
    if let Some(value) = env::var_os("STARWEAVER_FILES_POLICY") {
        config.files_policy = value.to_string_lossy().to_string();
    }
    if let Some(value) = env::var_os("STARWEAVER_SHELL_ENABLED") {
        config.shell_enabled = env_bool(&value.to_string_lossy());
    }
    if let Some(value) = env::var_os("STARWEAVER_OUTPUT") {
        if let Some(output) = parse_output_mode(&value.to_string_lossy()) {
            config.default_output = output;
        }
    }
    if let Some(value) = env::var_os("STARWEAVER_HITL") {
        if let Some(hitl) = parse_hitl_policy(&value.to_string_lossy()) {
            config.default_hitl = hitl;
        }
    }
    if let Some(value) = env::var_os("STARWEAVER_UPDATE_CHANNEL") {
        config.update_channel = value.to_string_lossy().to_string();
    }
    if let Some(value) = env::var_os("STARWEAVER_OPENAI_BASE_URL") {
        config.providers.openai.base_url = Some(value.to_string_lossy().to_string());
    }
    if let Some(value) = env::var_os("STARWEAVER_ANTHROPIC_BASE_URL") {
        config.providers.anthropic.base_url = Some(value.to_string_lossy().to_string());
    }
    if let Some(value) = env::var_os("STARWEAVER_GEMINI_BASE_URL") {
        config.providers.gemini.base_url = Some(value.to_string_lossy().to_string());
    }
    if let Some(value) = env::var_os("STARWEAVER_OPENAI_API_KEY_ENV") {
        config.providers.openai.api_key_env = Some(value.to_string_lossy().to_string());
    }
    if let Some(value) = env::var_os("STARWEAVER_ANTHROPIC_API_KEY_ENV") {
        config.providers.anthropic.api_key_env = Some(value.to_string_lossy().to_string());
    }
    if let Some(value) = env::var_os("STARWEAVER_GEMINI_API_KEY_ENV") {
        config.providers.gemini.api_key_env = Some(value.to_string_lossy().to_string());
    }
    if env::var_os("STARWEAVER_NO_AUTO_TRIM").is_some() {
        config.auto_trim = false;
    }
}

fn apply_cli_overrides(config: &mut CliConfig, cli: &Cli, project_dir: &std::path::Path) {
    if let Some(store) = cli.store.as_ref() {
        config.database_path = expand_path(store, project_dir);
    }
    if let Some(profile) = top_level_profile(cli) {
        config.default_profile = profile;
    }
    if let Some(output) = top_level_output(cli) {
        config.default_output = output;
    }
    if let Some(hitl) = top_level_hitl(cli) {
        config.default_hitl = hitl;
    }
}

fn top_level_profile(cli: &Cli) -> Option<String> {
    cli.command
        .as_ref()
        .and_then(|command| match command {
            CliCommand::Run(run) => run.profile.clone(),
            _ => None,
        })
        .or_else(|| cli.profile.clone())
}

fn top_level_output(cli: &Cli) -> Option<OutputMode> {
    cli.command
        .as_ref()
        .and_then(|command| match command {
            CliCommand::Run(run) => run.output,
            _ => None,
        })
        .or(cli.output)
}

fn top_level_hitl(cli: &Cli) -> Option<HitlPolicy> {
    cli.command
        .as_ref()
        .and_then(|command| match command {
            CliCommand::Run(run) => run.hitl,
            _ => None,
        })
        .or(cli.hitl)
}

fn env_bool(value: &str) -> bool {
    matches!(value, "1" | "true" | "TRUE" | "yes" | "on")
}

fn parse_output_mode(value: &str) -> Option<OutputMode> {
    match value {
        "text" | "Text" => Some(OutputMode::Text),
        "display-jsonl" | "display_jsonl" | "DisplayJsonl" => Some(OutputMode::DisplayJsonl),
        "silent" | "Silent" => Some(OutputMode::Silent),
        _ => None,
    }
}

fn parse_hitl_policy(value: &str) -> Option<HitlPolicy> {
    match value {
        "deny" | "Deny" => Some(HitlPolicy::Deny),
        "defer" | "Defer" => Some(HitlPolicy::Defer),
        "fail" | "Fail" => Some(HitlPolicy::Fail),
        "prompt" | "Prompt" => Some(HitlPolicy::Prompt),
        _ => None,
    }
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

/// Ensure local config directories exist.
pub fn ensure_config_dirs(config: &CliConfig) -> CliResult<()> {
    for path in [&config.project_dir, &config.file_store_path] {
        fs::create_dir_all(path).map_err(|error| io_error(path, error))?;
    }
    if let Some(parent) = config.database_path.parent() {
        fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    }
    Ok(())
}

/// Read current session pointer from project state.
pub fn read_current_session(config: &CliConfig) -> CliResult<Option<String>> {
    let path = config.project_dir.join("state.json");
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path).map_err(|error| io_error(&path, error))?;
    let value = serde_json::from_str::<serde_json::Value>(&content)?;
    Ok(value
        .get("current_session_id")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string))
}

/// Write current session pointer into project state through atomic rename.
pub fn write_current_session(config: &CliConfig, session_id: &str) -> CliResult<()> {
    fs::create_dir_all(&config.project_dir)
        .map_err(|error| io_error(&config.project_dir, error))?;
    let path = config.project_dir.join("state.json");
    let temp = config.project_dir.join(format!(
        "state.{}.{}.json.tmp",
        process::id(),
        format_thread_id(thread::current().id())
    ));
    let value = serde_json::json!({
        "current_session_id": session_id,
        "database_path": config.database_path,
        "profile": config.default_profile,
    });
    fs::write(&temp, serde_json::to_vec_pretty(&value)?).map_err(|error| io_error(&temp, error))?;
    fs::rename(&temp, &path).map_err(|error| io_error(&path, error))?;
    Ok(())
}

fn format_thread_id(id: thread::ThreadId) -> String {
    format!("{id:?}")
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect()
}

/// Config write scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigScope {
    /// Global config file.
    Global,
    /// Project config file.
    Project,
}

/// Set a config value.
pub fn set_config_value(
    config: &CliConfig,
    scope: ConfigScope,
    key: &str,
    value: &str,
) -> CliResult<()> {
    let parsed_value = parse_config_value(key, value)?;
    let root_dir = match scope {
        ConfigScope::Global => &config.global_dir,
        ConfigScope::Project => &config.project_dir,
    };
    let path = root_dir.join("config.toml");
    fs::create_dir_all(root_dir).map_err(|error| io_error(root_dir, error))?;
    let mut root = if path.exists() {
        let content = fs::read_to_string(&path).map_err(|error| io_error(&path, error))?;
        content
            .parse::<Value>()
            .map_err(|error| CliError::Config(error.to_string()))?
            .as_table()
            .cloned()
            .unwrap_or_default()
    } else {
        toml::map::Map::new()
    };
    if let Some((provider, field)) = split_provider_config_key(key) {
        let provider_root = root
            .entry("providers".to_string())
            .or_insert_with(|| Value::Table(toml::map::Map::new()));
        let provider_root_table = provider_root.as_table_mut().ok_or_else(|| {
            CliError::Usage("config section providers is not a table".to_string())
        })?;
        let selected_provider = provider_root_table
            .entry(provider.to_string())
            .or_insert_with(|| Value::Table(toml::map::Map::new()));
        let selected_provider_table = selected_provider.as_table_mut().ok_or_else(|| {
            CliError::Usage(format!(
                "config section providers.{provider} is not a table"
            ))
        })?;
        selected_provider_table.insert(field.to_string(), parsed_value);
    } else {
        let (section, field) = split_config_key(key)?;
        let section_value = root
            .entry(section.to_string())
            .or_insert_with(|| Value::Table(toml::map::Map::new()));
        let section_table = section_value
            .as_table_mut()
            .ok_or_else(|| CliError::Usage(format!("config section {section} is not a table")))?;
        section_table.insert(field.to_string(), parsed_value);
    }
    let temp = path.with_extension("toml.tmp");
    fs::write(&temp, toml::to_string_pretty(&Value::Table(root))?)
        .map_err(|error| io_error(&temp, error))?;
    fs::rename(&temp, &path).map_err(|error| io_error(&path, error))?;
    Ok(())
}

/// Initialize a config file.
pub fn init_config_file(config: &CliConfig, scope: ConfigScope, force: bool) -> CliResult<PathBuf> {
    let root_dir = match scope {
        ConfigScope::Global => &config.global_dir,
        ConfigScope::Project => &config.project_dir,
    };
    let path = root_dir.join("config.toml");
    if path.exists() && !force {
        return Err(CliError::Usage(format!(
            "config already exists at {}; pass --force to replace it",
            path.display()
        )));
    }
    fs::create_dir_all(root_dir).map_err(|error| io_error(root_dir, error))?;
    fs::write(&path, default_config_template(scope)).map_err(|error| io_error(&path, error))?;
    Ok(path)
}

pub const DEFAULT_TOOLS_TEMPLATE: &str = r#"[tools]
need_approval = ["shell", "write", "edit", "multi_edit", "delete", "move"]

"#;

pub const DEFAULT_MCP_TEMPLATE: &str = r#"{
  "servers": {}
}
"#;

pub const DEFAULT_PROJECT_GITIGNORE_TEMPLATE: &str = r"state.json
state.*.json.tmp
starweaver.sqlite
starweaver.sqlite-*
store/
";

const fn default_config_template(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::Global => {
            r#"[general]
default_profile = "general"
default_output = "text"
default_hitl = "deny"

[providers.openai]
enabled = true
api_key_env = "OPENAI_API_KEY"
base_url = "https://api.openai.com/v1"

[providers.codex]
base_url = "https://chatgpt.com/backend-api/codex"
max_tokens_parameter = "omit"

[providers.anthropic]
enabled = true
api_key_env = "ANTHROPIC_API_KEY"
base_url = "https://api.anthropic.com/v1"

[providers.gemini]
enabled = true
api_key_env = "GEMINI_API_KEY"
base_url = "https://generativelanguage.googleapis.com/v1beta"

[update]
channel = "stable"
"#
        }
        ConfigScope::Project => {
            r#"[general]
default_profile = "general"
default_output = "text"
default_hitl = "deny"

[environment]
provider = "local"
files_policy = "read_only"
shell_enabled = false
workspace_root = ".."

[trim]
auto_after_run = true
current_session_keep_recent_runs = 20
all_sessions_keep_days = 60
"#
        }
    }
}

/// Return a config value by key.
#[allow(clippy::too_many_lines)]
pub fn get_config_value(config: &CliConfig, key: &str) -> CliResult<String> {
    let value = match key {
        "general.default_profile" => config.default_profile.clone(),
        "general.profile_search_paths" => config
            .profile_search_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(":"),
        "general.default_output" => output_mode_name(config.default_output).to_string(),
        "general.default_hitl" => hitl_policy_name(config.default_hitl).to_string(),
        "skills.dirs" => config
            .skill_dirs
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(":"),
        "subagents.dirs" => config
            .subagent_dirs
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(":"),
        "subagents.disabled" => config.disabled_subagents.join(","),
        "storage.database_path" => config.database_path.display().to_string(),
        "storage.file_store_path" => config.file_store_path.display().to_string(),
        "environment.workspace_root" => config.workspace_root.display().to_string(),
        "environment.provider" => config.environment_provider.clone(),
        "environment.files_policy" => config.files_policy.clone(),
        "environment.shell_enabled" => config.shell_enabled.to_string(),
        "update.channel" => config.update_channel.clone(),
        "general.model" | "model.default.model" => config
            .default_model
            .as_ref()
            .map(|profile| profile.model_id.clone())
            .unwrap_or_default(),
        "general.model_settings" | "model.default.model_settings" => config
            .default_model
            .as_ref()
            .and_then(|profile| profile.model_settings.clone())
            .unwrap_or_default(),
        "general.model_cfg" | "model.default.model_cfg" => config
            .default_model
            .as_ref()
            .and_then(|profile| profile.model_cfg.clone())
            .unwrap_or_default(),
        "model.profiles" => serde_json::to_string(&config.model_profiles)?,
        "env" => serde_json::to_string(&config.env_vars)?,
        "providers.openai.enabled" => config.providers.openai.enabled.to_string(),
        "providers.openai.api_key_env" => config
            .providers
            .openai
            .api_key_env
            .clone()
            .unwrap_or_default(),
        "providers.openai.base_url" => config.providers.openai.base_url.clone().unwrap_or_default(),
        "providers.openai.endpoint_path" => config
            .providers
            .openai
            .endpoint_path
            .clone()
            .unwrap_or_default(),
        "providers.openai.max_tokens_parameter" => {
            max_tokens_parameter_name(config.providers.openai.max_tokens_parameter).to_string()
        }
        "providers.openai.ready" => provider_ready(&config.providers.openai).to_string(),
        "providers.codex.enabled" => config.providers.codex.enabled.to_string(),
        "providers.codex.base_url" => config.providers.codex.base_url.clone().unwrap_or_default(),
        "providers.codex.endpoint_path" => config
            .providers
            .codex
            .endpoint_path
            .clone()
            .unwrap_or_default(),
        "providers.codex.max_tokens_parameter" => {
            max_tokens_parameter_name(config.providers.codex.max_tokens_parameter).to_string()
        }
        "providers.anthropic.enabled" => config.providers.anthropic.enabled.to_string(),
        "providers.anthropic.api_key_env" => config
            .providers
            .anthropic
            .api_key_env
            .clone()
            .unwrap_or_default(),
        "providers.anthropic.base_url" => config
            .providers
            .anthropic
            .base_url
            .clone()
            .unwrap_or_default(),
        "providers.anthropic.endpoint_path" => config
            .providers
            .anthropic
            .endpoint_path
            .clone()
            .unwrap_or_default(),
        "providers.anthropic.max_tokens_parameter" => {
            max_tokens_parameter_name(config.providers.anthropic.max_tokens_parameter).to_string()
        }
        "providers.anthropic.ready" => provider_ready(&config.providers.anthropic).to_string(),
        "providers.gemini.enabled" => config.providers.gemini.enabled.to_string(),
        "providers.gemini.api_key_env" => config
            .providers
            .gemini
            .api_key_env
            .clone()
            .unwrap_or_default(),
        "providers.gemini.base_url" => config.providers.gemini.base_url.clone().unwrap_or_default(),
        "providers.gemini.endpoint_path" => config
            .providers
            .gemini
            .endpoint_path
            .clone()
            .unwrap_or_default(),
        "providers.gemini.max_tokens_parameter" => {
            max_tokens_parameter_name(config.providers.gemini.max_tokens_parameter).to_string()
        }
        "providers.gemini.ready" => provider_ready(&config.providers.gemini).to_string(),
        "trim.auto_after_run" => config.auto_trim.to_string(),
        "trim.current_session_keep_recent_runs" => {
            config.current_session_keep_recent_runs.to_string()
        }
        "trim.all_sessions_keep_days" => config.all_sessions_keep_days.to_string(),
        "metadata.tools" => serde_json::to_string(&config.tools_config)?,
        "metadata.mcp" => serde_json::to_string(&config.mcp_config)?,
        "metadata.compatibility" => serde_json::to_string(&config.compatibility_metadata)?,
        other => {
            if let Some((provider, field)) = split_provider_config_key(other) {
                if let Some(provider_config) = provider_config_by_name(config, provider) {
                    provider_config_value(provider_config, field)?
                } else {
                    return Err(CliError::NotFound(other.to_string()));
                }
            } else {
                return Err(CliError::NotFound(other.to_string()));
            }
        }
    };
    Ok(format!("{value}\n"))
}

fn provider_config_by_name<'a>(
    config: &'a CliConfig,
    provider: &str,
) -> Option<&'a ProviderConfig> {
    match provider {
        "openai" => Some(&config.providers.openai),
        "codex" => Some(&config.providers.codex),
        "anthropic" => Some(&config.providers.anthropic),
        "gemini" => Some(&config.providers.gemini),
        gateway => config.providers.gateways.get(gateway),
    }
}

fn provider_config_value(provider: &ProviderConfig, field: &str) -> CliResult<String> {
    let value = match field {
        "enabled" => provider.enabled.to_string(),
        "api_key_env" => provider.api_key_env.clone().unwrap_or_default(),
        "base_url" => provider.base_url.clone().unwrap_or_default(),
        "endpoint_path" => provider.endpoint_path.clone().unwrap_or_default(),
        "max_tokens_parameter" => {
            max_tokens_parameter_name(provider.max_tokens_parameter).to_string()
        }
        "ready" => provider_ready(provider).to_string(),
        other => return Err(CliError::NotFound(other.to_string())),
    };
    Ok(value)
}

fn provider_ready(provider: &ProviderConfig) -> bool {
    provider.enabled
        && provider.api_key_env.as_deref().is_some_and(|name| {
            let name = name.trim();
            !name.is_empty() && env::var(name).is_ok_and(|value| !value.trim().is_empty())
        })
}

/// Return tool policy entries requiring approval.
#[must_use]
pub fn tool_need_approval(config: &CliConfig) -> Vec<String> {
    let values = config
        .tools_config
        .get("tools")
        .and_then(|tools| tools.get("need_approval"))
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        });
    values.unwrap_or_else(default_need_approval)
}

fn default_need_approval() -> Vec<String> {
    ["shell", "write", "edit", "multi_edit", "delete", "move"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Return merged configured MCP server map.
#[must_use]
pub fn mcp_servers(config: &CliConfig) -> BTreeMap<String, serde_json::Value> {
    config
        .mcp_config
        .get("servers")
        .and_then(serde_json::Value::as_object)
        .map(|servers| {
            servers
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn read_tools_config(global_dir: &Path, project_dir: &Path) -> CliResult<serde_json::Value> {
    let mut value = serde_json::json!({});
    merge_toml_metadata(&mut value, &global_dir.join("tools.toml"))?;
    merge_toml_metadata(&mut value, &project_dir.join("tools.toml"))?;
    Ok(value)
}

fn read_mcp_config(
    global_dir: &std::path::Path,
    project_dir: &std::path::Path,
) -> CliResult<serde_json::Value> {
    let mut value = serde_json::json!({});
    merge_json_metadata(&mut value, &global_dir.join("mcp.json"))?;
    merge_json_metadata(&mut value, &project_dir.join("mcp.json"))?;
    Ok(value)
}

fn merge_toml_metadata(target: &mut serde_json::Value, path: &std::path::Path) -> CliResult<()> {
    if !path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(path).map_err(|error| io_error(path, error))?;
    let parsed = content
        .parse::<Value>()
        .map_err(|error| CliError::Config(error.to_string()))?;
    let json = serde_json::to_value(parsed)?;
    merge_json_value(target, json);
    Ok(())
}

fn merge_json_metadata(target: &mut serde_json::Value, path: &std::path::Path) -> CliResult<()> {
    if !path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(path).map_err(|error| io_error(path, error))?;
    let json = serde_json::from_str::<serde_json::Value>(&content)?;
    merge_json_value(target, json);
    Ok(())
}

fn merge_json_value(target: &mut serde_json::Value, overlay: serde_json::Value) {
    match (target, overlay) {
        (serde_json::Value::Object(target), serde_json::Value::Object(overlay)) => {
            for (key, value) in overlay {
                merge_json_value(target.entry(key).or_insert(serde_json::Value::Null), value);
            }
        }
        (target, overlay) => *target = overlay,
    }
}

const fn output_mode_name(output: OutputMode) -> &'static str {
    match output {
        OutputMode::Text => "text",
        OutputMode::DisplayJsonl => "display-jsonl",
        OutputMode::Silent => "silent",
    }
}

const fn max_tokens_parameter_name(parameter: MaxTokensParameter) -> &'static str {
    match parameter {
        MaxTokensParameter::Default => "default",
        MaxTokensParameter::MaxTokens => "max_tokens",
        MaxTokensParameter::MaxOutputTokens => "max_output_tokens",
        MaxTokensParameter::Omit => "omit",
    }
}

fn validated_max_tokens_parameter(value: &str) -> CliResult<MaxTokensParameter> {
    match value.trim() {
        "default" => Ok(MaxTokensParameter::Default),
        "max_tokens" => Ok(MaxTokensParameter::MaxTokens),
        "max_output_tokens" => Ok(MaxTokensParameter::MaxOutputTokens),
        "omit" => Ok(MaxTokensParameter::Omit),
        other => Err(CliError::Usage(format!(
            "invalid max_tokens_parameter: {other}; expected default, max_tokens, max_output_tokens, or omit"
        ))),
    }
}

const fn hitl_policy_name(hitl: HitlPolicy) -> &'static str {
    match hitl {
        HitlPolicy::Deny => "deny",
        HitlPolicy::Defer => "defer",
        HitlPolicy::Fail => "fail",
        HitlPolicy::Prompt => "prompt",
    }
}

fn split_provider_config_key(key: &str) -> Option<(&str, &str)> {
    let mut parts = key.split('.');
    let section = parts.next()?;
    let provider = parts.next()?;
    let field = parts.next()?;
    if parts.next().is_some() || section != "providers" || !valid_provider_config_name(provider) {
        return None;
    }
    match field {
        "enabled"
        | "api_key_env"
        | "base_url"
        | "endpoint_path"
        | "max_tokens_parameter"
        | "ready" => Some((provider, field)),
        _ => None,
    }
}

fn valid_provider_config_name(provider: &str) -> bool {
    !provider.is_empty()
        && provider
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn split_config_key(key: &str) -> CliResult<(&str, &str)> {
    if let Some((section, field)) = key.split_once('.') {
        match (section, field) {
            (
                "general",
                "default_profile"
                | "profile_search_paths"
                | "default_output"
                | "default_hitl"
                | "model"
                | "model_settings"
                | "model_cfg",
            )
            | ("skills", "dirs" | "additional_dirs")
            | ("subagents", "dirs" | "additional_dirs" | "disabled" | "disabled_builtins")
            | ("storage", "database_path" | "file_store_path")
            | ("environment", "workspace_root" | "provider" | "files_policy" | "shell_enabled")
            | ("update", "channel")
            | (
                "trim",
                "auto_after_run" | "current_session_keep_recent_runs" | "all_sessions_keep_days",
            ) => return Ok((section, field)),
            _ => {}
        }
    }
    Err(CliError::NotFound(key.to_string()))
}

fn validated_output_mode(value: &str) -> CliResult<&'static str> {
    match parse_output_mode(value.trim()) {
        Some(OutputMode::Text) => Ok("text"),
        Some(OutputMode::DisplayJsonl) => Ok("display-jsonl"),
        Some(OutputMode::Silent) => Ok("silent"),
        None => Err(CliError::Usage(format!(
            "invalid general.default_output: {value}; expected text, display-jsonl, or silent"
        ))),
    }
}

fn validated_hitl_policy(value: &str) -> CliResult<&'static str> {
    match parse_hitl_policy(value.trim()) {
        Some(HitlPolicy::Deny) => Ok("deny"),
        Some(HitlPolicy::Defer) => Ok("defer"),
        Some(HitlPolicy::Fail) => Ok("fail"),
        Some(HitlPolicy::Prompt) => Ok("prompt"),
        None => Err(CliError::Usage(format!(
            "invalid general.default_hitl: {value}; expected deny, defer, fail, or prompt"
        ))),
    }
}

fn validated_environment_provider(value: &str) -> CliResult<&str> {
    match value.trim() {
        "local" => Ok("local"),
        "virtual" => Ok("virtual"),
        other => Err(CliError::Usage(format!(
            "invalid environment.provider: {other}; expected local or virtual"
        ))),
    }
}

fn validated_files_policy(value: &str) -> CliResult<&str> {
    match value.trim() {
        "read_only" | "read-only" => Ok("read_only"),
        "read_write" | "read-write" => Ok("read_write"),
        "none" | "disabled" => Ok("none"),
        other => Err(CliError::Usage(format!(
            "invalid environment.files_policy: {other}; expected read_only, read_write, or none"
        ))),
    }
}

fn validated_non_empty<'a>(key: &str, value: &'a str) -> CliResult<&'a str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CliError::Usage(format!(
            "invalid {key}: value cannot be empty"
        )));
    }
    Ok(trimmed)
}

fn parse_config_value(key: &str, value: &str) -> CliResult<Value> {
    if let Some((_provider, field)) = split_provider_config_key(key) {
        return parse_provider_config_value(key, field, value);
    }
    let parsed = match key {
        "general.default_profile"
        | "general.model"
        | "general.model_settings"
        | "general.model_cfg"
        | "storage.database_path"
        | "storage.file_store_path"
        | "environment.workspace_root"
        | "providers.openai.base_url"
        | "providers.openai.endpoint_path"
        | "providers.codex.base_url"
        | "providers.codex.endpoint_path"
        | "providers.anthropic.base_url"
        | "providers.anthropic.endpoint_path"
        | "providers.gemini.base_url"
        | "providers.gemini.endpoint_path"
        | "update.channel" => Value::String(value.to_string()),
        "general.default_output" => Value::String(validated_output_mode(value)?.to_string()),
        "general.default_hitl" => Value::String(validated_hitl_policy(value)?.to_string()),
        "environment.provider" => Value::String(validated_environment_provider(value)?.to_string()),
        "environment.files_policy" => Value::String(validated_files_policy(value)?.to_string()),
        "general.profile_search_paths"
        | "skills.dirs"
        | "skills.additional_dirs"
        | "subagents.dirs"
        | "subagents.additional_dirs" => Value::Array(
            value
                .split(':')
                .filter(|path| !path.trim().is_empty())
                .map(|path| Value::String(path.to_string()))
                .collect(),
        ),
        "subagents.disabled" | "subagents.disabled_builtins" => Value::Array(
            value
                .split(',')
                .filter(|name| !name.trim().is_empty())
                .map(|name| Value::String(name.trim().to_string()))
                .collect(),
        ),
        "trim.auto_after_run" | "environment.shell_enabled" => value
            .parse::<bool>()
            .map(Value::Boolean)
            .map_err(|error| CliError::Usage(error.to_string()))?,
        "trim.current_session_keep_recent_runs" => Value::Integer(
            value
                .parse::<usize>()
                .map_err(|error| CliError::Usage(error.to_string()))?
                .try_into()
                .map_err(|error: std::num::TryFromIntError| CliError::Usage(error.to_string()))?,
        ),
        "trim.all_sessions_keep_days" => Value::Integer(
            value
                .parse::<u64>()
                .map_err(|error| CliError::Usage(error.to_string()))?
                .try_into()
                .map_err(|error: std::num::TryFromIntError| CliError::Usage(error.to_string()))?,
        ),
        _ => return Err(CliError::NotFound(key.to_string())),
    };
    Ok(parsed)
}

fn parse_provider_config_value(key: &str, field: &str, value: &str) -> CliResult<Value> {
    let parsed = match field {
        "enabled" => value
            .parse::<bool>()
            .map(Value::Boolean)
            .map_err(|error| CliError::Usage(error.to_string()))?,
        "api_key_env" => Value::String(validated_non_empty(key, value)?.to_string()),
        "base_url" | "endpoint_path" => Value::String(value.to_string()),
        "max_tokens_parameter" => Value::String(
            max_tokens_parameter_name(validated_max_tokens_parameter(value)?).to_string(),
        ),
        "ready" => {
            return Err(CliError::Usage(format!(
                "{key} is read-only; set api_key_env and export the API key"
            )))
        }
        other => return Err(CliError::NotFound(other.to_string())),
    };
    Ok(parsed)
}
