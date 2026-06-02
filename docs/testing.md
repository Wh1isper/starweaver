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

## Coverage

CI runs grouped line coverage gates with `cargo-llvm-cov`.

```bash
cargo install cargo-llvm-cov
make coverage-core
make coverage-agent
make coverage-service
make coverage-ci
make coverage
```

Default acceptance gates are 95% for core contract paths, 90% for agent SDK session/subagent paths, and 80% for CLI/service paths. Core and agent gates also enforce measured coverage floors over their full package groups. The dedicated coverage workflow installs `cargo-llvm-cov`, runs the grouped gates, generates `target/llvm-cov/lcov.info`, and uploads the LCOV artifact.

## Automation validation

Repository automation runs through the Rust `xtask` crate and Makefile targets.

```bash
make scripts-check
```

## Model cassette workflow

Provider replay fixtures can be recorded, scrubbed, and imported through repository scripts. The workflow records a live provider response, scrubs secrets and unstable ids, then imports a deterministic replay fixture.

```bash
make record-model-cassette ARGS="request.json --provider openai_chat --output cassette.json"
make scrub-model-cassette ARGS="cassette.json --output cassette.scrubbed.json"
make import-model-cassette ARGS="cassette.scrubbed.json"
make replay-check
```

`request.json` should include the canonical request fields used by replay fixtures: `model`, `history`, and `expected_provider_request`. Add `expected_response` or `expected_error` before importing a replay fixture. Use `--mock-response response.json` for deterministic script tests and `--dry-run` to inspect the resolved provider request.
