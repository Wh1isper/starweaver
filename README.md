# Starweaver

Starweaver is a Rust agent SDK for building local-first AI agents, CLIs, and service runtimes.
It gives you a typed agent loop, provider-neutral model protocol, function tools, structured
output, durable session primitives, first-party environment tools, and a CLI product surface in
one workspace.

> [!WARNING]
> **Desktop is work in progress.** `apps/starweaver-desktop` is experimental, is not a
> supported product, and is currently excluded from CI, release packaging, and active
> maintenance. Use the CLI, Rust/Python SDKs, or standalone RPC host for supported workflows.

## Why Starweaver

- Rust-native agent construction with `AgentBuilder`, `AgentApp`, and `AgentSession`.
- Provider-neutral model messages, settings, profiles, streaming parts, and request audit hooks.
- Typed function tools from Serde and `schemars`, plus toolsets, metadata, retries, approval, and deferred records.
- Structured output through JSON Schema, typed parsing, output functions, and validation retry.
- Runtime extension hooks for prompt preparation, request shaping, tool policy, output validation, usage, and trace recording.
- Durable execution foundations: context export/restore, checkpoints, session records, replay streams, and SQLite storage adapters.
- First-party SDK bundles for filesystem, shell, skills, task tracking, host search/scrape/media adapters, MCP, subagents, bounded CodeAct composition, reusable recipes, and opt-in macOS Computer Use observation, pointer/keyboard input, and bounded Accessibility metadata.
- A CLI launcher with profile-based local runs, install/update flow, display messages, local storage, and release artifacts.
- An experimental, maintenance-paused Tauri 2 Desktop foundation retained as WIP source code.

## Install

Install the latest public release:

```bash
curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh | sh
```

The installer downloads all components available for the current platform and requires a valid
`checksums.txt` entry for every archive. It installs `starweaver`, `starweaver-cli`, `sw`, and
`starweaver-rpc`; on macOS it also installs the full-family external-harness
`starweaver-computer-use-mcp` binary. It installs into `$HOME/.local/bin` for normal users and
`/usr/local/bin` for root. Override the location when needed:

```bash
curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh \
  | STARWEAVER_INSTALL_DIR="$HOME/bin" sh
```

Install a pinned release or prerelease:

```bash
curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh \
  | STARWEAVER_VERSION=vX.Y.Z sh
```

The default `latest` channel uses GitHub's latest stable release first and falls back to the
newest public prerelease when no stable release exists yet.

To omit the separate Computer Use MCP binary from a macOS install:

```bash
curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh \
  | STARWEAVER_EXCLUDE_COMPONENTS=computer-use sh
```

Installing the binary does not enable Computer Use in CLI or RPC. Both products keep the Toolset
default-disabled and use the Computer Use library in-process instead of this MCP binary. Launching the
standalone MCP server is its own explicit opt-in to the full canonical observe, pointer, and keyboard
family. Screen Recording and Accessibility/post-event permissions remain authoritative. Unsigned and
not-notarized executables may have less stable TCC identity and additional OS warnings, but signing is
not an input-availability gate. See [Computer Use](docs/computer-use.md) for configuration and security
boundaries.

Update all installed components available for the current platform from GitHub release artifacts:

```bash
starweaver update
```

The native update command discovers full and component Releases, verifies the exact Release manifest,
target, byte size, and SHA-256 before extraction, and never downloads or executes the mutable bootstrap
installer. It exits with `status=up-to-date` only when every selected component is complete at the
selected version; pass `--force` to reinstall the same version.

Run from a checkout:

```bash
make cli -- -p "hello" --output text
make sw -- --help
make sw -- -p "hello" --output text
make sw -- version
```

## SDK Quickstart

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, TestModel};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let agent = AgentBuilder::new(Arc::new(TestModel::with_text("Paris")))
    .instruction("Answer concisely.")
    .build();

let result = agent.run("What is the capital of France?").await?;
assert_eq!(result.output, "Paris");
# Ok(())
# }
```

Add typed tools:

```rust
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starweaver_agent::{typed_tool, AgentBuilder, TestModel, ToolContext, ToolResult};

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
struct LookupArgs {
    /// City to look up.
    city: String,
}

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let lookup = typed_tool::<LookupArgs, _, _>(
    "lookup_weather",
    Some("Look up weather for a city".to_string()),
    |_ctx: ToolContext, args: LookupArgs| async move {
        Ok(ToolResult::new(serde_json::json!({
            "city": args.city,
            "forecast": "clear"
        })))
    },
);

let agent = AgentBuilder::new(Arc::new(TestModel::with_text("clear")))
    .tool(Arc::new(lookup))
    .build();

let result = agent.run("What is the weather in Paris?").await?;
assert_eq!(result.output, "clear");
# Ok(())
# }
```

## Documentation

Published docs: <https://starweaver.wh1isper.top>

Start here:

- [Quickstart](docs/quickstart.md)
- [Agent SDK](docs/agent-sdk.md)
- [Python SDK](docs/python-sdk.md)
- [Agents](docs/agent.md)
- [Models](docs/models.md)
- [Tools](docs/tools.md)
- [Structured Output](docs/output.md)
- [CLI](docs/cli.md)
- [Computer Use](docs/computer-use.md)
- [Testing](docs/testing.md)
- [Release](docs/release.md)

Architecture and product decisions live in [spec/](spec/). User-facing guides live in [docs/](docs/).

## Workspace

Starweaver is organized as focused crates:

- `starweaver-agent`: public SDK facade, app/session helpers, bundles, subagents, profiles, and filters.
- `starweaver-runtime`: deterministic agent loop, graph state, tools, output, retries, capabilities, streams, traces, and checkpoints.
- `starweaver-model`: provider-neutral model protocol, settings, profiles, transports, wrappers, OAuth-backed adapters, and replay tests.
- `starweaver-tools`: function tools, toolsets, metadata, lifecycle, MCP foundations, approval, and deferred execution.
- `starweaver-context`: `AgentContext`, typed dependencies, state, event/message buses, notes, usage, and resumable state.
- `starweaver-environment`: local and virtual filesystem/shell providers, policies, resources, and environment snapshots.
- `starweaver-computer-use`: typed current-active-desktop service, canonical tool router, deterministic fake, macOS pixel observation plus bounded Accessibility and high-level pointer/keyboard backend, and feature-gated full-family stdio MCP binary.
- `starweaver-session`, `starweaver-stream`, `starweaver-storage`: durable session, replay, display stream, and SQLite storage contracts.
- `starweaver-cli`: local CLI product surface, launcher dispatch, profiles, TUI, storage, install, and update workflows.
- `starweaver-rpc-core`: generated IDL-first `starweaver.host` major-1 wire boundary, strict server/client codecs, typed errors/events/cursors, launch-envelope contracts, and narrow framing/projection helpers; the generated contract atomically replaces handwritten DTO authority.
- `starweaver-rpc`: standalone JSON-RPC host process for local and external integrations and implementer of the generated single-contract server boundary; HTTP exposes only `POST /rpc`.
- `apps/starweaver-desktop`: experimental WIP Tauri 2 shell; CI, release packaging, and active maintenance are paused.
- `packages/starweaver-py`: in-process Python SDK bindings, Python tool injection, live run control, message bus facades, typed HITL helpers, deterministic test models, sessions, and Python distribution artifacts.

The canonical host protocol lives under `protocol/host/`. Maintained generation writes the bundled OpenRPC artifact, core manifest, and Rust server bindings without changing frozen Desktop projections:

```bash
make rpc-idl-core-generate
```

The historical full `make rpc-idl-generate` target is retained for a future Desktop reactivation and must not be used while the maintenance freeze is active.

External TypeScript consumers can generate complete bindings into a directory they own. Starweaver does not maintain or publish a separate TypeScript protocol package:

```bash
cargo run -p xtask -- generate-rpc-typescript --output path/to/generated-host
# or
make rpc-typescript-generate OUTPUT=path/to/generated-host
make rpc-typescript-check
```

## Validation

```bash
make fmt-check
make check
make test
make docs-check
make docs-build
```

Desktop-specific validation and build targets are intentionally outside the maintained gate while the Desktop project is paused.

Full local gate:

```bash
make ci
```

## Release

Checked-in Rust and Python package metadata stays at `0.0.0-dev.0`. After validating a clean `main`,
push one canonical tag such as `vX.Y.Z`, `cli-vX.Y.Z`, `computer-use-vX.Y.Z`, `sdk-vX.Y.Z`, or
`python-vX.Y.Z`:

```bash
make release-tag-check TAG=vX.Y.Z
git tag -a vX.Y.Z -m "Starweaver vX.Y.Z"
git push origin vX.Y.Z
```

The tag router prepares the selected public version only in isolated CI checkouts, stages verified
assets in a draft Release, publishes the selected registries, and publishes the immutable GitHub
Release last. It does not create a release commit or branch. Desktop installers and Desktop-managed
runtime update assets remain disabled while Desktop is WIP. See [Release](docs/release.md) for scope,
asset, recovery, and validation details.

## Acknowledgements

Thank you to the projects that helped shape Starweaver's thinking, especially [Pydantic AI](https://github.com/pydantic/pydantic-ai) and [Yet Another Agents / ya-mono](https://github.com/Wh1isper/ya-mono).

## License

BSD-3-Clause
