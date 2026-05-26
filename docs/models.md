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

## Production request guard

Use the global guard in tests to prevent production HTTP requests:

```rust
use starweaver_model::block_real_model_requests;

let _guard = block_real_model_requests();
assert!(!starweaver_model::allow_real_model_requests());
```

`ProtocolModelClient` checks this guard before calling injected transport, and `ReqwestHttpClient` checks it at the HTTP boundary.
