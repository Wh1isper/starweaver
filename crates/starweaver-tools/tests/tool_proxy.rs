#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use serde_json::json;
use starweaver_core::{ConversationId, RunId};
use starweaver_tools::{
    string_tool, tool_proxy_toolset, FunctionTool, StaticToolset, ToolContext, ToolError,
    ToolInstruction, ToolResult,
};

fn context() -> ToolContext {
    ToolContext::new(RunId::from_string("run_test"), ConversationId::new(), 0)
}

fn result_content(result: &ToolResult) -> String {
    result.content["content"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn proxy_searches_namespaces_and_calls_tools() {
    let lookup = FunctionTool::new(
        "lookup_docs",
        Some("Look up documentation by topic".to_string()),
        json!({"type":"object","properties":{"topic":{"type":"string"}}}),
        |_ctx: ToolContext, args: serde_json::Value| async move {
            Ok(ToolResult::new(json!({"looked_up": args["topic"]})))
        },
    );
    let hidden_proxy_name = FunctionTool::new(
        "search_tools",
        Some("wrapped proxy helper name should be skipped".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move { Ok(ToolResult::new(json!(null))) },
    );
    let toolset = StaticToolset::new("docs")
        .with_id("docs_ns")
        .with_tool(Arc::new(lookup))
        .with_tool(Arc::new(hidden_proxy_name))
        .with_instruction(ToolInstruction::new("docs", "Documentation tools."));
    let proxy = tool_proxy_toolset(vec![Arc::new(toolset)]);
    let tools = proxy.get_tools();
    assert_eq!(tools.len(), 2);
    assert_eq!(proxy.max_retries(), Some(3));
    assert!(proxy
        .get_instructions()
        .into_iter()
        .any(|instruction| instruction.content.contains("docs_ns")));

    let search = tools
        .iter()
        .find(|tool| tool.name() == "search_tools")
        .unwrap();
    let search_output = result_content(
        &search
            .call(context(), json!({"query":"docs"}))
            .await
            .unwrap(),
    );
    assert!(search_output.contains("lookup_docs"));
    assert!(search_output.contains("namespace=\"docs_ns\""));
    assert!(!search_output.contains("wrapped proxy helper name"));

    let call = tools
        .iter()
        .find(|tool| tool.name() == "call_tool")
        .unwrap();
    let called = call
        .call(
            context(),
            json!({"name":"lookup_docs","arguments":{"topic":"agents"}}),
        )
        .await
        .unwrap();
    assert_eq!(called.content["looked_up"], "agents");
}

#[tokio::test]
async fn proxy_returns_xml_for_empty_unknown_and_execution_errors() {
    let failing = FunctionTool::new(
        "fail_tool",
        Some("Fails for testing".to_string()),
        json!({"type":"object","properties":{"value":{"type":"string"}}}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            Err(ToolError::Execution {
                tool: "fail_tool".to_string(),
                message: "bad <input> & quote\"".to_string(),
            })
        },
    );
    let toolset = StaticToolset::new("ops")
        .with_id("ops")
        .with_tool(Arc::new(failing));
    let proxy = tool_proxy_toolset(vec![Arc::new(toolset)]);
    let tools = proxy.get_tools();
    let search = tools
        .iter()
        .find(|tool| tool.name() == "search_tools")
        .unwrap();
    assert!(
        result_content(&search.call(context(), json!({"query":""})).await.unwrap())
            .contains("Parameter 'query' is required")
    );
    assert!(result_content(
        &search
            .call(context(), json!({"query":"missing"}))
            .await
            .unwrap()
    )
    .contains("No tools found"));

    let call = tools
        .iter()
        .find(|tool| tool.name() == "call_tool")
        .unwrap();
    assert!(
        result_content(&call.call(context(), json!({"name":""})).await.unwrap())
            .contains("Parameter 'name' is required")
    );
    assert!(result_content(
        &call
            .call(context(), json!({"name":"unknown","arguments":{}}))
            .await
            .unwrap()
    )
    .contains("not found"));
    let error = result_content(
        &call
            .call(
                context(),
                json!({"name":"fail_tool","arguments":{"value":"x"}}),
            )
            .await
            .unwrap(),
    );
    assert!(error.contains("tool-call-error"));
    assert!(error.contains("&lt;input&gt;"));
    assert!(error.contains("&amp;"));
}

#[tokio::test]
async fn proxy_propagates_approval_and_deferred_errors() {
    let approval = string_tool(
        "approval_tool",
        Some("Needs approval".to_string()),
        json!({"type":"object"}),
        |_ctx, _args| async move {
            Err(ToolError::ApprovalRequired {
                tool: "approval_tool".to_string(),
                metadata: json!({"reason":"review"}),
            })
        },
    );
    let deferred = string_tool(
        "deferred_tool",
        Some("Defers work".to_string()),
        json!({"type":"object"}),
        |_ctx, _args| async move {
            Err(ToolError::CallDeferred {
                tool: "deferred_tool".to_string(),
                metadata: json!({"worker":"remote"}),
            })
        },
    );
    let toolset = StaticToolset::new("control")
        .with_id("control")
        .with_tool(Arc::new(approval))
        .with_tool(Arc::new(deferred));
    let proxy = tool_proxy_toolset(vec![Arc::new(toolset)]);
    let call = proxy
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "call_tool")
        .unwrap();

    let approval_error = call
        .call(context(), json!({"name":"approval_tool","arguments":{}}))
        .await
        .unwrap_err();
    assert!(matches!(approval_error, ToolError::ApprovalRequired { .. }));

    let deferred_error = call
        .call(context(), json!({"name":"deferred_tool","arguments":{}}))
        .await
        .unwrap_err();
    assert!(matches!(deferred_error, ToolError::CallDeferred { .. }));
}
