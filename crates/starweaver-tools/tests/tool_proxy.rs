#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use serde_json::json;
use starweaver_core::{ConversationId, RunId};
use starweaver_tools::{
    dynamic_tool_proxy, json_tool, FunctionTool, StaticToolset, ToolContext, ToolError,
    ToolInstruction, ToolProxyToolset, ToolResult, Toolset,
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
    let proxy = dynamic_tool_proxy(vec![Arc::new(toolset)]);
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
async fn proxy_supports_prefixed_visible_tools_and_instructions() {
    let lookup = FunctionTool::new(
        "lookup_docs",
        Some("Look up documentation by topic".to_string()),
        json!({"type":"object","properties":{"topic":{"type":"string"}}}),
        |_ctx: ToolContext, args: serde_json::Value| async move {
            Ok(ToolResult::new(json!({"looked_up": args["topic"]})))
        },
    );
    let hidden_unprefixed_proxy_name = FunctionTool::new(
        "search_tools",
        Some("unprefixed proxy helper name remains an ordinary wrapped tool".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move { Ok(ToolResult::new(json!(null))) },
    );
    let hidden_prefixed_proxy_name = FunctionTool::new(
        "mcp_search_tool",
        Some("prefixed proxy helper name should be skipped".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move { Ok(ToolResult::new(json!(null))) },
    );
    let toolset = StaticToolset::new("docs")
        .with_id("docs_ns")
        .with_tool(Arc::new(lookup))
        .with_tool(Arc::new(hidden_unprefixed_proxy_name))
        .with_tool(Arc::new(hidden_prefixed_proxy_name));

    let proxy = ToolProxyToolset::new(vec![Arc::new(toolset)])
        .try_with_name_prefix("__mcp__")
        .unwrap();
    assert_eq!(proxy.name(), "tool_proxy");
    assert_eq!(proxy.prefix(), Some("mcp"));
    assert_eq!(proxy.search_tool_name(), "mcp_search_tool");
    assert_eq!(proxy.call_tool_name(), "mcp_call_tool");
    assert!(proxy.get_instructions().into_iter().any(|instruction| {
        instruction.group == "mcp-tool-proxy"
            && instruction.content.contains("mcp_search_tool")
            && instruction.content.contains("mcp_call_tool")
            && !instruction.content.contains("search_tools")
    }));

    let tools = proxy.get_tools();
    assert_eq!(
        tools.iter().map(|tool| tool.name()).collect::<Vec<_>>(),
        vec!["mcp_search_tool", "mcp_call_tool"]
    );

    let search = tools
        .iter()
        .find(|tool| tool.name() == "mcp_search_tool")
        .unwrap();
    let search_output = result_content(
        &search
            .call(context(), json!({"query":"docs"}))
            .await
            .unwrap(),
    );
    assert!(search_output.contains("lookup_docs"));
    assert!(search_output.contains("search_tools"));
    assert!(!search_output.contains("name=\"mcp_search_tool\""));
    assert!(!search_output.contains("prefixed proxy helper name should be skipped"));

    let call = tools
        .iter()
        .find(|tool| tool.name() == "mcp_call_tool")
        .unwrap();
    let called = call
        .call(
            context(),
            json!({"name":"lookup_docs","arguments":{"topic":"agents"}}),
        )
        .await
        .unwrap();
    assert_eq!(called.content["looked_up"], "agents");

    let missing = result_content(
        &call
            .call(context(), json!({"name":"missing","arguments":{}}))
            .await
            .unwrap(),
    );
    assert!(missing.contains("Use mcp_search_tool to discover available tools"));
}

#[test]
fn proxy_rejects_invalid_prefixes() {
    let Err(error) = ToolProxyToolset::new(vec![]).try_with_name_prefix("mcp-server") else {
        panic!("invalid prefix should be rejected");
    };
    assert_eq!(error.prefix(), "mcp-server");
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
    let proxy = dynamic_tool_proxy(vec![Arc::new(toolset)]);
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
    let approval = json_tool(
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
    let deferred = json_tool(
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
    let proxy = dynamic_tool_proxy(vec![Arc::new(toolset)]);
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
