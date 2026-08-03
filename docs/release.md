# Release

Starweaver keeps all checked-in maintained Rust and Python package metadata at the permanent development
version `0.0.0-dev.0`. A release tag points directly at reviewed source on `main`; no release-version
commit, release branch, automated commit, or version pull request is created. Release jobs prepare the
selected public version only inside their isolated checkout.

> [!WARNING]
> Desktop is WIP. Desktop version updates, validation, packaging, Desktop-managed RPC update assets,
> and publication remain paused and are not part of the maintained release workflow.

## Release scopes and tags

Each canonical tag selects one release scope:

| Scope        | Tag                   | Published components                                                                  |
| ------------ | --------------------- | ------------------------------------------------------------------------------------- |
| Full         | `vX.Y.Z`              | CLI suite, host protocol, Computer Use MCP, Rust SDK crates, and Python distributions |
| CLI          | `cli-vX.Y.Z`          | CLI suite and host protocol                                                           |
| Computer Use | `computer-use-vX.Y.Z` | macOS Computer Use MCP archives                                                       |
| Rust SDK     | `sdk-vX.Y.Z`          | maintained crates on crates.io                                                        |
| Python       | `python-vX.Y.Z`       | Python sdist and wheels on PyPI                                                       |

SemVer prerelease tags such as `cli-v1.2.0-beta.1` produce prerelease GitHub Releases. Build metadata is
not accepted. Full and Python prereleases must use exactly one canonical PEP 440-compatible spelling:
`alpha.N`, `beta.N`, `rc.N`, or `dev.N`. Full stable releases are the only releases allowed to become
GitHub's `Latest` release; component and prerelease releases set `latest=false`.

The first release made after adopting this component contract must be a full `vX.Y.Z` release. This
preserves the existing `/releases/latest` bootstrap path while newer launchers learn to discover both
full and component releases.

## 0.7 boundary migration

The architecture consolidation intentionally changes released 0.6 contracts: CLI and standalone RPC
are independent products, protocol and durable DTOs use their new typed/versioned owners, runtime
checkpoint/stream contracts moved to lower owning crates, and `AgentContext` fields moved under
explicit components. These are accepted as a pre-1.0 minor-version break, so the unified workspace and
Python distribution version advances to `0.7.0`; prohibited CLI/RPC coupling and broad mutable context
fields are not restored as compatibility shims.

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

## Validate before tagging

Release automation packages and publishes; it does not replace the pre-tag validation gate. From a
clean `main` checkout, run:

```bash
make ci
make release-api-check TAG=vX.Y.Z
make cli-smoke
make py-wheel-smoke
make publish-dry-run
```

`make release-api-check TAG=vX.Y.Z` verifies the reviewed Rust facade, classified Python exports,
semver compatibility, and a built-wheel smoke test. The semver check compares the permanent
`0.0.0-dev.0` workspace against `last_verified_release` from `spec/capabilities.toml` and derives the
allowed release type from the candidate tag. Intentional Rust facade changes are accepted with
`cargo run -p xtask -- check-agent-api --bless` after review; intentional Python changes update
`tests/fixtures/api/top-level-v1.json` in the same review.

Validate the intended tag before pushing it:

```bash
make release-tag-check TAG=cli-vX.Y.Z
```

For a local rehearsal of the exact metadata transform, use a disposable checkout and do not commit its
changes:

```bash
git worktree add --detach ../starweaver-release-rehearsal HEAD
make -C ../starweaver-release-rehearsal release-prepare SCOPE=cli VERSION=X.Y.Z
# Run any scope-specific build or validation commands, then remove the worktree.
git worktree remove ../starweaver-release-rehearsal
```

`release-prepare` requires the checked-in development version and updates only the selected scope. A
full or SDK rehearsal rewrites workspace versions and internal dependency constraints; a Python
rehearsal rewrites Python package metadata; CLI and Computer Use releases retain source package
metadata and inject their public distribution identity at build time. None of these rehearsal changes
belong on `main`.

## Create a release

After validation succeeds, create one canonical tag on the exact reviewed commit and push only that
tag:

```bash
git switch main
git pull --ff-only
git tag -a vX.Y.Z -m "Starweaver vX.Y.Z"
git push origin vX.Y.Z
```

Use the corresponding component tag for a component-only release. Tags are immutable and must point
to a commit reachable from `main`; do not move or recreate a release tag.

`.github/workflows/release-components.yml` is the maintained tag router. It parses the tag with
`xtask release-tag` and invokes only the selected reusable lanes:

- `.github/workflows/release-cli.yml` builds four CLI/RPC archives and three protocol assets;
- `.github/workflows/release-computer-use.yml` builds the two macOS MCP archives;
- `.github/workflows/release-python.yml` builds the sdist and wheels;
- `.github/workflows/release-sdk.yml` publishes maintained crates in dependency order; and
- `.github/workflows/release-python-publish.yml` publishes prepared Python distributions.

Every build checkout pins the source SHA parsed by the router, rather than resolving the tag again,
and runs the reviewed scoped `release-prepare` transform. Binary builds receive
`STARWEAVER_RELEASE_VERSION` and the exact tagged commit through `STARWEAVER_BUILD_REVISION`, so
runtime diagnostics report the public distribution version, source revision, and Rust target even
though checked-in package metadata remains `0.0.0-dev.0`.

The router enforces this publication order:

1. build every downloadable asset selected by the scope;
2. generate and verify `starweaver-release.json`, including exact asset sizes and SHA-256 digests;
3. generate `checksums.txt` covering primary assets and the release manifest;
4. create a private draft GitHub Release, or verify that a retained draft has byte-identical assets;
5. publish crates.io when the scope is `full` or `sdk`;
6. publish PyPI when the scope is `full` or `python` (after crates.io for `full`); and
7. publish the verified GitHub Release last.

A failure leaves a draft rather than a partially public release. Reruns accept only a draft whose
complete asset set is byte-identical. Once public, assets are immutable and automation refuses to
replace them. The dormant `.github/workflows/release.yml` remains only as a reference container for
frozen Desktop/runtime jobs and is not called by the maintained release path.

## Release assets

CLI archives are built for:

- `starweaver-cli-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz`
- `starweaver-cli-vX.Y.Z-x86_64-apple-darwin.tar.gz`
- `starweaver-cli-vX.Y.Z-aarch64-apple-darwin.tar.gz`
- `starweaver-cli-vX.Y.Z-x86_64-pc-windows-msvc.zip`

Unix archives contain `starweaver`, `starweaver-cli`, `sw`, and `starweaver-rpc`; Windows archives
contain the corresponding `.exe` files.

CLI and full releases also include:

- `starweaver-host-X.Y.Z.openrpc.json`;
- `starweaver-host-X.Y.Z.manifest.json`; and
- `starweaver-host-X.Y.Z-schemas.tar.gz`.

Computer Use and full releases include:

- `starweaver-computer-use-mcp-vX.Y.Z-x86_64-apple-darwin.tar.gz`; and
- `starweaver-computer-use-mcp-vX.Y.Z-aarch64-apple-darwin.tar.gz`.

Computer Use MCP archives contain only `starweaver-computer-use-mcp`. Windows and Linux Computer Use
release binaries remain unavailable. The macOS binaries are checksum-covered but are not Apple
Developer ID signed or notarized. See [Computer Use](computer-use.md) for permission and local-policy
boundaries.

Python and full releases include one sdist and wheels for CPython 3.11, 3.12, and 3.13 on the
configured Linux, macOS, and Windows targets. Every scope also publishes `starweaver-release.json`
and `checksums.txt`; an SDK-only release has no component download payload beyond those metadata
files.

External TypeScript consumers generate complete bindings from the public host contract with
`make rpc-typescript-generate OUTPUT=<empty-or-generator-owned-directory>`. Starweaver does not
publish a separate TypeScript package.

## Registry publication and recovery

Manual crate dry-run and publication commands remain:

```bash
make publish-dry-run
make publish
```

`make publish` is idempotent at the package-version level: already-published versions are skipped and
remaining crates continue in dependency order.

If crates.io publication is interrupted after a tag exists, dispatch the recovery workflow against
that immutable full or SDK tag:

```bash
gh workflow run publish-crates.yml -f tag=sdk-vX.Y.Z
```

The recovery job checks out the tag, parses its scope/version, applies the same ephemeral metadata
transform, requires the `Release` environment, and publishes only missing crates. Do not move the tag
or create a new version commit. PyPI and GitHub draft failures are recovered by rerunning the failed
jobs in the original tag workflow; `uv publish --check-url` skips distributions already visible on
PyPI.

## Required repository settings

- `CARGO_REGISTRY_TOKEN` is available to the `Release` environment.
- `PYPI_API_TOKEN` is available to the `Release` environment.
- The `Release` environment has the intended approval policy.
- GitHub Actions can create and edit Releases with job-scoped `contents: write` permission.
- A repository tag ruleset prevents canonical release tags from being updated or deleted; immutable
  GitHub Releases should also be enabled. Workflow SHA pinning remains authoritative during a run.
- Frozen Desktop updater variables may remain configured but are not consumed by maintained release
  lanes.

## After publishing

Verify the public install and update paths:

```bash
curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh \
  | STARWEAVER_VERSION=vX.Y.Z sh

starweaver version
starweaver update --dry-run
sw cli -p "hello" --output text
```
