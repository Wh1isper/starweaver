# Advanced Composition

This spec defines the advanced Python SDK application facades that sit above the
core agent/tool/session path. It incorporates the former advanced package audit
and abstraction plan into the root Python SDK contract.

The design principle is consistent: Python provides natural application
objects; Rust owns runtime authority, provider contracts, session evidence,
environment enforcement, and stream records.

## Source-Backed Package Lessons

The ya-mono audit provides semantic references, not code to copy.

| Package family             | Useful idea                                                                                            | Starweaver rule                                                                                                                |
| -------------------------- | ------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------ |
| `ya-agent-environment`     | environment lifecycle, file/shell operators, resource registry, file-tree helpers                      | Wrap Rust `starweaver-environment` and envd providers. Preserve capability roots versus prompt/file-tree roots.                |
| `ya-agent-sdk`             | runtime composition, context, message bus, toolsets, tool search/proxy, skills, media, subagents, HITL | Map concepts into Starweaver-owned Rust contracts and Python names. Do not port the runtime loop to Python.                    |
| `ya-agent-stream-protocol` | AG-UI adapters, replay buffers, SSE cursors                                                            | Add Python adapters as projections over `starweaver-stream`; raw stream records remain the source of truth.                    |
| `ya-ripgrep-core`          | optional native search accelerator                                                                     | Prefer Rust environment/search tools already owned by Starweaver; do not add a second search authority.                        |
| `ya-oauth`                 | typed OAuth store, token source, Codex device login                                                    | Expose helpers over `starweaver-oauth`; do not duplicate credential storage in Python.                                         |
| `ya-oauth-provider`        | OAuth-backed provider constructors and routing headers                                                 | Keep routing affinity in typed provider settings; generic metadata must not become transport headers.                          |
| `yaacli`                   | product composition around sessions, approvals, background work, rendering                             | Use as an integration example; TUI rendering and product policy stay outside `starweaver-py`.                                  |
| `ya-claw`                  | durable product runtime, DB controllers, workspace providers, workflows, schedules                     | `starweaver-py` provides primitives. Workflow, memory, schedule, bridge, DB schema, and Docker retention policy stay above it. |

## RuntimeConfig

Runtime config separates runtime/context behavior from provider settings.

```python
runtime_config = RuntimeConfig(
    context_window=200_000,
    compact_threshold=0.75,
    cold_start_trim_seconds=2.0,
    stream_resume=True,
)

agent = create_agent(model=model, runtime_config=runtime_config)
```

Rules:

- runtime config maps to Rust runtime/context config, not provider headers;
- compact thresholds are read at run time so restored sessions do not capture
  stale startup config;
- security, approval, and sandbox policy come from the current profile unless
  an administrative restore path explicitly says otherwise;
- unknown runtime config fields are rejected.

## Toolset

Python toolsets provide grouped static composition without replacing the Rust
tool loop.

```python
workspace = Toolset(
    "workspace",
    tools=[read_file, write_file],
    instructions=["Use workspace paths exactly."],
)

agent = create_agent(model=model, toolsets=[workspace])
```

Required P0 contract:

- stable name;
- static tools;
- grouped instructions;
- metadata;
- per-agent and per-run `toolsets=[...]`;
- conversion to native Starweaver tools/toolsets or capability bundles;
- raw tool metadata available for inspection;
- collision errors are typed and deterministic.

Later extensions can add async lifecycle, prepare hooks, dynamic filtering,
approval/deferred wrappers, capability tags, supersession, and
environment-backed toolsets. Those extensions must use Starweaver capability or
toolset contracts instead of a Python-only middleware stack.

## ToolLibrary, ToolSearchToolset, And ToolProxyToolset

Tool search and tool proxy are distinct strategies and must stay distinct.

`ToolSearchToolset` dynamically exposes selected real tools:

- index tool metadata;
- search name, description, parameter names, parameter descriptions, and
  namespace;
- load tools or namespaces into the visible set;
- persist loaded tool IDs and namespace IDs in session state;
- emit typed sideband events such as `connected`, `skipped`, and `error`.

`ToolProxyToolset` keeps the visible prompt surface stable:

- exposes fixed `search_tools` and `call_tool` tools;
- routes calls to hidden tools through scoped proxy state;
- is better when huge tool lists would harm prompt cache locality.

Recommended Python shape:

```python
library = ToolLibrary([filesystem, github, browser])

agent = create_agent(
    model=model,
    toolsets=[
        ToolSearchToolset(library, search="bm25"),
        ToolProxyToolset(library, prefix="mcp"),
    ],
)
```

Rules:

- do not merge search and proxy;
- namespace IDs and loaded tool IDs are serializable state, not Python object
  references;
- multiple proxies use scoped state keys;
- MCP is one namespace provider, not the proxy abstraction itself;
- raw library metadata remains inspectable without model calls.

## SkillRegistry

Skills are Starweaver skills, not a Python-only format.

Required P0:

- list configured skills;
- load a skill package;
- inspect skill instructions and tool summaries;
- attach skill-provided toolsets or bundles to an agent.

Rules:

- skill roots respect environment file visibility;
- prompt indexes can use frontmatter/summary data, but full skill content loads
  through normal activation rules;
- request-boundary hot reload and remote registry sync wait for stable local
  semantics.

## Environment, FileOperator, Shell, And WorkspaceBinding

Python environment objects are facades over Rust-owned providers.

Target objects:

- `Environment`
- `LocalEnvironment`
- `VirtualEnvironment`
- `EnvdEnvironment`
- `FileOperator`
- `Shell`
- `VirtualPath`
- `VirtualMount`
- `WorkspaceBinding`

File operations should be protocol-shaped:

```python
class FileOperator:
    async def read(self, path: str) -> str: ...
    async def write(self, path: str, content: str) -> None: ...
    async def list_dir_with_types(self, path: str) -> list[FileEntry]: ...
    async def walk_files(self, root: str) -> AsyncIterator[FileEntry]: ...
    async def truncate_to_tmp(
        self,
        content: bytes,
        *,
        suffix: str = ".txt",
    ) -> ResourceRef: ...
```

Shell operations cover foreground and background execution:

```python
class Shell:
    async def execute(self, command: str, **options) -> CompletedProcess: ...
    async def start(self, command: str, **options) -> ExecutionHandle: ...
    async def wait_process(self, handle: ExecutionHandle, **options) -> CompletedProcess: ...
    async def kill_process(self, handle: ExecutionHandle) -> None: ...
    async def write_stdin(self, handle: ExecutionHandle, data: str) -> None: ...
    async def send_signal(self, handle: ExecutionHandle, signal: str) -> None: ...
```

Rules:

- `allowed_paths` is a capability boundary;
- `instructions_paths` is a model context/file-tree boundary;
- model-facing paths are virtual POSIX paths;
- host paths and temp host paths do not leak into durable model semantics;
- file-only and temp-only environments are valid;
- shell is optional;
- enforcement lives in the provider/envd layer, not only in Python config;
- environment instructions are injected fresh by runtime processors.

## ResourceRegistry

Resources are long-lived runtime objects owned by an environment.

Target objects:

- `BaseResource`
- `ResumableResource`
- `InstructableResource`
- `ResourceRegistry`
- `ResourceRef`
- `ResourceRegistryState`

Rules:

- factories bind resources to an environment;
- resources can provide toolsets and context instructions;
- only explicitly resumable resources export state;
- agent context references resources; it does not own their lifecycle;
- resource state restores through provider/factory semantics, not by
  deserializing arbitrary Python objects;
- large tool outputs should use `ResourceRef` or temp spill when available.

## MediaUploader

Python media helpers configure Starweaver media/filter seams and optional
product adapters.

Target:

- `MediaUploader` protocol;
- first concrete adapter for an app resource store or S3-like backend;
- history media upload processor;
- model/profile-level media URL hook;
- typed stream evidence for upload errors and redaction decisions.

Rules:

- large binary media should not be embedded in durable state when a resource ref
  is available;
- optional storage dependencies remain optional;
- private media URLs and redaction details stay out of model-visible content.

## StreamAdapter

Stream adapters are projections over `starweaver-stream`.

Potential adapters:

- display-event adapter;
- AG-UI-style adapter;
- SSE cursor adapter;
- replay buffer helper.

Rules:

- raw stream records remain accessible;
- cursors are stable and ordered;
- adapters do not invent alternate run/session state;
- replay buffers may compact text/reasoning/tool-call chunks but must preserve
  subagent detail and unknown records.

## ProviderAuth And Model Construction

Python should expose typed convenience constructors over Rust provider and
OAuth contracts.

Target:

- `ProviderModel.openai(...)`;
- `ProviderModel.codex_oauth(...)`;
- gateway endpoint overrides;
- WebSocket Responses opt-in;
- refresh status helpers.

Rules:

- OAuth token stores stay in `starweaver-oauth`;
- Python exposes token snapshots and account metadata but does not duplicate
  credential storage;
- provider session/thread/client request IDs stay in typed provider routing
  settings;
- Python generic metadata must not become provider-routing transport.

## ProductRuntimeAdapter

Claw-like products need integration points, not product policy in the SDK.

Core hooks:

- session store facade;
- stream replay adapter;
- typed HITL helpers;
- environment binding;
- resource refs;
- usage and trace helpers.

Product-owned features:

- workflow graph;
- schedules;
- memory controller;
- agency/team policy;
- bridge controllers;
- service database schema;
- Docker cache and retention policy;
- TUI or web rendering.

## Acceptance Checks

Advanced composition is correct only if:

- Python can compose agents with tools, bundles, toolsets, subagents, skills,
  and environment-backed resources through Starweaver-owned contracts;
- dynamic tool search/proxy state is serializable by ID;
- environment helpers respect `allowed_paths` and `instructions_paths`
  separately;
- resource lifecycle belongs to the environment/provider;
- provider routing uses typed settings, not generic metadata;
- stream adapters preserve raw records and stable cursors;
- media helpers avoid embedding large binary state when refs are available;
- Claw can build a product runtime without `starweaver-py` owning Claw
  workflows, schedules, memory, UI, or Docker policy.

Validation commands:

```bash
cargo test -p starweaver-tools --locked
cargo test -p starweaver-agent --locked
cargo test -p starweaver-environment --locked
uv run pytest packages/starweaver-py/tests
make py-check
make docs-check
make fmt-check
make check
make test
git diff --check
```
