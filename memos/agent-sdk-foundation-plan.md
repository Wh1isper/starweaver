# Agent SDK Foundation Plan

This memo is the Agent SDK P0/P1 foundation evidence record. It records landed SDK foundation decisions, evidence paths, focused test targets, and how this foundation aligns with the current audited roadmap in `memos/implementation-todo.md`.

## Foundation Result

The Agent SDK foundation slice is implemented in the current workspace. `starweaver-agent` exposes a broader application-facing layer over runtime, model, context, tools, environment, and MCP foundations.

Landed qualities:

- ergonomic agent construction through `AgentBuilder`, `AgentApp`, `AgentSession`, and run-scoped options
- serializable app profiles through `AgentSpec`, `AgentSpecRegistry`, SDK policy presets, host adapter specs, MCP server specs, output profiles, skill config, and environment/durability policy config
- first-party tool bundle composition for filesystem, shell, task, host operations, skills, environment helpers, process-capable shell handles, and MCP adapter seams
- subagent delegation with inherited tool policy, denied tool parsing, auto-inherit support, approval metadata propagation, lifecycle events, trace parent propagation, and nested delegation guardrails
- fileops-loaded skill discovery over `EnvironmentProvider`, `SKILL.md` frontmatter parsing, summary toolset generation, activation, and metadata preservation
- host-backed search, scrape, download, media URL loading, and fallback media adapter seams through injectable clients and environment-backed execution paths
- process-capable shell provider traits, durable process snapshots, handle attachment, stdin/signal/status/wait/kill tool behavior, and deterministic virtual provider coverage
- live MCP adapter seam through `LiveMcpClient`, `live_mcp_toolset`, discovered tool snapshots, and deterministic tests
- docs updates for SDK app profiles, tool bundles, subagents, and MCP foundations

## Current Evidence Checked

Implementation files:

- `crates/starweaver-agent/src/presets.rs`
- `crates/starweaver-agent/src/subagent.rs`
- `crates/starweaver-agent/src/subagent_config.rs`
- `crates/starweaver-agent/src/bundles/skills.rs`
- `crates/starweaver-agent/src/bundles/environment/handle.rs`
- `crates/starweaver-agent/src/bundles/environment/shell.rs`
- `crates/starweaver-agent/src/bundles/external/web.rs`
- `crates/starweaver-agent/src/bundles/external/download.rs`
- `crates/starweaver-agent/src/bundles/external/media.rs`
- `crates/starweaver-agent/src/mcp_live.rs`
- `crates/starweaver-environment/src/lib.rs`
- `crates/starweaver-runtime/src/agent.rs`
- `crates/starweaver-tools/src/registry.rs`

Focused tests:

- `crates/starweaver-agent/tests/agent_spec_profiles.rs`
- `crates/starweaver-agent/tests/subagent_inheritance.rs`
- `crates/starweaver-agent/tests/skills.rs`
- `crates/starweaver-agent/tests/process_shell.rs`
- `crates/starweaver-agent/tests/live_mcp.rs`
- `crates/starweaver-agent/tests/bundles.rs`
- `crates/starweaver-agent/tests/subagent_config.rs`

Docs updated:

- `docs/sdk-app.md`
- `docs/tools.md`
- `docs/subagents.md`
- `docs/mcp.md`

Specs updated:

- `spec/sdk/01-agent-sdk-app.md`
- `spec/sdk/03-first-party-tool-bundles.md`
- `spec/sdk/04-subagents-skills.md`
- `spec/sdk/05-sdk-integration-map.md`

Foundation validation targets for this slice:

```bash
make check
make test
make docs-check
make fmt-check
git diff --check
cargo test -p starweaver-agent --locked
cargo test -p starweaver-tools --locked
cargo test -p starweaver-environment --locked
```

The current audit validation evidence lives in `memos/audit-evidence-2026-06-07.md`.

## Reference Study Map

Use local reference clones as future deepening guides.

| Area                  | Reference evidence                                                | Starweaver target                                                                     |
| --------------------- | ----------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| Agent construction    | pydantic-ai agent run/override APIs; ya-agent-sdk agent factories | `AgentBuilder`, `AgentApp`, `AgentSession`, `AgentRunOptions`                         |
| Context/session state | ya-agent-sdk context and session restore patterns                 | `AgentContext`, typed dependencies, state export/restore, session helpers             |
| Toolsets              | pydantic-ai `AbstractToolset`, prepared/wrapper toolsets          | `Toolset`, `ToolRegistry`, `PrefixedToolset`, `ToolProxyToolset`, first-party bundles |
| Skills                | ya-mono `SkillToolset`, skill docs, skill tests                   | fileops-loaded `SkillRegistry`, `SkillPackage`, `skill_tools()`                       |
| Environment           | ya-agent-sdk environment docs and resource ownership              | environment handles, provider-backed tools, process shell extension traits            |
| Subagents             | ya-agent-sdk subagent docs and tests                              | inherited tools, availability, lifecycle events, delegation guardrails                |
| MCP                   | MCP protocol and `rmcp` direction                                 | `LiveMcpClient`, `McpToolset`, future concrete transports                             |

## Public API Decisions Merged From Review

Kept as stable SDK concepts:

- `AgentBuilder` fluent builder pattern
- `AgentApp` reusable app wrapper over runtime agent plus SDK protocols
- `AgentSession` context-backed multi-run state container
- `AgentRunOptions` per-run override surface
- `AgentSpecRegistry` as the serialized-spec resolver for host-provided handles
- first-party bundle factories returning `DynToolset`
- model preset ownership in `starweaver-model`, with agent-layer re-exports
- markdown subagent parsing as a serializable spec loader

Added in this slice:

- SDK policy preset family: approval, retry, streaming, observability, environment, and durability
- expanded `AgentSpec` profile fields for policies, environment, skills, host adapters, MCP servers, output, and runtime/session profile data
- `SkillConfig`, `SkillSourceScope`, `SkillPackage`, `SkillRegistry`, `parse_skill_markdown`, and `skill_tools()`
- `SubagentToolInheritancePolicy`, inherited tool resolution, denied tool metadata, and nested delegation guardrails
- environment and process-shell toolset helpers at the SDK boundary
- process-capable shell provider trait and handle/snapshot APIs
- concrete host adapter specs plus injectable search, scrape, and media fallback client handles
- live MCP adapter helpers over discovered server snapshots

Clarified direction:

- serialized specs reference stable names resolved by `AgentSpecRegistry`
- programmatic handles for model adapters, HTTP clients, process managers, and live MCP clients stay in host registries or typed dependencies
- remote skill registry sync, durable subagent polling, and concrete MCP transports sit in later application/runtime slices

## Landed P0/P1 Test Slices

| Slice                | Current test file                  | Covered behavior                                                                                      |
| -------------------- | ---------------------------------- | ----------------------------------------------------------------------------------------------------- |
| AgentSpec profiles   | `agent_spec_profiles.rs`           | policy/spec fields, registry resolution, unknown-name diagnostics, explicit whole-registry attachment |
| Subagent inheritance | `subagent_inheritance.rs`          | required, optional, denied, auto-inherited, approval metadata, nested guardrails                      |
| Skills               | `skills.rs`                        | virtual provider scan, parser requirements, metadata preservation, activation, summaries              |
| Process shell        | `process_shell.rs`                 | process-capable provider handles, wait/status/input/signal/kill behavior, deterministic snapshots     |
| Live MCP             | `live_mcp.rs`                      | discovered MCP tools/instructions mapped into SDK toolsets                                            |
| Bundles and config   | `bundles.rs`, `subagent_config.rs` | tool metadata, shell process behavior, denied tool frontmatter                                        |

## Alignment With Current Audited Roadmap

The June 2026 audit moved active SDK deepening work into `memos/implementation-todo.md` and related specs:

| Foundation area         | Current roadmap owner                                                                        |
| ----------------------- | -------------------------------------------------------------------------------------------- |
| Durable service runtime | `memos/implementation-todo.md` P0.2 and `spec/ops/03-durable-service-runtime.md`             |
| CLI product workflows   | `memos/implementation-todo.md` P0.5 and `spec/ops/04-cli-product.md`                         |
| CLI audit               | `memos/implementation-todo.md` P1 and `spec/ops/04-cli-product.md`                           |
| Platform adapters       | `spec/README.md` and future platform specs                                                   |
| Host tool depth         | `memos/sdk-host-tool-gap-report.md` and `spec/sdk/03-first-party-tool-bundles.md`            |
| Pydantic AI maturity    | `spec/core/05-pydantic-ai-feature-map.md` and `spec/core/07-pydantic-ai-maturity-roadmap.md` |
| Refactor readiness      | `spec/ops/09-refactor-readiness.md`                                                          |

Use this memo for foundation evidence. Use `memos/implementation-todo.md` for current execution priority and validation gates.
