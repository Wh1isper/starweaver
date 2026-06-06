//! Configuration for the Starweaver Claw service.

use std::{env, net::SocketAddr, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{ClawError, ClawResult};

const DEFAULT_PORT: u16 = 9042;
const DEFAULT_DATA_DIR: &str = ".starweaver-claw/data";
const DEFAULT_WORKSPACE_DIRNAME: &str = "workspace";
const DEFAULT_RUN_STORE_DIRNAME: &str = "run-store";
const DEFAULT_DOCKER_IMAGE: &str = "ghcr.io/wh1isper/starweaver-claw-workspace:latest";

/// Workspace execution backend.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceBackend {
    /// Execute directly on the local host with path policy enforcement.
    Local,
    /// Execute through a reusable Docker workspace container.
    #[default]
    Docker,
}

/// Claw runtime settings.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClawSettings {
    /// Service display name.
    pub app_name: String,
    /// Environment name.
    pub environment: String,
    /// HTTP host.
    pub host: String,
    /// HTTP port.
    pub port: u16,
    /// Public base URL.
    pub public_base_url: String,
    /// Optional bearer token for API calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_token: Option<String>,
    /// Persistent data directory.
    pub data_dir: PathBuf,
    /// Run artifact store directory.
    pub run_store_dir: PathBuf,
    /// SQLite database path for durable local storage.
    pub sqlite_path: PathBuf,
    /// Default workspace directory.
    pub workspace_dir: PathBuf,
    /// Workspace backend.
    pub workspace_backend: WorkspaceBackend,
    /// Docker workspace image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker_image: Option<String>,
    /// Host workspace directory as observed by Docker service containers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker_host_workspace_dir: Option<PathBuf>,
    /// Default profile name.
    pub default_profile: String,
    /// Optional YAML profile seed file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_seed_file: Option<PathBuf>,
    /// Seed profiles during startup.
    pub auto_seed_profiles: bool,
    /// Schedule dispatcher toggle.
    pub schedule_dispatch_enabled: bool,
    /// Workflow dispatcher toggle.
    pub workflow_dispatch_enabled: bool,
    /// Heartbeat dispatcher toggle.
    pub heartbeat_enabled: bool,
    /// Shutdown timeout.
    pub shutdown_timeout_seconds: u64,
}

impl Default for ClawSettings {
    fn default() -> Self {
        let data_dir = PathBuf::from(DEFAULT_DATA_DIR);
        Self {
            app_name: "Starweaver Claw".to_string(),
            environment: "development".to_string(),
            host: "127.0.0.1".to_string(),
            port: DEFAULT_PORT,
            public_base_url: format!("http://127.0.0.1:{DEFAULT_PORT}"),
            api_token: None,
            run_store_dir: data_dir.join(DEFAULT_RUN_STORE_DIRNAME),
            sqlite_path: data_dir.join("starweaver_claw.sqlite3"),
            workspace_dir: data_dir.join(DEFAULT_WORKSPACE_DIRNAME),
            data_dir,
            workspace_backend: WorkspaceBackend::Docker,
            docker_image: Some(DEFAULT_DOCKER_IMAGE.to_string()),
            docker_host_workspace_dir: None,
            default_profile: "default".to_string(),
            profile_seed_file: None,
            auto_seed_profiles: false,
            schedule_dispatch_enabled: true,
            workflow_dispatch_enabled: true,
            heartbeat_enabled: false,
            shutdown_timeout_seconds: 30,
        }
    }
}

impl ClawSettings {
    /// Build settings from `STARWEAVER_CLAW_*` environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        let mut settings = Self::default();
        settings.app_name = env_string("APP_NAME", settings.app_name);
        settings.environment = env_string("ENVIRONMENT", settings.environment);
        settings.host = env_string("HOST", settings.host);
        settings.port = env_parse("PORT").unwrap_or(settings.port);
        settings.public_base_url = env_string("PUBLIC_BASE_URL", settings.public_base_url);
        settings.api_token = env_optional_string("API_TOKEN");
        settings.data_dir = env_path("DATA_DIR").unwrap_or(settings.data_dir);
        settings.run_store_dir = env_path("RUN_STORE_DIR")
            .unwrap_or_else(|| settings.data_dir.join(DEFAULT_RUN_STORE_DIRNAME));
        settings.sqlite_path = env_path("SQLITE_PATH")
            .or_else(|| env_path("DATABASE_PATH"))
            .unwrap_or_else(|| settings.data_dir.join("starweaver_claw.sqlite3"));
        settings.workspace_dir = env_path("WORKSPACE_DIR")
            .unwrap_or_else(|| settings.data_dir.join(DEFAULT_WORKSPACE_DIRNAME));
        settings.workspace_backend = env_optional_string("WORKSPACE_PROVIDER_BACKEND")
            .or_else(|| env_optional_string("WORKSPACE_BACKEND"))
            .as_deref()
            .map(parse_workspace_backend)
            .unwrap_or(settings.workspace_backend);
        settings.docker_image = env_optional_string("WORKSPACE_PROVIDER_DOCKER_IMAGE")
            .or_else(|| env_optional_string("DOCKER_IMAGE"))
            .or(settings.docker_image);
        settings.docker_host_workspace_dir =
            env_path("WORKSPACE_PROVIDER_DOCKER_HOST_WORKSPACE_DIR")
                .or_else(|| env_path("DOCKER_HOST_WORKSPACE_DIR"));
        settings.default_profile = env_string("DEFAULT_PROFILE", settings.default_profile);
        settings.profile_seed_file = env_path("PROFILE_SEED_FILE");
        settings.auto_seed_profiles =
            env_bool("AUTO_SEED_PROFILES").unwrap_or(settings.auto_seed_profiles);
        settings.schedule_dispatch_enabled =
            env_bool("SCHEDULE_DISPATCH_ENABLED").unwrap_or(settings.schedule_dispatch_enabled);
        settings.workflow_dispatch_enabled =
            env_bool("WORKFLOW_DISPATCH_ENABLED").unwrap_or(settings.workflow_dispatch_enabled);
        settings.heartbeat_enabled =
            env_bool("HEARTBEAT_ENABLED").unwrap_or(settings.heartbeat_enabled);
        settings.shutdown_timeout_seconds =
            env_parse("SHUTDOWN_TIMEOUT_SECONDS").unwrap_or(settings.shutdown_timeout_seconds);
        settings
    }

    /// Load settings from a TOML file and environment overrides.
    ///
    /// # Errors
    ///
    /// Returns I/O or TOML decode errors for invalid files.
    pub fn from_file(path: impl Into<PathBuf>) -> ClawResult<Self> {
        let path = path.into();
        let content = std::fs::read_to_string(path)?;
        let mut settings: Self =
            toml::from_str(&content).map_err(|error| ClawError::Failed(error.to_string()))?;
        let env_settings = Self::from_env();
        if env_optional_string("HOST").is_some() {
            settings.host = env_settings.host;
        }
        if env_optional_string("PORT").is_some() {
            settings.port = env_settings.port;
        }
        if env_optional_string("API_TOKEN").is_some() {
            settings.api_token = env_settings.api_token;
        }
        Ok(settings)
    }

    /// Return HTTP bind address.
    ///
    /// # Errors
    ///
    /// Returns parse errors for invalid `host:port` values.
    pub fn socket_addr(&self) -> ClawResult<SocketAddr> {
        format!("{}:{}", self.host, self.port)
            .parse()
            .map_err(|error| ClawError::InvalidRequest(format!("invalid bind address: {error}")))
    }

    /// Ensure required runtime directories exist.
    ///
    /// # Errors
    ///
    /// Returns filesystem errors when directories cannot be created.
    pub fn ensure_dirs(&self) -> ClawResult<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::create_dir_all(&self.run_store_dir)?;
        if let Some(parent) = self.sqlite_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir_all(&self.workspace_dir)?;
        Ok(())
    }
}

fn parse_workspace_backend(value: &str) -> WorkspaceBackend {
    match value.trim().to_ascii_lowercase().as_str() {
        "local" => WorkspaceBackend::Local,
        "docker" => WorkspaceBackend::Docker,
        _ => WorkspaceBackend::Docker,
    }
}

fn env_string(name: &str, default: String) -> String {
    env_optional_string(name).unwrap_or(default)
}

fn env_optional_string(name: &str) -> Option<String> {
    env::var(format!("STARWEAVER_CLAW_{name}"))
        .ok()
        .or_else(|| legacy_env_optional_string(name))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn legacy_env_optional_string(name: &str) -> Option<String> {
    let prefix = "YA";
    env::var(format!("{prefix}_CLAW_{name}")).ok()
}

fn env_path(name: &str) -> Option<PathBuf> {
    env_optional_string(name).map(PathBuf::from)
}

fn env_parse<T>(name: &str) -> Option<T>
where
    T: std::str::FromStr,
{
    env_optional_string(name).and_then(|value| value.parse::<T>().ok())
}

fn env_bool(name: &str) -> Option<bool> {
    env_optional_string(name).and_then(|value| match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    })
}
