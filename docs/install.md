# Install

Install Starweaver from GitHub Release artifacts, use crates.io packages for SDK code, or run from a
checkout while developing inside this repository.

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
STARWEAVER_VERSION=vX.Y.Z \
  curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh | sh
```

Custom install directory:

```bash
STARWEAVER_INSTALL_DIR="$HOME/bin" \
  curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh | sh
```

Installer environment variables:

| Variable                    | Purpose                                                  |
| --------------------------- | -------------------------------------------------------- |
| `STARWEAVER_VERSION`        | Install a specific release tag, such as `vX.Y.Z`.        |
| `STARWEAVER_INSTALL_DIR`    | Choose an install directory.                             |
| `STARWEAVER_COMPONENTS`     | Component list; current release artifacts provide `cli`. |
| `STARWEAVER_NO_MODIFY_PATH` | Set to `1` to skip shell profile updates.                |
| `STARWEAVER_GITHUB_REPO`    | Override the release repository for forks.               |

The CLI component installs `starweaver`, `starweaver-cli`, `sw`, and `starweaver-rpc`.

## Crates

```toml
[dependencies]
starweaver-agent = "X.Y.Z"
```

Use the workspace path while developing inside this repository:

```toml
[dependencies]
starweaver-agent = { path = "crates/starweaver-agent" }
```

## Update

Installed CLI binaries update through the launcher:

```bash
starweaver update
starweaver update cli
starweaver cli update
starweaver update --dry-run
starweaver update --force
```

The update command invokes the installer with `STARWEAVER_COMPONENTS=cli`, replaces CLI launcher
binaries, and preserves existing configuration and session data under `~/.starweaver`. It checks the
current CLI package version before installing and returns `status=up-to-date` when the selected
release is already installed. Use `--force` or `STARWEAVER_UPDATE_FORCE=1` to reinstall the selected
release.
