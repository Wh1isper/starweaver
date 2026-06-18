#![allow(missing_docs, clippy::unwrap_used)]

use std::{
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use starweaver_agent::{
    live_mcp_toolset, AgentBuilder, AgentContext, LiveMcpClient, LiveMcpError,
    LiveMcpServerSnapshot, McpPromptSpec, McpResourceSpec, McpSamplingSpec, McpSubscriptionSpec,
    McpToolSpec, McpTransport, RmcpLiveMcpClient, TestModel, ToolContext, ToolRegistry, ToolResult,
    TOOLSET_CLOSED_EVENT_KIND, TOOLSET_INITIALIZED_EVENT_KIND,
};
use starweaver_core::{ConversationId, RunId};
use starweaver_model::ToolCallPart;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};

#[tokio::test]
async fn live_mcp_adapter_discovers_toolset() {
    struct FakeMcp;

    #[async_trait]
    impl LiveMcpClient for FakeMcp {
        async fn discover(
            &self,
            id: &str,
            _transport: &McpTransport,
        ) -> Result<LiveMcpServerSnapshot, LiveMcpError> {
            Ok(LiveMcpServerSnapshot::new(id)
                .with_instructions("Use local MCP tools.")
                .with_tool(McpToolSpec::new(
                    "lookup",
                    serde_json::json!({"type": "object"}),
                )))
        }
    }

    let toolset = live_mcp_toolset(
        Arc::new(FakeMcp),
        "local",
        McpTransport::stdio("fake-server"),
    )
    .await
    .unwrap();

    assert_eq!(toolset.name(), "local");
    assert_eq!(toolset.get_tools()[0].name(), "lookup");
    assert_eq!(toolset.get_instructions().len(), 1);
}

#[tokio::test]
async fn live_mcp_discovered_tool_defers_with_mcp_metadata() {
    struct FakeMcp;

    #[async_trait]
    impl LiveMcpClient for FakeMcp {
        async fn discover(
            &self,
            id: &str,
            _transport: &McpTransport,
        ) -> Result<LiveMcpServerSnapshot, LiveMcpError> {
            Ok(LiveMcpServerSnapshot::new(id).with_tool(McpToolSpec::new(
                "lookup",
                serde_json::json!({"type": "object"}),
            )))
        }
    }

    let toolset = live_mcp_toolset(
        Arc::new(FakeMcp),
        "local",
        McpTransport::stdio("fake-server"),
    )
    .await
    .unwrap();
    let registry = ToolRegistry::new().with_toolset(&toolset);
    let call = ToolCallPart {
        id: "call_lookup".to_string(),
        name: "lookup".to_string(),
        arguments: serde_json::json!({"query": "docs"}).into(),
    };

    let result = registry
        .execute_call(
            ToolContext::new(RunId::new(), ConversationId::new(), 0),
            &call,
        )
        .await;

    assert!(result.is_error);
    assert_eq!(result.content["kind"], "call_deferred");
    assert_eq!(result.metadata["control_flow"], "call_deferred");
    assert_eq!(result.metadata["deferred"]["kind"], "mcp_tool_call");
    assert_eq!(result.metadata["deferred"]["server_id"], "local");
    assert_eq!(result.metadata["deferred"]["tool_name"], "lookup");
    assert_eq!(result.metadata["deferred"]["arguments"]["query"], "docs");
}

#[tokio::test]
async fn live_mcp_discovered_tool_executes_through_host_client() {
    struct ExecutingMcp {
        calls: Arc<Mutex<Vec<serde_json::Value>>>,
    }

    #[async_trait]
    impl LiveMcpClient for ExecutingMcp {
        async fn discover(
            &self,
            id: &str,
            _transport: &McpTransport,
        ) -> Result<LiveMcpServerSnapshot, LiveMcpError> {
            Ok(LiveMcpServerSnapshot::new(id).with_tool(
                McpToolSpec::new("lookup", serde_json::json!({"type": "object"}))
                    .with_description("Look up a value"),
            ))
        }

        async fn call_tool(
            &self,
            context: ToolContext,
            id: &str,
            transport: &McpTransport,
            tool_name: &str,
            arguments: serde_json::Value,
        ) -> Result<ToolResult, LiveMcpError> {
            self.calls.lock().unwrap().push(serde_json::json!({
                "run_step": context.run_step,
                "server_id": id,
                "transport": transport.kind(),
                "tool_name": tool_name,
                "arguments": arguments,
            }));
            Ok(ToolResult::new(serde_json::json!({
                "answer": "found"
            })))
        }
    }

    let calls = Arc::new(Mutex::new(Vec::new()));
    let toolset = live_mcp_toolset(
        Arc::new(ExecutingMcp {
            calls: calls.clone(),
        }),
        "local",
        McpTransport::stdio("fake-server"),
    )
    .await
    .unwrap();
    let registry = ToolRegistry::new().with_toolset(&toolset);
    let call = ToolCallPart {
        id: "call_lookup".to_string(),
        name: "lookup".to_string(),
        arguments: serde_json::json!({"query": "docs"}).into(),
    };

    let result = registry
        .execute_call(
            ToolContext::new(RunId::new(), ConversationId::new(), 7),
            &call,
        )
        .await;

    assert!(!result.is_error);
    assert_eq!(result.content["answer"], "found");
    assert_eq!(result.metadata["mcp_server_id"], "local");
    assert_eq!(result.metadata["mcp_transport"], "stdio");
    assert_eq!(result.metadata["mcp_tool_name"], "lookup");
    assert_eq!(
        *calls.lock().unwrap(),
        vec![serde_json::json!({
            "run_step": 7,
            "server_id": "local",
            "transport": "stdio",
            "tool_name": "lookup",
            "arguments": {"query": "docs"},
        })]
    );
}

#[tokio::test]
async fn rmcp_stdio_client_discovers_executes_and_closes_fixture_server() {
    let fixture = format!(
        "{}/tests/fixtures/rmcp_stdio_server.py",
        env!("CARGO_MANIFEST_DIR")
    );
    let transport = McpTransport::stdio("python3").with_args(vec![fixture]);
    let client = Arc::new(RmcpLiveMcpClient::new());
    let toolset = live_mcp_toolset(client.clone(), "rmcp-fixture", transport.clone())
        .await
        .unwrap();

    assert_eq!(toolset.name(), "rmcp-fixture");
    assert_eq!(toolset.get_tools()[0].name(), "lookup");
    assert_eq!(
        toolset.get_instructions()[0].content,
        "Use fixture MCP tools."
    );
    let registry = ToolRegistry::new().with_toolset(&toolset);
    let call = ToolCallPart {
        id: "call_lookup".to_string(),
        name: "lookup".to_string(),
        arguments: serde_json::json!({"query": "docs"}).into(),
    };

    let result = registry
        .execute_call(
            ToolContext::new(RunId::new(), ConversationId::new(), 3),
            &call,
        )
        .await;

    assert!(!result.is_error);
    assert_eq!(result.content["answer"], "fixture result");
    assert_eq!(result.content["query"], "docs");
    assert_eq!(result.metadata["mcp_server_id"], "rmcp-fixture");
    assert_eq!(result.metadata["mcp_transport"], "stdio");
    assert_eq!(result.metadata["mcp_tool_name"], "lookup");
    assert_eq!(result.metadata["rmcp_live"], true);

    client.close("rmcp-fixture", &transport).await.unwrap();
}

#[tokio::test]
async fn rmcp_streamable_http_client_discovers_executes_and_closes_fixture_server() {
    let fixture = format!(
        "{}/tests/fixtures/rmcp_streamable_http_server.py",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut child = Command::new("python3")
        .arg(fixture)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();
    let url = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let transport = McpTransport::streamable_http(url);
    let client = Arc::new(RmcpLiveMcpClient::new());
    let toolset = live_mcp_toolset(client.clone(), "rmcp-http-fixture", transport.clone())
        .await
        .unwrap();

    assert_eq!(toolset.name(), "rmcp-http-fixture");
    assert_eq!(toolset.get_tools()[0].name(), "lookup");
    assert_eq!(
        toolset.get_instructions()[0].content,
        "Use HTTP fixture MCP tools."
    );
    let registry = ToolRegistry::new().with_toolset(&toolset);
    let call = ToolCallPart {
        id: "call_lookup_http".to_string(),
        name: "lookup".to_string(),
        arguments: serde_json::json!({"query": "docs"}).into(),
    };

    let result = registry
        .execute_call(
            ToolContext::new(RunId::new(), ConversationId::new(), 4),
            &call,
        )
        .await;

    assert!(!result.is_error);
    assert_eq!(result.content["answer"], "http fixture result");
    assert_eq!(result.content["query"], "docs");
    assert_eq!(result.metadata["mcp_server_id"], "rmcp-http-fixture");
    assert_eq!(result.metadata["mcp_transport"], "streamable_http");
    assert_eq!(result.metadata["mcp_tool_name"], "lookup");
    assert_eq!(result.metadata["rmcp_live"], true);

    client.close("rmcp-http-fixture", &transport).await.unwrap();
    child.kill().await.unwrap();
    let _ = child.wait().await;
}

#[tokio::test]
async fn rmcp_streamable_http_client_reinitializes_after_expired_session() {
    let fixture = format!(
        "{}/tests/fixtures/rmcp_streamable_http_reconnect_server.py",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut child = Command::new("python3")
        .arg(fixture)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();
    let url = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let transport = McpTransport::streamable_http(url);
    let client = Arc::new(RmcpLiveMcpClient::new());
    let toolset = live_mcp_toolset(client.clone(), "rmcp-reconnect-fixture", transport.clone())
        .await
        .unwrap();
    let registry = ToolRegistry::new().with_toolset(&toolset);
    let call = ToolCallPart {
        id: "call_lookup_reconnect".to_string(),
        name: "lookup".to_string(),
        arguments: serde_json::json!({"query": "docs"}).into(),
    };

    let result = registry
        .execute_call(
            ToolContext::new(RunId::new(), ConversationId::new(), 5),
            &call,
        )
        .await;

    assert!(!result.is_error);
    assert_eq!(result.content["answer"], "reconnected result");
    assert_eq!(result.content["query"], "docs");
    assert_eq!(result.content["session"], "session-2");
    assert_eq!(result.content["initialize_count"], 2);
    assert_eq!(result.metadata["mcp_transport"], "streamable_http");
    assert_eq!(result.metadata["rmcp_live"], true);

    client
        .close("rmcp-reconnect-fixture", &transport)
        .await
        .unwrap();
    child.kill().await.unwrap();
    let _ = child.wait().await;
}

#[tokio::test]
async fn live_mcp_toolset_closes_host_client_on_run_exit() {
    struct ClosingMcp {
        closed: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl LiveMcpClient for ClosingMcp {
        async fn discover(
            &self,
            id: &str,
            _transport: &McpTransport,
        ) -> Result<LiveMcpServerSnapshot, LiveMcpError> {
            Ok(LiveMcpServerSnapshot::new(id)
                .with_tool(McpToolSpec::new(
                    "lookup",
                    serde_json::json!({"type": "object"}),
                ))
                .with_resource(
                    McpResourceSpec::new("resource://docs/index")
                        .with_name("Docs")
                        .with_mime_type("text/markdown"),
                )
                .with_prompt(McpPromptSpec::new(
                    "summarize",
                    serde_json::json!({"type": "object"}),
                ))
                .with_sampling(McpSamplingSpec::enabled())
                .with_subscription(McpSubscriptionSpec::new(
                    "docs-updates",
                    "resource://docs/index",
                )))
        }

        async fn close(&self, id: &str, _transport: &McpTransport) -> Result<(), LiveMcpError> {
            self.closed.lock().unwrap().push(id.to_string());
            Ok(())
        }
    }

    let closed = Arc::new(Mutex::new(Vec::new()));
    let toolset = live_mcp_toolset(
        Arc::new(ClosingMcp {
            closed: closed.clone(),
        }),
        "local",
        McpTransport::stdio("fake-server"),
    )
    .await
    .unwrap();
    let mut context = AgentContext::default();

    let result = AgentBuilder::new(Arc::new(TestModel::with_text("done")))
        .toolset(&toolset)
        .build()
        .run_with_context("hello", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert!(context.events.events().iter().any(|event| {
        event.kind == TOOLSET_INITIALIZED_EVENT_KIND
            && event.payload["name"] == serde_json::json!("local")
            && event.payload["state"] == serde_json::json!("initialized")
            && event.payload["metadata"]["mcp_server_id"] == serde_json::json!("local")
            && event.payload["metadata"]["mcp_transport"] == serde_json::json!("stdio")
            && event.payload["metadata"]["live_mcp"] == serde_json::json!(true)
            && event.payload["metadata"]["mcp_resource_count"] == serde_json::json!(1)
            && event.payload["metadata"]["mcp_prompt_count"] == serde_json::json!(1)
            && event.payload["metadata"]["mcp_sampling"] == serde_json::json!(true)
            && event.payload["metadata"]["mcp_subscription_count"] == serde_json::json!(1)
    }));
    assert_eq!(*closed.lock().unwrap(), vec!["local".to_string()]);
    assert!(context
        .events
        .events()
        .iter()
        .any(|event| event.kind == TOOLSET_CLOSED_EVENT_KIND
            && event.payload["name"] == serde_json::json!("local")
            && event.payload["state"] == serde_json::json!("closed")));
}
