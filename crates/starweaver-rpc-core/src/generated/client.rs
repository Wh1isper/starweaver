//! Generated strict client codecs and response correlation.

use super::{
    envelope::{HostCall, HostNotification, HostNotificationParams, HostRequest},
    errors::{HostError, HostErrorData},
    metadata::{Method, Notification},
    types::*,
    validation::{
        validate_launch_envelope, validate_method_params, validate_method_result,
        validate_notification_params,
    },
};
use serde_json::Value;

pub const LAUNCH_SCHEMA_NAME: &str = "starweaver.rpc.launch";
pub const LAUNCH_SCHEMA_VERSION: u32 = 1;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchEnvelopeCodecError {
    Parse,
    SchemaViolation,
    Serialization,
}
pub fn decode_launch_envelope(bytes: &[u8]) -> Result<LaunchEnvelope, LaunchEnvelopeCodecError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| LaunchEnvelopeCodecError::Parse)?;
    validate_launch_envelope(&value).map_err(|()| LaunchEnvelopeCodecError::SchemaViolation)?;
    serde_json::from_value(value).map_err(|_| LaunchEnvelopeCodecError::SchemaViolation)
}
pub fn encode_launch_envelope(
    envelope: &LaunchEnvelope,
) -> Result<Vec<u8>, LaunchEnvelopeCodecError> {
    let value =
        serde_json::to_value(envelope).map_err(|_| LaunchEnvelopeCodecError::Serialization)?;
    validate_launch_envelope(&value).map_err(|()| LaunchEnvelopeCodecError::SchemaViolation)?;
    serde_json::to_vec(&value).map_err(|_| LaunchEnvelopeCodecError::Serialization)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponseCorrelation {
    pub id: RequestId,
    pub method: Method,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodedHostRequest {
    pub bytes: Vec<u8>,
    pub correlation: ResponseCorrelation,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodeRequestError {
    Serialization,
    SchemaViolation,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeServerFrameError {
    Parse,
    InvalidEnvelope,
    UncorrelatedResponse,
    InvalidResult,
    InvalidRemoteError,
    InvalidNotification,
}
#[derive(Clone, Debug, PartialEq)]
pub enum HostResult {
    ApprovalDecide(ApprovalDecideResult),
    ApprovalList(ApprovalListResult),
    ApprovalShow(ApprovalShowResult),
    CatalogList(CatalogListResult),
    ClarificationResolve(ClarificationResolveResult),
    DeferredComplete(DeferredCompleteResult),
    DeferredFail(DeferredFailResult),
    DeferredList(DeferredListResult),
    DeferredShow(DeferredShowResult),
    DiagnosticsGet(DiagnosticsGetResult),
    EnvironmentAttach(EnvironmentAttachResult),
    EnvironmentDetach(EnvironmentDetachResult),
    EnvironmentHealth(EnvironmentHealthResult),
    EnvironmentList(EnvironmentListResult),
    EnvironmentMount(EnvironmentMountResult),
    EnvironmentMountsList(EnvironmentMountListResult),
    EnvironmentUnmount(EnvironmentUnmountResult),
    EventsReplay(EventsReplayResult),
    EventsSubscribe(EventsSubscribeResult),
    EventsUnsubscribe(EventsUnsubscribeResult),
    Initialize(InitializeResult),
    ModelSelect(ModelSelectResult),
    ModelSelectionGet(ModelSelectionGetResult),
    ProfileGet(ProfileGetResult),
    RunInterrupt(RunInterruptResult),
    RunResume(RunResumeResult),
    RunStart(RunStartResult),
    RunStatus(RunStatusResult),
    RunSteer(RunSteerResult),
    SessionCreate(SessionCreateResult),
    SessionDelete(SessionDeleteResult),
    SessionFork(SessionForkResult),
    SessionGet(SessionGetResult),
    SessionList(SessionListResult),
    SessionSearch(SessionSearchResult),
    Shutdown(ShutdownResult),
}
#[derive(Clone, Debug, PartialEq)]
pub struct CorrelatedHostResponse {
    pub correlation: ResponseCorrelation,
    pub result: Result<HostResult, HostError>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum HostServerFrame {
    Response(Box<CorrelatedHostResponse>),
    Notification(HostNotification),
}

#[allow(clippy::match_same_arms)]
pub fn encode_request_frame(
    request: &HostRequest,
) -> Result<EncodedHostRequest, EncodeRequestError> {
    let method = request.call.method();
    let params = match &request.call {
        HostCall::ApprovalDecide(params) => serde_json::to_value(params),
        HostCall::ApprovalList(params) => serde_json::to_value(params),
        HostCall::ApprovalShow(params) => serde_json::to_value(params),
        HostCall::CatalogList(params) => serde_json::to_value(params),
        HostCall::ClarificationResolve(params) => serde_json::to_value(params),
        HostCall::DeferredComplete(params) => serde_json::to_value(params),
        HostCall::DeferredFail(params) => serde_json::to_value(params),
        HostCall::DeferredList(params) => serde_json::to_value(params),
        HostCall::DeferredShow(params) => serde_json::to_value(params),
        HostCall::DiagnosticsGet(params) => serde_json::to_value(params),
        HostCall::EnvironmentAttach(params) => serde_json::to_value(params),
        HostCall::EnvironmentDetach(params) => serde_json::to_value(params),
        HostCall::EnvironmentHealth(params) => serde_json::to_value(params),
        HostCall::EnvironmentList(params) => serde_json::to_value(params),
        HostCall::EnvironmentMount(params) => serde_json::to_value(params),
        HostCall::EnvironmentMountsList(params) => serde_json::to_value(params),
        HostCall::EnvironmentUnmount(params) => serde_json::to_value(params),
        HostCall::EventsReplay(params) => serde_json::to_value(params),
        HostCall::EventsSubscribe(params) => serde_json::to_value(params),
        HostCall::EventsUnsubscribe(params) => serde_json::to_value(params),
        HostCall::Initialize(params) => serde_json::to_value(params),
        HostCall::ModelSelect(params) => serde_json::to_value(params),
        HostCall::ModelSelectionGet(params) => serde_json::to_value(params),
        HostCall::ProfileGet(params) => serde_json::to_value(params),
        HostCall::RunInterrupt(params) => serde_json::to_value(params),
        HostCall::RunResume(params) => serde_json::to_value(params),
        HostCall::RunStart(params) => serde_json::to_value(params),
        HostCall::RunStatus(params) => serde_json::to_value(params),
        HostCall::RunSteer(params) => serde_json::to_value(params),
        HostCall::SessionCreate(params) => serde_json::to_value(params),
        HostCall::SessionDelete(params) => serde_json::to_value(params),
        HostCall::SessionFork(params) => serde_json::to_value(params),
        HostCall::SessionGet(params) => serde_json::to_value(params),
        HostCall::SessionList(params) => serde_json::to_value(params),
        HostCall::SessionSearch(params) => serde_json::to_value(params),
        HostCall::Shutdown(params) => serde_json::to_value(params),
    }
    .map_err(|_| EncodeRequestError::Serialization)?;
    validate_method_params(method, &params).map_err(|()| EncodeRequestError::SchemaViolation)?;
    let bytes = serde_json::to_vec(&serde_json::json!({"jsonrpc":"2.0","id":request.id.as_str(),"method":method.metadata().name,"params":params})).map_err(|_| EncodeRequestError::Serialization)?;
    Ok(EncodedHostRequest {
        bytes,
        correlation: ResponseCorrelation {
            id: request.id.clone(),
            method,
        },
    })
}

pub fn decode_server_frame<F>(
    bytes: &[u8],
    resolve: F,
) -> Result<HostServerFrame, DecodeServerFrameError>
where
    F: FnOnce(&RequestId) -> Option<Method>,
{
    let value: Value = serde_json::from_slice(bytes).map_err(|_| DecodeServerFrameError::Parse)?;
    let object = value
        .as_object()
        .ok_or(DecodeServerFrameError::InvalidEnvelope)?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(DecodeServerFrameError::InvalidEnvelope);
    }
    if object.contains_key("id") {
        if object.len() != 3
            || (!object.contains_key("result") && !object.contains_key("error"))
            || (object.contains_key("result") && object.contains_key("error"))
        {
            return Err(DecodeServerFrameError::InvalidEnvelope);
        }
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .and_then(|value| RequestId::new(value).ok())
            .ok_or(DecodeServerFrameError::InvalidEnvelope)?;
        let method = resolve(&id).ok_or(DecodeServerFrameError::UncorrelatedResponse)?;
        let correlation = ResponseCorrelation { id, method };
        let result = if let Some(value) = object.get("result") {
            validate_method_result(method, value)
                .map_err(|()| DecodeServerFrameError::InvalidResult)?;
            Ok(decode_result(method, value.clone())?)
        } else {
            let error: HostError = serde_json::from_value(
                object
                    .get("error")
                    .cloned()
                    .ok_or(DecodeServerFrameError::InvalidEnvelope)?,
            )
            .map_err(|_| DecodeServerFrameError::InvalidRemoteError)?;
            if !is_remote_error_valid(method, &error) {
                return Err(DecodeServerFrameError::InvalidRemoteError);
            }
            Err(error)
        };
        return Ok(HostServerFrame::Response(Box::new(
            CorrelatedHostResponse {
                correlation,
                result,
            },
        )));
    }
    if object.len() != 3 || !object.contains_key("method") || !object.contains_key("params") {
        return Err(DecodeServerFrameError::InvalidEnvelope);
    }
    let notification = Notification::parse(
        object
            .get("method")
            .and_then(Value::as_str)
            .ok_or(DecodeServerFrameError::InvalidEnvelope)?,
    )
    .ok_or(DecodeServerFrameError::InvalidNotification)?;
    let params = object
        .get("params")
        .filter(|value| value.is_object())
        .ok_or(DecodeServerFrameError::InvalidNotification)?
        .clone();
    validate_notification_params(notification, &params)
        .map_err(|()| DecodeServerFrameError::InvalidNotification)?;
    let params = match notification {
        Notification::HostEvent => HostNotificationParams::HostEvent(Box::new(
            serde_json::from_value::<HostEventNotificationParams>(params)
                .map_err(|_| DecodeServerFrameError::InvalidNotification)?,
        )),
        Notification::SubscriptionClosed => HostNotificationParams::SubscriptionClosed(Box::new(
            serde_json::from_value::<SubscriptionClosedNotificationParams>(params)
                .map_err(|_| DecodeServerFrameError::InvalidNotification)?,
        )),
    };
    Ok(HostServerFrame::Notification(HostNotification { params }))
}

fn decode_result(method: Method, value: Value) -> Result<HostResult, DecodeServerFrameError> {
    match method {
        Method::ApprovalDecide => serde_json::from_value::<ApprovalDecideResult>(value)
            .map(HostResult::ApprovalDecide)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::ApprovalList => serde_json::from_value::<ApprovalListResult>(value)
            .map(HostResult::ApprovalList)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::ApprovalShow => serde_json::from_value::<ApprovalShowResult>(value)
            .map(HostResult::ApprovalShow)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::CatalogList => serde_json::from_value::<CatalogListResult>(value)
            .map(HostResult::CatalogList)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::ClarificationResolve => serde_json::from_value::<ClarificationResolveResult>(value)
            .map(HostResult::ClarificationResolve)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::DeferredComplete => serde_json::from_value::<DeferredCompleteResult>(value)
            .map(HostResult::DeferredComplete)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::DeferredFail => serde_json::from_value::<DeferredFailResult>(value)
            .map(HostResult::DeferredFail)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::DeferredList => serde_json::from_value::<DeferredListResult>(value)
            .map(HostResult::DeferredList)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::DeferredShow => serde_json::from_value::<DeferredShowResult>(value)
            .map(HostResult::DeferredShow)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::DiagnosticsGet => serde_json::from_value::<DiagnosticsGetResult>(value)
            .map(HostResult::DiagnosticsGet)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::EnvironmentAttach => serde_json::from_value::<EnvironmentAttachResult>(value)
            .map(HostResult::EnvironmentAttach)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::EnvironmentDetach => serde_json::from_value::<EnvironmentDetachResult>(value)
            .map(HostResult::EnvironmentDetach)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::EnvironmentHealth => serde_json::from_value::<EnvironmentHealthResult>(value)
            .map(HostResult::EnvironmentHealth)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::EnvironmentList => serde_json::from_value::<EnvironmentListResult>(value)
            .map(HostResult::EnvironmentList)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::EnvironmentMount => serde_json::from_value::<EnvironmentMountResult>(value)
            .map(HostResult::EnvironmentMount)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::EnvironmentMountsList => {
            serde_json::from_value::<EnvironmentMountListResult>(value)
                .map(HostResult::EnvironmentMountsList)
                .map_err(|_| DecodeServerFrameError::InvalidResult)
        }
        Method::EnvironmentUnmount => serde_json::from_value::<EnvironmentUnmountResult>(value)
            .map(HostResult::EnvironmentUnmount)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::EventsReplay => serde_json::from_value::<EventsReplayResult>(value)
            .map(HostResult::EventsReplay)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::EventsSubscribe => serde_json::from_value::<EventsSubscribeResult>(value)
            .map(HostResult::EventsSubscribe)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::EventsUnsubscribe => serde_json::from_value::<EventsUnsubscribeResult>(value)
            .map(HostResult::EventsUnsubscribe)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::Initialize => serde_json::from_value::<InitializeResult>(value)
            .map(HostResult::Initialize)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::ModelSelect => serde_json::from_value::<ModelSelectResult>(value)
            .map(HostResult::ModelSelect)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::ModelSelectionGet => serde_json::from_value::<ModelSelectionGetResult>(value)
            .map(HostResult::ModelSelectionGet)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::ProfileGet => serde_json::from_value::<ProfileGetResult>(value)
            .map(HostResult::ProfileGet)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::RunInterrupt => serde_json::from_value::<RunInterruptResult>(value)
            .map(HostResult::RunInterrupt)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::RunResume => serde_json::from_value::<RunResumeResult>(value)
            .map(HostResult::RunResume)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::RunStart => serde_json::from_value::<RunStartResult>(value)
            .map(HostResult::RunStart)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::RunStatus => serde_json::from_value::<RunStatusResult>(value)
            .map(HostResult::RunStatus)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::RunSteer => serde_json::from_value::<RunSteerResult>(value)
            .map(HostResult::RunSteer)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::SessionCreate => serde_json::from_value::<SessionCreateResult>(value)
            .map(HostResult::SessionCreate)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::SessionDelete => serde_json::from_value::<SessionDeleteResult>(value)
            .map(HostResult::SessionDelete)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::SessionFork => serde_json::from_value::<SessionForkResult>(value)
            .map(HostResult::SessionFork)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::SessionGet => serde_json::from_value::<SessionGetResult>(value)
            .map(HostResult::SessionGet)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::SessionList => serde_json::from_value::<SessionListResult>(value)
            .map(HostResult::SessionList)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::SessionSearch => serde_json::from_value::<SessionSearchResult>(value)
            .map(HostResult::SessionSearch)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
        Method::Shutdown => serde_json::from_value::<ShutdownResult>(value)
            .map(HostResult::Shutdown)
            .map_err(|_| DecodeServerFrameError::InvalidResult),
    }
}

const fn is_remote_error_valid(method: Method, error: &HostError) -> bool {
    let code_matches_data = matches!(
        (&error.data, error.code),
        (HostErrorData::AlreadyExists(_), -32011)
            | (HostErrorData::AuthorizationDenied(_), -32017)
            | (HostErrorData::ConfigurationFailed(_), -32050)
            | (HostErrorData::CursorInvalid(_), -32016)
            | (HostErrorData::EnvironmentUnavailable(_), -32031)
            | (HostErrorData::IdempotencyConflict(_), -32012)
            | (HostErrorData::InternalError(_), -32000)
            | (HostErrorData::InvalidParams(_), -32602)
            | (HostErrorData::InvalidRequest(_), -32600)
            | (HostErrorData::MethodNotFound(_), -32601)
            | (HostErrorData::NotFound(_), -32010)
            | (HostErrorData::NotInitialized(_), -32001)
            | (HostErrorData::ParseError(_), -32700)
            | (HostErrorData::RunConflict(_), -32013)
            | (HostErrorData::SessionSearchUnavailable(_), -32032)
            | (HostErrorData::StaleFence(_), -32014)
            | (HostErrorData::StorageUnavailable(_), -32015)
            | (HostErrorData::UnsupportedFeature(_), -32002)
    );
    if !code_matches_data {
        return false;
    }
    match method {
        Method::ApprovalDecide => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::AlreadyExists(_)
                | HostErrorData::IdempotencyConflict(_)
                | HostErrorData::RunConflict(_)
                | HostErrorData::StaleFence(_)
        ),
        Method::ApprovalList => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::SessionSearchUnavailable(_)
        ),
        Method::ApprovalShow => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::SessionSearchUnavailable(_)
        ),
        Method::CatalogList => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
        ),
        Method::ClarificationResolve => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::AlreadyExists(_)
                | HostErrorData::IdempotencyConflict(_)
                | HostErrorData::RunConflict(_)
                | HostErrorData::StaleFence(_)
        ),
        Method::DeferredComplete => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::AlreadyExists(_)
                | HostErrorData::IdempotencyConflict(_)
                | HostErrorData::RunConflict(_)
                | HostErrorData::StaleFence(_)
        ),
        Method::DeferredFail => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::AlreadyExists(_)
                | HostErrorData::IdempotencyConflict(_)
                | HostErrorData::RunConflict(_)
                | HostErrorData::StaleFence(_)
        ),
        Method::DeferredList => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::SessionSearchUnavailable(_)
        ),
        Method::DeferredShow => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::SessionSearchUnavailable(_)
        ),
        Method::DiagnosticsGet => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
        ),
        Method::EnvironmentAttach => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::AlreadyExists(_)
                | HostErrorData::IdempotencyConflict(_)
                | HostErrorData::RunConflict(_)
                | HostErrorData::StaleFence(_)
                | HostErrorData::EnvironmentUnavailable(_)
        ),
        Method::EnvironmentDetach => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::AlreadyExists(_)
                | HostErrorData::IdempotencyConflict(_)
                | HostErrorData::RunConflict(_)
                | HostErrorData::StaleFence(_)
                | HostErrorData::EnvironmentUnavailable(_)
        ),
        Method::EnvironmentHealth => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::SessionSearchUnavailable(_)
                | HostErrorData::EnvironmentUnavailable(_)
        ),
        Method::EnvironmentList => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::SessionSearchUnavailable(_)
                | HostErrorData::EnvironmentUnavailable(_)
        ),
        Method::EnvironmentMount => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::AlreadyExists(_)
                | HostErrorData::IdempotencyConflict(_)
                | HostErrorData::RunConflict(_)
                | HostErrorData::StaleFence(_)
                | HostErrorData::EnvironmentUnavailable(_)
        ),
        Method::EnvironmentMountsList => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::SessionSearchUnavailable(_)
                | HostErrorData::EnvironmentUnavailable(_)
        ),
        Method::EnvironmentUnmount => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::AlreadyExists(_)
                | HostErrorData::IdempotencyConflict(_)
                | HostErrorData::RunConflict(_)
                | HostErrorData::StaleFence(_)
                | HostErrorData::EnvironmentUnavailable(_)
        ),
        Method::EventsReplay => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::CursorInvalid(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::SessionSearchUnavailable(_)
        ),
        Method::EventsSubscribe => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::CursorInvalid(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::SessionSearchUnavailable(_)
        ),
        Method::EventsUnsubscribe => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::SessionSearchUnavailable(_)
        ),
        Method::Initialize => matches!(
            &error.data,
            HostErrorData::InvalidParams(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::InternalError(_)
        ),
        Method::ModelSelect => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::AlreadyExists(_)
                | HostErrorData::IdempotencyConflict(_)
                | HostErrorData::RunConflict(_)
                | HostErrorData::StaleFence(_)
        ),
        Method::ModelSelectionGet => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
        ),
        Method::ProfileGet => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
        ),
        Method::RunInterrupt => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::AlreadyExists(_)
                | HostErrorData::IdempotencyConflict(_)
                | HostErrorData::RunConflict(_)
                | HostErrorData::StaleFence(_)
        ),
        Method::RunResume => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::AlreadyExists(_)
                | HostErrorData::IdempotencyConflict(_)
                | HostErrorData::RunConflict(_)
                | HostErrorData::StaleFence(_)
        ),
        Method::RunStart => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::AlreadyExists(_)
                | HostErrorData::IdempotencyConflict(_)
                | HostErrorData::RunConflict(_)
                | HostErrorData::StaleFence(_)
        ),
        Method::RunStatus => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::SessionSearchUnavailable(_)
        ),
        Method::RunSteer => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::AlreadyExists(_)
                | HostErrorData::IdempotencyConflict(_)
                | HostErrorData::RunConflict(_)
                | HostErrorData::StaleFence(_)
        ),
        Method::SessionCreate => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::AlreadyExists(_)
                | HostErrorData::IdempotencyConflict(_)
                | HostErrorData::RunConflict(_)
                | HostErrorData::StaleFence(_)
        ),
        Method::SessionDelete => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::AlreadyExists(_)
                | HostErrorData::IdempotencyConflict(_)
                | HostErrorData::RunConflict(_)
                | HostErrorData::StaleFence(_)
        ),
        Method::SessionFork => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::AlreadyExists(_)
                | HostErrorData::IdempotencyConflict(_)
                | HostErrorData::RunConflict(_)
                | HostErrorData::StaleFence(_)
        ),
        Method::SessionGet => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::SessionSearchUnavailable(_)
        ),
        Method::SessionList => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
        ),
        Method::SessionSearch => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::SessionSearchUnavailable(_)
        ),
        Method::Shutdown => matches!(
            &error.data,
            HostErrorData::NotInitialized(_)
                | HostErrorData::UnsupportedFeature(_)
                | HostErrorData::NotFound(_)
                | HostErrorData::StorageUnavailable(_)
                | HostErrorData::AuthorizationDenied(_)
                | HostErrorData::InternalError(_)
                | HostErrorData::AlreadyExists(_)
                | HostErrorData::IdempotencyConflict(_)
                | HostErrorData::RunConflict(_)
                | HostErrorData::StaleFence(_)
        ),
    }
}
