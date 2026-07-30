# Install

Install Starweaver from GitHub Release artifacts, use crates.io packages for SDK code, or run from a
checkout while developing inside this repository. For native application packages, see
[Install and Update Starweaver Desktop](desktop-install.md), including verification and unsigned-app
warnings.

## From source

```bash
git clone https://github.com/Wh1isper/starweaver.git
cd starweaver
make check
make cli -- -p "hello" --output text
```

Useful source commands:

```bash
make sw -- version
make cli
make cli -- -p "hello" --output text
```

## From GitHub Releases

```bash
curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh | sh
```

Pinned install:

```bash
curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh \
  | STARWEAVER_VERSION=vX.Y.Z sh
```

Custom install directory:

```bash
curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh \
  | STARWEAVER_INSTALL_DIR="$HOME/bin" sh
```

Installer environment variables:

| Variable                        | Purpose                                                                              |
| ------------------------------- | ------------------------------------------------------------------------------------ |
| `STARWEAVER_VERSION`            | Install a specific release tag, such as `vX.Y.Z`.                                    |
| `STARWEAVER_INSTALL_DIR`        | Choose an install directory.                                                         |
| `STARWEAVER_EXCLUDE_COMPONENTS` | Comma-separated components to omit, such as macOS-only `computer-use`.               |
| `STARWEAVER_COMPONENTS`         | Compatibility-only explicit component selection. New usage should prefer exclusions. |
| `STARWEAVER_NO_MODIFY_PATH`     | Set to `1` to skip shell profile updates.                                            |
| `STARWEAVER_GITHUB_REPO`        | Override the release repository for forks.                                           |

By default the installer installs every component available for the detected platform. The CLI
component installs `starweaver`, `starweaver-cli`, `sw`, and `starweaver-rpc`. On macOS, the default
also installs the external-harness `starweaver-computer-use-mcp` binary. Linux and Windows currently
have no Computer Use release component and therefore install only the CLI component.

To omit the macOS MCP binary:

```bash
curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh \
  | STARWEAVER_EXCLUDE_COMPONENTS=computer-use sh
```

Installing that binary does not enable the default-denied Computer Use Toolset in CLI or RPC. See
[Computer Use](computer-use.md) for Screen Recording and Accessibility/post-event permissions,
CLI/RPC in-process configuration, the full macOS observe/pointer/keyboard surface, and the
unsigned/not-notarized binary warning. Default installation does not enable the CLI/RPC Toolset;
launching the installed standalone MCP server is a separate explicit opt-in.

## Crates

```toml
[dependencies]
starweaver-agent = "X.Y.Z"
# Optional provider-neutral current-desktop service and MCP binary package:
starweaver-computer-use = "X.Y.Z"
```

Use the workspace path while developing inside this repository:

```toml
[dependencies]
starweaver-agent = { path = "crates/starweaver-agent" }
```

## Update

Installed components update through the launcher:

```bash
starweaver update
starweaver update cli
starweaver cli update
starweaver update --dry-run
starweaver update --force
```

The default update installs all components available for the current platform and honors
`STARWEAVER_EXCLUDE_COMPONENTS`; `starweaver update cli` and `starweaver update computer-use` select
one component explicitly. Updates preserve existing configuration and session data under
`~/.starweaver`. Every installed component has an atomic local version manifest. The command returns
`status=up-to-date` only when every selected binary exists and its component manifest satisfies the
requested release, so a missing or stale Computer Use binary cannot be masked by the running CLI
version. Use `--force` or
`STARWEAVER_UPDATE_FORCE=1` to reinstall the selected release.
