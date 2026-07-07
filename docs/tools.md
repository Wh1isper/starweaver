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

Serde remains the execution-time validation contract. Use `#[serde(default)]`, aliases, enums, and nested structs for runtime input stability, and use schemars attributes for model-facing schema metadata.

## Tools with context dependencies

`ToolContext` carries execution metadata such as run ids, retry counters, trace context, cooperative cancellation, and typed dependencies. Inside the agent runtime, the active `AgentContext` is injected as a typed dependency, matching the typed run-context dependency pattern.

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

Use `ToolSearchToolset` through `tool_search_toolset` when discovery should load ordinary direct tools for the next model turn. The model initially sees only `tool_search`; after search records loaded tools or namespaces in `AgentContext`, the runtime's context-aware availability filter exposes the matched direct tools.

```rust
use std::sync::Arc;

use starweaver_agent::{
    string_tool, tool_search_toolset, AgentContext, StaticToolset, ToolContext, ToolRegistry,
    ToolResult,
};

let lookup = Arc::new(string_tool(
    "lookup_docs",
    Some("Look up documentation by topic".to_string()),
    serde_json::json!({"type": "object", "properties": {"topic": {"type": "string"}}}),
    |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
));
let docs = Arc::new(
    StaticToolset::new("docs")
        .with_id("docs_ns")
        .with_tool(lookup),
);
let search = tool_search_toolset(vec![docs]);
let registry = ToolRegistry::new().with_toolset(&search);
let mut context = AgentContext::default();

assert_eq!(registry.definitions_for_context(&context)[0].name, "tool_search");
context.record_tool_search_loaded(["lookup_docs"], ["docs_ns"]);
let names = registry
    .definitions_for_context(&context)
    .into_iter()
    .map(|definition| definition.name)
    .collect::<Vec<_>>();
assert_eq!(names, vec!["lookup_docs".to_string(), "tool_search".to_string()]);
```

Hosts that know a namespace should be available can use `ToolSearchToolset::preload_namespace` directly; it records the same state and emits the same `tool_search_loaded` event as a model-driven search. Hosts can also call `initialization_report` or `refresh_report` to inspect indexed tools, namespaces, availability counts, and search limits. `publish_initialization_report` and `publish_refresh_report` emit the same data as `tool_search_initialized` and `tool_search_refreshed` context events. When a cached or remote tool library changes, host code can call `refresh_loaded_state` to prune stale or unavailable loaded tools while publishing `tool_search_refreshed`, or `invalidate_loaded_state` to clear all loaded dynamic state while publishing `tool_search_invalidated`. Use `ToolSearchRefreshBinding` when a host owns inventory versions, file watcher events, or remote catalog polling and needs deterministic interval/debounce decisions before calling `refresh_loaded_state_if_due`. Scheduled refresh events include `refresh_scheduled`, `refresh_reason`, `refresh_ms`, and `inventory_version` diagnostics. Invalid search queries emit `tool_search_failed`; valid queries with no matches emit `tool_search_no_match`.

## Deferred toolsets

Use `DeferredToolset` when a model-visible tool should pause the run and hand work to an external worker or later resume step. Matching tools are marked with `starweaver_tool_kind: deferred` and `deferred_call: true`; when called without an inline deferred result, they return Starweaver's `call_deferred` control-flow error so the runtime enters `Waiting` and records a durable deferred tool request. Hosts can later resume the session with `AgentHitlResults::deferred_result`.

## Toolset lifecycle

Context-aware toolsets can opt into run lifecycle hooks with `ToolsetLifecyclePolicy`.
Set `enter_before_prepare` when a resource-backed toolset needs setup before inventory is
read, and set `exit_after_run` when the runtime should call `exit_with_context` before
the run exits. The runtime publishes lifecycle reports as context events, including
`toolset_initialized`, `toolset_failed`, `toolset_unavailable`, and `toolset_closed`.

## Retry and feedback semantics

Retry limits can be set at the tool, toolset, registry, or agent level. When no override is configured, tools get a default retry budget of 3 retries for unexpected execution failures. `ToolRegistry` uses that budget internally by rerunning the same tool call, so transient provider/runtime failures do not require the model to regenerate tool arguments.

Model correction retry is separate from internal execution retry. `ToolError::ModelRetry` and JSON/Serde argument validation failures (`ToolError::InvalidArguments`) ask the model to correct the tool call and consume the model retry budget. Use these only when the model produced bad tool input.

Expected conditions that the agent can reason about, such as a missing file, invalid path for the active environment, unsupported media format, HTTP 404, or an output-size limit, should be returned as `ToolError::Feedback`. Feedback is serialized as an agent-readable tool return with `success: false` and `is_error: false`; it does not consume model retry budget.

Developer or integration mistakes, such as missing required runtime dependencies, direct calls to tools that require an `AgentContextHandle`, or dynamic tool-proxy calls before the proxy is loaded, should be returned as `ToolError::UserError`. User errors are `is_error: true`, but they are not model-retryable and are not unexpected execution failures, so they do not consume model retry budget or internal unexpected retry budget.

The runtime passes retry counters through `ToolContext` and records retry metadata on model-retryable tool returns and exhausted internal execution retries.

## Timeout metadata

Execution timeouts can be set at the tool, static toolset, or registry level. Tool-specific values win over inherited defaults, and `ToolRegistry` enforces the effective timeout during dispatch.

```rust
use std::sync::Arc;

use starweaver_agent::{string_tool, ToolContext, ToolRegistry, ToolResult};

let echo = string_tool(
    "echo",
    Some("Echo a JSON value".to_string()),
    serde_json::json!({"type": "object", "properties": {}}),
    |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
)
.with_timeout_ms(1_000);

let registry = ToolRegistry::new()
    .with_timeout_ms(5_000)
    .with_tool(Arc::new(echo));

assert_eq!(registry.timeout_ms_for("echo"), Some(1_000));
```

## Execution hooks

`ToolRegistry` can wrap local tool execution with ordered middleware through
`ToolExecutionHook`. Global hooks run before tool-specific hooks on input, and
tool-specific hooks run before global hooks on output. Pre-execution hooks can
mutate `ToolContext` and JSON arguments. Post-execution hooks can mutate ordinary
success/error outcomes before they become `ToolReturnPart` records.

Approval and deferred-control-flow outcomes are observable but not replaceable by
post hooks. This keeps HITL and deferred execution semantics stable while still
allowing tracing, cleanup, and audit middleware to run.

Function tools can also preprocess host/user input supplied during HITL approval.
The preprocessor can produce replacement arguments and approval metadata before
`AgentSession::resume_after_hitl` executes the approved tool.

```rust
use starweaver_agent::{
    string_tool, ToolContext, ToolResult, ToolUserInputPreprocessResult,
};

let _tool = string_tool(
    "write_file",
    Some("Write a reviewed file".to_string()),
    serde_json::json!({"type": "object"}),
    |_ctx: ToolContext, args: serde_json::Value| async move {
        Ok(ToolResult::new(args))
    },
)
.with_user_input_preprocessor(|_ctx, user_input| async move {
    Ok(ToolUserInputPreprocessResult::new()
        .with_override_arguments(user_input))
});
```

## Return schemas

Tools can expose a JSON Schema for successful tool results. The schema is provider-neutral metadata on `ToolDefinition`; hosts and UI layers can use it for validation, display, or replay contracts.

```rust
use std::sync::Arc;

use starweaver_agent::{string_tool, ToolContext, ToolRegistry, ToolResult};

let schema = serde_json::json!({
    "type": "object",
    "properties": {"ok": {"type": "boolean"}},
    "required": ["ok"]
});
let status = string_tool(
    "status",
    Some("Return status".to_string()),
    serde_json::json!({"type": "object", "properties": {}}),
    |_ctx: ToolContext, _args: serde_json::Value| async move {
        Ok(ToolResult::new(serde_json::json!({"ok": true})))
    },
)
.with_return_schema(schema.clone());

let definition = ToolRegistry::new()
    .with_tool(Arc::new(status))
    .definitions()
    .remove(0);
assert_eq!(definition.return_schema, Some(schema));
```

## Tool metadata tags

Tools can attach provider-neutral capability tags through `tags` and `hidden_by_tags` metadata.
Use `tags` to group tools by capability and `hidden_by_tags` to let host policy hide tools when
another active capability tag covers the same workflow.

```rust
use starweaver_agent::{
    string_tool, tool_metadata_hidden_by_tags, tool_metadata_tags, Tool, ToolContext, ToolResult,
};

let tool = string_tool(
    "search_files",
    Some("Search files".to_string()),
    serde_json::json!({"type": "object", "properties": {}}),
    |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
)
.with_tags(["filesystem", "search"])
.with_hidden_by_tag("remote-search");

let definition = tool.definition();
assert_eq!(
    tool_metadata_tags(&definition.metadata),
    vec!["filesystem".to_string(), "search".to_string()]
);
assert_eq!(
    tool_metadata_hidden_by_tags(&definition.metadata),
    vec!["remote-search".to_string()]
);
```

## Tool kind metadata

Use `ToolKind` when a host or UI needs to distinguish normal function tools from output,
external, approval-gated, or deferred tools. The kind is stored in the stable
`starweaver_tool_kind` metadata field and can be read back from any `ToolDefinition`.

```rust
use starweaver_agent::{
    string_tool, tool_metadata_kind, Tool, ToolContext, ToolKind, ToolResult,
};

let tool = string_tool(
    "external_search",
    Some("Search externally".to_string()),
    serde_json::json!({"type": "object", "properties": {}}),
    |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
)
.with_kind(ToolKind::External);

let definition = tool.definition();
assert_eq!(tool_metadata_kind(&definition.metadata), Some(ToolKind::External));
```

`FunctionTool` and `TypedFunctionTool` also support provider strict-schema preference and sequential execution preference. OpenAI tool requests receive `strict` when it is set; sequential preference remains provider-neutral metadata for runtime, hooks, and UI policy.

Runtime tool scheduling is parallel by default for independent tool calls returned in the same model response. The runtime falls back to model-order sequential execution when `AgentRuntimePolicy.tool_execution` is `Sequential`, when any requested tool definition has `sequential = true`, or when a response repeats the same tool name in one batch. Tool returns are still applied to model history in the original model tool-call order.

```rust
use starweaver_agent::{string_tool, ToolContext, Tool};

let tool = string_tool(
    "status",
    Some("Return status".to_string()),
    serde_json::json!({"type": "object", "properties": {}}),
    |_ctx: ToolContext, args: serde_json::Value| async move {
        Ok(starweaver_agent::ToolResult::new(args))
    },
)
.with_strict_schema(true)
.with_sequential(true);

let definition = tool.definition();
assert_eq!(definition.strict, Some(true));
assert_eq!(definition.sequential, Some(true));
```

`ToolError::with_private_metadata(...)` attaches host-only error details to
`ToolReturnPart.private_metadata`. These values are kept out of model-visible
tool content and public tool-return metadata, matching `ToolResult` private
metadata behavior for successful tool calls.

## Availability

Tools can decide whether they should be exposed for the current `AgentContext`. The runtime filters unavailable tools before the model request is prepared.

```rust
use std::sync::Arc;

use starweaver_agent::{AgentContext, string_tool, ToolContext, ToolRegistry, ToolResult};

let gated = string_tool(
    "gated",
    Some("Only visible when enabled".to_string()),
    serde_json::json!({"type": "object", "properties": {}}),
    |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
)
.with_availability(|context| {
    context
        .metadata
        .get("enable_gated_tool")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
});

let registry = ToolRegistry::new().with_tool(Arc::new(gated));
let hidden = AgentContext::default();
let mut visible = AgentContext::default();
visible
    .metadata
    .insert("enable_gated_tool".to_string(), serde_json::json!(true));

assert!(registry.definitions_for_context(&hidden).is_empty());
assert_eq!(registry.definitions_for_context(&visible)[0].name, "gated");

let report = registry.availability_report(&hidden);
assert!(report.available.is_empty());
assert_eq!(report.unavailable, vec!["gated".to_string()]);
```

The runtime publishes a `tools_unavailable` context event whenever context-aware filtering skips tools before a model request. Streaming runs surface that event as a custom stream record, so hosts can show diagnostics without exposing unavailable tools to the model.

Hosts that need fail-closed behavior can set `ToolConfig::unavailable_tool_policy` to
`ToolAvailabilityPolicy::FailRun`. The runtime still publishes `tools_unavailable`, then fails the
run before the model request is sent.

```rust
use starweaver_agent::{ToolAvailabilityPolicy, ToolConfig};

let tool_config = ToolConfig {
    unavailable_tool_policy: ToolAvailabilityPolicy::FailRun,
    ..ToolConfig::default()
};
assert_eq!(
    tool_config.unavailable_tool_policy,
    ToolAvailabilityPolicy::FailRun
);
```

Tools can also prepare their model-facing definition for a specific context. Use
`with_prepare_definition` when the tool remains executable but its description,
metadata, schema, or request visibility depends on tenant, policy, or host state.
Returning `None` hides the tool for that prepared request without removing it
from the registry.

```rust
use std::sync::Arc;

use starweaver_agent::{AgentContext, string_tool, ToolContext, ToolRegistry, ToolResult};

let tool = string_tool(
    "tenant_lookup",
    Some("Lookup tenant data".to_string()),
    serde_json::json!({"type": "object", "properties": {}}),
    |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
)
.with_prepare_definition(|context, mut definition| {
    let tenant = context.metadata.get("tenant").and_then(serde_json::Value::as_str)?;
    definition.description = Some(format!("Lookup data for tenant {tenant}"));
    Some(definition)
});

let registry = ToolRegistry::new().with_tool(Arc::new(tool));
assert!(registry.definitions_for_context(&AgentContext::default()).is_empty());
```

Function tools can run ordered raw-JSON argument validators before execution.
Validators can normalize arguments in place or return `ToolError::InvalidArguments`
to reject a call. `TypedFunctionTool` runs the same validator chain before Serde
parses typed arguments.

```rust
use std::sync::Arc;

use starweaver_agent::{
    string_tool, ToolContext, ToolError, ToolRegistry, ToolResult,
};

let tool = string_tool(
    "validated",
    Some("Validated tool".to_string()),
    serde_json::json!({"type": "object", "properties": {}}),
    |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
)
.with_argument_validator(|_context, arguments| {
    if arguments.get("allow").and_then(serde_json::Value::as_bool) == Some(true) {
        arguments["normalized"] = serde_json::json!(true);
        Ok(())
    } else {
        Err(ToolError::InvalidArguments {
            tool: "validated".to_string(),
            message: "allow must be true".to_string(),
        })
    }
});

let registry = ToolRegistry::new().with_tool(Arc::new(tool));
assert_eq!(registry.names(), vec!["validated"]);
```

## Fileops-loaded skills

Skills are markdown packages discovered through the active `EnvironmentProvider`, matching the SDK's fileops path space. `SkillRegistry::scan` reads compact frontmatter summaries from `.agents/skills/*/SKILL.md` and `skills/*/SKILL.md`; `SkillRegistry::activate` loads the full markdown body when a host or agent workflow chooses a skill. `SkillSourceScope` applies deterministic precedence: built-in, user shared, user tool-specific, workspace shared, then workspace tool-specific. Within a default scope, `skills` overrides `.agents/skills` for duplicate names.

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

Use `scan_with_report` when hosts should continue past duplicate or malformed skill files and surface diagnostics to users. Pass the report to `AgentBuilder::skills_report` when those diagnostics should be emitted as a `skills_scanned` runtime event at run start.

```rust
use std::sync::Arc;

use starweaver_agent::{
    AgentBuilder, AgentStreamEvent, SkillRegistry, SkillScanDiagnosticKind, SkillSourceScope,
    TestModel, SKILL_SCAN_EVENT_KIND,
};
use starweaver_environment::VirtualEnvironmentProvider;

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let provider = Arc::new(
    VirtualEnvironmentProvider::new("skills")
        .with_file(
            ".agents/skills/research/SKILL.md",
            r"---
name: research
description: Shared research
---
Shared instructions.
",
        )
        .with_file(
            "skills/research/SKILL.md",
            r"---
name: research
description: Tool-specific research
---
Tool instructions.
",
        ),
);

let report = SkillRegistry::scan_with_report(provider, &[SkillSourceScope::new("")]).await?;
assert_eq!(
    report.registry().get("research").unwrap().description,
    "Tool-specific research"
);
assert!(report
    .diagnostics()
    .iter()
    .any(|diagnostic| diagnostic.kind == SkillScanDiagnosticKind::DuplicateOverridden));

let stream = AgentBuilder::new(Arc::new(TestModel::with_text("ok")))
    .skills_report(report)
    .build()
    .run_stream("hello")
    .await?;
assert!(stream.events().iter().any(|record| {
    matches!(
        &record.event,
        AgentStreamEvent::Custom { event } if event.kind == SKILL_SCAN_EVENT_KIND
    )
}));
# Ok(())
# }
```

`skill_tools(registry.packages())` converts the discovered summaries into a model-facing instruction block. The full body stays loaded on activation so hosts can implement request-boundary reloads and file synchronization hooks. `SkillRegistry::activate_with_context` emits a `skill_activated` context event without placing the full body in the event payload.

Hosts that watch skill directories can call `reload_with_report` or `reload_with_context` after a file change. Reload reports compare the previous registry with the new scan and classify added, removed, and modified skills without embedding full skill bodies in events. Use `SkillReloadBinding` when a host owns skill-directory versions or file watcher events and needs deterministic interval/debounce decisions before calling `reload_with_context_if_due`. Scheduled reload events include `reload_scheduled`, `reload_reason`, `reload_ms`, and `inventory_version` diagnostics.

```rust
use std::sync::Arc;

use starweaver_agent::{
    AgentContext, SkillRegistry, SkillReloadChangeKind, SkillSourceScope,
    SKILL_RELOAD_EVENT_KIND,
};
use starweaver_environment::{EnvironmentProvider, VirtualEnvironmentProvider};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let provider = Arc::new(VirtualEnvironmentProvider::new("skills").with_file(
    "skills/research/SKILL.md",
    r"---
name: research
description: Gather sources
---
Use search tools.
",
));
let registry = SkillRegistry::scan(provider.clone(), &[SkillSourceScope::new("")]).await?;

provider
    .write_text(
        "skills/research/SKILL.md",
        r"---
name: research
description: Gather sources with citations
---
Use search tools.
",
    )
    .await?;

let mut context = AgentContext::default();
let report = registry
    .reload_with_context(provider, &[SkillSourceScope::new("")], &mut context)
    .await?;

assert!(report
    .changes()
    .iter()
    .any(|change| change.kind == SkillReloadChangeKind::Modified));
assert!(context
    .events
    .events()
    .iter()
    .any(|event| event.kind == SKILL_RELOAD_EVENT_KIND));
# Ok(())
# }
```

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
