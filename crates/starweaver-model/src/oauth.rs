//! OAuth-backed model provider integration.

mod codex_model;
mod headers;
mod http_client;

pub use codex_model::{
    CodexOAuthResponsesModel, build_codex_model, build_codex_model_with_profile,
    codex_model_profile,
};
pub use headers::{
    CODEX_ORIGINATOR, RESERVED_OAUTH_EXTRA_HEADERS, build_codex_headers, build_session_headers,
    patch_codex_responses_body,
};
pub use http_client::OAuthBearerHttpClient;
