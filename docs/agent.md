# Agents

An agent combines a model, instructions, optional tools, output policy, capability hooks, and runtime policy.

## Basic agent

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, TestModel};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let agent = AgentBuilder::new(Arc::new(TestModel::with_text("ok")))
    .instruction("Answer with short responses.")
    .build();

let result = agent.run("Say ok").await?;
assert_eq!(result.output, "ok");
# Ok(())
# }
```

## Agent builder responsibilities

`AgentBuilder` lives in `starweaver-agent` and produces a `starweaver-runtime::Agent`. It can configure:

- static and dynamic instructions
- model settings and request parameters
- tools and toolsets
- structured output schemas and validators
- output functions
- usage limits
- history processors
- capability hooks and bundles
- runtime policy

## Runtime result

`AgentResult` contains:

- `output`: final text
- `structured_output`: parsed JSON value when configured
- `messages`: canonical message history
- `state`: checkpointable run state
- `history_len`: prior history length

Use `new_messages()` to read messages produced by the current run.
