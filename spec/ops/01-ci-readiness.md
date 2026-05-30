# CI Readiness and Coverage Gates

CI must prove that Starweaver's core provider compatibility, SDK examples, and test coverage stay healthy. Replay fixtures are a first-class gate because provider correctness underpins the runtime, SDK, MCP, durable service, and CLI layers.

## Required CI Steps

```mermaid
flowchart TD
    fmt[format check]
    check[cargo check and clippy]
    scripts[xtask automation validation]
    replay[model replay check]
    tests[workspace tests]
    coverage[coverage gate]
    docs[docs examples and site]
    precommit[pre-commit]

    fmt --> check
    fmt --> scripts
    fmt --> replay
    fmt --> tests
    fmt --> coverage
    fmt --> docs
    fmt --> precommit
```

CI commands:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test -p starweaver-model --test fixture_schema --test replay --test replay_tooling --test request_parameters --test stream_replay --locked
cargo test --workspace --all-targets --all-features --locked
make coverage-ci
make scripts-check
make docs-check
mdbook build
```

Local aggregate:

```bash
make ci
```

Focused gates:

```bash
make replay-check
make coverage-ci
make scripts-check
make docs-check
```

Replay fixture recording helpers:

```bash
make record-model-cassette ARGS="request.json --provider openai_chat --output cassette.json"
make scrub-model-cassette ARGS="cassette.json --output cassette.scrubbed.json"
make import-model-cassette ARGS="cassette.scrubbed.json"
make scripts-check
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
- coverage gate passes
- script smoke tests pass
- docs examples pass
- feature matrix has status for every Pydantic AI core docs page
- feature matrix has status for every ya-agent-sdk first-party module family
- replay matrix includes supported provider families and known gaps
- specs reflect all public crate boundaries
