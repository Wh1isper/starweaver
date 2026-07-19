//! RPC-owned host configuration.

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{RpcHostError, RpcHostResult, auth::parse_scope_list};
use crate::{RpcHttpAuthConfig, RpcHttpScope};

const DEFAULT_PROFILE_NAME: &str = "default";
const DEFAULT_MODEL_ID: &str = "openai-responses:gpt-5";

/// One RPC-owned agent profile.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RpcProfileConfig {
    /// Optional human-readable profile label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Logical production model id using `protocol:model` or `provider@protocol:model` syntax.
    pub model_id: String,
    /// Optional built-in model settings preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_settings: Option<String>,
    /// Optional built-in model capability/config preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_config: Option<String>,
    /// Static instructions owned by this RPC profile.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instructions: Vec<String>,
    /// First-party SDK toolset names enabled for this profile.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub toolsets: Vec<String>,
    /// RPC-owned subagent declarations available to this parent profile.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagents: Vec<String>,
    /// Unit-test-only deterministic response. Never deserialized from RPC config.
    #[serde(skip)]
    pub(crate) test_response: Option<String>,
}

impl Default for RpcProfileConfig {
    fn default() -> Self {
        Self {
            label: Some("Default RPC agent".to_string()),
            model_id: DEFAULT_MODEL_ID.to_string(),
            model_settings: None,
            model_config: None,
            instructions: Vec::new(),
            toolsets: Vec::new(),
            subagents: Vec::new(),
            test_response: None,
        }
    }
}

/// One RPC-owned named subagent backed by another configured profile.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RpcSubagentConfig {
    /// Child profile used to materialize the subagent runtime.
    pub profile: String,
    /// Optional model-visible description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Parent tools that must be inherited or delegation is unavailable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_tools: Vec<String>,
    /// Parent tools inherited when available.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub optional_tools: Vec<String>,
}

/// Provider endpoint configuration owned by the standalone RPC product.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RpcProviderConfig {
    /// Whether profiles may materialize this provider.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Environment variable containing the API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// Optional provider or gateway base URL override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Optional endpoint path override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_path: Option<String>,
}

impl Default for RpcProviderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            api_key_env: None,
            base_url: None,
            endpoint_path: None,
        }
    }
}

/// RPC client interaction capabilities that permit host-side model tools.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RpcClientCapabilitiesConfig {
    /// The connected client can enumerate and resolve durable HITL requests.
    #[serde(default)]
    pub hitl: bool,
    /// The client has dedicated rendering and answer input for clarifying questions.
    #[serde(default)]
    pub clarifying_questions: bool,
}

impl RpcClientCapabilitiesConfig {
    fn validate(&self) -> RpcHostResult<()> {
        if self.clarifying_questions && !self.hitl {
            return Err(RpcHostError::Invalid(
                "client_capabilities.clarifying_questions requires client_capabilities.hitl"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

/// Session-search backend selected by RPC-owned configuration.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RpcSessionSearchBackend {
    /// Canonical `SQLite` records with optional bounded local display mirrors.
    #[default]
    Local,
}

/// RPC-owned optional session-search configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RpcSessionSearchConfig {
    /// Install the optional provider.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Provider backend. External production backends are a follow-on.
    #[serde(default)]
    pub backend: RpcSessionSearchBackend,
    /// Optional local display/offload root matching this database namespace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_root: Option<PathBuf>,
    /// Maximum request text bytes.
    #[serde(default = "default_search_query_bytes")]
    pub max_query_bytes: usize,
    /// Maximum result page size.
    #[serde(default = "default_search_page_size")]
    pub max_page_size: u32,
    /// Maximum files in one bounded display scan.
    #[serde(default = "default_search_files")]
    pub max_display_files: usize,
    /// Maximum aggregate bytes in one bounded display scan.
    #[serde(default = "default_search_bytes")]
    pub max_total_display_bytes: u64,
    /// Maximum display candidates retained before ranking.
    #[serde(default = "default_search_hits")]
    pub max_display_hits: usize,
    /// Maximum local display scan wall time in milliseconds.
    #[serde(default = "default_search_timeout_ms")]
    pub scan_timeout_ms: u64,
}

impl Default for RpcSessionSearchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            backend: RpcSessionSearchBackend::Local,
            display_root: None,
            max_query_bytes: default_search_query_bytes(),
            max_page_size: default_search_page_size(),
            max_display_files: default_search_files(),
            max_total_display_bytes: default_search_bytes(),
            max_display_hits: default_search_hits(),
            scan_timeout_ms: default_search_timeout_ms(),
        }
    }
}

/// Resolved configuration for the standalone RPC product.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RpcConfig {
    /// RPC config file considered during resolution.
    pub config_path: PathBuf,
    /// `SQLite` database used for product-neutral durable evidence.
    pub database_path: PathBuf,
    /// RPC-owned client state directory.
    pub state_dir: PathBuf,
    /// Workspace root exposed through the local environment provider.
    pub workspace_root: PathBuf,
    /// Default RPC agent profile name.
    pub default_profile: String,
    /// RPC-owned agent profiles.
    pub profiles: BTreeMap<String, RpcProfileConfig>,
    /// RPC-owned provider endpoint definitions.
    pub providers: BTreeMap<String, RpcProviderConfig>,
    /// RPC-owned named subagent declarations.
    pub subagents: BTreeMap<String, RpcSubagentConfig>,
    /// Client interaction capabilities explicitly supported by the RPC frontend.
    pub client_capabilities: RpcClientCapabilitiesConfig,
    /// HTTP transport authentication and request-origin policy.
    pub http_auth: RpcHttpAuthConfig,
    /// Optional RPC-owned session-search provider configuration.
    pub session_search: RpcSessionSearchConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileConfig {
    server: Option<FileServerConfig>,
    profiles: Option<BTreeMap<String, RpcProfileConfig>>,
    providers: Option<BTreeMap<String, RpcProviderConfig>>,
    subagents: Option<BTreeMap<String, RpcSubagentConfig>>,
    client_capabilities: Option<RpcClientCapabilitiesConfig>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileServerConfig {
    database_path: Option<PathBuf>,
    state_dir: Option<PathBuf>,
    workspace_root: Option<PathBuf>,
    default_profile: Option<String>,
    http_auth: Option<FileHttpAuthConfig>,
    session_search: Option<RpcSessionSearchConfig>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileHttpAuthConfig {
    token_env: Option<String>,
    token_file: Option<PathBuf>,
    scopes: Option<BTreeSet<RpcHttpScope>>,
    allowed_origins: Option<BTreeSet<String>>,
    allowed_hosts: Option<BTreeSet<String>>,
}

impl RpcConfig {
    /// Resolve standalone RPC configuration without importing CLI configuration.
    ///
    /// The optional file defaults to `$STARWEAVER_CONFIG_DIR/rpc.toml` (or
    /// `~/.starweaver/rpc.toml`) and has an RPC-specific schema. `STARWEAVER_RPC_CONFIG`,
    /// `STARWEAVER_SESSION_DB` (`STARWEAVER_STORE` compatibility alias), and
    /// `STARWEAVER_RPC_PROFILE` remain process-level overrides.
    ///
    /// # Errors
    ///
    /// Returns an error when the current directory or RPC config file cannot be read or parsed.
    pub fn resolve(store: Option<String>) -> RpcHostResult<Self> {
        let current_dir = env::current_dir().map_err(RpcHostError::Io)?;
        let global_dir = match env::var_os("STARWEAVER_CONFIG_DIR") {
            Some(path) => PathBuf::from(path),
            None => default_global_dir()?,
        };
        let config_path = env::var_os("STARWEAVER_RPC_CONFIG")
            .map_or_else(|| global_dir.join("rpc.toml"), PathBuf::from);
        let file = read_file_config(&config_path)?;
        let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
        let server = file.server.unwrap_or_default();
        let database_path = store
            .map(PathBuf::from)
            .or_else(|| env::var_os("STARWEAVER_SESSION_DB").map(PathBuf::from))
            .or_else(|| env::var_os("STARWEAVER_STORE").map(PathBuf::from))
            .unwrap_or_else(|| {
                server.database_path.map_or_else(
                    || starweaver_storage::canonical_session_database_path(&global_dir),
                    |path| resolve_path(config_dir, path),
                )
            });
        let state_dir = server.state_dir.map_or_else(
            || global_dir.join("rpc"),
            |path| resolve_path(config_dir, path),
        );
        let workspace_root = server
            .workspace_root
            .map_or(current_dir, |path| resolve_path(config_dir, path));
        let http_auth = resolve_http_auth(config_dir, server.http_auth)?;
        let mut session_search = server.session_search.unwrap_or_default();
        session_search.display_root = session_search
            .display_root
            .map(|path| resolve_path(config_dir, path));
        if let Ok(value) = env::var("STARWEAVER_RPC_SESSION_SEARCH") {
            session_search.enabled = match value.as_str() {
                "1" | "true" | "yes" => true,
                "0" | "false" | "no" => false,
                _ => {
                    return Err(RpcHostError::Invalid(
                        "STARWEAVER_RPC_SESSION_SEARCH must be true or false".to_string(),
                    ));
                }
            };
        }
        if session_search.max_query_bytes == 0
            || session_search.max_page_size == 0
            || session_search.max_display_files == 0
            || session_search.max_total_display_bytes == 0
            || session_search.max_display_hits == 0
            || session_search.scan_timeout_ms == 0
        {
            return Err(RpcHostError::Invalid(
                "session search limits must be greater than zero".to_string(),
            ));
        }
        let default_profile = env::var("STARWEAVER_RPC_PROFILE")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or(server.default_profile)
            .unwrap_or_else(|| DEFAULT_PROFILE_NAME.to_string());
        let mut profiles = BTreeMap::from([(
            DEFAULT_PROFILE_NAME.to_string(),
            RpcProfileConfig::default(),
        )]);
        if let Some(configured) = file.profiles {
            profiles.extend(configured);
        }
        let mut providers = default_provider_configs();
        if let Some(configured) = file.providers {
            providers.extend(configured);
        }
        let subagents = file.subagents.unwrap_or_default();
        let client_capabilities = file.client_capabilities.unwrap_or_default();
        client_capabilities.validate()?;
        Ok(Self {
            config_path,
            database_path,
            state_dir,
            workspace_root,
            default_profile,
            profiles,
            providers,
            subagents,
            client_capabilities,
            http_auth,
            session_search,
        })
    }

    /// Build an isolated deterministic unit-test configuration.
    #[cfg(test)]
    pub(crate) fn for_tests(root: &Path) -> Self {
        let profile = RpcProfileConfig {
            label: Some("Deterministic RPC test agent".to_string()),
            model_id: "test:ok".to_string(),
            test_response: Some("ok".to_string()),
            ..RpcProfileConfig::default()
        };
        Self {
            config_path: root.join("rpc.toml"),
            database_path: root.join("starweaver.sqlite"),
            state_dir: root.join("rpc-state"),
            workspace_root: root.join("workspace"),
            default_profile: DEFAULT_PROFILE_NAME.to_string(),
            profiles: BTreeMap::from([(DEFAULT_PROFILE_NAME.to_string(), profile)]),
            providers: default_provider_configs(),
            subagents: BTreeMap::new(),
            client_capabilities: RpcClientCapabilitiesConfig::default(),
            http_auth: RpcHttpAuthConfig::default(),
            session_search: RpcSessionSearchConfig::default(),
        }
    }
}

fn resolve_http_auth(
    config_dir: &Path,
    configured: Option<FileHttpAuthConfig>,
) -> RpcHostResult<RpcHttpAuthConfig> {
    let configured = configured.unwrap_or_default();
    let mut resolved = RpcHttpAuthConfig::default();
    if let Some(token_env) = configured.token_env {
        if token_env.trim().is_empty() {
            return Err(RpcHostError::Invalid(
                "server.http_auth.token_env must not be empty".to_string(),
            ));
        }
        resolved.token_env = token_env;
    }
    resolved.token_file = configured
        .token_file
        .map(|path| resolve_path(config_dir, path));
    if let Some(scopes) = configured.scopes {
        if scopes.is_empty() {
            return Err(RpcHostError::Invalid(
                "server.http_auth.scopes must not be empty".to_string(),
            ));
        }
        resolved.scopes = scopes;
    }
    if let Ok(scopes) = env::var("STARWEAVER_RPC_SCOPES") {
        resolved.scopes = parse_scope_list(&scopes)?;
    }
    resolved.allowed_origins = configured.allowed_origins.unwrap_or_default();
    resolved.allowed_hosts = configured.allowed_hosts.unwrap_or_default();
    Ok(resolved)
}

fn read_file_config(path: &Path) -> RpcHostResult<FileConfig> {
    match fs::read_to_string(path) {
        Ok(content) => toml::from_str(&content).map_err(|error| {
            RpcHostError::Invalid(format!(
                "failed to parse RPC config {}: {error}",
                path.display()
            ))
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(FileConfig::default()),
        Err(error) => Err(RpcHostError::Io(error)),
    }
}

fn resolve_path(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn default_provider_configs() -> BTreeMap<String, RpcProviderConfig> {
    BTreeMap::from([
        (
            "openai".to_string(),
            RpcProviderConfig {
                api_key_env: Some("OPENAI_API_KEY".to_string()),
                base_url: Some("https://api.openai.com/v1".to_string()),
                ..RpcProviderConfig::default()
            },
        ),
        (
            "anthropic".to_string(),
            RpcProviderConfig {
                api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
                base_url: Some("https://api.anthropic.com/v1".to_string()),
                ..RpcProviderConfig::default()
            },
        ),
        (
            "gemini".to_string(),
            RpcProviderConfig {
                api_key_env: Some("GEMINI_API_KEY".to_string()),
                base_url: Some("https://generativelanguage.googleapis.com/v1beta".to_string()),
                ..RpcProviderConfig::default()
            },
        ),
    ])
}

const fn default_true() -> bool {
    true
}

const fn default_search_query_bytes() -> usize {
    4 * 1024
}

const fn default_search_page_size() -> u32 {
    100
}

const fn default_search_files() -> usize {
    1_000
}

const fn default_search_bytes() -> u64 {
    64 * 1024 * 1024
}

const fn default_search_hits() -> usize {
    10_000
}

const fn default_search_timeout_ms() -> u64 {
    2_000
}

fn default_global_dir() -> RpcHostResult<PathBuf> {
    starweaver_storage::default_starweaver_config_dir().map_err(RpcHostError::Io)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn parses_rpc_owned_profiles_and_provider_gateways() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("rpc.toml");
        fs::write(
            &config_path,
            r#"
[server]
default_profile = "gateway"
database_path = "state/rpc.sqlite3"
workspace_root = "workspace"

[client_capabilities]
hitl = true
clarifying_questions = true

[server.http_auth]
token_env = "RPC_TEST_TOKEN"
token_file = "secrets/http-token"
scopes = ["read", "run"]
allowed_origins = ["https://rpc-host.example"]

[profiles.gateway]
model_id = "homelab@openai-responses:gpt-5.5"
model_config = "gpt_5"
toolsets = ["filesystem"]

[providers.homelab]
api_key_env = "HOMELAB_API_KEY"
base_url = "https://models.example.test/v1"
"#,
        )
        .unwrap();
        let file = read_file_config(&config_path).unwrap();
        assert_eq!(
            file.client_capabilities,
            Some(RpcClientCapabilitiesConfig {
                hitl: true,
                clarifying_questions: true,
            })
        );
        let server = file.server.unwrap();
        assert_eq!(server.default_profile.as_deref(), Some("gateway"));
        let http_auth = server.http_auth.unwrap();
        assert_eq!(http_auth.token_env.as_deref(), Some("RPC_TEST_TOKEN"));
        assert_eq!(
            http_auth.token_file.as_deref(),
            Some(Path::new("secrets/http-token"))
        );
        assert_eq!(
            http_auth.scopes.unwrap(),
            BTreeSet::from([RpcHttpScope::Read, RpcHttpScope::Run])
        );
        assert_eq!(
            file.profiles.unwrap()["gateway"].model_id,
            "homelab@openai-responses:gpt-5.5"
        );
        assert_eq!(
            file.providers.unwrap()["homelab"].api_key_env.as_deref(),
            Some("HOMELAB_API_KEY")
        );
    }

    #[test]
    fn clarifying_questions_require_general_hitl_support() {
        let error = RpcClientCapabilitiesConfig {
            hitl: false,
            clarifying_questions: true,
        }
        .validate()
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("requires client_capabilities.hitl")
        );
    }
}
