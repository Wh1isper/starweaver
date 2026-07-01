//! Codex OAuth device-code login and refresh helpers.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{
    CODEX_BASE_URL, CODEX_CLIENT_ID, CODEX_ISSUER, CODEX_REVOKE_ENDPOINT, CODEX_SCOPES,
    CODEX_TOKEN_ENDPOINT,
    error::{OAuthError, OAuthResult},
    jwt::{account_from_id_token, validate_same_account},
    store::OAuthStore,
    token_source::StoreBackedTokenSource,
    types::{
        OAuthAccount, OAuthProviderRecord, OAuthProviderRefresher, OAuthTokens, default_oauth_type,
    },
};

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

pub async fn response_json<T: DeserializeOwned>(response: reqwest::Response) -> OAuthResult<T> {
    if response.status().is_success() {
        Ok(response.json::<T>().await?)
    } else {
        provider_status_error(response).await
    }
}

pub async fn provider_status_error<T>(response: reqwest::Response) -> OAuthResult<T> {
    let status = response.status().as_u16();
    let text = response.text().await?;
    let body = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
    Err(OAuthError::ProviderStatus { status, body })
}
