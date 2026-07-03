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
