# Dependencies

`AgentContext` stores typed and named dependencies. Runtime hooks and tools can read the same dependency values during a run.

## Tool dependency

```rust
use std::sync::Arc;

use serde_json::json;
use starweaver_agent::{AgentBuilder, FunctionTool, TestModel, ToolContext, ToolRegistry, ToolResult};
use starweaver_context::AgentContext;

#[derive(Debug)]
struct Tenant(String);

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let tool = FunctionTool::new(
    "tenant",
    Some("Return tenant".to_string()),
    json!({"type": "object"}),
    |ctx: ToolContext, _args: serde_json::Value| async move {
        let tenant = ctx.dependency::<Tenant>().expect("tenant dependency");
        Ok(ToolResult::new(json!({"tenant": tenant.0})))
    },
);

let tools = ToolRegistry::new().with_tool(Arc::new(tool));
let mut context = AgentContext::default();
context.insert_dependency(Tenant("acme".to_string()));

let agent = AgentBuilder::new(Arc::new(TestModel::with_text("done")))
    .tool_registry(tools)
    .build();
let result = agent.run_with_context("hello", &mut context).await?;
assert_eq!(result.output, "done");
# Ok(())
# }
```

Dependencies are process-local and skipped during serialization. Service runtimes should rehydrate them from application configuration after restoring resumable state.
