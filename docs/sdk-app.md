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
