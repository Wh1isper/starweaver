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

# async fn example() -> Result<(), starweaver_agent::AgentError> {
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

supervisor.shutdown(Some(Duration::from_secs(1))).await;
# Ok(())
# }
```

Async `delegate` returns a stable background `agent_id` and a new `attempt_id` for each execution attempt. Use the attempt ID for `steer_subagent`, `cancel_subagent`, and one bounded `wait_subagent`; do not implement polling loops. A post-terminal continuation may reuse its known `agent_id`, but receives a fresh attempt ID. Unknown caller-supplied agent IDs and arbitrary model-supplied host metadata are rejected.

A host that enables this mode must also keep the task runtime alive, serialize parent continuations, deliver the supervisor completion callback, and call `shutdown` before dropping its runtime. Interactive CLI/TUI uses async-only delegation with a session-scoped supervisor. One-shot CLI runs remain blocking, while worker mode uses `SubagentDelegationMode::Disabled` and installs no delegation tools.

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
