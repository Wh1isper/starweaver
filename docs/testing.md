# Testing

Use deterministic models and the production-request guard for safe tests.

## TestModel

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, TestModel};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let test_model = Arc::new(TestModel::with_text("test response"));
let agent = AgentBuilder::new(test_model).build();

let result = agent.run("hello").await?;
assert_eq!(result.output, "test response");
# Ok(())
# }
```

## Scoped override

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, TestModel};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let production_agent = AgentBuilder::new(Arc::new(TestModel::with_text("prod"))).build();
let test_agent = production_agent
    .override_config()
    .model(Arc::new(TestModel::with_text("test")))
    .build();

let result = test_agent.run("hello").await?;
assert_eq!(result.output, "test");
# Ok(())
# }
```

## Production-request guard

```rust
use starweaver_model::block_real_model_requests;

let _guard = block_real_model_requests();
assert!(!starweaver_model::allow_real_model_requests());
```

Run validation with:

```bash
make fmt-check && make check && make test
```
