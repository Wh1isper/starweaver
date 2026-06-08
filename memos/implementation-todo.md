# Implementation TODO

Current priority: finish Starweaver foundation work across core/model/context/runtime/tools/agent/environment/session/stream/storage. CLI parity audit is postponed until foundation gates stay stable.

## P0 Foundation Closeout

Owner: foundation crates.

Scope:

01. Keep provider-neutral model protocol and replay fixtures stable.
02. Keep model wrappers covered by order and request/response transform tests.
03. Keep tool schema, toolset combinators, metadata, retries, approvals, deferred records, and MCP foundations deterministic.
04. Keep runtime capability hooks, structured output modes, output functions, retry semantics, stream records, trace seams, and executor checkpoints covered.
05. Keep SDK `AgentBuilder`, `AgentApp`, `AgentSession`, `AgentSpec`, first-party tool bundles, filters, media helpers, and subagent registry covered.
06. Keep environment provider contracts and local/virtual provider tests green.
07. Keep `starweaver-session` contracts for sessions, runs, checkpoints, approvals, deferred records, resume snapshots, and compact traces stable.
08. Keep `starweaver-stream` display/replay contracts, UI adapters, sanitizers, realtime compaction, replay logs, and stream archives stable.
09. Keep `starweaver-storage` SQLite schema focused on shared session/run/checkpoint/stream/approval/deferred/replay/snapshot tables.
10. Keep docs examples compiling.

Validation:

```bash
cargo fmt --check
cargo test -p starweaver-core -p starweaver-model -p starweaver-context -p starweaver-runtime -p starweaver-tools -p starweaver-agent -p starweaver-environment -p starweaver-session -p starweaver-stream -p starweaver-storage --locked
make replay-check
make docs-check
```

## P0.1 Storage Foundation

Owner: `starweaver-storage`, `starweaver-session`, `starweaver-stream`.

Tasks:

1. Keep `SQLITE_MIGRATIONS` product-neutral.
2. Keep migration status DTOs minimal: applied, pending, latest, current.
3. Keep adapter modules split: connection, errors, migrations, session store, replay log, stream archive.
4. Add idempotency tests for migration application.
5. Add round-trip tests for sessions/runs, replay events, stream archive raw/display/snapshots, and live subscriptions.
6. Keep schema names product-neutral.

Validation:

```bash
cargo test -p starweaver-storage --locked
```

## P0.2 Stream and UI Adapter Foundation

Owner: `starweaver-stream`, `starweaver-runtime`.

Tasks:

1. Keep `DisplayMessage` as the Starweaver-native wire event.
2. Keep JSONL and AGUI-compatible adapters explicit.
3. Keep sanitizer behavior deterministic for trusted and external views.
4. Keep compaction snapshots replayable by scope/cursor.
5. Add tests for stream deltas and part-end records.

Validation:

```bash
cargo test -p starweaver-stream -p starweaver-runtime --locked
```

## P0.3 SDK and Tool Foundation

Owner: `starweaver-agent`, `starweaver-tools`, `starweaver-environment`.

Tasks:

1. Keep `AgentSpec` v2 YAML round-trip and registry resolution tests.
2. Keep first-party tool bundle constructors small and host-neutral.
3. Keep toolset combinator tests for prefix/include/exclude/metadata behavior.
4. Keep live MCP client as a host adapter seam.
5. Keep filters/media helper tests deterministic.

Validation:

```bash
cargo test -p starweaver-agent -p starweaver-tools -p starweaver-environment --locked
```

## P1 CLI Audit Parking Lot

CLI parity audit resumes after the P0 gates stay green.

Expected areas:

- live stdout streaming for headless output
- AGUI-compatible top-level event adapter coverage
- slash command parity
- TUI model/session/cost/task/HITL/media workflows
- startup asset seeding and config import
- shell environment isolation and review flows
- media and browser configuration
- worktree flag semantics
- session-folder import/export

Validation after resuming:

```bash
cargo test -p starweaver-cli --locked
make scripts-check
```

## P2 Platform and Service Adapters

Platform and service adapters should graduate from specs after ownership, call sites, storage scope, and validation commands are concrete.

Candidate areas:

- service transports over `ReplayTransport`
- hosted orchestration adapters
- A2A adapters
- distributed replay event-log adapter
- OpenTelemetry exporter integration

## Current Acceptance Gate

```bash
make fmt-check
make check
make test
make replay-check
make scripts-check
make docs-check
```
