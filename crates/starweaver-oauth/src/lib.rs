#![allow(clippy::missing_errors_doc)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::must_use_candidate)]
//! OAuth credential storage and Codex OAuth helpers for Starweaver.
//!
//! This crate is the Rust migration of the reference `ya-oauth` package. It keeps
//! the OAuth auth file under `~/.starweaver/auth.json` by default and exposes a
//! store-backed token source for OAuth-backed model providers.

use std::{
    collections::BTreeMap,
    env,
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE, Engine as _};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

/// Codex OAuth issuer.
pub const CODEX_ISSUER: &str = "https://auth.openai.com";
/// Codex OAuth public client id used by `OpenAI` Codex.
pub const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// Codex OAuth token endpoint.
pub const CODEX_TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
/// Codex OAuth revoke endpoint.
pub const CODEX_REVOKE_ENDPOINT: &str = "https://auth.openai.com/oauth/revoke";
/// Codex model endpoint base URL.
pub const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
/// Codex device auth redirect URI.
pub const CODEX_DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
/// Codex OAuth scopes.
pub const CODEX_SCOPES: &[&str] = &[
    "openid",
    "profile",
    "email",
    "offline_access",
    "api.connectors.read",
    "api.connectors.invoke",
];

/// Environment variable overriding the OAuth auth file path.
pub const STARWEAVER_OAUTH_AUTH_FILE_ENV: &str = "STARWEAVER_OAUTH_AUTH_FILE";

/// OAuth result alias.
pub type OAuthResult<T> = Result<T, OAuthError>;

/// OAuth storage and provider error.
#[derive(Debug, Error)]
pub enum OAuthError {
    /// Filesystem operation failed.
    #[error("filesystem error at {}: {source}", path.display())]
    Io {
        /// Path involved in the failure.
        path: PathBuf,
        /// Source IO error.
        #[source]
        source: std::io::Error,
    },
    /// JSON serialization failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// HTTP transport failed.
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    /// Provider returned a non-success status.
    #[error("provider status {status}: {body}")]
    ProviderStatus {
        /// HTTP status code.
        status: u16,
        /// Provider response body.
        body: Value,
    },
    /// Provider record is missing from the auth store.
    #[error("OAuth provider is not logged in: {provider}")]
    NotLoggedIn {
        /// Provider name.
        provider: String,
    },
    /// Refresh token is missing from the provider record.
    #[error("OAuth provider {provider} is missing a refresh token")]
    MissingRefreshToken {
        /// Provider name.
        provider: String,
    },
    /// OAuth response did not include a required field.
    #[error("invalid OAuth response: {0}")]
    InvalidResponse(String),
    /// JWT payload decoding failed.
    #[error("invalid JWT payload: {0}")]
    InvalidJwt(String),
    /// Refresh returned a token for a different account.
    #[error("Codex refresh returned a different account; log in again")]
    AccountMismatch,
    /// Device authorization timed out.
    #[error("Codex device authorization timed out")]
    DeviceAuthorizationTimeout,
}

fn io_error(path: impl Into<PathBuf>, source: std::io::Error) -> OAuthError {
    OAuthError::Io {
        path: path.into(),
        source,
    }
}

/// OAuth token material stored for a provider.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OAuthTokens {
    /// Optional provider identity token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
    /// Access token used as bearer credential.
    pub access_token: String,
    /// Optional refresh token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
}

/// Account metadata derived from provider tokens.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OAuthAccount {
    /// Account email when present in the identity token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// `ChatGPT` user id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chatgpt_user_id: Option<String>,
    /// `ChatGPT` account id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chatgpt_account_id: Option<String>,
    /// `ChatGPT` plan type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chatgpt_plan_type: Option<String>,
    /// Whether the account is `FedRAMP`.
    #[serde(default)]
    pub chatgpt_account_is_fedramp: bool,
}

/// Stored OAuth configuration and credential record for one provider.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OAuthProviderRecord {
    /// OAuth provider record type.
    #[serde(rename = "type", default = "default_oauth_type")]
    pub provider_type: String,
    /// OAuth issuer.
    pub issuer: String,
    /// OAuth client id.
    pub client_id: String,
    /// Token endpoint.
    pub token_endpoint: String,
    /// Optional revoke endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoke_endpoint: Option<String>,
    /// Optional provider API base URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// OAuth scopes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    /// Provider tokens.
    pub tokens: OAuthTokens,
    /// Provider account metadata.
    #[serde(default)]
    pub account: OAuthAccount,
    /// Last successful refresh time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_refresh_at: Option<DateTime<Utc>>,
}

impl OAuthProviderRecord {
    /// Return an updated copy, preserving refresh response fields providers omit.
    #[must_use]
    pub fn with_refreshed_tokens(
        &self,
        id_token: Option<String>,
        access_token: Option<String>,
        refresh_token: Option<String>,
        account: Option<OAuthAccount>,
    ) -> Self {
        Self {
            provider_type: self.provider_type.clone(),
            issuer: self.issuer.clone(),
            client_id: self.client_id.clone(),
            token_endpoint: self.token_endpoint.clone(),
            revoke_endpoint: self.revoke_endpoint.clone(),
            base_url: self.base_url.clone(),
            scopes: self.scopes.clone(),
            tokens: OAuthTokens {
                id_token: id_token.or_else(|| self.tokens.id_token.clone()),
                access_token: access_token.unwrap_or_else(|| self.tokens.access_token.clone()),
                refresh_token: refresh_token.or_else(|| self.tokens.refresh_token.clone()),
            },
            account: account.unwrap_or_else(|| self.account.clone()),
            last_refresh_at: Some(Utc::now()),
        }
    }

    /// Return redacted record data safe for diagnostics.
    pub fn redacted_value(&self) -> Value {
        let mut value = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Some(tokens) = value.get_mut("tokens").and_then(Value::as_object_mut) {
            for token in tokens.values_mut() {
                if !token.is_null() {
                    *token = Value::String("<redacted>".to_string());
                }
            }
        }
        value
    }

    /// Return a compact status value without token material.
    pub fn status_value(&self) -> Value {
        json!({
            "issuer": self.issuer,
            "client_id": self.client_id,
            "token_endpoint": self.token_endpoint,
            "revoke_endpoint": self.revoke_endpoint,
            "base_url": self.base_url,
            "scopes": self.scopes,
            "account": self.account,
            "has_access_token": !self.tokens.access_token.trim().is_empty(),
            "has_refresh_token": self.tokens.refresh_token.as_ref().is_some_and(|token| !token.trim().is_empty()),
            "last_refresh_at": self.last_refresh_at,
        })
    }
}

fn default_oauth_type() -> String {
    "oauth2".to_string()
}

/// On-disk auth file schema.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AuthFile {
    /// Schema version.
    #[serde(default = "default_auth_version")]
    pub version: u32,
    /// Provider records keyed by provider name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, OAuthProviderRecord>,
}

impl Default for AuthFile {
    fn default() -> Self {
        Self {
            version: default_auth_version(),
            providers: BTreeMap::new(),
        }
    }
}

const fn default_auth_version() -> u32 {
    1
}

/// Provider token state safe for request construction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TokenSnapshot {
    /// Provider name.
    pub provider_name: String,
    /// Access token.
    pub access_token: String,
    /// Account metadata.
    #[serde(default)]
    pub account: OAuthAccount,
    /// Optional provider base URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Additional provider metadata.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
}

/// Async token source consumed by OAuth-backed model providers.
#[async_trait]
pub trait OAuthTokenSource: Send + Sync {
    /// Return the current token snapshot.
    async fn get_token(&self) -> OAuthResult<TokenSnapshot>;

    /// Refresh provider credentials and return the refreshed snapshot.
    async fn refresh_token(&self) -> OAuthResult<TokenSnapshot>;
}

/// Provider-specific record refresher used by store-backed token sources.
#[async_trait]
pub trait OAuthProviderRefresher: Send + Sync {
    /// Refresh a provider record.
    async fn refresh_provider(
        &self,
        record: &OAuthProviderRecord,
    ) -> OAuthResult<OAuthProviderRecord>;
}

/// File-backed OAuth credential store with process-level locking.
#[derive(Clone, Debug)]
pub struct OAuthStore {
    path: PathBuf,
    lock_path: PathBuf,
}

impl OAuthStore {
    /// Create a store at an explicit path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let lock_path = path.with_extension(
            path.extension()
                .and_then(|extension| extension.to_str())
                .map_or_else(
                    || "lock".to_string(),
                    |extension| format!("{extension}.lock"),
                ),
        );
        Self { path, lock_path }
    }

    /// Create a store at the default Starweaver auth path.
    pub fn default_store() -> Self {
        Self::new(default_auth_path())
    }

    /// Return the default auth path.
    pub fn default_path() -> PathBuf {
        default_auth_path()
    }

    /// Return the backing auth file path.
    pub const fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Return the backing lock file path.
    pub const fn lock_path(&self) -> &PathBuf {
        &self.lock_path
    }

    /// Load the full auth file.
    pub fn load(&self) -> OAuthResult<AuthFile> {
        self.ensure_parent()?;
        let _lock = FileLock::acquire(&self.lock_path)?;
        self.load_unlocked()
    }

    /// Save the full auth file.
    pub fn save(&self, auth_file: &AuthFile) -> OAuthResult<()> {
        self.ensure_parent()?;
        let _lock = FileLock::acquire(&self.lock_path)?;
        self.save_unlocked(auth_file)
    }

    /// Load one provider record.
    pub fn get_provider(&self, provider_name: &str) -> OAuthResult<Option<OAuthProviderRecord>> {
        Ok(self.load()?.providers.get(provider_name).cloned())
    }

    /// Compatibility alias for CLI callers.
    pub fn load_provider(&self, provider_name: &str) -> OAuthResult<Option<OAuthProviderRecord>> {
        self.get_provider(provider_name)
    }

    /// Save one provider record.
    pub fn set_provider(
        &self,
        provider_name: &str,
        record: OAuthProviderRecord,
    ) -> OAuthResult<()> {
        self.update(|auth_file| {
            auth_file
                .providers
                .insert(provider_name.to_string(), record);
            Ok(())
        })
    }

    /// Compatibility alias for CLI callers.
    pub fn save_provider(
        &self,
        provider_name: &str,
        record: OAuthProviderRecord,
    ) -> OAuthResult<()> {
        self.set_provider(provider_name, record)
    }

    /// Delete one provider record and return the deleted record.
    pub fn delete_provider(&self, provider_name: &str) -> OAuthResult<Option<OAuthProviderRecord>> {
        self.update(|auth_file| Ok(auth_file.providers.remove(provider_name)))
    }

    /// Remove one provider record and return whether it existed.
    pub fn remove_provider(&self, provider_name: &str) -> OAuthResult<bool> {
        Ok(self.delete_provider(provider_name)?.is_some())
    }

    /// Update the auth file while holding the store lock.
    pub fn update<T>(
        &self,
        updater: impl FnOnce(&mut AuthFile) -> OAuthResult<T>,
    ) -> OAuthResult<T> {
        self.ensure_parent()?;
        let _lock = FileLock::acquire(&self.lock_path)?;
        let mut auth_file = self.load_unlocked()?;
        let result = updater(&mut auth_file)?;
        self.save_unlocked(&auth_file)?;
        Ok(result)
    }

    fn ensure_parent(&self) -> OAuthResult<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
            set_dir_private(parent);
        }
        Ok(())
    }

    fn load_unlocked(&self) -> OAuthResult<AuthFile> {
        if !self.path.exists() {
            return Ok(AuthFile::default());
        }
        set_file_private(&self.path);
        let mut content = String::new();
        File::open(&self.path)
            .map_err(|error| io_error(&self.path, error))?
            .read_to_string(&mut content)
            .map_err(|error| io_error(&self.path, error))?;
        Ok(serde_json::from_str(&content)?)
    }

    fn save_unlocked(&self, auth_file: &AuthFile) -> OAuthResult<()> {
        let temp_path = self.path.with_extension("json.tmp");
        {
            let mut file = File::create(&temp_path).map_err(|error| io_error(&temp_path, error))?;
            set_file_private(&temp_path);
            serde_json::to_writer_pretty(&mut file, auth_file)?;
            file.write_all(b"\n")
                .map_err(|error| io_error(&temp_path, error))?;
            file.sync_all()
                .map_err(|error| io_error(&temp_path, error))?;
        }
        fs::rename(&temp_path, &self.path).map_err(|error| io_error(&self.path, error))?;
        set_file_private(&self.path);
        Ok(())
    }
}

impl Default for OAuthStore {
    fn default() -> Self {
        Self::default_store()
    }
}

struct FileLock {
    file: File,
}

impl FileLock {
    fn acquire(path: &Path) -> OAuthResult<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|error| io_error(path, error))?;
        file.lock_exclusive()
            .map_err(|error| io_error(path, error))?;
        Ok(Self { file })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[cfg(unix)]
fn set_dir_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn set_dir_private(_path: &Path) {}

#[cfg(unix)]
fn set_file_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_file_private(_path: &Path) {}

/// Return the default auth directory under `~/.starweaver`.
pub fn default_auth_dir() -> PathBuf {
    env::var_os("HOME")
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join(".starweaver")
}

/// Return the default auth file path under `~/.starweaver/auth.json`.
pub fn default_auth_path() -> PathBuf {
    env::var_os(STARWEAVER_OAUTH_AUTH_FILE_ENV)
        .map_or_else(|| default_auth_dir().join("auth.json"), PathBuf::from)
}

/// Token source backed by `OAuthStore` and a provider-specific refresher.
#[derive(Clone)]
pub struct StoreBackedTokenSource {
    provider_name: String,
    store: OAuthStore,
    refresh_provider: Arc<dyn OAuthProviderRefresher>,
}

impl StoreBackedTokenSource {
    /// Create a store-backed token source.
    pub fn new(
        provider_name: impl Into<String>,
        store: OAuthStore,
        refresh_provider: Arc<dyn OAuthProviderRefresher>,
    ) -> Self {
        Self {
            provider_name: provider_name.into(),
            store,
            refresh_provider,
        }
    }

    /// Return the provider name.
    pub fn provider_name(&self) -> &str {
        &self.provider_name
    }

    /// Return the backing store.
    pub const fn store(&self) -> &OAuthStore {
        &self.store
    }
}

#[async_trait]
impl OAuthTokenSource for StoreBackedTokenSource {
    async fn get_token(&self) -> OAuthResult<TokenSnapshot> {
        let record = self
            .store
            .get_provider(&self.provider_name)?
            .ok_or_else(|| OAuthError::NotLoggedIn {
                provider: self.provider_name.clone(),
            })?;
        Ok(snapshot_from_record(&self.provider_name, &record))
    }

    async fn refresh_token(&self) -> OAuthResult<TokenSnapshot> {
        let record = self
            .store
            .get_provider(&self.provider_name)?
            .ok_or_else(|| OAuthError::NotLoggedIn {
                provider: self.provider_name.clone(),
            })?;
        let refreshed = self.refresh_provider.refresh_provider(&record).await?;
        self.store
            .set_provider(&self.provider_name, refreshed.clone())?;
        Ok(snapshot_from_record(&self.provider_name, &refreshed))
    }
}

fn snapshot_from_record(provider_name: &str, record: &OAuthProviderRecord) -> TokenSnapshot {
    TokenSnapshot {
        provider_name: provider_name.to_string(),
        access_token: record.tokens.access_token.clone(),
        account: record.account.clone(),
        base_url: record.base_url.clone(),
        metadata: BTreeMap::new(),
    }
}

/// Codex OAuth profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexOAuthProfile {
    /// OAuth issuer.
    pub issuer: String,
    /// OAuth client id.
    pub client_id: String,
    /// OAuth token endpoint.
    pub token_endpoint: String,
    /// OAuth revoke endpoint.
    pub revoke_endpoint: String,
    /// Codex API base URL.
    pub base_url: String,
    /// OAuth scopes.
    pub scopes: Vec<String>,
}

impl CodexOAuthProfile {
    /// Return the device user-code endpoint.
    pub fn device_user_code_endpoint(&self) -> String {
        format!(
            "{}/api/accounts/deviceauth/usercode",
            self.issuer.trim_end_matches('/')
        )
    }

    /// Return the device token endpoint.
    pub fn device_token_endpoint(&self) -> String {
        format!(
            "{}/api/accounts/deviceauth/token",
            self.issuer.trim_end_matches('/')
        )
    }

    /// Return the browser verification URL.
    pub fn verification_url(&self) -> String {
        format!("{}/codex/device", self.issuer.trim_end_matches('/'))
    }

    /// Return the device redirect URI.
    pub fn device_redirect_uri(&self) -> String {
        format!("{}/deviceauth/callback", self.issuer.trim_end_matches('/'))
    }
}

impl Default for CodexOAuthProfile {
    fn default() -> Self {
        Self {
            issuer: CODEX_ISSUER.to_string(),
            client_id: CODEX_CLIENT_ID.to_string(),
            token_endpoint: CODEX_TOKEN_ENDPOINT.to_string(),
            revoke_endpoint: CODEX_REVOKE_ENDPOINT.to_string(),
            base_url: CODEX_BASE_URL.to_string(),
            scopes: CODEX_SCOPES.iter().map(ToString::to_string).collect(),
        }
    }
}

/// Codex device authorization data shown to users during login.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeviceCode {
    /// Browser verification URL.
    pub verification_url: String,
    /// One-time user code.
    pub user_code: String,
    /// Device authorization id.
    pub device_auth_id: String,
    /// Suggested polling interval in seconds.
    pub interval: u64,
}

#[derive(Debug, Deserialize)]
struct UserCodeResponse {
    device_auth_id: String,
    #[serde(alias = "usercode")]
    user_code: String,
    #[serde(default)]
    interval: Option<Value>,
}

impl UserCodeResponse {
    fn into_device_code(self, profile: &CodexOAuthProfile) -> OAuthResult<DeviceCode> {
        Ok(DeviceCode {
            verification_url: profile.verification_url(),
            user_code: self.user_code,
            device_auth_id: self.device_auth_id,
            interval: parse_interval(self.interval)?,
        })
    }
}

fn parse_interval(value: Option<Value>) -> OAuthResult<u64> {
    match value {
        None => Ok(5),
        Some(Value::Number(number)) => number.as_u64().ok_or_else(|| {
            OAuthError::InvalidResponse("invalid device polling interval".to_string())
        }),
        Some(Value::String(text)) => text.parse::<u64>().map_err(|error| {
            OAuthError::InvalidResponse(format!("invalid device polling interval: {error}"))
        }),
        Some(_) => Err(OAuthError::InvalidResponse(
            "invalid device polling interval".to_string(),
        )),
    }
}

/// Device token response returned after browser authorization.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeviceTokenResponse {
    /// Authorization code.
    pub authorization_code: String,
    /// PKCE code challenge.
    pub code_challenge: String,
    /// PKCE code verifier.
    pub code_verifier: String,
}

#[allow(clippy::struct_field_names)]
#[derive(Debug, Deserialize)]
struct TokenResponse {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
}

/// Codex OAuth device-code login and refresh client.
#[derive(Clone, Debug)]
pub struct CodexOAuthClient {
    profile: CodexOAuthProfile,
    store: OAuthStore,
    http_client: reqwest::Client,
}

impl CodexOAuthClient {
    /// Create a Codex OAuth client using the default store and HTTP client.
    pub fn new() -> OAuthResult<Self> {
        Self::with_store(OAuthStore::default_store())
    }

    /// Create a Codex OAuth client with an explicit store.
    pub fn with_store(store: OAuthStore) -> OAuthResult<Self> {
        Self::with_profile_and_store(CodexOAuthProfile::default(), store)
    }

    /// Create a Codex OAuth client with an explicit profile and store.
    pub fn with_profile_and_store(
        profile: CodexOAuthProfile,
        store: OAuthStore,
    ) -> OAuthResult<Self> {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            profile,
            store,
            http_client,
        })
    }

    /// Create a Codex OAuth client with an injected reqwest client.
    #[must_use]
    pub const fn with_http_client(
        profile: CodexOAuthProfile,
        store: OAuthStore,
        http_client: reqwest::Client,
    ) -> Self {
        Self {
            profile,
            store,
            http_client,
        }
    }

    /// Return the Codex OAuth profile.
    pub const fn profile(&self) -> &CodexOAuthProfile {
        &self.profile
    }

    /// Return the backing OAuth store.
    pub const fn store(&self) -> &OAuthStore {
        &self.store
    }

    /// Request a device code and user code for browser login.
    pub async fn request_device_code(&self) -> OAuthResult<DeviceCode> {
        let response = self
            .http_client
            .post(self.profile.device_user_code_endpoint())
            .json(&json!({ "client_id": self.profile.client_id }))
            .send()
            .await?;
        response_json::<UserCodeResponse>(response)
            .await?
            .into_device_code(&self.profile)
    }

    /// Poll until browser authorization returns a device token response.
    pub async fn poll_device_token(
        &self,
        device_code: &DeviceCode,
        timeout_seconds: u64,
    ) -> OAuthResult<DeviceTokenResponse> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_seconds);
        loop {
            let response = self
                .http_client
                .post(self.profile.device_token_endpoint())
                .json(&json!({
                    "device_auth_id": device_code.device_auth_id,
                    "user_code": device_code.user_code,
                }))
                .send()
                .await?;
            if response.status().is_success() {
                return response_json::<DeviceTokenResponse>(response).await;
            }
            let status = response.status().as_u16();
            if matches!(status, 403 | 404) && tokio::time::Instant::now() < deadline {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                let sleep_for = Duration::from_secs(device_code.interval).min(remaining);
                tokio::time::sleep(sleep_for).await;
                continue;
            }
            if matches!(status, 403 | 404) {
                return Err(OAuthError::DeviceAuthorizationTimeout);
            }
            return provider_status_error(response).await;
        }
    }

    /// Exchange a device authorization code for OAuth tokens.
    pub async fn exchange_device_code(
        &self,
        code_response: &DeviceTokenResponse,
    ) -> OAuthResult<OAuthProviderRecord> {
        let response = self
            .http_client
            .post(&self.profile.token_endpoint)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code_response.authorization_code.as_str()),
                ("redirect_uri", self.profile.device_redirect_uri().as_str()),
                ("client_id", self.profile.client_id.as_str()),
                ("code_verifier", code_response.code_verifier.as_str()),
            ])
            .send()
            .await?;
        let token_response = response_json::<TokenResponse>(response).await?;
        self.record_from_token_response(token_response)
    }

    /// Complete the device-code login flow and return the device code plus provider record.
    pub async fn login_device_code(
        &self,
        timeout_seconds: u64,
    ) -> OAuthResult<(DeviceCode, OAuthProviderRecord)> {
        let device_code = self.request_device_code().await?;
        let token_code = self
            .poll_device_token(&device_code, timeout_seconds)
            .await?;
        let record = self.exchange_device_code(&token_code).await?;
        Ok((device_code, record))
    }

    /// Refresh one Codex provider record.
    pub async fn refresh_record(
        &self,
        record: &OAuthProviderRecord,
    ) -> OAuthResult<OAuthProviderRecord> {
        let refresh_token = record.tokens.refresh_token.as_ref().ok_or_else(|| {
            OAuthError::MissingRefreshToken {
                provider: "codex".to_string(),
            }
        })?;
        let response = self
            .http_client
            .post(&self.profile.token_endpoint)
            .json(&json!({
                "client_id": self.profile.client_id,
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
            }))
            .send()
            .await?;
        let token_response = response_json::<TokenResponse>(response).await?;
        let account = if let Some(id_token) = token_response.id_token.as_deref() {
            account_from_id_token(id_token)?
        } else {
            record.account.clone()
        };
        validate_same_account(&record.account, &account)?;
        Ok(record.with_refreshed_tokens(
            token_response.id_token,
            token_response.access_token,
            token_response.refresh_token,
            Some(account),
        ))
    }

    /// Revoke the refresh token or access token for one provider record.
    pub async fn revoke_record(&self, record: &OAuthProviderRecord) -> OAuthResult<()> {
        let Some(endpoint) = record.revoke_endpoint.as_deref() else {
            return Ok(());
        };
        let Some(token) = record
            .tokens
            .refresh_token
            .as_deref()
            .or(Some(record.tokens.access_token.as_str()))
        else {
            return Ok(());
        };
        let response = self
            .http_client
            .post(endpoint)
            .form(&[
                ("client_id", self.profile.client_id.as_str()),
                ("token", token),
            ])
            .send()
            .await?;
        if response.status().as_u16() < 400 {
            Ok(())
        } else {
            provider_status_error(response).await
        }
    }

    /// Build a token source backed by this client's store and refresh flow.
    pub fn make_token_source(&self) -> StoreBackedTokenSource {
        StoreBackedTokenSource::new("codex", self.store.clone(), Arc::new(self.clone()))
    }

    fn record_from_token_response(
        &self,
        token_response: TokenResponse,
    ) -> OAuthResult<OAuthProviderRecord> {
        let access_token = token_response.access_token.ok_or_else(|| {
            OAuthError::InvalidResponse(
                "Codex token response did not include access_token".to_string(),
            )
        })?;
        let account = if let Some(id_token) = token_response.id_token.as_deref() {
            account_from_id_token(id_token)?
        } else {
            OAuthAccount::default()
        };
        Ok(OAuthProviderRecord {
            provider_type: default_oauth_type(),
            issuer: self.profile.issuer.clone(),
            client_id: self.profile.client_id.clone(),
            token_endpoint: self.profile.token_endpoint.clone(),
            revoke_endpoint: Some(self.profile.revoke_endpoint.clone()),
            base_url: Some(self.profile.base_url.clone()),
            scopes: self.profile.scopes.clone(),
            tokens: OAuthTokens {
                id_token: token_response.id_token,
                access_token,
                refresh_token: token_response.refresh_token,
            },
            account,
            last_refresh_at: Some(Utc::now()),
        })
    }
}

#[async_trait]
impl OAuthProviderRefresher for CodexOAuthClient {
    async fn refresh_provider(
        &self,
        record: &OAuthProviderRecord,
    ) -> OAuthResult<OAuthProviderRecord> {
        self.refresh_record(record).await
    }
}

/// Build a Codex token source backed by the default or provided store.
pub fn create_codex_token_source(store: Option<OAuthStore>) -> OAuthResult<StoreBackedTokenSource> {
    Ok(CodexOAuthClient::with_store(store.unwrap_or_default())?.make_token_source())
}

async fn response_json<T: DeserializeOwned>(response: reqwest::Response) -> OAuthResult<T> {
    if response.status().is_success() {
        Ok(response.json::<T>().await?)
    } else {
        provider_status_error(response).await
    }
}

async fn provider_status_error<T>(response: reqwest::Response) -> OAuthResult<T> {
    let status = response.status().as_u16();
    let text = response.text().await?;
    let body = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
    Err(OAuthError::ProviderStatus { status, body })
}

/// Decode a JWT payload without signature validation for local metadata extraction.
pub fn decode_jwt_payload(jwt: &str) -> OAuthResult<Value> {
    let parts = jwt.split('.').collect::<Vec<_>>();
    if parts.len() != 3 || parts[1].is_empty() {
        return Err(OAuthError::InvalidJwt("invalid JWT format".to_string()));
    }
    let mut payload = parts[1].to_string();
    payload.push_str(&"=".repeat((4 - payload.len() % 4) % 4));
    let decoded = URL_SAFE
        .decode(payload.as_bytes())
        .map_err(|error| OAuthError::InvalidJwt(error.to_string()))?;
    let value = serde_json::from_slice::<Value>(&decoded)?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(OAuthError::InvalidJwt(
            "JWT payload is not an object".to_string(),
        ))
    }
}

/// Extract Codex-compatible `ChatGPT` account metadata from an ID token.
pub fn account_from_id_token(id_token: &str) -> OAuthResult<OAuthAccount> {
    let claims = decode_jwt_payload(id_token)?;
    let profile_data = claims
        .get("https://api.openai.com/profile")
        .and_then(Value::as_object);
    let auth_data = claims
        .get("https://api.openai.com/auth")
        .and_then(Value::as_object);
    Ok(OAuthAccount {
        email: string_claim_value(&claims, "email")
            .or_else(|| string_claim_map(profile_data, "email")),
        chatgpt_user_id: string_claim_map(auth_data, "chatgpt_user_id")
            .or_else(|| string_claim_map(auth_data, "user_id")),
        chatgpt_account_id: string_claim_map(auth_data, "chatgpt_account_id"),
        chatgpt_plan_type: plan_type_claim(
            auth_data.and_then(|object| object.get("chatgpt_plan_type")),
        ),
        chatgpt_account_is_fedramp: auth_data
            .and_then(|object| object.get("chatgpt_account_is_fedramp"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn string_claim_value(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

fn string_claim_map(value: Option<&serde_json::Map<String, Value>>, key: &str) -> Option<String> {
    value
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

fn plan_type_claim(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
        Some(Value::Object(object)) => ["raw_value", "value", "name"]
            .into_iter()
            .find_map(|key| object.get(key).and_then(Value::as_str))
            .filter(|text| !text.is_empty())
            .map(ToString::to_string),
        _ => None,
    }
}

fn validate_same_account(old: &OAuthAccount, new: &OAuthAccount) -> OAuthResult<()> {
    if old
        .chatgpt_account_id
        .as_ref()
        .zip(new.chatgpt_account_id.as_ref())
        .is_some_and(|(old, new)| old != new)
    {
        return Err(OAuthError::AccountMismatch);
    }
    if old
        .chatgpt_user_id
        .as_ref()
        .zip(new.chatgpt_user_id.as_ref())
        .is_some_and(|(old, new)| old != new)
    {
        return Err(OAuthError::AccountMismatch);
    }
    Ok(())
}

/// Return redacted record data safe for diagnostics.
pub fn redact_record(record: &OAuthProviderRecord) -> Value {
    record.redacted_value()
}
