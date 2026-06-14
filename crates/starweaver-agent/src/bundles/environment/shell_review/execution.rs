//! Shell command review execution flow.

use serde_json::{Map, Value};
use starweaver_context::{AgentContext, AgentContextHandle};
use starweaver_core::Usage;
use starweaver_model::{
    ContentPart, ModelMessage, ModelRequest, ModelRequestContext, ModelRequestParameters,
    ModelRequestPart, ModelResponse, OutputMode,
};
use starweaver_tools::{ToolApprovalState, ToolContext, ToolError, ToolResult};

use crate::bundles::helpers::tool_execution_error;

use super::parsing::parse_shell_review_decision;
use super::{
    ShellReviewConfig, ShellReviewContextSnapshot, ShellReviewDecision, ShellReviewHandle,
    ShellReviewPreviousDecision, ShellReviewRecord, ShellReviewRequest, ShellReviewRiskLevel,
    DEFAULT_SHELL_REVIEW_PROMPT,
};

/// Review a shell command and return a blocked result when policy denies execution.
pub async fn review_shell_command_or_block(
    context: &ToolContext,
    command: &str,
    cwd: Option<&str>,
    background: bool,
    mut environment_keys: Vec<String>,
    timeout_seconds: u64,
    mut snapshot: ShellReviewContextSnapshot,
) -> Result<Option<ToolResult>, ToolError> {
    let Some(agent_context) = context.dependency::<AgentContext>() else {
        return Ok(None);
    };
    let Some(handle) = agent_context.dependencies.get::<ShellReviewHandle>() else {
        return Ok(None);
    };
    let tool_call_id = tool_call_id(context);
    let tool_call_approved = matches!(context.approval, Some(ToolApprovalState::Approved { .. }));
    if snapshot.timeout_seconds.is_none() {
        snapshot.timeout_seconds = Some(timeout_seconds);
    }
    snapshot.tool_call_id.clone_from(&tool_call_id);
    snapshot.tool_call_approved = tool_call_approved;
    environment_keys.sort();

    let mut request = ShellReviewRequest {
        command: command.to_string(),
        cwd: cwd.map(str::to_string),
        background,
        environment_keys,
        context_snapshot: Some(snapshot),
        previous_reviews: Vec::new(),
    };

    let fingerprint = request.command_fingerprint();
    if tool_call_approved {
        handle.update_last_matching_approval(tool_call_id.as_deref(), &fingerprint);
        return Ok(None);
    }

    request.previous_reviews = previous_shell_reviews(&handle, &request, tool_call_id.as_deref());
    let decision = review_shell_command(context, &handle, &request).await?;
    let mut record =
        ShellReviewRecord::new(request.clone(), decision.clone(), tool_call_id.clone());
    if !decision.requires_approval(handle.config()) {
        record.approved = true;
        handle.push_record(record);
        return Ok(None);
    }
    handle.push_record(record);

    let metadata = request.to_approval_metadata(&decision);
    if decision.requires_defer(handle.config()) {
        return Err(ToolError::ApprovalRequired {
            tool: "shell_exec".to_string(),
            metadata,
        });
    }
    if decision.requires_deny(handle.config()) {
        return Ok(Some(ToolResult::new(serde_json::json!({
            "stdout": "",
            "stderr": "",
            "return_code": 1,
            "error": format!("Shell command blocked by review: {}", decision.reason),
            "shell_review": decision,
        }))));
    }
    Ok(None)
}

fn previous_shell_reviews(
    handle: &ShellReviewHandle,
    request: &ShellReviewRequest,
    tool_call_id: Option<&str>,
) -> Vec<ShellReviewPreviousDecision> {
    let records = handle.records();
    let fingerprint = request.command_fingerprint();
    let mut previous = Vec::new();
    let mut seen = Vec::<usize>::new();
    for pass in 0..3 {
        for (index, record) in records.iter().enumerate().rev() {
            if seen.contains(&index) {
                continue;
            }
            let matches = match pass {
                0 => tool_call_id.is_some() && record.tool_call_id.as_deref() == tool_call_id,
                1 => record.request.command_fingerprint() == fingerprint,
                _ => true,
            };
            if !matches {
                continue;
            }
            previous.push(ShellReviewPreviousDecision {
                approved: record.approved,
                risk_level: record.decision.risk_level,
                reason: record.decision.reason.clone(),
                command: Some(record.request.command.clone()),
                cwd: record.request.cwd.clone(),
            });
            seen.push(index);
        }
    }
    previous
}

async fn review_shell_command(
    context: &ToolContext,
    handle: &ShellReviewHandle,
    request: &ShellReviewRequest,
) -> Result<ShellReviewDecision, ToolError> {
    let config = handle.config();
    let Some(model) = config.model.as_ref().filter(|_| config.enabled) else {
        return Ok(ShellReviewDecision {
            risk_level: ShellReviewRiskLevel::Low,
            reason: "Shell review is disabled.".to_string(),
        });
    };

    let response = model
        .request_stream_final(
            vec![ModelMessage::Request(shell_review_model_request(
                config, request,
            ))],
            config.model_settings.clone(),
            shell_review_request_params(),
            ModelRequestContext::new(context.run_id.clone(), context.conversation_id.clone()),
        )
        .await
        .map_err(|error| {
            tool_execution_error("shell_exec", format!("Shell review failed: {error}"))
        })?;
    record_shell_review_usage(context, &response);
    parse_shell_review_decision(&response).ok_or_else(|| {
        tool_execution_error(
            "shell_exec",
            format!(
                "Shell review model returned an invalid decision: {}",
                response.text_output()
            ),
        )
    })
}

fn shell_review_model_request(
    config: &ShellReviewConfig,
    request: &ShellReviewRequest,
) -> ModelRequest {
    ModelRequest {
        parts: vec![
            ModelRequestPart::SystemPrompt {
                text: config
                    .system_prompt
                    .clone()
                    .unwrap_or_else(|| DEFAULT_SHELL_REVIEW_PROMPT.to_string()),
                metadata: Map::new(),
            },
            ModelRequestPart::UserPrompt {
                content: vec![ContentPart::Text {
                    text: request.to_prompt(),
                }],
                name: Some("shell_review".to_string()),
                metadata: Map::new(),
            },
        ],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Map::new(),
    }
}

fn shell_review_request_params() -> ModelRequestParameters {
    ModelRequestParameters {
        output_schema: Some(serde_json::json!({
            "type": "object",
            "properties": {
                "risk_level": {
                    "type": "string",
                    "enum": ["low", "medium", "high", "extra_high"]
                },
                "reason": {"type": "string"}
            },
            "required": ["risk_level", "reason"],
            "additionalProperties": false
        })),
        output_mode: Some(OutputMode::Prompted),
        ..ModelRequestParameters::default()
    }
}

fn record_shell_review_usage(context: &ToolContext, response: &ModelResponse) {
    if response.usage == Usage::default() {
        return;
    }
    if let Some(handle) = context.dependency::<AgentContextHandle>() {
        let mut snapshot = handle.snapshot();
        snapshot.add_usage(&response.usage);
        handle.replace(snapshot);
    }
}

fn tool_call_id(context: &ToolContext) -> Option<String> {
    context
        .metadata
        .get("tool_call_id")
        .or_else(|| context.metadata.get("starweaver.tool_call_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}
