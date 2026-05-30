# Graph Inspection

The runtime graph is a deterministic state machine. Applications can inspect transitions for debugging, tests, dashboards, and durable execution diagnostics.

```rust
use starweaver_agent::{AgentBuilder, AgentNode, AgentRunState, ConversationId, RunId, TestModel};

# async fn example() -> Result<(), starweaver_agent::GraphError> {
let agent = AgentBuilder::new(std::sync::Arc::new(TestModel::with_text("ok"))).build();
let mut state = AgentRunState::new(RunId::new(), ConversationId::new());
state.output = Some("ok".to_string());

let trace = agent.inspect_graph(AgentNode::FinalizeRun, &state)?;
assert!(trace.is_complete());
assert_eq!(trace.steps()[0].current, AgentNode::FinalizeRun);
# Ok(())
# }
```

`AgentGraphTrace` is compact and serializable. `AgentCheckpoint` remains the durable evidence source for real runs, while graph inspection gives tools and UIs a safe way to explain why the runtime chooses a next node.

## Iteration trace

Use `run_iter` when the application wants a compact trace of an actual run.

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, AgentIterationKind, TestModel};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let agent = AgentBuilder::new(Arc::new(TestModel::with_text("ok"))).build();
let iter = agent.run_iter("hello").await?;

assert_eq!(iter.result.output, "ok");
assert!(iter.iterations.is_complete());
assert!(iter.iterations.steps().iter().any(|step| {
    step.kind == AgentIterationKind::ModelRequest
}));
# Ok(())
# }
```

`AgentSession::run_iter` exposes the same shape for SDK applications that keep context across turns.
