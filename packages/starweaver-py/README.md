# Starweaver Python

`starweaver` is the in-process Python SDK and binding package for Starweaver.
It keeps the Rust runtime as the canonical agent loop while exposing Python
ergonomics for agents, tools, models, sessions, streams, resources, media, and
deterministic tests.

Python tools are injected directly as native Starweaver runtime tools. They do
not use MCP, stdio, or another binary protocol.

Supported Python versions are CPython 3.11 through 3.13. Local development and
single-version CI jobs default to Python 3.13.

## Quickstart

```python
import asyncio

from starweaver import create_agent, tool
from starweaver.testing import TestModel


@tool
async def add(left: int, right: int) -> dict[str, int]:
    return {"total": left + right}


async def main() -> None:
    model = TestModel.responses(
        [
            TestModel.tool_call_response(
                [{"id": "call_add", "name": "add", "arguments": {"left": 2, "right": 3}}]
            ),
            {"text": "done"},
        ]
    )
    result = await create_agent(model=model, tools=[add]).run("Add two numbers")
    print(result.output)


asyncio.run(main())
```

## Core Surfaces

- `create_agent()` builds a Python facade over the Rust runtime.
- `@tool`, raw callables, and `BaseTool` register in-process tools.
- Independent tool calls run in parallel by default; use `sequential=True` for
  ordered side effects.
- `Toolset`, `ToolSearchToolset`, and `ToolProxyToolset` compose grouped tools.
- `ProviderModel`, `TestModel`, and `FunctionModel` cover production providers,
  deterministic scripts, and callback-backed tests.
- `OutputSchema`, `OutputPolicy`, validators, and output functions handle final
  output.
- `AgentSession` and `SessionArchive` preserve reusable session state.
- `EnvironmentProvider`, `ResourceRef`, `SkillRegistry`, and `MediaUploader`
  expose environment, resource, skill, and media boundaries.

## Stream Boundary

`run_stream()` currently returns `AgentRun`, an async iterator over canonical
`StreamEvent` records plus a Python facade for `recv()`, `join()`, `result()`,
`status()`, `recoverable_state()`, `interrupt()`, active messages, steering, and
streamed HITL helpers. The stable portable contract is the canonical
`StreamEvent.raw` record sequence and collected run result. Python live-control
ergonomics are the current facade, not a separately frozen live-handle
protocol.

`AgentStream` remains a compatibility alias for `AgentRun`.

## Documentation

Start with `docs/python-sdk.md` in the repository. The Python docs are organized
by feature:

- `docs/python/agents.md`
- `docs/python/tools.md`
- `docs/python/toolsets.md`
- `docs/python/models.md`
- `docs/python/output.md`
- `docs/python/sessions-streams.md`
- `docs/python/environments-skills.md`
- `docs/python/media.md`
- `docs/python/testing.md`
- `docs/python/examples.md`
- `docs/python/stability.md`

Runnable examples live in `examples/python/`.

## Validation

From the repository root:

```bash
make py-sync
make py-lint
make py-rust-check
make py-test
make py-wheel-smoke
make py-check
```
