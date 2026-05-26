# Repository Guidelines

## Repository Overview

`starweaver-agent-sdk` is a Rust workspace for building Starweaver, an agent SDK with CLI tooling and planned runtime/platform capabilities.

Current workspace members:

- `crates/starweaver-core` — shared SDK identity and early core primitives
- `crates/starweaver-cli` — `starweaver` command-line entry point

Planned areas live in `spec/` until their responsibilities, integration points, and validation paths are clear:

- Core abstractions
- CLI workflows
- Claw runtime services
- Agent platform capabilities

## Spec Workflow

Use `spec/` for product and architecture decisions before introducing new crates or public APIs.

Current specs:

- `spec/README.md` — spec index and planned area map
- `spec/00-repository.md` — repository scaffold, current workspace shape, and planned areas

After changing repository structure, workspace boundaries, command behavior, CI, or planned module responsibilities, review and update:

- `spec/*`
- `README.md`
- `AGENTS.md`
- `Cargo.toml`
- crate manifests under `crates/*/Cargo.toml`
- `Makefile`
- `.pre-commit-config.yaml`
- `.github/workflows/*.yml`

## Development Workflow

After changing code, run:

1. `make fmt-check`
2. `make check`
3. `make test`

For full local validation, run:

```bash
make ci
```

For repository-wide hooks, run:

```bash
make lint
```

## Coding Conventions

- Use English for code, documentation, commit messages, and file names.
- Keep workspace metadata aligned across `Cargo.toml`, crate manifests, `Makefile`, `.pre-commit-config.yaml`, and `.github/workflows/ci.yml`.
- Keep early abstractions minimal and add SDK concepts as concrete needs emerge.
- Add crates from specs when the boundary has clear responsibilities, call sites, and validation commands.
