# Subagents, Environments, Skills, and Media

## Scope

This document lists only remaining non-aligned behavior for subagents, environments/resources, skills, media, tasks/notes, and host adapters.

## Subagent Gaps

- Markdown `SubagentSpec` can now project into `AgentSpec` plus inheritance policy, and `SubagentConfig::from_agent_spec` materializes executable child agents through `AgentSpecRegistry`.
- Hook/capability inheritance is declarative through `SubagentCapabilityInheritancePolicy` and projected markdown metadata (`inherit_hooks`, `inherit_capabilities`, and `denied_capabilities`).
- `AgentSpec` subagent materialization applies selected toolsets, selected subagents, registered skill roots, registered capability bundles, approval presets, child-owned environment providers, declarative hook/capability inheritance, built-in toolset wrappers (`approval_required`, `deferred`, `dynamic`, `dynamic_search`, `filtered`, and `renamed`), and host-defined toolset wrapper factories. `AgentSpec::runtime_builder` can bind registered environment providers for owned runtimes.
- `SubagentExecutionHook` wraps delegated child runs with typed metadata, mutable child context access before execution, and completed/failed outcomes carrying output, run id, and usage.
- Denied required inherited tools are covered in live delegation failure tests with `subagent_failed` metadata diagnostics.
- Multi-level nested delegation is covered by `sdk_subagent_registry_supports_multi_level_nested_delegation`, including child-to-grandchild lifecycle events and nested stream source attribution.
- Parent streams now include blocking child stream records with source attribution, but not true real-time child interleaving while the child run is still executing.

Required direction:

- Add queue ownership only if true real-time child interleaving is adopted as an SDK contract.
- Add real-time child interleaving tests only if non-blocking child streams become an SDK contract.

## Environment And Resource Gaps

- `EnvironmentProviderFactoryRegistry` restores exported provider state by metadata kind; built-in portable defaults cover virtual providers and provider-scoped `ResourceRef` values, and trusted local restore is available through an explicit host policy factory.
- `ResourceRestoreFactoryRegistry` restores typed external `ResourceRef` values by `resource_kind` before provider restore, while preserving untyped provider-scoped references.
- Virtual provider process snapshots are exported and restored through `EnvironmentState`, and process-capable shell tools use `ProcessShellProvider` handles.
- No unified `Environment` object equivalent with setup/teardown/fork lifecycle.
- No full `ResourceRegistry` equivalent for live resource handles, resource-provided toolsets, setup/teardown, fork, and close lifecycle.
- Concrete restore adapters for browser sessions, remote storage objects, and media artifacts are not productized. Live OS process reattachment is intentionally host-owned because process identity, permissions, and lifetime are not portable.
- Composite environments and sandbox/workspace provider factories are not first-class SDK APIs.
- Local provider restore is trusted-host only; cross-host portability requires a resource abstraction.

Required direction:

- Add concrete resource adapters for browser, remote storage, and media resources.
- Add adapter-backed resource state restore tests for browser, remote storage, and media resources.
- Add setup/teardown/fork lifecycle only after provider ownership rules are clear.

## Media Gaps

- The SDK media upload filter supports host upload adapters, provider-scoped `ResourceRef` replacements, and failure diagnostics that keep original inline media.
- `AgentResult::media_outputs()` and `image_outputs()` expose generated `ModelResponsePart::File` outputs through `OutputMedia` wrappers.
- S3/resource-store upload adapters are not complete product APIs because external resource ownership, retention, and restore policy are not stable.
- Generated media outputs are not integrated with external durable resource records beyond provider-returned URLs/resource URIs.

Required direction:

- Add external durable media resource records and replay tests once resource ownership is stable.
- Add provider capability matrix tests for image, audio, video, document, and file outputs.

## Task, Notes, And Host Adapter Gaps

- Host adapters exist as traits and handles, and `agent_runtime_builder_runs_host_search_adapter` proves a host-provided search handle can run through `AgentRuntimeBuilder`.
- Browser, document, and external host resources need resource-state contracts before parity can be claimed.

Required direction:

- Add browser, document, and external host resource-state contracts once provider ownership rules are clear.
