# Message History

Starweaver stores canonical model history as `ModelMessage` values. Results expose both all messages and messages produced by the current run.

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, TestModel};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let agent = AgentBuilder::new(Arc::new(TestModel::with_text("first"))).build();
let first = agent.run("hello").await?;
assert_eq!(first.new_messages().len(), first.all_messages().len());

let second_agent = agent
    .override_config()
    .model(Arc::new(TestModel::with_text("second")))
    .build();
let second = second_agent
    .run_with_history("continue", first.all_messages().to_vec())
    .await?;

assert_eq!(second.output, "second");
assert!(second.all_messages().len() > second.new_messages().len());
# Ok(())
# }
```

History processors can compact or filter provider-facing history while preserving the canonical run state.
