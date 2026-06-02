//! OAuth-backed model support for CLI provider profiles.

use std::{collections::BTreeMap, env, fs, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use starweaver_model::{HttpRequest, HttpResponse, ModelError, ModelHttpClient, ReqwestHttpClient};

use crate::{error::io_error, CliError, CliResult};

/// `OpenAI` Codex model endpoint base URL.
pub const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

const CODEX_ORIGINATOR: &str = "starweaver_cli";

/// File-backed OAuth credential store.
#[derive(Clone, Debug)]
pub struct OAuthStore {
    path: PathBuf,
}

impl OAuthStore {
    /// Build a store at `~/.starweaver/auth.json` or `STARWEAVER_OAUTH_AUTH_FILE`.
    #[must_use]
    pub fn default_path() -> PathBuf {
        env::var_os("STARWEAVER_OAUTH_AUTH_FILE").map_or_else(
            || {
                env::var_os("HOME")
                    .map_or_else(|| PathBuf::from("."), PathBuf::from)
                    .join(".starweaver/auth.json")
            },
            PathBuf::from,
        )
    }

    /// Create an OAuth store.
    #[must_use]
    pub const fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Load a provider record.
    pub fn load_provider(&self, provider_name: &str) -> CliResult<Option<OAuthProviderRecord>> {
        let auth = self.load()?;
        Ok(auth.providers.get(provider_name).cloned())
    }

    /// Save a provider record.
    pub fn save_provider(&self, provider_name: &str, record: OAuthProviderRecord) -> CliResult<()> {
        let mut auth = self.load()?;
        auth.providers.insert(provider_name.to_string(), record);
        self.save(&auth)
    }

    /// Remove a provider record.
    pub fn remove_provider(&self, provider_name: &str) -> CliResult<bool> {
        let mut auth = self.load()?;
        let removed = auth.providers.remove(provider_name).is_some();
        self.save(&auth)?;
        Ok(removed)
    }

    /// Return the backing auth file path.
    #[must_use]
    pub const fn path(&self) -> &PathBuf {
        &self.path
    }

    fn load(&self) -> CliResult<AuthFile> {
        if !self.path.exists() {
            return Ok(AuthFile::default());
        }
        let content =
            fs::read_to_string(&self.path).map_err(|error| io_error(&self.path, error))?;
        serde_json::from_str(&content).map_err(CliError::from)
    }

    fn save(&self, auth: &AuthFile) -> CliResult<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
            set_dir_private(parent);
        }
        let temp = self.path.with_extension("json.tmp");
        fs::write(&temp, serde_json::to_vec_pretty(auth)?)
            .map_err(|error| io_error(&temp, error))?;
        set_file_private(&temp);
        fs::rename(&temp, &self.path).map_err(|error| io_error(&self.path, error))?;
        set_file_private(&self.path);
        Ok(())
    }
}

#[cfg(unix)]
fn set_dir_private(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn set_dir_private(_path: &std::path::Path) {}

#[cfg(unix)]
fn set_file_private(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_file_private(_path: &std::path::Path) {}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct AuthFile {
    #[serde(default = "default_auth_version")]
    version: u32,
    #[serde(default)]
    providers: BTreeMap<String, OAuthProviderRecord>,
}

const fn default_auth_version() -> u32 {
    1
}

/// Stored OAuth provider record compatible with the shared auth file schema.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OAuthProviderRecord {
    #[serde(rename = "type", default = "default_oauth_type")]
    provider_type: String,
    issuer: String,
    client_id: String,
    token_endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    revoke_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    #[serde(default)]
    scopes: Vec<String>,
    tokens: OAuthTokens,
    #[serde(default)]
    account: OAuthAccount,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_refresh_at: Option<DateTime<Utc>>,
}

impl OAuthProviderRecord {
    /// Return a redacted status object for CLI auth commands.
    #[must_use]
    pub fn status_value(&self) -> Value {
        json!({
            "issuer": self.issuer,
            "client_id": self.client_id,
            "token_endpoint": self.token_endpoint,
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

#[allow(clippy::struct_field_names)]
#[derive(Clone, Debug, Deserialize, Serialize)]
struct OAuthTokens {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id_token: Option<String>,
    access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
}

/// Account metadata derived from Codex tokens.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct OAuthAccount {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chatgpt_user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chatgpt_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chatgpt_plan_type: Option<String>,
    #[serde(default)]
    chatgpt_account_is_fedramp: bool,
}

/// HTTP client wrapper that attaches Codex OAuth headers and refreshes once on 401.
pub struct CodexOAuthHttpClient {
    inner: Arc<dyn ModelHttpClient>,
    store: OAuthStore,
    refresh_client: reqwest::Client,
}

impl CodexOAuthHttpClient {
    /// Create a Codex OAuth HTTP client.
    pub fn new() -> Result<Self, ModelError> {
        Ok(Self {
            inner: Arc::new(ReqwestHttpClient::new()?),
            store: OAuthStore::new(OAuthStore::default_path()),
            refresh_client: reqwest::Client::new(),
        })
    }

    fn prepare_request(mut request: HttpRequest, record: &OAuthProviderRecord) -> HttpRequest {
        request.headers.insert(
            "Authorization".to_string(),
            format!("Bearer {}", record.tokens.access_token),
        );
        request
            .headers
            .extend(build_codex_headers(&request, &record.account));
        patch_codex_responses_body(&mut request);
        request
    }

    fn load_codex_record(&self) -> Result<OAuthProviderRecord, ModelError> {
        self.store
            .load_provider("codex")
            .map_err(|error| ModelError::Transport(error.to_string()))?
            .ok_or_else(|| {
                ModelError::Transport(
                    "OAuth provider codex is not logged in; set STARWEAVER_OAUTH_AUTH_FILE or add credentials to the shared auth store"
                        .to_string(),
                )
            })
    }

    async fn refresh_record(
        &self,
        record: &OAuthProviderRecord,
    ) -> Result<OAuthProviderRecord, ModelError> {
        let refresh_token =
            record.tokens.refresh_token.as_ref().ok_or_else(|| {
                ModelError::Transport("Codex refresh token is missing".to_string())
            })?;
        let response = self
            .refresh_client
            .post(record.token_endpoint.as_str())
            .json(&json!({
                "client_id": record.client_id,
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
            }))
            .send()
            .await
            .map_err(|error| ModelError::Transport(error.to_string()))?;
        let status = response.status().as_u16();
        let body = response
            .json::<Value>()
            .await
            .map_err(|error| ModelError::Transport(error.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(ModelError::ProviderStatus {
                status,
                body,
                retryable: false,
            });
        }
        let access_token = body
            .get("access_token")
            .and_then(Value::as_str)
            .map_or_else(|| record.tokens.access_token.clone(), str::to_string);
        let refreshed = OAuthProviderRecord {
            provider_type: record.provider_type.clone(),
            issuer: record.issuer.clone(),
            client_id: record.client_id.clone(),
            token_endpoint: record.token_endpoint.clone(),
            revoke_endpoint: record.revoke_endpoint.clone(),
            base_url: record.base_url.clone(),
            scopes: record.scopes.clone(),
            tokens: OAuthTokens {
                id_token: body
                    .get("id_token")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| record.tokens.id_token.clone()),
                access_token,
                refresh_token: body
                    .get("refresh_token")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| record.tokens.refresh_token.clone()),
            },
            account: record.account.clone(),
            last_refresh_at: Some(Utc::now()),
        };
        self.store
            .save_provider("codex", refreshed.clone())
            .map_err(|error| ModelError::Transport(error.to_string()))?;
        Ok(refreshed)
    }
}

#[async_trait]
impl ModelHttpClient for CodexOAuthHttpClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, ModelError> {
        let record = self.load_codex_record()?;
        let request_with_auth = Self::prepare_request(request.clone(), &record);
        match self.inner.send(request_with_auth).await {
            Err(ModelError::ProviderStatus { status: 401, .. }) => {
                let refreshed = self.refresh_record(&record).await?;
                self.inner
                    .send(Self::prepare_request(request, &refreshed))
                    .await
            }
            result => result,
        }
    }

    async fn send_event_stream(&self, request: HttpRequest) -> Result<Vec<Value>, ModelError> {
        let record = self.load_codex_record()?;
        let request_with_auth = Self::prepare_request(request.clone(), &record);
        match self.inner.send_event_stream(request_with_auth).await {
            Err(ModelError::ProviderStatus { status: 401, .. }) => {
                let refreshed = self.refresh_record(&record).await?;
                self.inner
                    .send_event_stream(Self::prepare_request(request, &refreshed))
                    .await
            }
            result => result,
        }
    }
}

fn build_codex_headers(request: &HttpRequest, account: &OAuthAccount) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::from([("originator".to_string(), CODEX_ORIGINATOR.to_string())]);
    if let Some(account_id) = account.chatgpt_account_id.as_ref() {
        headers.insert("ChatGPT-Account-ID".to_string(), account_id.clone());
    }
    if account.chatgpt_account_is_fedramp {
        headers.insert("X-OpenAI-Fedramp".to_string(), "true".to_string());
    }
    if let Some(conversation_id) = metadata_string(request, "starweaver.conversation_id") {
        headers.insert("session_id".to_string(), conversation_id.clone());
        headers.insert("session-id".to_string(), conversation_id.clone());
        headers.insert("thread_id".to_string(), conversation_id.clone());
        headers.insert("thread-id".to_string(), conversation_id);
    }
    if let Some(run_id) = metadata_string(request, "starweaver.run_id") {
        headers.insert("x-client-request-id".to_string(), run_id);
    }
    headers
}

fn metadata_string(request: &HttpRequest, key: &str) -> Option<String> {
    request
        .metadata
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn patch_codex_responses_body(request: &mut HttpRequest) {
    if !request
        .url
        .trim_end_matches('/')
        .ends_with("/backend-api/codex/responses")
    {
        return;
    }
    let Some(body) = request.body.as_object_mut() else {
        return;
    };
    if body
        .get("instructions")
        .map_or(true, serde_json::Value::is_null)
    {
        body.insert("instructions".to_string(), Value::String(String::new()));
    }
    body.insert("store".to_string(), Value::Bool(false));
}
