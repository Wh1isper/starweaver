# Release

Starweaver uses one workspace version for crates, CLI artifacts, Python distributions, and Desktop
shell package metadata. The repository development version should stay on a pre-release version such as `X.Y.Z-dev.0`. A release
commit promotes that version to the public release version `X.Y.Z`.

Publishing a GitHub Release for a `vX.Y.Z` tag is the publishing trigger. The tag must point at a
commit whose workspace version is exactly `X.Y.Z`.

## 0.7 boundary migration

The architecture consolidation intentionally changes released 0.6 contracts: CLI and standalone RPC are independent products, protocol and durable DTOs use their new typed/versioned owners, runtime checkpoint/stream contracts moved to lower owning crates, and `AgentContext` fields moved under explicit components. These are accepted as a pre-1.0 minor-version break, so the unified workspace and Python distribution version advances to `0.7.0`; prohibited CLI/RPC coupling and broad mutable context fields are not restored as compatibility shims.

## Current context migration notes

The Phase 3 context decomposition includes intentional Rust source changes:

- execution-only fields moved below `AgentContext.runtime`: `force_inject_context`,
  `injected_context_tags`, `context_manage_tool_names`, `tool_tags`, `tool_id_wrapper`,
  `agent_stream_queues`, `wrapper_metadata`, `lifecycle`, and `current_run_step`;
- agent-owned tool state moved below `AgentContext.tools`: `shell_env` became
  `shell_environment`, `deferred_tool_metadata` became `deferred_call_metadata`,
  `auto_load_files` retained its name, `task_manager` became `tasks`,
  `tool_search_loaded_tools` became `loaded_tool_names`, and
  `tool_search_loaded_namespaces` became `loaded_tool_namespaces`;
- tool calls no longer receive `Arc<AgentContext>` as an immutable typed dependency;
- tools that read model/tool limits or shell configuration should use `ToolRuntimeSnapshot`;
- tools that read attached host integrations should use `HostCapabilities` or their explicit typed
  dependency;
- first-party tools that mutate handoff, task, usage, or dynamic tool-search state use the
  capability-specific `ContextHandoffHandle`, `TaskContextHandle`, `UsageContextHandle`, or
  `ToolSearchContextHandle`; broad `AgentContextHandle` injection remains only in the Legacy
  compatibility profile;
- Filtered is the first-party structural-narrowing default, while Strict omits ambient application
  dependencies and intersects requested host, shell, and mutable context capabilities with the
  per-tool `ToolCapabilityGrant` explicitly installed by the host.

Non-secret fields retain the flattened serialized `AgentContext` key layout, and legacy flat JSON
input remains readable. `shell_env` is intentionally input-compatible only: its values are restored
into `context.tools.shell_environment` but are never emitted by context or resumable-state
serialization. Direct Rust field access must be updated to `context.runtime.<execution_field>` or
`context.tools.<tool_state_field>`. These changes must be called out in release notes and reviewed
under the workspace semver policy.

## Prepare release

Prepare a release branch from the repository root:

```bash
gh workflow run prepare-release.yml -f version=X.Y.Z
```

The workflow:

1. validates the requested semver version,
2. installs the pinned `cargo-semver-checks` `0.48.0`,
3. runs `make upversion VERSION=X.Y.Z`, updating Rust, Python, and Desktop shell metadata,
4. runs the IDL, RPC, independent-client, Desktop-boundary, Python, documentation, publish-dry-run, and `make release-api-check` gates,
5. pushes `release/vX.Y.Z`,
6. writes the manual pull request URL to the workflow summary.

After the release pull request is merged into `main`, publish the GitHub Release:

```bash
merge_commit=$(gh pr view <release-pr-number> --json mergeCommit --jq .mergeCommit.oid)
gh release create vX.Y.Z --target "$merge_commit" --title "Starweaver vX.Y.Z" --generate-notes
```

Use the release pull request merge commit as the release target, not the mutable `main` branch, so
the tag always points at the reviewed release commit.

`make release-api-check` verifies three reviewed boundaries: the `starweaver-agent` root,
`prelude`, and `advanced` allowlist snapshot; the classified Python top-level export snapshot; and
Rust semver compatibility against the latest release. `starweaver-storage` is excluded from the
registry comparison for 0.7 because no 0.6 crate was published; remove that first-publication
exception after 0.7 becomes its baseline. The gate also smoke-tests the built Python wheel.
Intentional Rust facade changes are accepted with `cargo run -p xtask -- check-agent-api --bless`
after review; intentional Python changes update `tests/fixtures/api/top-level-v1.json` in the same
review.

For fully local preparation, install the same checker version used by CI first:

```bash
cargo install cargo-semver-checks --version 0.48.0 --locked
make upversion VERSION=X.Y.Z
make ci
make release-api-check
make cli-smoke
make py-wheel-smoke
make publish-dry-run
git add Cargo.toml Cargo.lock pyproject.toml uv.lock packages/starweaver-py \
  apps/starweaver-desktop/package.json apps/starweaver-desktop/src-tauri/tauri.conf.json
git commit -m "Prepare release vX.Y.Z"
git push
gh release create vX.Y.Z --target "$(git rev-parse HEAD)" --title "Starweaver vX.Y.Z" --generate-notes
```

## Release workflow

Publishing the GitHub Release triggers `.github/workflows/release.yml`:

1. build CLI launcher binaries from the release tag,
2. package the self-contained public host OpenRPC bundle, generated manifest, and canonical source schemas,
3. build Python source and wheel distributions for `packages/starweaver-py`,
4. upload binary archives, protocol artifacts, Python distributions, and `checksums.txt` to the GitHub Release,
5. publish all workspace crates in dependency order through the `Release` environment,
6. publish the Python package to PyPI through the `Release` environment.

The Desktop shell version participates in unified preparation, but the current shell foundation is
not uploaded by this workflow. Desktop installers, signing/notarization, native updater metadata,
and managed runtime artifacts remain gated by `spec/desktop/06-runtime-updates-and-release.md`.

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

The release also includes:

- `starweaver-host-X.Y.Z.openrpc.json`, the self-contained public OpenRPC bundle;
- `starweaver-host-X.Y.Z.manifest.json`, the generated protocol identity and inventory manifest;
- `starweaver-host-X.Y.Z-schemas.tar.gz`, the canonical split source schemas and pinned tooling profile; and
- `checksums.txt` with SHA-256 checksums for all binary, protocol, and Python artifacts.

External TypeScript consumers generate complete bindings from the public contract with
`make rpc-typescript-generate OUTPUT=<empty-or-generator-owned-directory>`. Starweaver does not
publish or maintain a separate TypeScript package.

Python distributions include an sdist plus wheels for CPython 3.11, 3.12, and 3.13 on the configured
Linux, macOS, and Windows targets.

`make py-wheel-smoke` installs the locally built wheel into a clean virtual
environment, runs a deterministic in-process agent smoke, and runs the
Claw-like Python library-path and minimal product-runtime smoke examples
against the installed artifact.

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

## Recover interrupted crate publishing

If the release workflow published release assets or Python distributions but failed before all crates
reached crates.io, do not rerun the original release workflow from an outdated release tag. First
merge a reviewed publishing fix, then dispatch the dedicated crate-publish workflow from that commit:

```bash
gh workflow run publish-crates.yml -f version=X.Y.Z
```

The workflow requires the `Release` environment approval, verifies that the checked-out workspace
has exactly the requested version, and runs the idempotent `make publish` command. Already-published
crate versions are skipped; remaining crates are published in dependency order. Preserve the existing
GitHub Release tag during recovery; do not move, delete, or recreate it.

## Required repository settings

- `CARGO_REGISTRY_TOKEN` secret is configured.
- `PYPI_API_TOKEN` secret is configured with a PyPI API token for the `starweaver` package.
- The `Release` environment exists and requires the intended approval policy.
- Before the initial GitHub Release is created, the target tag, such as `vX.Y.Z`, does not already
  exist. Recovery publishing reuses the existing release tag without changing it.
- GitHub Actions has `contents: write` permission so release assets can be uploaded.

The release workflow maps `PYPI_API_TOKEN` to `UV_PUBLISH_TOKEN` for `uv publish`.
It publishes with `uv publish --check-url https://pypi.org/simple/starweaver/` so reruns skip
distribution files that are already visible on PyPI.

## After publishing

Verify the public install path:

```bash
STARWEAVER_VERSION=vX.Y.Z \
  curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh | sh

starweaver version
sw cli -p "hello" --output text
```
