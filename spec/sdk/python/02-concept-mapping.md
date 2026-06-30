# Concept Mapping

This spec maps Python SDK concepts into Starweaver-native Rust seams. The goal
is to make Python application code feel natural without creating a second
runtime contract.

## Current Rust Seams

The binding should use existing Starweaver seams instead of creating a parallel
agent runtime:

| Rust seam                           | Binding use                                                                                              |
| ----------------------------------- | -------------------------------------------------------------------------------------------------------- |
| `AgentBuilder`                      | Python `create_agent(...)`, `Agent`, `AgentApp` construction                                             |
| `AgentSession`                      | Python multi-turn sessions, run options, HITL resume, state export                                       |
| `AgentRunOptions`                   | Per-run Python overrides for instructions, model settings, request params, tools, and toolsets           |
| `AgentContext`                      | Python context facade, metadata/state/notes/messages/dependencies access                                 |
| `ResumableState`                    | JSON-serializable Python state export and restore                                                        |
| `Tool`                              | `PythonTool` adapter around Python callables                                                             |
| `FunctionTool`                      | Rust reference implementation for callable-backed tools                                                  |
| `ToolContext`                       | Python `ToolContext` facade passed into Python tools                                                     |
| `ToolResult`                        | Python `ToolResult` return type and conversion target                                                    |
| `ToolError`                         | Python exception mapping for retry, approval, deferred, validation, execution, cancellation, and timeout |
| `Toolset` and `StaticToolset`       | Python `Toolset` and grouped tools with instructions and lifecycle                                       |
| approval/deferred metadata          | Python HITL and deferred tool flow                                                                       |
| `AgentStreamHandle`                 | Python async iterator and interruption surface                                                           |
| `SubagentSpec` and SDK registry     | Python subagent config and delegation                                                                    |
| `AgentSpec` and `AgentSpecRegistry` | Serializable Python agent profiles and registry-backed construction                                      |
| `EnvironmentProvider`               | Python-visible local/virtual/sandbox/envd providers and future Python provider adapters                  |
| `ModelAdapter`                      | Starweaver-native model handles; optional future Python model adapter                                    |
| usage ledger and trace context      | Python usage snapshots and observability hooks                                                           |

## Python Concept Map

The Python SDK should map the major `ya-agent-sdk` style concepts to
Starweaver-native contracts as follows.

| Python concept      | Rust owner                                 | P0 shape                                                                                                   | Later shape                                                                                      |
| ------------------- | ------------------------------------------ | ---------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `create_agent(...)` | `starweaver-agent::AgentBuilder`           | Build an `Agent` from model, instructions, tools, toolsets, output policy, and capabilities                | Registry-backed `AgentSpec`, presets, environment and durability profiles                        |
| `Agent`             | `AgentApp` plus `RuntimeAgent`             | Async context manager with `run`, `run_stream`, and `new_session`                                          | Application-level lifecycle hooks and resource management                                        |
| `AgentSession`      | `starweaver-agent::AgentSession`           | Multi-turn stateful session with `run`, `run_stream`, `export_state`, and `from_state`                     | Store-backed sessions, replay cursors, stream archives                                           |
| `RunOptions`        | `AgentRunOptions`                          | Per-run instructions, request params, tools, toolsets, and replace-tools flag                              | Typed model/output/approval/trace overrides                                                      |
| `AgentContext`      | `starweaver-context::AgentContext`         | Read/write facade for state, metadata, notes, message bus, and dependencies                                | Capability-aware facade with usage, tool search, environment, and resource views                 |
| `ResumableState`    | `starweaver-context::ResumableState`       | Python JSON object and Pydantic model wrapper                                                              | Full/curated export modes and versioned migrations                                               |
| `@tool`             | `starweaver-tools::Tool`                   | Decorator returning `PythonTool` with Pydantic or explicit JSON schema                                     | Availability, prepare-definition, retry, timeout, return schema, strict mode, and per-tool hooks |
| `BaseTool`          | `Tool`                                     | Subclassable Python base class with `name`, `schema`, and `call`                                           | Full lifecycle and cancellation hooks                                                            |
| `Toolset`           | `Toolset`                                  | Grouped Python tools plus instructions                                                                     | Async enter/exit/prepare lifecycle and dynamic discovery                                         |
| `ToolContext`       | `ToolContext`                              | Python object exposing run ids, retry, metadata, approval/deferred handles, cancellation, and dependencies | Controlled `AgentContext` handle and environment/resource helpers                                |
| `ToolResult`        | `ToolResult`                               | JSON content plus optional metadata, model content, user content, and private metadata                     | Binary/resource return helpers and display content helpers                                       |
| tool exceptions     | `ToolError`                                | Python exceptions mapped into Starweaver control flow                                                      | Rich Python tracebacks in private metadata and policy-aware redaction                            |
| `stream_agent(...)` | `AgentStreamHandle`                        | `async for event in agent.run_stream(...)`                                                                 | replay cursor, backpressure options, event filters, and child stream attribution                 |
| HITL approval       | approval metadata and `AgentHitlResults`   | Raise `ApprovalRequired` from Python tools and resume with decisions                                       | Durable approval records and Claw approval UI integration                                        |
| deferred tools      | deferred metadata and `AgentHitlResults`   | Raise `CallDeferred` and resume with deferred result                                                       | Store-backed deferred jobs and service orchestration                                             |
| subagents           | `SubagentSpec`, registry, delegation tools | Python `SubagentConfig` mapped into SDK registry                                                           | Nested sessions, stream evidence, inherited tools, worker mode                                   |
| skills              | skill registry and skill toolsets          | List/load Starweaver skills from provider-visible paths                                                    | Hot reload and remote registry sync                                                              |
| environment         | `EnvironmentProvider`                      | Expose local/virtual provider handles and environment-backed bundles                                       | Python-defined environment providers                                                             |
| resources           | environment resource references            | Use resource refs as model/tool content and environment handles                                            | Resumable resource registry and Python resource providers                                        |
| message bus         | `AgentContext` message bus                 | Python send/consume helpers                                                                                | Idempotent targeted/broadcast messages with multimodal payloads                                  |
| models              | `ModelAdapter` and model presets           | Use Starweaver model handles or registry-resolved model specs                                              | Python-defined `ModelAdapter` only after runtime requirements are proven                         |
| observability       | trace context and trace recorders          | Expose trace ids, run ids, usage snapshots                                                                 | Python logging bridge and OTel exporter options                                                  |

## Python API Sketch

P0 should support this application shape:

```python
from pydantic import BaseModel, Field

from starweaver import ToolContext, ToolResult, create_agent, tool


class LookupArgs(BaseModel):
    query: str = Field(description="Search query submitted by the model")


@tool(name="lookup", description="Lookup an internal value")
async def lookup(ctx: ToolContext, args: LookupArgs) -> ToolResult:
    return ToolResult({"value": args.query, "run_id": ctx.run_id})


async def main() -> None:
    async with create_agent(
        model="test:text:ready",
        instructions=["Answer with short responses."],
        tools=[lookup],
    ) as agent:
        result = await agent.run("Say ready")
        assert result.output == "ready"
```

Streaming should feel like normal Python async iteration:

```python
async for event in agent.run_stream("Research this topic"):
    if event.kind == "message_delta":
        print(event.text, end="")
```

Sessions and state should be explicit:

```python
session = agent.new_session()
await session.run("Remember that the project is Starweaver")

state = session.export_state(mode="curated")
restored = agent.session_from_state(state)
result = await restored.run("What project did I mention?")
```

HITL should preserve Starweaver control flow:

```python
from starweaver import ApprovalRequired


@tool(name="deploy")
async def deploy(ctx: ToolContext, args: DeployArgs) -> dict:
    raise ApprovalRequired(
        reason="Deployment changes production state",
        metadata={"service": args.service},
    )


run = await session.run("Deploy api")
if run.needs_approval:
    resumed = await session.resume_after_hitl(
        approvals={run.pending_approvals[0].id: {"approved": True}}
    )
```

## Mapping Principles

- Python convenience should be a facade over Starweaver-owned contracts.
- Python decorators should produce explicit native handles, not hidden global
  runtime state.
- Pydantic should help build schemas, but Rust should own validation inside the
  agent loop.
- Python exceptions should map into public Starweaver control flow, not opaque
  foreign errors.
- Raw Starweaver records should remain accessible for forward compatibility.
- Python APIs should prefer explicit sessions over hidden global conversation
  state.

## P0 Minimum Concept Set

The first implementation should only claim Python SDK viability when these
concepts work together:

- `create_agent(...)`
- deterministic test model
- `@tool`
- `ToolContext`
- `ToolResult`
- `Agent.run(...)`
- `AgentSession.run(...)`
- `AgentSession.export_state(...)`
- Python exception mapping for ordinary execution errors

Streaming, HITL, deferred tools, subagents, and environment providers can land
after the minimal in-process agent is stable, but their API names should be
reserved early enough to avoid churn.
