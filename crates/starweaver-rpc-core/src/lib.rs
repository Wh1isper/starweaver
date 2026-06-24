//! JSON-RPC host protocol helpers for Starweaver.

use std::future::Future;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use starweaver_stream::{
    display_to_agui_event, DisplayMessage, ReplayCursor, ReplayEvent, ReplayEventKind, ReplayScope,
};

mod environment;

pub use environment::{
    environment_attachment_lease_result, environment_attachment_list_result,
    environment_attachment_refs, environment_attachment_result, environment_health_result,
    is_valid_environment_attachment_id, EnvironmentAttachParams, EnvironmentAttachmentAccessMode,
    EnvironmentAttachmentLease, EnvironmentAttachmentRef, EnvironmentAttachmentScope,
    EnvironmentAttachmentScopeKind, EnvironmentAttachmentStatus, EnvironmentDetachParams,
    EnvironmentHealthParams, EnvironmentListParams, EnvironmentReadiness,
    EnvironmentReadinessCapabilities, EnvironmentReadinessPhase, EnvironmentReadinessPolicy,
    EnvironmentReadinessRequest,
};

/// JSON-RPC parse error code.
pub const PARSE_ERROR: i64 = -32_700;
/// JSON-RPC invalid request error code.
pub const INVALID_REQUEST: i64 = -32_600;
/// JSON-RPC method not found error code.
pub const METHOD_NOT_FOUND: i64 = -32_601;
/// JSON-RPC invalid params error code.
pub const INVALID_PARAMS: i64 = -32_602;
/// JSON-RPC server error code.
pub const SERVER_ERROR: i64 = -32_000;
/// Starweaver host error code for a feature unavailable on this connection.
pub const UNSUPPORTED_FEATURE: i64 = -32_002;
/// Starweaver host error code for a create conflict with an existing record.
pub const ALREADY_EXISTS: i64 = -32_011;
/// Starweaver host error code for a run-state conflict.
pub const RUN_CONFLICT: i64 = -32_013;
/// Starweaver host error code for an unavailable environment attachment.
pub const ENVIRONMENT_UNAVAILABLE: i64 = -32_031;
/// Starweaver host error code for configuration or profile resolution failures.
pub const CONFIGURATION_FAILED: i64 = -32_050;

/// JSON-RPC 2.0 request object accepted by Starweaver host transports.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    /// JSON-RPC protocol version. Must be `2.0` when present.
    #[serde(default)]
    pub jsonrpc: Option<String>,
    /// Request id. Missing ids are JSON-RPC notifications.
    #[serde(default)]
    pub id: Option<Value>,
    /// RPC method.
    pub method: String,
    /// Method params.
    #[serde(default)]
    pub params: Value,
}

/// JSON-RPC host error.
#[derive(Debug)]
pub struct RpcError {
    /// JSON-RPC error code.
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
}

impl RpcError {
    /// Create a host error.
    #[must_use]
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Result of handling one JSON-RPC text frame.
#[derive(Debug)]
pub struct JsonRpcOutcome {
    /// Response frame to write. Notifications have no response.
    pub response: Option<Value>,
    /// Whether the request asked the host to shut down successfully.
    pub shutdown: bool,
}

/// Parse and dispatch one JSON-RPC 2.0 text frame.
///
/// # Errors
///
/// The dispatcher returns method-level errors as `RpcError`; framing errors are
/// converted into JSON-RPC error responses directly.
#[must_use]
pub fn handle_json_rpc_text(
    text: &str,
    mut dispatch: impl FnMut(&str, &Value) -> Result<Value, RpcError>,
) -> JsonRpcOutcome {
    let value = match serde_json::from_str::<Value>(text) {
        Ok(value) => value,
        Err(error) => {
            return JsonRpcOutcome {
                response: Some(error_response(
                    &Value::Null,
                    PARSE_ERROR,
                    &format!("parse error: {error}"),
                )),
                shutdown: false,
            };
        }
    };
    let request = match request_from_value(value) {
        Ok(request) => request,
        Err(response) => {
            return JsonRpcOutcome {
                response: Some(response),
                shutdown: false,
            };
        }
    };
    let id = request.id.clone();
    let result = dispatch(&request.method, &request.params);
    let shutdown = request.method == "shutdown" && result.is_ok();
    let Some(id) = id else {
        return JsonRpcOutcome {
            response: None,
            shutdown,
        };
    };
    let response = match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => error_response(&id, error.code, &error.message),
    };
    JsonRpcOutcome {
        response: Some(response),
        shutdown,
    }
}

/// Parse and dispatch one JSON-RPC 2.0 text frame through an async dispatcher.
///
/// # Errors
///
/// The dispatcher returns method-level errors as `RpcError`; framing errors are
/// converted into JSON-RPC error responses directly.
pub async fn handle_json_rpc_text_async<F, Fut>(text: &str, mut dispatch: F) -> JsonRpcOutcome
where
    F: FnMut(String, Value) -> Fut,
    Fut: Future<Output = Result<Value, RpcError>>,
{
    let value = match serde_json::from_str::<Value>(text) {
        Ok(value) => value,
        Err(error) => {
            return JsonRpcOutcome {
                response: Some(error_response(
                    &Value::Null,
                    PARSE_ERROR,
                    &format!("parse error: {error}"),
                )),
                shutdown: false,
            };
        }
    };
    let request = match request_from_value(value) {
        Ok(request) => request,
        Err(response) => {
            return JsonRpcOutcome {
                response: Some(response),
                shutdown: false,
            };
        }
    };
    let id = request.id.clone();
    let method = request.method;
    let result = dispatch(method.clone(), request.params).await;
    let shutdown = method == "shutdown" && result.is_ok();
    let Some(id) = id else {
        return JsonRpcOutcome {
            response: None,
            shutdown,
        };
    };
    let response = match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => error_response(&id, error.code, &error.message),
    };
    JsonRpcOutcome {
        response: Some(response),
        shutdown,
    }
}

fn request_from_value(value: Value) -> Result<JsonRpcRequest, Value> {
    if value.is_array() {
        return Err(error_response(
            &Value::Null,
            INVALID_REQUEST,
            "invalid request: batch arrays are unsupported",
        ));
    }
    let Some(object) = value.as_object() else {
        return Err(error_response(
            &Value::Null,
            INVALID_REQUEST,
            "invalid request: expected object",
        ));
    };
    let id = object.get("id").cloned().unwrap_or(Value::Null);
    let request = serde_json::from_value::<JsonRpcRequest>(value).map_err(|error| {
        error_response(&id, INVALID_REQUEST, &format!("invalid request: {error}"))
    })?;
    if request.jsonrpc.as_deref() != Some("2.0") {
        return Err(error_response(
            &id,
            INVALID_REQUEST,
            "invalid request: jsonrpc must be 2.0",
        ));
    }
    Ok(request)
}

/// Stream payload format requested by a host client.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamPayloadFormat {
    /// Starweaver/AGUI-compatible top-level event object.
    #[default]
    Agui,
    /// Native Starweaver `DisplayMessage`.
    DisplayMessage,
}

impl StreamPayloadFormat {
    /// Parse a stream payload format.
    ///
    /// # Errors
    ///
    /// Returns an RPC invalid-params error for unknown formats.
    pub fn parse(value: Option<&str>) -> Result<Self, RpcError> {
        match value.unwrap_or("agui") {
            "agui" | "agui_json" | "agui-json" => Ok(Self::Agui),
            "display_message" | "display-message" | "display_json" | "display-json" => {
                Ok(Self::DisplayMessage)
            }
            other => Err(RpcError::new(
                INVALID_PARAMS,
                format!("unknown stream payload format: {other}"),
            )),
        }
    }

    /// Stable serialized name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agui => "agui",
            Self::DisplayMessage => "display_message",
        }
    }
}

/// Projected run output item carried by `run.output`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOutputItem {
    session_id: String,
    run_id: String,
    cursor: ReplayCursor,
    payload_format: StreamPayloadFormat,
    payload: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_message: Option<DisplayMessage>,
}

/// Build a JSON-RPC notification frame.
#[must_use]
pub fn notification(method: &str, params: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    })
}

/// Build a JSON-RPC error response frame.
#[must_use]
pub fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message},
    })
}

/// Parse requested stream payload format from params.
///
/// # Errors
///
/// Returns an RPC invalid-params error for unknown formats.
pub fn stream_payload_format(params: &Value) -> Result<StreamPayloadFormat, RpcError> {
    let value = params
        .get("stream")
        .and_then(|stream| {
            stream
                .get("payloadFormat")
                .or_else(|| stream.get("format"))
                .and_then(Value::as_str)
        })
        .or_else(|| params.get("payloadFormat").and_then(Value::as_str))
        .or_else(|| params.get("format").and_then(Value::as_str));
    StreamPayloadFormat::parse(value)
}

/// Parse a replay cursor from params and validate its scope.
///
/// # Errors
///
/// Returns an RPC invalid-params error when the cursor is malformed or scoped
/// differently from the requested replay.
pub fn replay_cursor_from_params(
    params: &Value,
    default_scope: ReplayScope,
) -> Result<Option<ReplayCursor>, RpcError> {
    if let Some(cursor) = params.get("cursor") {
        let cursor = serde_json::from_value::<ReplayCursor>(cursor.clone())
            .map_err(|error| RpcError::new(INVALID_PARAMS, format!("invalid cursor: {error}")))?;
        cursor
            .validate_scope(&default_scope)
            .map_err(|error| RpcError::new(INVALID_PARAMS, error.to_string()))?;
        return Ok(Some(cursor));
    }
    Ok(optional_usize(params, "after").map(|sequence| ReplayCursor::new(default_scope, sequence)))
}

/// Build a run/session attachment result from replay events.
#[must_use]
pub fn attachment_result(
    session_id: &str,
    run_id: Option<&str>,
    active: bool,
    events: &[ReplayEvent],
    format: StreamPayloadFormat,
) -> Value {
    let events = events
        .iter()
        .filter_map(|event| output_item(event, format))
        .collect::<Vec<_>>();
    json!({
        "sessionId": session_id,
        "runId": run_id,
        "active": active,
        "payloadFormat": format.as_str(),
        "events": events,
    })
}

/// Build a replay window result.
#[must_use]
pub fn replay_result(
    session_id: &str,
    run_id: Option<&str>,
    scope: &ReplayScope,
    events: &[ReplayEvent],
    requested_cursor: Option<&ReplayCursor>,
    next_sequence: usize,
) -> Value {
    let messages = display_messages(events);
    let latest_cursor = events
        .last()
        .map(|event| ReplayCursor::new(event.scope.clone(), event.sequence))
        .or_else(|| requested_cursor.cloned());
    json!({
        "sessionId": session_id,
        "runId": run_id,
        "scope": scope,
        "latestCursor": latest_cursor,
        "nextSequence": next_sequence,
        "events": events,
        "messages": messages,
    })
}

/// Convert one replay event into a projected run output item.
#[must_use]
pub fn output_item(event: &ReplayEvent, format: StreamPayloadFormat) -> Option<RunOutputItem> {
    let ReplayEventKind::DisplayMessage(message) = &event.event else {
        return None;
    };
    let display_message = (**message).clone();
    let payload = match format {
        StreamPayloadFormat::Agui => json!(display_to_agui_event(&display_message)),
        StreamPayloadFormat::DisplayMessage => json!(display_message),
    };
    Some(RunOutputItem {
        session_id: display_message.session_id.as_str().to_string(),
        run_id: display_message.run_id.as_str().to_string(),
        cursor: ReplayCursor::new(event.scope.clone(), event.sequence),
        payload_format: format,
        payload,
        display_message: matches!(format, StreamPayloadFormat::DisplayMessage)
            .then_some(display_message),
    })
}

fn display_messages(events: &[ReplayEvent]) -> Vec<DisplayMessage> {
    events
        .iter()
        .filter_map(|event| match &event.event {
            ReplayEventKind::DisplayMessage(message) => Some((**message).clone()),
            _ => None,
        })
        .collect()
}

fn optional_usize(params: &Value, key: &str) -> Option<usize> {
    params
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use serde_json::json;
    use starweaver_core::{RunId, SessionId};
    use starweaver_stream::{DisplayMessage, DisplayMessageKind, ReplayEvent};

    use super::*;

    #[test]
    fn handles_json_rpc_request_notification_and_shutdown() {
        let outcome = handle_json_rpc_text(
            r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{"ok":true}}"#,
            |method, params| Ok(json!({"method": method, "ok": params["ok"]})),
        );
        assert!(!outcome.shutdown);
        let response = outcome.response.unwrap();
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 1);
        assert_eq!(response["result"], json!({"method": "ping", "ok": true}));

        let notification = handle_json_rpc_text(
            r#"{"jsonrpc":"2.0","method":"ping","params":{}}"#,
            |_method, _params| Ok(json!({"ignored": true})),
        );
        assert!(notification.response.is_none());
        assert!(!notification.shutdown);

        let shutdown = handle_json_rpc_text(
            r#"{"jsonrpc":"2.0","id":"stop","method":"shutdown","params":{}}"#,
            |_method, _params| Ok(json!({"status": "shutdown"})),
        );
        assert!(shutdown.shutdown);
        assert_eq!(shutdown.response.unwrap()["id"], "stop");
    }

    #[test]
    fn rejects_invalid_json_rpc_frames_before_dispatch() {
        let batch = handle_json_rpc_text("[]", |_method, _params| {
            panic!("invalid frame should not dispatch")
        });
        assert_eq!(batch.response.unwrap()["error"]["code"], INVALID_REQUEST);

        let wrong_version = handle_json_rpc_text(
            r#"{"jsonrpc":"1.0","id":7,"method":"ping"}"#,
            |_method, _params| panic!("invalid frame should not dispatch"),
        );
        let response = wrong_version.response.unwrap();
        assert_eq!(response["id"], 7);
        assert_eq!(response["error"]["code"], INVALID_REQUEST);

        let missing_version =
            handle_json_rpc_text(r#"{"id":8,"method":"ping"}"#, |_method, _params| {
                panic!("invalid frame should not dispatch")
            });
        let response = missing_version.response.unwrap();
        assert_eq!(response["id"], 8);
        assert_eq!(response["error"]["code"], INVALID_REQUEST);

        let parse_error = handle_json_rpc_text("{", |_method, _params| {
            panic!("invalid frame should not dispatch")
        });
        assert_eq!(parse_error.response.unwrap()["error"]["code"], PARSE_ERROR);
    }

    #[test]
    fn parses_stream_payload_format_from_top_level_or_stream_object() {
        assert_eq!(
            stream_payload_format(&json!({"payloadFormat": "display-message"})).unwrap(),
            StreamPayloadFormat::DisplayMessage
        );
        assert_eq!(
            stream_payload_format(&json!({"stream": {"format": "agui-json"}})).unwrap(),
            StreamPayloadFormat::Agui
        );
        assert!(stream_payload_format(&json!({"format": "bad"})).is_err());
    }

    #[test]
    fn parses_environment_attachment_refs_and_rejects_duplicates() {
        let refs = environment_attachment_refs(&json!({
            "environmentAttachments": [
                {
                    "id": "workspace",
                    "kind": "envd",
                    "mode": "read_write",
                    "default": true,
                    "endpointRef": "http://127.0.0.1:8766/rpc",
                    "environmentId": "env_cli_default"
                }
            ]
        }))
        .unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].id, "workspace");
        assert_eq!(refs[0].kind, "envd");
        assert_eq!(refs[0].mode, EnvironmentAttachmentAccessMode::ReadWrite);
        assert!(refs[0].is_default);
        assert_eq!(
            refs[0].requested_endpoint_ref(),
            Some("http://127.0.0.1:8766/rpc")
        );

        let duplicate = environment_attachment_refs(&json!({
            "environments": [
                {"id": "workspace"},
                {"id": "workspace"}
            ]
        }));
        assert!(duplicate.is_err());

        let invalid_id = environment_attachment_refs(&json!({
            "environment": {"id": "../bad"}
        }));
        assert!(invalid_id.is_err());

        let missing_default = environment_attachment_refs(&json!({
            "environmentAttachments": [
                {"id": "workspace"},
                {"id": "data"}
            ]
        }));
        assert!(missing_default.is_err());

        let one_default = environment_attachment_refs(&json!({
            "environmentAttachments": [
                {"id": "workspace", "default": true},
                {"id": "data"}
            ]
        }))
        .unwrap();
        assert_eq!(one_default.len(), 2);
        assert!(one_default[0].is_default);
    }

    #[test]
    fn parses_environment_attachment_lease_refs_and_results() {
        let refs = environment_attachment_refs(&json!({
            "environmentAttachments": [
                {
                    "id": "workspace",
                    "attachmentLeaseId": "envatt_workspace",
                    "default": true
                },
                {
                    "id": "data",
                    "kind": "envd",
                    "mode": "read_only",
                    "endpointRef": "http://127.0.0.1:8770/rpc",
                    "environmentId": "dataset"
                }
            ]
        }))
        .unwrap();
        assert_eq!(
            refs[0].requested_attachment_lease_id(),
            Some("envatt_workspace")
        );
        assert_eq!(refs[1].mode, EnvironmentAttachmentAccessMode::ReadOnly);

        let lease = EnvironmentAttachmentLease {
            attachment_lease_id: "envatt_workspace".to_string(),
            scope: EnvironmentAttachmentScope {
                kind: EnvironmentAttachmentScopeKind::Session,
                session_id: Some("session_123".to_string()),
            },
            id: "workspace".to_string(),
            kind: "local".to_string(),
            mode: EnvironmentAttachmentAccessMode::ReadWrite,
            is_default: true,
            mount_root: "/environment/workspace".to_string(),
            status: EnvironmentAttachmentStatus::Ready,
            readiness: EnvironmentReadiness {
                transport: EnvironmentReadinessPhase::Ready,
                environment: EnvironmentReadinessPhase::Ready,
                capabilities: EnvironmentReadinessCapabilities {
                    files: vec!["read".to_string(), "list".to_string()],
                    command: vec!["run".to_string()],
                    process: Vec::new(),
                },
                message: None,
            },
            endpoint_ref: None,
            environment_id: None,
            metadata: serde_json::Map::new(),
        };
        let result = environment_attachment_lease_result(&lease);
        assert_eq!(
            result["attachment"]["attachmentLeaseId"],
            "envatt_workspace"
        );
        assert_eq!(result["attachment"]["scope"]["kind"], "session");
        assert_eq!(result["attachment"]["default"], true);
        assert_eq!(result["attachment"]["readiness"]["transport"], "ready");
    }

    #[test]
    fn output_item_projects_display_message_payloads() {
        let mut message = DisplayMessage::new(
            7,
            SessionId::from_string("session_rpc"),
            RunId::from_string("run_rpc"),
            DisplayMessageKind::RunStarted,
        );
        message.payload = json!({"status": "running"});
        let event = ReplayEvent::display(ReplayScope::run("run_rpc"), message);
        let agui = output_item(&event, StreamPayloadFormat::Agui).unwrap();
        let agui_value = serde_json::to_value(agui).unwrap();
        assert_eq!(agui_value["payloadFormat"], "agui");
        assert_eq!(agui_value["payload"]["type"], "RUN_STARTED");

        let native = output_item(&event, StreamPayloadFormat::DisplayMessage).unwrap();
        let native_value = serde_json::to_value(native).unwrap();
        assert_eq!(native_value["payloadFormat"], "display_message");
        assert_eq!(native_value["displayMessage"]["type"], "RUN_STARTED");
    }
}
