# MCP

Starweaver supports MCP foundations in `starweaver-tools` and provider-native MCP request metadata in `starweaver-model`.

## Static MCP toolset

```rust
use starweaver_agent::{McpToolSpec, McpToolset, McpToolsetConfig, McpTransport, Toolset};

let toolset = McpToolset::new(
    McpToolsetConfig::new(
        "docs",
        McpTransport::StreamableHttp {
            url: "https://mcp.example.test".to_string(),
            headers: Default::default(),
        },
    )
    .with_tool(
        McpToolSpec::new("search", serde_json::json!({"type": "object"}))
            .with_description("Search docs"),
    ),
);

assert_eq!(toolset.name(), "docs");
assert_eq!(toolset.get_tools().len(), 1);
```

## Provider-native MCP

`NativeMcpServer` maps MCP server metadata into provider-native tool definitions for providers that execute MCP directly.

Live MCP clients, transports, resources, prompts, sampling, roots, logging, completions, notifications, subscriptions, long-running tasks, and protocol tests are tracked in `spec/sdk/03-first-party-tool-bundles.md` and `memos/implementation-todo.md`. Starweaver's live MCP work should use the official Model Context Protocol Rust SDK at <https://github.com/modelcontextprotocol/rust-sdk> through the `rmcp` crate.

## Host-backed live MCP bridge

`LiveMcpClient` is the SDK seam for hosts that use `rmcp` or another MCP runtime to discover a live server. The helper returns a normal `McpToolset`, so discovered tools participate in the same registry, prefixing, and instruction flows as static MCP specs.

```rust
use std::sync::Arc;

use async_trait::async_trait;
use starweaver_agent::{
    live_mcp_toolset, LiveMcpClient, LiveMcpError, LiveMcpServerSnapshot, McpToolSpec,
    McpTransport,
};

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
            instructions: Some("Use docs tools for repository questions.".to_string()),
            tools: vec![McpToolSpec::new("lookup", serde_json::json!({"type": "object"}))],
        })
    }
}

# async fn example() -> Result<(), starweaver_agent::LiveMcpError> {
let toolset = live_mcp_toolset(
    Arc::new(FakeMcp),
    "docs",
    McpTransport::stdio("docs-mcp"),
)
.await?;

assert_eq!(toolset.name(), "docs");
assert_eq!(toolset.get_tools()[0].name(), "lookup");
# Ok(())
# }
```

Serialized `AgentSpec` values reference MCP servers by stable names. The host resolves those names into live clients or static toolsets through `AgentSpecRegistry`, keeping subprocess commands, URLs, headers, and credentials outside agent profile files.
