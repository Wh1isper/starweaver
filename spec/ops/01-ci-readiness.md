# CI Readiness and Coverage Gates

CI must prove that Starweaver's core provider compatibility and SDK examples stay healthy. Replay fixtures are a first-class gate because provider correctness underpins the runtime, SDK, MCP, durable service, and CLI layers.

## Required CI Steps

```mermaid
flowchart TD
    fmt[format check]
    check[cargo check]
    clippy[clippy]
    replay[model replay check]
    tests[workspace tests]
    docs[docs examples]

    fmt --> check
    check --> clippy
    clippy --> replay
    replay --> tests
    tests --> docs
```

CI commands:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test -p starweaver-model --test replay --test request_parameters --locked
cargo test --workspace --all-targets --all-features --locked
python3 scripts/check-docs-examples.py
```

Local aggregate:

```bash
make ci
```

Focused replay gate:

```bash
make replay-check
```

## Replay Readiness

Provider replay coverage is accepted when:

- every implemented provider family has text response fixtures
- every implemented provider family with tools has tool call and tool return history fixtures
- native provider tools have request-only fixtures
- settings, profiles, request parameters, and output schema mapping have focused tests
- fixture coverage appears in `memos/implementation-todo.md`
- unmigrated replay categories are explicitly listed

## Feature Coverage Matrix

The TODO memo owns the working matrix for:

- Pydantic AI docs features
- Pydantic AI provider tests
- ya-agent-sdk modules and tests
- Starweaver specs
- current implementation status
- next implementation batch

## Release Gate

Before a release candidate:

- all CI gates pass
- docs examples pass
- feature matrix has status for every Pydantic AI core docs page
- feature matrix has status for every ya-agent-sdk first-party module family
- replay matrix includes supported provider families and known gaps
- specs reflect all public crate boundaries
