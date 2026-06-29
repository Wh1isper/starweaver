//! Codex OAuth request headers and body patching.

use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Map, Value};
use starweaver_oauth::OAuthAccount;

use crate::{
    transport::{HttpMethod, HttpRequest},
    ModelError,
};

/// Codex request header originator used by Starweaver OAuth-backed requests.
pub const CODEX_ORIGINATOR: &str = "starweaver";

pub(super) const CODEX_USER_AGENT_HEADER: &str = "User-Agent";
pub(super) const CODEX_OPENAI_BETA_HEADER: &str = "OpenAI-Beta";
const CODEX_RESPONSES_WEBSOCKET_BETA: &str = "responses_websockets=2026-02-06";
const CODEX_WS_STREAM_REQUEST_START_MS_CLIENT_METADATA_KEY: &str =
    "x-codex-ws-stream-request-start-ms";

/// Reserved headers that user-provided OAuth extra headers may not override.
pub const RESERVED_OAUTH_EXTRA_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "chatgpt-account-id",
    "x-openai-fedramp",
    "originator",
    "version",
];

/// Build Codex request headers without an Authorization header.
///
/// # Errors
///
/// Returns an error when `extra_headers` attempts to override an OAuth/Codex reserved header.
pub fn build_codex_headers(
    account: &OAuthAccount,
    extra_headers: Option<&BTreeMap<String, String>>,
) -> Result<BTreeMap<String, String>, ModelError> {
    let mut headers = BTreeMap::from([("originator".to_string(), CODEX_ORIGINATOR.to_string())]);
    if let Some(account_id) = account.chatgpt_account_id.as_ref() {
        headers.insert("ChatGPT-Account-ID".to_string(), account_id.clone());
    }
    if account.chatgpt_account_is_fedramp {
        headers.insert("X-OpenAI-Fedramp".to_string(), "true".to_string());
    }
    for (key, value) in extra_headers.unwrap_or(&BTreeMap::new()) {
        if RESERVED_OAUTH_EXTRA_HEADERS
            .iter()
            .any(|reserved| key.eq_ignore_ascii_case(reserved))
        {
            return Err(ModelError::Transport(format!(
                "extra_headers may not override reserved OAuth/Codex header: {key}"
            )));
        }
        headers.insert(key.clone(), value.clone());
    }
    Ok(headers)
}

/// Build Codex session/thread headers with underscore and hyphen variants.
#[must_use]
pub fn build_session_headers(
    session_id: Option<&str>,
    thread_id: Option<&str>,
) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    if let Some(session_id) = session_id.filter(|value| !value.is_empty()) {
        headers.insert("session_id".to_string(), session_id.to_string());
        headers.insert("session-id".to_string(), session_id.to_string());
    }
    if let Some(thread_id) = thread_id.filter(|value| !value.is_empty()) {
        headers.insert("thread_id".to_string(), thread_id.to_string());
        headers.insert("thread-id".to_string(), thread_id.to_string());
        headers.insert("x-client-request-id".to_string(), thread_id.to_string());
    }
    headers
}

pub(super) fn validate_safe_extra_headers(
    extra_headers: &BTreeMap<String, String>,
) -> Result<(), ModelError> {
    for key in extra_headers.keys() {
        if RESERVED_OAUTH_EXTRA_HEADERS
            .iter()
            .any(|reserved| key.eq_ignore_ascii_case(reserved))
        {
            return Err(ModelError::Transport(format!(
                "extra_headers may not override reserved OAuth/Codex header: {key}"
            )));
        }
    }
    Ok(())
}

pub(super) fn trace_session_headers(request: &HttpRequest) -> BTreeMap<String, String> {
    let session_id = metadata_string(request, "provider.codex.session_id");
    let thread_id = metadata_string(request, "provider.codex.thread_id");
    build_session_headers(session_id.as_deref(), thread_id.as_deref())
}

fn metadata_string(request: &HttpRequest, key: &str) -> Option<String> {
    request
        .metadata
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// Align Codex Responses API body requirements.
pub fn patch_codex_responses_body(request: &mut HttpRequest) {
    if request.method != HttpMethod::Post || !is_codex_responses_path(&request.url) {
        return;
    }
    let Some(body) = request.body.as_object_mut() else {
        return;
    };
    if body
        .get("instructions")
        .is_none_or(codex_instructions_value_is_falsy)
    {
        body.insert("instructions".to_string(), Value::String(String::new()));
    }
    body.insert("store".to_string(), Value::Bool(false));
}

pub(super) fn patch_codex_websocket_request(request: &mut HttpRequest) {
    if request.method != HttpMethod::Post
        || !is_codex_responses_path(&request.url)
        || request
            .metadata
            .get("starweaver.response_stream_transport")
            .and_then(Value::as_str)
            != Some("websocket")
    {
        return;
    }
    append_comma_header_value_case_insensitive(
        &mut request.headers,
        CODEX_OPENAI_BETA_HEADER,
        CODEX_RESPONSES_WEBSOCKET_BETA,
    );
    insert_websocket_request_start_metadata(&mut request.body);
}

fn append_comma_header_value_case_insensitive(
    headers: &mut BTreeMap<String, String>,
    name: &str,
    value: &str,
) {
    let existing_key = headers
        .keys()
        .find(|key| key.eq_ignore_ascii_case(name))
        .cloned();
    if let Some(existing_key) = existing_key {
        if let Some(existing_value) = headers.get_mut(&existing_key) {
            if existing_value
                .split(',')
                .map(str::trim)
                .any(|part| part.eq_ignore_ascii_case(value))
            {
                return;
            }
            if !existing_value.trim().is_empty() {
                existing_value.push_str(", ");
            }
            existing_value.push_str(value);
        }
    } else {
        headers.insert(name.to_string(), value.to_string());
    }
}

fn insert_websocket_request_start_metadata(body: &mut Value) {
    let Some(body) = body.as_object_mut() else {
        return;
    };
    let client_metadata = body
        .entry("client_metadata".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(client_metadata) = client_metadata.as_object_mut() else {
        return;
    };
    client_metadata
        .entry(CODEX_WS_STREAM_REQUEST_START_MS_CLIENT_METADATA_KEY.to_string())
        .or_insert_with(|| Value::String(unix_time_millis_string()));
}

fn unix_time_millis_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis())
        .to_string()
}

fn is_codex_responses_path(url: &str) -> bool {
    reqwest::Url::parse(url)
        .is_ok_and(|url| url.path().trim_end_matches('/') == "/backend-api/codex/responses")
}

fn codex_instructions_value_is_falsy(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(value) => !value,
        Value::Number(value) => {
            value.as_i64().is_some_and(|value| value == 0)
                || value.as_u64().is_some_and(|value| value == 0)
                || value.as_f64().is_some_and(|value| value == 0.0)
        }
        Value::String(value) => value.is_empty(),
        Value::Array(value) => value.is_empty(),
        Value::Object(value) => value.is_empty(),
    }
}
