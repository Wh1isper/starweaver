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
- JSON-RPC host protocol
- platform integration
- release acceptance

## Operations Shape

```mermaid
flowchart TD
    ci[CI validation]
    replay[Replay fixtures]
    specs[Specs and implementation plan]
    sdk[SDK]
    session[Shared session contracts]
    stream[Shared stream contracts]
    storage[Shared SQLite storage]
    cli[CLI product]
    hostrpc[JSON-RPC host protocol]
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
    cli --> hostrpc
    session --> hostrpc
    stream --> hostrpc
    hostrpc --> service
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
- `04-cli-product.md` — CLI-first product surface with CLI commands as a shell-friendly subset, TUI as the terminal client, standalone JSON-RPC host process for Desktop/local clients, headless stdio display streams, session restore from display messages, DisplayMessage rendering with AGUI display adapters, launcher dispatch, and GitHub install/update flow
- `05-observability.md` — OpenTelemetry GenAI tracing, Langfuse-friendly OTLP export, nested agent/model/tool spans, and trace-to-session correlation
- `06-json-rpc-host-protocol.md` — Starweaver-owned JSON-RPC host-control protocol, stdio/HTTP/socket/WebSocket transport profiles, typed method/event/error contracts, stream replay/subscription semantics, projections, idempotency, and acceptance gates

## Readiness Model

A feature moves from planned to accepted when it has:

- spec coverage
- implementation
- targeted tests
- docs examples where user-facing
- CI coverage
- implementation plan update
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
