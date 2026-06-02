//! CLI configuration resolution.

use std::{env, fs, path::PathBuf, process, thread};

use serde::{Deserialize, Serialize};
use toml::Value;

use crate::{
    args::{Cli, CliCommand, HitlPolicy, OutputMode},
    error::io_error,
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
    /// Provider API configuration.
    pub providers: ProviderConfigs,
    /// Tool config metadata loaded from tools.toml.
    pub tools_config: serde_json::Value,
    /// MCP config metadata loaded from mcp.json.
    pub mcp_config: serde_json::Value,
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
    trim: Option<TrimConfig>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct GeneralConfig {
    default_profile: Option<String>,
    profile_search_paths: Option<Vec<String>>,
    default_output: Option<OutputMode>,
    default_hitl: Option<HitlPolicy>,
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

/// Provider API configuration.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ProviderConfigs {
    /// `OpenAI` provider config.
    pub openai: ProviderConfig,
    /// Anthropic provider config.
    pub anthropic: ProviderConfig,
    /// Gemini provider config.
    pub gemini: ProviderConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileProviderConfigs {
    openai: Option<FileProviderConfig>,
    anthropic: Option<FileProviderConfig>,
    gemini: Option<FileProviderConfig>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileProviderConfig {
    enabled: Option<bool>,
    api_key_env: Option<String>,
    base_url: Option<String>,
    endpoint_path: Option<String>,
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
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            api_key_env: None,
            base_url: None,
            endpoint_path: None,
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
            workspace_root: project_dir
                .parent()
                .map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf),
            environment_provider: "local".to_string(),
            files_policy: "read_only".to_string(),
            shell_enabled: false,
            default_output: OutputMode::DisplayJsonl,
            default_hitl: HitlPolicy::Deny,
            update_channel: "stable".to_string(),
            providers: default_provider_configs(),
            tools_config: serde_json::Value::Null,
            mcp_config: serde_json::Value::Null,
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
    }
}

fn apply_file_config(config: &mut CliConfig, path: &PathBuf) -> CliResult<()> {
    if !path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(path).map_err(|error| io_error(path, error))?;
    let parsed = toml::from_str::<FileConfig>(&content)?;
    let base = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    if let Some(general) = parsed.general {
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
}

fn apply_env(config: &mut CliConfig) {
    if let Some(value) = env::var_os("STARWEAVER_PROFILE") {
        config.default_profile = value.to_string_lossy().to_string();
    }
    if let Some(value) = env::var_os("STARWEAVER_PROFILE_PATHS") {
        config.profile_search_paths = env::split_paths(&value).collect();
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
        "storage.database_path" => config.database_path.display().to_string(),
        "storage.file_store_path" => config.file_store_path.display().to_string(),
        "environment.workspace_root" => config.workspace_root.display().to_string(),
        "environment.provider" => config.environment_provider.clone(),
        "environment.files_policy" => config.files_policy.clone(),
        "environment.shell_enabled" => config.shell_enabled.to_string(),
        "update.channel" => config.update_channel.clone(),
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
        "providers.openai.ready" => provider_ready(&config.providers.openai).to_string(),
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
        "providers.gemini.ready" => provider_ready(&config.providers.gemini).to_string(),
        "trim.auto_after_run" => config.auto_trim.to_string(),
        "trim.current_session_keep_recent_runs" => {
            config.current_session_keep_recent_runs.to_string()
        }
        "trim.all_sessions_keep_days" => config.all_sessions_keep_days.to_string(),
        "metadata.tools" => serde_json::to_string(&config.tools_config)?,
        "metadata.mcp" => serde_json::to_string(&config.mcp_config)?,
        other => return Err(CliError::NotFound(other.to_string())),
    };
    Ok(format!("{value}\n"))
}

fn provider_ready(provider: &ProviderConfig) -> bool {
    provider.enabled
        && provider.api_key_env.as_deref().is_some_and(|name| {
            let name = name.trim();
            !name.is_empty() && env::var(name).is_ok_and(|value| !value.trim().is_empty())
        })
}

fn read_tools_config(
    global_dir: &std::path::Path,
    project_dir: &std::path::Path,
) -> CliResult<serde_json::Value> {
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
    if parts.next().is_some() || section != "providers" {
        return None;
    }
    match (provider, field) {
        (
            "openai" | "anthropic" | "gemini",
            "enabled" | "api_key_env" | "base_url" | "endpoint_path",
        ) => Some((provider, field)),
        _ => None,
    }
}

fn split_config_key(key: &str) -> CliResult<(&str, &str)> {
    if let Some((section, field)) = key.split_once('.') {
        match (section, field) {
            (
                "general",
                "default_profile" | "profile_search_paths" | "default_output" | "default_hitl",
            )
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
    let parsed = match key {
        "general.default_profile"
        | "storage.database_path"
        | "storage.file_store_path"
        | "environment.workspace_root"
        | "providers.openai.base_url"
        | "providers.openai.endpoint_path"
        | "providers.anthropic.base_url"
        | "providers.anthropic.endpoint_path"
        | "providers.gemini.base_url"
        | "providers.gemini.endpoint_path"
        | "update.channel" => Value::String(value.to_string()),
        "general.default_output" => Value::String(validated_output_mode(value)?.to_string()),
        "general.default_hitl" => Value::String(validated_hitl_policy(value)?.to_string()),
        "environment.provider" => Value::String(validated_environment_provider(value)?.to_string()),
        "environment.files_policy" => Value::String(validated_files_policy(value)?.to_string()),
        "providers.openai.api_key_env"
        | "providers.anthropic.api_key_env"
        | "providers.gemini.api_key_env" => {
            Value::String(validated_non_empty(key, value)?.to_string())
        }
        "general.profile_search_paths" => Value::Array(
            value
                .split(':')
                .filter(|path| !path.trim().is_empty())
                .map(|path| Value::String(path.to_string()))
                .collect(),
        ),
        "trim.auto_after_run"
        | "environment.shell_enabled"
        | "providers.openai.enabled"
        | "providers.anthropic.enabled"
        | "providers.gemini.enabled" => value
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
