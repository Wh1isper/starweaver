# Agent Loop and Model Protocol Audit - 2026-06-08

## Scope

This audit validates Starweaver's agentic loop against Pydantic AI reference behavior and adds coverage for provider protocol mappings used by the loop.

Reference repository:

- `refs/pydantic-ai` at commit `837b03e`

Reference areas reviewed:

- `pydantic_ai_slim/pydantic_ai/_agent_graph.py`
- `pydantic_ai_slim/pydantic_ai/run.py`
- `tests/test_agent.py`
- `tests/test_tools.py`
- `tests/test_usage_limits.py`
- `tests/test_messages.py`

## Pydantic AI Semantics Used as Baseline

- The run graph progresses deterministically from user prompt to model request to tool-call handling.
- Canonical message history records request and response turns and is the source of continuation state.
- Tool calls in a model response are executed before the next model request, and tool returns are sent in the next request.
- Output validation retries create retry prompts and consume the output retry budget.
- Provider adapters normalize canonical messages into protocol-specific wire shapes while preserving tool-call and tool-return boundaries.

## Starweaver Runtime Review

Reviewed files:

- `crates/starweaver-runtime/src/agent/run_loop.rs`
- `crates/starweaver-runtime/src/agent/runtime_helpers.rs`
- `crates/starweaver-runtime/src/run.rs`
- `crates/starweaver-runtime/tests/agent.rs`
- `crates/starweaver-runtime/tests/toolset.rs`

Findings and changes:

- Added regression coverage that after-model response hooks mutate the canonical response history, not only the final output.
- Fixed the run state by adding `AgentRunState::replace_latest_response` and using it after `after_model_response` hooks.
- Updated toolset instruction coverage to assert toolset instructions are prepared as request parameters, matching the current provider preparation path.

## Model Protocol Review

Reviewed protocol adapters:

- OpenAI Chat Completions: `crates/starweaver-model/src/providers/openai_chat.rs`
- OpenAI Responses: `crates/starweaver-model/src/providers/openai_responses.rs`
- Anthropic Messages: `crates/starweaver-model/src/providers/anthropic.rs`
- Gemini generateContent: `crates/starweaver-model/src/providers/gemini.rs`
- AWS Bedrock Converse: `crates/starweaver-model/src/providers/bedrock.rs`

Added tests:

- `crates/starweaver-model/tests/protocol_agent_loop_mapping.rs`
- `crates/starweaver-model/tests/protocol_client_agent_loop.rs`

Coverage added for each protocol:

- Canonical request history with system/instruction/user parts.
- Canonical model response with a function/tool call.
- Canonical tool return mapped back into the next provider request.
- Provider response parsing back to canonical text, tool call, provider metadata, usage, and finish reason.
- Client-level full tool-loop coverage through `ProtocolModelClient`, proving `prepare_model_request`, profile normalization, tool definitions, HTTP body construction, and response parsing work together for all five protocol families.

Late background explorer gap closure:

- Fixed Anthropic Messages user-content mapping so advertised image/document input support no longer drops non-text `ContentPart`s.
- Added Anthropic image/document client coverage and updated the Anthropic image replay fixture to assert image blocks are preserved in the provider request.
- Confirmed unsupported Anthropic audio/video content now fails with an explicit mapping error instead of silently dropping content.

## TUI Model Picker Review

Reviewed Starweaver TUI and local YAACLI availability. The installed YAACLI source was not present locally, but Starweaver already had model profile metadata and JSON-RPC model selection foundations.

Changed files:

- `crates/starweaver-cli/src/tui/state.rs`
- `crates/starweaver-cli/src/tui/terminal.rs`
- `crates/starweaver-cli/src/tui/render.rs`
- `crates/starweaver-cli/src/tui/tests.rs`
- `crates/starweaver-cli/src/service.rs`

Behavior added:

- `/model` opens a keyboard-driven model picker panel instead of only printing a transcript list.
- Up/Down navigate model choices, Enter selects, Esc closes.
- The picker shows highlighted profile details including model id, settings/config presets, context window, and source.
- Direct `/model <profile>` selection still works.
- TUI-selected profile is persisted through existing TUI state handling.

## Validation

Commands run successfully:

```bash
cargo test -p starweaver-cli tui::tests -- --nocapture
cargo test -p starweaver-runtime capability_hooks_can_mutate_response_and_record_lifecycle -- --nocapture
cargo test -p starweaver-model --test protocol_agent_loop_mapping -- --nocapture
cargo test -p starweaver-runtime --test toolset -- --nocapture
cargo fmt --all
make check
make test
make fmt-check
make replay-check
```

Final repository-level status:

- `make check`: passed
- `make test`: passed
- `make fmt-check`: passed
- `make replay-check`: passed
