//! Agent loop helper functions.

use std::collections::BTreeSet;

use starweaver_model::ModelRequestParameters;

use crate::run::AgentRunState;

pub(super) fn record_tool_control_flow(
    state: &mut AgentRunState,
    tool_return: &starweaver_model::ToolReturnPart,
) {
    let control_flow = tool_return_control_flow(tool_return);
    match control_flow {
        Some("approval_required") => state
            .pending_approval_tool_returns
            .push(tool_return.clone()),
        Some("call_deferred") => state.deferred_tool_returns.push(tool_return.clone()),
        _ => {}
    }
}

pub(super) fn tool_return_control_flow(
    tool_return: &starweaver_model::ToolReturnPart,
) -> Option<&str> {
    tool_return
        .metadata
        .get("control_flow")
        .and_then(serde_json::Value::as_str)
}

pub(super) fn has_pending_tool_control_flow(state: &AgentRunState) -> bool {
    !state.pending_approval_tool_returns.is_empty() || !state.deferred_tool_returns.is_empty()
}

pub(super) fn is_tool_retry_return(tool_return: &starweaver_model::ToolReturnPart) -> bool {
    matches!(
        tool_return
            .metadata
            .get("error_kind")
            .and_then(serde_json::Value::as_str),
        Some("model_retry" | "invalid_arguments")
    ) || matches!(
        tool_return
            .content
            .get("kind")
            .and_then(serde_json::Value::as_str),
        Some("model_retry" | "invalid_arguments")
    )
}

pub(super) fn mark_tool_retry_return(
    tool_return: &mut starweaver_model::ToolReturnPart,
    retry: usize,
    max_retries: usize,
) {
    tool_return
        .metadata
        .insert("retry".to_string(), serde_json::json!(retry));
    tool_return
        .metadata
        .insert("max_retries".to_string(), serde_json::json!(max_retries));
}

pub(super) fn merge_request_params(
    current: &ModelRequestParameters,
    overlay: &ModelRequestParameters,
) -> ModelRequestParameters {
    let mut merged = current.clone();
    let mut names = merged
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<BTreeSet<_>>();
    for tool in &overlay.tools {
        if names.insert(tool.name.clone()) {
            merged.tools.push(tool.clone());
        }
    }
    let mut native_names = merged
        .native_tools
        .iter()
        .map(|tool| tool.tool_type.clone())
        .collect::<BTreeSet<_>>();
    for tool in &overlay.native_tools {
        if native_names.insert(tool.tool_type.clone()) {
            merged.native_tools.push(tool.clone());
        }
    }
    if overlay.output_schema.is_some() {
        merged.output_schema.clone_from(&overlay.output_schema);
    }
    merged.http.headers.extend(overlay.http.headers.clone());
    merged
        .http
        .extra_body
        .extend(overlay.http.extra_body.clone());
    if overlay.http.endpoint_url.is_some() {
        merged
            .http
            .endpoint_url
            .clone_from(&overlay.http.endpoint_url);
    }
    if overlay.http.timeout_ms.is_some() {
        merged.http.timeout_ms = overlay.http.timeout_ms;
    }
    merged.http.metadata.extend(overlay.http.metadata.clone());
    merged.extra_body.extend(overlay.extra_body.clone());
    merged
}
