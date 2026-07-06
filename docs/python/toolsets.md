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

## Function Toolsets

Use `FunctionToolset` when a tool family should be authored with decorators
and shared defaults:

```python
from starweaver import FunctionToolset, ToolContext, create_agent
from starweaver.testing import TestModel


functions = FunctionToolset(
    "functions",
    id="functions",
    instructions=["Use function tools when requested."],
    max_retries=2,
    timeout_ms=30_000,
    metadata={"source": "functions"},
)


@functions.tool
async def lookup(ctx: ToolContext, value: str) -> dict[str, str]:
    return {"value": value, "run_id": ctx.run_id}


@functions.tool_plain(name="mode")
def current_mode() -> str:
    return "review"


agent = create_agent(model=TestModel.text("ready"), toolsets=[functions])
```

`tool()` accepts a `ToolContext` parameter. `tool_plain()` rejects context
parameters and is useful for deterministic helpers. Toolset-level metadata,
retry, timeout, strictness, and sequential defaults apply to functions added
through the toolset unless the individual tool overrides them. When a tool or
toolset enables `strict=True`, each registered tool must provide an explicit
description or a docstring.

## Dynamic Toolsets

Use `AbstractToolset` when the visible tools or instructions depend on the
current agent context. `PythonDynamicToolset` is available as an explicit
compatibility base with the same Python contract. In both cases the Python object
is adapted into a native Starweaver toolset, so the Rust runtime still owns
preparation, lifecycle events, tool execution, retry, cancellation, and stream
evidence.

```python
from starweaver import (
    AbstractToolset,
    ToolsetContext,
    ToolsetLifecyclePolicy,
    ToolsetPreparation,
    create_agent,
    tool,
)
from starweaver.testing import TestModel


@tool
async def lookup(value: str) -> dict[str, str]:
    return {"value": value}


class WorkspaceToolset(AbstractToolset):
    name = "workspace"
    id = "workspace"

    async def enter(self, ctx: ToolsetContext) -> None:
        self.run_id = ctx.run_id

    async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation:
        return ToolsetPreparation(
            tools=[lookup],
            instructions=[f"Use workspace tools for run {ctx.run_id}."],
        )

    async def exit(self, ctx: ToolsetContext) -> None:
        self.run_id = None


agent = create_agent(
    model=TestModel.text("ready"),
    toolsets=[
        WorkspaceToolset(
            lifecycle_policy=ToolsetLifecyclePolicy(
                read_timeout_ms=5_000,
                exit_timeout_ms=2_000,
            )
        )
    ],
)
```

Subclass `PythonDynamicToolset` instead when product code should carry the
explicit dynamic-toolset name; it uses the same Python contract.

`prepare()` may return a `ToolsetPreparation` or another native `Toolset`.
`get_tools()` and `get_instructions()` can be overridden instead of `prepare()`
for simple dynamic inventory. Toolsets used by durable products should provide
a stable explicit `id`.

`ToolsetContext` is a read-only projection built by the Rust runtime at
preparation time. It exposes `agent_id`, `run_id`, `session_id`,
`conversation_id`, `run_step`, `metadata`, `workspace_root`, `environment`,
`resources`, and `raw_context`. The environment value is an existing
`EnvironmentProvider` facade, and resources are exposed through
`ResourceRegistry`; Python code does not receive a mutable `AgentContext`.

`refresh()` is optional. The default implementation calls `prepare()`. When a
context-aware toolset is prepared again for the same run, such as during HITL
approval resume, Starweaver calls `refresh()` and emits a refreshed lifecycle
report.

Use `validate_toolsets_for_durability()` before storing or restoring durable
product profiles. The check reads Python and native toolset identities without
materializing dynamic toolsets:

```python
from starweaver import validate_toolsets_for_durability


validation = validate_toolsets_for_durability([WorkspaceToolset()])
validation.require_ids().require_serializable_dynamic_state()
```

`validate_toolset_ids()` is the lower-level identity check when a product wants
to collect warnings without immediately requiring every toolset to have a
durable id.

`SessionArchive.from_session(...)` records required toolset IDs for the current
agent profile. `agent.session_from_archive(...)` compares those IDs with the
currently registered Python toolsets and fails if a required ID is missing.
Python callable objects are not serialized; products must re-register the
current toolset objects during startup. The restored run then uses the current
profile approval policy and current environment provider bindings.

`ToolsetLifecyclePolicy` maps directly to the Rust lifecycle policy. It controls
initialization/read/exit timeouts, whether `enter()` runs before preparation,
whether `exit()` runs after the run, and whether unavailable toolsets fail the
run. `AbstractToolset.with_lifecycle(policy)` returns a native toolset wrapper
with that policy applied:

```python
readonly_workspace = WorkspaceToolset().with_lifecycle(
    ToolsetLifecyclePolicy(
        enter_before_prepare=False,
        exit_after_run=False,
        read_timeout_ms=1_000,
    )
)
```

Lifecycle evidence is emitted as native stream sideband events and can be read
through typed Python reports:

```python
from starweaver import StreamAdapter


stream = agent.run_stream("use workspace")
stream_result = await stream.join()
reports = [
    event.toolset_lifecycle_report
    for event in stream_result.events
    if event.toolset_lifecycle_report is not None
]
assert reports[0].state == "initialized"
assert StreamAdapter(stream_result.events).toolset_lifecycle_reports() == reports
```

## Dynamic Factories

Use `toolset_factory()` when the visible toolset depends on the current run
context:

```python
from starweaver import FunctionToolset, ToolsetContext, toolset_factory


def build_workspace_tools() -> FunctionToolset:
    workspace = FunctionToolset("workspace", id="workspace")

    @workspace.tool_plain
    def read_file(path: str) -> str:
        return path

    return workspace


@toolset_factory(id="workspace")
def workspace_tools(ctx: ToolsetContext) -> FunctionToolset:
    return build_workspace_tools()


agent = create_agent(model=TestModel.text("ready"), toolsets=[workspace_tools])
```

Factories may be passed at agent construction time or per run. They may return
a single `Toolset`, an `AbstractToolset`, a native toolset, a sequence of
toolsets, or `ToolsetPreparation(toolsets=[...])`. `per_run_step=False` caches
the factory selection for one run:

```python
@agent.toolset(id="profile_tools", per_run_step=False)
def profile_tools(ctx: ToolsetContext) -> FunctionToolset:
    return build_workspace_tools()
```

Durable products should give factories stable explicit IDs and validate them
with `validate_toolsets_for_durability()` before storing or restoring profiles.

## Wrapper Methods

`Toolset`, `AbstractToolset`, and `FunctionToolset` expose chainable wrappers
that map to native Rust toolset combinators:

```python
from starweaver import FunctionToolset, ToolsetLifecyclePolicy


workspace = FunctionToolset("workspace", id="workspace")


@workspace.tool_plain
def read_file(path: str) -> str:
    return path


readonly = workspace.filtered(include=["read_file"])
read_predicate = workspace.filtered(predicate=lambda definition: "read" in definition["name"])
prepared = workspace.prepared(lambda ctx, definitions: definitions)
namespaced = workspace.prefixed("workspace")
renamed = workspace.renamed({"read_file": "workspace_read"})
audited = workspace.with_metadata(bundle="workspace", audit="enabled")
reviewed = workspace.approval_required("*", reason="review workspace access")
deferred = workspace.deferred(["read_file"], reason="external worker")
short_lived = workspace.with_lifecycle(
    ToolsetLifecyclePolicy(enter_before_prepare=False, exit_after_run=False)
)
```

Use `create_agent(..., approval_required_tools=[...])` when approval is a
profile-level policy over registered toolsets rather than a wrapper owned by one
toolset definition. The direct builder policy uses the same native matching
rules as `AgentBuilder::approval_required_tools`: tool name, toolset name/id,
metadata bundle, or `*`.

Available wrappers:

- `prefixed(prefix)` exposes each tool as `{prefix}_{name}` and prefixes
  instruction groups.
- `filtered(include=[...])` or `filtered(exclude=[...])` keeps or removes
  static tool names.
- `filtered(predicate=...)` evaluates a Python predicate over prepared tool
  definition dictionaries.
- `renamed({"old": "new"})` exposes selected tools under new names while
  preserving execution through the original callable.
- `with_metadata(...)` merges additional runtime metadata into every exposed
  tool definition.
- `prepared(callback)` lets Python inspect and return the model-facing tool
  definition list for the current `ToolsetContext`. Return `None` to keep the
  prepared definitions unchanged.
- `approval_required(names="*", reason=None)` suspends matching calls for HITL
  approval before execution.
- `deferred(names="*", reason=None)` turns matching calls into native deferred
  work items.
- `with_lifecycle(policy)` applies a Rust `ToolsetLifecyclePolicy` to an
  `AbstractToolset` or `FunctionToolset`.

For dynamic Python toolsets, wrappers are applied after the Rust runtime
prepares the current context inventory. HITL resume also re-prepares
context-aware toolsets before executing an approved call, so custom
`AbstractToolset` subclasses keep their dynamic behavior through approval and
deferred flows.

## Tool Libraries

`ToolLibrary` is a serializable collection of toolsets used by dynamic search
and proxy facades:

```python
from starweaver import ToolLibrary


library = ToolLibrary([workspace])
definitions = library.tool_definitions()
library.validate_ids().raise_for_errors()
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

Loaded tool names and namespace IDs are stored in exported session state as
`tool_search_loaded_tools` and `tool_search_loaded_namespaces`. Search
initialization, successful loads, empty matches, and invalid queries are visible
as stream sideband events such as `tool_search_initialized`,
`tool_search_loaded`, `tool_search_no_match`, and `tool_search_failed`.

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

When a prefix is supplied, the visible proxy tools are scoped as
`{prefix}_search_tool` and `{prefix}_call_tool`. The wrapped tools remain hidden
from the provider tool list; successful search or proxy calls still record the
same loaded tool and namespace IDs in session state.

## MCP Toolsets

`McpToolset` exposes Starweaver's Rust MCP toolset config through typed Python
objects. It is a deferred-call toolset: declared MCP tools appear in the native
tool list, and calls suspend with MCP request metadata for the host product to
complete or route.

```python
from starweaver import McpToolSpec, McpToolset, McpTransport


github = McpToolset(
    "github",
    transport=McpTransport.streamable_http("https://example.com/mcp"),
    headers={"authorization": "Bearer token"},
    tool_prefix="github",
    include_instructions=True,
    instructions="Use GitHub MCP tools for repository tasks.",
    tools=[
        McpToolSpec(
            "search",
            parameters={
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"],
            },
            description="Search repositories.",
        )
    ],
)
```

Use `McpTransport.stdio(...)` for a local child process or
`McpTransport.sse(...)` for older SSE endpoints. Resources, prompts, sampling,
and subscriptions can be declared with the matching `Mcp*Spec` dataclasses.
The Python layer builds typed config only; transport lifecycle, deferred calls,
approval, and replay evidence remain native Starweaver behavior.

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

`CapabilityBundle` packages composition that is broader than tools:
instructions, Python tools, model settings, request params, output validators,
output functions, and narrow hook-level Python capabilities.

```python
from starweaver import CapabilityBundle, PythonCapability, create_agent, tool
from starweaver.testing import TestModel


@tool
async def audit(value: str) -> dict[str, str]:
    return {"value": value}


def mark_run(state: dict[str, object]) -> dict[str, object]:
    metadata = dict(state.get("metadata") or {})
    metadata["audit_bundle"] = True
    return {**state, "metadata": metadata}


bundle = CapabilityBundle(
    "audit-bundle",
    instructions=["Prefer concise audit notes."],
    tools=[audit],
    hooks=[PythonCapability("audit-start", on_run_start=mark_run)],
)
agent = create_agent(model=TestModel.text("ready"), capability_bundles=[bundle])
```

`PythonCapability` currently exposes a typed `on_run_start(state)` callback for
run-start state observation or mutation. Broader provider-message, request,
tool-call, and output mutation hooks remain Rust-side extension points until
they have typed Python contracts.
