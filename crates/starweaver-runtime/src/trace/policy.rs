use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::{Metadata, TraceContext};
use uuid::Uuid;

use super::{
    DynTraceRecorder, SpanEvent, SpanHandle, SpanSpec, SpanStatus, TraceLevel, TraceRecorder,
};

/// Policy for debug-level trace capture.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceDebugPolicy {
    /// Drop debug spans and events.
    #[default]
    Drop,
    /// Keep debug spans and events, but redact raw payload-like attributes.
    Redacted,
    /// Keep debug spans, events, and payload-like attributes.
    FullPayload,
}

/// Redaction and debug-capture policy applied before trace records reach an exporter.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TraceRedactionPolicy {
    /// Debug span/event handling.
    #[serde(default)]
    pub debug: TraceDebugPolicy,
    /// Case-insensitive keys that should always be redacted.
    #[serde(default = "default_sensitive_keys")]
    pub sensitive_keys: BTreeSet<String>,
    /// Case-insensitive raw payload keys redacted when debug policy is `Redacted`.
    #[serde(default = "default_debug_payload_keys")]
    pub debug_payload_keys: BTreeSet<String>,
    /// Replacement value used for redacted content.
    #[serde(default = "default_redaction_value")]
    pub redaction_value: Value,
}

impl Default for TraceRedactionPolicy {
    fn default() -> Self {
        Self {
            debug: TraceDebugPolicy::Drop,
            sensitive_keys: default_sensitive_keys(),
            debug_payload_keys: default_debug_payload_keys(),
            redaction_value: default_redaction_value(),
        }
    }
}

impl TraceRedactionPolicy {
    /// Default-safe policy: drop debug telemetry and redact sensitive keys.
    #[must_use]
    pub fn default_safe() -> Self {
        Self::default()
    }

    /// Keep debug telemetry while redacting raw payload-like fields.
    #[must_use]
    pub fn debug_redacted() -> Self {
        Self {
            debug: TraceDebugPolicy::Redacted,
            ..Self::default()
        }
    }

    /// Keep debug telemetry and payload-like fields while still redacting secrets.
    #[must_use]
    pub fn debug_full_payloads() -> Self {
        Self {
            debug: TraceDebugPolicy::FullPayload,
            ..Self::default()
        }
    }

    /// Set debug capture behavior.
    #[must_use]
    pub const fn with_debug_policy(mut self, debug: TraceDebugPolicy) -> Self {
        self.debug = debug;
        self
    }

    /// Add a case-insensitive sensitive key.
    #[must_use]
    pub fn with_sensitive_key(mut self, key: impl AsRef<str>) -> Self {
        self.sensitive_keys.insert(normalize_key(key.as_ref()));
        self
    }

    /// Add a case-insensitive debug payload key.
    #[must_use]
    pub fn with_debug_payload_key(mut self, key: impl AsRef<str>) -> Self {
        self.debug_payload_keys.insert(normalize_key(key.as_ref()));
        self
    }

    pub(super) fn sanitize_span_spec(&self, mut spec: SpanSpec) -> Option<SpanSpec> {
        let redact_debug_payloads = match (spec.level, self.debug) {
            (TraceLevel::Debug, TraceDebugPolicy::Drop) => return None,
            (TraceLevel::Debug, TraceDebugPolicy::Redacted) => true,
            (TraceLevel::Debug, TraceDebugPolicy::FullPayload) | (TraceLevel::Info, _) => false,
        };
        scrub_attributes(&mut spec.attributes, self, redact_debug_payloads);
        Some(spec)
    }

    pub(super) fn sanitize_event(&self, mut event: SpanEvent) -> Option<SpanEvent> {
        let redact_debug_payloads = match (event.level, self.debug) {
            (TraceLevel::Debug, TraceDebugPolicy::Drop) => return None,
            (TraceLevel::Debug, TraceDebugPolicy::Redacted) => true,
            (TraceLevel::Debug, TraceDebugPolicy::FullPayload) | (TraceLevel::Info, _) => false,
        };
        scrub_attributes(&mut event.attributes, self, redact_debug_payloads);
        Some(event)
    }
}

/// Trace recorder wrapper that applies [`TraceRedactionPolicy`] before forwarding records.
pub struct PolicyTraceRecorder {
    inner: DynTraceRecorder,
    policy: TraceRedactionPolicy,
    dropped_spans: Arc<Mutex<BTreeSet<String>>>,
}

impl PolicyTraceRecorder {
    /// Wrap a recorder with the default-safe redaction policy.
    #[must_use]
    pub fn new(inner: DynTraceRecorder) -> Self {
        Self::with_policy(inner, TraceRedactionPolicy::default_safe())
    }

    /// Wrap a recorder with an explicit redaction policy.
    #[must_use]
    pub fn with_policy(inner: DynTraceRecorder, policy: TraceRedactionPolicy) -> Self {
        Self {
            inner,
            policy,
            dropped_spans: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    /// Return the active redaction policy.
    #[must_use]
    pub const fn policy(&self) -> &TraceRedactionPolicy {
        &self.policy
    }

    fn dropped_handle(&self, parent: &TraceContext) -> SpanHandle {
        let span_id = format!("dropped_span_{}", Uuid::new_v4());
        if let Ok(mut spans) = self.dropped_spans.lock() {
            spans.insert(span_id.clone());
        }
        let mut metadata = Metadata::default();
        metadata.insert("trace_dropped".to_string(), serde_json::json!(true));
        let context = TraceContext {
            trace_id: parent.trace_id.clone(),
            span_id: Some(span_id.clone()),
            parent_span_id: parent
                .span_id
                .clone()
                .or_else(|| parent.parent_span_id.clone()),
            trace_state: parent.trace_state.clone(),
            metadata,
        };
        SpanHandle::new(context, span_id)
    }

    fn is_dropped_span(&self, span_id: &str) -> bool {
        self.dropped_spans
            .lock()
            .is_ok_and(|spans| spans.contains(span_id))
    }

    fn remove_dropped_span(&self, span_id: &str) -> bool {
        self.dropped_spans
            .lock()
            .is_ok_and(|mut spans| spans.remove(span_id))
    }
}

impl TraceRecorder for PolicyTraceRecorder {
    fn start_span(&self, spec: SpanSpec, parent: &TraceContext) -> SpanHandle {
        let Some(spec) = self.policy.sanitize_span_spec(spec) else {
            return self.dropped_handle(parent);
        };
        self.inner.start_span(spec, parent)
    }

    fn record_event(&self, span: &SpanHandle, event: SpanEvent) {
        if self.is_dropped_span(span.span_id()) {
            return;
        }
        if let Some(event) = self.policy.sanitize_event(event) {
            self.inner.record_event(span, event);
        }
    }

    fn close_span(&self, span: &SpanHandle, status: SpanStatus) {
        if self.remove_dropped_span(span.span_id()) {
            return;
        }
        self.inner.close_span(span, status);
    }
}

fn scrub_attributes(
    attributes: &mut BTreeMap<String, Value>,
    policy: &TraceRedactionPolicy,
    redact_debug_payloads: bool,
) {
    for (key, value) in attributes {
        scrub_value_for_key(key, value, policy, redact_debug_payloads);
    }
}

fn scrub_value_for_key(
    key: &str,
    value: &mut Value,
    policy: &TraceRedactionPolicy,
    redact_debug_payloads: bool,
) {
    if policy.should_redact_key(key, redact_debug_payloads) {
        *value = policy.redaction_value.clone();
        return;
    }

    match value {
        Value::Object(object) => {
            for (nested_key, nested_value) in object {
                scrub_value_for_key(nested_key, nested_value, policy, redact_debug_payloads);
            }
        }
        Value::Array(values) => {
            for nested_value in values {
                scrub_nested_value(nested_value, policy, redact_debug_payloads);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn scrub_nested_value(
    value: &mut Value,
    policy: &TraceRedactionPolicy,
    redact_debug_payloads: bool,
) {
    match value {
        Value::Object(object) => {
            for (nested_key, nested_value) in object {
                scrub_value_for_key(nested_key, nested_value, policy, redact_debug_payloads);
            }
        }
        Value::Array(values) => {
            for nested_value in values {
                scrub_nested_value(nested_value, policy, redact_debug_payloads);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

impl TraceRedactionPolicy {
    fn should_redact_key(&self, key: &str, redact_debug_payloads: bool) -> bool {
        let key = normalize_key(key);
        self.sensitive_keys.contains(&key)
            || key.ends_with("_token")
            || key_component_suffix(&key, '.', "token")
            || key.contains("secret")
            || key.contains("password")
            || (redact_debug_payloads
                && (self.debug_payload_keys.contains(&key)
                    || key.starts_with("raw_")
                    || key.ends_with("_payload")
                    || key_component_suffix(&key, '.', "payload")))
    }
}

fn key_component_suffix(key: &str, separator: char, suffix: &str) -> bool {
    key.rsplit_once(separator)
        .is_some_and(|(_, component)| component == suffix)
}

fn normalize_key(key: &str) -> String {
    key.trim().to_ascii_lowercase()
}

fn default_redaction_value() -> Value {
    serde_json::json!({ "redacted": true })
}

fn default_sensitive_keys() -> BTreeSet<String> {
    [
        "api_key",
        "apikey",
        "authorization",
        "cookie",
        "id_token",
        "password",
        "proxy-authorization",
        "refresh_token",
        "secret",
        "set-cookie",
        "token",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn default_debug_payload_keys() -> BTreeSet<String> {
    [
        "arguments",
        "body",
        "content",
        "headers",
        "http_body",
        "messages",
        "payload",
        "raw_event",
        "raw_request",
        "raw_response",
        "request_body",
        "response_body",
        "result",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}
