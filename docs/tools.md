# Tools

Tools are provider-neutral function definitions with typed JSON arguments and JSON results. Runtime tool execution uses `starweaver-tools`; SDK tool implementation bundles live above the core runtime.

## Typed function tools

```rust
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starweaver_agent::{typed_tool, ToolContext, ToolRegistry, ToolResult};

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
struct LookupArgs {
    query: String,
}

let lookup = typed_tool::<LookupArgs, _, _>(
    "lookup",
    Some("Lookup a value".to_string()),
    |_ctx: ToolContext, args: LookupArgs| async move {
        Ok(ToolResult::new(serde_json::json!({"value": args.query})))
    },
);

let tools = ToolRegistry::new().with_tool(Arc::new(lookup));
assert!(!tools.is_empty());
```

`typed_tool` derives JSON Schema from the Rust argument type with `schemars`, then validates model-provided JSON before executing the tool function.

## Tool context and AgentContext dependencies

`ToolContext` carries execution metadata such as run ids, retry counters, trace context, and typed dependencies. Inside the agent runtime, the active `AgentContext` is injected as a typed dependency, matching the pydantic-ai `RunContext.deps` pattern.

```rust
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starweaver_agent::{typed_tool, AgentBuilder, AgentContext, TestModel, ToolContext, ToolResult};

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
struct ReadNoteArgs {
    key: String,
}

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let read_note = typed_tool::<ReadNoteArgs, _, _>(
    "read_note",
    Some("Read one AgentContext note".to_string()),
    |ctx: ToolContext, args: ReadNoteArgs| async move {
        let agent_context = ctx.dependency::<AgentContext>().expect("agent context");
        let value = agent_context.notes.get(&args.key).unwrap_or_default();
        Ok(ToolResult::new(serde_json::json!({"value": value})))
    },
);

let mut context = AgentContext::default();
context.notes.set("lang", "Chinese");
let agent = AgentBuilder::new(Arc::new(TestModel::with_text("done")))
    .tool(Arc::new(read_note))
    .build();

let result = agent.run_with_context("read note", &mut context).await?;
assert_eq!(result.output, "done");
# Ok(())
# }
```

## Toolsets

```rust
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starweaver_agent::{
    typed_tool, StaticToolset, ToolContext, ToolInstruction, ToolResult, Toolset,
};

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
struct EchoArgs {
    value: String,
}

let echo = typed_tool::<EchoArgs, _, _>(
    "echo",
    Some("Echo input".to_string()),
    |_ctx: ToolContext, args: EchoArgs| async move {
        Ok(ToolResult::new(serde_json::json!({"value": args.value})))
    },
);

let toolset = StaticToolset::new("basic")
    .with_tool(Arc::new(echo))
    .with_instruction(ToolInstruction::new("basic", "Use tools for exact lookup."));

assert_eq!(toolset.name(), "basic");
assert_eq!(toolset.get_tools().len(), 1);
assert_eq!(toolset.get_instructions().len(), 1);
```

## First-party environment bundles

First-party filesystem and shell bundles resolve the active environment from `AgentContext`, so the same toolset can run against virtual, local, or sandbox providers.

```rust
use std::sync::Arc;

use starweaver_agent::{filesystem_tools, AgentBuilder, AgentSession, TestModel};
use starweaver_environment::VirtualEnvironmentProvider;

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let provider = Arc::new(VirtualEnvironmentProvider::new("docs").with_file("README.md", "hello"));
let agent = AgentBuilder::new(Arc::new(TestModel::with_text("done")))
    .toolset(&filesystem_tools())
    .build();
let mut session = AgentSession::new(agent).with_environment(provider);

let result = session.run("read README.md").await?;
assert_eq!(result.output, "done");
# Ok(())
# }
```

## Tool proxy

`ToolProxyToolset` is a core toolset combinator that wraps many toolsets behind two stable model-facing tools: `search_tools` and `call_tool`. Use `PrefixedToolset` around the proxy when multiple proxy surfaces need distinct names.

```rust
use std::sync::Arc;

use starweaver_agent::{
    filesystem_tools, tool_proxy_toolset, DynToolset, PrefixedToolset, ToolRegistry,
};

let proxy = tool_proxy_toolset(vec![filesystem_tools()]);
let prefixed: DynToolset = Arc::new(PrefixedToolset::new("workspace", proxy));
let registry = ToolRegistry::new().with_toolset(&prefixed);
assert!(!registry.is_empty());
```

## Retry metadata

Retry limits can be set at the tool, toolset, registry, or agent level. The runtime passes retry counters through `ToolContext` and records retry metadata on retryable tool returns.
