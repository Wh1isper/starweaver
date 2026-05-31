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
