# Quickstart

This guide builds a small agent, adds a typed tool, and runs it with deterministic model output.
The examples use `TestModel` so they compile and run without provider credentials.

## Add the SDK

From a published release:

```toml
[dependencies]
starweaver-agent = "0.0.1"
```

From a checkout before the first release:

```toml
[dependencies]
starweaver-agent = { path = "crates/starweaver-agent" }
```

## Run an agent

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, TestModel};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let agent = AgentBuilder::new(Arc::new(TestModel::with_text("Paris")))
    .instruction("Answer in one word.")
    .build();

let result = agent.run("What is the capital of France?").await?;
assert_eq!(result.output, "Paris");
# Ok(())
# }
```

`AgentBuilder::build()` returns the reusable runtime agent. Use `build_app()` when your
application needs SDK sessions, subagents, or app-level helper protocols.

## Add a typed tool

Tools are ordinary async Rust functions with typed JSON arguments. Starweaver derives the
model-facing JSON Schema from the argument type.

```rust
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starweaver_agent::{typed_tool, AgentBuilder, TestModel, ToolContext, ToolResult};

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
struct TicketArgs {
    /// Ticket identifier to fetch.
    id: String,
}

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let fetch_ticket = typed_tool::<TicketArgs, _, _>(
    "fetch_ticket",
    Some("Fetch a support ticket".to_string()),
    |_ctx: ToolContext, args: TicketArgs| async move {
        Ok(ToolResult::new(serde_json::json!({
            "id": args.id,
            "priority": "high"
        })))
    },
);

let agent = AgentBuilder::new(Arc::new(TestModel::with_text("high")))
    .instruction("Use tools when useful.")
    .tool(Arc::new(fetch_ticket))
    .build();

let result = agent.run("What is the priority for ticket T-1?").await?;
assert_eq!(result.output, "high");
# Ok(())
# }
```

## Return structured output

Use `OutputPolicy::typed_named` when the application wants typed JSON output.

```rust
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starweaver_agent::{AgentBuilder, OutputPolicy, TestModel};

#[derive(Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
struct Answer {
    answer: String,
}

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let agent = AgentBuilder::new(Arc::new(TestModel::with_text(r#"{"answer":"Paris"}"#)))
    .output_policy(OutputPolicy::typed_named::<Answer>("answer"))
    .build();

let result = agent.run("Return JSON.").await?;
let answer: Answer = result.structured()?;
assert_eq!(answer, Answer { answer: "Paris".to_string() });
# Ok(())
# }
```

## Keep a session

`AgentSession` keeps `AgentContext` across turns. Use it when you need message history, usage,
state, dependencies, notes, or export/restore.

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, TestModel};
use starweaver_model::ModelResponse;

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let app = AgentBuilder::new(Arc::new(TestModel::with_responses(vec![
    ModelResponse::text("ok"),
    ModelResponse::text("ok"),
])))
.build_app();
let mut session = app.session();

let first = session.run("hello").await?;
let second = session.run("again").await?;

assert_eq!(first.output, "ok");
assert_eq!(second.output, "ok");
# Ok(())
# }
```

## Run the CLI locally

```bash
make cli -- -p "hello" --output text
make sw -- version
```
