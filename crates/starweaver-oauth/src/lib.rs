#![allow(clippy::missing_errors_doc)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::must_use_candidate)]
//! OAuth credential storage and Codex OAuth helpers for Starweaver.
//!
//! This crate is the Rust migration of the reference `ya-oauth` package. It keeps
//! the OAuth auth file under `~/.starweaver/auth.json` by default and exposes a
//! store-backed token source for OAuth-backed model providers.

mod codex;
mod error;
mod jwt;
mod store;
mod token_source;
mod types;

pub use codex::{
    create_codex_token_source, CodexOAuthClient, CodexOAuthProfile, DeviceCode, DeviceTokenResponse,
};
pub use error::{OAuthError, OAuthResult};
pub use jwt::{account_from_id_token, decode_jwt_payload};
pub use store::{default_auth_dir, default_auth_path, OAuthStore};
pub use token_source::StoreBackedTokenSource;
pub use types::{
    AuthFile, OAuthAccount, OAuthProviderRecord, OAuthProviderRefresher, OAuthTokenSource,
    OAuthTokens, TokenSnapshot,
};

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

/// Return redacted record data safe for diagnostics.
pub fn redact_record(record: &OAuthProviderRecord) -> serde_json::Value {
    record.redacted_value()
}
