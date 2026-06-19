# Install

Starweaver is preparing its first public release, `0.0.1`. Before that release exists, run the
CLI and SDK examples from a checkout. After the release is published, use GitHub Release artifacts
or crates.io packages.

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

After `v0.0.1` is published:

```bash
curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh | sh
```

Pinned install:

```bash
STARWEAVER_VERSION=v0.0.1 \
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
| `STARWEAVER_VERSION`        | Install a specific release tag, such as `v0.0.1`.        |
| `STARWEAVER_INSTALL_DIR`    | Choose an install directory.                             |
| `STARWEAVER_COMPONENTS`     | Component list; current release artifacts provide `cli`. |
| `STARWEAVER_NO_MODIFY_PATH` | Set to `1` to skip shell profile updates.                |
| `STARWEAVER_GITHUB_REPO`    | Override the release repository for forks.               |

## Crates

After `0.0.1` is published:

```toml
[dependencies]
starweaver-agent = "0.0.1"
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
```

The update command invokes the installer with `STARWEAVER_COMPONENTS=cli`, replaces CLI launcher
binaries, and preserves existing configuration and session data under `~/.starweaver`.
