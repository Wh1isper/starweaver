#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_core::{ConversationId, RunId};
use starweaver_tools::{
    DynToolset, McpToolSpec, McpToolset, McpToolsetConfig, McpTransport, NativeMcpServer,
    ToolRegistry,
};

#[test]
fn mcp_toolset_exposes_declared_tools_and_instructions() {
    let toolset = McpToolset::new(
        McpToolsetConfig::new(
            "weather",
            McpTransport::streamable_http("http://localhost:8000/mcp"),
        )
        .with_include_instructions(true)
        .with_instructions("Use weather MCP tools.")
        .with_tool_prefix("weather")
        .with_tool(
            McpToolSpec::new(
                "forecast",
                serde_json::json!({"type": "object", "properties": {"city": {"type": "string"}}}),
            )
            .with_description("Get a forecast")
            .with_task(true),
        ),
    );

    let toolset: DynToolset = Arc::new(toolset);
    let registry = ToolRegistry::new().with_toolset(&toolset);
    let definitions = registry.definitions();

    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].name, "weather_forecast");
    assert_eq!(
        definitions[0].description.as_deref(),
        Some("Get a forecast")
    );
    assert_eq!(definitions[0].metadata["mcp_server_id"], "weather");
    assert_eq!(definitions[0].metadata["mcp_transport"], "streamable_http");
    assert_eq!(definitions[0].metadata["mcp_tool_name"], "forecast");
    assert_eq!(definitions[0].metadata["mcp_task"], true);
    assert_eq!(registry.get_instructions(), vec!["Use weather MCP tools."]);
}

#[tokio::test]
async fn mcp_tool_call_defers_to_mcp_runtime_metadata() {
    let toolset = McpToolset::new(
        McpToolsetConfig::new(
            "math",
            McpTransport::stdio("python").with_args(vec!["server.py".to_string()]),
        )
        .with_tool(McpToolSpec::new(
            "add",
            serde_json::json!({"type": "object"}),
        )),
    );
    let toolset: DynToolset = Arc::new(toolset);
    let registry = ToolRegistry::new().with_toolset(&toolset);
    let call = starweaver_model::ToolCallPart {
        id: "call_1".to_string(),
        name: "add".to_string(),
        arguments: serde_json::json!({"a": 2, "b": 3}),
    };

    let result = registry
        .execute_call(
            starweaver_tools::ToolContext::new(RunId::new(), ConversationId::new(), 0),
            &call,
        )
        .await;

    assert!(result.is_error);
    assert_eq!(result.content["kind"], "call_deferred");
    assert_eq!(result.metadata["control_flow"], "call_deferred");
    assert_eq!(result.metadata["deferred"]["kind"], "mcp_tool_call");
    assert_eq!(result.metadata["deferred"]["server_id"], "math");
    assert_eq!(result.metadata["deferred"]["tool_name"], "add");
    assert_eq!(result.metadata["deferred"]["arguments"]["a"], 2);
}

#[test]
fn native_mcp_server_maps_to_provider_native_tool_definition() {
    let mut headers = serde_json::Map::new();
    headers.insert("X-Custom".to_string(), serde_json::json!("value"));

    let native = NativeMcpServer::new("github", "https://api.githubcopilot.com/mcp/")
        .with_authorization_token("secret")
        .with_description("GitHub MCP server")
        .with_allowed_tools(vec!["search_repositories".to_string()])
        .with_headers(headers)
        .native_tool_definition();

    assert_eq!(native.tool_type, "mcp");
    assert_eq!(native.config["server_label"], "github");
    assert_eq!(
        native.config["server_url"],
        "https://api.githubcopilot.com/mcp/"
    );
    assert_eq!(native.config["authorization"], "secret");
    assert_eq!(native.config["server_description"], "GitHub MCP server");
    assert_eq!(native.config["allowed_tools"][0], "search_repositories");
    assert_eq!(native.config["headers"]["X-Custom"], "value");
    assert_eq!(native.config["require_approval"], "never");
}

#[test]
fn native_mcp_server_maps_openai_connector_uri() {
    let native = NativeMcpServer::new("calendar", "x-openai-connector:connector_googlecalendar")
        .native_tool_definition();

    assert_eq!(native.tool_type, "mcp");
    assert_eq!(native.config["server_label"], "calendar");
    assert_eq!(native.config["connector_id"], "connector_googlecalendar");
    assert!(native.config.get("server_url").is_none());
}
