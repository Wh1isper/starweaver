# Install

Starweaver is a workspace crate set. Add the SDK facade from the workspace or consume the lower-level crates directly.

## Workspace development

Install Rust, mdBook, and cargo-llvm-cov for the full local gate.

```bash
git clone https://github.com/Wh1isper/starweaver
cd starweaver
cargo install cargo-llvm-cov
make ci
```

`make ci` runs the core local gate. `make ci-all` also runs the coverage gate.

## Crate layers

| Crate                | Use for                                                         |
| -------------------- | --------------------------------------------------------------- |
| `starweaver-agent`   | application-facing builder and SDK helpers                      |
| `starweaver-runtime` | core agent loop and checkpointable runtime                      |
| `starweaver-model`   | model messages, settings, profiles, and provider clients        |
| `starweaver-tools`   | function tool schema, toolsets, registries, and MCP foundations |
| `starweaver-context` | lifecycle context, state, events, message bus, and dependencies |

## Local validation

```bash
make fmt-check
make check
make test
make scripts-check
make docs-check
```

Coverage uses `cargo-llvm-cov`:

```bash
make coverage-ci
make coverage
```

`make ci` runs formatting, Rust checks, replay checks, tests, script checks, and docs checks/build. `make ci-all` runs `make ci` plus coverage.
