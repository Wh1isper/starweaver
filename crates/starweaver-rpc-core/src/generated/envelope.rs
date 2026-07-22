//! Strict generated JSON-RPC envelopes.

use super::{
    errors::{HostError, HostErrorData},
    metadata::{Method, Notification},
    types::*,
    validation::{validate_method_params, validate_notification_params},
};
use serde_json::Value;
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostCall {
    ApprovalDecide(ApprovalDecideParams),
    ApprovalList(InteractionListParams),
    ApprovalShow(ApprovalShowParams),
    CatalogList(CatalogListParams),
    ClarificationResolve(ClarificationResolveParams),
    DeferredComplete(DeferredCompleteParams),
    DeferredFail(DeferredFailParams),
    DeferredList(InteractionListParams),
    DeferredShow(DeferredShowParams),
    DiagnosticsGet(DiagnosticsGetParams),
    EnvironmentAttach(EnvironmentAttachParams),
    EnvironmentDetach(EnvironmentDetachParams),
    EnvironmentHealth(EnvironmentHealthParams),
    EnvironmentList(EnvironmentListParams),
    EnvironmentMount(EnvironmentMountParams),
    EnvironmentMountsList(EnvironmentMountListParams),
    EnvironmentUnmount(EnvironmentUnmountParams),
    EventsReplay(EventsReplayParams),
    EventsSubscribe(EventsSubscribeParams),
    EventsUnsubscribe(EventsUnsubscribeParams),
    Initialize(InitializeParams),
    ModelSelect(ModelSelectParams),
    ModelSelectionGet(ModelSelectionGetParams),
    ProfileGet(ProfileGetParams),
    RunInterrupt(RunInterruptParams),
    RunResume(RunResumeParams),
    RunStart(RunStartParams),
    RunStatus(RunStatusParams),
    RunSteer(RunSteerParams),
    SessionCreate(SessionCreateParams),
    SessionDelete(SessionDeleteParams),
    SessionFork(SessionForkParams),
    SessionGet(SessionGetParams),
    SessionList(SessionListParams),
    SessionSearch(SessionSearchParams),
    Shutdown(ShutdownParams),
}
impl HostCall {
    #[must_use]
    pub const fn method(&self) -> Method {
        match self {
            Self::ApprovalDecide(_) => Method::ApprovalDecide,
            Self::ApprovalList(_) => Method::ApprovalList,
            Self::ApprovalShow(_) => Method::ApprovalShow,
            Self::CatalogList(_) => Method::CatalogList,
            Self::ClarificationResolve(_) => Method::ClarificationResolve,
            Self::DeferredComplete(_) => Method::DeferredComplete,
            Self::DeferredFail(_) => Method::DeferredFail,
            Self::DeferredList(_) => Method::DeferredList,
            Self::DeferredShow(_) => Method::DeferredShow,
            Self::DiagnosticsGet(_) => Method::DiagnosticsGet,
            Self::EnvironmentAttach(_) => Method::EnvironmentAttach,
            Self::EnvironmentDetach(_) => Method::EnvironmentDetach,
            Self::EnvironmentHealth(_) => Method::EnvironmentHealth,
            Self::EnvironmentList(_) => Method::EnvironmentList,
            Self::EnvironmentMount(_) => Method::EnvironmentMount,
            Self::EnvironmentMountsList(_) => Method::EnvironmentMountsList,
            Self::EnvironmentUnmount(_) => Method::EnvironmentUnmount,
            Self::EventsReplay(_) => Method::EventsReplay,
            Self::EventsSubscribe(_) => Method::EventsSubscribe,
            Self::EventsUnsubscribe(_) => Method::EventsUnsubscribe,
            Self::Initialize(_) => Method::Initialize,
            Self::ModelSelect(_) => Method::ModelSelect,
            Self::ModelSelectionGet(_) => Method::ModelSelectionGet,
            Self::ProfileGet(_) => Method::ProfileGet,
            Self::RunInterrupt(_) => Method::RunInterrupt,
            Self::RunResume(_) => Method::RunResume,
            Self::RunStart(_) => Method::RunStart,
            Self::RunStatus(_) => Method::RunStatus,
            Self::RunSteer(_) => Method::RunSteer,
            Self::SessionCreate(_) => Method::SessionCreate,
            Self::SessionDelete(_) => Method::SessionDelete,
            Self::SessionFork(_) => Method::SessionFork,
            Self::SessionGet(_) => Method::SessionGet,
            Self::SessionList(_) => Method::SessionList,
            Self::SessionSearch(_) => Method::SessionSearch,
            Self::Shutdown(_) => Method::Shutdown,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostRequest {
    pub id: RequestId,
    pub call: HostCall,
}
#[derive(Clone, Debug, PartialEq)]
pub struct HostResponse {
    pub id: RequestId,
    pub result: Result<Value, HostError>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostErrorResponse {
    pub id: Option<RequestId>,
    pub error: HostError,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodeRequestError {
    pub id: Option<RequestId>,
    pub error: HostError,
}
impl DecodeRequestError {
    #[must_use]
    pub fn into_response(self) -> HostErrorResponse {
        HostErrorResponse {
            id: self.id,
            error: self.error,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostNotificationParams {
    HostEvent(Box<HostEventNotificationParams>),
    SubscriptionClosed(Box<SubscriptionClosedNotificationParams>),
}
impl HostNotificationParams {
    #[must_use]
    pub const fn notification(&self) -> Notification {
        match self {
            Self::HostEvent(_) => Notification::HostEvent,
            Self::SubscriptionClosed(_) => Notification::SubscriptionClosed,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostNotification {
    pub params: HostNotificationParams,
}
pub fn decode_request_frame(bytes: &[u8]) -> Result<HostRequest, DecodeRequestError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| decode_error(None, parse_error()))?;
    let object = value
        .as_object()
        .ok_or_else(|| decode_error(None, invalid_request()))?;
    let recovered_id = object
        .get("id")
        .and_then(Value::as_str)
        .and_then(|value| RequestId::new(value).ok());
    if object.len() != 4
        || !["jsonrpc", "id", "method", "params"]
            .iter()
            .all(|key| object.contains_key(*key))
    {
        return Err(decode_error(recovered_id, invalid_request()));
    }
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(decode_error(recovered_id, invalid_request()));
    }
    let id = recovered_id.ok_or_else(|| decode_error(None, invalid_request()))?;
    let method = Method::parse(
        object
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| decode_error(Some(id.clone()), invalid_request()))?,
    )
    .ok_or_else(|| decode_error(Some(id.clone()), method_not_found()))?;
    let params = object
        .get("params")
        .filter(|value| value.is_object())
        .ok_or_else(|| decode_error(Some(id.clone()), invalid_request()))?
        .clone();
    validate_method_params(method, &params)
        .map_err(|()| decode_error(Some(id.clone()), invalid_params()))?;
    let call = match method {
        Method::ApprovalDecide => HostCall::ApprovalDecide(
            serde_json::from_value::<ApprovalDecideParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::ApprovalList => HostCall::ApprovalList(
            serde_json::from_value::<InteractionListParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::ApprovalShow => HostCall::ApprovalShow(
            serde_json::from_value::<ApprovalShowParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::CatalogList => HostCall::CatalogList(
            serde_json::from_value::<CatalogListParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::ClarificationResolve => HostCall::ClarificationResolve(
            serde_json::from_value::<ClarificationResolveParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::DeferredComplete => HostCall::DeferredComplete(
            serde_json::from_value::<DeferredCompleteParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::DeferredFail => HostCall::DeferredFail(
            serde_json::from_value::<DeferredFailParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::DeferredList => HostCall::DeferredList(
            serde_json::from_value::<InteractionListParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::DeferredShow => HostCall::DeferredShow(
            serde_json::from_value::<DeferredShowParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::DiagnosticsGet => HostCall::DiagnosticsGet(
            serde_json::from_value::<DiagnosticsGetParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::EnvironmentAttach => HostCall::EnvironmentAttach(
            serde_json::from_value::<EnvironmentAttachParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::EnvironmentDetach => HostCall::EnvironmentDetach(
            serde_json::from_value::<EnvironmentDetachParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::EnvironmentHealth => HostCall::EnvironmentHealth(
            serde_json::from_value::<EnvironmentHealthParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::EnvironmentList => HostCall::EnvironmentList(
            serde_json::from_value::<EnvironmentListParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::EnvironmentMount => HostCall::EnvironmentMount(
            serde_json::from_value::<EnvironmentMountParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::EnvironmentMountsList => HostCall::EnvironmentMountsList(
            serde_json::from_value::<EnvironmentMountListParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::EnvironmentUnmount => HostCall::EnvironmentUnmount(
            serde_json::from_value::<EnvironmentUnmountParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::EventsReplay => HostCall::EventsReplay(
            serde_json::from_value::<EventsReplayParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::EventsSubscribe => HostCall::EventsSubscribe(
            serde_json::from_value::<EventsSubscribeParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::EventsUnsubscribe => HostCall::EventsUnsubscribe(
            serde_json::from_value::<EventsUnsubscribeParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::Initialize => HostCall::Initialize(
            serde_json::from_value::<InitializeParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::ModelSelect => HostCall::ModelSelect(
            serde_json::from_value::<ModelSelectParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::ModelSelectionGet => HostCall::ModelSelectionGet(
            serde_json::from_value::<ModelSelectionGetParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::ProfileGet => HostCall::ProfileGet(
            serde_json::from_value::<ProfileGetParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::RunInterrupt => HostCall::RunInterrupt(
            serde_json::from_value::<RunInterruptParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::RunResume => HostCall::RunResume(
            serde_json::from_value::<RunResumeParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::RunStart => HostCall::RunStart(
            serde_json::from_value::<RunStartParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::RunStatus => HostCall::RunStatus(
            serde_json::from_value::<RunStatusParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::RunSteer => HostCall::RunSteer(
            serde_json::from_value::<RunSteerParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::SessionCreate => HostCall::SessionCreate(
            serde_json::from_value::<SessionCreateParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::SessionDelete => HostCall::SessionDelete(
            serde_json::from_value::<SessionDeleteParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::SessionFork => HostCall::SessionFork(
            serde_json::from_value::<SessionForkParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::SessionGet => HostCall::SessionGet(
            serde_json::from_value::<SessionGetParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::SessionList => HostCall::SessionList(
            serde_json::from_value::<SessionListParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::SessionSearch => HostCall::SessionSearch(
            serde_json::from_value::<SessionSearchParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
        Method::Shutdown => HostCall::Shutdown(
            serde_json::from_value::<ShutdownParams>(params)
                .map_err(|_| decode_error(Some(id.clone()), invalid_params()))?,
        ),
    };
    Ok(HostRequest { id, call })
}
pub fn encode_response_frame(response: &HostResponse) -> Result<Vec<u8>, serde_json::Error> {
    match &response.result {
        Ok(result) => serde_json::to_vec(
            &serde_json::json!({"jsonrpc":"2.0","id":response.id.as_str(),"result":result}),
        ),
        Err(error) => serde_json::to_vec(
            &serde_json::json!({"jsonrpc":"2.0","id":response.id.as_str(),"error":error}),
        ),
    }
}
pub fn encode_error_response_frame(
    response: &HostErrorResponse,
) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(
        &serde_json::json!({"jsonrpc":"2.0","id":response.id.as_ref().map(RequestId::as_str),"error":response.error}),
    )
}
pub fn encode_notification_frame(
    notification: &HostNotification,
) -> Result<Vec<u8>, serde_json::Error> {
    let method = notification.params.notification().metadata().name;
    match &notification.params {
        HostNotificationParams::HostEvent(params) => {
            encode_notification_params(Notification::HostEvent, method, params)
        }
        HostNotificationParams::SubscriptionClosed(params) => {
            encode_notification_params(Notification::SubscriptionClosed, method, params)
        }
    }
}
fn encode_notification_params<T: serde::Serialize>(
    notification: Notification,
    method: &str,
    params: &T,
) -> Result<Vec<u8>, serde_json::Error> {
    let params = serde_json::to_value(params)?;
    validate_notification_params(notification, &params).map_err(|()| {
        serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "generated notification violated its schema",
        ))
    })?;
    serde_json::to_vec(&serde_json::json!({"jsonrpc":"2.0","method":method,"params":params}))
}
const fn decode_error(id: Option<RequestId>, error: HostError) -> DecodeRequestError {
    DecodeRequestError { id, error }
}
fn parse_error() -> HostError {
    HostError {
        code: -32700,
        message: "parse error".to_string(),
        data: HostErrorData::ParseError(ParseErrorData {
            kind: ParseErrorDataKind::Value,
            retryable: false,
            reconciliation_required: false,
            diagnostic_ref: None,
            resource_kind: None,
        }),
    }
}
fn invalid_request() -> HostError {
    HostError {
        code: -32600,
        message: "invalid request".to_string(),
        data: HostErrorData::InvalidRequest(InvalidRequestData {
            kind: InvalidRequestDataKind::Value,
            retryable: false,
            reconciliation_required: false,
            diagnostic_ref: None,
            resource_kind: None,
        }),
    }
}
fn method_not_found() -> HostError {
    HostError {
        code: -32601,
        message: "method not found".to_string(),
        data: HostErrorData::MethodNotFound(MethodNotFoundData {
            kind: MethodNotFoundDataKind::Value,
            retryable: false,
            reconciliation_required: false,
            diagnostic_ref: None,
            resource_kind: None,
        }),
    }
}
fn invalid_params() -> HostError {
    HostError {
        code: -32602,
        message: "invalid params".to_string(),
        data: HostErrorData::InvalidParams(InvalidParamsData {
            kind: InvalidParamsDataKind::Value,
            retryable: false,
            reconciliation_required: false,
            diagnostic_ref: None,
            resource_kind: None,
        }),
    }
}
