# Product Boundary And Package Shape

`starweaver-py` is the Python product layer for Starweaver's in-process SDK.
It is a real Python package plus a PyO3 native extension, not a wrapper around
the CLI, JSON-RPC host protocol, or MCP.

## Decision

The Python SDK should make Python applications feel first-class while keeping
Starweaver's runtime contract native:

- Python owns application ergonomics and callback adaptation.
- Rust owns execution, model protocol, tool-loop semantics, state, streaming,
  usage, tracing, and durable evidence.
- Python tools enter the runtime as native Starweaver tools in the same process.
- Advanced Python control APIs must be backed by neutral Rust SDK seams before
  they become public.

## Why This Exists

Claw and other Python products need to define application resources, callbacks,
tools, and orchestration in Python while still using Starweaver's agent loop and
evidence model. A sidecar process or MCP-only bridge would make Python a host
integration instead of an SDK.

The package should support this shape:

```python
from starweaver import create_agent, tool


@tool
async def lookup(query: str) -> dict[str, str]:
    return {"query": query, "value": "ready"}


async def main() -> None:
    async with create_agent(model=model, tools=[lookup]) as agent:
        result = await agent.run("Use lookup")
        print(result.output)
```

## Goals

- Provide a first-class Python package named `starweaver`.
- Keep the agent/tool/session path in process.
- Let Python functions and classes become native Starweaver `Tool`
  implementations.
- Expose Pythonic agent, session, stream, state, output, HITL, subagent,
  capability, and model helper APIs.
- Expose the active-run control plane for steering, interruption, message bus,
  and typed HITL through Rust-backed live handles.
- Preserve Starweaver-native result, stream, usage, state, and error evidence.
- Keep Python package implementation details isolated from core Rust crates.
- Give Claw a narrow library path first, then deeper resource/environment
  integration after the lifecycle is proven.

## Non-Goals

- Do not wrap `sw`, `starweaver-cli`, or `starweaver-rpc` as the core Python
  execution path.
- Do not use MCP as the Python tool injection mechanism.
- Do not mirror every Rust API one for one.
- Do not move Python dependencies into core Rust crates.
- Do not create a second runtime, stream protocol, message bus, or state
  format.
- Do not make Claw-specific product policy part of `starweaver-py`.
- Do not hide Starweaver ids that applications need for persistence, replay,
  approval decisions, deferred work, and trace correlation.
- Do not implement live steering by mutating exported state or latest-context
  snapshots.

## Owned Surfaces

`packages/starweaver-py` owns:

- `pyproject.toml` and Python package metadata
- the PyO3 extension crate `starweaver-py`
- the extension module `starweaver._native`
- pure Python facades under `python/starweaver`
- Python decorators, dataclasses, context managers, and helper objects
- Pydantic integration and type-hint schema extraction
- Python callback scheduling, GIL boundaries, and traceback capture
- Python tests and wheel packaging

It does not own:

- runtime state transitions
- provider request mapping
- native tool-loop semantics
- message history normalization
- stream record schema
- session store contracts
- environment provider contracts
- usage accounting and pricing contracts
- host-control JSON-RPC protocol

If Python needs a new runtime behavior, first express it as a Rust SDK contract
that is useful without Python.

## Current Package Shape

Current package paths:

```text
packages/starweaver-py/
  Cargo.toml
  pyproject.toml
  src/
    agent.rs
    capability.rs
    context.rs
    conversion.rs
    environment.rs
    errors.rs
    lib.rs
    media.rs
    model.rs
    output.rs
    runtime.rs
    skills.rs
    store.rs
    stream.rs
    subagent.rs
    testing.rs
    tool.rs
    toolset.rs
  python/starweaver/
    __init__.py
    _native.pyi
    agent.py
    capability.py
    environment.py
    errors.py
    media.py
    model.py
    observability.py
    output.py
    py.typed
    resources.py
    runtime.py
    skills.py
    store.py
    stream_adapter.py
    subagent.py
    testing.py
    tool.py
    toolset.py
  tests/
    test_package.py
```

Deferred module splits:

```text
python/starweaver/
  hitl.py
  messages.py
  providers.py
  run.py
  state.py
  stream.py
```

These splits should happen only when they improve ownership clarity. Today the
related public concepts intentionally live in `agent.py`, `model.py`,
`resources.py`, `store.py`, and `stream_adapter.py`; do not create empty
namespace churn just to match a planned layout.

## Naming And Versioning

- Rust crate: `starweaver-py`
- Python distribution: `starweaver`
- Python import: `starweaver`
- Native extension: `starweaver._native`
- Supported Python range: CPython 3.11 through 3.13
- Local default: Python 3.13 through the repository `.python-version`

The Python version should follow the workspace release version. Public Python
API additions should preserve raw escape hatches so downstream applications are
not blocked by helper-class churn.

## Dependency Boundary

The binding crate may depend on Starweaver SDK/runtime crates, including:

- `starweaver-agent`
- `starweaver-context`
- `starweaver-core`
- `starweaver-model`
- `starweaver-oauth-provider`
- `starweaver-runtime`
- `starweaver-session`
- `starweaver-stream`
- `starweaver-storage` when native store facades are exposed
- `starweaver-tools`
- `starweaver-usage`
- `starweaver-environment` when environment wrappers are added

Core crates must not depend on `starweaver-py`.

The root Rust workspace excludes `packages/starweaver-py`. Keep that boundary
unless the PyO3 lint, build, and wheel behavior can be made workspace-native
without weakening core crate rules.

## Lint And Unsafe Boundary

PyO3 generates FFI glue, so any required `unsafe_code = "allow"` exception must
stay local to `packages/starweaver-py/Cargo.toml`.

Core Starweaver crates keep the workspace `unsafe_code = "forbid"` rule.

## Provider Boundary

Python provider helpers should build Starweaver model adapters. They must not
become untyped HTTP escape hatches.

Allowed Python provider surfaces:

- deterministic test models
- callback-backed `FunctionModel`
- `ProviderModel` helpers backed by Rust provider adapters
- typed `ModelSettings`
- typed `RequestParams`
- profile/model-id helpers that resolve through Starweaver-owned code

Provider-specific routing affinity remains in typed provider settings. Generic
Python metadata must not become a place to smuggle provider session headers.

## Claw Boundary

Claw can build on top of `starweaver-py` by registering tools, resources,
approval UIs, and product policies. Those decisions should stay above the
binding crate.

`starweaver-py` should expose the primitives Claw needs:

- tools
- sessions
- streams
- state export/restore
- session-store records and archive helpers
- HITL decisions
- active control
- resource references
- environment bindings
- stream replay adapters
- usage and trace evidence
- observability evidence

It should not encode Claw-specific queue names, UI concepts, policy decisions,
or storage layout.

## Product Boundary Questions

| Question                                            | Current recommendation                                                                            |
| --------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| Should `AgentStream` be renamed?                    | `AgentRun` is the public live handle; `AgentStream` remains a compatibility alias.                |
| Should `Agent.session()` exist?                     | Yes. It is the Pythonic alias over `new_session()` and `session_from_state(...)`.                 |
| Should Python expose raw `AgentContext`?            | Not as a mutable live object. Expose typed facades and raw state snapshots.                       |
| Should Python support custom model adapters?        | Later. Current priority is tools, sessions, streams, output, and active control.                  |
| Should public docs expose provisional control APIs? | Stable docs cover implemented control APIs; provisional hook surfaces stay in specs until tested. |
