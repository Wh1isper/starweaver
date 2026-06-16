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

| Area                     | Crates                                                                                | Status            | Next maturity step                                                                          |
| ------------------------ | ------------------------------------------------------------------------------------- | ----------------- | ------------------------------------------------------------------------------------------- |
| Agent builder            | `starweaver-agent`, `starweaver-runtime`                                              | landed            | keep builder ergonomics aligned with docs examples                                          |
| Agent app/session        | `starweaver-agent`, `starweaver-context`                                              | landed            | expand session helpers around durable stores                                                |
| Model protocol           | `starweaver-model`                                                                    | landed            | deepen provider normalization fixtures                                                      |
| Model wrappers           | `starweaver-model`                                                                    | landed            | add wrappers for fallback, instrumentation, and request transforms as concrete needs emerge |
| Request preparation      | `starweaver-model`, `starweaver-runtime`                                              | landed            | preserve snapshots through replay and audit fixtures                                        |
| Streaming parts          | `starweaver-model`, `starweaver-runtime`, `starweaver-stream`                         | landed            | broaden delta fixtures across providers                                                     |
| Tool schema              | `starweaver-tools`, `starweaver-runtime`                                              | landed            | keep JSON schema snapshots stable                                                           |
| Toolsets and combinators | `starweaver-tools`                                                                    | landed            | add deterministic tests for nested composition and prefixing                                |
| Deferred tools           | `starweaver-tools`, `starweaver-runtime`, `starweaver-session`                        | landed            | add application-facing handlers and session-store replay examples                           |
| Prepare-tools hooks      | `starweaver-runtime`                                                                  | landed            | add dependency-aware hook examples                                                          |
| Structured output        | `starweaver-runtime`, `starweaver-agent`                                              | landed            | expand typed parsing and retry diagnostics                                                  |
| Output functions         | `starweaver-runtime`                                                                  | landed            | add multi-output selection examples                                                         |
| Capability middleware    | `starweaver-runtime`, `starweaver-agent`                                              | landed            | add reusable bundles for SDK applications                                                   |
| Context state            | `starweaver-context`, `starweaver-session`                                            | landed            | add persistence examples through `SqliteSessionStore`                                       |
| Durable execution        | `starweaver-runtime`, `starweaver-session`, `starweaver-stream`, `starweaver-storage` | landed foundation | extend service-host examples after platform ownership is defined                            |
| Observability seams      | `starweaver-runtime`, `starweaver-core`                                               | partial           | add OpenTelemetry exporter adapters and redaction policy tests                              |
| UI adapters              | `starweaver-stream`                                                                   | landed            | deepen sanitizer and protocol adapter tests                                                 |
| Testing                  | all foundation crates                                                                 | landed            | keep coverage gates and replay checks current                                               |

## Foundation Acceptance Gates

Foundation maturity is accepted when these commands pass:

```bash
cargo fmt --check
cargo test -p starweaver-core -p starweaver-model -p starweaver-context -p starweaver-runtime -p starweaver-tools -p starweaver-agent -p starweaver-environment -p starweaver-session -p starweaver-stream -p starweaver-storage --locked
make replay-check
make docs-check
```

## Follow-up Buckets

| Bucket                  | Status    | Direction                                                                                                             |
| ----------------------- | --------- | --------------------------------------------------------------------------------------------------------------------- |
| provider replay breadth | active    | add fixtures for request parameters, stream parts, and provider-specific normalization                                |
| toolset composition     | active    | keep combinators small and deterministic                                                                              |
| durable storage         | active    | keep SQLite schema focused on shared session, run, checkpoint, approval, deferred, replay, and stream archive records |
| UI protocol adapters    | active    | map `DisplayMessage` into external wire formats through explicit adapters                                             |
| CLI migration audit     | postponed | resume after foundation gates stay stable                                                                             |
| evals                   | postponed | add dataset, evaluator, and reporting layers after SDK and CLI behavior stabilize                                     |
