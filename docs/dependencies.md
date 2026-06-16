# Dependencies

`AgentContext` stores typed and named dependencies. Runtime hooks and tools can read the same dependency values during a run.

## Tool dependency

```rust
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starweaver_agent::{
    typed_tool, AgentBuilder, AgentContext, TestModel, ToolContext, ToolRegistry, ToolResult,
};

#[derive(Debug)]
struct Tenant(String);

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
struct TenantArgs {}

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let tool = typed_tool::<TenantArgs, _, _>(
    "tenant",
    Some("Return tenant".to_string()),
    |ctx: ToolContext, _args: TenantArgs| async move {
        let tenant = ctx.dependency::<Tenant>().expect("tenant dependency");
        Ok(ToolResult::new(serde_json::json!({"tenant": tenant.0})))
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

## Notes

`AgentContext` also carries serializable notes for lightweight session memory. Notes round-trip through `ResumableState`; runtime context exposes note keys while keeping note values out of model-facing prompt text.

```rust
use starweaver_agent::AgentContext;

let mut context = AgentContext::default();
context.notes.set("lang", "Chinese");
context.notes.set("os", "macOS");

let restored = AgentContext::from_state(context.export_state());
assert_eq!(restored.notes.get("lang"), Some("Chinese"));

let runtime_context = restored.render_runtime_context(true).unwrap();
assert!(runtime_context.contains("key=\"lang\""));
assert!(!runtime_context.contains("Chinese"));
```
