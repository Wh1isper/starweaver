# Subagents

Subagents are SDK-level application protocols. Register named runtime agents in `SubagentRegistry`, then delegate prompts through the SDK app or registry.

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, SubagentConfig, TestModel};
use starweaver_context::AgentContext;

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

use starweaver_agent::{AgentBuilder, SubagentConfig, SubagentTask, TestModel};
use starweaver_context::AgentContext;

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child"))).build());
let app = AgentBuilder::new(Arc::new(TestModel::with_text("parent")))
    .subagent(SubagentConfig::new("research", child))
    .build_app();

let mut context = AgentContext::default();
let task = SubagentTask::new("collect facts")
    .with_metadata(serde_json::json!({"task_id": "research-1"}));
let delegated = app
    .subagents()
    .delegate_task("research", task, &mut context)
    .await?;

assert_eq!(delegated.name, "research");
assert_eq!(delegated.output(), "child");
# Ok(())
# }
```

The registry shares usage and dependencies with child contexts. The task envelope is the extension point for lifecycle, cancellation, polling, and nested delegation guardrails.
