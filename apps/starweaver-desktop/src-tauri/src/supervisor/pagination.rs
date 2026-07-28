use serde::Serialize;
use starweaver_rpc_core::generated as host;

use super::SupervisorError;

pub(super) fn continuation(result: &host::HostResult) -> Result<Option<String>, SupervisorError> {
    match result {
        host::HostResult::ApprovalList(value) => from_value(value),
        host::HostResult::ClarificationList(value) => from_value(value),
        host::HostResult::DeferredList(value) => from_value(value),
        host::HostResult::EnvironmentList(value) => from_value(value),
        host::HostResult::RunList(value) => from_value(value),
        host::HostResult::SessionList(value) => from_value(value),
        host::HostResult::SessionSearch(value) => from_value(value),
        host::HostResult::WorkspaceList(value) => from_value(value),
        _ => Ok(None),
    }
}

fn from_value<T: Serialize>(value: &T) -> Result<Option<String>, SupervisorError> {
    let value = serde_json::to_value(value).map_err(|_| SupervisorError::transport())?;
    let page = value
        .get("page")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(SupervisorError::transport)?;
    let has_more = page
        .get("hasMore")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(SupervisorError::transport)?;
    let cursor = page
        .get("nextCursor")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    match (has_more, cursor) {
        (false, _) => Ok(None),
        (true, Some(cursor)) => Ok(Some(cursor)),
        (true, None) => Err(SupervisorError::transport()),
    }
}
