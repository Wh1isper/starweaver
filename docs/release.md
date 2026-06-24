# Release

Starweaver uses one workspace version for crates and CLI artifacts. The repository development
version should stay on a pre-release version such as `X.Y.Z-dev.0`. A release commit promotes that
version to the public release version `X.Y.Z`.

Publishing a GitHub Release for a `vX.Y.Z` tag is the publishing trigger. The tag must point at a
commit whose workspace version is exactly `X.Y.Z`.

## Prepare release

Prepare a release branch from the repository root:

```bash
gh workflow run prepare-release.yml -f version=X.Y.Z
```

The workflow:

1. validates the requested semver version,
2. runs `make upversion VERSION=X.Y.Z`,
3. runs fast release preparation checks,
4. pushes `release/vX.Y.Z`,
5. writes the manual pull request URL to the workflow summary.

After the release pull request is merged into `main`, publish the GitHub Release:

```bash
gh release create vX.Y.Z --target main --title "Starweaver vX.Y.Z" --generate-notes
```

For fully local preparation:

```bash
make upversion VERSION=X.Y.Z
make ci
make cli-smoke
make publish-dry-run
git add Cargo.toml Cargo.lock
git commit -m "Prepare release vX.Y.Z"
git push
gh release create vX.Y.Z --target main --title "Starweaver vX.Y.Z" --generate-notes
```

## Release workflow

Publishing the GitHub Release triggers `.github/workflows/release.yml`:

1. build CLI launcher binaries from the release tag,
2. upload binary archives and `checksums.txt` to the GitHub Release,
3. publish all workspace crates in dependency order through the `Release` environment.

Release-event publishing is packaging-only. Run validation before merging the release pull request,
not inside `.github/workflows/release.yml`.

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
starweaver-rpc
```

Windows archives contain:

```text
starweaver.exe
starweaver-cli.exe
sw.exe
starweaver-rpc.exe
```

The release also includes `checksums.txt` with SHA-256 checksums for all archives.

## Publish crates

Manual dry-run:

```bash
make publish-dry-run
```

Dependent crates cannot always be fully dry-run against crates.io before the matching Starweaver
dependency versions are published. The dry-run target validates the release package lists and
dry-runs the dependency-free first-wave crates: `starweaver-core`, `starweaver-usage`, and
`starweaver-oauth`.

Manual publish after validation and approval:

```bash
make publish
```

## Required repository settings

- `CARGO_REGISTRY_TOKEN` secret is configured.
- The `Release` environment exists and requires the intended approval policy.
- The target tag, such as `vX.Y.Z`, does not already exist.
- GitHub Actions has `contents: write` permission so release assets can be uploaded.

## After publishing

Verify the public install path:

```bash
STARWEAVER_VERSION=vX.Y.Z \
  curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh | sh

starweaver version
sw cli -p "hello" --output text
```
