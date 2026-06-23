# First-Party Agent SDK

The SDK layer is the application-facing Starweaver product surface. It integrates the core runtime with sessions, presets, environment-backed tool bundles, subagents, skills, media handling, tool proxying, and policy configuration.

The SDK should feel ready to use while remaining extensible for custom models, tools, environments, and service runtimes.

## SDK Layer Shape

```mermaid
flowchart TD
    app[Application]
    builder[AgentBuilder]
    app_api[AgentApp]
    session[AgentSession]
    runtime[starweaver-runtime]
    context[AgentContext]
    capabilities[Capabilities]
    env[EnvironmentProvider]
    bundles[First-party tool bundles]
    subagents[Subagent registry]

    app --> builder
    builder --> app_api
    app_api --> session
    session --> runtime
    session --> context
    builder --> capabilities
    capabilities --> bundles
    bundles --> env
    app_api --> subagents
    subagents --> session
```

## SDK Responsibilities

- Provide ergonomic builders over the core runtime.
- Provide application sessions with context export/restore.
- Provide policy presets for model, tools, approval, output, streaming, observability, and durability.
- Assemble first-party capability bundles and toolsets.
- Bind environment providers to filesystem, shell, process, resource, and sandbox tools.
- Keep environment-backed bundles implementation-neutral so local, virtual,
  envd-backed, sandbox, and composite providers can share the same tool surface.
- Treat envd as a standalone service/protocol consumed through an SDK adapter,
  not as the SDK environment layer itself.
- Load serializable subagent and skill specs.
- Provide unified delegation and lifecycle events.
- Expose docs and examples for application developers.

## Reference Feature Families

| Feature family       | Starweaver SDK target                                     |
| -------------------- | --------------------------------------------------------- |
| agent construction   | `AgentBuilder` and `AgentApp`                             |
| streaming            | `AgentSession::run_stream` and service streams            |
| context              | `starweaver-context::AgentContext`                        |
| resumable state      | `AgentSession::export_state` and `session_from_state`     |
| lifecycle extensions | capabilities and runtime hooks                            |
| policy filters       | capability bundles with context-aware hooks               |
| environment          | `EnvironmentProvider` and environment-backed tool bundles |
| subagents            | `SubagentSpec`, registry, delegation lifecycle            |
| notes/tasks/bus      | context stores and first-party tool bundles               |
| skills               | serializable skill specs and tool bundles                 |
| tool proxy           | first-party proxy toolset features                        |
| observability        | OTel GenAI spans, Langfuse metadata, trace propagation    |

## SDK Acceptance Gates

- docs examples compile
- SDK session tests pass
- subagent lifecycle tests pass
- environment provider fakes cover file and shell operations
- first-party tool bundles register through capabilities
- runtime kernel behavior remains owned by core crates
