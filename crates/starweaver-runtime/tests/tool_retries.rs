//! Runtime tool retry semantics tests.

#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use starweaver_model::{
    ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart, TestModel, ToolCallPart,
};
use starweaver_runtime::{Agent, AgentCapability, CapabilityResult, RetryEventKind};
use starweaver_tools::{FunctionTool, ToolContext, ToolError, ToolRegistry, ToolResult};

fn retry_response(call_id: &str, tool_name: &str) -> ModelResponse {
    ModelResponse {
        parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
            id: call_id.to_string(),
            name: tool_name.to_string(),
            arguments: serde_json::json!({"value": call_id}).into(),
        })],
        ..ModelResponse::text("")
    }
}

#[tokio::test]
async fn invalid_arguments_creates_model_retry_prompt_and_updates_tool_counter() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let calls_clone = calls.clone();
    let tool = FunctionTool::new(
        "flaky_args",
        Some("Retry once".to_string()),
        serde_json::json!({"type": "object"}),
        move |ctx: ToolContext, args: serde_json::Value| {
            let calls = calls_clone.clone();
            async move {
                if let Ok(mut calls) = calls.lock() {
                    calls.push((ctx.retry, ctx.max_retries));
                }
                if ctx.retry == 0 {
                    Err(ToolError::InvalidArguments {
                        tool: "flaky_args".to_string(),
                        message: "model supplied invalid arguments".to_string(),
                    })
                } else {
                    Ok(ToolResult::new(args))
                }
            }
        },
    )
    .with_max_retries(2);
    let model = Arc::new(TestModel::with_responses(vec![
        retry_response("call_1", "flaky_args"),
        retry_response("call_2", "flaky_args"),
        ModelResponse::text("done"),
    ]));

    let result = match Agent::new(model.clone())
        .with_tools(ToolRegistry::new().with_tool(Arc::new(tool)))
        .run("call flaky_args")
        .await
    {
        Ok(result) => result,
        Err(error) => {
            panic!("agent run should succeed after one model correction retry: {error}")
        }
    };

    assert_eq!(
        calls
            .lock()
            .map_or_else(|_| Vec::new(), |calls| calls.clone()),
        vec![(0, 2), (1, 2)]
    );
    assert_eq!(result.state.usage.tool_calls, 1);
    let captured = model.captured_messages();
    let retry_request = &captured[1];
    assert!(retry_request.iter().any(|message| matches!(
        message,
        ModelMessage::Request(request)
            if request.parts.iter().any(|part| matches!(
                part,
                ModelRequestPart::ToolReturn(return_part)
                    if return_part.is_error
                        && return_part.metadata.get("error_kind") == Some(&serde_json::json!("invalid_arguments"))
                        && return_part.metadata.get("retry") == Some(&serde_json::json!(1))
                        && return_part.metadata.get("max_retries") == Some(&serde_json::json!(2))
            ))
    )));
}

#[tokio::test]
async fn feedback_is_agent_readable_without_model_retry_budget() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let calls_clone = calls.clone();
    let tool = FunctionTool::new(
        "missing_file_like",
        Some("Diagnostic feedback".to_string()),
        serde_json::json!({"type": "object"}),
        move |ctx: ToolContext, _args: serde_json::Value| {
            let calls = calls_clone.clone();
            async move {
                calls.lock().unwrap().push((ctx.retry, ctx.max_retries));
                Err(ToolError::Feedback {
                    tool: "missing_file_like".to_string(),
                    message: "file not found; verify the path with ls/glob".to_string(),
                })
            }
        },
    )
    .with_max_retries(2);
    let model = Arc::new(TestModel::with_responses(vec![
        retry_response("call_1", "missing_file_like"),
        ModelResponse::text("done"),
    ]));

    let result = match Agent::new(model.clone())
        .with_tools(ToolRegistry::new().with_tool(Arc::new(tool)))
        .run("call missing_file_like")
        .await
    {
        Ok(result) => result,
        Err(error) => panic!("diagnostic tool return should let the model continue: {error}"),
    };

    assert_eq!(result.output, "done");
    assert_eq!(calls.lock().unwrap().as_slice(), &[(0, 2)]);
    let captured = model.captured_messages();
    let followup_request = &captured[1];
    assert!(followup_request.iter().any(|message| matches!(
        message,
        ModelMessage::Request(request)
            if request.parts.iter().any(|part| matches!(
                part,
                ModelRequestPart::ToolReturn(return_part)
                    if !return_part.is_error
                        && return_part.metadata.get("error_kind") == Some(&serde_json::json!("feedback"))
                        && return_part.metadata.get("runtime_retryable") == Some(&serde_json::json!(false))
            ))
    )));
}

#[tokio::test]
async fn user_error_is_error_without_model_or_internal_retry() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let calls_clone = calls.clone();
    let tool = FunctionTool::new(
        "bad_wiring",
        Some("Developer wiring error".to_string()),
        serde_json::json!({"type": "object"}),
        move |ctx: ToolContext, _args: serde_json::Value| {
            let calls = calls_clone.clone();
            async move {
                calls.lock().unwrap().push((ctx.retry, ctx.max_retries));
                Err(ToolError::UserError {
                    tool: "bad_wiring".to_string(),
                    message: "missing runtime dependency".to_string(),
                })
            }
        },
    )
    .with_max_retries(2);
    let model = Arc::new(TestModel::with_responses(vec![
        retry_response("call_1", "bad_wiring"),
        ModelResponse::text("done"),
    ]));

    let result = match Agent::new(model.clone())
        .with_tools(ToolRegistry::new().with_tool(Arc::new(tool)))
        .run("call bad_wiring")
        .await
    {
        Ok(result) => result,
        Err(error) => {
            panic!("user errors are model-visible tool returns, not retry prompts: {error}")
        }
    };

    assert_eq!(result.output, "done");
    assert_eq!(calls.lock().unwrap().as_slice(), &[(0, 2)]);
    let captured = model.captured_messages();
    assert_eq!(captured.len(), 2);
    let followup_request = &captured[1];
    assert!(followup_request.iter().any(|message| matches!(
        message,
        ModelMessage::Request(request)
            if request.parts.iter().any(|part| matches!(
                part,
                ModelRequestPart::ToolReturn(return_part)
                    if return_part.is_error
                        && return_part.metadata.get("error_kind") == Some(&serde_json::json!("user_error"))
                        && return_part.metadata.get("runtime_retryable") == Some(&serde_json::json!(false))
                        && return_part.metadata.get("unexpected") == Some(&serde_json::json!(false))
            ))
    )));
}

#[tokio::test]
async fn unexpected_execution_error_retries_inside_tool_registry_without_model_retry() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let calls_clone = calls.clone();
    let tool = FunctionTool::new(
        "transient",
        Some("Transient failure".to_string()),
        serde_json::json!({"type": "object"}),
        move |ctx: ToolContext, args: serde_json::Value| {
            let calls = calls_clone.clone();
            async move {
                calls.lock().unwrap().push((ctx.retry, ctx.max_retries));
                if ctx.retry < 2 {
                    Err(ToolError::Execution {
                        tool: "transient".to_string(),
                        message: "temporary provider failure".to_string(),
                    })
                } else {
                    Ok(ToolResult::new(args))
                }
            }
        },
    );
    let model = Arc::new(TestModel::with_responses(vec![
        retry_response("call_1", "transient"),
        ModelResponse::text("done"),
    ]));

    let result = match Agent::new(model.clone())
        .with_tools(ToolRegistry::new().with_tool(Arc::new(tool)))
        .run("call transient")
        .await
    {
        Ok(result) => result,
        Err(error) => {
            panic!("internal registry retry should recover transient tool failure: {error}")
        }
    };

    assert_eq!(result.output, "done");
    assert_eq!(calls.lock().unwrap().as_slice(), &[(0, 3), (1, 3), (2, 3)]);
    let captured = model.captured_messages();
    assert_eq!(captured.len(), 2);
}

#[tokio::test]
async fn capability_hooks_observe_final_tool_boundary_not_internal_retries() {
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let tool_events = events.clone();
    let tool = FunctionTool::new(
        "flaky",
        Some("Retry inside registry".to_string()),
        serde_json::json!({"type": "object"}),
        move |ctx: ToolContext, args: serde_json::Value| {
            let tool_events = tool_events.clone();
            async move {
                tool_events
                    .lock()
                    .unwrap()
                    .push(format!("execute:{}", ctx.retry));
                if ctx.retry == 0 {
                    Err(ToolError::Execution {
                        tool: "flaky".to_string(),
                        message: "again".to_string(),
                    })
                } else {
                    Ok(ToolResult::new(args))
                }
            }
        },
    )
    .with_max_retries(1);
    let model = Arc::new(TestModel::with_responses(vec![
        retry_response("call_1", "flaky"),
        ModelResponse::text("done"),
    ]));
    let hook = Arc::new(ToolBoundaryRecorder {
        events: events.clone(),
    });

    let result = Agent::new(model)
        .with_tools(ToolRegistry::new().with_tool(Arc::new(tool)))
        .with_capability(hook)
        .run("call flaky")
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert_eq!(
        events.lock().unwrap().as_slice(),
        [
            "before:flaky:0",
            "execute:0",
            "execute:1",
            "after:flaky:false",
        ]
    );
}

struct ToolBoundaryRecorder {
    events: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl AgentCapability for ToolBoundaryRecorder {
    async fn before_tool_execution(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        tool_context: &mut ToolContext,
        call: &ToolCallPart,
    ) -> CapabilityResult<()> {
        self.events
            .lock()
            .unwrap()
            .push(format!("before:{}:{}", call.name, tool_context.retry));
        Ok(())
    }

    async fn after_tool_result(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        call: &ToolCallPart,
        tool_return: &mut starweaver_model::ToolReturnPart,
    ) -> CapabilityResult<()> {
        self.events
            .lock()
            .unwrap()
            .push(format!("after:{}:{}", call.name, tool_return.is_error));
        Ok(())
    }

    async fn on_retry(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        kind: RetryEventKind,
        retries: usize,
        message: &str,
    ) -> CapabilityResult<()> {
        self.events.lock().unwrap().push(format!(
            "retry:{}:{retries}:{message}",
            match kind {
                RetryEventKind::Output => "output",
                RetryEventKind::Tool => "tool",
            }
        ));
        Ok(())
    }
}
