# Install

Install the latest Starweaver launcher and CLI binaries from GitHub Releases:

```bash
curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh | sh
```

The installer downloads the platform archive, verifies `checksums.txt` when the release provides it, installs `starweaver`, `starweaver-cli`, and `sw`, and adds the install directory to the shell profile when needed.

Pinned install:

```bash
STARWEAVER_VERSION=v0.1.0 \
  curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh | sh
```

Custom install directory:

```bash
STARWEAVER_INSTALL_DIR="$HOME/bin" \
  curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh | sh
```

Installer environment variables:

| Variable                    | Purpose                                                 |
| --------------------------- | ------------------------------------------------------- |
| `STARWEAVER_VERSION`        | install a specific release tag, such as `v0.1.0`        |
| `STARWEAVER_INSTALL_DIR`    | choose an install directory                             |
| `STARWEAVER_COMPONENTS`     | component list; current release artifacts provide `cli` |
| `STARWEAVER_NO_MODIFY_PATH` | set to `1` to skip shell profile updates                |
| `STARWEAVER_GITHUB_REPO`    | override the release repository for forks               |

## Update

Update the installed CLI binaries through the launcher:

```bash
starweaver update
starweaver update cli
starweaver cli update
```

The update command invokes the installer with `STARWEAVER_COMPONENTS=cli`, replaces CLI launcher binaries, and preserves existing configuration and session data.

## Workspace development

From a checkout, validate the workspace:

```bash
make fmt-check
make check
make test
```

Run the CLI from source:

```bash
make cli -- -p "hello" --output text
make sw -- version
```
