# Roadmap And Validation

This spec defines the implementation phases and the gates that must hold before
`starweaver-py` is treated as application-ready.

## Phase 0: Architecture Decision

Status: these specs.

Deliverables:

- confirm in-process binding boundary
- confirm crate/package names
- confirm Python import name
- choose P0 model/test strategy
- choose async bridge approach
- confirm Claw's first integration scenario

Validation:

- spec review
- no code changes required

## Phase 1: Minimal In-Process Agent

Deliverables:

- `crates/starweaver-py` workspace member
- maturin project setup
- `starweaver` Python package
- `TestModel` and `FunctionModel` bindings
- `create_agent(...)`
- `Agent.run(...)`
- `AgentSession.run(...)`
- `PythonTool` with Pydantic schema extraction
- `ToolResult` conversion
- Python exception to `ToolError` mapping
- reviewed lint boundary for PyO3/FFI requirements
- Rust and Python tests for one tool call and one multi-turn session

Validation:

- `cargo check -p starweaver-py --locked`
- `cargo test -p starweaver-py --locked`
- `uv run pytest crates/starweaver-py/tests`

Exit criteria:

- Python code can create an agent and call one Python tool in process.
- No P0 agent/tool/session path shells out to Starweaver binaries.
- No P0 Python tool path uses MCP.
- Deterministic tests pass without provider credentials.

## Phase 2: Streaming, State, And HITL

Deliverables:

- `run_stream` async iterator
- typed `StreamEvent` classes
- stream interruption
- `export_state` and `session_from_state`
- approval-required exception and resume flow
- deferred-call exception and resume flow
- cancellation propagation into Python tools
- traceback capture and redaction policy

Validation:

- Python integration tests for stream final result, interruption, approval
  resume, deferred resume, and state restore
- Rust tests for dispatcher cancellation and error mapping

Exit criteria:

- A Python stream can be interrupted and the running Python tool is cancelled.
- Approval/deferred records are resumed through Starweaver session APIs.
- State restored from Python can continue a multi-turn session.

## Phase 3: SDK Parity Surface

Deliverables:

- Python `Toolset`
- toolset lifecycle
- per-run `RunOptions`
- subagent registration and delegation
- skill registry helpers
- environment provider wrappers
- output validators and Pydantic output
- model registry/profile helpers
- usage and trace access

Validation:

- integration tests for toolsets, subagents, skills, environment-backed tools,
  output validation retry, and usage snapshots
- docs examples compile or run through Python test snippets

Exit criteria:

- Python can compose agents with toolsets, subagents, skills, and environment
  providers through Starweaver-owned contracts.
- Python output validation participates in the Starweaver retry loop.
- Usage and trace evidence can be read from Python results and stream events.

## Phase 4: Claw Product Readiness

Deliverables:

- Claw-owned Python resource/tool integration example
- store-backed session and stream archive integration where needed
- packaging matrix for supported Python and platform versions
- CI wheel build
- public docs under `docs/` after API stabilizes
- migration guide from ad hoc Python orchestration to `starweaver-py`

Validation:

- Claw integration test or example app
- wheel smoke tests on supported platforms
- full workspace validation before release

Exit criteria:

- Claw can build its agent runtime path on the Python library.
- The Python package can be installed and smoke tested from CI wheels.
- Public docs describe a stable API, not a provisional binding experiment.

## Acceptance Gates

Before `starweaver-py` is considered ready for application use:

01. Python tools execute in process as Rust `Tool` implementations.
02. No P0 agent/tool/session path shells out to `sw`, `starweaver-cli`, or
    `starweaver-rpc`.
03. No P0 Python tool injection path uses MCP.
04. Tool schema, result, retry, approval, deferred, cancellation, and timeout
    behavior round trip through Starweaver's native `Tool` and `ToolError`
    contracts.
05. `AgentSession` state export and restore work from Python.
06. Streaming yields typed Python events backed by Starweaver stream records.
07. Python callback dispatch does not hold the GIL across Rust runtime awaits.
08. Cancellation propagates from Starweaver stream/session APIs into running
    Python tools.
09. Pydantic schema extraction has positive and negative tests.
10. The package has deterministic tests that do not require live provider
    credentials.
11. Any required FFI or `unsafe` lint exception is scoped to
    `crates/starweaver-py` and documented in that crate.
12. Public docs are added only after the reviewed API shape is stable enough
    for users.

## Open Decisions

| Decision                        | Options                                                               | Recommendation                                                                               |
| ------------------------------- | --------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| Python import name              | `starweaver`, `starweaver_py`, `starweaver_sdk`                       | Use `starweaver` for the import and reserve `starweaver-py` for crate/project naming         |
| PyPI name                       | `starweaver`, `starweaver-py`                                         | Prefer `starweaver` if available; otherwise publish `starweaver-py` with `import starweaver` |
| Python floor                    | 3.10, 3.11, 3.12                                                      | Choose after Claw runtime constraints are confirmed                                          |
| Async bridge                    | `pyo3-async-runtimes`, custom dispatcher, hybrid                      | Hybrid: use official bridge where suitable and keep a Starweaver dispatcher abstraction      |
| P0 model support                | test models only, registry models, direct provider helpers            | Start with test models plus registry-resolved models if the factory boundary is clean        |
| Python model adapters           | P0, P1, never                                                         | Not P0; tool injection is the priority                                                       |
| Default Python tool concurrency | sequential, parallel async                                            | Sequential by default with opt-in after tests                                                |
| State model                     | raw JSON, Pydantic wrapper, both                                      | Both: Rust-owned JSON plus Python validation helpers                                         |
| Claw resource mapping           | tools first, environment provider first, resource registry first      | Tools first, then resource refs, then provider integration                                   |
| FFI lint boundary               | inherit workspace forbid, binding-local exception, separate workspace | Prefer binding-local exception only if PyO3 requires it                                      |

## Review Checklist

- Does the spec preserve Starweaver-native ownership boundaries?
- Does it give Python developers the expected agent/tool/session ergonomics?
- Does Python tool injection stay in process?
- Does the plan avoid MCP/RPC/binary control flow for the core library path?
- Does the async strategy have a testable cancellation and GIL story?
- Does Claw have a plausible first integration path without waiting for every
  future resource/provider feature?
- Are the first implementation phases small enough to validate with
  deterministic tests?
- Is each public Python convenience clearly mapped to a Rust-owned contract?
- Are process-local Python dependencies separated from resumable state?
- Are docs deferred until after API review?
