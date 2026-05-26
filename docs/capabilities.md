# Capabilities

Capabilities are runtime extension hooks. They can inspect context, annotate tools, mutate model requests, skip model calls, validate output, and observe completion.

```rust
use std::sync::Arc;

use async_trait::async_trait;
use starweaver_agent::{AgentBuilder, AgentCapability, CapabilityResult, TestModel};
use starweaver_context::AgentContext;
use starweaver_runtime::AgentRunState;

struct CompleteMarker;

#[async_trait]
impl AgentCapability for CompleteMarker {
    async fn on_run_complete_with_context(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
    ) -> CapabilityResult<()> {
        context.state.set(
            "last_output",
            serde_json::json!(state.output.clone().unwrap_or_default()),
        );
        Ok(())
    }
}

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let mut context = AgentContext::default();
let agent = AgentBuilder::new(Arc::new(TestModel::with_text("ok")))
    .capability(Arc::new(CompleteMarker))
    .build();

let result = agent.run_with_context("hello", &mut context).await?;
assert_eq!(result.output, "ok");
assert_eq!(context.state.get("last_output"), Some(&serde_json::json!("ok")));
# Ok(())
# }
```

Capability bundles can package hooks, instructions, tools, settings, output validators, output functions, history processors, and usage limits.
