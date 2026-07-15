# Subagents

Subagents are SDK-level application protocols. Register named runtime agents in `SubagentRegistry`, then delegate prompts through the SDK app or registry.

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, AgentContext, SubagentConfig, TestModel};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child"))).build());
let app = AgentBuilder::new(Arc::new(TestModel::with_text("parent")))
    .subagent(SubagentConfig::new("research", child))
    .build_app();

let parent = app.run("plan").await?;
assert_eq!(parent.output, "parent");

let mut context = AgentContext::default();
let child = app
    .subagents()
    .delegate("research", "collect facts", &mut context)
    .await?;
assert_eq!(child.output, "child");
# Ok(())
# }
```

Use `SubagentTask` when the application wants to attach task metadata and receive a delegation envelope.

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, AgentContext, SubagentConfig, SubagentTask, TaskId, TestModel};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child"))).build());
let app = AgentBuilder::new(Arc::new(TestModel::with_text("parent")))
    .subagent(SubagentConfig::new("research", child))
    .build_app();

let mut context = AgentContext::default();
let task = SubagentTask::new("collect facts")
    .with_id(TaskId::from_string("research-1"))
    .with_metadata(serde_json::json!({"source": "docs"}));
let delegated = app
    .subagents()
    .delegate_task("research", task, &mut context)
    .await?;

assert_eq!(delegated.name, "research");
assert_eq!(delegated.task.id.as_str(), "research-1");
assert_eq!(delegated.output(), "child");
# Ok(())
# }
```

The registry shares usage and dependencies with child contexts. The task envelope is the extension point for lifecycle, cancellation, polling, and nested delegation guardrails.

Use `SubagentConfig::with_execution_hook` to wrap delegated child runs with application policy. A `SubagentExecutionHook` receives typed metadata before and after the child run, can mutate the child context before execution, and observes the final output, usage, run id, or error without changing the default delegation contract.

```rust
use std::sync::Arc;

use async_trait::async_trait;
use starweaver_agent::{
    AgentBuilder, AgentContext, SubagentConfig, SubagentExecutionHook,
    SubagentExecutionMetadata, SubagentExecutionOutcome, TestModel,
};

struct AuditHook;

#[async_trait]
impl SubagentExecutionHook for AuditHook {
    async fn before_subagent_run(
        &self,
        metadata: SubagentExecutionMetadata,
        child_context: &mut AgentContext,
    ) -> Result<(), starweaver_agent::AgentError> {
        child_context.metadata.insert(
            "subagent.audit_name".to_string(),
            serde_json::json!(metadata.name),
        );
        Ok(())
    }

    async fn after_subagent_run(
        &self,
        _metadata: SubagentExecutionMetadata,
        _child_context: &AgentContext,
        outcome: SubagentExecutionOutcome,
    ) -> Result<(), starweaver_agent::AgentError> {
        assert!(matches!(outcome, SubagentExecutionOutcome::Completed { .. }));
        Ok(())
    }
}

let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child"))).build());
let config = SubagentConfig::new("research", child).with_execution_hook(Arc::new(AuditHook));

assert_eq!(config.name, "research");
```

Expose delegation to the model with the typed `delegate` tool when an agent should choose a registered child agent during a run.

```rust
use std::sync::Arc;

use starweaver_agent::{
    AgentBuilder, AgentContext, AgentContextHandle, SubagentConfig, SubagentRegistry, TestModel,
    ToolContext,
};
use starweaver_agent::{ConversationId, RunId};

# async fn example() -> Result<(), starweaver_agent::ToolError> {
let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child"))).build());
let registry = Arc::new(
    SubagentRegistry::new().with_subagent(SubagentConfig::new("research", child)),
);
let delegate = registry.delegate_tool();
let parent = AgentContext::default();
let context_handle = AgentContextHandle::new(parent.clone());
let mut dependencies = parent.dependencies.clone();
dependencies.insert(parent);
dependencies.insert(context_handle);

let result = delegate
    .call(
        ToolContext::new(RunId::default(), ConversationId::default(), 0)
            .with_dependencies(dependencies),
        serde_json::json!({"name": "research", "prompt": "collect facts"}),
    )
    .await?;

assert_eq!(result.content["name"], "research");
assert_eq!(result.content["output"], "child");
# Ok(())
# }
```

## Asynchronous Delegation

The SDK keeps blocking delegation as its compatibility default. Long-lived hosts can opt into async-only model delegation by injecting one `BackgroundSubagentSupervisor` that outlives individual parent runtimes:

```rust
use std::sync::Arc;
use std::time::Duration;

use starweaver_agent::{
    AgentBuilder, BackgroundSubagentSupervisor, SubagentConfig, SubagentDelegationMode, TestModel,
};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child"))).build());
let supervisor = Arc::new(BackgroundSubagentSupervisor::new());
let agent = AgentBuilder::new(Arc::new(TestModel::with_text("parent")))
    .subagent(SubagentConfig::new("research", child))
    .subagent_delegation_mode(SubagentDelegationMode::Async)
    .background_subagent_supervisor(supervisor.clone())
    .build();

let names = agent.tools().names();
assert!(names.contains(&"delegate".to_string()));
assert!(names.contains(&"steer_subagent".to_string()));
assert!(names.contains(&"cancel_subagent".to_string()));
assert!(names.contains(&"wait_subagent".to_string()));

supervisor
    .shutdown_checked(Some(Duration::from_secs(1)))
    .await?;
# Ok(())
# }
```

Async `delegate` returns a stable background `agent_id` and a new `attempt_id` for each execution attempt. Use the attempt ID for `steer_subagent`, `cancel_subagent`, and one bounded `wait_subagent`; do not implement polling loops. A post-terminal continuation may reuse its known `agent_id`, but receives a fresh attempt ID. Unknown caller-supplied agent IDs and arbitrary model-supplied host metadata are rejected.

A host that enables this mode must also keep the task runtime alive, serialize parent continuations, deliver the supervisor completion callback, and call `shutdown_checked` before dropping its runtime. A deadline error retains owned work: keep the supervisor and runtime alive and retry. The no-result `shutdown` method is a best-effort compatibility helper, not the strict product-host lifecycle API. Interactive CLI/TUI uses async-only delegation with a session-scoped supervisor. One-shot CLI runs remain blocking, while worker mode uses `SubagentDelegationMode::Disabled` and installs no delegation tools.

### Durable RPC delegation

The standalone RPC host materializes named subagents from RPC-owned profiles. Parent profiles explicitly select the names they expose:

```toml
[profiles.default]
model_id = "openai-responses:gpt-5"
subagents = ["research"]

[profiles.research]
model_id = "openai-responses:gpt-5-mini"
toolsets = ["filesystem", "grep"]

[subagents.research]
profile = "research"
description = "Collect bounded evidence"
required_tools = ["grep"]
optional_tools = ["view"]
```

RPC always exposes the async-only topology for configured subagents: `delegate`, `steer_subagent`, `cancel_subagent`, `wait_subagent`, and `subagent_info`. The hidden blocking backend remains executable internally but is absent from model definitions; `spawn_delegate` is not installed. Child profiles have nested subagents disabled.

One supervisor is retained per durable session for the service lifetime. Acceptance, fenced owner lease and heartbeat, execution transitions, terminal outcome, result retention, delivery claims, and parent/child/continuation IDs are persisted through `SessionStore`. The store rejects a second active attempt for the same session-scoped `agent_id`. A late terminal result is atomically linked to one new run with `trigger_type = "async_subagent_result"`; RPC never mutates the terminal parent run. Continuation admission binds a typed cause (`attempt_id`, agent and run identities, result digest/size, and trace) to the SHA-256 digest of the exact canonical durable input. The store independently derives that input, including complete artifact content or an expiry summary, and returns a store-derived cause in the receipt before marking delivery complete. Exact callback or restart retries cannot admit another logical continuation for the same attempt.

Inline result previews are bounded. Oversized successful output is atomically committed to a host-owned artifact with its preview, pre-truncation byte size, versioned domain-separated digest, and retention deadline. SDK and session storage use the same result-digest helper. Per-artifact and aggregate namespace byte limits are enforced in the same storage transaction against store-owned current time, after exact terminal retries are recognized; in-process prechecks count only unexpired projections. The first terminal commit also stores a fingerprint of the complete immutable result, artifact binding, trace, owner generation, terminal timestamp, and initial retention projection. Restart reconciliation stores the same fingerprint when it synthesizes a process-loss terminal outcome, and retention expiry atomically backfills a missing legacy fingerprint only while the complete terminal projection still exists. Already-expired legacy evidence without a fingerprint remains fail closed. Later replay compares that fingerprint even after mutable delivery or retention state changes. Loads reject expired or digest-mismatched content, and RPC resolves complete artifact content before continuation admission or pending-result hydration. Expiry deletes retained content while preserving terminal, delivery, digest, size, and terminal-commit fingerprint audit evidence. Durable failure text is reduced to a safe public category.

On startup and during service operation, RPC preserves non-terminal attempts with an unexpired foreign-host owner lease and rejects an acceptance whose lease is already expired at store-owned current time. An expired owner cannot revive itself by heartbeat, advance execution, or make the first terminal commit; only exact replay of already-committed terminal evidence remains idempotent. The reconciler retains a deadline-expired result claim while its linked consumer run is active under a live matching admission lease, acknowledges a durably completed consumer, and releases absent, failed/cancelled, or lease-lost consumers. Completion handlers invoke reconciliation and reload the record; they never sleep until a claim deadline or directly release a run-owned claim. Failed/cancelled consumer release records a durable automatic-continuation suppression run ID, so reconciliation cannot immediately create a replacement automatic run while a later explicit run can still consume the result exactly once. A cancelled causal parent, deletion fence, inactive session, unknown profile, or shutdown likewise suppresses automatic continuation while preserving durable result evidence. Service shutdown stops admission, cooperatively cancels children, aborts and joins every owned worker under one deadline, and reconciles remaining records. Context deltas become visible only after terminal persistence succeeds; resumed child history contributes only a validated suffix. Finalizers are registered behind a start barrier, panic becomes failed terminal evidence delivered through the normal fallback-message and callback path, and shutdown drains both finalizers and their owned execution workers. Durable panic terminal writes use the ordinary fenced retry and heartbeat loop. Transient commit, heartbeat, and store-read failures retain local active ownership and retry at or below the heartbeat interval; one in-flight heartbeat refresh is selected alongside worker completion and cancellation so a slow store call cannot starve either branch. An applied commit whose response was lost is recovered by exact terminal replay. Local active state is abandoned only after a stale fence, missing record, changed owner, or expired lease confirms owner loss. Shutdown treats confirmed owner loss as drained, bounds each forced terminal-persistence await by its absolute deadline, and still drains late finalizers after an error. Checked shutdown calls are serialized, and a cancellation-safe drain guard re-registers temporarily removed finalizer handles if the shutdown future itself is cancelled. Active evidence without a live finalizer skips the cooperative wait so retry budget remains available for store drain and exact replay. A successful `shutdown_checked` drains every finalizer and store-owned background operation. If its deadline expires, it returns without an unbounded join and retains unfinished finalizer handles and SQLite blocking-operation tracking; keep the runtime and supervisor alive, then retry checked shutdown so the store can drain and exact-replay any terminal commit. Store implementations must explicitly provide either a cancellation-safe no-op drain or actual outstanding-operation tracking; the default fails closed.

Durable attempt inspection and cancellation are trusted RPC application operations. They are not added to the external v1 JSON-RPC method table and do not reuse session CRUD or `run.cancel` semantics.

## Lifecycle Events

Delegation publishes typed lifecycle payloads through the parent context event bus. Applications can observe `subagent_started`, `subagent_completed`, and `subagent_failed` records and deserialize the payload as `SubagentLifecycleEvent`.

```rust
use std::sync::Arc;

use starweaver_agent::{
    AgentBuilder, SubagentConfig, SubagentLifecycleEvent, SubagentLifecycleKind, SubagentTask,
    AgentContext, TaskId, TestModel,
};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child"))).build());
let app = AgentBuilder::new(Arc::new(TestModel::with_text("parent")))
    .subagent(SubagentConfig::new("research", child))
    .build_app();
let task = SubagentTask::new("collect facts")
    .with_id(TaskId::from_string("research-1"));
let mut context = AgentContext::default();

app.subagents()
    .delegate_task("research", task, &mut context)
    .await?;

let started: SubagentLifecycleEvent =
    serde_json::from_value(context.events.events()[0].payload.clone()).unwrap();
assert_eq!(started.kind, SubagentLifecycleKind::Started);
assert_eq!(started.task_id.as_str(), "research-1");
# Ok(())
# }
```

## Stream Attribution

When the `delegate` tool runs inside a parent agent stream, child stream records are merged into the parent stream with `AgentStreamRecord.source` set to subagent attribution. The source records the child agent id, child agent name, task id, child run id, parent run id, and the original child sequence before rebasing into the parent sequence.

In blocking mode, the parent waits for the child run to finish and emits attributed child records before the parent tool return. In async mode, child records retain the same source attribution while the supervisor owns the attempt independently of the accepting parent turn. Both modes let live handles, stream archives, and replay consumers identify child records without changing the top-level stream event enum.

## Markdown Configuration

`SubagentSpec` is the serializable portion of a subagent definition. It can be loaded from markdown frontmatter and passed to service or CLI layers without carrying a runtime agent handle.

```rust
use starweaver_agent::parse_subagent_markdown;

let spec = parse_subagent_markdown(r"
---
name: debugger
description: Debug code issues
tools:
  - grep
  - view
optional_tools: edit, shell
model: anthropic:claude-sonnet-4
---
You are a debugging expert.
").unwrap();

assert_eq!(spec.name, "debugger");
assert_eq!(spec.tools, vec!["grep", "view"]);
assert_eq!(spec.optional_tools, vec!["edit", "shell"]);
assert_eq!(spec.system_prompt, "You are a debugging expert.");
```

Runtime `SubagentConfig` keeps the executable agent handle in programmatic code. This split lets files, services, and CLI commands exchange serializable specs while applications decide how each spec maps to a concrete runtime agent.

Use `project_subagent_spec` when a host wants to materialize markdown subagent files through the same `AgentSpec` registry path as regular agents. The projection returns a child `AgentSpec`, `SubagentToolInheritancePolicy`, and `SubagentCapabilityInheritancePolicy`; pass the agent spec and tool policy into `SubagentConfig::from_agent_spec` to build an executable child agent without manually constructing a nested `AgentBuilder`. The capability policy is also preserved in the projected agent metadata, so `SubagentConfig::from_agent_spec` applies it automatically. If the child spec resolves an environment provider through `AgentSpecRegistry`, the delegated child context uses that provider; otherwise it inherits the provider already attached to the parent context.

If a markdown subagent omits `model` or sets `model: inherit`, the projection requires a concrete inherited model id from the host because executable Rust agents still resolve models through `AgentSpecRegistry`. `tools` and `optional_tools` become inherited parent-tool policy, not child-owned toolsets. With no explicit tool lists, the projected policy inherits all parent tools except denied tools and guarded delegation tools.

## Tool Inheritance

`SubagentToolInheritancePolicy` controls which parent tools are appended to a child agent at delegation time. Required tools gate availability, optional tools attach when present, denied tools are removed, and tools with metadata `auto_inherit=true` are inherited by default.

```rust
use std::sync::Arc;

use starweaver_agent::{
    FunctionTool, SubagentToolInheritancePolicy, ToolContext, ToolRegistry, ToolResult,
};

let mut metadata = serde_json::Map::new();
metadata.insert("auto_inherit".to_string(), serde_json::json!(true));
let task_list = Arc::new(
    FunctionTool::new(
        "task_list",
        Some("List tasks".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    )
    .with_metadata(metadata),
);
let parent = ToolRegistry::new().with_tool(task_list);
let policy = SubagentToolInheritancePolicy::new(vec!["task_list".to_string()], vec![]);
let inherited = policy.resolve(&parent).unwrap();

assert_eq!(inherited.names(), vec!["task_list"]);
```

`SubagentConfig::with_tool_inheritance(policy)` attaches the policy to one child. The SDK also keeps nested delegation guarded by default through a subagent stack in context metadata; opt into nested coordination with `with_nested_delegation(true)` when the application has explicit recursion policy.

When `subagent_info` is called inside an agent run, it uses the same parent tool registry that delegation would use and reports each child subagent's `available`, `inherited_tools`, and `diagnostics` fields. If delegation is attempted while a required inherited tool is missing or denied, the parent context receives a `subagent_failed` lifecycle event whose metadata includes `error_kind`, `tool_name`, and a human-readable message.

Markdown frontmatter can include `denied_tools`; the parsed `SubagentSpec` stores that list in metadata so services and CLI layers can map it into runtime inheritance policy.

## Capability Inheritance

`SubagentCapabilityInheritancePolicy` controls whether a delegated child receives parent SDK builder hooks or capability bundles. Inheritance is explicit: parent hook capabilities are inherited only when `hooks` is enabled, and parent capability bundles are inherited only when `capability_bundles` is enabled. `denied_capabilities` filters by hook capability id, bundle capability id, or bundle name.

```rust
use std::sync::Arc;

use starweaver_agent::{
    AgentBuilder, StaticCapabilityBundle, SubagentCapabilityInheritancePolicy, SubagentConfig,
    TestModel,
};

let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child"))).build());
let bundle = Arc::new(StaticCapabilityBundle::new("parent-bundle"));

let agent = AgentBuilder::new(Arc::new(TestModel::with_text("parent")))
    .capability_bundle(bundle)
    .subagent(
        SubagentConfig::new("child", child).with_capability_inheritance(
            SubagentCapabilityInheritancePolicy::default().with_capability_bundles(true),
        ),
    )
    .build();
# let _ = agent;
```

Markdown frontmatter can declare the same policy with `inherit_hooks`, `inherit_capabilities`, and `denied_capabilities`. `project_subagent_spec` keeps those settings in the projection and `SubagentConfig::from_agent_spec` applies them from the projected `AgentSpec` metadata.
