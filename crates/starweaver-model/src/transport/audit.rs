use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{HttpMethod, HttpRequest};

/// Shared provider request audit recorder.
pub type DynProviderRequestAuditRecorder = Arc<dyn ProviderRequestAuditRecorder>;

/// Provider request payload capture level.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRequestAuditPayloadPolicy {
    /// Do not capture this payload section.
    #[default]
    Omit,
    /// Capture this payload section after redacting configured sensitive keys.
    Redacted,
    /// Capture this payload section without audit-level redaction.
    Full,
}

/// Explicit policy for provider request audit snapshots.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderRequestAuditPolicy {
    /// Header capture policy.
    #[serde(default)]
    pub headers: ProviderRequestAuditPayloadPolicy,
    /// JSON body capture policy.
    #[serde(default)]
    pub body: ProviderRequestAuditPayloadPolicy,
    /// Case-insensitive header names redacted by [`ProviderRequestAuditPayloadPolicy::Redacted`].
    #[serde(default = "default_sensitive_header_keys")]
    pub sensitive_header_keys: BTreeSet<String>,
    /// Case-insensitive JSON object keys redacted by [`ProviderRequestAuditPayloadPolicy::Redacted`].
    #[serde(default = "default_sensitive_body_keys")]
    pub sensitive_body_keys: BTreeSet<String>,
    /// Replacement value used for redacted fields.
    #[serde(default = "default_redaction_value")]
    pub redaction_value: Value,
}

impl Default for ProviderRequestAuditPolicy {
    fn default() -> Self {
        Self {
            headers: ProviderRequestAuditPayloadPolicy::Omit,
            body: ProviderRequestAuditPayloadPolicy::Omit,
            sensitive_header_keys: default_sensitive_header_keys(),
            sensitive_body_keys: default_sensitive_body_keys(),
            redaction_value: default_redaction_value(),
        }
    }
}

impl ProviderRequestAuditPolicy {
    /// Capture method, URL, timeout, and request metadata only.
    #[must_use]
    pub fn metadata_only() -> Self {
        Self::default()
    }

    /// Capture headers and JSON body after audit-level sensitive-field redaction.
    #[must_use]
    pub fn redacted_payloads() -> Self {
        Self {
            headers: ProviderRequestAuditPayloadPolicy::Redacted,
            body: ProviderRequestAuditPayloadPolicy::Redacted,
            ..Self::default()
        }
    }

    /// Capture full headers and JSON body. Use only for explicit local debugging or fixture work.
    #[must_use]
    pub fn full_payloads() -> Self {
        Self {
            headers: ProviderRequestAuditPayloadPolicy::Full,
            body: ProviderRequestAuditPayloadPolicy::Full,
            ..Self::default()
        }
    }

    /// Add a case-insensitive sensitive header name.
    #[must_use]
    pub fn with_sensitive_header_key(mut self, key: impl AsRef<str>) -> Self {
        self.sensitive_header_keys
            .insert(normalize_key(key.as_ref()));
        self
    }

    /// Add a case-insensitive sensitive JSON object key.
    #[must_use]
    pub fn with_sensitive_body_key(mut self, key: impl AsRef<str>) -> Self {
        self.sensitive_body_keys.insert(normalize_key(key.as_ref()));
        self
    }
}

/// Captured provider request snapshot stored outside redacted observability spans.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderRequestAuditSnapshot {
    /// Provider adapter name.
    pub provider_name: String,
    /// Model name used by the adapter.
    pub model_name: String,
    /// Whether the request was sent through the streaming path.
    pub stream: bool,
    /// HTTP method.
    pub method: HttpMethod,
    /// Absolute endpoint URL.
    pub url: String,
    /// Captured headers, when enabled by policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
    /// Captured JSON body, when enabled by policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    /// Request timeout in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Request metadata for audit lookup and correlation.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

impl ProviderRequestAuditSnapshot {
    /// Build a policy-filtered snapshot from an HTTP request.
    #[must_use]
    pub fn from_request(
        provider_name: impl Into<String>,
        model_name: impl Into<String>,
        stream: bool,
        request: &HttpRequest,
        policy: &ProviderRequestAuditPolicy,
    ) -> Self {
        Self {
            provider_name: provider_name.into(),
            model_name: model_name.into(),
            stream,
            method: request.method,
            url: request.url.clone(),
            headers: capture_headers(&request.headers, policy),
            body: capture_body(&request.body, policy),
            timeout_ms: request
                .timeout
                .and_then(|duration| u64::try_from(duration.as_millis()).ok()),
            metadata: request.metadata.clone(),
        }
    }
}

/// Recorder for provider request audit snapshots.
pub trait ProviderRequestAuditRecorder: Send + Sync {
    /// Record a provider request snapshot.
    fn record_provider_request(&self, snapshot: ProviderRequestAuditSnapshot);
}

/// Deterministic in-memory provider request audit recorder.
#[derive(Default)]
pub struct InMemoryProviderRequestAuditRecorder {
    snapshots: Mutex<Vec<ProviderRequestAuditSnapshot>>,
}

impl InMemoryProviderRequestAuditRecorder {
    /// Create an empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return captured snapshots.
    #[must_use]
    pub fn snapshots(&self) -> Vec<ProviderRequestAuditSnapshot> {
        self.snapshots
            .lock()
            .map_or_else(|_| Vec::new(), |v| v.clone())
    }

    /// Remove all captured snapshots and return them.
    #[must_use]
    pub fn take_snapshots(&self) -> Vec<ProviderRequestAuditSnapshot> {
        self.snapshots
            .lock()
            .map_or_else(|_| Vec::new(), |mut v| std::mem::take(&mut *v))
    }
}

impl ProviderRequestAuditRecorder for InMemoryProviderRequestAuditRecorder {
    fn record_provider_request(&self, snapshot: ProviderRequestAuditSnapshot) {
        if let Ok(mut snapshots) = self.snapshots.lock() {
            snapshots.push(snapshot);
        }
    }
}

#[derive(Clone)]
pub struct ProviderRequestAuditCapture {
    recorder: DynProviderRequestAuditRecorder,
    policy: ProviderRequestAuditPolicy,
}

impl ProviderRequestAuditCapture {
    pub(crate) fn new(
        recorder: DynProviderRequestAuditRecorder,
        policy: ProviderRequestAuditPolicy,
    ) -> Self {
        Self { recorder, policy }
    }

    pub(crate) fn record(
        &self,
        provider_name: &str,
        model_name: &str,
        stream: bool,
        request: &HttpRequest,
    ) {
        self.recorder
            .record_provider_request(ProviderRequestAuditSnapshot::from_request(
                provider_name,
                model_name,
                stream,
                request,
                &self.policy,
            ));
    }
}

fn capture_headers(
    headers: &BTreeMap<String, String>,
    policy: &ProviderRequestAuditPolicy,
) -> Option<BTreeMap<String, String>> {
    match policy.headers {
        ProviderRequestAuditPayloadPolicy::Omit => None,
        ProviderRequestAuditPayloadPolicy::Full => Some(headers.clone()),
        ProviderRequestAuditPayloadPolicy::Redacted => Some(
            headers
                .iter()
                .map(|(key, value)| {
                    if policy.sensitive_header_keys.contains(&normalize_key(key)) {
                        (key.clone(), policy.redaction_value.to_string())
                    } else {
                        (key.clone(), value.clone())
                    }
                })
                .collect(),
        ),
    }
}

fn capture_body(body: &Value, policy: &ProviderRequestAuditPolicy) -> Option<Value> {
    match policy.body {
        ProviderRequestAuditPayloadPolicy::Omit => None,
        ProviderRequestAuditPayloadPolicy::Full => Some(body.clone()),
        ProviderRequestAuditPayloadPolicy::Redacted => {
            let mut body = body.clone();
            redact_json_value(&mut body, policy);
            Some(body)
        }
    }
}

fn redact_json_value(value: &mut Value, policy: &ProviderRequestAuditPolicy) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if policy.sensitive_body_keys.contains(&normalize_key(key)) {
                    *value = policy.redaction_value.clone();
                } else {
                    redact_json_value(value, policy);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_json_value(value, policy);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn normalize_key(key: &str) -> String {
    key.to_ascii_lowercase()
}

fn default_sensitive_header_keys() -> BTreeSet<String> {
    [
        "authorization",
        "proxy-authorization",
        "x-api-key",
        "api-key",
        "cookie",
        "set-cookie",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn default_sensitive_body_keys() -> BTreeSet<String> {
    [
        "api_key",
        "apikey",
        "authorization",
        "access_token",
        "refresh_token",
        "id_token",
        "token",
        "password",
        "secret",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn default_redaction_value() -> Value {
    serde_json::json!({ "redacted": true })
}
