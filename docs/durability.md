# Durability

Durable execution is handled through `AgentExecutor`. The runtime emits checkpoints at explicit agent-loop boundaries, and the executor decides whether to continue or suspend.

```rust
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use starweaver_agent::{
    AgentBuilder, AgentCheckpoint, AgentExecutionDecision, AgentExecutionNode, AgentExecutor,
    AgentExecutorError, TestModel,
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

Service runtimes can persist `AgentCheckpoint` values, suspend on interruption, and resume from stored state.

## Resume evidence for SessionStore implementations

Every `AgentCheckpoint` includes `resume: AgentResumeEvidence`. This compact evidence is designed for the CLI, service runtimes, and external applications that implement a real `SessionStore`.

A durable store should persist these records together:

- session id and conversation id
- exported `AgentContext` state, including compact-restore inputs (`user_prompts`, `previous_assistant_response_reference`, and `steering_messages`)
- `AgentStreamRecord` events with sequence numbers
- `AgentCheckpoint` values and their `resume` evidence
- environment state reference from the service layer
- trace id and span ids from the service tracer

```rust
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use starweaver_agent::{
    AgentBuilder, AgentCheckpoint, AgentExecutionDecision, AgentExecutor, AgentExecutorError,
    TestModel,
};

#[derive(Default)]
struct SessionStoreExecutor {
    checkpoints: Mutex<Vec<AgentCheckpoint>>,
}

#[async_trait]
impl AgentExecutor for SessionStoreExecutor {
    async fn checkpoint(
        &self,
        checkpoint: AgentCheckpoint,
    ) -> Result<AgentExecutionDecision, AgentExecutorError> {
        self.checkpoints.lock().expect("checkpoint lock").push(checkpoint);
        Ok(AgentExecutionDecision::Continue)
    }
}

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let store = Arc::new(SessionStoreExecutor::default());
let agent = AgentBuilder::new(Arc::new(TestModel::with_text("ok")))
    .build()
    .with_executor(store.clone());

agent.run("hello").await?;
let checkpoints = store.checkpoints.lock().expect("checkpoint lock");
let final_checkpoint = checkpoints.last().expect("checkpoint exists");
assert_eq!(final_checkpoint.resume.cursor.message_cursor, final_checkpoint.state.message_history.len());
# Ok(())
# }
```

`resume` gives durable runtimes a stable cursor contract. The full `state` field remains available for exact runtime restoration and audit archives.
