use std::sync::Arc;

use serde_json::json;
use starweaver_agent::{json_tool, FunctionModel, StaticToolset, ToolError, ToolResult};
use starweaver_model::{
    context_origin_metadata, ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart,
    ToolCallPart,
};

pub(super) fn local_echo_model() -> FunctionModel {
    FunctionModel::new(move |messages, _settings, _info| {
        let prompt = latest_user_prompt(&messages).unwrap_or_default();
        Ok(ModelResponse::text(format!("local echo: {prompt}")))
    })
}

#[cfg(test)]
pub(super) fn capture_subagent_inheritance_model() -> FunctionModel {
    FunctionModel::new(move |messages, settings, _info| {
        let Some(settings) = settings else {
            return Err(starweaver_model::ModelError::Transport(
                "missing inherited model settings".to_string(),
            ));
        };
        if settings
            .provider_options
            .as_ref()
            .and_then(|options| options.get("store"))
            != Some(&json!(false))
        {
            return Err(starweaver_model::ModelError::Transport(format!(
                "missing inherited store=false in settings: {settings:?}"
            )));
        }
        let rendered = format!("{messages:?}");
        if !rendered.contains("<context-window>200000</context-window>") {
            return Err(starweaver_model::ModelError::Transport(format!(
                "missing inherited context window in messages: {rendered}"
            )));
        }
        let prompt = latest_user_prompt(&messages).unwrap_or_default();
        Ok(ModelResponse::text(format!("captured: {prompt}")))
    })
}

pub(super) fn scripted_tool_model(tool_name: &'static str) -> FunctionModel {
    FunctionModel::new(move |messages, _settings, _info| {
        if messages.iter().any(message_has_tool_return) {
            return Ok(ModelResponse::text(format!("{tool_name} handled")));
        }
        Ok(ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: format!("{tool_name}_call"),
                name: tool_name.to_string(),
                arguments: json!({"action": tool_name}).into(),
            })],
            usage: starweaver_usage::Usage::default(),
            model_name: Some(tool_name.to_string()),
            provider: None,
            finish_reason: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::default(),
        })
    })
}

fn latest_user_prompt(messages: &[ModelMessage]) -> Option<String> {
    messages.iter().rev().find_map(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().rev().find_map(|part| match part {
            ModelRequestPart::UserPrompt {
                content, metadata, ..
            } if context_origin_metadata(metadata).is_none() => {
                content.iter().find_map(|part| match part {
                    starweaver_model::ContentPart::Text { text } => Some(text.clone()),
                    _ => None,
                })
            }
            _ => None,
        }),
        ModelMessage::Response(_) => None,
    })
}

fn message_has_tool_return(message: &ModelMessage) -> bool {
    match message {
        ModelMessage::Request(request) => request
            .parts
            .iter()
            .any(|part| matches!(part, ModelRequestPart::ToolReturn(_))),
        ModelMessage::Response(_) => false,
    }
}

pub(super) fn control_flow_toolset() -> StaticToolset {
    let approval_tool = json_tool(
        "approval_probe",
        Some("Deterministic approval probe".to_string()),
        json!({"type": "object"}),
        |_context, arguments| async move {
            Err(ToolError::ApprovalRequired {
                tool: "approval_probe".to_string(),
                metadata: json!({"arguments": arguments, "reason": "cli approval probe"}),
            })
        },
    );
    let deferred_tool = json_tool(
        "deferred_probe",
        Some("Deterministic deferred-call probe".to_string()),
        json!({"type": "object"}),
        |_context, arguments| async move {
            Err(ToolError::CallDeferred {
                tool: "deferred_probe".to_string(),
                metadata: json!({"arguments": arguments, "reason": "cli deferred probe"}),
            })
        },
    );
    StaticToolset::new("cli_control_flow")
        .with_id("cli_control_flow")
        .with_tool(Arc::new(approval_tool))
        .with_tool(Arc::new(deferred_tool))
}

#[allow(dead_code)]
fn ok_tool_result(value: serde_json::Value) -> ToolResult {
    ToolResult::new(value)
}
