# Product Boundary And Package Shape

`starweaver-py` is the planned in-process Python SDK and binding layer for
Starweaver. It should provide Python-native ergonomics while delegating agent
execution, tool-loop semantics, state, model protocol, streaming, usage, and
environment contracts to existing Starweaver Rust crates.

## Why This Exists

Claw should be able to depend on a Python library, define Python tools and
resources, and run Starweaver agents without standing up an MCP server or a
Starweaver host process. That requires a real binding layer:

- Python code owns application orchestration and product resources.
- Rust code owns the Starweaver runtime and durable evidence.
- Python tools are registered as native Starweaver tools in the same process.

This is different from host-control integration. JSON-RPC and MCP remain useful
for process boundaries, hosts, and external tools, but they should not be the
core library path for Claw's Python integration.

## Goals

- Provide a first-class Python package for Starweaver agents.
- Keep core execution in process with the Rust runtime.
- Let Python tools be injected as real Starweaver `Tool` implementations.
- Map the important SDK concepts into Python: agents, sessions, tools,
  toolsets, context, resumable state, streaming, HITL, subagents, skills,
  environments, resources, models, usage, and observability.
- Give Claw a Python-native integration surface above Starweaver without
  forcing Claw through MCP or a sidecar binary.
- Keep Rust crates Python-free except for the dedicated binding crate.
- Keep the Python surface ergonomic while the owned contracts remain
  Starweaver-native.

## Non-Goals

- Do not wrap `sw`, `starweaver-cli`, or `starweaver-rpc` as the core Python
  execution path.
- Do not use MCP as the Python tool injection mechanism.
- Do not mirror every Rust API one for one.
- Do not make `starweaver-core`, `starweaver-runtime`, `starweaver-agent`,
  `starweaver-tools`, `starweaver-context`, or `starweaver-model` depend on
  Python.
- Do not replace the Rust SDK or CLI product surface.
- Do not move provider-specific routing headers into generic Python metadata.
  Provider affinity remains typed Starweaver model/provider settings.
- Do not make Claw-specific product policy part of the binding crate. Claw can
  layer on top of `starweaver-py`.

## Binding Ownership

`starweaver-py` owns Python names, decorators, async iterators, dataclasses,
Pydantic helpers, conversion code, and Python callback dispatch. It does not
own the agent loop, provider protocol, tool loop, context state format,
environment contracts, stream archive protocol, or durable session semantics.

The Python API may intentionally look more decorator-oriented than the Rust SDK.
That is acceptable because the convenience belongs to the Python product layer.
The Rust SDK keeps explicit builders, typed context setters, `AgentSession`,
and Starweaver-native type names.

## Candidate Package Layout

The repository owns the Python distribution under `packages/` so future
language packages can share the same package domain:

```text
packages/starweaver-py/
  Cargo.toml
  pyproject.toml
  src/lib.rs
  src/agent.rs
  src/error.rs
  src/model.rs
  src/session.rs
  src/state.rs
  src/stream.rs
  src/tool.rs
  src/toolset.rs
  python/starweaver/__init__.py
  python/starweaver/agent.py
  python/starweaver/errors.py
  python/starweaver/model.py
  python/starweaver/session.py
  python/starweaver/state.py
  python/starweaver/stream.py
  python/starweaver/testing.py
  python/starweaver/tool.py
  python/starweaver/toolset.py
  tests/
```

Rust crate name: `starweaver-py`.

Python import name: `starweaver`.

Native extension module: `starweaver._native`.

## Packaging Baseline

- Use PyO3 for Rust/Python bindings.
- Use maturin mixed Rust/Python project layout for wheel builds.
- Evaluate `pyo3-async-runtimes` for Rust future to Python awaitable bridging,
  but keep the callback dispatcher design explicit before committing to a
  specific bridge.
- Keep pure Python ergonomics in `python/starweaver/*`; keep Rust object
  ownership and conversion code in `src/*`.
- Treat any FFI or `unsafe` lint exception as binding-crate local. Core
  Starweaver crates should keep the workspace `unsafe_code = "forbid"` rule.
- Keep `packages/starweaver-py` excluded from the Rust workspace until the
  PyO3/FFI and wheel CI boundary is stable; validate it through `uv`, maturin,
  and Python package CI.

External implementation references:

- [PyO3 user guide](https://pyo3.rs/)
- [maturin user guide](https://www.maturin.rs/)
- [pyo3-async-runtimes docs](https://docs.rs/pyo3-async-runtimes/latest/pyo3_async_runtimes/)

## Workspace Boundary

The binding crate can depend on:

- `starweaver-agent`
- `starweaver-context`
- `starweaver-core`
- `starweaver-environment`
- `starweaver-model`
- `starweaver-runtime`
- `starweaver-session`
- `starweaver-stream`
- `starweaver-tools`
- `starweaver-usage`

The core crates should not depend on `starweaver-py`. If core behavior must be
changed for Python, the change should first be expressed as a neutral Rust
contract that is useful without Python.

## Product Boundary Questions

- Should the PyPI distribution be named `starweaver` or `starweaver-py`?
- Which Python version floor should Claw require?
- Should the binding crate join the Rust workspace after the PyO3/FFI and wheel
  CI boundary is stable?
- Does PyO3 require a binding-local lint exception for generated FFI glue?
- Which provider/model factory should be exposed to Python first?
