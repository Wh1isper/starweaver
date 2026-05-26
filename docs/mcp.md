# MCP

Starweaver currently supports MCP foundations in `starweaver-tools` and provider-native MCP request metadata in `starweaver-model`.

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
assert_eq!(toolset.tools().len(), 1);
```

## Provider-native MCP

`NativeMcpServer` maps MCP server metadata into provider-native tool definitions for providers that execute MCP directly.

Live MCP clients, transports, resources, prompts, sampling, elicitation, and protocol tests are tracked in `spec/09-mcp-strategy.md`.
