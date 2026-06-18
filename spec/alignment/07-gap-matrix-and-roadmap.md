# Gap Matrix and Roadmap

## Summary

This matrix contains only remaining non-aligned work after the current implementation pass.

## Priority Matrix

| Priority | Remaining gap               | Missing work                                                                                                                                                                                 | Primary files                                                                        |
| -------- | --------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| P1       | Declarative subagent parity | True real-time child stream interleaving policy if adopted.                                                                                                                                  | `crates/starweaver-agent/src/subagent/*`, `crates/starweaver-agent/src/presets/*`    |
| P1       | Durable service hardening   | Concrete browser, remote-storage, and media resource adapters beyond the generic typed `ResourceRestoreFactoryRegistry` seam; live OS process reattachment only if a trusted host adopts it. | `crates/starweaver-environment`, `crates/starweaver-agent/src/runtime.rs`            |
| P2       | MCP optional host contracts | Standalone SSE only if a product still requires the older protocol; roots, logging, completions, notifications, and task-worker host policies only if adopted.                               | `crates/starweaver-agent/src/mcp_live.rs`, `crates/starweaver-agent/src/mcp_rmcp.rs` |
| P2       | Media resource adapters     | External durable resource-store records and generated non-image provider fixtures.                                                                                                           | `crates/starweaver-agent/src/filters/media/*`, `crates/starweaver-environment`       |

## P1 Work Package 1: Subagents

Required implementation:

- Add true real-time stream interleaving policy only if adopted.

Acceptance:

- Config-only subagents cover the same executable behavior as programmatic child agents.
- Parent usage, trace records, and subagent execution hooks include child attribution.

## P1 Work Package 2: Durable Runtime

Required implementation:

- Productize concrete browser, remote-storage, and media resource adapters on top of `ResourceRestoreFactoryRegistry`.

Acceptance:

- Service-level tests cover each concrete external resource adapter once those contracts are adopted.
- Runtime facade tests continue to pass with in-memory and SQLite storage adapters without adding a dependency from `starweaver-agent` to `starweaver-storage`.

## P2 Work Package 3: MCP Optional Host Contracts

Required implementation:

- Add standalone SSE only if a product explicitly adopts that older protocol outside `rmcp` 1.7.
- Add roots, logging, completions, notifications, and task-worker policies only when host ownership, UI, replay, and security contracts are stable.

Acceptance:

- Stdio, streamable HTTP, expired-session reinitialization, and protocol-level approval/deferred `rmcp` integration tests stay green.
- Any optional host contract has deterministic protocol fixtures and stream/replay evidence before it is considered aligned.

## P2 Work Package 4: Media Matrix

Required implementation:

- Add external durable media resource records once ownership, retention, and restore policy are stable.
- For future provider adapters, add provider-private continuation fixtures where the provider exposes durable replay payloads before claiming parity.
- For future provider adapters outside the shared HTTP/SSE path, consume the shared cancellation token and preserve retryable stream-resume evidence before claiming parity.

Acceptance:

- Provider request snapshots are stable across existing normal, JSON-restored fixture state, and representative compacted history.
- Unsupported non-image media falls back or errors with provider-specific diagnostics.

## Observability Evidence

- OTel GenAI projection maps Starweaver runtime/model/tool spans to tested semantic fields.
- Provider request audit snapshots are captured through an explicit protocol-client recorder path, separate from redacted span events.
