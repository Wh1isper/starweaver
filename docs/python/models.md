# Python Models

Python model objects adapt deterministic test models, callback-backed models,
and production provider adapters into the Rust runtime.

## Deterministic Models

`TestModel` returns scripted responses without calling an external provider:

```python
from starweaver import create_agent
from starweaver.testing import TestModel


async def run_test_model() -> None:
    result = await create_agent(model=TestModel.text("ready")).run("status")
    assert result.output == "ready"
```

Use response scripts for tool loops:

```python
from starweaver import create_agent, tool
from starweaver.testing import TestModel


@tool
async def echo(value: str) -> dict[str, str]:
    return {"value": value}


async def run_tool_loop() -> None:
    model = TestModel.responses(
        [
            TestModel.tool_call_response(
                [{"id": "call_echo", "name": "echo", "arguments": {"value": "hi"}}]
            ),
            {"text": "done"},
        ]
    )
    result = await create_agent(model=model, tools=[echo]).run("use echo")
    assert result.output == "done"
```

## FunctionModel

`FunctionModel` is a deterministic model backed by a Python callback. The
callback receives canonical message history plus an info object containing
prepared request params, merged model settings, and request context.

```python
from starweaver import create_agent, tool
from starweaver.testing import FunctionModel


@tool
async def echo(value: str) -> dict[str, str]:
    return {"value": value}


def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
    params = info["params"]
    assert isinstance(params, dict)
    if len(messages) == 1:
        return {"tool_calls": [{"id": "call_echo", "name": "echo", "arguments": {"value": "hi"}}]}
    return {"text": "done"}


async def run_function_model() -> None:
    result = await create_agent(model=FunctionModel(respond), tools=[echo]).run("use echo")
    assert result.output == "done"
```

## Provider Models

`ProviderModel` creates production model adapters using the Rust provider
transport and profile code. API-key providers can read the default environment
variable or receive an explicit key.

```python
from starweaver import ModelSettings, ProviderModel, create_agent


async def run_provider() -> None:
    model = ProviderModel.openai(
        "gpt-5-mini",
        model_settings=ModelSettings(timeout_ms=30_000),
    )
    result = await create_agent(model=model).run("Write one sentence.")
    print(result.output)
```

`ProviderModel.openai(...)` defaults to the OpenAI Responses protocol. Pass
`protocol="chat"` for the OpenAI Chat adapter, or call
`openai_responses(...)` and `openai_chat(...)` directly when a product wants an
explicit protocol in its profile builder.

`ProviderModel.from_model_id(...)` accepts Python package model IDs:

- `openai:gpt-5-mini`
- `openai_responses:gpt-5-mini`
- `openai_chat:gpt-5-mini`
- `anthropic:claude-sonnet-4-5`
- `gemini:gemini-3.5-flash`
- `oauth@codex:gpt-5.5`

The Python package currently does not resolve CLI gateway profile IDs such as
`homelab@openai-responses-ws:gpt-5.5`. Use `base_url`, `endpoint_path`,
`api_key_env`, and typed provider helpers directly until CLI profile resolution
is exposed to Python.

## Provider Auth And Codex OAuth

`ProviderModel.codex_oauth(...)` uses the Starweaver OAuth store and does not
require an API key:

```python
from starweaver import ModelSettings, ProviderAuth, ProviderModel, create_agent


async def run_codex_oauth() -> None:
    auth = ProviderAuth.codex_oauth()
    status = auth.status()
    if status["logged_in"]:
        account = auth.account_metadata()
        assert account is None or "email" in account

    model = ProviderModel.codex_oauth(
        "gpt-5.5",
        auth=auth,
        model_settings=ModelSettings.preset("openai_responses_high_fast"),
    )
    result = await create_agent(model=model).run("Reply with one word.")
    print(result.output)
```

`ProviderAuth.status()` returns a safe auth snapshot for product diagnostics.
For OAuth-backed Codex auth it includes account metadata, token presence
booleans, the auth file path, and the last successful refresh timestamp, but it
does not expose token material. Use `ProviderAuth.redacted_record()` only for
diagnostics that need the full provider record shape with token fields replaced
by `"<redacted>"`.

Pass `auth_file=...` to `ProviderAuth.codex_oauth(...)` or
`ProviderModel.codex_oauth(...)` when a service owns an explicit Starweaver
OAuth store path.

Codex routing helpers accept typed provider settings such as `session_id` and
`thread_id`. OpenAI Responses helpers accept `stream_transport` values
`"http"`, `"websocket"`, or `"auto"`.

Generic request metadata and trace metadata remain audit context. They do not
become provider routing headers; use typed provider settings for routing
affinity.

## Model Settings

`ModelSettings` carries provider-neutral settings plus typed provider escape
hatches:

```python
from starweaver import ModelSettings


settings = ModelSettings(
    temperature=0.2,
    timeout_ms=30_000,
    provider_settings={
        "openai_responses": {"stream_transport": "auto"},
    },
)
```

Use `ModelSettings.preset(...)` for Rust model settings presets:

```python
settings = ModelSettings.preset("openai_responses_high")
```

## Request Params

`RequestParams` forwards request-level data into model preparation:

```python
from starweaver import RequestParams


params = RequestParams(
    metadata={"purpose": "smoke-test"},
    extra_body={"reasoning": {"effort": "low"}},
)
```

Attach settings and params at agent construction or per run. Per-run values
override the agent defaults only for that invocation.
