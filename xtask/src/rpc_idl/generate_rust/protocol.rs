use std::fmt::Write as _;

use super::{pascal, snake};
use crate::rpc_idl::model::ProtocolIr;

pub fn identity(ir: &ProtocolIr) -> String {
    format!(
        "//! Generated protocol identity.\n\npub const PROTOCOL_NAME: &str = {:?};\npub const PROTOCOL_MAJOR: u32 = {};\npub const PROTOCOL_REVISION: &str = {:?};\npub const SCHEMA_DIGEST: &str = {:?};\npub const PROTOCOL_IDENTITY: ProtocolIdentityRef = ProtocolIdentityRef {{ name: PROTOCOL_NAME, major: PROTOCOL_MAJOR, revision: PROTOCOL_REVISION, schema_digest: SCHEMA_DIGEST }};\n#[derive(Clone, Copy, Debug, Eq, PartialEq)]\npub struct ProtocolIdentityRef {{ pub name: &'static str, pub major: u32, pub revision: &'static str, pub schema_digest: &'static str }}\n",
        ir.identity.name, ir.identity.major, ir.identity.revision, ir.identity.schema_digest
    )
}

pub fn errors(ir: &ProtocolIr) -> String {
    let mut out = String::from(
        "//! Generated typed public errors.\n\nuse serde::{Deserialize, Serialize};\nuse super::types::*;\n\n",
    );
    for error in ir.errors.values() {
        let _ = writeln!(
            out,
            "/// JSON-RPC code for the generated `{}` public error.\npub const ERROR_CODE_{}: i64 = {};",
            error.name,
            snake(&error.name).to_ascii_uppercase(),
            error.code
        );
    }
    out.push_str("\n#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]\n#[serde(untagged)]\npub enum HostErrorData {\n");
    for error in ir.errors.values() {
        let _ = writeln!(out, "    {}({}),", error.name, error.data_type);
    }
    out.push_str("}\n#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]\n#[serde(deny_unknown_fields)]\npub struct HostError { pub code: i64, pub message: String, pub data: HostErrorData }\n\n");
    for method in ir.methods.values() {
        let ty = format!("{}Error", pascal(&method.name));
        let _ = writeln!(
            out,
            "#[derive(Clone, Debug, Eq, PartialEq)]\npub enum {ty} {{"
        );
        for name in &method.errors {
            let _ = writeln!(
                out,
                "    {name} {{ message: String, data: {} }},",
                ir.errors[name].data_type
            );
        }
        let _ = writeln!(
            out,
            "}}\nimpl From<{ty}> for HostError {{ fn from(error: {ty}) -> Self {{ match error {{"
        );
        for name in &method.errors {
            let error = &ir.errors[name];
            let _ = writeln!(
                out,
                "    {ty}::{name} {{ message, data }} => Self {{ code: {}, message, data: HostErrorData::{name}(data) }},",
                error.code
            );
        }
        out.push_str("} } }\n");
        let _ = writeln!(
            out,
            "impl From<HostError> for {ty} {{ fn from(error: HostError) -> Self {{ match (error.message, error.data) {{"
        );
        for name in &method.errors {
            let _ = writeln!(
                out,
                "    (message, HostErrorData::{name}(data)) => Self::{name} {{ message, data }},"
            );
        }
        out.push_str("    (_, _) => Self::InternalError { message: \"internal error\".to_string(), data: InternalErrorData { kind: InternalErrorDataKind::Value, retryable: false, reconciliation_required: true, diagnostic_ref: None, resource_kind: None } },\n} } }\n\n");
    }
    out
}

pub fn metadata(ir: &ProtocolIr) -> String {
    let mut out = String::from(
        "//! Generated method, notification, event-class, and event-profile metadata.\n\nuse super::types::EventProfile;\n#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum Transport { Stdio, Http }\n#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum Idempotency { None, Idempotent, Effectful, Connection }\n#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub struct MethodMetadata { pub method: Method, pub name: &'static str, pub features: &'static [&'static str], pub transports: &'static [Transport], pub scopes: &'static [&'static str], pub idempotency: Idempotency }\n#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub struct NotificationMetadata { pub notification: Notification, pub name: &'static str, pub features: &'static [&'static str], pub transports: &'static [Transport], pub scopes: &'static [&'static str] }\n#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub struct EventClassMetadata { pub event_class: EventClass, pub name: &'static str, pub schema_type: &'static str, pub feature: Option<&'static str>, pub scopes: &'static [&'static str] }\n#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub struct EventProfileMetadata { pub profile: EventProfile, pub name: &'static str, pub event_classes: &'static [EventClass] }\n#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]\npub enum EventClass {\n",
    );
    for class in ir.event_classes.values() {
        let _ = writeln!(out, "    {},", pascal(&class.name));
    }
    out.push_str("}\npub const EVENT_CLASSES: &[EventClassMetadata] = &[\n");
    for class in ir.event_classes.values() {
        let feature = class.feature.as_ref().map_or_else(
            || "None".to_string(),
            |feature| format!("Some({feature:?})"),
        );
        let scopes = strings(&class.scopes);
        let _ = writeln!(
            out,
            "EventClassMetadata {{ event_class: EventClass::{}, name: {:?}, schema_type: {:?}, feature: {feature}, scopes: &{scopes} }},",
            pascal(&class.name),
            class.name,
            class.schema_type,
        );
    }
    out.push_str(
        "];\nimpl EventClass { #[must_use] pub fn parse(value: &str) -> Option<Self> { match value {\n",
    );
    for class in ir.event_classes.values() {
        let _ = writeln!(
            out,
            "{:?} => Some(Self::{}),",
            class.name,
            pascal(&class.name)
        );
    }
    out.push_str("_ => None } } #[must_use] pub fn metadata(self) -> &'static EventClassMetadata { EVENT_CLASSES.iter().find(|entry| entry.event_class == self).expect(\"generated event-class metadata is exhaustive\") } #[must_use] pub fn is_admitted(self, features: &[&str], scopes: &[&str]) -> bool { let metadata = self.metadata(); let feature_admitted = metadata.feature.is_none_or(|feature| features.contains(&feature)); feature_admitted && metadata.scopes.iter().all(|scope| scopes.contains(scope)) } }\n");
    out.push_str(
        "#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]\npub enum Method {\n",
    );
    for method in ir.methods.values() {
        let _ = writeln!(out, "    {},", pascal(&method.name));
    }
    out.push_str("}\npub const METHODS: &[MethodMetadata] = &[\n");
    for method in ir.methods.values() {
        let features = strings(&method.features);
        let scopes = strings(&method.scopes);
        let transports = method
            .transports
            .iter()
            .map(|value| {
                if value == "stdio" {
                    "Transport::Stdio"
                } else {
                    "Transport::Http"
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "MethodMetadata {{ method: Method::{}, name: {:?}, features: &{features}, transports: &[{transports}], scopes: &{scopes}, idempotency: Idempotency::{} }},",
            pascal(&method.name),
            method.name,
            pascal(&method.idempotency)
        );
    }
    out.push_str(
        "];\nimpl Method { #[must_use] pub fn parse(value: &str) -> Option<Self> { match value {\n",
    );
    for method in ir.methods.values() {
        let _ = writeln!(
            out,
            "{:?} => Some(Self::{}),",
            method.name,
            pascal(&method.name)
        );
    }
    out.push_str("_ => None } } #[must_use] pub fn metadata(self) -> &'static MethodMetadata { METHODS.iter().find(|entry| entry.method == self).expect(\"generated metadata is exhaustive\") } #[must_use] pub fn is_admitted(self, features: &[&str], scopes: &[&str], transport: Transport) -> bool { let metadata = self.metadata(); metadata.transports.contains(&transport) && metadata.features.iter().all(|feature| features.contains(feature)) && metadata.scopes.iter().all(|scope| *scope == \"public\" || scopes.contains(scope)) } }\n");
    out.push_str(
        "#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]\npub enum Notification {\n",
    );
    for notification in ir.notifications.values() {
        let _ = writeln!(out, "    {},", pascal(&notification.name));
    }
    out.push_str("}\npub const NOTIFICATIONS: &[NotificationMetadata] = &[\n");
    for notification in ir.notifications.values() {
        let features = strings(&notification.features);
        let scopes = strings(&notification.scopes);
        let transports = notification
            .transports
            .iter()
            .map(|value| {
                if value == "stdio" {
                    "Transport::Stdio"
                } else {
                    "Transport::Http"
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "NotificationMetadata {{ notification: Notification::{}, name: {:?}, features: &{features}, transports: &[{transports}], scopes: &{scopes} }},",
            pascal(&notification.name),
            notification.name
        );
    }
    out.push_str(
        "];\nimpl Notification { #[must_use] pub fn parse(value: &str) -> Option<Self> { match value {\n",
    );
    for notification in ir.notifications.values() {
        let _ = writeln!(
            out,
            "{:?} => Some(Self::{}),",
            notification.name,
            pascal(&notification.name)
        );
    }
    out.push_str("_ => None } } #[must_use] pub fn metadata(self) -> &'static NotificationMetadata { NOTIFICATIONS.iter().find(|entry| entry.notification == self).expect(\"generated notification metadata is exhaustive\") } }\n");
    out.push_str("pub const EVENT_PROFILES: &[EventProfileMetadata] = &[\n");
    for (profile, event_classes) in &ir.event_profiles {
        let event_classes = event_classes
            .iter()
            .map(|class| format!("EventClass::{}", pascal(class)))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "EventProfileMetadata {{ profile: EventProfile::{}, name: {:?}, event_classes: &[{event_classes}] }},",
            pascal(profile),
            profile
        );
    }
    out.push_str(
        "];\nimpl EventProfile { #[must_use] pub fn metadata(self) -> &'static EventProfileMetadata { EVENT_PROFILES.iter().find(|entry| entry.profile == self).expect(\"generated event-profile metadata is exhaustive\") } #[must_use] pub fn allows_event_class(self, event_class: EventClass) -> bool { self.metadata().event_classes.contains(&event_class) } #[must_use] pub fn is_admitted(self, features: &[&str], scopes: &[&str]) -> bool { self.metadata().event_classes.iter().all(|event_class| event_class.is_admitted(features, scopes)) } }\n",
    );
    out
}

pub fn server(ir: &ProtocolIr) -> String {
    let mut out = String::from(
        "//! Generated exhaustive server boundary.\n\nuse async_trait::async_trait;\nuse super::{errors::*, types::*};\n#[async_trait]\npub trait HostServer: Send + Sync { type Context: Send + Sync;\n",
    );
    for method in ir.methods.values() {
        let _ = writeln!(
            out,
            "async fn {}(&self, context: &Self::Context, params: {}) -> Result<{}, {}Error>;",
            snake(&method.name),
            method.params_type,
            method.result_type,
            pascal(&method.name)
        );
    }
    out.push_str("}\n");
    out
}

pub fn dispatcher(ir: &ProtocolIr) -> String {
    let mut out = String::from(
        "//! Generated exhaustive typed dispatcher.\n\nuse super::{envelope::{HostCall, HostRequest, HostResponse}, server::HostServer, errors::{HostError, HostErrorData}, metadata::Method, types::{InternalErrorData, InternalErrorDataKind}, validation::validate_method_result};\npub async fn dispatch<S: HostServer>(server: &S, context: &S::Context, request: HostRequest) -> HostResponse { let id = request.id; let result = match request.call {\n",
    );
    for method in ir.methods.values() {
        let _ = writeln!(
            out,
            "HostCall::{}(params) => server.{}(context, params).await.map_err(Into::<HostError>::into).and_then(|value| encode_result(Method::{}, value)),",
            pascal(&method.name),
            snake(&method.name),
            pascal(&method.name)
        );
    }
    out.push_str("}; HostResponse { id, result } }\nfn encode_result<T: serde::Serialize>(method: Method, value: T) -> Result<serde_json::Value, HostError> { let value = serde_json::to_value(value).map_err(|_| encoding_error())?; validate_method_result(method, &value).map_err(|()| encoding_error())?; Ok(value) }\nfn encoding_error() -> HostError { HostError { code: -32000, message: \"failed to encode valid typed result\".to_string(), data: HostErrorData::InternalError(InternalErrorData { kind: InternalErrorDataKind::Value, retryable: false, reconciliation_required: true, diagnostic_ref: None, resource_kind: None }) } }\n");
    out
}

pub fn envelope(ir: &ProtocolIr) -> String {
    let mut out = String::from(
        "//! Strict generated JSON-RPC envelopes.\n\nuse serde_json::Value;\nuse super::{errors::{HostError, HostErrorData}, metadata::{Method, Notification}, types::*, validation::{validate_method_params, validate_notification_params}};\n#[derive(Clone, Debug, Eq, PartialEq)]\npub enum HostCall {\n",
    );
    for method in ir.methods.values() {
        let _ = writeln!(out, "{}({}),", pascal(&method.name), method.params_type);
    }
    out.push_str(
        "}\nimpl HostCall { #[must_use] pub const fn method(&self) -> Method { match self {\n",
    );
    for method in ir.methods.values() {
        let _ = writeln!(
            out,
            "Self::{}(_) => Method::{},",
            pascal(&method.name),
            pascal(&method.name)
        );
    }
    out.push_str("} } }\n#[derive(Clone, Debug, Eq, PartialEq)] pub struct HostRequest { pub id: RequestId, pub call: HostCall }\n#[derive(Clone, Debug, PartialEq)] pub struct HostResponse { pub id: RequestId, pub result: Result<Value, HostError> }\n#[derive(Clone, Debug, Eq, PartialEq)] pub struct HostErrorResponse { pub id: Option<RequestId>, pub error: HostError }\n#[derive(Clone, Debug, Eq, PartialEq)] pub struct DecodeRequestError { pub id: Option<RequestId>, pub error: HostError }\nimpl DecodeRequestError { #[must_use] pub fn into_response(self) -> HostErrorResponse { HostErrorResponse { id: self.id, error: self.error } } }\n#[derive(Clone, Debug, Eq, PartialEq)]\npub enum HostNotificationParams {\n");
    for notification in ir.notifications.values() {
        let _ = writeln!(
            out,
            "{}(Box<{}>),",
            pascal(&notification.name),
            notification.params_type
        );
    }
    out.push_str("}\nimpl HostNotificationParams { #[must_use] pub const fn notification(&self) -> Notification { match self {\n");
    for notification in ir.notifications.values() {
        let _ = writeln!(
            out,
            "Self::{}(_) => Notification::{},",
            pascal(&notification.name),
            pascal(&notification.name)
        );
    }
    out.push_str("} } }\n#[derive(Clone, Debug, Eq, PartialEq)] pub struct HostNotification { pub params: HostNotificationParams }\n");
    out.push_str("pub fn decode_request_frame(bytes: &[u8]) -> Result<HostRequest, DecodeRequestError> { let value: Value = serde_json::from_slice(bytes).map_err(|_| decode_error(None, parse_error()))?; let object = value.as_object().ok_or_else(|| decode_error(None, invalid_request()))?; let recovered_id = object.get(\"id\").and_then(Value::as_str).and_then(|value| RequestId::new(value).ok()); if object.len() != 4 || ![\"jsonrpc\",\"id\",\"method\",\"params\"].iter().all(|key| object.contains_key(*key)) { return Err(decode_error(recovered_id, invalid_request())); } if object.get(\"jsonrpc\").and_then(Value::as_str) != Some(\"2.0\") { return Err(decode_error(recovered_id, invalid_request())); } let id = recovered_id.ok_or_else(|| decode_error(None, invalid_request()))?; let method = Method::parse(object.get(\"method\").and_then(Value::as_str).ok_or_else(|| decode_error(Some(id.clone()), invalid_request()))?).ok_or_else(|| decode_error(Some(id.clone()), method_not_found()))?; let params = object.get(\"params\").filter(|value| value.is_object()).ok_or_else(|| decode_error(Some(id.clone()), invalid_request()))?.clone(); validate_method_params(method, &params).map_err(|()| decode_error(Some(id.clone()), invalid_params()))?; let call = match method {\n");
    for method in ir.methods.values() {
        let _ = writeln!(
            out,
            "Method::{} => HostCall::{}(serde_json::from_value::<{}>(params).map_err(|_| decode_error(Some(id.clone()), invalid_params()))?),",
            pascal(&method.name),
            pascal(&method.name),
            method.params_type
        );
    }
    out.push_str("}; Ok(HostRequest { id, call }) }\npub fn encode_response_frame(response: &HostResponse) -> Result<Vec<u8>, serde_json::Error> { match &response.result { Ok(result) => serde_json::to_vec(&serde_json::json!({\"jsonrpc\":\"2.0\",\"id\":response.id.as_str(),\"result\":result})), Err(error) => serde_json::to_vec(&serde_json::json!({\"jsonrpc\":\"2.0\",\"id\":response.id.as_str(),\"error\":error})) } }\npub fn encode_error_response_frame(response: &HostErrorResponse) -> Result<Vec<u8>, serde_json::Error> { serde_json::to_vec(&serde_json::json!({\"jsonrpc\":\"2.0\",\"id\":response.id.as_ref().map(RequestId::as_str),\"error\":response.error})) }\npub fn encode_notification_frame(notification: &HostNotification) -> Result<Vec<u8>, serde_json::Error> { let method = notification.params.notification().metadata().name; match &notification.params {\n");
    for notification in ir.notifications.values() {
        let _ = writeln!(
            out,
            "HostNotificationParams::{}(params) => encode_notification_params(Notification::{}, method, params),",
            pascal(&notification.name),
            pascal(&notification.name)
        );
    }
    out.push_str("} }\nfn encode_notification_params<T: serde::Serialize>(notification: Notification, method: &str, params: &T) -> Result<Vec<u8>, serde_json::Error> { let params = serde_json::to_value(params)?; validate_notification_params(notification, &params).map_err(|()| serde_json::Error::io(std::io::Error::new(std::io::ErrorKind::InvalidData, \"generated notification violated its schema\")))?; serde_json::to_vec(&serde_json::json!({\"jsonrpc\":\"2.0\",\"method\":method,\"params\":params})) }\nconst fn decode_error(id: Option<RequestId>, error: HostError) -> DecodeRequestError { DecodeRequestError { id, error } }\n");
    out.push_str("fn parse_error() -> HostError { HostError { code: -32700, message: \"parse error\".to_string(), data: HostErrorData::ParseError(ParseErrorData { kind: ParseErrorDataKind::Value, retryable: false, reconciliation_required: false, diagnostic_ref: None, resource_kind: None }) } }\nfn invalid_request() -> HostError { HostError { code: -32600, message: \"invalid request\".to_string(), data: HostErrorData::InvalidRequest(InvalidRequestData { kind: InvalidRequestDataKind::Value, retryable: false, reconciliation_required: false, diagnostic_ref: None, resource_kind: None }) } }\nfn method_not_found() -> HostError { HostError { code: -32601, message: \"method not found\".to_string(), data: HostErrorData::MethodNotFound(MethodNotFoundData { kind: MethodNotFoundDataKind::Value, retryable: false, reconciliation_required: false, diagnostic_ref: None, resource_kind: None }) } }\nfn invalid_params() -> HostError { HostError { code: -32602, message: \"invalid params\".to_string(), data: HostErrorData::InvalidParams(InvalidParamsData { kind: InvalidParamsDataKind::Value, retryable: false, reconciliation_required: false, diagnostic_ref: None, resource_kind: None }) } }\n");
    out
}

fn strings(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| format!("{value:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}
