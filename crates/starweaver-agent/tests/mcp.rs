#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{
    AgentBuilder, McpToolSpec, McpToolset, McpToolsetConfig, McpTransport, NativeMcpServer,
    StaticCapabilityBundle, TestModel,
};
use starweaver_model::ModelRequestParameters;

#[test]
fn facade_reexports_mcp_toolset_types() {
    let toolset = McpToolset::new(
        McpToolsetConfig::new("docs", McpTransport::sse("http://localhost:8000/sse")).with_tool(
            McpToolSpec::new("search", serde_json::json!({"type": "object"})),
        ),
    );

    assert_eq!(toolset.config().id, "docs");
    assert!(toolset
        .tool_name_conflict_hint()
        .contains("PrefixedToolset"));
}

#[tokio::test]
async fn builder_applies_native_mcp_server_request_params() {
    let model = Arc::new(TestModel::with_text("ok"));
    let mut params = ModelRequestParameters::default();
    params.native_tools.push(
        NativeMcpServer::new("deepwiki", "https://mcp.deepwiki.com/mcp").native_tool_definition(),
    );
    let agent = AgentBuilder::new(model.clone())
        .request_params(params)
        .build();

    let result = agent.run("hello").await.unwrap();

    assert_eq!(result.output, "ok");
    let captured = model.captured_params();
    assert_eq!(captured[0].native_tools.len(), 1);
    assert_eq!(captured[0].native_tools[0].tool_type, "mcp");
    assert_eq!(
        captured[0].native_tools[0].config["server_label"],
        "deepwiki"
    );
}

#[tokio::test]
async fn capability_bundle_can_contribute_native_mcp_server() {
    let model = Arc::new(TestModel::with_text("ok"));
    let mut params = ModelRequestParameters::default();
    params.native_tools.push(
        NativeMcpServer::new("calendar", "x-openai-connector:connector_googlecalendar")
            .native_tool_definition(),
    );
    let bundle = StaticCapabilityBundle::new("mcp").with_request_params(params);

    let result = AgentBuilder::new(model.clone())
        .capability_bundle(Arc::new(bundle))
        .build()
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
    let captured = model.captured_params();
    assert_eq!(captured[0].native_tools.len(), 1);
    assert_eq!(
        captured[0].native_tools[0].config["connector_id"],
        "connector_googlecalendar"
    );
}
