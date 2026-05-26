# Pre-1.0 Reference Notes

These notes capture reference mapping and phase-specific implementation observations. They are intentionally kept outside `spec/` so the architecture specs can read as Starweaver's own design baseline.

## Reference Mapping

| Reference       | Ideas currently informing Starweaver                                                                                                                                                                                      |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Pydantic AI     | Agent abstraction, provider-neutral model history, model settings, model profiles, tool schema, toolsets, structured output, validators, retries, usage limits, capabilities, history processors, and deterministic tests |
| Pydantic Graph  | Explicit graph loop semantics, node state, dependency separation, replayable execution, and persistence boundaries                                                                                                        |
| ya-agent-sdk    | Lifecycle-wide context, agent assembly, streaming runs, tool bundles, approval policies, subagents, session export/restore, and environment abstraction                                                                   |
| ya-mono runtime | Event bus, message bus, resumable resources, service execution, interruption, and workspace/environment patterns                                                                                                          |
| MCP protocol    | Official `rmcp` SDK, tool discovery/call lifecycle, transports, resources, prompts, sampling, roots, notifications, long-running tasks, and provider-native MCP mapping                                                   |

## Current Implementation Snapshot

- Runtime kernel foundation exists and is covered by local tests.
- SDK facade exists through `AgentBuilder`, `AgentApp`, and subagent registry.
- Docs examples compile through `scripts/check-docs-examples.py`.
- GitHub CI includes docs example validation.
- `make ci` passes in the current milestone.

## Current Gap Map

### SDK Ergonomics

- richer `AgentBuilder` output policy
- `AgentApp` context/session entrypoints
- model/provider presets
- tool bundle registration
- clearer typed output helpers
- scoped test helper polish

### Subagents

- lifecycle events
- timeout/cancellation/retry policy
- parent-child usage and event propagation
- optional durable polling model

### Tool Bundles

- environment abstraction spec-to-code path
- filesystem bundle
- shell bundle
- approval-gated execution policy
- deterministic tool fakes

### MCP

- live client traits
- stdio transport
- HTTP transport
- live discovery and call integration
- resources and prompts
- local test server

### Provider Depth

- more streaming delta coverage
- provider-native tool call chunk handling
- native structured output coverage
- expanded replay fixtures
- provider-specific profile quirks

### Durability and Service Runtime

- session model
- checkpoint persistence adapters
- interruption and resume semantics
- event replay stream
- SSE stream contracts
- A2A and AGUI platform adapter contracts

## Pre-1.0 Cleanup Reminder

Before a 1.0 release:

- remove reference-dependent language from public positioning
- turn phase snapshots into changelog or release notes
- keep specs focused on Starweaver's stable architecture
- keep docs focused on users and API behavior
- keep memos out of published docs unless deliberately curated
