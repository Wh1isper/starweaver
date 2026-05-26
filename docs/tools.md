# Tools

Tools are provider-neutral function definitions with JSON arguments and JSON results. Runtime tool execution uses `starweaver-tools`; SDK tool implementation bundles live above the core runtime.

## Function tool

```rust
use std::sync::Arc;

use serde_json::json;
use starweaver_agent::{FunctionTool, ToolContext, ToolRegistry, ToolResult};

let lookup = FunctionTool::new(
    "lookup",
    Some("Lookup a value".to_string()),
    json!({
        "type": "object",
        "properties": {
            "query": {"type": "string"}
        },
        "required": ["query"]
    }),
    |_ctx: ToolContext, args: serde_json::Value| async move {
        Ok(ToolResult::new(json!({"value": args["query"]})))
    },
);

let tools = ToolRegistry::new().with_tool(Arc::new(lookup));
assert!(!tools.is_empty());
```

## Toolsets

```rust
use std::sync::Arc;

use serde_json::json;
use starweaver_agent::{
    FunctionTool, StaticToolset, ToolContext, ToolInstruction, ToolResult, Toolset,
};

let tool = FunctionTool::new(
    "echo",
    Some("Echo input".to_string()),
    json!({"type": "object"}),
    |_ctx: ToolContext, args: serde_json::Value| async move {
        Ok(ToolResult::new(args))
    },
);

let toolset = StaticToolset::new("basic")
    .with_tool(Arc::new(tool))
    .with_instruction(ToolInstruction::new("basic", "Use tools for exact lookup."));

assert_eq!(toolset.name(), "basic");
```

## Retry metadata

Retry limits can be set at the tool, toolset, registry, or agent level. The runtime passes retry counters through `ToolContext` and records retry metadata on retryable tool returns.
