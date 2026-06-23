# Agent Foundation Feature Map

This map tracks Starweaver foundation coverage against agent runtime concepts. Product surfaces consume these foundations through `starweaver-agent`, `starweaver-session`, `starweaver-stream`, `starweaver-storage`, and `starweaver-cli`.

## Scope

- agent construction and run lifecycle
- model request/response abstractions
- provider settings, profiles, transports, and replay fixtures
- tool schema, toolsets, metadata, retries, approvals, and deferred records
- structured output validation and output functions
- context state, typed dependencies, usage, events, messages, and resumable state
- streaming records, display messages, replay logs, compaction, and UI adapters
- durable session records, stream archives, SQLite adapters, and migration status
- deterministic testing, request guards, replay fixtures, and docs examples

## Coverage Matrix

| Area                     | Crates                                                                                | Status            | Planning source                            |
| ------------------------ | ------------------------------------------------------------------------------------- | ----------------- | ------------------------------------------ |
| Agent builder            | `starweaver-agent`, `starweaver-runtime`                                              | landed            | core/spec docs                             |
| Agent app/session        | `starweaver-agent`, `starweaver-context`                                              | landed            | SDK/spec docs                              |
| Model protocol           | `starweaver-model`                                                                    | landed            | core/spec docs                             |
| Model wrappers           | `starweaver-model`                                                                    | landed            | core/spec docs                             |
| Request preparation      | `starweaver-model`, `starweaver-runtime`                                              | landed            | core/spec docs                             |
| Streaming parts          | `starweaver-model`, `starweaver-runtime`, `starweaver-stream`                         | landed            | core/ops specs                             |
| Tool schema              | `starweaver-tools`, `starweaver-runtime`                                              | landed            | core/spec docs                             |
| Toolsets and combinators | `starweaver-tools`                                                                    | landed            | core/spec docs                             |
| Deferred tools           | `starweaver-tools`, `starweaver-runtime`, `starweaver-session`                        | landed            | core/ops specs                             |
| Prepare-tools hooks      | `starweaver-runtime`                                                                  | landed            | core/spec docs                             |
| Structured output        | `starweaver-runtime`, `starweaver-agent`                                              | landed            | core/spec docs                             |
| Output functions         | `starweaver-runtime`                                                                  | landed            | core/spec docs                             |
| Capability middleware    | `starweaver-runtime`, `starweaver-agent`                                              | landed            | core/spec docs                             |
| Context state            | `starweaver-context`, `starweaver-session`                                            | landed            | core/ops specs                             |
| Durable execution        | `starweaver-runtime`, `starweaver-session`, `starweaver-stream`, `starweaver-storage` | landed foundation | core/ops specs                             |
| Observability seams      | `starweaver-runtime`, `starweaver-core`                                               | partial           | `../ops/05-observability.md`               |
| UI adapters              | `starweaver-stream`                                                                   | landed            | `../ops/02-shared-execution-components.md` |
| Testing                  | all foundation crates                                                                 | landed            | `../ops/01-ci-readiness.md`                |

## Foundation Acceptance Gates

Foundation maturity is accepted when these commands pass:

```bash
cargo fmt --check
cargo test -p starweaver-core -p starweaver-model -p starweaver-context -p starweaver-runtime -p starweaver-tools -p starweaver-agent -p starweaver-environment -p starweaver-session -p starweaver-stream -p starweaver-storage --locked
make replay-check
make docs-check
```

## Planning Source

This file records feature coverage only. Follow-up work should live in the spec
that owns the changed contract.
