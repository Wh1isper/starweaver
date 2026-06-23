# Starweaver Specs

This directory records architecture and product decisions before APIs, crates, or workflows graduate into stable surfaces.

## Spec Map

Core foundation:

- `core/README.md` — core scope, contracts, and acceptance gates
- `core/01-agent-loop.md` — deterministic run loop, graph states, retries, streaming, and durable execution seam
- `core/02-model-provider-replay.md` — provider-neutral model protocol, replay fixtures, transport, settings, profiles, and CI gates
- `core/03-tools-output-capabilities.md` — tool schema, tool loop, structured output, output functions, validators, hooks, and capability bundles
- `core/04-context-state-executor.md` — AgentContext, StateStore, events, messages, notes, usage, checkpoints, and executor preparation
- `core/05-agent-foundation-feature-map.md` — Agent foundation feature coverage map across agents, providers, tools, output, streaming, and testing
- `core/06-message-request-abstractions.md` — Starweaver-native message AST, model request envelope, preparation pipeline, streaming parts, and provider boundary
- `core/08-boundaries-and-usage.md` — runtime/context/SDK/usage boundaries, usage snapshot pricing contract, and cleanup acceptance gates

SDK layer:

- `sdk/README.md` — SDK product boundary and application-facing contract
- `sdk/01-agent-sdk-app.md` — AgentBuilder, AgentApp, AgentSession, policy presets, app composition, and docs surface
- `sdk/02-environment-provider.md` — EnvironmentProvider, filesystem, shell, resources, environment state, policies, and sandbox mapping
- `sdk/03-first-party-tool-bundles.md` — filesystem, shell, search, media, task, skill, and tool-proxy bundles implemented through capabilities and context
- `sdk/04-subagents-skills.md` — serializable subagent specs, delegation lifecycle, inherited tools, skills, and nested coordination
- `sdk/05-sdk-integration-map.md` — SDK integration map for agents, context, filters, environment, toolsets, subagents, media, and presets

Agent SDK environment layer:

- `environment/README.md` — Starweaver Agent SDK environment layer, ownership rules, provider families, and envd relationship
- `environment/01-sdk-provider-contract.md` — `EnvironmentProvider`, process/shell extension traits, descriptors, capabilities, snapshots, and restore boundary
- `environment/02-tool-binding-and-envd-adapter.md` — environment-backed tool binding, `EnvdEnvironmentProvider`, CLI direct mode, host RPC attachments, and boundary rules

Envd service protocol:

- `envd/README.md` — standalone envd service architecture, ownership rules, implementation shape, and Starweaver reference integration
- `envd/01-service-interface-and-state.md` — envd service trait, environment state, mount state, process state, operation/effect records, and capability model
- `envd/02-implementations-and-modes.md` — local ephemeral mode, implementation-owned state lifecycle, RPC server mode, RPC client mode, and future sandbox/composite backends
- `envd/03-rpc-protocol.md` — JSON-RPC method groups, stdio/http transports, request/response envelopes, errors, streaming, and idempotency
- `envd/04-provider-and-host-integration.md` — reference Starweaver provider adapter, host RPC, session metadata, approval, and dependency boundaries
- `envd/05-api-backlog.md` — unfinished envd API work that should wait for a concrete implementation or call site

Readiness review:

- `alignment/README.md` — review source snapshot, document map, and high-level findings
- `alignment/01-agent-core-abstractions.md` — core agent abstraction inventory
- `alignment/02-agent-sdk-surface-parity.md` — application SDK surface parity against Starweaver SDK surfaces
- `alignment/03-runtime-context-session-streaming.md` — runtime, context, state, message bus, durable session, and streaming alignment
- `alignment/04-tools-toolsets-hitl.md` — tools, toolsets, hooks, dynamic discovery, MCP, approval, and deferred execution
- `alignment/05-models-output-provider-alignment.md` — model settings, profiles, provider mapping, output modes, usage, and replay gates
- `alignment/06-subagents-environments-skills-media.md` — subagents, environments, resources, skills, media, tasks, notes, and host adapters

Operations and products:

- `ops/README.md` — operational layer scope and readiness model
- `ops/01-ci-readiness.md` — replay CI, docs examples, feature coverage matrix, and release acceptance gates
- `ops/02-shared-execution-components.md` — shared session storage and stream protocol contracts
- `ops/03-durable-service-runtime.md` — durable sessions, stream archive, resume, interruption, service transports, display-message replay, and storage contracts
- `ops/04-cli-product.md` — CLI-first product surface, display-message rendering, launcher dispatch, and GitHub install/update flow
- `ops/05-observability.md` — OpenTelemetry GenAI tracing, Langfuse-friendly OTLP export, nested agent/model/tool spans, and trace-to-session correlation
- `ops/06-json-rpc-host-protocol.md` — Starweaver-owned JSON-RPC host-control protocol, stdio/HTTP transport profiles, typed method/event/error contracts, replay subscriptions, projections, and idempotency

## Architecture Shape

```mermaid
flowchart TD
    core[starweaver-core]
    usage[starweaver-usage]
    oauth[starweaver-oauth]
    oauth_provider[starweaver-oauth-provider]
    model[starweaver-model]
    tools[starweaver-tools]
    context[starweaver-context]
    runtime[starweaver-runtime]
    agent[starweaver-agent]
    env[starweaver-environment]
    env_specs[environment SDK specs]
    envd_specs[envd service specs]
    envd_core[starweaver-envd-core]
    envd_client[starweaver-envd-client]
    envd[starweaver-envd]
    session[starweaver-session]
    stream[starweaver-stream]
    storage[starweaver-storage]
    cli[starweaver-cli]
    rpc_core[starweaver-rpc-core]
    rpc[starweaver-rpc]
    platform[future platform adapters]

    usage --> model
    usage --> context
    usage --> runtime
    usage --> agent
    usage --> cli
    core --> model
    core --> tools
    core --> context
    oauth --> model
    oauth --> oauth_provider
    oauth_provider --> agent
    oauth --> cli
    oauth_provider --> cli
    model --> runtime
    tools --> runtime
    context --> runtime
    runtime --> agent
    env --> agent
    env_specs --> env
    envd_specs --> envd_core
    envd_core --> envd_client
    envd_core --> envd
    envd_core --> env
    envd --> cli
    envd_client --> cli
    envd -. local envd attachment .-> rpc
    envd_client -. remote envd endpoint .-> rpc
    session --> agent
    stream --> agent
    session --> storage
    stream --> storage
    agent --> cli
    storage -. future storage convergence .-> cli
    stream --> rpc_core
    rpc_core --> cli
    cli --> rpc
    session --> platform
    stream --> platform
    agent --> platform
```

## Design Rules

- Core crates stay provider-neutral and product-neutral.
- `starweaver-usage` is a leaf accounting crate; usage data and optional pricing are not owned by `starweaver-core` or `starweaver-runtime`.
- Runtime contracts expose stable stream records, checkpoints, usage snapshot events, traces, and capability hooks.
- SDK ergonomics live in `starweaver-agent`; concrete environment resources live in `starweaver-environment`.
- `starweaver-environment` owns the Agent SDK environment provider contracts,
  provider registry, SDK state snapshots, and adapters.
- Envd is a standalone environment service/protocol; Starweaver can consume it
  through an envd-backed provider, and other agent runtimes can use envd through
  their own adapters.
- Envd has runtime-neutral core and client crates; the client must be usable
  without Starweaver's Agent SDK.
- Durable state is split between `starweaver-session`, `starweaver-stream`, and `starweaver-storage`.
- CLI is the current product surface and stays focused on local/headless execution.
- `starweaver-rpc-core` owns shared JSON-RPC frame parsing, standard request/error envelopes, replay cursor helpers, and stream payload projection; `starweaver-rpc` is the standalone local host process and calls the shared RPC server API while deeper method-handler extraction continues.
- Platform adapters graduate from specs after responsibilities, call sites, and validation commands are clear.

## Current Priorities

- Build envd as a standalone environment service with a reusable client crate.
- Keep Starweaver environment integration at the `EnvironmentProvider` adapter boundary.
- Keep unfinished envd API work in `envd/05-api-backlog.md` instead of reviving
  completed implementation plans.
