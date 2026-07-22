//! Generated exhaustive typed dispatcher.

use super::{
    envelope::{HostCall, HostRequest, HostResponse},
    errors::{HostError, HostErrorData},
    metadata::Method,
    server::HostServer,
    types::{InternalErrorData, InternalErrorDataKind},
    validation::validate_method_result,
};
pub async fn dispatch<S: HostServer>(
    server: &S,
    context: &S::Context,
    request: HostRequest,
) -> HostResponse {
    let id = request.id;
    let result = match request.call {
        HostCall::ApprovalDecide(params) => server
            .approval_decide(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::ApprovalDecide, value)),
        HostCall::ApprovalList(params) => server
            .approval_list(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::ApprovalList, value)),
        HostCall::ApprovalShow(params) => server
            .approval_show(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::ApprovalShow, value)),
        HostCall::CatalogList(params) => server
            .catalog_list(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::CatalogList, value)),
        HostCall::ClarificationResolve(params) => server
            .clarification_resolve(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::ClarificationResolve, value)),
        HostCall::DeferredComplete(params) => server
            .deferred_complete(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::DeferredComplete, value)),
        HostCall::DeferredFail(params) => server
            .deferred_fail(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::DeferredFail, value)),
        HostCall::DeferredList(params) => server
            .deferred_list(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::DeferredList, value)),
        HostCall::DeferredShow(params) => server
            .deferred_show(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::DeferredShow, value)),
        HostCall::DiagnosticsGet(params) => server
            .diagnostics_get(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::DiagnosticsGet, value)),
        HostCall::EnvironmentAttach(params) => server
            .environment_attach(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::EnvironmentAttach, value)),
        HostCall::EnvironmentDetach(params) => server
            .environment_detach(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::EnvironmentDetach, value)),
        HostCall::EnvironmentHealth(params) => server
            .environment_health(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::EnvironmentHealth, value)),
        HostCall::EnvironmentList(params) => server
            .environment_list(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::EnvironmentList, value)),
        HostCall::EnvironmentMount(params) => server
            .environment_mount(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::EnvironmentMount, value)),
        HostCall::EnvironmentMountsList(params) => server
            .environment_mounts_list(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::EnvironmentMountsList, value)),
        HostCall::EnvironmentUnmount(params) => server
            .environment_unmount(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::EnvironmentUnmount, value)),
        HostCall::EventsReplay(params) => server
            .events_replay(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::EventsReplay, value)),
        HostCall::EventsSubscribe(params) => server
            .events_subscribe(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::EventsSubscribe, value)),
        HostCall::EventsUnsubscribe(params) => server
            .events_unsubscribe(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::EventsUnsubscribe, value)),
        HostCall::Initialize(params) => server
            .initialize(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::Initialize, value)),
        HostCall::ModelSelect(params) => server
            .model_select(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::ModelSelect, value)),
        HostCall::ModelSelectionGet(params) => server
            .model_selection_get(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::ModelSelectionGet, value)),
        HostCall::ProfileGet(params) => server
            .profile_get(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::ProfileGet, value)),
        HostCall::RunInterrupt(params) => server
            .run_interrupt(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::RunInterrupt, value)),
        HostCall::RunResume(params) => server
            .run_resume(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::RunResume, value)),
        HostCall::RunStart(params) => server
            .run_start(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::RunStart, value)),
        HostCall::RunStatus(params) => server
            .run_status(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::RunStatus, value)),
        HostCall::RunSteer(params) => server
            .run_steer(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::RunSteer, value)),
        HostCall::SessionCreate(params) => server
            .session_create(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::SessionCreate, value)),
        HostCall::SessionDelete(params) => server
            .session_delete(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::SessionDelete, value)),
        HostCall::SessionFork(params) => server
            .session_fork(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::SessionFork, value)),
        HostCall::SessionGet(params) => server
            .session_get(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::SessionGet, value)),
        HostCall::SessionList(params) => server
            .session_list(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::SessionList, value)),
        HostCall::SessionSearch(params) => server
            .session_search(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::SessionSearch, value)),
        HostCall::Shutdown(params) => server
            .shutdown(context, params)
            .await
            .map_err(Into::<HostError>::into)
            .and_then(|value| encode_result(Method::Shutdown, value)),
    };
    HostResponse { id, result }
}
fn encode_result<T: serde::Serialize>(
    method: Method,
    value: T,
) -> Result<serde_json::Value, HostError> {
    let value = serde_json::to_value(value).map_err(|_| encoding_error())?;
    validate_method_result(method, &value).map_err(|()| encoding_error())?;
    Ok(value)
}
fn encoding_error() -> HostError {
    HostError {
        code: -32000,
        message: "failed to encode valid typed result".to_string(),
        data: HostErrorData::InternalError(InternalErrorData {
            kind: InternalErrorDataKind::Value,
            retryable: false,
            reconciliation_required: true,
            diagnostic_ref: None,
            resource_kind: None,
        }),
    }
}
