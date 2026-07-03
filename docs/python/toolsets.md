# Python Toolsets

Toolsets group tools and tool instructions so applications can move a
capability as one unit. Python toolsets wrap the native Starweaver toolset
contract; they do not replace Rust tool lifecycle, retry policy, or stream
evidence.

## Static Toolsets

Use `Toolset` when a group of tools and instructions should be attached
together:

```python
from starweaver import Toolset, create_agent, tool
from starweaver.testing import TestModel


@tool
async def lookup(value: str) -> dict[str, str]:
    return {"value": value}


workspace = Toolset(
    "workspace",
    tools=[lookup],
    instructions=["Use workspace tools when the user asks for local facts."],
)
agent = create_agent(model=TestModel.text("ready"), toolsets=[workspace])
```

Toolsets can be attached at agent construction or per run:

```python
result = await agent.run("lookup x", toolsets=[workspace])
```

## Tool Libraries

`ToolLibrary` is a serializable collection of toolsets used by dynamic search
and proxy facades:

```python
from starweaver import ToolLibrary


library = ToolLibrary([workspace])
definitions = library.tool_definitions()
assert definitions
```

## Tool Search

`ToolSearchToolset` exposes the native dynamic tool-search tool. The model can
search a hidden library before deciding which concrete tools to call.

```python
from starweaver import ToolSearchToolset


agent = create_agent(
    model=TestModel.text("ready"),
    toolsets=[ToolSearchToolset([workspace], max_results=5)],
)
```

Use it when the model should choose from a large tool library without exposing
every tool in the initial request.

## Tool Proxy

`ToolProxyToolset` exposes a fixed search/call proxy over hidden toolsets:

```python
from starweaver import ToolProxyToolset


agent = create_agent(
    model=TestModel.text("ready"),
    toolsets=[ToolProxyToolset([workspace], prefix="workspace", max_results=5)],
)
```

Use it when the model should route through a stable proxy tool instead of
receiving the underlying tools directly.

## Environment Toolsets

First-party environment tools are ordinary toolsets backed by an attached
`EnvironmentProvider`:

```python
from starweaver import EnvironmentProvider, create_agent, environment_toolsets
from starweaver.testing import TestModel


environment = EnvironmentProvider.virtual(files={"README.md": "hello"})
agent = create_agent(
    model=TestModel.text("ready"),
    environment=environment,
    toolsets=environment_toolsets(),
)
```

Use `filesystem_toolset()` or `shell_toolset()` when only one environment
surface should be exposed.

## Capability Bundles

`CapabilityBundle` packages static composition that is broader than tools:
instructions, Python tools, model settings, request params, output validators,
and output functions.

```python
from starweaver import CapabilityBundle, create_agent, tool
from starweaver.testing import TestModel


@tool
async def audit(value: str) -> dict[str, str]:
    return {"value": value}


bundle = CapabilityBundle(
    "audit-bundle",
    instructions=["Prefer concise audit notes."],
    tools=[audit],
)
agent = create_agent(model=TestModel.text("ready"), capability_bundles=[bundle])
```

The Python capability API is currently bundle-oriented. Hook-level capability
callbacks should use a typed Python hook contract before becoming public API.
Raw Rust `AgentCapability` callbacks remain a Rust-side extension point.
