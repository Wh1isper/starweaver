# Durability

Durable execution is handled through `AgentExecutor`. The runtime emits checkpoints at explicit agent-loop boundaries, and the executor decides whether to continue or suspend.

```rust
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use starweaver_agent::{AgentBuilder, TestModel};
use starweaver_runtime::{
    AgentCheckpoint, AgentExecutionDecision, AgentExecutionNode, AgentExecutor, AgentExecutorError,
};

#[derive(Default)]
struct RecordingExecutor {
    nodes: Mutex<Vec<AgentExecutionNode>>,
}

#[async_trait]
impl AgentExecutor for RecordingExecutor {
    async fn checkpoint(
        &self,
        checkpoint: AgentCheckpoint,
    ) -> Result<AgentExecutionDecision, AgentExecutorError> {
        self.nodes.lock().expect("nodes lock").push(checkpoint.node);
        Ok(AgentExecutionDecision::Continue)
    }
}

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let executor = Arc::new(RecordingExecutor::default());
let agent = AgentBuilder::new(Arc::new(TestModel::with_text("ok")))
    .build()
    .with_executor(executor.clone());

let result = agent.run("hello").await?;
assert_eq!(result.output, "ok");
assert!(executor.nodes.lock().expect("nodes lock").contains(&AgentExecutionNode::RunComplete));
# Ok(())
# }
```

Future service runtimes can persist `AgentCheckpoint` values, suspend on interruption, and resume from stored state.
