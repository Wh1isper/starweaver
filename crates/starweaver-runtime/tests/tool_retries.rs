//! Runtime per-tool retry budget tests.

#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use starweaver_model::{
    ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart, TestModel, ToolCallPart,
};
use starweaver_runtime::{Agent, AgentCapability, AgentError, CapabilityResult, RetryEventKind};
use starweaver_tools::{
    FunctionTool, StaticToolset, ToolContext, ToolError, ToolRegistry, ToolResult, Toolset,
};

fn retry_response(call_id: &str, tool_name: &str) -> ModelResponse {
    ModelResponse {
        parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
            id: call_id.to_string(),
            name: tool_name.to_string(),
            arguments: serde_json::json!({"value": call_id}),
        })],
        ..ModelResponse::text("")
    }
}

#[tokio::test]
async fn tool_model_retry_creates_retry_prompt_and_updates_tool_counter() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let calls_clone = calls.clone();
    let tool = FunctionTool::new(
        "flaky",
        Some("Retry once".to_string()),
        serde_json::json!({"type": "object"}),
        move |ctx: ToolContext, args: serde_json::Value| {
            let calls = calls_clone.clone();
            async move {
                if let Ok(mut calls) = calls.lock() {
                    calls.push((ctx.retry, ctx.max_retries));
                }
                if ctx.retry == 0 {
                    Err(ToolError::ModelRetry {
                        tool: "flaky".to_string(),
                        message: "try again with a better value".to_string(),
                    })
                } else {
                    Ok(ToolResult::new(args))
                }
            }
        },
    )
    .with_max_retries(2);
    let model = Arc::new(TestModel::with_responses(vec![
        retry_response("call_1", "flaky"),
        retry_response("call_2", "flaky"),
        ModelResponse::text("done"),
    ]));

    let Ok(result) = Agent::new(model.clone())
        .with_tools(ToolRegistry::new().with_tool(Arc::new(tool)))
        .run("call flaky")
        .await
    else {
        panic!("agent run should succeed after one tool retry");
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
                        && return_part.metadata.get("retry") == Some(&serde_json::json!(1))
                        && return_part.metadata.get("max_retries") == Some(&serde_json::json!(2))
            ))
    )));
}

#[tokio::test]
async fn tool_retry_limit_is_per_tool() {
    let failing = FunctionTool::new(
        "failing",
        Some("Always retry".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            Err(ToolError::ModelRetry {
                tool: "failing".to_string(),
                message: "again".to_string(),
            })
        },
    )
    .with_max_retries(1);
    let other_calls = Arc::new(Mutex::new(0));
    let other_calls_clone = other_calls.clone();
    let other = FunctionTool::new(
        "other",
        Some("Succeeds".to_string()),
        serde_json::json!({"type": "object"}),
        move |_ctx: ToolContext, args: serde_json::Value| {
            let other_calls = other_calls_clone.clone();
            async move {
                if let Ok(mut other_calls) = other_calls.lock() {
                    *other_calls += 1;
                }
                Ok(ToolResult::new(args))
            }
        },
    );
    let model = Arc::new(TestModel::with_responses(vec![
        retry_response("call_1", "failing"),
        retry_response("call_2", "other"),
        retry_response("call_3", "failing"),
        retry_response("call_4", "failing"),
    ]));

    let Err(error) = Agent::new(model)
        .with_tools(
            ToolRegistry::new()
                .with_tool(Arc::new(failing))
                .with_tool(Arc::new(other)),
        )
        .run("call tools")
        .await
    else {
        panic!("failing tool should exhaust its retry budget");
    };

    assert!(matches!(
        error,
        AgentError::ToolRetryLimitExceeded {
            tool,
            max_retries: 1
        } if tool == "failing"
    ));
    assert_eq!(
        other_calls
            .lock()
            .map_or_else(|_| 0, |other_calls| *other_calls),
        1
    );
}

#[tokio::test]
async fn toolset_retry_default_is_used_for_member_tools() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let observed_clone = observed.clone();
    let tool = FunctionTool::new(
        "member",
        Some("Member".to_string()),
        serde_json::json!({"type": "object"}),
        move |ctx: ToolContext, _args: serde_json::Value| {
            let observed = observed_clone.clone();
            async move {
                if let Ok(mut observed) = observed.lock() {
                    observed.push((ctx.retry, ctx.max_retries));
                }
                Err(ToolError::ModelRetry {
                    tool: "member".to_string(),
                    message: "again".to_string(),
                })
            }
        },
    );
    let toolset = StaticToolset::new("set")
        .with_max_retries(2)
        .with_tool(Arc::new(tool));
    let toolset: Arc<dyn Toolset> = Arc::new(toolset);
    let model = Arc::new(TestModel::with_responses(vec![
        retry_response("call_1", "member"),
        retry_response("call_2", "member"),
        retry_response("call_3", "member"),
    ]));

    let Err(error) = Agent::new(model)
        .with_tools(ToolRegistry::new().with_toolset(&toolset))
        .run("call member")
        .await
    else {
        panic!("toolset member should exhaust inherited retry budget");
    };

    assert!(matches!(
        error,
        AgentError::ToolRetryLimitExceeded {
            tool,
            max_retries: 2
        } if tool == "member"
    ));
    assert_eq!(
        observed
            .lock()
            .map_or_else(|_| Vec::new(), |observed| observed.clone()),
        vec![(0, 2), (1, 2), (2, 2)]
    );
}

#[tokio::test]
async fn capability_hooks_observe_tool_execution_and_retry_boundaries() {
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let tool_events = events.clone();
    let tool = FunctionTool::new(
        "flaky",
        Some("Retry once".to_string()),
        serde_json::json!({"type": "object"}),
        move |ctx: ToolContext, args: serde_json::Value| {
            let tool_events = tool_events.clone();
            async move {
                tool_events
                    .lock()
                    .unwrap()
                    .push(format!("execute:{}", ctx.retry));
                if ctx.retry == 0 {
                    Err(ToolError::ModelRetry {
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
        retry_response("call_2", "flaky"),
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
            "after:flaky:true",
            "retry:tool:1:flaky",
            "before:flaky:1",
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
