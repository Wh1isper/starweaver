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
| `STARWEAVER_VERSION`            | Install a full or binary-component tag, such as `vX.Y.Z` or `cli-vX.Y.Z`.            |
| `STARWEAVER_INSTALL_DIR`        | Choose an install directory.                                                         |
| `STARWEAVER_EXCLUDE_COMPONENTS` | Comma-separated components to omit, such as macOS-only `computer-use`.               |
| `STARWEAVER_COMPONENTS`         | Compatibility-only explicit component selection. New usage should prefer exclusions. |
| `STARWEAVER_NO_MODIFY_PATH`     | Set to `1` to skip shell profile updates.                                            |
| `STARWEAVER_GITHUB_REPO`        | Override the release repository for forks.                                           |

By default the installer follows GitHub's latest full stable release and installs every component
available for the detected platform. The CLI component installs `starweaver`, `starweaver-cli`, `sw`,
and `starweaver-rpc`. On macOS, the default also installs the external-harness
`starweaver-computer-use-mcp` binary. Linux and Windows currently have no Computer Use release
component and therefore install only the CLI component. A component tag defaults to that one
component; explicit `STARWEAVER_COMPONENTS` can override the selection.

The bootstrap installer requires `checksums.txt`, requires an entry for every selected archive, and
verifies SHA-256 before extraction. Missing checksum metadata, a missing expected binary, or a digest
mismatch fails installation.

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
STARWEAVER_VERSION=cli-vX.Y.Z starweaver update cli
STARWEAVER_UPDATE_CHANNEL=beta starweaver update
```

The default update selects all components available for the current platform and honors
`STARWEAVER_EXCLUDE_COMPONENTS`; `starweaver update cli` and `starweaver update computer-use` select
one component explicitly. The stable channel compares full and matching component Releases and picks
the highest semantic version, preferring a component-specific tag when versions are equal. The
`beta`/`prerelease` channel also considers prereleases. An explicit `STARWEAVER_VERSION` may pin or
downgrade to a full version, a full tag, or the matching component tag.

Updates are implemented by the native launcher; they never download or execute a mutable installer
script. For each selected component the launcher downloads the exact tag's
`starweaver-release.json`, verifies its tag, scope, channel, source revision, target, archive size, and
SHA-256, then safely extracts an archive with exact binary inventory. All selected components are
downloaded and verified before installation starts. One install-directory lock serializes replacement.
Installation writes an in-progress marker, stages replacements, replaces the running executable before
committing JSON state containing the exact tag, source revision, target, and version, then removes the
marker. A normal error restores old files or removes newly created files; process termination leaves the
marker so the next update retries instead of trusting partial state.

Updates preserve configuration and session data under `~/.starweaver`. The command returns
`status=up-to-date` only when every selected path is a file and its immutable component identity matches
the selected Release. A missing, stale, interrupted, or equal-version-but-different-tag installation is
therefore repaired. Use `--force` or `STARWEAVER_UPDATE_FORCE=1` to reinstall the selected release.
