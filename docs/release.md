# Release

Starweaver uses one workspace version for maintained Rust crates, CLI artifacts, and Python distributions. The repository development version should stay on a pre-release version such as `X.Y.Z-dev.0`. A release commit promotes that version to the public release version `X.Y.Z`.

> [!WARNING]
> Desktop is WIP. Desktop version updates, validation, packaging, Desktop-managed RPC update assets,
> and publication are paused and are not part of the maintained release workflow.

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
3. runs `make upversion VERSION=X.Y.Z`, updating maintained Rust and Python metadata,
4. runs maintained IDL, RPC, independent-client, Python, documentation, publish-dry-run, and `make release-api-check` gates,
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
git add Cargo.toml Cargo.lock pyproject.toml uv.lock packages/starweaver-py
git commit -m "Prepare release vX.Y.Z"
git push
gh release create vX.Y.Z --target "$(git rev-parse HEAD)" --title "Starweaver vX.Y.Z" --generate-notes
```

## Release workflow

Publishing the GitHub Release triggers `.github/workflows/release.yml`:

1. build CLI launcher archives, including the standalone RPC binary,
2. build the observe-only `starweaver-computer-use-mcp` binary for both macOS Rust targets,
3. package the self-contained public host OpenRPC bundle, generated manifest, and canonical source schemas,
4. build Python source and wheel distributions for `packages/starweaver-py`,
5. upload maintained core assets with `checksums.txt`,
6. publish maintained workspace crates, including `starweaver-computer-use`, in dependency order through the `Release` environment, and
7. publish the Python package to PyPI through the `Release` environment.

The retained Desktop installer and Desktop-managed runtime update jobs are statically disabled while
Desktop remains WIP. They do not build, sign, attest, or upload release assets.

Release-event publishing is packaging-only. Run validation before merging the release pull request,
not inside `.github/workflows/release.yml`. Published core asset names are immutable: release jobs
refuse to replace an existing asset and publish payloads before checksums. A transient failure before
any upload may be rerun. If
an upload stops after creating only part of a lane, maintainers must inspect and explicitly remove the
partial assets before retrying, or publish a new version; automation never deletes a previously
published asset on its own.

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

- `starweaver-computer-use-mcp-vX.Y.Z-x86_64-apple-darwin.tar.gz`;
- `starweaver-computer-use-mcp-vX.Y.Z-aarch64-apple-darwin.tar.gz`;
- `starweaver-host-X.Y.Z.openrpc.json`, the self-contained public OpenRPC bundle;
- `starweaver-host-X.Y.Z.manifest.json`, the generated protocol identity and inventory manifest;
- `starweaver-host-X.Y.Z-schemas.tar.gz`, the canonical split source schemas and pinned tooling profile; and
- `checksums.txt` for maintained CLI, protocol, and Python release assets.

Computer Use MCP archives contain only `starweaver-computer-use-mcp`. Windows and Linux Computer
Use release binaries are TBD and are not built or published. The macOS binaries are checksum-covered
but are not Apple Developer ID signed or notarized. Computer Use is therefore a provisional
observe-only component, not a production-ready macOS capture identity, even when the default
installer includes it. Stable TCC identity, notarization, and permission continuity for exact release
bytes are required before that status can graduate; see [Computer Use](computer-use.md) for the
permission and local-policy boundary.

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
dependency versions are published. The dry-run target validates the release package lists, including
`starweaver-computer-use`, and dry-runs the dependency-free first-wave crates:
`starweaver-core`, `starweaver-usage`, and
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
- Desktop updater signing variables may remain configured, but the frozen Desktop/runtime jobs do not consume them.
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
curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh \
  | STARWEAVER_VERSION=vX.Y.Z sh

starweaver version
sw cli -p "hello" --output text
```
