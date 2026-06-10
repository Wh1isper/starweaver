# Implementation TODO

Starweaver is ready for the next release-preparation pass. The foundation, SDK host tools, preset registry, filter parity, CLI display/session work, storage, stream, and replay surfaces have current test coverage.

## Ready-to-go Gate

Run this gate before release prep or a broad merge:

```bash
make fmt-check
make check
make test
make replay-check
make scripts-check
make docs-check
```

Recent focused validation also passed with:

```bash
cargo fmt --check
cargo check
cargo test --quiet
```

## Keep Stable

- Provider-neutral model protocol, settings, profiles, transports, wrappers, and replay fixtures.
- Runtime loop semantics, capability hooks, structured output, retries, stream records, and checkpoints.
- First-party SDK tool bundles, host-tool result layering, filesystem/shell provider behavior, media helpers, and filters.
- Session, stream, storage, and CLI display/session restoration contracts.
- Docs examples and scripts.

## Parking Lot

These are not blockers for the current ready-to-go state:

- Continue adding provider-native replay fixtures as public provider APIs evolve.
- Deepen live MCP host-adapter integration after concrete transport needs land.
- Resume CLI parity audit for slash commands, richer TUI workflows, browser/media configuration, and worktree flag semantics when product scope requires it.
- Graduate platform/service adapters from specs only after ownership, call sites, storage scope, and validation commands are concrete.
