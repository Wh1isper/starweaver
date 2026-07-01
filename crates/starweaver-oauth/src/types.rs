//! OAuth storage data types and token source traits.

use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::OAuthResult;

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

pub fn default_oauth_type() -> String {
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
