# Starweaver

Starweaver is a Rust agent SDK for local-first agent products. It combines a typed SDK facade,
a deterministic runtime loop, provider-neutral model and tool contracts, structured output,
durable session evidence, first-party environment tools, and a CLI surface.

Use it when you want application code to own the runtime contract instead of hiding the agent loop
behind provider-specific request formats.

```mermaid
flowchart LR
    app["Application or CLI"]
    agent["starweaver-agent"]
    runtime["starweaver-runtime"]
    model["starweaver-model"]
    tools["starweaver-tools"]
    context["starweaver-context"]
    ops["session, stream, storage"]

    app --> agent
    agent --> runtime
    runtime --> model
    runtime --> tools
    runtime --> context
    runtime --> ops
```

## What you can build

- SDK agents with static and dynamic instructions.
- In-process Python agents and Python tool injection over the Rust runtime.
- Provider-neutral model integrations and deterministic test models.
- Typed function tools, toolsets, MCP-backed tools, and host-backed tools.
- Structured JSON output with typed parsing and validation retry.
- Multi-turn sessions with `AgentContext`, usage, dependencies, notes, and resumable state.
- Subagent registries for application-owned delegation.
- Durable runtimes with checkpoints, display streams, replay records, and SQLite adapters.
- Local CLI workflows with profiles, launcher dispatch, install/update, and display JSONL.

## Learning path

1. [Install](install.md): install from release artifacts or run from source.
2. [Quickstart](quickstart.md): build and run your first agent.
3. [Agent SDK](agent-sdk.md): understand the SDK surface and crate boundaries.
4. [Python SDK](python-sdk.md): inject Python tools into the same runtime.
5. [Tools](tools.md): add typed function tools and toolsets.
6. [Structured Output](output.md): return JSON with schemas and typed parsing.
7. [Session and Stream Contracts](session-stream.md): integrate durable product surfaces.
8. [Release](release.md): prepare and publish Starweaver releases.

## Stability

The public surface focuses on SDK foundations, deterministic testing, tool/runtime contracts, local
CLI workflows, and release automation. Some host integration surfaces are intentionally explicit:
live external resources, product-specific service transports, and hosted platform adapters remain
integration points rather than hidden defaults.
