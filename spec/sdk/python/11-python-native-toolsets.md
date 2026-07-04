# Python Native Toolsets

This spec defines a more Pythonic toolset construction layer for
`starweaver-py`. The goal is to provide a Pydantic AI-like authoring
experience while preserving Starweaver's Rust-native tool execution,
toolset lifecycle, evidence, approval, deferred, retry, and stream contracts.

## Design Goal

Python authors should be able to build a toolset as a normal Python object.
The primary contract is an `AbstractToolset`-style interface that can expose a
context-aware and lifecycle-aware dynamic inventory:

```python
from starweaver import AbstractToolset, ToolsetContext, ToolsetPreparation, tool


class WorkspaceToolset(AbstractToolset):
    name = "workspace"
    id = "workspace"

    async def enter(self, ctx: ToolsetContext) -> None:
        self.root = ctx.workspace_root

    async def get_tools(self, ctx: ToolsetContext):
        return [read_file, write_file] if not ctx.metadata.get("readonly") else [read_file]

    async def get_instructions(self, ctx: ToolsetContext):
        return [f"Workspace root: {self.root}"]

    async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation:
        return ToolsetPreparation(
            tools=await self.get_tools(ctx),
            instructions=await self.get_instructions(ctx),
        )
```

`FunctionToolset` is the convenience implementation for common local functions:

```python
from starweaver import FunctionToolset, ToolContext, create_agent


workspace = FunctionToolset(
    "workspace",
    id="workspace",
    instructions="Use workspace paths exactly as provided.",
    max_retries=2,
)


@workspace.tool
async def read_file(ctx: ToolContext, path: str) -> str:
    """Read a UTF-8 file from the attached workspace."""
    return await ctx.environment.read_text(path)


@workspace.tool_plain
def current_mode() -> str:
    """Return the current workspace mode."""
    return "review"


agent = create_agent(model=model, toolsets=[workspace])
```

The user experience should feel native to Python. The runtime behavior should
remain native to Starweaver:

- tools become `starweaver-tools::Tool` values;
- toolsets become `starweaver-tools::Toolset` values;
- the Rust registry prepares inventories;
- the Rust runtime schedules, retries, approves, defers, and records tool calls;
- Python callback execution uses the existing PyO3 callback path;
- raw Starweaver evidence remains available.

The dynamic Python object is not a second runtime. It is adapted into a native
`DynToolset` by `PythonDynamicToolset`, and the Rust registry remains
responsible for when preparation, lifecycle, conflict detection, approval,
deferred control flow, retry, cancellation, and evidence happen.

## Reference Lessons From Pydantic AI

Pydantic AI's toolset model is a useful API reference:

- `AbstractToolset` is the base interface for custom toolset classes;
- `FunctionToolset` groups local functions;
- `@toolset.tool` registers context-aware tools;
- `@toolset.tool_plain` registers context-free tools;
- toolsets can provide instructions;
- toolsets can be attached at agent construction and at run time;
- dynamic toolset factories can derive toolsets from run context;
- wrappers can prefix, filter, rename, prepare, require approval, defer loading,
  include return schemas, and attach metadata;
- durable execution requires stable toolset IDs;
- wrapper hierarchies can be visited and replaced.

Starweaver should adopt the ergonomic ideas, not the runtime architecture.
Starweaver already has Rust-owned toolsets, wrappers, lifecycle reports,
approval/deferred control flow, dynamic search, proxy, and stream evidence.
The Python API should expose those contracts rather than building a second
Python middleware stack.

## Current Starweaver Baseline

Current `starweaver-py` exposes:

- `@tool` and `Tool`;
- `BaseTool`;
- `ToolContext` and `ToolResult`;
- static `Toolset`;
- `ToolLibrary`;
- `ToolSearchToolset`;
- `ToolProxyToolset`;
- `filesystem_toolset()`;
- `shell_toolset()`;
- `environment_toolsets()`;
- per-agent and per-run `toolsets=[...]`.

Current Rust tools already provide:

- `StaticToolset`;
- `DynamicToolset`;
- `LazyToolset`;
- `PreparedToolset`;
- `FilteredToolset`;
- `RenamedToolset`;
- `PrefixedToolset`;
- `ApprovalRequiredToolset`;
- `DeferredToolset`;
- `ToolsetLifecyclePolicy`;
- `ToolsetPreparation`;
- lifecycle events for initialized, unavailable, failed, refreshed, and
  closed states.

The gap is not basic capability. The gap is a polished Python builder layer
and the missing PyO3 bridges for context-aware dynamic/lifecycle toolsets.

## Public API Shape

### AbstractToolset

`AbstractToolset` is the primary Python customization contract. It should be a
lightweight base class or protocol that Python products can subclass without
learning Rust internals.

```python
class AbstractToolset:
    name: str
    id: str | None = None
    max_retries: int | None = None
    timeout_ms: int | None = None
    lifecycle_policy: ToolsetLifecyclePolicy | None = None

    async def enter(self, ctx: ToolsetContext) -> None: ...
    async def exit(self, ctx: ToolsetContext) -> None: ...

    async def get_tools(
        self,
        ctx: ToolsetContext,
    ) -> Iterable[Tool | BaseTool | Callable[..., object]]: ...

    async def get_instructions(
        self,
        ctx: ToolsetContext,
    ) -> Iterable[str | ToolInstruction] | str | None: ...

    async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation: ...

    def to_native(self) -> _native.Toolset: ...
```

Rules:

- `name` is required and must be stable for diagnostics.
- `id` is optional for transient runs but required for durable products.
- `get_tools()` and `get_instructions()` may be sync or async.
- `prepare()` is optional; the default implementation calls `get_tools()` and
  `get_instructions()`.
- `enter()` and `exit()` are optional lifecycle hooks.
- Python exceptions map to preparation failures or unavailable reports according
  to `ToolsetLifecyclePolicy`.
- returned tools use the existing `Tool` / `BaseTool` / callable conversion path.
- returned nested toolsets are allowed only through `ToolsetPreparation`, so the
  runtime can flatten and validate the inventory deliberately.
- `to_native()` returns a `PythonDynamicToolset` bridge, not a static snapshot.

`AbstractToolset` should also expose wrapper methods so custom dynamic toolsets
compose like native toolsets:

```python
toolset.prefixed("workspace")
toolset.filtered(include=["read_file"])
toolset.renamed({"read_file": "workspace_read"})
toolset.approval_required(names="*")
toolset.deferred(names=["open_browser"])
toolset.with_lifecycle(policy)
```

### PythonDynamicToolset

`PythonDynamicToolset` is the PyO3 bridge that implements
`starweaver_tools::Toolset` for a Python `AbstractToolset` instance.

Rust responsibilities:

- hold a GIL-safe reference to the Python object;
- build `ToolsetContext` from `AgentContext` at preparation time;
- call sync or async Python methods from Rust;
- convert returned Python tools/toolsets/instructions into native values;
- enforce lifecycle policy timeouts;
- propagate cancellation into Python callbacks when possible;
- emit lifecycle reports and preserve raw error evidence;
- never serialize Python callable objects.

Python-facing rules:

- users normally subclass `AbstractToolset`, not `PythonDynamicToolset`;
- `PythonDynamicToolset` can remain private or semi-private unless advanced
  embedding requires it;
- the bridge must not cache a prepared inventory across run steps unless the
  lifecycle policy explicitly asks for run-scoped preparation;
- durable restore validates toolset IDs against currently registered Python
  objects instead of attempting to deserialize Python objects.

### Toolset

`Toolset` remains the simple static grouped toolset:

```python
Toolset(
    "workspace",
    id="workspace",
    tools=[read_file, write_file],
    instructions=["Use workspace paths exactly."],
    max_retries=2,
    timeout_ms=30_000,
)
```

Rules:

- It should stay lightweight and predictable.
- It should keep accepting `Tool`, `BaseTool`, and raw callables.
- It should expose `tool_definitions()` and `instruction_records()`.
- It should convert to a native `DynToolset` once and reuse that handle.

### FunctionToolset

Add `FunctionToolset` as the built-in `AbstractToolset` implementation for local
function registration.

```python
class FunctionToolset(AbstractToolset):
    def __init__(
        self,
        name: str,
        *,
        id: str | None = None,
        instructions: str | Iterable[str | InstructionCallback] | None = None,
        max_retries: int | None = None,
        timeout_ms: int | None = None,
        strict: bool | None = None,
        sequential: bool = False,
        metadata: Mapping[str, object] | None = None,
    ) -> None: ...

    def tool(self, func=None, /, **options): ...
    def tool_plain(self, func=None, /, **options): ...
    def add_tool(self, tool: Tool | BaseTool) -> Tool: ...
    def add_function(self, func, /, **options) -> Tool: ...
    def instructions(self, func=None, /, **options): ...
```

Decorator examples:

```python
files = FunctionToolset("files", id="files")


@files.tool
async def read(ctx: ToolContext, path: str) -> str:
    """Read a file."""
    return await ctx.environment.read_text(path)


@files.tool_plain(name="clock")
def now() -> str:
    """Return current service time."""
    return "2026-01-01T00:00:00Z"


@files.instructions
def file_instructions(ctx: ToolsetContext) -> str:
    return f"Workspace root: {ctx.workspace_root}"
```

Rules:

- `@toolset.tool` allows `ToolContext` as the first parameter.
- `@toolset.tool_plain` rejects context parameters.
- `add_function(...)` mirrors the decorator options.
- toolset-level defaults apply to every tool unless the tool overrides them.
- tool metadata is merged as `toolset metadata < tool metadata`.
- duplicate tool names fail at registration time inside the toolset.
- duplicate names across toolsets fail deterministically during native registry
  preparation unless a prefix/rename wrapper resolves the conflict.
- dynamic instruction callbacks use the same `ToolsetContext` bridge as custom
  `AbstractToolset` subclasses.

### ToolsetContext

Dynamic Python instructions and dynamic toolset factories need a context view.

```python
@dataclass(frozen=True)
class ToolsetContext:
    run_id: str
    session_id: str | None
    agent_id: str
    run_step: int
    workspace_root: str | None
    metadata: Mapping[str, object]
    environment: EnvironmentProvider | None
    resources: ResourceRegistry
    raw_context: Mapping[str, object]
```

Rules:

- It is a read-only view.
- It does not expose mutable `AgentContext`.
- It may offer deliberate operations such as `state_get`, `state_set`, or
  `messages.send` only after those map cleanly to Rust contracts.
- It must be built from Rust context at preparation time.
- It must not hold non-threadsafe Rust references across awaits.

### Dynamic Toolset Factories

Support two forms:

```python
def workspace_tools(ctx: ToolsetContext) -> Toolset:
    if ctx.metadata.get("readonly"):
        return files.filtered(include=["read"])
    return files


agent = create_agent(model=model, toolsets=[workspace_tools])
```

```python
@agent.toolset(per_run_step=True)
def workspace_tools(ctx: ToolsetContext) -> Toolset:
    return build_workspace_toolset(ctx)
```

Rules:

- `per_run_step=True` evaluates at each run step.
- `per_run_step=False` evaluates once per run.
- factories return a `Toolset` or a sequence of toolsets.
- factories must have stable IDs for durable products.
- dynamic factories need explicit validation before use in durable execution.

`starweaver-py` should support this through the `PythonDynamicToolset` bridge.
Dynamic factories are not a separate runtime layer; they are convenience
constructors for context-aware `AbstractToolset` instances.

## Wrapper API

Expose Python methods that map to Rust wrappers.

```python
safe_shell = shell_toolset().approval_required("*", reason="shell review")
readonly = workspace.filtered(include=["read_file", "list_files"])
prefixed = github.prefixed("github")
renamed = tools.renamed({"open_issue": "issue_create"})
deferred = browser.deferred(["open_browser"])
prepared = tools.prepared(lambda ctx, tools: tools)
```

Target methods:

- `toolset.prefixed(prefix: str) -> Toolset`
- `toolset.filtered(include=..., exclude=..., predicate=...) -> Toolset`
- `toolset.renamed(mapping: Mapping[str, str]) -> Toolset`
- `toolset.approval_required(names="*", reason=None) -> Toolset`
- `toolset.deferred(names="*", reason=None) -> Toolset`
- `toolset.with_metadata(**metadata) -> Toolset`
- `toolset.prepared(callback) -> Toolset`
- `toolset.with_lifecycle(policy=...) -> Toolset`

Implementation rule:

- wrappers that can be expressed by existing Rust wrappers should call native
  constructors;
- wrappers that require Python callbacks need explicit PyO3 callback bridges;
- wrapper IDs are deterministic from the inner ID when possible;
- leaf toolsets used in durable execution must have stable explicit IDs.

## Lifecycle Semantics

Toolset lifecycle should use the Rust lifecycle model.

Target Python objects:

- `ToolsetLifecyclePolicy`
- `ToolsetLifecycleReport`
- `ToolsetLifecycleState`

Target callbacks:

```python
class AbstractToolset:
    async def enter(self, ctx: ToolsetContext) -> None: ...
    async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation: ...
    async def refresh(self, ctx: ToolsetContext) -> ToolsetPreparation: ...
    async def exit(self, ctx: ToolsetContext) -> None: ...
```

Rules:

- lifecycle callbacks are optional;
- lifecycle timeout is enforced by Rust;
- unavailable toolsets return an unavailable report instead of throwing when
  policy allows fallback;
- failures produce lifecycle events;
- cleanup runs on normal completion, failure, and interruption when
  `exit_after_run` is set;
- lifecycle events appear in stream/session evidence.

Do not add a Python lifecycle stack that runs outside the Rust registry. The
Rust registry must be the authority that decides the visible inventory.

## Instructions

Toolset instructions should support:

- static strings;
- grouped instruction records;
- dynamic callbacks from `ToolsetContext`;
- static/dynamic cache hints when Rust instruction records support them;
- ordered append after agent-level instructions.

Rules:

- instruction callbacks are not tool calls;
- instruction callback failures fail preparation or mark the toolset
  unavailable according to lifecycle policy;
- instruction text is model-visible and must not contain private metadata;
- dynamic instructions are recomputed at the configured preparation boundary.

## Dynamic Tool Search And Proxy

`ToolSearchToolset` and `ToolProxyToolset` remain separate.

Search behavior:

- model sees search/load tools;
- selected real tools become visible after discovery;
- loaded tool IDs and namespaces are serializable state.

Proxy behavior:

- model sees fixed search/call proxy tools;
- underlying tools remain hidden;
- proxy state is scoped by toolset ID or prefix.

Rules:

- do not merge search and proxy APIs;
- tool libraries expose metadata without model calls;
- search/proxy state uses IDs and namespaces, not Python object references;
- large libraries should prefer proxy when prompt cache locality matters;
- interactive products may prefer search when users need visible tool
  connection events.

## MCP Toolsets

Python should expose a typed MCP constructor over Rust MCP config:

```python
mcp = McpToolset(
    "github",
    transport=McpTransport.streamable_http("https://example.com/mcp"),
    headers={"authorization": "Bearer ..."},
    tool_prefix="github",
    include_instructions=True,
)
```

Rules:

- MCP config is typed;
- auth is supplied by product policy or provider helpers;
- sampling, prompts, resources, and subscriptions map to Rust MCP contracts;
- transport lifecycle is governed by toolset lifecycle policy;
- MCP is one toolset provider, not the generic toolset abstraction.

## Approval And Deferred Control Flow

Python builder options should map to native control flow:

```python
shell = FunctionToolset("shell", requires_approval=True)


@shell.tool(requires_approval=False)
async def inspect(ctx: ToolContext, command: str) -> str: ...


@shell.tool(requires_approval=True)
async def execute(ctx: ToolContext, command: str) -> str: ...
```

Rules:

- tool-level approval overrides toolset defaults;
- wrappers can apply approval to existing native toolsets;
- approval metadata is not model-visible unless explicitly returned by a tool;
- deferred tools produce canonical deferred IDs;
- typed HITL helpers build decisions and deferred results;
- product APIs persist approvals/deferred records outside the SDK.

## Error Model

Registration errors:

- invalid tool name;
- duplicate tool name within a toolset;
- missing description when strict mode requires it;
- invalid schema;
- dynamic factory returns unsupported value.

Preparation errors:

- duplicate tool name across prepared toolsets;
- lifecycle timeout;
- unavailable required toolset;
- dynamic instruction failure;
- Python callback failure.

Call errors:

- invalid model arguments;
- Python exception;
- timeout;
- cancellation;
- approval required;
- call deferred;
- model retry.

Rules:

- Python exceptions map to stable Starweaver errors;
- private traceback metadata stays private;
- cancellation propagates into Python callbacks;
- approval/deferred control exceptions are not swallowed by wrappers;
- raw error payloads remain available.

## Serialization And Durability

Durable products such as a Claw-like service need stable toolset identity.

Rules:

- every leaf toolset used by a durable product should have an explicit `id`;
- wrapper IDs should derive from the inner ID and wrapper kind unless an
  explicit ID is provided;
- dynamic loaded state stores tool IDs and namespaces;
- Python callable objects are never serialized;
- dynamic factories are re-registered by current process startup;
- restored sessions validate that required toolset IDs are present;
- runtime policy comes from the current profile, not stale archived Python
  objects.

## Rust To Python Implementation Plan

### Step 1: Define The AbstractToolset Contract

Add the Python public contract and pure-Python validation helpers:

- add `AbstractToolset`;
- add `ToolsetContext`;
- add `ToolsetPreparation`;
- add `ToolsetLifecyclePolicy`, `ToolsetLifecycleReport`, and
  `ToolsetLifecycleState` facades;
- define default `prepare()` in terms of `get_tools()` and
  `get_instructions()`;
- define optional `enter()` and `exit()` hooks;
- define wrapper methods on `AbstractToolset`;
- make `ensure_toolset()` accept `AbstractToolset` via `to_native()`;
- add tests for sync and async method support, bad return values, missing names,
  duplicate names, and durable ID warnings.

This step establishes the product-facing API before adding the native callback
bridge.

### Step 2: Add PythonDynamicToolset Bridge

Implement the PyO3 bridge that turns an `AbstractToolset` object into a native
`DynToolset`:

- implement a Rust `PythonDynamicToolset` wrapper;
- call Python `get_tools()`, `get_instructions()`, `prepare()`, `enter()`, and
  `exit()` from Rust;
- support sync and async Python callbacks;
- build `ToolsetContext` from `AgentContext`;
- convert Python tools and nested prepared toolsets to native values;
- enforce lifecycle policy timeouts;
- preserve cancellation and approval/deferred control-flow semantics;
- emit lifecycle reports for initialized, unavailable, failed, refreshed, and
  closed states;
- add Python and Rust tests for per-agent and per-run dynamic toolsets.

This is the core feature required for Claw-like Python product runtimes.

### Step 3: Implement FunctionToolset On AbstractToolset

Build the ergonomic local-function implementation on top of the dynamic
contract:

- add `FunctionToolset`;
- add `tool` and `tool_plain` decorators;
- add `add_function` and `add_tool`;
- add static and dynamic instructions;
- add defaults for retry, timeout, strict, sequential, metadata;
- add tests for schema inference, duplicate names, and per-run use.

`FunctionToolset` should use the same bridge as custom `AbstractToolset`
subclasses unless all callbacks are static and a native `StaticToolset`
optimization is harmless.

### Step 4: Expose Native Wrappers

Add PyO3 constructors for existing Rust wrappers:

- prefixed;
- renamed;
- filtered by static include/exclude;
- approval required;
- deferred;
- metadata;
- lifecycle policy values.

Static filtering can be native. Predicate filtering waits for the callback
bridge.

### Step 5: Add Dynamic Factories And Decorators

Support dynamic factories as a thin layer over `AbstractToolset`:

- `create_agent(..., toolsets=[factory])`;
- `agent.run(..., toolsets=[factory])`;
- `@agent.toolset`;
- run-scoped and run-step-scoped evaluation modes;
- stable ID validation for durable products.

Factories return `Toolset`, `AbstractToolset`, or sequences of either. They are
adapted into `PythonDynamicToolset` instances before entering the Rust runtime.

### Step 6: Extend ToolsetContext

Expose a read-only context view for preparation:

- run/session IDs;
- run step;
- metadata;
- environment handle;
- resource refs;
- raw safe state snapshot.

This requires a Rust-to-Python conversion that does not hold borrowed Rust
state across awaits.

### Step 7: Durable Toolset Validation

Add validation helpers:

```python
report = validate_toolsets_for_durability([workspace, github])
report.require_ids()
report.require_serializable_dynamic_state()
```

The report should be useful to product runtimes before they start a durable
session.

## Testing Requirements

Python tests:

- custom `AbstractToolset` subclass with `get_tools`;
- custom `AbstractToolset` subclass with `prepare`;
- custom `AbstractToolset` lifecycle hooks for enter and exit;
- `PythonDynamicToolset` preparation from agent context;
- `FunctionToolset` constructor with initial tools;
- `@toolset.tool` with `ToolContext`;
- `@toolset.tool_plain` without context;
- static and dynamic instructions;
- toolset defaults and per-tool overrides;
- duplicate names within a toolset;
- duplicate names across toolsets;
- per-agent and per-run toolsets;
- wrapper chaining;
- approval/deferred wrappers preserve canonical IDs;
- cancellation reaches running Python tool callbacks;
- private traceback metadata does not become model-visible.

Rust tests:

- `PythonDynamicToolset` implements the native `Toolset` preparation contract;
- PyO3 wrapper constructors produce the expected `DynToolset`;
- lifecycle policies map correctly;
- dynamic callback bridge respects timeouts and cancellation;
- lifecycle events are emitted;
- tool registry conflict errors remain deterministic.

Validation commands:

```bash
cargo test -p starweaver-tools --locked
cargo test -p starweaver-agent --locked
uv run pytest packages/starweaver-py/tests
make py-check
git diff --check
```

## Acceptance Criteria

The Python native toolset work is complete when:

- a Python author can subclass `AbstractToolset` and provide context-aware
  tools, instructions, preparation, and lifecycle hooks;
- `PythonDynamicToolset` adapts that object into the native Rust `Toolset`
  lifecycle without bypassing runtime-owned preparation;
- a Python author can build a useful toolset with decorators only;
- the same toolset can be attached to an agent or to a single run;
- wrappers are chainable and backed by Rust wrappers;
- dynamic factories can safely use run context through the callback bridge;
- durable products can validate stable toolset IDs;
- the Rust runtime remains responsible for preparation, execution, approval,
  deferred control flow, retries, cancellation, and evidence;
- public docs show only APIs implemented by tests.
