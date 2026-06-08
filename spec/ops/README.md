# Operations, Durability, and Products

The operations layer turns core runtime evidence and SDK contracts into validated releases, durable execution foundations, and product surfaces.

## Scope

- CI and readiness gates
- provider replay coverage
- feature coverage matrix
- shared session storage and stream protocol components
- durable execution and service runtime contracts
- OpenTelemetry GenAI observability
- Langfuse-friendly OTLP export
- CLI product
- platform integration
- release acceptance

## Operations Shape

```mermaid
flowchart TD
    ci[CI validation]
    replay[Replay fixtures]
    specs[Specs and TODO matrix]
    sdk[SDK]
    session[Shared session contracts]
    stream[Shared stream contracts]
    storage[Shared SQLite storage]
    cli[CLI product]
    service[Future service adapters]
    observability[Observability]
    platform[Platform]

    specs --> ci
    replay --> ci
    sdk --> session
    sdk --> stream
    session --> storage
    stream --> storage
    storage --> cli
    session --> cli
    stream --> cli
    session --> service
    stream --> service
    storage --> service
    service --> observability
    cli --> observability
    service --> platform
```

## Spec Map

- `01-ci-readiness.md` — replay CI, docs examples, feature coverage matrix, and release acceptance gates
- `02-shared-execution-components.md` — shared session storage and stream protocol contracts
- `03-durable-service-runtime.md` — durable sessions, `SessionStore`, stream archive, resume, interruption, service transports, display-message replay, and storage contracts
- `04-cli-product.md` — CLI-first product surface with JSON-RPC stdio as the complete local runtime and management API, CLI commands as a shell-friendly subset, TUI as the terminal client, Desktop as a future client, headless stdio display streams, session restore from display messages, DisplayMessage rendering with AGUI compatibility adapters, launcher dispatch, and GitHub install/update flow
- `05-observability.md` — OpenTelemetry GenAI tracing, Langfuse-friendly OTLP export, nested agent/model/tool spans, and trace-to-session correlation
- `07-ya-mono-parity-migration.md` — foundation and CLI parity reference map with CLI audit postponed
- `09-refactor-readiness.md` — code size budget, storage convergence, runtime/model/filter decomposition, and contract hardening

## Readiness Model

A feature moves from planned to accepted when it has:

- spec coverage
- implementation
- targeted tests
- docs examples where user-facing
- CI coverage
- TODO matrix update
- clear ownership in crate map
- trace/span semantics when the feature affects runtime, model, tool, subagent, or service execution

## Acceptance Gates

- `make replay-check`
- `make fmt-check`
- `make check`
- `make test`
- `make scripts-check`
- `make docs-check`
- `make coverage-ci`
- `make ci`
