use std::fmt::Write as _;

use super::pascal;
use crate::rpc_idl::model::ProtocolIr;

pub fn render(ir: &ProtocolIr) -> String {
    let mut out = String::from(
        "//! Generated strict client codecs and response correlation.\n\nuse serde_json::Value;\nuse super::{errors::{HostError, HostErrorData}, envelope::{HostCall, HostNotification, HostNotificationParams, HostRequest}, metadata::{Method, Notification}, types::*, validation::{validate_launch_envelope, validate_method_params, validate_method_result, validate_notification_params}};\n\npub const LAUNCH_SCHEMA_NAME: &str = \"starweaver.rpc.launch\";\npub const LAUNCH_SCHEMA_VERSION: u32 = 1;\n#[derive(Clone, Copy, Debug, Eq, PartialEq)]\npub enum LaunchEnvelopeCodecError { Parse, SchemaViolation, Serialization }\npub fn decode_launch_envelope(bytes: &[u8]) -> Result<LaunchEnvelope, LaunchEnvelopeCodecError> { let value: Value = serde_json::from_slice(bytes).map_err(|_| LaunchEnvelopeCodecError::Parse)?; validate_launch_envelope(&value).map_err(|()| LaunchEnvelopeCodecError::SchemaViolation)?; serde_json::from_value(value).map_err(|_| LaunchEnvelopeCodecError::SchemaViolation) }\npub fn encode_launch_envelope(envelope: &LaunchEnvelope) -> Result<Vec<u8>, LaunchEnvelopeCodecError> { let value = serde_json::to_value(envelope).map_err(|_| LaunchEnvelopeCodecError::Serialization)?; validate_launch_envelope(&value).map_err(|()| LaunchEnvelopeCodecError::SchemaViolation)?; serde_json::to_vec(&value).map_err(|_| LaunchEnvelopeCodecError::Serialization) }\n\n#[derive(Clone, Debug, Eq, PartialEq)]\npub struct ResponseCorrelation { pub id: RequestId, pub method: Method }\n#[derive(Clone, Debug, Eq, PartialEq)]\npub struct EncodedHostRequest { pub bytes: Vec<u8>, pub correlation: ResponseCorrelation }\n#[derive(Clone, Copy, Debug, Eq, PartialEq)]\npub enum EncodeRequestError { Serialization, SchemaViolation }\n#[derive(Clone, Copy, Debug, Eq, PartialEq)]\npub enum DecodeServerFrameError { Parse, InvalidEnvelope, UncorrelatedResponse, InvalidResult, InvalidRemoteError, InvalidNotification }\n#[derive(Clone, Debug, PartialEq)]\npub enum HostResult {\n",
    );
    for method in ir.methods.values() {
        let _ = writeln!(out, "    {}({}),", pascal(&method.name), method.result_type);
    }
    out.push_str(
        "}\n#[derive(Clone, Debug, PartialEq)]\npub struct CorrelatedHostResponse { pub correlation: ResponseCorrelation, pub result: Result<HostResult, HostError> }\n#[derive(Clone, Debug, PartialEq)]\npub enum HostServerFrame { Response(Box<CorrelatedHostResponse>), Notification(HostNotification) }\n\n#[allow(clippy::match_same_arms)]\npub fn encode_request_frame(request: &HostRequest) -> Result<EncodedHostRequest, EncodeRequestError> { let method = request.call.method(); let params = match &request.call {\n",
    );
    for method in ir.methods.values() {
        let _ = writeln!(
            out,
            "    HostCall::{}(params) => serde_json::to_value(params),",
            pascal(&method.name)
        );
    }
    out.push_str(
        "}.map_err(|_| EncodeRequestError::Serialization)?; validate_method_params(method, &params).map_err(|()| EncodeRequestError::SchemaViolation)?; let bytes = serde_json::to_vec(&serde_json::json!({\"jsonrpc\":\"2.0\",\"id\":request.id.as_str(),\"method\":method.metadata().name,\"params\":params})).map_err(|_| EncodeRequestError::Serialization)?; Ok(EncodedHostRequest { bytes, correlation: ResponseCorrelation { id: request.id.clone(), method } }) }\n\npub fn decode_server_frame<F>(bytes: &[u8], resolve: F) -> Result<HostServerFrame, DecodeServerFrameError> where F: FnOnce(&RequestId) -> Option<Method> { let value: Value = serde_json::from_slice(bytes).map_err(|_| DecodeServerFrameError::Parse)?; let object = value.as_object().ok_or(DecodeServerFrameError::InvalidEnvelope)?; if object.get(\"jsonrpc\").and_then(Value::as_str) != Some(\"2.0\") { return Err(DecodeServerFrameError::InvalidEnvelope); } if object.contains_key(\"id\") { if object.len() != 3 || (!object.contains_key(\"result\") && !object.contains_key(\"error\")) || (object.contains_key(\"result\") && object.contains_key(\"error\")) { return Err(DecodeServerFrameError::InvalidEnvelope); } let id = object.get(\"id\").and_then(Value::as_str).and_then(|value| RequestId::new(value).ok()).ok_or(DecodeServerFrameError::InvalidEnvelope)?; let method = resolve(&id).ok_or(DecodeServerFrameError::UncorrelatedResponse)?; let correlation = ResponseCorrelation { id, method }; let result = if let Some(value) = object.get(\"result\") { validate_method_result(method, value).map_err(|()| DecodeServerFrameError::InvalidResult)?; Ok(decode_result(method, value.clone())?) } else { let error: HostError = serde_json::from_value(object.get(\"error\").cloned().ok_or(DecodeServerFrameError::InvalidEnvelope)?).map_err(|_| DecodeServerFrameError::InvalidRemoteError)?; if !is_remote_error_valid(method, &error) { return Err(DecodeServerFrameError::InvalidRemoteError); } Err(error) }; return Ok(HostServerFrame::Response(Box::new(CorrelatedHostResponse { correlation, result }))); } if object.len() != 3 || !object.contains_key(\"method\") || !object.contains_key(\"params\") { return Err(DecodeServerFrameError::InvalidEnvelope); } let notification = Notification::parse(object.get(\"method\").and_then(Value::as_str).ok_or(DecodeServerFrameError::InvalidEnvelope)?).ok_or(DecodeServerFrameError::InvalidNotification)?; let params = object.get(\"params\").filter(|value| value.is_object()).ok_or(DecodeServerFrameError::InvalidNotification)?.clone(); validate_notification_params(notification, &params).map_err(|()| DecodeServerFrameError::InvalidNotification)?; let params = match notification {\n",
    );
    for notification in ir.notifications.values() {
        let _ = writeln!(
            out,
            "    Notification::{} => HostNotificationParams::{}(Box::new(serde_json::from_value::<{}>(params).map_err(|_| DecodeServerFrameError::InvalidNotification)?)),",
            pascal(&notification.name),
            pascal(&notification.name),
            notification.params_type
        );
    }
    out.push_str(
        "}; Ok(HostServerFrame::Notification(HostNotification { params })) }\n\nfn decode_result(method: Method, value: Value) -> Result<HostResult, DecodeServerFrameError> { match method {\n",
    );
    for method in ir.methods.values() {
        let _ = writeln!(
            out,
            "    Method::{} => serde_json::from_value::<{}>(value).map(HostResult::{}).map_err(|_| DecodeServerFrameError::InvalidResult),",
            pascal(&method.name),
            method.result_type,
            pascal(&method.name)
        );
    }
    out.push_str("} }\n\nconst fn is_remote_error_valid(method: Method, error: &HostError) -> bool { let code_matches_data = matches!((&error.data, error.code),\n");
    for (index, error) in ir.errors.values().enumerate() {
        let separator = if index == 0 { "    " } else { "    | " };
        let _ = writeln!(
            out,
            "{separator}(HostErrorData::{}(_), {})",
            error.name, error.code
        );
    }
    out.push_str("); if !code_matches_data { return false; } match method {\n");
    for method in ir.methods.values() {
        let variants = method
            .errors
            .iter()
            .map(|error| format!("HostErrorData::{error}(_)"))
            .collect::<Vec<_>>()
            .join(" | ");
        let _ = writeln!(
            out,
            "    Method::{} => matches!(&error.data, {}),",
            pascal(&method.name),
            variants
        );
    }
    out.push_str("} }\n");
    out
}
