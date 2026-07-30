//! RPC-owned host configuration.

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fmt, fs,
    io::Read as _,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use starweaver_rpc_core::generated as host;

use crate::{RpcHostError, RpcHostResult, auth::parse_scope_list};
use crate::{RpcHttpAuthConfig, RpcHttpScope};

const DEFAULT_PROFILE_NAME: &str = "default";
const DEFAULT_MODEL_ID: &str = "openai-responses:gpt-5";
const MAX_LAUNCH_ENVELOPE_BYTES: u64 = 1024 * 1024;
const MAX_RPC_CONFIG_BYTES: u64 = 1024 * 1024;

/// Validated immutable evidence for the process bootstrap configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RpcLaunchEvidence {
    /// Effective public launch-schema version.
    pub schema_version: u32,
    /// Digest of the exact validated launch envelope or standalone configuration identity.
    pub envelope_digest: String,
    /// Configuration generation selected before process startup.
    pub configuration_generation: u64,
    /// Launch mode reported during initialize.
    pub mode: String,
    /// Stable execution-domain identity.
    pub execution_domain_id: String,
    /// Stable database identity.
    pub database_identity: String,
}

/// One RPC-owned agent profile.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RpcProfileConfig {
    /// Optional human-readable profile label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Logical production model id using `protocol:model`, `provider@protocol:model`, or
    /// `oauth@provider:model` syntax.
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
    /// MCP server names selected from the explicit RPC MCP config file. Empty selects all servers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<String>,
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
            mcp_servers: Vec::new(),
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

/// One private environment catalog entry owned by the RPC product.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct RpcEnvironmentConfig {
    /// Optional reviewed public display label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Provider-private materialization details.
    #[serde(flatten)]
    pub source: RpcEnvironmentSourceConfig,
    /// Public resource-ref allowlist. Keys are accepted from protocol requests; values remain
    /// process-private and only `label` may enter durable or host-visible projections.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub resources: BTreeMap<String, RpcEnvironmentResourceConfig>,
}

/// Provider-private configured environment source.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RpcEnvironmentSourceConfig {
    /// Local sources are reserved for the built-in `local` entry and rejected in file config.
    Local,
    /// An explicitly configured local envd transport.
    Envd {
        /// Private local endpoint reference; never projected or persisted.
        endpoint_ref: String,
        /// Concrete environment identity within envd. Defaults to envd's default identity.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        environment_id: Option<String>,
        /// Optional environment variable containing the endpoint credential.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth_token_env: Option<String>,
    },
}

/// One allowlisted resource resolver entry.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct RpcEnvironmentResourceConfig {
    /// Reviewed public label stored in durable mount evidence.
    pub label: String,
    /// Provider-private source reference passed only to the selected environment implementation.
    pub source_ref: String,
}

/// Safe catalog projection suitable for protocol handling.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RpcEnvironmentCatalogEntry {
    /// Generated-schema-safe public environment identity.
    pub environment_id: String,
    /// Optional reviewed public label.
    pub display_name: Option<String>,
}

/// Private source produced after allowlisted catalog resolution.
#[derive(Clone, Eq, PartialEq)]
pub enum ResolvedRpcEnvironmentSource {
    /// Built-in local provider. The session workspace grant supplies its run-time root.
    Local,
    /// Configured envd provider details.
    Envd {
        /// Private validated local endpoint reference.
        endpoint_ref: String,
        /// Concrete envd environment identity.
        environment_id: String,
        /// Optional credential read from the configured environment variable.
        auth_token: Option<String>,
    },
}

/// Private allowlisted resource resolution. Only `label` is safe to persist or project.
#[derive(Clone, Eq, PartialEq)]
pub struct ResolvedRpcEnvironmentResource {
    /// Reviewed safe label.
    pub label: String,
    /// Provider-private source reference.
    pub source_ref: String,
}

impl fmt::Debug for RpcEnvironmentConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RpcEnvironmentConfig")
            .field("display_name", &self.display_name)
            .field("source", &self.source)
            .field("resources", &self.resources)
            .finish()
    }
}

impl fmt::Debug for RpcEnvironmentSourceConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => formatter.write_str("Local"),
            Self::Envd {
                endpoint_ref: _,
                environment_id,
                auth_token_env,
            } => formatter
                .debug_struct("Envd")
                .field("endpoint_ref", &"<redacted>")
                .field(
                    "environment_id",
                    &environment_id.as_ref().map(|_| "<configured>"),
                )
                .field(
                    "auth_token_env",
                    &auth_token_env.as_ref().map(|_| "<configured>"),
                )
                .finish(),
        }
    }
}

impl fmt::Debug for RpcEnvironmentResourceConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RpcEnvironmentResourceConfig")
            .field("label", &self.label)
            .field("source_ref", &"<redacted>")
            .finish()
    }
}

impl fmt::Debug for ResolvedRpcEnvironmentSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => formatter.write_str("Local"),
            Self::Envd { .. } => formatter
                .debug_struct("Envd")
                .field("endpoint_ref", &"<redacted>")
                .field("environment_id", &"<redacted>")
                .field("auth_token", &"<redacted>")
                .finish(),
        }
    }
}

impl fmt::Debug for ResolvedRpcEnvironmentResource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedRpcEnvironmentResource")
            .field("label", &self.label)
            .field("source_ref", &"<redacted>")
            .finish()
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

/// Current active-desktop surface admitted by RPC Computer Use.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RpcComputerUseDesktopScope {
    /// Observe only the operating system's primary display.
    #[default]
    PrimaryDisplay,
    /// Observe the complete visible desktop display topology.
    VisibleDesktop,
}

/// RPC-owned process-local Computer Use policy and principal capabilities.
///
/// These capabilities are intentionally independent from generic host method scopes. A `run`
/// scope never implies active-desktop observation authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RpcComputerUseConfig {
    /// Install the process-local Computer Use coordinator.
    #[serde(default)]
    pub enabled: bool,
    /// Current active-desktop capture scope.
    #[serde(default)]
    pub desktop_scope: RpcComputerUseDesktopScope,
    /// Maximum lifetime of one fresh run-local admission in milliseconds.
    #[serde(default = "default_computer_use_grant_ttl_ms")]
    pub grant_ttl_ms: u64,
    /// Grant observation capability to the trusted local stdio principal.
    #[serde(default)]
    pub stdio_observe: bool,
    /// Grant observation capability to the configured HTTP credential principal.
    #[serde(default)]
    pub http_observe: bool,
}

impl Default for RpcComputerUseConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            desktop_scope: RpcComputerUseDesktopScope::PrimaryDisplay,
            grant_ttl_ms: default_computer_use_grant_ttl_ms(),
            stdio_observe: false,
            http_observe: false,
        }
    }
}

impl RpcComputerUseConfig {
    fn validate(&self) -> RpcHostResult<()> {
        if self.grant_ttl_ms == 0 || self.grant_ttl_ms > 15 * 60 * 1_000 {
            return Err(RpcHostError::Invalid(
                "computer_use.grant_ttl_ms must be between 1 and 900000".to_string(),
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
    /// Validated process bootstrap evidence surfaced by initialize.
    pub launch: RpcLaunchEvidence,
    /// `SQLite` database used for product-neutral durable evidence.
    pub database_path: PathBuf,
    /// RPC-owned client state directory.
    pub state_dir: PathBuf,
    /// Standalone-only startup workspace intent. Supervised hosts never carry a launch root.
    pub initial_workspace_root: Option<PathBuf>,
    /// Default RPC agent profile name.
    pub default_profile: String,
    /// RPC-owned agent profiles.
    pub profiles: BTreeMap<String, RpcProfileConfig>,
    /// RPC-owned provider endpoint definitions.
    pub providers: BTreeMap<String, RpcProviderConfig>,
    /// RPC-private configured environment catalog, including the reserved built-in `local` entry.
    pub environments: BTreeMap<String, RpcEnvironmentConfig>,
    /// RPC-owned named subagent declarations.
    pub subagents: BTreeMap<String, RpcSubagentConfig>,
    /// Explicit MCP configuration file, when installed.
    pub mcp_config_path: Option<PathBuf>,
    /// Validated MCP servers loaded from the explicit file.
    pub mcp_servers: BTreeMap<String, starweaver_tools::McpServerConfig>,
    /// Client interaction capabilities explicitly supported by the RPC frontend.
    pub client_capabilities: RpcClientCapabilitiesConfig,
    /// HTTP transport authentication and request-origin policy.
    pub http_auth: RpcHttpAuthConfig,
    /// Process-local, separately authorized Computer Use policy.
    pub computer_use: RpcComputerUseConfig,
    /// Optional RPC-owned session-search provider configuration.
    pub session_search: RpcSessionSearchConfig,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RpcRuntimeMaterialization {
    pub(crate) default_profile: String,
    pub(crate) profiles: BTreeMap<String, RpcProfileConfig>,
    pub(crate) providers: BTreeMap<String, RpcProviderConfig>,
    pub(crate) environments: BTreeMap<String, RpcEnvironmentConfig>,
    pub(crate) subagents: BTreeMap<String, RpcSubagentConfig>,
    pub(crate) mcp_servers: BTreeMap<String, starweaver_tools::McpServerConfig>,
    pub(crate) client_capabilities: RpcClientCapabilitiesConfig,
    #[cfg(test)]
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    test_responses: BTreeMap<String, String>,
}

impl RpcRuntimeMaterialization {
    pub(crate) fn public_document(&self) -> host::ConfigDocument {
        project_runtime_document(&self.default_profile, &self.profiles, &self.providers)
    }

    pub(crate) fn apply_public_document(
        &self,
        document: &host::ConfigDocument,
    ) -> RpcHostResult<Self> {
        let mut profiles = BTreeMap::new();
        for profile in &document.profiles {
            let existing = self.profiles.get(&profile.name);
            let configured = RpcProfileConfig {
                label: existing.and_then(|value| value.label.clone()),
                model_id: profile.model_id.clone(),
                model_settings: profile.model_settings.clone(),
                model_config: profile.model_config.clone(),
                instructions: profile.instructions.clone(),
                toolsets: profile.toolsets.clone(),
                subagents: existing
                    .map(|value| value.subagents.clone())
                    .unwrap_or_default(),
                mcp_servers: existing
                    .map(|value| value.mcp_servers.clone())
                    .unwrap_or_default(),
                test_response: None,
            };
            if profiles.insert(profile.name.clone(), configured).is_some() {
                return Err(RpcHostError::Invalid(
                    "runtime config contains duplicate profile names".to_string(),
                ));
            }
        }
        if !profiles.contains_key(&document.default_profile) {
            return Err(RpcHostError::Invalid(
                "runtime config defaultProfile is not declared in profiles".to_string(),
            ));
        }
        let mut providers = BTreeMap::new();
        for provider in &document.providers {
            let configured = RpcProviderConfig {
                enabled: provider.enabled,
                api_key_env: self
                    .providers
                    .get(&provider.name)
                    .and_then(|value| value.api_key_env.clone()),
                base_url: provider.base_url.clone(),
                endpoint_path: provider.endpoint_path.clone(),
            };
            if providers
                .insert(provider.name.clone(), configured)
                .is_some()
            {
                return Err(RpcHostError::Invalid(
                    "runtime config contains duplicate provider names".to_string(),
                ));
            }
        }
        let mut candidate = self.clone();
        candidate
            .default_profile
            .clone_from(&document.default_profile);
        candidate.profiles = profiles;
        candidate.providers = providers;
        #[cfg(test)]
        candidate
            .test_responses
            .retain(|name, _| candidate.profiles.contains_key(name));
        candidate.validate_persistable()?;
        Ok(candidate)
    }

    fn validate_persistable(&self) -> RpcHostResult<()> {
        if !self.profiles.contains_key(&self.default_profile) {
            return Err(RpcHostError::Invalid(
                "runtime materialization default profile is unavailable".to_string(),
            ));
        }
        self.client_capabilities.validate()?;
        for (name, server) in &self.mcp_servers {
            if !server.env.is_empty() || !server.headers.is_empty() {
                return Err(RpcHostError::Invalid(format!(
                    "MCP server {name} uses inline environment or header values; persisted runtime snapshots require credential references"
                )));
            }
            if let Some(url) = server.url.as_deref() {
                let lower = url.to_ascii_lowercase();
                let authority = url
                    .split_once("://")
                    .map_or(url, |(_, remainder)| remainder)
                    .split('/')
                    .next()
                    .unwrap_or_default();
                if authority.contains('@')
                    || ["token=", "api_key=", "apikey=", "authorization="]
                        .iter()
                        .any(|marker| lower.contains(marker))
                {
                    return Err(RpcHostError::Invalid(format!(
                        "MCP server {name} endpoint contains inline credential material"
                    )));
                }
            }
        }
        Ok(())
    }
}

fn project_runtime_document(
    default_profile: &str,
    profiles: &BTreeMap<String, RpcProfileConfig>,
    providers: &BTreeMap<String, RpcProviderConfig>,
) -> host::ConfigDocument {
    host::ConfigDocument {
        default_profile: default_profile.to_string(),
        profiles: profiles
            .iter()
            .map(|(name, profile)| host::LaunchProfile {
                instructions: profile.instructions.clone(),
                model_config: profile.model_config.clone(),
                model_id: profile.model_id.clone(),
                model_settings: profile.model_settings.clone(),
                name: name.clone(),
                toolsets: profile.toolsets.clone(),
            })
            .collect(),
        providers: providers
            .iter()
            .map(|(name, provider)| host::ConfigProvider {
                base_url: provider.base_url.clone(),
                enabled: provider.enabled,
                endpoint_path: provider.endpoint_path.clone(),
                name: name.clone(),
            })
            .collect(),
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileConfig {
    server: Option<FileServerConfig>,
    profiles: Option<BTreeMap<String, RpcProfileConfig>>,
    providers: Option<BTreeMap<String, RpcProviderConfig>>,
    environments: Option<BTreeMap<String, RpcEnvironmentConfig>>,
    subagents: Option<BTreeMap<String, RpcSubagentConfig>>,
    client_capabilities: Option<RpcClientCapabilitiesConfig>,
    computer_use: Option<RpcComputerUseConfig>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileServerConfig {
    database_path: Option<PathBuf>,
    state_dir: Option<PathBuf>,
    workspace_root: Option<PathBuf>,
    default_profile: Option<String>,
    mcp_config_path: Option<PathBuf>,
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
    #[allow(clippy::too_many_lines)]
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
        let workspace_root = server.workspace_root.map_or_else(
            || current_dir.clone(),
            |path| resolve_path(config_dir, path),
        );
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
        let mcp_config_path = resolve_mcp_config_path(
            &current_dir,
            config_dir,
            env::var_os("STARWEAVER_RPC_MCP_CONFIG").map(PathBuf::from),
            server.mcp_config_path,
        );
        let mcp_servers = mcp_config_path
            .as_deref()
            .map(load_mcp_servers)
            .transpose()?
            .unwrap_or_default();
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
        let environments = resolve_environment_catalog(file.environments.unwrap_or_default())?;
        let subagents = file.subagents.unwrap_or_default();
        let client_capabilities = file.client_capabilities.unwrap_or_default();
        client_capabilities.validate()?;
        let computer_use = file.computer_use.unwrap_or_default();
        computer_use.validate()?;
        Ok(Self {
            launch: standalone_launch_evidence(&config_path, &database_path),
            config_path,
            database_path,
            state_dir,
            initial_workspace_root: Some(workspace_root),
            default_profile,
            profiles,
            providers,
            environments,
            subagents,
            mcp_config_path,
            mcp_servers,
            client_capabilities,
            http_auth,
            computer_use,
            session_search,
        })
    }

    /// Build an isolated deterministic unit-test configuration.
    /// Resolve an exact public supervised-process launch envelope without consulting standalone
    /// configuration files, current-directory defaults, or process-level RPC overrides.
    ///
    /// # Errors
    ///
    /// Returns an error before storage is opened when the envelope is too large, malformed,
    /// unsupported, contains relative authority-bearing paths, or requests native local shell.
    pub fn from_launch_envelope(path: &Path) -> RpcHostResult<Self> {
        if !path.is_absolute() {
            return Err(RpcHostError::Invalid(
                "launch envelope path must be absolute".to_string(),
            ));
        }
        let metadata = fs::metadata(path).map_err(RpcHostError::Io)?;
        if metadata.len() > MAX_LAUNCH_ENVELOPE_BYTES {
            return Err(RpcHostError::Invalid(
                "launch envelope exceeds the 1 MiB limit".to_string(),
            ));
        }
        let bytes = fs::read(path).map_err(RpcHostError::Io)?;
        if u64::try_from(bytes.len()).map_or(true, |length| length > MAX_LAUNCH_ENVELOPE_BYTES) {
            return Err(RpcHostError::Invalid(
                "launch envelope exceeds the 1 MiB limit".to_string(),
            ));
        }
        let envelope = host::decode_launch_envelope(&bytes).map_err(|_| {
            RpcHostError::Invalid("launch envelope violates its public schema".to_string())
        })?;
        let database_path = PathBuf::from(&envelope.database.path);
        let state_dir = PathBuf::from(&envelope.state_directory);
        for (name, authority_path) in [
            ("database.path", &database_path),
            ("stateDirectory", &state_dir),
        ] {
            if !authority_path.is_absolute() {
                return Err(RpcHostError::Invalid(format!(
                    "launch envelope {name} must be absolute"
                )));
            }
        }
        if envelope.capability_caps.native_local_shell {
            return Err(RpcHostError::Invalid(
                "native local shell is unavailable for supervised Desktop launches".to_string(),
            ));
        }
        if envelope.capability_caps.clarifying_questions && !envelope.capability_caps.hitl {
            return Err(RpcHostError::Invalid(
                "clarifyingQuestions requires hitl in launch capability caps".to_string(),
            ));
        }
        let mut profiles = BTreeMap::new();
        for profile in envelope.profiles {
            if profile.toolsets.iter().any(|toolset| toolset == "shell") {
                return Err(RpcHostError::Invalid(
                    "native shell toolsets are unavailable for supervised Desktop launches"
                        .to_string(),
                ));
            }
            let name = profile.name;
            let configured = RpcProfileConfig {
                label: None,
                model_id: profile.model_id,
                model_settings: profile.model_settings,
                model_config: profile.model_config,
                instructions: profile.instructions,
                toolsets: profile.toolsets,
                subagents: Vec::new(),
                mcp_servers: Vec::new(),
                test_response: None,
            };
            if profiles.insert(name.clone(), configured).is_some() {
                return Err(RpcHostError::Invalid(format!(
                    "launch envelope contains duplicate profile {name}"
                )));
            }
        }
        if !profiles.contains_key(&envelope.default_profile) {
            return Err(RpcHostError::Invalid(
                "launch defaultProfile is not declared in profiles".to_string(),
            ));
        }
        let mut providers = BTreeMap::new();
        for provider in envelope.providers {
            let name = provider.name;
            let configured = RpcProviderConfig {
                enabled: provider.enabled,
                api_key_env: provider.credential_env,
                base_url: provider.base_url,
                endpoint_path: provider.endpoint_path,
            };
            if providers.insert(name.clone(), configured).is_some() {
                return Err(RpcHostError::Invalid(format!(
                    "launch envelope contains duplicate provider {name}"
                )));
            }
        }
        let launch = RpcLaunchEvidence {
            schema_version: envelope.schema.version,
            envelope_digest: sha256_digest(&bytes),
            configuration_generation: envelope.configuration_generation.get(),
            mode: "workspace_execution".to_string(),
            execution_domain_id: envelope.execution_domain_id,
            database_identity: envelope.database.identity,
        };
        Ok(Self {
            config_path: path.to_path_buf(),
            launch,
            database_path,
            state_dir,
            initial_workspace_root: None,
            default_profile: envelope.default_profile,
            profiles,
            providers,
            environments: resolve_environment_catalog(BTreeMap::new())?,
            subagents: BTreeMap::new(),
            mcp_config_path: None,
            mcp_servers: BTreeMap::new(),
            client_capabilities: RpcClientCapabilitiesConfig {
                hitl: envelope.capability_caps.hitl,
                clarifying_questions: envelope.capability_caps.clarifying_questions,
            },
            http_auth: RpcHttpAuthConfig::default(),
            computer_use: RpcComputerUseConfig::default(),
            session_search: RpcSessionSearchConfig::default(),
        })
    }

    #[cfg(test)]
    #[allow(clippy::expect_used)]
    pub(crate) fn for_tests(root: &Path) -> Self {
        let profile = RpcProfileConfig {
            label: Some("Deterministic RPC test agent".to_string()),
            model_id: "test:ok".to_string(),
            test_response: Some("ok".to_string()),
            ..RpcProfileConfig::default()
        };
        Self {
            launch: standalone_launch_evidence(
                &root.join("rpc.toml"),
                &root.join("starweaver.sqlite"),
            ),
            config_path: root.join("rpc.toml"),
            database_path: root.join("starweaver.sqlite"),
            state_dir: root.join("rpc-state"),
            initial_workspace_root: Some(root.join("workspace")),
            default_profile: DEFAULT_PROFILE_NAME.to_string(),
            profiles: BTreeMap::from([(DEFAULT_PROFILE_NAME.to_string(), profile)]),
            providers: default_provider_configs(),
            environments: resolve_environment_catalog(BTreeMap::new())
                .expect("built-in environment catalog must be valid"),
            subagents: BTreeMap::new(),
            mcp_config_path: None,
            mcp_servers: BTreeMap::new(),
            client_capabilities: RpcClientCapabilitiesConfig::default(),
            http_auth: RpcHttpAuthConfig::default(),
            computer_use: RpcComputerUseConfig::default(),
            session_search: RpcSessionSearchConfig::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn workspace_root_for_tests(&self) -> &Path {
        self.initial_workspace_root
            .as_deref()
            .unwrap_or_else(|| panic!("test configuration must declare an initial workspace"))
    }

    /// Build the complete, normalized, credential-reference-only runtime declaration.
    pub(crate) fn runtime_materialization(&self) -> RpcHostResult<RpcRuntimeMaterialization> {
        let materialization = RpcRuntimeMaterialization {
            default_profile: self.default_profile.clone(),
            profiles: self.profiles.clone(),
            providers: self.providers.clone(),
            environments: self.environments.clone(),
            subagents: self.subagents.clone(),
            mcp_servers: self.mcp_servers.clone(),
            client_capabilities: self.client_capabilities.clone(),
            #[cfg(test)]
            test_responses: self
                .profiles
                .iter()
                .filter_map(|(name, profile)| {
                    profile
                        .test_response
                        .as_ref()
                        .map(|response| (name.clone(), response.clone()))
                })
                .collect(),
        };
        materialization.validate_persistable()?;
        Ok(materialization)
    }

    pub(crate) fn with_runtime_materialization(
        &self,
        materialization: &RpcRuntimeMaterialization,
    ) -> RpcHostResult<Self> {
        materialization.validate_persistable()?;
        let mut candidate = self.clone();
        candidate
            .default_profile
            .clone_from(&materialization.default_profile);
        candidate.profiles.clone_from(&materialization.profiles);
        candidate.providers.clone_from(&materialization.providers);
        candidate
            .environments
            .clone_from(&materialization.environments);
        candidate.subagents.clone_from(&materialization.subagents);
        candidate
            .mcp_servers
            .clone_from(&materialization.mcp_servers);
        candidate
            .client_capabilities
            .clone_from(&materialization.client_capabilities);
        #[cfg(test)]
        for (name, response) in &materialization.test_responses {
            if let Some(profile) = candidate.profiles.get_mut(name) {
                profile.test_response = Some(response.clone());
            }
        }
        Ok(candidate)
    }

    /// Re-read the fixed standalone sources into one complete runtime declaration.
    pub(crate) fn runtime_materialization_from_source(
        &self,
    ) -> RpcHostResult<RpcRuntimeMaterialization> {
        if self.launch.mode != "standalone" {
            return self.runtime_materialization();
        }
        let file = read_existing_file_config(&self.config_path)?;
        let config_dir = self.config_path.parent().unwrap_or_else(|| Path::new("."));
        let server = file.server.unwrap_or_default();
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
        let source_root = self.initial_workspace_root.as_deref().unwrap_or(config_dir);
        let mcp_config_path =
            resolve_mcp_config_path(source_root, config_dir, None, server.mcp_config_path);
        let mcp_servers = mcp_config_path
            .as_deref()
            .map(load_mcp_servers)
            .transpose()?
            .unwrap_or_default();
        let client_capabilities = file.client_capabilities.unwrap_or_default();
        client_capabilities.validate()?;
        let candidate = RpcRuntimeMaterialization {
            default_profile: server
                .default_profile
                .unwrap_or_else(|| DEFAULT_PROFILE_NAME.to_string()),
            profiles,
            providers,
            environments: resolve_environment_catalog(file.environments.unwrap_or_default())?,
            subagents: file.subagents.unwrap_or_default(),
            mcp_servers,
            client_capabilities,
            #[cfg(test)]
            test_responses: BTreeMap::new(),
        };
        candidate.validate_persistable()?;
        Ok(candidate)
    }

    /// Return safe, deterministically ordered public catalog entries.
    #[must_use]
    pub fn environment_catalog_entries(&self) -> Vec<RpcEnvironmentCatalogEntry> {
        self.environments
            .iter()
            .map(|(environment_id, configured)| RpcEnvironmentCatalogEntry {
                environment_id: environment_id.clone(),
                display_name: configured.display_name.clone(),
            })
            .collect()
    }

    /// Resolve a public configured environment id into provider-private materialization details.
    ///
    /// The built-in `local` entry is always rooted exactly at `workspace_root`; file configuration
    /// cannot introduce another local root. Envd credentials are read only at resolution time.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown id, unavailable credential, or invalid private endpoint.
    pub fn resolve_environment_source(
        &self,
        environment_id: &str,
    ) -> RpcHostResult<ResolvedRpcEnvironmentSource> {
        let configured = self.environments.get(environment_id).ok_or_else(|| {
            RpcHostError::Invalid(format!(
                "unknown configured environmentId: {environment_id}"
            ))
        })?;
        match &configured.source {
            RpcEnvironmentSourceConfig::Local => {
                if environment_id != "local" {
                    return Err(RpcHostError::Invalid(
                        "only the reserved local environment may use the local provider"
                            .to_string(),
                    ));
                }
                Ok(ResolvedRpcEnvironmentSource::Local)
            }
            RpcEnvironmentSourceConfig::Envd {
                endpoint_ref,
                environment_id,
                auth_token_env,
            } => {
                let auth_token = auth_token_env
                    .as_deref()
                    .map(|name| {
                        env::var(name).map_err(|_| {
                            RpcHostError::Invalid(format!(
                                "configured environment credential variable is unavailable: {name}"
                            ))
                        })
                    })
                    .transpose()?;
                starweaver_envd_client::validate_local_endpoint_ref(
                    endpoint_ref,
                    auth_token.as_deref(),
                )
                .map_err(|error| RpcHostError::Invalid(error.to_string()))?;
                Ok(ResolvedRpcEnvironmentSource::Envd {
                    endpoint_ref: endpoint_ref.clone(),
                    environment_id: environment_id.clone().unwrap_or_else(|| {
                        starweaver_envd_core::DEFAULT_ENVIRONMENT_ID.to_string()
                    }),
                    auth_token,
                })
            }
        }
    }

    /// Resolve one request `resourceRef` through the configured environment allowlist.
    ///
    /// The returned source reference is provider-private. Only the reviewed `label` may be copied
    /// into durable records, host events, diagnostics, or protocol responses.
    ///
    /// # Errors
    ///
    /// Returns an error when the environment or resource ref is not configured and allowlisted.
    pub fn resolve_environment_resource(
        &self,
        environment_id: &str,
        resource_ref: &str,
        workspace_root: Option<&Path>,
    ) -> RpcHostResult<ResolvedRpcEnvironmentResource> {
        let configured = self.environments.get(environment_id).ok_or_else(|| {
            RpcHostError::Invalid(format!(
                "unknown configured environmentId: {environment_id}"
            ))
        })?;
        let resource = configured.resources.get(resource_ref).ok_or_else(|| {
            RpcHostError::Invalid(format!(
                "resourceRef is not allowlisted for environmentId {environment_id}"
            ))
        })?;
        let source_ref = match &configured.source {
            RpcEnvironmentSourceConfig::Local => workspace_root
                .ok_or_else(|| {
                    RpcHostError::Invalid(
                        "local environment resource requires a live session workspace grant"
                            .to_string(),
                    )
                })?
                .join(&resource.source_ref)
                .to_string_lossy()
                .into_owned(),
            RpcEnvironmentSourceConfig::Envd { .. } => resource.source_ref.clone(),
        };
        Ok(ResolvedRpcEnvironmentResource {
            label: resource.label.clone(),
            source_ref,
        })
    }
}

fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn standalone_launch_evidence(config_path: &Path, database_path: &Path) -> RpcLaunchEvidence {
    let envelope_digest =
        sha256_digest(format!("{}\n{}", config_path.display(), database_path.display()).as_bytes());
    let database_digest = sha256_digest(
        format!(
            "starweaver.database.locator.v1\0{}",
            database_path.display()
        )
        .as_bytes(),
    );
    RpcLaunchEvidence {
        schema_version: host::LAUNCH_SCHEMA_VERSION,
        envelope_digest,
        configuration_generation: 0,
        mode: "standalone".to_string(),
        execution_domain_id: "standalone-local".to_string(),
        database_identity: format!("database-{}", &database_digest[7..39]),
    }
}

fn resolve_environment_catalog(
    configured: BTreeMap<String, RpcEnvironmentConfig>,
) -> RpcHostResult<BTreeMap<String, RpcEnvironmentConfig>> {
    let mut environments = BTreeMap::new();
    for (environment_id, environment) in configured {
        starweaver_rpc_core::generated::EnvironmentId::new(environment_id.clone()).map_err(
            |error| RpcHostError::Invalid(format!("invalid configured environmentId: {error}")),
        )?;
        match &environment.source {
            RpcEnvironmentSourceConfig::Local => {
                if environment_id != "local" {
                    return Err(RpcHostError::Invalid(format!(
                        "configured environment {environment_id} cannot use the reserved local provider"
                    )));
                }
            }
            RpcEnvironmentSourceConfig::Envd {
                endpoint_ref,
                environment_id: concrete_environment_id,
                auth_token_env,
            } => {
                if concrete_environment_id
                    .as_ref()
                    .is_some_and(String::is_empty)
                {
                    return Err(RpcHostError::Invalid(format!(
                        "environment {environment_id} envd environment_id must not be empty"
                    )));
                }
                if auth_token_env.as_ref().is_some_and(String::is_empty) {
                    return Err(RpcHostError::Invalid(format!(
                        "environment {environment_id} auth_token_env must not be empty"
                    )));
                }
                if endpoint_ref.starts_with("http://") && auth_token_env.is_none() {
                    return Err(RpcHostError::Invalid(format!(
                        "environment {environment_id} HTTP endpoint requires auth_token_env"
                    )));
                }
                let structural_token = endpoint_ref
                    .starts_with("http://")
                    .then_some("configured-token-placeholder");
                starweaver_envd_client::validate_local_endpoint_ref(endpoint_ref, structural_token)
                    .map_err(|error| {
                        RpcHostError::Invalid(format!(
                            "environment {environment_id} has invalid endpoint_ref: {error}"
                        ))
                    })?;
            }
        }
        if environment
            .display_name
            .as_ref()
            .is_some_and(String::is_empty)
        {
            return Err(RpcHostError::Invalid(format!(
                "environment {environment_id} display_name must not be empty"
            )));
        }
        for (resource_ref, resource) in &environment.resources {
            if resource_ref.is_empty() || resource_ref.len() > 1024 {
                return Err(RpcHostError::Invalid(format!(
                    "environment {environment_id} resourceRef must contain 1 to 1024 bytes"
                )));
            }
            if resource.label.is_empty() {
                return Err(RpcHostError::Invalid(format!(
                    "environment {environment_id} resource {resource_ref} label must not be empty"
                )));
            }
            if resource.source_ref.is_empty() {
                return Err(RpcHostError::Invalid(format!(
                    "environment {environment_id} resource {resource_ref} source_ref must not be empty"
                )));
            }
            if matches!(&environment.source, RpcEnvironmentSourceConfig::Local)
                && (Path::new(&resource.source_ref).is_absolute()
                    || Path::new(&resource.source_ref)
                        .components()
                        .any(|component| {
                            matches!(
                                component,
                                std::path::Component::ParentDir
                                    | std::path::Component::RootDir
                                    | std::path::Component::Prefix(_)
                            )
                        }))
            {
                return Err(RpcHostError::Invalid(format!(
                    "local environment resource {resource_ref} must stay relative to workspace_root"
                )));
            }
        }
        environments.insert(environment_id, environment);
    }
    environments
        .entry("local".to_string())
        .or_insert_with(|| RpcEnvironmentConfig {
            display_name: Some("Local workspace".to_string()),
            source: RpcEnvironmentSourceConfig::Local,
            resources: BTreeMap::new(),
        });
    Ok(environments)
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
    read_file_config_with_missing_policy(path, true)
}

fn read_existing_file_config(path: &Path) -> RpcHostResult<FileConfig> {
    read_file_config_with_missing_policy(path, false)
}

fn read_file_config_with_missing_policy(
    path: &Path,
    allow_missing: bool,
) -> RpcHostResult<FileConfig> {
    let expected = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && allow_missing => {
            return Ok(FileConfig::default());
        }
        Err(error) => return Err(error.into()),
    };
    if expected.file_type().is_symlink() || !expected.is_file() {
        return Err(RpcHostError::Invalid(format!(
            "RPC config must be a regular file and not a symlink: {}",
            path.display()
        )));
    }
    if expected.len() > MAX_RPC_CONFIG_BYTES {
        return Err(RpcHostError::Invalid(format!(
            "RPC config exceeds the {MAX_RPC_CONFIG_BYTES}-byte limit: {}",
            path.display()
        )));
    }

    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file() {
        return Err(RpcHostError::Invalid(format!(
            "RPC config must remain a regular file while opening: {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if expected.dev() != opened.dev() || expected.ino() != opened.ino() {
            return Err(RpcHostError::Invalid(format!(
                "RPC config changed while opening: {}",
                path.display()
            )));
        }
    }
    if opened.len() > MAX_RPC_CONFIG_BYTES {
        return Err(RpcHostError::Invalid(format!(
            "RPC config exceeds the {MAX_RPC_CONFIG_BYTES}-byte limit: {}",
            path.display()
        )));
    }

    let mut bytes = Vec::new();
    file.take(MAX_RPC_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RPC_CONFIG_BYTES {
        return Err(RpcHostError::Invalid(format!(
            "RPC config exceeds the {MAX_RPC_CONFIG_BYTES}-byte limit: {}",
            path.display()
        )));
    }
    let content = String::from_utf8(bytes).map_err(|_| {
        RpcHostError::Invalid(format!("RPC config is not UTF-8: {}", path.display()))
    })?;
    toml::from_str(&content).map_err(|error| {
        RpcHostError::Invalid(format!(
            "failed to parse RPC config {}: {error}",
            path.display()
        ))
    })
}

fn resolve_mcp_config_path(
    current_dir: &Path,
    config_dir: &Path,
    environment_path: Option<PathBuf>,
    file_path: Option<PathBuf>,
) -> Option<PathBuf> {
    environment_path
        .map(|path| resolve_path(current_dir, path))
        .or_else(|| file_path.map(|path| resolve_path(config_dir, path)))
}

fn load_mcp_servers(
    path: &Path,
) -> RpcHostResult<BTreeMap<String, starweaver_tools::McpServerConfig>> {
    let mut servers = starweaver_tools::McpConfigDocument::from_path(path)
        .map_err(|error| RpcHostError::Invalid(error.to_string()))?
        .servers;
    let mcp_config_dir = path.parent().unwrap_or_else(|| Path::new("."));
    for server in servers.values_mut() {
        if let Some(cwd) = server.cwd.as_deref().map(PathBuf::from)
            && cwd.is_relative()
        {
            server.cwd = Some(
                resolve_path(mcp_config_dir, cwd)
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    Ok(servers)
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

const fn default_computer_use_grant_ttl_ms() -> u64 {
    5 * 60 * 1_000
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

[computer_use]
enabled = true
desktop_scope = "visible_desktop"
grant_ttl_ms = 45000
stdio_observe = true
http_observe = false

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
        assert_eq!(
            file.computer_use,
            Some(RpcComputerUseConfig {
                enabled: true,
                desktop_scope: RpcComputerUseDesktopScope::VisibleDesktop,
                grant_ttl_ms: 45_000,
                stdio_observe: true,
                http_observe: false,
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
    fn computer_use_grant_ttl_is_bounded_and_default_denied() {
        assert!(!RpcComputerUseConfig::default().enabled);
        assert!(
            RpcComputerUseConfig {
                grant_ttl_ms: 0,
                ..RpcComputerUseConfig::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            RpcComputerUseConfig {
                grant_ttl_ms: 900_001,
                ..RpcComputerUseConfig::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn rpc_config_source_is_bounded_and_requires_a_regular_file() {
        let temp = tempfile::tempdir().unwrap();
        let missing = read_file_config(&temp.path().join("missing.toml")).unwrap();
        assert!(missing.server.is_none());
        assert!(missing.profiles.is_none());

        let oversized = temp.path().join("oversized.toml");
        fs::write(
            &oversized,
            vec![b' '; usize::try_from(MAX_RPC_CONFIG_BYTES + 1).unwrap()],
        )
        .unwrap();
        assert!(matches!(
            read_file_config(&oversized),
            Err(RpcHostError::Invalid(message)) if message.contains("byte limit")
        ));
        assert!(matches!(
            read_file_config(temp.path()),
            Err(RpcHostError::Invalid(message)) if message.contains("regular file")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rpc_config_source_rejects_symbolic_links() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target.toml");
        let link = temp.path().join("rpc.toml");
        fs::write(&target, "[server]\ndefault_profile = \"default\"\n").unwrap();
        symlink(&target, &link).unwrap();
        assert!(matches!(
            read_file_config(&link),
            Err(RpcHostError::Invalid(message)) if message.contains("symlink")
        ));
    }

    #[test]
    fn supervised_launch_is_closed_absolute_and_does_not_resolve_private_config() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("sessions.sqlite");
        let state = temp.path().join("state");
        let envelope_path = temp.path().join("launch.json");
        let envelope = serde_json::json!({
            "schema": {"name": "starweaver.rpc.launch", "version": 1},
            "mode": "workspace_execution",
            "database": {"identity": "database-local", "path": database},
            "stateDirectory": state,
            "executionDomainId": "local-user",
            "configurationGeneration": "7",
            "defaultProfile": "desktop",
            "profiles": [{
                "name": "desktop",
                "modelId": "local_echo",
                "instructions": [],
                "toolsets": ["filesystem"]
            }],
            "providers": [],
            "capabilityCaps": {
                "hitl": true,
                "clarifyingQuestions": true,
                "nativeLocalShell": false
            }
        });
        fs::write(&envelope_path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        fs::write(temp.path().join("rpc.toml"), "not valid toml = [").unwrap();

        let config = RpcConfig::from_launch_envelope(&envelope_path).unwrap();
        assert_eq!(config.database_path, database);
        assert_eq!(config.initial_workspace_root, None);
        assert_eq!(config.state_dir, state);
        assert_eq!(config.launch.configuration_generation, 7);
        assert_eq!(config.launch.mode, "workspace_execution");
        assert_eq!(config.default_profile, "desktop");
        assert!(config.client_capabilities.clarifying_questions);
        assert_eq!(config.computer_use, RpcComputerUseConfig::default());
        assert!(config.launch.envelope_digest.starts_with("sha256:"));

        let mut open = envelope.clone();
        open["privateFallback"] = serde_json::json!("rpc.toml");
        fs::write(&envelope_path, serde_json::to_vec(&open).unwrap()).unwrap();
        assert!(RpcConfig::from_launch_envelope(&envelope_path).is_err());

        let mut relative = envelope;
        relative["database"]["path"] = serde_json::json!("relative.sqlite");
        fs::write(&envelope_path, serde_json::to_vec(&relative).unwrap()).unwrap();
        assert!(RpcConfig::from_launch_envelope(&envelope_path).is_err());
    }

    #[test]
    fn supervised_launch_denies_native_shell_and_duplicate_authority_names() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("launch.json");
        let mut envelope = serde_json::json!({
            "schema": {"name": "starweaver.rpc.launch", "version": 1},
            "mode": "workspace_execution",
            "database": {"identity": "database-local", "path": temp.path().join("db.sqlite")},
            "stateDirectory": temp.path().join("state"),
            "executionDomainId": "local-user",
            "configurationGeneration": "1",
            "defaultProfile": "desktop",
            "profiles": [{
                "name": "desktop",
                "modelId": "local_echo",
                "instructions": [],
                "toolsets": []
            }],
            "providers": [],
            "capabilityCaps": {
                "hitl": false,
                "clarifyingQuestions": false,
                "nativeLocalShell": true
            }
        });
        fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(RpcConfig::from_launch_envelope(&path).is_err());

        envelope["capabilityCaps"]["nativeLocalShell"] = serde_json::json!(false);
        envelope["profiles"][0]["toolsets"] = serde_json::json!(["shell"]);
        fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(RpcConfig::from_launch_envelope(&path).is_err());

        envelope["profiles"] = serde_json::json!([
            {"name":"desktop","modelId":"local_echo","instructions":[],"toolsets":[]},
            {"name":"desktop","modelId":"other","instructions":[],"toolsets":[]}
        ]);
        fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(RpcConfig::from_launch_envelope(&path).is_err());
    }

    #[test]
    fn resolves_mcp_config_sources_against_their_own_base() {
        let current_dir = Path::new("/process/workspace");
        let config_dir = Path::new("/config/root");
        assert_eq!(
            resolve_mcp_config_path(
                current_dir,
                config_dir,
                None,
                Some(PathBuf::from("mcp.json")),
            ),
            Some(config_dir.join("mcp.json"))
        );
        assert_eq!(
            resolve_mcp_config_path(
                current_dir,
                config_dir,
                Some(PathBuf::from("override.json")),
                Some(PathBuf::from("mcp.json")),
            ),
            Some(current_dir.join("override.json"))
        );
    }

    #[test]
    fn loads_mcp_config_relative_to_its_own_directory() {
        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path().join("config");
        fs::create_dir_all(&config_dir).unwrap();
        let path = config_dir.join("mcp.json");
        fs::write(
            &path,
            r#"{"servers":{"local":{"command":"server","cwd":"workspace"}}}"#,
        )
        .unwrap();

        let servers = load_mcp_servers(&path).unwrap();
        assert_eq!(
            servers["local"].cwd.as_deref(),
            Some(config_dir.join("workspace").to_string_lossy().as_ref())
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

    #[test]
    fn configured_environment_catalog_is_public_id_keyed_and_resource_allowlisted() {
        let file: FileConfig = toml::from_str(
            r#"
[environments.dataset]
kind = "envd"
display_name = "Research dataset"
endpoint_ref = "http://127.0.0.1:45879/envd"
environment_id = "private-envd-id"
auth_token_env = "PRIVATE_ENVD_TOKEN"

[environments.dataset.resources.reports]
label = "Quarterly reports"
source_ref = "private://bucket/path?credential=secret"
"#,
        )
        .unwrap();
        let environments = resolve_environment_catalog(file.environments.unwrap()).unwrap();
        assert_eq!(
            environments.keys().cloned().collect::<Vec<_>>(),
            vec!["dataset", "local"]
        );
        assert_eq!(
            environments["dataset"].resources["reports"].label,
            "Quarterly reports"
        );
        assert_eq!(
            environments["dataset"].resources["reports"].source_ref,
            "private://bucket/path?credential=secret"
        );
        let public = environments
            .iter()
            .map(|(id, entry)| RpcEnvironmentCatalogEntry {
                environment_id: id.clone(),
                display_name: entry.display_name.clone(),
            })
            .collect::<Vec<_>>();
        let public_debug = format!("{public:?}");
        assert!(!public_debug.contains("45879"));
        assert!(!public_debug.contains("credential"));
        assert!(!public_debug.contains("PRIVATE_ENVD_TOKEN"));
        let private_config_debug = format!("{:?}", environments["dataset"]);
        assert!(!private_config_debug.contains("45879"));
        assert!(!private_config_debug.contains("private-envd-id"));
        assert!(!private_config_debug.contains("PRIVATE_ENVD_TOKEN"));
        assert!(!private_config_debug.contains("bucket/path"));
    }

    #[test]
    fn local_environment_is_reserved_and_has_no_configurable_root() {
        let configured = BTreeMap::from([(
            "other-local".to_string(),
            RpcEnvironmentConfig {
                display_name: None,
                source: RpcEnvironmentSourceConfig::Local,
                resources: BTreeMap::new(),
            },
        )]);
        let error = resolve_environment_catalog(configured).unwrap_err();
        assert!(error.to_string().contains("reserved local provider"));

        let catalog = resolve_environment_catalog(BTreeMap::new()).unwrap();
        assert_eq!(catalog.len(), 1);
        assert!(matches!(
            catalog["local"].source,
            RpcEnvironmentSourceConfig::Local
        ));

        let local = RpcEnvironmentConfig {
            display_name: Some("Workspace".to_string()),
            source: RpcEnvironmentSourceConfig::Local,
            resources: BTreeMap::from([(
                "reports".to_string(),
                RpcEnvironmentResourceConfig {
                    label: "Workspace reports".to_string(),
                    source_ref: "data/reports".to_string(),
                },
            )]),
        };
        let local_catalog =
            resolve_environment_catalog(BTreeMap::from([("local".to_string(), local)])).unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut config = RpcConfig::for_tests(root.path());
        config.environments = local_catalog;
        let resolved = config
            .resolve_environment_resource(
                "local",
                "reports",
                Some(config.workspace_root_for_tests()),
            )
            .unwrap();
        assert_eq!(resolved.label, "Workspace reports");
        assert_eq!(
            PathBuf::from(resolved.source_ref),
            root.path().join("workspace/data/reports")
        );

        let escaping = RpcEnvironmentConfig {
            display_name: None,
            source: RpcEnvironmentSourceConfig::Local,
            resources: BTreeMap::from([(
                "escape".to_string(),
                RpcEnvironmentResourceConfig {
                    label: "Escape".to_string(),
                    source_ref: "../private".to_string(),
                },
            )]),
        };
        assert!(
            resolve_environment_catalog(BTreeMap::from([("local".to_string(), escaping)])).is_err()
        );
    }

    #[test]
    fn invalid_generated_environment_identity_and_unknown_resource_are_rejected() {
        let too_long = "x".repeat(129);
        let configured = BTreeMap::from([(
            too_long,
            RpcEnvironmentConfig {
                display_name: None,
                source: RpcEnvironmentSourceConfig::Envd {
                    endpoint_ref: "unix:///tmp/envd.sock".to_string(),
                    environment_id: None,
                    auth_token_env: None,
                },
                resources: BTreeMap::new(),
            },
        )]);
        assert!(resolve_environment_catalog(configured).is_err());

        let root = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(root.path());
        let error = config
            .resolve_environment_resource(
                "local",
                "unconfigured/path",
                Some(config.workspace_root_for_tests()),
            )
            .unwrap_err();
        assert!(error.to_string().contains("not allowlisted"));
        assert_eq!(
            config.resolve_environment_source("local").unwrap(),
            ResolvedRpcEnvironmentSource::Local
        );
    }
}
