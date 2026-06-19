# Release

Starweaver uses one workspace version for crates and CLI artifacts. The repository development
version should stay on a pre-release version such as `0.0.1-dev.0`. A release commit promotes that
version to the public release version, such as `0.0.1`.

Publishing a GitHub Release for a `vX.Y.Z` tag is the publishing trigger. The tag must point at a
commit whose workspace version is exactly `X.Y.Z`.

## First release

Prepare the first public release branch from the repository root:

```bash
gh workflow run prepare-release.yml -f version=0.0.1
```

The workflow:

1. validates the requested semver version,
2. runs `make upversion VERSION=0.0.1`,
3. runs fast release preparation checks,
4. pushes `release/v0.0.1`,
5. writes the manual pull request URL to the workflow summary.

After the release pull request is merged into `main`, publish the GitHub Release:

```bash
gh release create v0.0.1 --target main --title "Starweaver v0.0.1" --generate-notes
```

For fully local preparation:

```bash
make upversion VERSION=0.0.1
make ci
make cli-smoke
make publish-dry-run
git add Cargo.toml Cargo.lock crates/*/Cargo.toml
git commit -m "Prepare release v0.0.1"
git push
gh release create v0.0.1 --target main --title "Starweaver v0.0.1" --generate-notes
```

## Release workflow

Publishing the GitHub Release triggers `.github/workflows/release.yml`:

1. validate the release tag against the workspace version,
2. run `make ci`,
3. run `make cli-smoke`,
4. build CLI launcher binaries,
5. upload binary archives and `checksums.txt` to the GitHub Release,
6. dry-run first-wave publish packages,
7. publish all workspace crates in dependency order through the `Release` environment.

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

The release also includes `checksums.txt` with SHA-256 checksums for all archives.

## Publish crates

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
- GitHub Actions has `contents: write` permission so release assets can be uploaded.

## After publishing

Verify the public install path:

```bash
STARWEAVER_VERSION=v0.0.1 \
  curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh | sh

starweaver version
sw cli -p "hello" --output text
```
