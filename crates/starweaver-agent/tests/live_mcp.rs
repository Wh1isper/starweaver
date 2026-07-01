#![allow(missing_docs, clippy::unwrap_used)]

use std::{
    future::Future,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        AnnotateAble, CallToolRequestParams, CallToolResult, Implementation, ListPromptsResult,
        ListResourcesResult, ListToolsResult, PaginatedRequestParams, Prompt, PromptArgument,
        RawResource, ServerCapabilities, ServerInfo, Tool,
    },
    service::{MaybeSendFuture, RequestContext},
    transport::{
        StreamableHttpServerConfig, StreamableHttpService,
        streamable_http_server::session::local::LocalSessionManager,
    },
};
use starweaver_agent::{
    AgentBuilder, AgentContext, LiveMcpClient, LiveMcpError, LiveMcpServerSnapshot, McpPromptSpec,
    McpResourceSpec, McpSamplingSpec, McpSubscriptionSpec, McpToolSpec, McpTransport,
    RmcpLiveMcpClient, TOOLSET_CLOSED_EVENT_KIND, TOOLSET_INITIALIZED_EVENT_KIND, TestModel,
    ToolContext, ToolRegistry, ToolResult, live_mcp_toolset,
};
use starweaver_core::{ConversationId, RunId};
use starweaver_model::ToolCallPart;
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;

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
    let transport = McpTransport::stdio(env!("CARGO_BIN_EXE_starweaver_agent_rmcp_stdio_fixture"));
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

#[derive(Clone, Copy)]
enum StreamableHttpFixtureKind {
    Standard,
    Reconnect,
}

struct StreamableHttpFixture {
    url: String,
    session_manager: Arc<LocalSessionManager>,
    shutdown: CancellationToken,
    handle: JoinHandle<()>,
}

impl StreamableHttpFixture {
    async fn expire_sessions(&self) {
        self.session_manager.sessions.write().await.clear();
    }

    async fn stop(self) {
        self.shutdown.cancel();
        self.handle.await.unwrap();
    }
}

#[derive(Clone)]
struct FixtureMcpServer {
    kind: StreamableHttpFixtureKind,
    session: Option<String>,
    initialize_count: Arc<AtomicUsize>,
}

impl FixtureMcpServer {
    fn new(kind: StreamableHttpFixtureKind, initialize_count: Arc<AtomicUsize>) -> Self {
        let session = match kind {
            StreamableHttpFixtureKind::Standard => None,
            StreamableHttpFixtureKind::Reconnect => {
                let count = initialize_count.fetch_add(1, Ordering::SeqCst) + 1;
                Some(format!("session-{count}"))
            }
        };
        Self {
            kind,
            session,
            initialize_count,
        }
    }

    const fn instructions(&self) -> &'static str {
        match self.kind {
            StreamableHttpFixtureKind::Standard => "Use HTTP fixture MCP tools.",
            StreamableHttpFixtureKind::Reconnect => "Use reconnect fixture MCP tools.",
        }
    }

    const fn answer(&self) -> &'static str {
        match self.kind {
            StreamableHttpFixtureKind::Standard => "http fixture result",
            StreamableHttpFixtureKind::Reconnect => "reconnected result",
        }
    }
}

impl ServerHandler for FixtureMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
        .with_instructions(self.instructions())
        .with_server_info(Implementation::new("starweaver-http-fixture", "0.0.1"))
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + MaybeSendFuture + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(vec![Tool::new(
            "lookup",
            "Look up HTTP fixture data.",
            serde_json::json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"],
            })
            .as_object()
            .unwrap()
            .clone(),
        )])))
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, McpError>> + MaybeSendFuture + '_ {
        std::future::ready(Ok(ListResourcesResult::with_all_items(vec![
            RawResource::new("resource://fixture/http-docs", "HTTP Fixture Docs")
                .with_description("HTTP fixture documentation.")
                .with_mime_type("text/markdown")
                .no_annotation(),
        ])))
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, McpError>> + MaybeSendFuture + '_ {
        std::future::ready(Ok(ListPromptsResult::with_all_items(vec![Prompt::new(
            "summarize",
            Some("Summarize HTTP fixture docs."),
            Some(vec![
                PromptArgument::new("topic")
                    .with_description("Topic to summarize.")
                    .with_required(false),
            ]),
        )])))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + MaybeSendFuture + '_ {
        let query = request
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.get("query"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let mut structured = serde_json::json!({
            "answer": self.answer(),
            "query": query,
        });
        if let Some(session) = &self.session {
            structured["session"] = serde_json::Value::String(session.clone());
            structured["initialize_count"] =
                serde_json::Value::from(self.initialize_count.load(Ordering::SeqCst));
        }
        std::future::ready(Ok(CallToolResult::structured(structured)))
    }
}

async fn start_streamable_http_fixture(kind: StreamableHttpFixtureKind) -> StreamableHttpFixture {
    let session_manager = Arc::new(LocalSessionManager::default());
    let shutdown = CancellationToken::new();
    let initialize_count = Arc::new(AtomicUsize::new(0));
    let service = StreamableHttpService::new(
        {
            let initialize_count = initialize_count.clone();
            move || Ok(FixtureMcpServer::new(kind, initialize_count.clone()))
        },
        session_manager.clone(),
        StreamableHttpServerConfig::default()
            .with_sse_keep_alive(None)
            .with_cancellation_token(shutdown.child_token()),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let router = axum::Router::new().nest_service("/mcp", service);
    let handle = tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move { shutdown.cancelled_owned().await })
                .await
                .unwrap();
        }
    });
    StreamableHttpFixture {
        url: format!("http://{addr}/mcp"),
        session_manager,
        shutdown,
        handle,
    }
}

#[tokio::test]
async fn rmcp_streamable_http_client_discovers_executes_and_closes_fixture_server() {
    let fixture = start_streamable_http_fixture(StreamableHttpFixtureKind::Standard).await;
    let transport = McpTransport::streamable_http(fixture.url.clone());
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
    fixture.stop().await;
}

#[tokio::test]
async fn rmcp_streamable_http_client_reinitializes_after_expired_session() {
    let fixture = start_streamable_http_fixture(StreamableHttpFixtureKind::Reconnect).await;
    let transport = McpTransport::streamable_http(fixture.url.clone());
    let client = Arc::new(RmcpLiveMcpClient::new());
    let toolset = live_mcp_toolset(client.clone(), "rmcp-reconnect-fixture", transport.clone())
        .await
        .unwrap();
    fixture.expire_sessions().await;
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
    fixture.stop().await;
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
    assert!(
        context
            .events
            .events()
            .iter()
            .any(|event| event.kind == TOOLSET_CLOSED_EVENT_KIND
                && event.payload["name"] == serde_json::json!("local")
                && event.payload["state"] == serde_json::json!("closed"))
    );
}
