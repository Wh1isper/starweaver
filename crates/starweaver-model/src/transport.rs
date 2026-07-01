//! HTTP transport boundary for production model adapters.

mod audit;
mod client;
pub(crate) mod config;
mod reqwest_client;
mod retry;
mod sse;
mod types;
mod websocket;

pub(crate) use audit::ProviderRequestAuditCapture;
pub use audit::{
    DynProviderRequestAuditRecorder, InMemoryProviderRequestAuditRecorder,
    ProviderRequestAuditPayloadPolicy, ProviderRequestAuditPolicy, ProviderRequestAuditRecorder,
    ProviderRequestAuditSnapshot,
};
pub use client::{DynHttpClient, ModelEventStream, ModelHttpClient, ModelWebSocketEventSession};
pub(crate) use config::extend_headers_case_insensitive;
pub use config::{
    AuthConfig, HttpModelConfig, HttpRequestOptions, build_http_request, merge_extra_body,
};
pub use reqwest_client::ReqwestHttpClient;
pub use retry::{
    DynSleeper, ModelSleeper, NoopSleeper, RetryPolicy, TokioSleeper, is_retryable_status,
    send_with_retries, should_retry_error,
};
pub use types::{HttpMethod, HttpRequest, HttpResponse, MaxTokensParameter};
pub use websocket::should_fallback_websocket_to_http;
