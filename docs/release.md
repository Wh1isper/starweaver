# Release

Starweaver uses one workspace version for crates and CLI artifacts. The repository development
version should stay on a pre-release version such as `0.0.1-dev.0`. A release preparation workflow
promotes that version to the public release version, such as `0.0.1`.

## First release

Prepare the first public release from the repository root:

```bash
gh workflow run prepare-release.yml -f version=0.0.1 -f run_full_ci=true
```

The workflow:

1. validates the requested semver version,
2. runs `make upversion VERSION=0.0.1`,
3. runs full CI and CLI smoke checks when `run_full_ci=true`,
4. opens a `release/v0.0.1` pull request.

For local preparation:

```bash
make upversion VERSION=0.0.1
make ci
make cli-smoke
make publish-dry-run
```

## Draft release workflow

When a `release/vX.Y.Z` pull request is merged into `main`, `draft-release.yml` validates the
merged release commit, runs `make ci-all`, runs `make cli-smoke`, builds CLI launcher binaries,
creates a draft GitHub Release for tag `vX.Y.Z`, and uploads binary archives plus `checksums.txt`.

CLI archives are built for:

- `starweaver-cli-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz`
- `starweaver-cli-vX.Y.Z-x86_64-apple-darwin.tar.gz`
- `starweaver-cli-vX.Y.Z-aarch64-apple-darwin.tar.gz`
- `starweaver-cli-vX.Y.Z-x86_64-pc-windows-msvc.zip`

Unix archives contain:

```text
starweaver
starweaver-cli
sw
```

Windows archives contain:

```text
starweaver.exe
starweaver-cli.exe
sw.exe
```

The draft release also includes `checksums.txt` with SHA-256 checksums for all archives.

## Publish crates

Publishing the draft GitHub Release triggers `.github/workflows/release.yml`:

1. validate the release tag against the workspace version,
2. run `make ci-all`,
3. run `make cli-smoke`,
4. dry-run first-wave publish packages,
5. publish all workspace crates in dependency order through the `Release` environment.

Manual dry-run:

```bash
make publish-dry-run
```

For the first release, dependent crates cannot be fully dry-run against crates.io until their
Starweaver dependencies have been published. The dry-run target validates the release package
lists and dry-runs the dependency-free first-wave crates: `starweaver-core`, `starweaver-usage`,
and `starweaver-oauth`.

Manual publish after validation and approval:

```bash
make publish
```

## Required repository settings

- `CARGO_REGISTRY_TOKEN` secret is configured.
- The `Release` environment exists and requires the intended approval policy.
- The target tag, such as `v0.0.1`, does not already exist.
- GitHub Actions can create the `release/vX.Y.Z` pull request.

## After publishing

Verify the public install path:

```bash
STARWEAVER_VERSION=v0.0.1 \
  curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh | sh

starweaver version
sw cli -p "hello" --output text
```
