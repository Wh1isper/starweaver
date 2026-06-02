# Starweaver

Starweaver is a Rust agent SDK for building provider-neutral AI agents with a solid core runtime, reusable tool schema, first-party SDK ergonomics, and durable execution foundations.

The project focuses on these workspace layers:

- `starweaver-model`: provider-neutral model messages, settings, profiles, transport, and replay-tested provider mappings.
- `starweaver-tools`: function tools, toolsets, MCP foundations, tool metadata, approval/deferred markers, and execution primitives.
- `starweaver-runtime`: the deterministic agent loop, tool loop, output validation, retries, stream records, capability hooks, trace recording, and executor checkpoints.
- `starweaver-agent`: the public SDK facade with `AgentBuilder`, `AgentApp`, SDK sessions, spec presets, first-party tool bundles, subagents, and application-facing helpers.
- `starweaver-environment`: filesystem, shell, resources, policy, state snapshots, virtual provider tests, and local provider foundations.
- `starweaver-session`: shared durable session contracts for input parts, `SessionStore`, session/run records, resume snapshots, approvals, deferred records, and compact trace projections.
- `starweaver-stream`: shared display and replay stream contracts for display messages, replay event logs, replay transports, realtime compaction buffers, stream archives, and protocol envelopes.
- `starweaver-claw`: durable orchestration host that re-exports shared session/stream contracts and will provide concrete storage, stream, service, and coordinator adapters.
- `starweaver-cli`: CLI-first product surface for headless stdio runs, display-message rendering, session restore, launcher dispatch, and install/update workflows.

Planned layers are specified before public API graduation:

- `starweaver-platform`: hosted orchestration and external protocol adapters.

## Design References

Starweaver builds on ideas proven in two reference projects:

- [Pydantic AI](https://github.com/pydantic/pydantic-ai) for core agent concepts, model abstraction, tool schema, output validation, retries, capabilities, and testing patterns.
- [ya-mono](https://github.com/Wh1isper/ya-mono) for application runtime, context, tool implementations, interruption, resumable execution, and service patterns.

## CLI install

Install the latest launcher and CLI binaries from GitHub Releases:

```bash
curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh | sh
```

Run the CLI through the product launcher:

```bash
starweaver cli -p "hello" --output text
sw cli -p "hello" --output text
starweaver update cli
starweaver update claw
```

CLI configuration examples live in `examples/cli/`. `starweaver update claw` updates Claw through an explicit target.

## Documentation

Published docs: <https://starweaver.wh1isper.top>

Repository docs live in `docs/` and are built with mdBook. Architecture and product decisions live in `spec/`. The detailed implementation roadmap lives in `memos/implementation-todo.md`.

Useful starting points:

- [docs/index.md](docs/index.md)
- [docs/agent.md](docs/agent.md)
- [docs/models.md](docs/models.md)
- [docs/tools.md](docs/tools.md)
- [docs/output.md](docs/output.md)
- [docs/testing.md](docs/testing.md)
- [docs/session-stream.md](docs/session-stream.md)
- [docs/release.md](docs/release.md)
- [spec/README.md](spec/README.md)
- [spec/ops/02-shared-execution-components.md](spec/ops/02-shared-execution-components.md)
- [memos/implementation-todo.md](memos/implementation-todo.md)

## Quick Example

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, TestModel};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let agent = AgentBuilder::new(Arc::new(TestModel::with_text("Paris")))
    .instruction("Answer concisely.")
    .build();

let result = agent.run("What is the capital of France?").await?;
assert_eq!(result.output, "Paris");
# Ok(())
# }
```

## Validation

```bash
make ci
```

Focused commands:

```bash
make replay-check
make coverage-core
make coverage-agent
make coverage-service
make coverage-ci
make coverage
make cli-examples-check
make install-script-check
make scripts-check
make docs-check
make docs-build
make upversion VERSION=0.2.0
```

## Repository Automation

Repository automation is implemented in the Rust `xtask` crate and wrapped by Makefile targets.

```bash
make cli-examples-check
make install-script-check
make scripts-check
make replay-summary
make record-model-cassette ARGS="request.json --provider openai_chat --output cassette.json"
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development workflow, documentation rules, and validation commands.

## License

BSD-3-Clause
