# Refactor Readiness

This spec records planned refactors discovered during the June 2026 pydantic-ai, ya-mono, and repository implementation audit. Current active work focuses on foundation crates and product-neutral contracts.

## Current Hotspots

| Area              | Current pressure                                                               | Target                                                                      |
| ----------------- | ------------------------------------------------------------------------------ | --------------------------------------------------------------------------- |
| Runtime loop      | agent loop helpers and retry/output logic are growing                          | keep state machine, tool loop, output loop, and checkpoint logic modular    |
| Model providers   | provider request mapping and streaming deltas require broad fixtures           | split protocol helpers from provider-specific mapping when complexity grows |
| SDK filters/media | media and filter helpers need stable host seams                                | keep host handles in registries and product-neutral helper modules          |
| Storage           | SQLite adapter owns session, replay, stream archive, and migration logic       | keep schema foundation-only and split helpers by adapter responsibility     |
| CLI               | CLI product contains config, runner, profiles, TUI, launcher, and storage glue | resume product refactors after foundation gates stay green                  |
| Docs/specs        | roadmap references can drift during foundation work                            | keep docs user-facing and specs decision-focused                            |

## Code Size Budget

Prefer these limits for new or refactored files:

| File type               | Soft limit                      |
| ----------------------- | ------------------------------- |
| Runtime modules         | 700 lines                       |
| Provider modules        | 700 lines                       |
| CLI command modules     | 600 lines                       |
| Storage adapter modules | 600 lines                       |
| Tests                   | 900 lines per focused test file |
| Docs pages              | one focused topic per page      |

## Phase 1: Shared Storage Convergence

Goal: keep `starweaver-storage` foundation-only and easy to test.

Actions:

1. Keep migrations limited to shared session, run, checkpoint, stream, approval, deferred, replay, and snapshot tables.
2. Keep migration status DTOs separate from product import reports.
3. Keep connection, migration, session store, replay log, and stream archive modules independently testable.
4. Add focused tests for migration status, idempotency, session store, replay log, and stream archive behavior.
5. Keep product-specific imports outside the shared storage crate.

Validation:

```bash
cargo test -p starweaver-storage --locked
```

## Phase 2: Runtime and Model Decomposition

Goal: preserve runtime readability as capability hooks, output modes, model wrappers, and stream deltas mature.

Actions:

1. Keep request preparation and provider mapping in `starweaver-model`.
2. Keep model wrappers in dedicated modules with order tests.
3. Keep runtime tool loop, output loop, capability hooks, and checkpoint handling separated.
4. Add replay fixtures for request snapshots and stream delta normalization.
5. Preserve deterministic testing through `TestModel` and `FunctionModel`.

Validation:

```bash
cargo test -p starweaver-model -p starweaver-runtime --locked
make replay-check
```

## Phase 3: SDK Filter, Media, and Tool Bundle Boundaries

Goal: keep SDK helper modules composable and host-neutral.

Actions:

1. Keep filters in dedicated modules with deterministic order tests.
2. Keep media helpers separate from host credentials and transport clients.
3. Keep first-party tool bundle constructors small and registry-driven.
4. Keep MCP live clients as host adapters.
5. Keep AgentSpec host-materialized fields resolved through registries.

Validation:

```bash
cargo test -p starweaver-agent -p starweaver-tools --locked
```

## Phase 4: CLI Product Cleanup

Goal: resume CLI refactors after foundation gates are stable.

Actions:

1. Split command parsing and command execution by domain.
2. Keep launcher/update logic small and install-script tested.
3. Move config import/export into focused modules.
4. Keep TUI state, rendering, and terminal handling separate.
5. Converge local persistence onto shared storage adapters where behavior is stable.

Validation:

```bash
cargo test -p starweaver-cli --locked
make scripts-check
```

## Acceptance Gates

```bash
cargo fmt --check
cargo test -p starweaver-core -p starweaver-model -p starweaver-context -p starweaver-runtime -p starweaver-tools -p starweaver-agent -p starweaver-environment -p starweaver-session -p starweaver-stream -p starweaver-storage --locked
cargo test -p starweaver-cli --locked
make replay-check
make docs-check
make scripts-check
```
