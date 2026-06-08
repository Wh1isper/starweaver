# Reference Parity and Migration Notes

This spec records the current Starweaver reference-parity boundary against `Wh1isper/ya-mono`. The active Starweaver roadmap keeps the shared agent SDK foundations primary and postpones CLI parity audit until foundation gates stay stable.

## Current Scope

Active foundation work:

- SDK filters and media helper foundations
- provider-neutral model protocol, model wrappers, request snapshots, and replay fixtures
- tool schema, toolsets, combinators, metadata, approval/deferred records, and MCP foundations
- runtime capability hooks, output modes, retry semantics, stream records, trace seams, and executor checkpoints
- `AgentApp`, `AgentSession`, `AgentSpec`, first-party tool bundles, and subagent registry
- environment provider contracts and local/virtual provider tests
- durable session contracts in `starweaver-session`
- display/replay contracts, UI adapters, sanitizers, compaction, and stream archives in `starweaver-stream`
- SQLite foundation adapters and migration status in `starweaver-storage`

Postponed work:

- CLI parity audit and `.yaacli` import/export behavior
- additional product surfaces beyond CLI
- distributed service transports and remote execution adapters

## Foundation Acceptance Gates

```bash
cargo fmt --check
cargo test -p starweaver-core -p starweaver-model -p starweaver-context -p starweaver-runtime -p starweaver-tools -p starweaver-agent -p starweaver-environment -p starweaver-session -p starweaver-stream -p starweaver-storage --locked
make replay-check
make docs-check
```

## CLI Audit Parking Lot

CLI audit work remains tracked as a postponed product phase. It should resume after the foundation gates above stay green and storage/stream/display contracts are stable.

Expected CLI audit areas:

- live stdout streaming for headless output
- AGUI-compatible top-level event adapter coverage
- slash command parity
- TUI model/session/cost/task/HITL/media workflows
- startup asset seeding and config import
- shell environment isolation and review flows
- media and browser configuration
- worktree flag semantics
- session-folder import/export

## Storage Boundary

`starweaver-storage` currently owns product-neutral SQLite tables for:

- sessions
- runs
- checkpoints
- raw stream records
- approvals
- deferred tools
- replay events
- replay snapshots

Product-specific schemas and importers should live in the future product that owns those behaviors. Shared storage keeps foundation contracts reusable by SDK apps, CLI, and platform adapters.

## Validation Plan

1. Keep foundation crate tests green.
2. Keep docs examples compiling.
3. Keep replay fixtures stable for provider mappings.
4. Resume CLI audit after foundation stabilization.
5. Add platform/service specs only when ownership, call sites, and validation gates are concrete.
