# SDK Apps

`AgentApp` keeps application-facing protocols above the core runtime. It wraps a runtime agent and carries SDK-level registries such as subagents.

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, TestModel};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let app = AgentBuilder::new(Arc::new(TestModel::with_text("planned")))
    .instruction("Plan before answering.")
    .build_app();

let result = app.run("plan").await?;
assert_eq!(result.output, "planned");
assert_eq!(app.subagents().subagents().len(), 0);
# Ok(())
# }
```

Use `build()` when only the core runtime agent is needed. Use `build_app()` when the application needs SDK protocols such as subagent delegation or session management.

## Sessions

`AgentSession` keeps an `AgentContext` next to the app's runtime agent. Use it for multi-turn applications that need message history, usage, state, events, typed dependencies, and export/restore through one SDK object.

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, FunctionModel, Usage};
use starweaver_model::ModelResponse;

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let model = FunctionModel::new(|_messages, _settings, _info| {
    Ok(ModelResponse {
        usage: Usage {
            requests: 1,
            ..Usage::default()
        },
        ..ModelResponse::text("ok")
    })
});
let app = AgentBuilder::new(Arc::new(model)).build_app();
let mut session = app.session();

let first = session.run("hello").await?;
let second = session.run("again").await?;

assert_eq!(first.output, "ok");
assert_eq!(second.output, "ok");
assert_eq!(session.context().usage.requests, 2);
# Ok(())
# }
```

Restore a session from exported state when an application persists context between process lifetimes.

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, FunctionModel, Usage};
use starweaver_model::ModelResponse;

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let model = FunctionModel::new(|_messages, _settings, _info| {
    Ok(ModelResponse {
        usage: Usage {
            requests: 1,
            ..Usage::default()
        },
        ..ModelResponse::text("ok")
    })
});
let app = AgentBuilder::new(Arc::new(model)).build_app();
let mut session = app.session();
session.run("hello").await?;

let state = session.export_state();
let mut restored = app.session_from_state(state);
let result = restored.run("again").await?;

assert_eq!(result.output, "ok");
assert_eq!(restored.context().usage.requests, 2);
# Ok(())
# }
```

## Smooth durable application shape

A production application can depend on `starweaver-agent` for the programming surface and keep service concerns in its own `SessionStore` implementation.

```mermaid
flowchart TD
    User[User request] --> App[AgentApp or AgentSession]
    App --> Runtime[Runtime Agent]
    Runtime --> Model[ModelAdapter]
    Runtime --> Tools[ToolRegistry]
    Runtime --> Stream[AgentStreamRecord]
    Runtime --> Checkpoint[AgentCheckpoint]
    Stream --> Store[SessionStore]
    Checkpoint --> Store
    App --> Context[AgentContext export]
    Context --> Store
```

Recommended shape for Claw, CLI, and external services:

1. Build an agent through `AgentBuilder` and policies from application configuration.
2. Create an `AgentSession` per conversation and persist `session.export_state()` after each run.
3. Use `run_stream` for UI/SSE output and persist each `AgentStreamRecord` by sequence.
4. Install an `AgentExecutor` that writes every `AgentCheckpoint` and `AgentResumeEvidence` to the store.
5. Persist environment references, trace ids, and approval/deferred metadata in the service layer alongside checkpoint ids.

This keeps the SDK surface small for application programmers: `AgentBuilder`, `AgentSession`, stream events, checkpoints, and direct APIs cover most durable app needs.
