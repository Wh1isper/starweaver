# Agent SDK Foundation Plan

This memo defines the next execution phase: harden Starweaver Agent SDK foundations before expanding application surfaces such as durable service orchestration and CLI product workflows.

## Phase Goal

Make `starweaver-agent` a clean, composable SDK layer over the core runtime, model, context, tools, and environment crates.

Target qualities:

- ergonomic agent construction and run/session APIs
- clean separation between SDK conveniences and runtime primitives
- toolset, context, environment, and subagent designs aligned with proven reference patterns
- strong tests around public SDK contracts and edge cases
- documentation examples that compile through `make docs-check`
- architecture improvements applied boldly when they simplify ownership or remove awkward seams

## Reference Study Map

Use the local reference clones as implementation guides.

### ya-agent-sdk reference areas

| Area                  | Reference paths                                                                                                     | Starweaver focus                                                            |
| --------------------- | ------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------- |
| Agent construction    | `refs/ya-mono/packages/ya-agent-sdk/ya_agent_sdk/agents`                                                            | `AgentBuilder`, `AgentApp`, `AgentSession`, run/session ergonomics          |
| Context/session state | `refs/ya-mono/packages/ya-agent-sdk/ya_agent_sdk/context`                                                           | `AgentContext`, typed dependencies, export/restore, session helpers         |
| Toolsets              | `refs/ya-mono/packages/ya-agent-sdk/ya_agent_sdk/toolsets`                                                          | first-party bundles, hooks, approval metadata, inherited tools              |
| Environment           | `refs/ya-mono/packages/ya-agent-sdk/ya_agent_sdk/environment`; `refs/ya-mono/packages/ya-agent-environment`         | environment handles, resource-backed toolsets, provider lifecycle           |
| Subagents             | `refs/ya-mono/packages/ya-agent-sdk/ya_agent_sdk/subagents`; `refs/ya-mono/packages/ya-agent-sdk/tests/subagents`   | unified delegation, subagent specs, inherited tool policy, lifecycle events |
| Filters and streaming | `refs/ya-mono/packages/ya-agent-sdk/ya_agent_sdk/filters`; `refs/ya-mono/packages/ya-agent-sdk/ya_agent_sdk/stream` | history processors, stream facade, event projection                         |
| SDK tests             | `refs/ya-mono/packages/ya-agent-sdk/tests`                                                                          | public contract coverage and regression tests                               |

### pydantic-ai reference areas

| Area              | Reference paths                                                               | Starweaver focus                                                                  |
| ----------------- | ----------------------------------------------------------------------------- | --------------------------------------------------------------------------------- |
| Agent APIs        | `refs/pydantic-ai/pydantic_ai_slim/pydantic_ai/agent`                         | per-run toolsets, overrides, run context/deps, output handling                    |
| Toolsets          | `refs/pydantic-ai/pydantic_ai_slim/pydantic_ai/toolsets`                      | combined/dynamic/prepared toolsets, wrapper toolsets, tool preparation            |
| Tools             | `refs/pydantic-ai/pydantic_ai_slim/pydantic_ai/tools.py`                      | function tool schema, validation, retries, approval/deferred semantics            |
| Native tools      | `refs/pydantic-ai/pydantic_ai_slim/pydantic_ai/native_tools`; `builtin_tools` | provider-native tool requests and built-in tool modeling                          |
| Durable execution | `refs/pydantic-ai/pydantic_ai_slim/pydantic_ai/durable_exec`                  | future runtime resume shape after SDK foundations solidify                        |
| Tests             | `refs/pydantic-ai/tests`                                                      | replay, model, toolset, output, tool-choice, and agent behavior coverage patterns |

## Current Starweaver SDK Baseline

Implemented SDK surface:

- `AgentBuilder` for model, settings, request params, output policy, validators, output functions, tools/toolsets, dynamic instructions, capabilities, usage limits, subagents, and test-model overrides.
- `AgentApp` as reusable application wrapper over a built runtime agent.
- `AgentSession` for stateful context, notes, metadata, message bus, trace context, state export/restore, environment attachment, and streaming helpers.
- `AgentSpec`, `AgentSpecRegistry`, `ModelPreset`, `SdkPreset`, and `text_output_preset`.
- first-party tool bundles: filesystem, shell, task, host operations, and tool proxy re-export.
- markdown subagent config parsing and SDK subagent registry foundations.
- runtime/core re-exports for application-facing ergonomics.

Implemented foundation tests:

- `crates/starweaver-agent/tests/builder.rs`
- `crates/starweaver-agent/tests/app_facade.rs`
- `crates/starweaver-agent/tests/session.rs`
- `crates/starweaver-agent/tests/bundles.rs`
- `crates/starweaver-agent/tests/presets.rs`
- `crates/starweaver-agent/tests/subagents.rs`
- `crates/starweaver-agent/tests/subagent_config.rs`
- `crates/starweaver-agent/tests/subagent_lifecycle.rs`

## Architecture Hardening Targets

### A1 Agent construction and run/session ergonomics

Review whether `AgentBuilder`, `AgentApp`, and `AgentSession` expose the smallest stable public surface.

Expected improvements:

- clearer split between reusable agent configuration and per-run/session overrides
- sharper naming for app/session/context helpers
- tests for builder composition order and override precedence
- doc examples for minimal agent, session restore, tool bundle registration, and structured output

### A2 Toolset composition at SDK boundary

Build on the landed core toolset abstraction.

Expected improvements:

- per-run additional toolsets and override toolsets if the API shape stays clean
- prepared/wrapper toolset hooks where capability filtering needs a public SDK seam
- first-party bundle metadata for auto-inherit and approval policy
- richer tests for instruction deduplication, namespacing, approval metadata, and proxy composition

### A3 Environment and resource-backed tools

Keep environment ownership in the environment crate and expose clean SDK handles.

Expected improvements:

- environment-provided toolsets and resource-provided toolsets as explicit SDK composition points
- provider capability traits for file/search/shell/process/resource operations after call sites prove the split
- deterministic virtual environment coverage for new tool behavior
- local provider tests for policy, hidden files, ignore rules, and path boundaries

### A4 Subagent foundations

Prioritize unified delegation and inherited tool policy in the SDK layer.

Expected improvements:

- unified `delegate` tool surface with typed args
- subagent availability based on required/optional toolsets
- auto-inherit metadata for task and context-management tools
- parent-child context inheritance rules captured in tests
- lifecycle events and trace parent propagation as SDK contracts

### A5 Spec presets and prompt assets

Keep preset code small and make prompt assets explicit.

Expected improvements:

- `AgentSpec` fields aligned with actual builder capabilities
- preset validation for referenced models, toolsets, subagents, and output policy
- examples under `examples/prompts` kept as assets rather than code presets
- docs and tests covering spec loading and failure modes

### A6 SDK coverage and quality gates

Expand focused SDK coverage before application surfaces.

Target checks:

```bash
cargo test -p starweaver-agent --locked
cargo test -p starweaver-runtime --test dependencies --test toolset --locked
cargo test -p starweaver-tools --test typed_tool --test toolset --test prefixed --locked
make docs-check
make fmt-check && make check && make test
```

Add tests before changing public APIs, then update docs examples after API shape stabilizes.

## Next Work Breakdown

### Step 1 Reference audit

Read and extract concrete patterns from reference code:

- agent construction and per-run toolsets
- context deps and session restore
- environment/resource toolset composition
- unified subagent delegation
- tool inheritance and approval metadata
- prepared/dynamic/wrapper toolsets

Output: update this memo with an evidence table mapping reference pattern to Starweaver code target.

### Step 2 SDK API review

Audit `crates/starweaver-agent/src` and tests for awkward public seams.

Focus files:

- `src/lib.rs`
- `src/builder.rs`
- `src/app.rs`
- `src/session.rs`
- `src/presets.rs`
- `src/bundles.rs`
- `src/bundles/*`
- `src/subagent_config.rs`
- SDK tests under `crates/starweaver-agent/tests`

Output: a small implementation plan with API changes, migration impact, and test additions.

### Step 3 Implement high-confidence improvements

Prioritize changes that simplify architecture or improve coverage:

1. per-run SDK toolset composition and override tests
2. environment/resource-provided toolset composition if the seam is straightforward
3. unified delegation tool with typed schema and inheritance policy
4. first-party bundle metadata for approval and auto-inherit
5. docs examples for the stable SDK path

### Step 4 Validate

Run focused checks after each slice and full validation before declaring the phase complete:

```bash
cargo test -p starweaver-agent --locked
cargo test -p starweaver-tools --locked
cargo test -p starweaver-runtime --locked
make docs-check
make fmt-check && make check && make test
```

## Sequencing Decision

Agent SDK foundation work comes before application expansion.

Application surfaces sequenced later:

- durable service runtime deepening
- checkpoint reload through service storage
- CLI profile/session product workflows
- service-backed SSE and approval endpoints
- platform adapters

The immediate implementation path stays inside `starweaver-agent`, with supporting changes in `starweaver-tools`, `starweaver-runtime`, `starweaver-context`, and `starweaver-environment` when SDK architecture requires them.
