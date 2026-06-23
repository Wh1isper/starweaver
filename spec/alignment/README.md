# Readiness Review

## Source Snapshots

| Source                   | Local path        | Commit | Role                                                                                                  |
| ------------------------ | ----------------- | ------ | ----------------------------------------------------------------------------------------------------- |
| Core agent baseline      | internal snapshot | pinned | Agent loop, messages, models, tools, output, usage, retries, streaming                                |
| Application SDK baseline | internal snapshot | pinned | Agent construction, streaming, context/state restore, environments, toolsets, HITL, subagents, skills |

## Method

This directory records remaining behavior gaps, product decisions, and verification gates that still need work.

```mermaid
flowchart TD
    core["Core agent baseline"]
    sdk["Application SDK baseline"]
    star["Current Starweaver workspace"]
    docs["spec/alignment remaining gaps"]

    core --> docs
    sdk --> docs
    star --> docs
```

## Documents

| File                                        | Remaining non-aligned area                                               |
| ------------------------------------------- | ------------------------------------------------------------------------ |
| `01-agent-core-abstractions.md`             | Core abstraction decisions and provider replay status                    |
| `02-agent-sdk-surface-parity.md`            | Application SDK construction and streaming API gaps                      |
| `03-runtime-context-session-streaming.md`   | runtime, context, session, live stream, durable session gaps             |
| `04-tools-toolsets-hitl.md`                 | tool metadata, HITL, MCP, and event taxonomy gaps                        |
| `05-models-output-provider-alignment.md`    | provider replay evidence, output exactness, usage, and media-output gaps |
| `06-subagents-environments-skills-media.md` | subagent, environment/resource, and media workflow gaps                  |

## Remaining Theme

The remaining asymmetry is mostly exact SDK contract parity. Execution order
belongs in the implementation batch that picks up a concrete gap; completed
roadmaps are not kept as active planning documents.

- Python-style decorator syntax is intentionally mapped to Rust-native builders; multi-output selector ergonomics are not yet mirrored.
- External resource adapter breadth remains narrower than the rest of the SDK; MCP stdio, streamable HTTP, session reinitialization, and protocol-level HITL/deferred paths have direct `rmcp` evidence.
- Durable SDK HITL orchestration, live interruption recovery, provider stream resume, replay-cursor transport, and typed resource restore seams now have store-bound service-level evidence; concrete external resource adapters remain product-owned.
- Subagents now materialize from `SubagentSpec`/`AgentSpec` projections for executable child agents, including registered skill roots, capability bundles, approval presets, child-owned environment providers, declarative hook/capability inheritance, built-in/deferred toolset wrappers, host-defined toolset wrapper factories, and typed `SubagentExecutionHook` callbacks, while product-owned built-in subagents are explicit registry entries rather than an implicit flag.
- Provider replay coverage now proves known provider-private continuation payloads for OpenAI Responses and Anthropic; future adapters must add same-provider private replay fixtures before claiming parity.
- Future non-HTTP/SSE provider cancellation/resume adapters, true real-time subagent stream interleaving, older standalone MCP SSE support, optional MCP roots/logging/completions/notifications host contracts, browser/remote-storage/media resource adapters, and live OS process reattachment need product-level APIs.
