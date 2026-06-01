#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use async_trait::async_trait;
use starweaver_agent::{
    live_mcp_toolset, LiveMcpClient, LiveMcpError, LiveMcpServerSnapshot, McpToolSpec, McpTransport,
};

#[tokio::test]
async fn live_mcp_bridge_discovers_toolset() {
    struct FakeMcp;

    #[async_trait]
    impl LiveMcpClient for FakeMcp {
        async fn discover(
            &self,
            id: &str,
            _transport: &McpTransport,
        ) -> Result<LiveMcpServerSnapshot, LiveMcpError> {
            Ok(LiveMcpServerSnapshot {
                id: id.to_string(),
                instructions: Some("Use local MCP tools.".to_string()),
                tools: vec![McpToolSpec::new(
                    "lookup",
                    serde_json::json!({"type": "object"}),
                )],
            })
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
