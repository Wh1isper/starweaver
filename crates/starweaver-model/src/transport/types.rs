use std::{collections::BTreeMap, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Stable HTTP method set required by model transports.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    /// HTTP POST.
    Post,
}

/// Request sent to an injected model HTTP client.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HttpRequest {
    /// HTTP method.
    pub method: HttpMethod,
    /// Absolute endpoint URL.
    pub url: String,
    /// Headers after adapter defaults, provider config, and request overrides are merged.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// JSON request body after extra body merge.
    pub body: Value,
    /// Optional request timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Duration>,
    /// Request metadata for tracing and auditing.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

/// Response returned by an injected model HTTP client.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response headers.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// JSON response body.
    pub body: Value,
}

impl HttpResponse {
    /// Return a successful JSON response.
    #[must_use]
    pub const fn ok(body: Value) -> Self {
        Self {
            status: 200,
            headers: BTreeMap::new(),
            body,
        }
    }
}

/// Max-token request parameter mapping for provider or gateway HTTP configs.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaxTokensParameter {
    /// Use the protocol adapter's default mapping.
    #[default]
    Default,
    /// Emit `max_tokens`.
    MaxTokens,
    /// Emit `max_output_tokens`.
    MaxOutputTokens,
    /// Emit `max_completion_tokens`.
    MaxCompletionTokens,
    /// Omit provider max-token fields.
    Omit,
}
