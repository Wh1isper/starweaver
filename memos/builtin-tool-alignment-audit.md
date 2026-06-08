# Built-in Tool Alignment Audit

This memo tracks provider-native tools, SDK first-party bundles, and advanced tool follow-ups after the latest Pydantic AI and ya-mono audit.

## Model-Layer Native Tools

Starweaver represents provider-executed tools through `NativeToolDefinition` in `crates/starweaver-model/src/adapter.rs`. Provider adapters map these definitions into wire requests and replay fixtures validate stable behavior.

| Native tool kind                      | Current state                                 | Evidence                                                                        | Remaining replay work                                      |
| ------------------------------------- | --------------------------------------------- | ------------------------------------------------------------------------------- | ---------------------------------------------------------- |
| OpenAI Responses `web_search_preview` | landed                                        | `OpenAiResponsesAdapter`, `native_tool_coverage.rs`, native web search fixtures | maintain with provider changes                             |
| OpenAI Responses `mcp`                | landed                                        | `NativeMcpServer`, `native_mcp.rs`, `mcp.rs`, native MCP fixtures               | add live server integration after concrete transports land |
| OpenAI Responses `code_interpreter`   | pass-through landed                           | `native_tool_coverage.rs`                                                       | dedicated request/response fixture                         |
| OpenAI Responses `image_generation`   | pass-through and response file parsing landed | `native_tool_coverage.rs`, image/file output parsing fixtures                   | dedicated request fixture                                  |
| OpenAI Responses `file_search`        | pass-through and parser branch landed         | `native_tool_coverage.rs`                                                       | dedicated request/response fixture                         |
| OpenAI Responses `web_fetch`          | pass-through landed                           | `native_tool_coverage.rs`                                                       | dedicated fixture                                          |
| OpenAI Responses `memory`             | pass-through landed                           | `native_tool_coverage.rs`                                                       | dedicated fixture                                          |
| Gemini `google_search`                | landed                                        | Gemini native tool mapper and request fixture                                   | maintain with provider changes                             |
| Gemini `code_execution`               | landed                                        | Gemini native tool mapper and request fixture                                   | maintain with provider changes                             |
| Gemini generic native tools           | generic object mapping landed                 | `url_context` coverage in `native_tool_coverage.rs`                             | add fixture when public API depends on a specific tool     |
| Provider-private tool search          | pending                                       | SDK uses `ToolProxyToolset` for public large-tool surfaces                      | revisit after provider private tool APIs stabilize         |

## First-Party SDK Tool Bundles

### Filesystem Bundle

Toolset: `filesystem`

Tools: `view`, `ls`, `write`, `edit`, `multi_edit`, `glob`, `grep`, `mkdir`, `delete`, `move`, `copy`, `resource_ref`.

Current state:

- `view`, `ls`, `write`, `edit`, `multi_edit`, `glob`, `grep`, and `resource_ref` execute through the active `EnvironmentProvider`.
- `mkdir`, `delete`, `move`, and `copy` currently return operation envelopes pending richer provider mutation traits.

### Shell Bundle

Toolset: `shell`

Tools: `shell_exec`, `shell_wait`, `shell_status`, `shell_input`, `shell_signal`, `shell_kill`.

Current state:

- Foreground execution runs through `EnvironmentProvider::run_shell`.
- Process-capable shell providers, durable process snapshots, handles, wait/status/input/signal/kill behavior, and deterministic tests are landed.
- Host policy, sandbox-backed execution, and CLI shell review remain active work.

### Task Bundle

Toolset: `task`

Tools: `task_create`, `task_get`, `task_update`, `task_list`.

Current state: task tools emit structured operation envelopes for SDK hosts or service layers to persist and route.

### Host-Operation Bundle

Toolset: `host_operations`

Current default tools: `search`, `fetch`, `scrape`, `download`, `read_image`, `read_video`, `read_audio`, `load_media_url`, `summarize`, `note`, `note_get`, `thinking`.

Current state:

- `search` executes through injectable `HostSearchClientHandle` or configured Brave Search environment settings.
- `scrape` executes through injectable `HostScrapeClientHandle`, Firecrawl settings, Cloudflare seam, or local static-HTML fallback.
- `fetch` remains a public compatibility tool and shares the internal HTTP substrate.
- `download` writes text-like resources into the active `EnvironmentProvider`; binary downloads return a structured requirement for binary/resource provider extensions.
- `load_media_url` classifies HTTP/HTTPS media and document URLs and checks model media capabilities.
- `read_image`, `read_video`, and `read_audio` execute through injectable fallback media understanding clients when configured.
- `summarize`, `note`, `note_get`, and `thinking` remain lightweight context/control envelopes.

Target cleanup:

- Keep `search` and `scrape` as the compact public web surface for default model-facing use.
- Keep `fetch` available as compatibility/internal-adapter behavior until callers migrate.
- Add binary/resource write extensions and concrete fallback media clients.
- Keep PDF/Office conversion as skill workflows backed by shell tools.

### Tool Proxy

Toolset: `tool_proxy`

Tools: `search_tools`, `call_tool`.

Current state:

- `ToolProxyToolset` exposes a fixed two-tool proxy over wrapped toolsets.
- Prefixing and namespacing compose through `PrefixedToolset` and `namespaced_toolset`.
- Approval and deferred control-flow errors propagate through `call_tool`.

Remaining parity depth from ya-agent-sdk:

- Pluggable search strategies such as BM25 and keyword ranking.
- Loaded namespace/tool state stored in `AgentContext` for restore and follow-up calls.
- Namespace initialization report events for loaded, skipped, and failed tool namespaces.
- Session restore evidence showing which large tool surfaces were active.
- Optional namespace failure reporting that can be displayed by CLI and service hosts.

## Advanced Tool Follow-Ups From Pydantic AI

- Add tool definition fields for strictness, timeout, return schema, native fallback hints, capability owner, and approval/deferred policy instead of relying on generic metadata.
- Add tool argument validators and per-tool prepare hooks.
- Add toolset combinators: filtered, prepared, renamed, approval-required, dynamic, and deferred-loading.
- Add advanced tool return envelopes with application return value, model-visible content, user-visible content, and private metadata.
- Add SDK-level deferred tool requests/results and inline handler capability.

## Focused Validation

```bash
cargo test -p starweaver-agent --test bundles --locked
cargo test -p starweaver-agent --test process_shell --locked
cargo test -p starweaver-agent --test live_mcp --locked
cargo test -p starweaver-model --test native_tool_coverage --locked
cargo test -p starweaver-model --test native_mcp --locked
cargo test -p starweaver-model --test replay --locked
cargo test -p starweaver-tools --locked
make replay-check
```
