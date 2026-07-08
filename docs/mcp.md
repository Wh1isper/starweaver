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

Live MCP stdio and streamable HTTP execution are available through `RmcpLiveMcpClient`, which uses the official Model Context Protocol Rust SDK at <https://github.com/modelcontextprotocol/rust-sdk> through the `rmcp` crate. Streamable HTTP includes `rmcp` session reinitialization for expired sessions. The older standalone SSE transport is not exposed by `rmcp` 1.7; roots, logging, completions, notifications, and host-owned long-running task workers remain product-level host contracts.

## Host-backed live MCP adapter

`LiveMcpClient` is the SDK seam for hosts that use `rmcp` or another MCP runtime to discover and call a live server. `RmcpLiveMcpClient` provides the built-in `rmcp` stdio and streamable HTTP implementation. The helper returns a lifecycle-aware toolset, so discovered tools participate in the same registry, prefixing, and instruction flows as static MCP specs while the runtime can call `LiveMcpClient::close` before the run exits. Discovery-only clients can omit `call_tool`; discovered tool calls then produce deferred MCP tool-call records until the host supplies a result. Tools declared with required MCP task support are also deferred as `mcp_tool_call` records because Starweaver does not execute MCP task workers inside the live adapter.
Live MCP preparation emits `toolset_initialized` lifecycle evidence with `mcp_server_id`, `mcp_transport`, `live_mcp`, resource count, prompt count, sampling availability, and subscription count metadata. Cleanup emits `toolset_closed` with the same identity metadata.

```rust
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use starweaver_agent::{
    live_mcp_toolset, LiveMcpClient, LiveMcpError, LiveMcpServerSnapshot, McpPromptSpec,
    McpResourceSpec, McpSamplingSpec, McpSubscriptionSpec, McpToolSpec, McpTransport,
    ToolContext, ToolResult,
};

struct FakeMcp;

#[async_trait]
impl LiveMcpClient for FakeMcp {
    async fn discover(
        &self,
        id: &str,
        _transport: &McpTransport,
    ) -> Result<LiveMcpServerSnapshot, LiveMcpError> {
        Ok(LiveMcpServerSnapshot::new(id)
            .with_instructions("Prefer repository docs for repository questions.")
            .with_tool(McpToolSpec::new("lookup", serde_json::json!({"type": "object"})))
            .with_resource(McpResourceSpec::new("resource://docs/index"))
            .with_prompt(McpPromptSpec::new(
                "summarize-docs",
                serde_json::json!({"type": "object"}),
            ))
            .with_sampling(McpSamplingSpec::enabled())
            .with_subscription(McpSubscriptionSpec::new(
                "docs-updates",
                "resource://docs/index",
            )))
    }

    async fn call_tool(
        &self,
        _context: ToolContext,
        id: &str,
        _transport: &McpTransport,
        tool_name: &str,
        arguments: Value,
    ) -> Result<ToolResult, LiveMcpError> {
        if id == "docs" && tool_name == "lookup" {
            return Ok(ToolResult::new(serde_json::json!({
                "query": arguments.get("query").cloned().unwrap_or(Value::Null),
                "result": "found"
            })));
        }
        Err(LiveMcpError::Adapter(format!("unknown MCP tool {id}/{tool_name}")))
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
