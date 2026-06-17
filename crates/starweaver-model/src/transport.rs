//! HTTP transport boundary for production model adapters.

mod client;
pub(crate) mod config;
mod reqwest_client;
mod retry;
mod sse;
mod types;

pub use client::{DynHttpClient, ModelEventStream, ModelHttpClient};
pub(crate) use config::extend_headers_case_insensitive;
pub use config::{
    build_http_request, merge_extra_body, AuthConfig, HttpModelConfig, HttpRequestOptions,
};
pub use reqwest_client::ReqwestHttpClient;
pub use retry::{
    is_retryable_status, send_with_retries, should_retry_error, DynSleeper, ModelSleeper,
    NoopSleeper, RetryPolicy, TokioSleeper,
};
pub use types::{HttpMethod, HttpRequest, HttpResponse, MaxTokensParameter};
