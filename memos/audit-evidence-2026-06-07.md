# Audit Evidence 2026-06-07

This memo records the active audit evidence after the foundation-first scope decision.

## Reference Checkouts

| Reference          | Evidence                             | Current use                                                                                                            |
| ------------------ | ------------------------------------ | ---------------------------------------------------------------------------------------------------------------------- |
| `refs/pydantic-ai` | local checkout in `refs/pydantic-ai` | core agent concepts, provider mapping, tools, output validation, retries, streaming, and testing patterns              |
| `refs/ya-mono`     | local checkout in `refs/ya-mono`     | application runtime, context/state ideas, tool implementations, resumable execution concepts, and CLI parity reference |

## Active Findings

### Foundation coverage

Implemented or in active foundation scope:

- model request/response parts, stream deltas, request preparation, provider details, settings, profiles, wrappers, and replay fixtures
- runtime graph state, capability hooks, structured output parsing, output functions, retry budgets, usage limits, trace seams, and executor checkpoints
- tool schema, toolsets, combinators, metadata, approval/deferred control-flow records, registries, and MCP foundations
- SDK builder/app/session/spec/subagent/tool-bundle/filter/media helpers
- environment provider contracts and local/virtual providers
- durable session records and `SessionStore` traits
- display/replay stream records, UI adapters, sanitizers, compaction buffers, replay transports, and stream archives
- product-neutral SQLite storage adapters and migration status

### Validation evidence

Foundation gate previously passed:

```bash
cargo fmt --check && cargo test -p starweaver-core -p starweaver-model -p starweaver-context -p starweaver-runtime -p starweaver-tools -p starweaver-agent -p starweaver-environment -p starweaver-session -p starweaver-stream -p starweaver-storage --locked
```

Re-run required after current repository cleanup:

```bash
cargo fmt --check
cargo test -p starweaver-core -p starweaver-model -p starweaver-context -p starweaver-runtime -p starweaver-tools -p starweaver-agent -p starweaver-environment -p starweaver-session -p starweaver-stream -p starweaver-storage --locked
cargo test -p starweaver-cli --locked
cargo metadata --no-deps --format-version 1
```

## Postponed Audit Areas

CLI parity audit remains postponed. Expected areas when resumed:

- live stdout streaming
- AGUI-compatible top-level event mapping
- slash command parity
- TUI workflow parity
- startup asset seeding
- config import/export
- shell environment isolation and review flows
- media/browser configuration
- worktree flag semantics
- session-folder import/export

## Current Repository Decision

The workspace keeps foundation crates and CLI. Future platform/service adapters stay in specs until ownership, call sites, storage scope, and validation gates are concrete.
