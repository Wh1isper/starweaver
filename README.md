# Starweaver

Starweaver is a Rust agent SDK for building local-first AI agents, CLIs, and service runtimes.
It gives you a typed agent loop, provider-neutral model protocol, function tools, structured
output, durable session primitives, first-party environment tools, and a CLI product surface in
one workspace.

## Why Starweaver

- Rust-native agent construction with `AgentBuilder`, `AgentApp`, and `AgentSession`.
- Provider-neutral model messages, settings, profiles, streaming parts, and request audit hooks.
- Typed function tools from Serde and `schemars`, plus toolsets, metadata, retries, approval, and deferred records.
- Structured output through JSON Schema, typed parsing, output functions, and validation retry.
- Runtime extension hooks for prompt preparation, request shaping, tool policy, output validation, usage, and trace recording.
- Durable execution foundations: context export/restore, checkpoints, session records, replay streams, and SQLite storage adapters.
- First-party SDK bundles for filesystem, shell, skills, task tracking, host search/scrape/media adapters, MCP, and subagents.
- A CLI launcher with profile-based local runs, install/update flow, display messages, local storage, and release artifacts.
- A three-platform Tauri 2 Desktop foundation with a least-authority renderer bridge, single-instance lifecycle, and native CI matrix.

## Install

Install the latest public release:

```bash
curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh | sh
```

The installer downloads the matching `starweaver-cli` archive, verifies `checksums.txt` when
available, and installs `starweaver`, `starweaver-cli`, `sw`, and `starweaver-rpc`. It installs into
`$HOME/.local/bin` for normal users and `/usr/local/bin` for root. Override the location when
needed:

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

Update an installed CLI from GitHub release artifacts:

```bash
starweaver update
```

The update command checks the current CLI package version before invoking the installer. When the
selected release is already installed it exits with `status=up-to-date`; pass `--force` to reinstall
the same version.

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
- `starweaver-session`, `starweaver-stream`, `starweaver-storage`: durable session, replay, display stream, and SQLite storage contracts.
- `starweaver-cli`: local CLI product surface, launcher dispatch, profiles, TUI, storage, install, and update workflows.
- `starweaver-rpc-core`: current major-1 JSON-RPC helpers plus the planned generated IDL-first major-2 wire boundary and stream/replay projections.
- `starweaver-rpc`: standalone JSON-RPC host process for local and external host integrations and future implementer of the generated major-2 server contract.
- `apps/starweaver-desktop`: Tauri 2 shell foundation for Linux, macOS, and Windows; RPC supervision remains gated on the public launch contract and IDL-first host major 2.
- `packages/starweaver-py`: in-process Python SDK bindings, Python tool injection, live run control, message bus facades, typed HITL helpers, deterministic test models, sessions, and Python distribution artifacts.

## Validation

```bash
make fmt-check
make check
make test
make docs-check
make docs-build
make desktop-check
```

Build the current-platform Desktop shell without an installer:

```bash
make desktop-build
```

Full local gate:

```bash
make ci
```

## Release

Prepare a release:

```bash
gh workflow run prepare-release.yml -f version=X.Y.Z
```

The workflow pushes `release/vX.Y.Z` for review. After that release commit reaches `main`, publish `vX.Y.Z` as a GitHub Release; the published Release event builds CLI archives and Python distributions, uploads checksums, and publishes crates plus the Python package through the `Release` environment.

## Acknowledgements

Thank you to the projects that helped shape Starweaver's thinking, especially [Pydantic AI](https://github.com/pydantic/pydantic-ai) and [Yet Another Agents / ya-mono](https://github.com/Wh1isper/ya-mono).

## License

BSD-3-Clause
