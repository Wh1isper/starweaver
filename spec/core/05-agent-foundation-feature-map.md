# Agent Foundation Feature Map

This non-normative map characterizes design coverage against agent runtime concepts. Product surfaces consume these foundations through `starweaver-agent`, `starweaver-session`, `starweaver-stream`, `starweaver-storage`, and `starweaver-cli`. Current implementation status is generated from `../capabilities.toml` into [`../capability-status.md`](../capability-status.md); when wording here differs, the generated registry view is authoritative.

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

| Area                     | Crates                                                                                | Registry capability                       | Planning source                            |
| ------------------------ | ------------------------------------------------------------------------------------- | ----------------------------------------- | ------------------------------------------ |
| Agent builder            | `starweaver-agent`, `starweaver-runtime`                                              | `runtime.agent_loop`                      | core/spec docs                             |
| Agent app/session        | `starweaver-agent`, `starweaver-context`                                              | —                                         | SDK/spec docs                              |
| Model protocol           | `starweaver-model`                                                                    | `model.canonical_content`                 | core/spec docs                             |
| Model wrappers           | `starweaver-model`                                                                    | `model.wrappers`                          | core/spec docs                             |
| Request preparation      | `starweaver-model`, `starweaver-runtime`                                              | `runtime.agent_loop`                      | core/spec docs                             |
| Streaming parts          | `starweaver-model`, `starweaver-runtime`, `starweaver-stream`                         | `stream.versioned_records`                | core/ops specs                             |
| Tool schema              | `starweaver-tools`, `starweaver-runtime`                                              | —                                         | core/spec docs                             |
| Toolsets and combinators | `starweaver-tools`                                                                    | —                                         | core/spec docs                             |
| Deferred tools           | `starweaver-tools`, `starweaver-runtime`, `starweaver-session`                        | —                                         | core/ops specs                             |
| Prepare-tools hooks      | `starweaver-runtime`                                                                  | `runtime.capability_middleware`           | core/spec docs                             |
| Structured output        | `starweaver-runtime`, `starweaver-agent`                                              | —                                         | core/spec docs                             |
| Output functions         | `starweaver-runtime`                                                                  | —                                         | core/spec docs                             |
| Capability middleware    | `starweaver-runtime`, `starweaver-agent`                                              | `runtime.capability_middleware`           | core/spec docs                             |
| Context state            | `starweaver-context`, `starweaver-session`                                            | `context.versioned_checkpoints`           | core/ops specs                             |
| Durable execution        | `starweaver-runtime`, `starweaver-session`, `starweaver-stream`, `starweaver-storage` | `session.atomic_storage`, `stream.replay` | core/ops specs                             |
| Observability seams      | `starweaver-runtime`, `starweaver-core`                                               | —                                         | `../ops/05-observability.md`               |
| UI projection            | `starweaver-stream`                                                                   | `stream.ui_projection`                    | `../ops/02-shared-execution-components.md` |
| Testing                  | all foundation crates                                                                 | —                                         | `../ops/01-ci-readiness.md`                |

## Foundation Acceptance Gates

Foundation maturity is accepted when these commands pass:

```bash
cargo fmt --check
cargo test -p starweaver-core -p starweaver-model -p starweaver-context -p starweaver-runtime -p starweaver-tools -p starweaver-agent -p starweaver-environment -p starweaver-session -p starweaver-stream -p starweaver-storage --locked
make replay-check
make docs-check
```

## Planning Source

This file records non-normative design coverage only. Follow-up work should live in the spec that owns the changed contract, while current implementation status belongs exclusively to the generated capability status view.
