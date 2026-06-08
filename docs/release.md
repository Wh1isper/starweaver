# Release

Starweaver uses a unified workspace version and publishes Rust crates plus CLI binary archives from GitHub Actions.

## Prepare a release

Start a release preparation workflow from the repository root:

```bash
gh workflow run prepare-release.yml -f version=0.2.0 -f run_full_ci=true
```

The workflow validates the requested semver version, runs `make upversion VERSION=...`, optionally runs `make ci-all`, and opens a `release/vX.Y.Z` pull request.

For local preparation:

```bash
make upversion VERSION=0.2.0
make ci
```

## Draft release workflow

When a `release/vX.Y.Z` pull request is merged into `main`, `draft-release.yml` validates the merged release commit, runs `make ci-all`, builds CLI launcher binaries, creates a draft GitHub Release for tag `vX.Y.Z`, and uploads binary archives plus `checksums.txt`.

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

The draft release also includes `checksums.txt` with SHA-256 checksums for all archives. `scripts/install.sh` downloads these artifacts, verifies checksums when present, and installs `starweaver`, `starweaver-cli`, and `sw`.

Local release smoke for CLI artifacts:

```bash
make cli-smoke
```

## Publish crates

Publishing the draft GitHub Release triggers `.github/workflows/release.yml`:

1. Validate the release tag against the workspace version.
2. Run `make ci-all`.
3. Dry-run the root crate publish package.
4. Publish crates in dependency order through the `Release` environment.

Manual dry-run:

```bash
make publish-dry-run
```

Manual publish after validation and approval:

```bash
make publish
```

## Install and update validation

Validate installer semantics through xtask:

```bash
make install-script-check
make scripts-check
```

Update installed CLI binaries:

```bash
starweaver update
starweaver update cli
starweaver cli update
```

CLI update commands set `STARWEAVER_COMPONENTS=cli` and preserve local configuration/session data.
