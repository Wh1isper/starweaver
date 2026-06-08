# Pre-1.0 Reference Notes

These notes summarize the current pre-1.0 reference posture after the foundation-first scope decision.

## References

| Reference   | Current use                                                                                                                       |
| ----------- | --------------------------------------------------------------------------------------------------------------------------------- |
| Pydantic AI | Agent concepts, provider-neutral model abstractions, tool schema, structured output, retries, capabilities, and testing patterns. |
| ya-mono     | Application runtime ideas, context/state patterns, tool implementations, resumable execution concepts, and CLI parity reference.  |

## Current Foundation State

- `starweaver-model` owns provider-neutral messages, settings, profiles, transports, wrappers, request snapshots, deterministic test models, production request guard, and replay tests.
- `starweaver-tools` owns function tool schema, toolsets, metadata, approval/deferred metadata, registries, combinators, and MCP foundations.
- `starweaver-runtime` owns the deterministic agent loop, capability hooks, output validation, retries, stream records, trace seams, and executor checkpoints.
- `starweaver-agent` owns SDK ergonomics, apps, sessions, specs, subagents, filters, media helpers, and tool bundle helpers.
- `starweaver-session` owns durable session/run/checkpoint/approval/deferred records and `SessionStore` contracts.
- `starweaver-stream` owns display/replay protocol records, UI adapters, sanitizers, compaction, replay logs, and stream archives.
- `starweaver-storage` owns product-neutral SQLite migrations and storage adapters.

## Pre-1.0 Acceptance Priorities

1. Keep foundation crate tests green.
2. Keep provider replay fixtures stable.
3. Keep docs examples compiling.
4. Keep storage schema product-neutral.
5. Resume CLI audit after foundation stabilization.
6. Add platform/service adapter specs after ownership and validation gates are concrete.

## Validation Gate

```bash
cargo fmt --check
cargo test -p starweaver-core -p starweaver-model -p starweaver-context -p starweaver-runtime -p starweaver-tools -p starweaver-agent -p starweaver-environment -p starweaver-session -p starweaver-stream -p starweaver-storage --locked
make replay-check
make docs-check
```
