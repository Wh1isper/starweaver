use std::collections::BTreeSet;

use starweaver_model::{ModelMessage, ModelRequestPart, ModelResponsePart, ToolReturnPart};
use starweaver_session::{ApprovalRecord, ApprovalStatus, DeferredToolRecord, ExecutionStatus};

use crate::CliError;

pub(super) fn existing_resume_tool_return_ids(history: &[ModelMessage]) -> BTreeSet<String> {
    let Some(last_tool_response_index) = history.iter().rposition(|message| match message {
        ModelMessage::Response(response) => response
            .parts
            .iter()
            .any(|part| matches!(part, ModelResponsePart::ToolCall(_))),
        ModelMessage::Request(_) => false,
    }) else {
        return BTreeSet::new();
    };
    history
        .iter()
        .skip(last_tool_response_index.saturating_add(1))
        .filter_map(|message| match message {
            ModelMessage::Request(request) => Some(&request.parts),
            ModelMessage::Response(_) => None,
        })
        .flat_map(|parts| parts.iter())
        .filter_map(|part| match part {
            ModelRequestPart::ToolReturn(tool_return) => Some(tool_return.tool_call_id.clone()),
            _ => None,
        })
        .collect()
}

pub(super) fn latest_tool_call_order(history: &[ModelMessage]) -> Vec<String> {
    history
        .iter()
        .rev()
        .find_map(|message| match message {
            ModelMessage::Response(response) => Some(
                response
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        ModelResponsePart::ToolCall(call) => Some(call.id.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            ),
            ModelMessage::Request(_) => None,
        })
        .unwrap_or_default()
}

pub(super) fn tool_return_control_flow(tool_return: &ToolReturnPart) -> Option<&str> {
    tool_return
        .metadata
        .get("control_flow")
        .and_then(serde_json::Value::as_str)
}

pub(super) const fn deferred_status_is_unresolved(status: ExecutionStatus) -> bool {
    matches!(
        status,
        ExecutionStatus::Pending | ExecutionStatus::Running | ExecutionStatus::Waiting
    )
}

pub(super) fn pending_hitl_resume_error(
    run_id: &str,
    pending_approvals: &[String],
    pending_deferred: &[String],
) -> CliError {
    let mut details = Vec::new();
    if !pending_approvals.is_empty() {
        details.push(format!("approvals={}", pending_approvals.join(",")));
    }
    if !pending_deferred.is_empty() {
        details.push(format!("deferred_tools={}", pending_deferred.join(",")));
    }
    CliError::Usage(format!(
        "cannot resume run {run_id}: pending HITL decisions remain ({})",
        details.join("; ")
    ))
}

pub(super) fn approval_tool_return(record: &ApprovalRecord) -> Option<ToolReturnPart> {
    let decision = record.decision.as_ref();
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "control_flow_resolution".to_string(),
        serde_json::json!("approval"),
    );
    metadata.insert(
        "approval_id".to_string(),
        serde_json::json!(record.approval_id),
    );
    metadata.insert(
        "approval_status".to_string(),
        serde_json::json!(record.status),
    );
    if let Some(decision) = decision {
        metadata.insert("decision".to_string(), serde_json::json!(decision));
    }
    match record.status {
        ApprovalStatus::Approved => {
            let mut content = serde_json::json!({
                "approved": true,
                "approval_id": record.approval_id,
                "tool_name": record.action_name,
                "request": record.request,
            });
            if let Some(reason) = decision.and_then(|decision| decision.reason.as_ref()) {
                content["reason"] = serde_json::json!(reason);
            }
            Some(
                ToolReturnPart::new(
                    record.action_id.clone(),
                    record.action_name.clone(),
                    content,
                )
                .with_metadata(metadata),
            )
        }
        ApprovalStatus::Denied | ApprovalStatus::Expired | ApprovalStatus::Cancelled => {
            let reason = decision
                .and_then(|decision| decision.reason.clone())
                .unwrap_or_else(|| format!("approval {}", approval_status_name(record.status)));
            Some(
                ToolReturnPart::new(
                    record.action_id.clone(),
                    record.action_name.clone(),
                    serde_json::json!({
                        "approved": false,
                        "approval_id": record.approval_id,
                        "tool_name": record.action_name,
                        "reason": reason,
                    }),
                )
                .with_error(true)
                .with_metadata(metadata),
            )
        }
        ApprovalStatus::Pending => None,
    }
}

pub(super) fn deferred_tool_return(record: &DeferredToolRecord) -> Option<ToolReturnPart> {
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "control_flow_resolution".to_string(),
        serde_json::json!("deferred"),
    );
    metadata.insert(
        "deferred_id".to_string(),
        serde_json::json!(record.deferred_id),
    );
    metadata.insert(
        "deferred_status".to_string(),
        serde_json::json!(record.status),
    );
    match record.status {
        ExecutionStatus::Completed => Some(
            ToolReturnPart::new(
                record.tool_call_id.clone(),
                record.tool_name.clone(),
                record.response.clone(),
            )
            .with_metadata(metadata),
        ),
        ExecutionStatus::Failed | ExecutionStatus::Cancelled => Some(
            ToolReturnPart::new(
                record.tool_call_id.clone(),
                record.tool_name.clone(),
                if record.response.is_null() {
                    serde_json::json!({"error": deferred_status_name(record.status)})
                } else {
                    record.response.clone()
                },
            )
            .with_error(true)
            .with_metadata(metadata),
        ),
        ExecutionStatus::Pending | ExecutionStatus::Running | ExecutionStatus::Waiting => None,
    }
}

const fn approval_status_name(status: ApprovalStatus) -> &'static str {
    match status {
        ApprovalStatus::Pending => "pending",
        ApprovalStatus::Approved => "approved",
        ApprovalStatus::Denied => "denied",
        ApprovalStatus::Expired => "expired",
        ApprovalStatus::Cancelled => "cancelled",
    }
}

const fn deferred_status_name(status: ExecutionStatus) -> &'static str {
    match status {
        ExecutionStatus::Pending => "pending",
        ExecutionStatus::Running => "running",
        ExecutionStatus::Waiting => "waiting",
        ExecutionStatus::Completed => "completed",
        ExecutionStatus::Failed => "failed",
        ExecutionStatus::Cancelled => "cancelled",
    }
}
