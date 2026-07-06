# Python Examples

The checked-in examples live under `examples/python/`. They are intentionally
small and composable so product code can lift one feature at a time.

## Basic Tool Agent

Run:

```bash
uv run python examples/python/basic_tool_agent.py
```

The example uses `TestModel` and a Python `@tool`, so it does not call an
external provider.

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

## Compose Runtime Features

Run:

```bash
uv run python examples/python/compose_runtime.py
```

This example combines an environment provider, environment-backed toolsets, a
static toolset, and structured output.

## Claw-Like Runtime Smoke

Run:

```bash
uv run python examples/python/claw_like_runtime.py
```

This deterministic product-style smoke uses a Python `AbstractToolset`,
`AgentSession.run_stream()`, active steering, typed approval resume, native
SQLite session storage, raw stream archive, and replay log. It proves the
library path a Claw-like Python service should use without invoking `sw`,
`starweaver-rpc`, MCP, or an external provider. The repository `make py-lint`
gate type-checks this example, and `make py-wheel-smoke` runs it from an
installed wheel.

## Claw Product Runtime

Run:

```bash
uv run python examples/python/claw_product_runtime.py
```

This example keeps product policy above the SDK. It uses product-owned SQLite
tables for sessions, runs, runtime instances, and notifications, plus a separate
native Starweaver SQLite store for canonical session, stream, archive, and
replay evidence. The smoke covers service startup, profile resolution, queued
input merge, active steering, HITL suspension, restart restore, approval resume,
bridge HITL request/approval projection over canonical approval IDs,
product-owned virtual workspace snapshots, sandbox status in run details,
product API sandbox status and TTL cleanup endpoints, durable async task tools,
background async task execution with parent wake notifications, session trace
tools over canonical replay evidence,
scheduled, heartbeat, and workflow-triggered runs through the same coordinator,
memory extraction and agency-fire runs through product-owned toolsets, a
product API facade with bearer auth and SSE-style replay over stored
notifications/run records, a product dispatcher loop for active schedules and
heartbeat fires, UI replay from stored stream/display records, and
orphan-running startup recovery. The repository `make py-lint` gate type-checks
this example, and `make py-wheel-smoke` runs it from an installed wheel.

## Provider Smoke Test

Run with an explicit model ID:

```bash
STARWEAVER_PY_PROVIDER_MODEL="oauth@codex:gpt-5.5" \
  uv run python examples/python/provider_smoke.py
```

The default is `oauth@codex:gpt-5.5`, matching the local `sw` config profile
used during documentation validation. It uses the Starweaver OAuth store and
does not require an API key. API-key providers can be tested with a Python model
ID such as `openai_responses:gpt-5-mini`.

## Example Selection

Use deterministic examples for package and product tests. Use provider examples
only as smoke tests, because they depend on local auth, provider availability,
quota, model rollout, and network behavior.
