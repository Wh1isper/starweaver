# Repository Guidelines

## Repository Overview

`starweaver-agent-sdk` is a Rust workspace for agent SDK primitives, command-line tooling, runtime services, and agent platform foundations.

Workspace members:

- `crates/starweaver-core` — core SDK primitives and shared abstractions
- `crates/starweaver-cli` — `starweaver` command-line entry point
- `crates/starweaver-claw` — runtime service foundations
- `crates/starweaver-agent-platform` — agent platform foundations

## Development Workflow

After changing code, run:

1. `make fmt-check`
2. `make check`
3. `make test`

For full local validation, run:

```bash
make ci
```

## Coding Conventions

- Use English for code, documentation, commit messages, and file names.
- Keep workspace metadata aligned across `Cargo.toml`, crate manifests, `Makefile`, `.pre-commit-config.yaml`, and `.github/workflows/ci.yml`.
- Prefer small crates with explicit boundaries and shared primitives in `starweaver-core`.
- Keep early abstractions minimal and add SDK concepts as concrete needs emerge.
