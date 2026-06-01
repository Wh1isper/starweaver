# Built-in Tool Alignment Audit

This note tracks Starweaver alignment with provider-executed native tools and first-party SDK tool bundles. It is a working memo; specs describe Starweaver principles and public API direction.

## Model-layer native tools

Starweaver represents provider-executed tools through `NativeToolDefinition` in `crates/starweaver-model/src/adapter.rs`. Provider adapters map those definitions into wire requests while replay fixtures validate stable provider behavior.

| Native tool kind                      | Implementation evidence                                                              | Test coverage                                                                                                     | Replay fixture status                                                                               |
| ------------------------------------- | ------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| OpenAI Responses `web_search_preview` | `OpenAiResponsesAdapter::build_request` appends native tool definitions              | `crates/starweaver-model/tests/replay.rs::maps_native_tools_to_openai_responses_tools`; `native_tool_coverage.rs` | `tests/fixtures/openai_responses/native_web_search_request.json`; `native_web_search_response.json` |
| OpenAI Responses `mcp`                | `NativeMcpServer::native_tool_definition`; OpenAI Responses native mapping           | `crates/starweaver-tools/tests/mcp.rs`; `crates/starweaver-model/tests/native_mcp.rs`; replay mapping test        | `tests/fixtures/openai_responses/native_mcp_request.json`; `native_mcp_approval_response.json`      |
| OpenAI Responses `code_interpreter`   | generic native tool pass-through                                                     | `crates/starweaver-model/tests/native_tool_coverage.rs`                                                           | dedicated replay fixture pending                                                                    |
| OpenAI Responses `image_generation`   | native tool pass-through plus `image_generation_call` response parsing to file parts | `crates/starweaver-model/tests/native_tool_coverage.rs`; OpenAI Responses replay response parsing                 | `tests/fixtures/openai_responses/file_image_output.json`; request fixture pending                   |
| OpenAI Responses `file_search`        | native tool pass-through plus `file_search_call` parser branch                       | `crates/starweaver-model/tests/native_tool_coverage.rs`                                                           | dedicated request/response fixture pending                                                          |
| OpenAI Responses `web_fetch`          | generic native tool pass-through                                                     | `crates/starweaver-model/tests/native_tool_coverage.rs`                                                           | dedicated replay fixture pending                                                                    |
| OpenAI Responses `memory`             | generic native tool pass-through                                                     | `crates/starweaver-model/tests/native_tool_coverage.rs`                                                           | dedicated replay fixture pending                                                                    |
| Gemini `google_search`                | `GeminiGenerateContentAdapter::gemini_native_tool` maps to `googleSearch`            | `crates/starweaver-model/tests/native_tool_coverage.rs`; Gemini replay response set                               | `tests/fixtures/gemini/native_google_search_request.json`                                           |
| Gemini `code_execution`               | `GeminiGenerateContentAdapter::gemini_native_tool` maps to `codeExecution`           | `crates/starweaver-model/tests/native_tool_coverage.rs`; Gemini replay response set                               | `tests/fixtures/gemini/native_code_execution_request.json`                                          |
| Gemini generic native tools           | generic object mapping keyed by `tool_type`                                          | `crates/starweaver-model/tests/native_tool_coverage.rs` with `url_context`                                        | dedicated fixture added when a provider contract requires it                                        |
| Provider-private tool search          | planned provider/private built-in mode                                               | public SDK path uses core `ToolProxyToolset`                                                                      | tracked in roadmap                                                                                  |

## First-party tool bundle names

Implemented in `crates/starweaver-agent/src/bundles` and covered by `crates/starweaver-agent/tests/bundles.rs`. Bundle internals now use category modules: `environment/` for filesystem and shell, `external/` for host web/media/download/context tools, plus top-level re-export shims for `web` and `media` public types.

### Filesystem bundle

Toolset: `filesystem`

Tools:

- `view`
- `ls`
- `write`
- `edit`
- `multi_edit`
- `glob`
- `grep`
- `mkdir`
- `delete`
- `move`
- `copy`
- `resource_ref`

Execution status:

- `view`, `ls`, `write`, `edit`, `multi_edit`, `glob`, `grep`, and `resource_ref` execute against the active `EnvironmentProvider` stored in `AgentContext` dependencies.
- `mkdir`, `delete`, `move`, and `copy` currently emit host/provider operation envelopes pending richer provider operation traits.

### Shell bundle

Toolset: `shell`

Tools:

- `shell_exec`
- `shell_wait`
- `shell_status`
- `shell_input`
- `shell_signal`
- `shell_kill`

Execution status:

- Foreground `shell_exec` executes through `EnvironmentProvider::run_shell`.
- Background `shell_exec` and lifecycle tools emit durable operation envelopes until a process-capable provider lands.
- `shell_exec` carries approval metadata for host policy integration.

### Task bundle

Toolset: `task`

Tools:

- `task_create`
- `task_get`
- `task_update`
- `task_list`

Execution status: task tools emit operation envelopes for an SDK host or service layer to persist and route.

### Host-operation bundle

Toolset: `host_operations`

Current tools:

- `search`
- `fetch`
- `scrape`
- `download`
- `read_image`
- `read_video`
- `read_audio`
- `load_media_url`
- `summarize`
- `note`
- `note_get`
- `thinking`

Target public host-backed tools for web, media, download, and document work:

- `search`
- `scrape`
- `download`
- `load_media_url`
- `read_image`
- `read_video`
- `read_audio`

Execution status: host-operation tools execute web, scrape, download, and media helper paths through SDK adapters. Notes, thinking, and context handoff remain lightweight operation envelopes, and task tracking lives in the dedicated task bundle.

### Tool proxy

Core implementation: `crates/starweaver-tools/src/tool_proxy.rs`

Agent SDK re-export: `crates/starweaver-agent/src/bundles/tool_proxy.rs`

Tools:

- `search_tools`
- `call_tool`

Execution status:

- `ToolProxyToolset` exposes a fixed two-tool proxy over wrapped toolsets.
- Prefixing is composed externally with `PrefixedToolset` / `namespaced_toolset`.
- Approval and deferred control-flow errors propagate through `call_tool`.

## Coverage gaps to track

- Dedicated replay fixtures for OpenAI Responses `code_interpreter`, `image_generation` request mapping, `file_search` request/response, `web_fetch`, and `memory`.
- Provider-backed execution for currently envelope-only filesystem, shell lifecycle, task, and host-operation tools.
- Host-backed replacements for ya-agent-sdk web/search/crawler/media/download tools are tracked in `memos/sdk-host-tool-gap-report.md`.
- Target public web tools: `search(query, num)` and `scrape(url)`. Raw `fetch` behavior becomes an internal adapter.
- Target media/download tools: `download(urls, save_dir)`, `load_media_url(url)`, `read_image(url)`, `read_video(url)`, and `read_audio(url)`. Document conversion is provided through skill workflows backed by shell commands.
- Search/scrape/media/download adapters should cover protocol validation, host network policy delegation, streaming size limits, text truncation, binary guards, safe environment writes, URL accessibility metadata, citation metadata, model capability detection, usage accounting, and deterministic fixtures.
- `glob`, `grep`, `search_tools`, and `call_tool` have direct executable Starweaver replacements; the fixed `ToolProxyToolset` is the public SDK replacement for large searchable tool surfaces.
- Unified delegation tool and skill-contributed toolsets in the SDK subagent/skill layer.

## Validation evidence

Last verified local validation set:

```bash
make fmt-check && make check && make test && make docs-check && make replay-check
```

Focused checks for this audit:

```bash
cargo test -p starweaver-agent --test bundles --locked
cargo test -p starweaver-model --test native_tool_coverage --locked
cargo test -p starweaver-model --test native_mcp --locked
cargo test -p starweaver-model --test replay --locked
cargo test -p starweaver-tools --test mcp --locked
```
