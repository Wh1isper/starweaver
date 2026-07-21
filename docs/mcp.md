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

## Standalone RPC MCP configuration

The standalone RPC host loads MCP only from an explicitly selected JSON file. It does not scan CLI or editor configuration.

```toml
# rpc.toml
[server]
mcp_config_path = "mcp.json"

[profiles.default]
model_id = "openai-responses:gpt-5"
mcp_servers = ["docs"]
```

```json
{
  "servers": {
    "docs": {
      "transport": "stdio",
      "command": "docs-mcp",
      "args": ["--serve"],
      "cwd": "workspace",
      "tool_prefix": "docs",
      "include_instructions": true,
      "init_timeout_ms": 10000,
      "read_timeout_ms": 30000,
      "exit_timeout_ms": 10000
    },
    "remote": {
      "transport": "streamable_http",
      "url": "https://mcp.example.test/mcp",
      "headers": {}
    }
  }
}
```

`mcp_config_path` is relative to `rpc.toml`. `STARWEAVER_RPC_MCP_CONFIG` overrides it; a relative environment value is relative to the process working directory. A relative stdio `cwd` is relative to the MCP JSON file. Profile `mcp_servers` is a duplicate-free set; omitting it selects every server in the explicit file. RPC supports stdio and streamable HTTP and rejects standalone SSE before starting a run.

The document is strict: unknown or transport-mismatched fields, missing commands or URLs, non-string environment/header values, invalid URLs, and zero timeouts fail startup. `init_timeout_ms` bounds connection and discovery, `read_timeout_ms` bounds each tool call, and `exit_timeout_ms` bounds transport cleanup. Cleanup defaults to 10 seconds when omitted. Do not commit credentials in the JSON file; header values are literal strings and are not variable-expanded. Deferred records and events expose only the transport kind and never serialize URLs, headers, commands, arguments, working directories, or environment values.

RPC creates MCP clients lazily during runtime tool preparation. Connections are isolated by run and closed when that run exits. A failed or timed-out close retains the run-scoped entry so an explicit later lifecycle retry remains possible instead of falsely reporting it closed. Calls on one MCP connection may execute concurrently; close and connection replacement take an exclusive fence. `toolset_initialized` evidence includes tool/resource/prompt counts and a credential-free digest of the discovered tool names, schemas, and task annotations.

Continuation materialization binds the selected MCP server names and static configuration, not the live discovered inventory. Discovery is intentionally dynamic per run: an MCP server may change its model-visible inventory between a waiting run and its continuation. The inventory digest is lifecycle evidence for diagnosing that drift; `Preserve` does not freeze it.
