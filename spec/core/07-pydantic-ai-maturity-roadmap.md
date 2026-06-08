# Pydantic AI Maturity Roadmap

This roadmap tracks foundation maturity work inspired by Pydantic AI. The current focus is core/model/context/runtime/tools/agent/environment/session/stream/storage. CLI parity work is postponed until foundation gates stay stable.

## Landed Foundation Slices

- capability middleware and reusable capability bundles
- approval and deferred records through tools, runtime state, session records, and CLI commands
- toolset combinators and deterministic prefixing/filtering behavior
- `AgentSpec` v2 profile shape for serialized application configuration
- structured output modes, typed parsing, output functions, and retry diagnostics
- model wrappers and request preparation snapshots
- display/replay UI adapters and sanitizer foundations
- SQLite session, run, checkpoint, approval, deferred, replay, and stream archive storage

## Active Maturity Tracks

### Capability Middleware

Goal: keep capability hooks reusable across runtime and SDK applications.

Acceptance:

- deterministic hook order tests
- dependency-aware examples
- capability bundle docs
- runtime stream evidence for capability actions

### Deferred Tools

Goal: keep deferred and approval control flow serializable, replayable, and host-neutral.

Acceptance:

- session records for pending and resolved interactions
- runtime retry and resume coverage
- CLI command coverage for local review flows
- docs examples showing application-managed decisions

### Toolset Combinators

Goal: support predictable toolset composition without product-specific code.

Acceptance:

- prefix, include, exclude, and metadata merge tests
- schema snapshot tests
- clear error reporting for duplicate names
- docs examples for composing first-party bundles

### AgentSpec v2

Goal: provide a serializable application profile surface while keeping credentials and live handles in host registries.

Acceptance:

- YAML round-trip tests
- model, toolset, host adapter, MCP, retry, output, environment, streaming, observability, and durability sections
- host-materialized policy tests in CLI and SDK registries
- docs examples for file-backed profiles

### Output Modes

Goal: align structured output, text output, and tool-return output with a small set of predictable runtime paths.

Acceptance:

- typed structured output parsing tests
- semantic retry tests
- output function tests
- clear display/replay evidence for output retries and terminal output

### Model Wrappers

Goal: compose model behavior through explicit wrappers instead of product-specific branches.

Acceptance:

- wrapper order tests
- request/response transform snapshots
- instrumentation and fallback examples
- replay fixture compatibility

### UI Adapter Maturity

Goal: keep `DisplayMessage` as the Starweaver-native protocol while mapping into external protocols through explicit adapters.

Acceptance:

- JSONL adapter tests
- AGUI adapter tests
- sanitizer tests for trusted and external views
- replay compaction tests

## Validation Gates

```bash
cargo fmt --check
cargo test -p starweaver-core -p starweaver-model -p starweaver-context -p starweaver-runtime -p starweaver-tools -p starweaver-agent -p starweaver-environment -p starweaver-session -p starweaver-stream -p starweaver-storage --locked
make replay-check
make docs-check
```

## Postponed Work

CLI parity and evaluation framework work resume after foundation storage, stream, model wrapper, toolset, capability, and SDK profile gates stay stable.
