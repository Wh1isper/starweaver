# Models

`starweaver-model` defines provider-neutral model primitives and provider protocol clients.

## Deterministic test model

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, TestModel};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let model = Arc::new(TestModel::with_text("deterministic"));
let agent = AgentBuilder::new(model).build();

let result = agent.run("hello").await?;
assert_eq!(result.output, "deterministic");
# Ok(())
# }
```

## Function model

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, FunctionModel};
use starweaver_model::{latest_user_text, ModelResponse};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let model = FunctionModel::new(|messages, _settings, _info| {
    let prompt = latest_user_text(&messages).unwrap_or_default();
    Ok(ModelResponse::text(format!("echo: {prompt}")))
});
let agent = AgentBuilder::new(Arc::new(model)).build();

let result = agent.run("hello").await?;
assert_eq!(result.output, "echo: hello");
# Ok(())
# }
```

## Streaming test model

`TestModel` and `FunctionModel` support both `request` and `request_stream`, so runtime streaming tests can exercise provider delta events without a production model.

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, AgentStreamEvent, TestModel};
use starweaver_model::{ModelResponse, ModelResponseStreamEvent, PartDelta, PartEnd, PartStart};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let model = TestModel::with_stream_events(vec![vec![
    ModelResponseStreamEvent::PartStart(PartStart {
        index: 0,
        part_kind: "text".to_string(),
    }),
    ModelResponseStreamEvent::PartDelta(PartDelta {
        index: 0,
        delta: "hel".to_string(),
    }),
    ModelResponseStreamEvent::PartEnd(PartEnd { index: 0 }),
    ModelResponseStreamEvent::FinalResult(Box::new(ModelResponse::text("hello"))),
]]);
let agent = AgentBuilder::new(Arc::new(model)).build();

let stream = agent.run_stream("hello").await?;
assert!(stream.events().iter().any(|record| matches!(
    record.event,
    AgentStreamEvent::ModelStream { .. }
)));
# Ok(())
# }
```

## Production request guard

Use the global guard in tests to prevent production HTTP requests:

```rust
use starweaver_model::block_real_model_requests;

let _guard = block_real_model_requests();
assert!(!starweaver_model::allow_real_model_requests());
```

`ProtocolModelClient` checks this guard before calling injected transport, and `ReqwestHttpClient` checks it at the HTTP boundary.
