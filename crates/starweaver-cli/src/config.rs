//! CLI configuration resolution.

use std::{env, fs, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{args::Cli, error::io_error, CliError, CliResult};

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
    /// Automatic trim after a run.
    pub auto_trim: bool,
    /// Recent runs to keep for automatic trim.
    pub current_session_keep_recent_runs: usize,
    /// All sessions keep-days placeholder.
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
    trim: Option<TrimConfig>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct GeneralConfig {
    default_profile: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct StorageConfig {
    database_path: Option<String>,
    file_store_path: Option<String>,
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
            auto_trim: true,
            current_session_keep_recent_runs: 20,
            all_sessions_keep_days: 60,
        };
        apply_file_config(&mut config, &global_dir.join("config.toml"))?;
        apply_file_config(&mut config, &project_dir.join("config.toml"))?;
        apply_env(&mut config);
        if let Some(store) = cli.store.as_ref() {
            config.database_path = expand_path(store, &project_dir);
        }
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
    }
    if let Some(storage) = parsed.storage {
        if let Some(database_path) = storage.database_path {
            config.database_path = expand_path(&database_path, base);
        }
        if let Some(file_store_path) = storage.file_store_path {
            config.file_store_path = expand_path(&file_store_path, base);
        }
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

fn apply_env(config: &mut CliConfig) {
    if let Some(value) = env::var_os("STARWEAVER_PROFILE") {
        config.default_profile = value.to_string_lossy().to_string();
    }
    if let Some(value) = env::var_os("STARWEAVER_SESSION_DB") {
        config.database_path = PathBuf::from(value);
    }
    if let Some(value) = env::var_os("STARWEAVER_FILE_STORE") {
        config.file_store_path = PathBuf::from(value);
    }
    if env::var_os("STARWEAVER_NO_AUTO_TRIM").is_some() {
        config.auto_trim = false;
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
    let temp = path.with_extension("json.tmp");
    let value = serde_json::json!({
        "current_session_id": session_id,
        "database_path": config.database_path,
        "profile": config.default_profile,
    });
    fs::write(&temp, serde_json::to_vec_pretty(&value)?).map_err(|error| io_error(&temp, error))?;
    fs::rename(&temp, &path).map_err(|error| io_error(&path, error))?;
    Ok(())
}

/// Return a config value by key.
pub fn get_config_value(config: &CliConfig, key: &str) -> CliResult<String> {
    let value = match key {
        "general.default_profile" => config.default_profile.clone(),
        "storage.database_path" => config.database_path.display().to_string(),
        "storage.file_store_path" => config.file_store_path.display().to_string(),
        "trim.auto_after_run" => config.auto_trim.to_string(),
        "trim.current_session_keep_recent_runs" => {
            config.current_session_keep_recent_runs.to_string()
        }
        other => return Err(CliError::NotFound(other.to_string())),
    };
    Ok(format!("{value}\n"))
}
