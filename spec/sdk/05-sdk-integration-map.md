# SDK Integration Map

This spec maps application-facing agent concepts into Starweaver's first-party SDK architecture. Its maturity labels are non-normative planning guidance. Current implementation status is generated from `../capabilities.toml` into [`../capability-status.md`](../capability-status.md), which is authoritative when the views differ.

## Integration Principles

- Policy filters are ordered SDK capabilities with explicit hook points and context evidence.
- Environment modules are `EnvironmentProvider` implementations and environment-backed tool bundles.
- Context helpers are `AgentContext` state, notes, messages, tasks, usage, and typed dependencies.
- Subagent configuration is `SubagentSpec`, `SubagentConfig`, registry entries, and delegation tools.
- First-party SDK features remain extensible through traits, capabilities, toolsets, typed dependencies, and host-provided handles.

## Module Map

| Feature family        | Target                                                     | Registry capability             | Spec owner                                                                 | Validation path                  |
| --------------------- | ---------------------------------------------------------- | ------------------------------- | -------------------------------------------------------------------------- | -------------------------------- |
| agent construction    | `AgentBuilder`, `AgentApp`, `AgentSession`                 | `runtime.agent_loop`            | `sdk/01-agent-sdk-app.md`                                                  | SDK session and builder tests    |
| lifecycle hooks       | runtime hooks and capability lifecycle                     | `runtime.capability_middleware` | `core/03-tools-output-capabilities.md`                                     | capability tests                 |
| capability middleware | ordered wrappers, IDs, per-run instances                   | `runtime.capability_middleware` | `core/03-tools-output-capabilities.md`                                     | capability ordering tests        |
| context compaction    | ordered message-preparation capabilities and context state | —                               | `core/04-context-state-executor.md`                                        | capability/filter tests          |
| policy guards         | request guards, approval/deferred metadata                 | —                               | `core/03-tools-output-capabilities.md`                                     | guard/control-flow tests         |
| streaming             | runtime stream records and service/CLI adapters            | `stream.versioned_records`      | `core/01`, `ops/03`, `ops/04`                                              | stream/replay tests              |
| context stores        | notes, message bus, state, tasks, usage                    | `context.versioned_checkpoints` | `core/04-context-state-executor.md`                                        | context and bundle tests         |
| environment           | provider families and policy                               | `environment.provider`          | `sdk/02-environment-provider.md`                                           | fake/local/process tests         |
| filters               | named policy filter capabilities                           | —                               | this spec; `core/03-tools-output-capabilities.md`                          | SDK filter order tests           |
| toolsets              | first-party bundles, MCP, proxy                            | —                               | `sdk/03-first-party-tool-bundles.md`                                       | toolset/proxy/MCP tests          |
| toolset wrappers      | filtered/prepared/renamed/approval/dynamic/deferred        | —                               | `core/03-tools-output-capabilities.md`                                     | wrapper tests                    |
| deferred tools        | SDK requests/results and inline handlers                   | —                               | `ops/03`, `core/03`                                                        | control-flow and service tests   |
| subagents             | specs, registry, inherited tools, async supervisor         | —                               | `sdk/04-subagents-skills.md`; `sdk/06-async-subagent-execution.md`         | subagent lifecycle/product tests |
| agent session tools   | query/control bundles over host-injected capabilities      | —                               | `sdk/03-first-party-tool-bundles.md`; `ops/08-agent-session-management.md` | bundle and product tests         |
| skills                | fileops-loaded skills and tool summaries                   | —                               | `sdk/04-subagents-skills.md`                                               | skill tests                      |
| media                 | binary/resource/data-url parts and preflight               | —                               | `sdk/03-first-party-tool-bundles.md`                                       | media/preflight/provider tests   |
| config/specs          | AgentSpec, presets, host handles                           | —                               | `sdk/01-agent-sdk-app.md`                                                  | spec/profile tests               |
| UI projection         | AG-UI/Vercel display projection                            | `stream.ui_projection`          | `ops/02-shared-execution-components.md`                                    | adapter conformance tests        |
| model wrappers        | fallback/concurrency/instrumentation wrappers              | `model.wrappers`                | `core/02-model-provider-replay.md`                                         | model wrapper tests              |
| provider lifecycle    | future provider-owned lifecycle depth                      | —                               | future model RFC                                                           | future contract tests            |

## Filters as Capabilities

```mermaid
flowchart TD
    filter[Named filter capability]
    bundle[StaticCapabilityBundle]
    runtime[Runtime prepare_model_messages hook]
    context[AgentContext]
    metadata[Run metadata]

    filter --> bundle
    bundle --> runtime
    runtime --> context
    runtime --> metadata
```

### Current Filter Status

The first SDK filter capability slice is landed in `crates/starweaver-agent/src/filters.rs`:

- `DEFAULT_FILTER_ORDER`
- `default_filter_bundle()`
- `default_filter_capabilities()`
- `NamedFilterCapability`
- `CacheFriendlyCompactCapability`
- `MediaUploader` seam
- media preflight and upload replacement behavior
- cold-start tool-return trimming
- capability/media support filtering
- compact keep-message behavior
- handoff metadata support
- prompt-only file-inspection reminders, background/bus metadata injection, environment/runtime context injection, and true instruction metadata injection
- system prompt reinjection composition
- tool-call argument repair
- reasoning normalization

Current order:

```text
reasoning_normalize -> media_split -> media_compress -> media_preflight -> media_upload -> tool_args -> handoff -> auto_load_files -> capability -> bus_message -> background_shell -> compact -> cold_start -> environment_context -> auto_load_files_after_compact -> runtime_context -> system_prompt
```

Remaining filter depth:

| Filter family       | Current state                          | Remaining work                                                                                    |
| ------------------- | -------------------------------------- | ------------------------------------------------------------------------------------------------- |
| auto-load files     | escaped path reminders only            | focused request parts and restore-path fixtures                                                   |
| background shell    | process provider substrate exists      | completed process injection, output spill files, lifecycle UI evidence                            |
| bus messages        | context message bus exists             | consume-once request pipeline behavior and retry safety tests                                     |
| cold start          | tool-return trimming slice             | idle-window heuristics and cache-friendly compaction evidence                                     |
| environment context | metadata/provider-driven injection     | provider summary, workspace policy, resource state, sandbox evidence                              |
| handoff             | metadata-driven slice                  | restored-history reconstruction with keep tags and steering parts                                 |
| media preflight     | byte sniffing and policy checks landed | compression, alpha compositing, tall splitting, GIF policy, count limits across nested structures |
| media upload        | adapter seam landed                    | S3/resource-store adapters and failure fallback fixtures                                          |
| model switch        | profile presets exist                  | model-switch event normalization and history evidence                                             |
| reasoning normalize | first normalization slice              | provider-specific reasoning/thinking reconstruction fixtures                                      |
| runtime context     | SDK provider-bound injection exists    | refresh-after-tool-return and non-durable provider-message trace fixtures                         |
| system prompt       | landed                                 | preserve coverage as capabilities evolve                                                          |
| tool args           | repair slice landed                    | malformed/truncated argument fixture depth                                                        |

## Environment Integration

```mermaid
flowchart LR
    virtual[Virtual provider]
    local[Local provider]
    process[Process-capable provider]
    sandbox[Sandbox provider]
    provider[EnvironmentProvider]
    bundles[Environment-backed bundles]

    virtual --> provider
    local --> provider
    process --> provider
    sandbox --> provider
    provider --> bundles
```

Current state:

- Virtual provider and local provider foundations are landed.
- File read/write/list/glob/grep policies are landed.
- Process-capable shell traits, handles, and deterministic tests are landed.
- Sandboxed provider implementation and aligned filesystem/shell path spaces remain active work.

## Skill Integration

Skills load from configured roots through provider file operations. Current SDK support includes:

- `SkillPackage`, `SkillSourceScope`, `SkillRegistry`, `parse_skill_markdown`, and `skill_tools()`.
- Virtual-provider scan tests and metadata preservation.
- Summary toolset generation and activation metadata.

Remaining work:

- CLI startup seeding for bundled skills and subagents.
- Shared `~/.agents` discovery/import options for Starweaver skill and subagent roots.
- Exact precedence tests for shared user, tool-specific user, shared project, and tool-specific project roots.
- Public `list_skills`, `load_skill`, and `reload_skills` tools over the active provider-visible skill cache.
- Hot reload at request boundaries in development profiles.
- Skill tool-requirement materialization and activation telemetry.
- Remote skill registry sync after local/project/global behavior stabilizes.

## Subagent Integration

Current support includes serializable subagent configs, frontmatter parsing, inherited tools, denied tools, optional/required/auto-inherited policies, lifecycle events, trace parent propagation, nested delegation guardrails, optional async model-visible `delegate`, hidden blocking backend, bounded wait, and in-process result delivery.

Remaining work is governed by `06-async-subagent-execution.md`:

- A product-lifetime supervisor that owns child task/control handles instead of detached per-runtime work.
- Distinct `SubagentAttemptId` allocation for post-terminal conversation continuation, retention across waiting/checkpoint resume, per-attempt notification identity, and bounded result retention.
- Model-visible steer/cancel, cancellation/resume propagation, shutdown drain, and host completion callbacks.
- Serialized delta context/usage merge under concurrent child completion.
- TUI async-only, one-shot headless blocking, worker-disabled, and durable RPC continuation profiles.
- Subagent model/settings/config overrides aligned with Starweaver config.
- Self-fork behavior for current-context child agents.
- Lifecycle stream evidence for accepted, started, waiting, steered, cancel-requested, completed, failed, cancelled, resumed, and delivered work.

## Media Processing

Current landed media foundations:

- `ContentPart::Binary`, `ContentPart::ResourceRef`, and `ContentPart::DataUrl`.
- Data URL parsing, content-type detection, media policy, preflight evidence, and corruption evidence.
- Provider mapping tests for multimodal content.
- SDK media preflight processor and upload adapter seam.

Remaining media migration work:

- Base64-budget-aware compression for static images.
- Alpha compositing before JPEG conversion.
- Tall screenshot splitting with overlap.
- Animated GIF retention and support filtering.
- Newest-media count limits across user messages and nested tool returns.
- S3 protocol and provider resource-store upload adapters.
- Binary/resource download integration with `EnvironmentProvider` resource traits.
- Concrete fallback media understanding clients with usage accounting.

## Agent Framework Design Map

| Design area               | Starweaver shape                                                                                                                           |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| capability middleware     | `AgentCapability` IDs, ordering, wrappers, per-run instances, deferred/on-demand loading                                                   |
| deferred tools            | SDK request/result records layered over durable approval/deferred storage                                                                  |
| RunContext breadth        | unified run context façade over `AgentContext`, `ToolContext`, run state, trace, usage, approval, available tools, and loaded capabilities |
| toolset combinators       | wrapper toolsets that transform discovery and execution                                                                                    |
| AgentSpec schema          | generated schema, templates, dependency schema, capability specs, host-policy materialization                                              |
| UI adapter trust boundary | sanitize client history, file URLs, dangling tool calls, system prompts, and download modes                                                |
| model wrappers            | fallback, concurrency limit, instrumentation, provider lifecycle                                                                           |
| advanced output           | multiple outputs, native/prompted/image modes, streamed structured output helpers                                                          |

## Review Gate

Before implementing the next SDK batch:

1. Update the spec that owns the changed contract with status, owner, and validation command.
2. Add a focused test for the behavior before broadening public API.
3. Keep docs examples compiling through `make docs-check` when user-facing examples change.
4. Keep capability/toolset additions aligned with durable session and stream contracts.
