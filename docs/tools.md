# Tools

Tools are provider-neutral function definitions with typed JSON arguments and JSON results. Runtime tool execution uses `starweaver-tools`; SDK tool implementation bundles live above the core runtime.

## Typed function tools

Use `typed_tool` when your tool arguments can be represented as a Rust struct. Starweaver derives the model-facing JSON Schema from that type with `schemars` and validates model-provided JSON with Serde before executing the tool function.

```rust
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starweaver_agent::{typed_tool, ToolContext, ToolRegistry, ToolResult};

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
struct LookupArgs {
    /// Search query to look up.
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

Field doc comments and `#[schemars(description = "...")]` become per-argument descriptions in the tool schema. Use doc comments for ordinary fields and `#[schemars(...)]` when the schema description should differ from Rust docs.

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
struct SearchArgs {
    /// Query text submitted by the model.
    query: String,
    #[serde(default = "default_limit")]
    #[schemars(description = "Maximum number of results to return.")]
    limit: usize,
}

const fn default_limit() -> usize {
    10
}
```

Serde remains the execution-time validation contract. Use `#[serde(default)]`, aliases, enums, and nested structs for runtime input compatibility, and use schemars attributes for model-facing schema metadata.

## Tools with context dependencies

`ToolContext` carries execution metadata such as run ids, retry counters, trace context, and typed dependencies. Inside the agent runtime, the active `AgentContext` is injected as a typed dependency, matching the pydantic-ai `RunContext.deps` pattern.

```rust
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starweaver_agent::{typed_tool, AgentBuilder, AgentContext, TestModel, ToolContext, ToolResult};

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
struct ReadNoteArgs {
    /// Note key to read from AgentContext.
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

## Function tools with manual schemas

Use `string_tool` or `FunctionTool` when a tool schema is already available as JSON or comes from another protocol such as MCP.

```rust
use std::sync::Arc;

use starweaver_agent::{string_tool, ToolContext, ToolRegistry, ToolResult};

let parameters = serde_json::json!({
    "type": "object",
    "properties": {
        "value": {
            "type": "string",
            "description": "Value to echo."
        }
    },
    "required": ["value"]
});

let echo = string_tool(
    "echo",
    Some("Echo a JSON value".to_string()),
    parameters,
    |_ctx: ToolContext, args: serde_json::Value| async move {
        Ok(ToolResult::new(args))
    },
);

let registry = ToolRegistry::new().with_tool(Arc::new(echo));
assert!(!registry.is_empty());
```

## Toolsets

Group related tools into a `StaticToolset` when they should share instructions, metadata, registration, or namespacing.

```rust
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starweaver_agent::{
    typed_tool, StaticToolset, ToolContext, ToolInstruction, ToolResult, Toolset,
};

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
struct EchoArgs {
    /// Text to echo.
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

Tool instructions are grouped by `ToolInstruction::group`; the registry keeps one instruction per group to keep prompts compact.

## Registering tools on agents and sessions

Register reusable tools on `AgentBuilder` with `.tool(...)`, `.toolset(...)`, or `.tool_registry(...)`.

```rust
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starweaver_agent::{typed_tool, AgentBuilder, TestModel, ToolContext, ToolResult};

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
struct PingArgs {
    /// Text returned by the ping tool.
    text: String,
}

let ping = typed_tool::<PingArgs, _, _>(
    "ping",
    Some("Return ping text".to_string()),
    |_ctx: ToolContext, args: PingArgs| async move {
        Ok(ToolResult::new(serde_json::json!({"text": args.text})))
    },
);

let _agent = AgentBuilder::new(Arc::new(TestModel::with_text("ready")))
    .tool(Arc::new(ping))
    .build();
```

For one run or one session call, use `AgentRunOptions` to add run-scoped tools, toolsets, settings, params, or instructions while keeping the reusable session agent unchanged.

```rust
use std::sync::Arc;

use starweaver_agent::{string_tool, AgentBuilder, AgentRunOptions, TestModel, ToolContext, ToolResult};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let tool = Arc::new(string_tool(
    "run_echo",
    Some("Echo one run payload".to_string()),
    serde_json::json!({"type": "object", "properties": {}}),
    |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
));
let mut session = AgentBuilder::new(Arc::new(TestModel::with_text("done")))
    .build_app()
    .session();
let result = session
    .run_with_options("use the run tool", AgentRunOptions::new().tool(tool))
    .await?;
assert_eq!(result.output, "done");
# Ok(())
# }
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

`glob` and `grep` use ripgrep-style matching over the active `EnvironmentProvider`. Bare patterns such as `*.rs` match at any depth, scoped patterns such as `src/*.rs` match one path segment under `src`, and recursive patterns such as `**/*.rs` match root-level and nested Rust files.

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

## Fileops-loaded skills

Skills are markdown packages discovered through the active `EnvironmentProvider`, matching the SDK's fileops path space. `SkillRegistry::scan` reads compact frontmatter summaries from `skills/*/SKILL.md` and `.agents/skills/*/SKILL.md`; `SkillRegistry::activate` loads the full markdown body when a host or agent workflow chooses a skill.

```rust
use std::sync::Arc;

use starweaver_agent::{SkillRegistry, SkillSourceScope};
use starweaver_environment::VirtualEnvironmentProvider;

# async fn example() -> Result<(), starweaver_agent::SkillError> {
let provider = Arc::new(
    VirtualEnvironmentProvider::new("skills").with_file(
        "skills/research/SKILL.md",
        r"---
name: research
description: Gather and cite sources
---
Use search tools and cite sources.
",
    ),
);
let registry = SkillRegistry::scan(provider.clone(), &[SkillSourceScope::new("")]).await?;
let active = SkillRegistry::activate(provider, "skills/research/SKILL.md").await?;

assert_eq!(registry.get("research").unwrap().description, "Gather and cite sources");
assert_eq!(active.body.unwrap(), "Use search tools and cite sources.");
# Ok(())
# }
```

`skill_tools(registry.packages())` converts the discovered summaries into a model-facing instruction block. The full body stays loaded on activation so hosts can implement request-boundary reloads and file synchronization hooks.

## Process-capable shell providers

The shell bundle runs foreground commands through `EnvironmentProvider::run_shell`. Background commands use a `ProcessShellProvider` dependency attached with `attach_process_shell`, which lets durable hosts expose handles for `shell_wait`, `shell_status`, `shell_input`, `shell_signal`, and `shell_kill`.

```rust
use std::sync::Arc;

use starweaver_agent::{
    attach_environment, attach_process_shell, shell_tools, AgentContext, ConversationId, RunId,
    ToolContext, ToolRegistry,
};
use starweaver_environment::VirtualEnvironmentProvider;

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let provider = Arc::new(VirtualEnvironmentProvider::new("process"));
let mut agent_context = AgentContext::default();
attach_environment(&mut agent_context, provider.clone());
attach_process_shell(&mut agent_context, provider);
let mut dependencies = agent_context.dependencies.clone();
dependencies.insert(agent_context);
let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
    .with_dependencies(dependencies);
let mut registry = ToolRegistry::new();
registry.insert_toolset(&shell_tools());

let started = registry
    .execute_call(
        context,
        &starweaver_model::ToolCallPart {
            id: "start".to_string(),
            name: "shell_exec".to_string(),
            arguments: serde_json::json!({"command": "sleep 1", "background": true}).into(),
        },
    )
    .await;
assert_eq!(started.content["status"], "running");
# Ok(())
# }
```
