//! JSON-RPC helpers for the envd protocol transport profile.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{EnvdError, EnvdErrorCode};

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

/// JSON-RPC request object accepted by envd transports.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    /// JSON-RPC protocol version. Must be `2.0`.
    #[serde(default)]
    pub jsonrpc: Option<String>,
    /// Request id. Missing ids are notifications.
    #[serde(default)]
    pub id: Option<Value>,
    /// RPC method.
    pub method: String,
    /// Method params.
    #[serde(default)]
    pub params: Value,
}

/// JSON-RPC envd transport error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnvdRpcError {
    /// JSON-RPC error code.
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
}

impl EnvdRpcError {
    /// Create a transport error.
    #[must_use]
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<EnvdError> for EnvdRpcError {
    fn from(error: EnvdError) -> Self {
        let code = match error.code {
            EnvdErrorCode::InvalidRequest => INVALID_PARAMS,
            EnvdErrorCode::NotFound => -32_010,
            EnvdErrorCode::AccessDenied => -32_011,
            EnvdErrorCode::Unsupported => -32_002,
            EnvdErrorCode::Provider => SERVER_ERROR,
        };
        Self::new(code, error.message)
    }
}

/// Parse one JSON-RPC text frame.
///
/// # Errors
///
/// Returns a ready-to-send JSON-RPC error response when framing is invalid.
pub fn parse_json_rpc_text(text: &str) -> Result<JsonRpcRequest, Value> {
    let value = serde_json::from_str::<Value>(text).map_err(|error| {
        error_response(&Value::Null, PARSE_ERROR, &format!("parse error: {error}"))
    })?;
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

/// Build a JSON-RPC success response frame.
#[must_use]
pub fn success_response(id: &Value, result: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
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
