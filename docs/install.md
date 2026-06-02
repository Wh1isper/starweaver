# Install

Starweaver can be used as a Rust workspace and as an installed CLI product.

## CLI install

Install the latest GitHub Release into `~/.local/bin`:

```bash
curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh | sh
```

The installer downloads the platform archive, verifies `checksums.txt` when the release provides it, installs `starweaver`, `starweaver-cli`, and `sw` for CLI installs, installs `starweaver-claw` for Claw installs, and adds the install directory to the shell profile when needed.

Pinned install:

```bash
STARWEAVER_VERSION=v0.1.0 curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh | sh
```

Custom install directory:

```bash
STARWEAVER_INSTALL_DIR="$HOME/bin" curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh | sh
```

Installer environment variables:

| Variable                    | Purpose                                                 |
| --------------------------- | ------------------------------------------------------- |
| `STARWEAVER_VERSION`        | install a specific release tag, such as `v0.1.0`        |
| `STARWEAVER_INSTALL_DIR`    | choose an install directory                             |
| `STARWEAVER_COMPONENTS`     | comma-separated components, such as `cli` or `cli,claw` |
| `STARWEAVER_NO_MODIFY_PATH` | set to `1` to skip shell profile updates                |
| `STARWEAVER_GITHUB_REPO`    | override the release repository for forks               |

Launcher usage:

```bash
starweaver version
starweaver doctor
starweaver cli -p "hello" --output text
sw cli -p "hello" --output text
```

Update the CLI binaries through the launcher:

```bash
starweaver update
starweaver update cli
starweaver cli update
```

Claw binaries use a separate update target:

```bash
starweaver update claw
starweaver claw update
```

The update command invokes the same installer with `STARWEAVER_COMPONENTS=cli` or `STARWEAVER_COMPONENTS=claw`, so CLI updates replace CLI launcher binaries and Claw updates replace the Claw command binary. Claw uses an explicit update command; it is not updated automatically during CLI updates.

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
