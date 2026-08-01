# Repository Guidelines

## Repository Overview

`starweaver-agent-sdk` is a Rust workspace for building Starweaver, an agent SDK with CLI tooling and planned runtime/platform capabilities.

Implementation boundary rule:

- **Desktop maintenance freeze:** `apps/starweaver-desktop`, Desktop-specific generated bindings, `spec/desktop`, and Desktop packaging/update automation are retained as WIP reference code but are not actively maintained. Do not implement, refactor, fix, test, build, package, publish, regenerate, or update Desktop-specific code, documentation, specs, or automation unless the user explicitly lifts this freeze. Keep all disabled Desktop CI and release jobs disabled. Cross-cutting work must exclude `starweaver-desktop` and must not depend on Desktop passing. Shared protocol source and core RPC work may continue, but do not update frozen Desktop projections as part of that work.
- Starweaver Desktop's eventual first release is the local product itself, not a dogfooding edition. Use local product, local release, or prerelease terminology as appropriate; do not label the product or its UX as dogfood/dogfooding.
- Code, tests, public symbols, module names, and observable IDs should use Starweaver-native names.
- `starweaver-runtime` owns the agentic loop; `starweaver-context` owns neutral run/session evidence; `starweaver-agent` owns SDK ergonomics and first-party bundles; `starweaver-usage` owns usage accounting, limits, snapshots, and optional USD pricing estimates.
- Durable session/run IDs are generic request metadata (`starweaver.durable_session_id`, `starweaver.durable_run_id`, plus CLI-scoped aliases). Stable provider-routing affinity lives in `AgentContext.session_id` and typed `ModelSettings` provider settings, not generic durable metadata or model HTTP headers. Provider-specific headers such as `session_id`, `session-id`, `thread_id`, `thread-id`, and `x-client-request-id` belong in Codex/OpenAI subscription OAuth code only and must be sourced from typed provider routing settings. Codex routing values preserve short IDs only when they use the safe Starweaver ASCII identity alphabet, and canonicalize unsafe, non-ASCII, or over-64-byte values to stable SHA-256-derived 64-byte-safe IDs before header construction.

Current workspace members:

- `crates/starweaver-core` — shared SDK identity, IDs, metadata, trace context, cooperative cancellation token, product-neutral events and event-kind identifiers, execution-node and run-lifecycle vocabularies, protocol identities, versioned durable-record codecs, subagent specs, and XML helpers
- `crates/starweaver-usage` — usage accounting, snapshot contracts, usage limits, and optional USD pricing estimates
- `crates/starweaver-model` — provider-neutral model messages, settings, profiles, native tool request definitions, protocol clients, injectable HTTP transport, deterministic test models, production-request guard, model wrappers, OAuth-backed provider model adapters, and replay tests
- `crates/starweaver-oauth` — OAuth credential storage under `~/.starweaver`, Codex device login, token refresh, JWT metadata extraction, and store-backed token sources
- `crates/starweaver-oauth-provider` — OAuth-backed provider helpers, Codex model construction helpers, and refresh supervisor utilities
- `crates/starweaver-context` — AgentContext, explicit agent-tool state, checkpointable run state, versioned checkpoint records, executor callback contracts, explicit runtime-ephemeral state, narrow tool runtime snapshots, read-only host capability views, typed dependencies, resumable state, state store, product-neutral event-bus integration, message bus, and usage ledger
- `crates/starweaver-runtime` — core agent loop, graph state machine, typed request-phase transitions, static and dynamic instructions, semantic retry, tool execution over provider-neutral tool schema, per-tool retry budgets, approval/deferred control-flow recording, prepare-tools hooks, structured output, typed structured output parsing, output functions, message history continuation, history processors, system prompt reinjection, usage-limit enforcement, typed usage snapshot events, typed stream emission, safe public error projection, scoped overrides, context integration, capability hooks, capability bundles, trace recording, checkpoint emission, direct executor behavior, and compatibility re-exports
- `crates/starweaver-tools` — function tool schema, prefixed tools/toolsets, MCP toolset foundations, tool metadata, retry budget metadata, approval/deferred control-flow metadata, tool registries, toolset combinators, and execution primitives
- `crates/starweaver-agent` — ergonomic SDK facade, `AgentBuilder`, `AgentApp`, SDK-level subagent registry, first-party tool bundles, spec presets, session helpers, media/filter helpers, and application-facing helpers
- `crates/starweaver-environment` — `EnvironmentProvider`, virtual and local provider foundations, file and shell policies, resource references, environment state snapshots, and envd-backed provider adapters
- `crates/starweaver-computer-use` — typed current-active-desktop service/state machine, canonical tool schemas/router, deterministic fake, macOS pixel, bounded Accessibility, and pointer/keyboard backend, explicit unsupported backends, and optional feature-gated `starweaver-computer-use-mcp` stdio binary
- `crates/starweaver-envd-core` — runtime-neutral envd service trait, DTOs, protocol identity, JSON-RPC frame helpers, state descriptors, and error mapping
- `crates/starweaver-envd-client` — stdio/http `EnvdRpcClient` over the shared envd service interface
- `crates/starweaver-envd` — `LocalEnvd`, local ephemeral envd state, JSON-RPC dispatcher, stdio/http server transports, and standalone `starweaver-envd` binary
- `crates/starweaver-session` — shared durable session contracts for canonical input parts, family-aware stream cursor refs, `SessionStore` traits, versioned session/run records, typed atomic terminal status/output/diagnostic projections, product-neutral durable host-event/outbox records and filtered replay positions, fenced background-subagent execution, run-aware delivery release/reconciliation, integrity-checked quota-bounded artifact retention, typed continuation causes and atomic admission, resume snapshots, approvals, deferred records, and compact trace projections
- `crates/starweaver-stream` — typed raw agent stream records, source attribution and sinks, shared display/replay contracts, family-aware replay cursors, replay event logs, replay transports, realtime compaction buffers, stream archives, UI adapters, sanitization, and protocol envelopes
- `crates/starweaver-storage` — canonical shared SQLite migrations, atomic evidence domain operations, canonical durable host-event log/outbox materialization, concrete `SessionStore`, replay event-log, stream archive adapters, typed query facade, and migration status reporting
- `crates/starweaver-cli` — independent CLI/TUI product surface for headless runs, terminal interaction, display rendering, session restore, launcher dispatch, direct environment/envd connectivity, and install/update workflows
- `crates/starweaver-rpc-core` — target generated single `starweaver.host` major-1 JSON-RPC wire boundary plus narrow framing/projection helpers; the IDL atomically replaces handwritten DTO, registry, and fixture authority
- `crates/starweaver-rpc` — independent standalone JSON-RPC host product for local and external host integrations, owning `rpc.toml`, AgentSpec/profile/model materialization, handlers, coordination, authorization, subscriptions, environment attachments, and transports; it implements the generated single-contract server boundary with no retained legacy dispatch
- `apps/starweaver-desktop/src-tauri` — frozen WIP Tauri 2 Desktop shell retained for reference; its CI, packaging, release, and active maintenance are paused
- `packages/starweaver-py` — in-process Python SDK bindings, Python tool injection, live `AgentRun` control, message bus facades, typed HITL helpers, deterministic test models, sessions, stream records, and Python distribution artifacts

Planned areas live in `spec/` until their responsibilities, integration points, and validation paths are clear:

- Agent platform capabilities (`starweaver-platform`)

## Layering Rules

- `starweaver-core`: shared SDK identity, IDs, metadata, trace context, cooperative cancellation token, product-neutral events and event-kind identifiers, execution-node and run-lifecycle vocabularies, protocol identities, versioned durable-record codecs, subagent specs, and XML helpers.
- `starweaver-usage`: leaf crate for usage accounting, usage snapshot contracts, usage limits, and optional `pricing` feature helpers. Pricing estimates use fixed-point micro USD in `PricingEstimate::amount_micros_usd`.
- `starweaver-model`: provider-neutral model protocol, settings, profiles, transports, model wrappers, provider request mapping, and OAuth-backed provider adapters.
- `starweaver-oauth`: OAuth auth file storage, Codex device-code login, token refresh, and store-backed token sources. Default auth path is `~/.starweaver/auth.json`.
- `starweaver-oauth-provider`: OAuth provider construction helpers and proactive refresh supervision.
- `starweaver-tools`: tool schema, toolsets, metadata, tool context, combinators, and protocol-level tool execution primitives.
- `starweaver-context`: AgentContext, explicit agent-tool state, checkpointable run state, checkpoint/executor callback contracts, explicit runtime-ephemeral state, narrow tool runtime snapshots, read-only host capability views, typed dependencies, resumable state, state store, event bus, message bus, and usage ledger.
- `starweaver-runtime`: core agent loop, explicit typed phase transitions, tool loop, output loop, capabilities, usage-limit enforcement, usage snapshot publication, stream and checkpoint emission, safe public projection of typed runtime/model errors, direct executor behavior, trace spans, and compatibility re-exports.
- `starweaver-agent`: SDK ergonomics, tool implementation bundles, subagent protocols, application wrappers, filters, media helpers, and policy presets. New stable imports belong in `starweaver_agent::prelude`; advanced contracts use explicit owning-layer namespaces, while root re-exports are a 0.x compatibility facade.
- `ask_user_question` is main-agent-only. Subagent inheritance must reject it when required, strip it from optional/inherit-all paths, and deny it again after each child agent's final static, dynamic, and capability tool preparation.
- First-party tool bundles use Filtered dependency requirements. Strict tools receive only requested authority intersected with the host-installed per-tool `ToolCapabilityGrant`; named `HostCapabilities`, shell projection, and capability-specific mutable handles are deny-by-default. Never add a new broad mutable context handle when a narrow grant can own the operation.
- `starweaver-environment`: environment provider contracts, file/shell policy, resource references, resumable environment state snapshots, and `EnvdEnvironmentProvider`.
- `starweaver-computer-use`: one process-local typed service, canonical eight-tool schemas/router, deterministic fake, macOS pixel observation, bounded Accessibility snapshots, and high-level pointer/keyboard input, with explicit unsupported Windows/Linux backends. Status and model-visible tools never grant native permission. CLI/RPC opt-in may request Screen Recording once on first open and Accessibility/post-event onboarding once on first accessibility-enabled observation; post-event and AX trust are requested and re-probed independently. MCP stdio never prompts implicitly, while `--request-permissions` requests Screen Recording, post-event, and AX trust and treats the immediate result as authoritative. Accessibility traversal is budgeted, protected values are redacted, content is untrusted/non-durable, and native identifiers never leave the backend. The reviewed `platform::macos_accessibility` module owns audited `objc2`/Core Foundation retain-and-cast work, `platform::macos_input` owns the sole audited unsafe `CGEventKeyboardSetUnicodeString` call, and `platform::macos_session` owns the typed `CGSessionCopyCurrentDictionary` cast and numeric conversion used by the continuous 10-millisecond lock/console transition sampler; unsafe Rust remains denied outside those narrow macOS modules. CLI and RPC compose the opt-in Toolset in-process, never through MCP. `[computer_use].enabled = true` injects the full canonical family into every effective profile and maintained direct mode uses `InputApprovalPolicy::Never`. RPC additionally requires ordinary caller/run authorization, expiry, cancellation, and revocation; there are no transport-specific observe settings or per-pointer/per-keyboard principals. Launching the standalone stdio MCP server likewise opts into the full canonical family; there are no `--allow-pointer`/`--allow-keyboard` gates. macOS Screen Recording and Accessibility/post-event permission, fingerprinted plus continuously sampled active same-user unlocked-session generation, geometry/stale-basis validation, periodic long-action fences, backend-owned execute/close serialization, idempotency, retained held-input cleanup with before/after permission probes, receipt-preserving cancellation, and lifecycle revocation remain authoritative. Signing/notarization affects TCC continuity and OS warnings, not input availability. Native calls use owned supervisor tasks behind one serialized backend gate; cancellation/timeout has bounded cleanup and abandonment poisons the lifecycle before reuse. No Starweaver graphical Desktop product is assumed. The library does not extend `EnvironmentProvider` or depend on model-native tools, browser/CDP, runtime, RPC, CLI, graphical products, remote sessions, or unattended execution.
- Each environment provider owns one scratch area shared by ordinary file operations and its shell. Local providers use fallible creation plus last-owner RAII cleanup, never persist the physical scratch root as configured authority, and return absolute provider-visible paths without a workspace alias. They prefer an exclusive child of the OS temp directory; if that creation fails, they safely initialize `.starweaver/tmp/.gitignore` and use an exclusive `<workspace>/.starweaver/tmp/<instance-id>` child while leaving the shared fallback root in place. Construction never scans or removes sibling fallback instances; stale cleanup requires a separate ownership/liveness contract. Virtual providers use `.starweaver/scratch`. Composite providers prefer the default shell mount and reject ambiguous absolute-path routing. CLI/TUI and RPC normally use OS temp; a separate CLI grant for the temp root is ordinary file authority, not ownership of that root.
- `starweaver-envd-core`: runtime-neutral envd service protocol, DTOs, state descriptors, JSON-RPC frame helpers, and error mapping.
- `starweaver-envd-client`: stdio/http envd client implementing the shared envd service interface.
- `starweaver-envd`: local envd implementation, ephemeral local state, service dispatcher, stdio/http transports, and standalone envd binary.
- `starweaver-session`: shared durable session contracts for input parts, `SessionStore` traits, session/run records, typed atomic terminal status/output/diagnostic projections, product-neutral durable host-event/outbox records and filtered replay positions, fenced background execution, run-aware delivery dispositions, artifact evidence, typed continuation causes and atomic admission, resume snapshots, approvals, deferred records, and compact trace projections.
- `starweaver-stream`: typed raw agent stream records, source attribution and sinks, display/replay stream contracts, UI adapters, sanitizers, realtime compaction buffers, stream archives, and protocol envelopes.
- `starweaver-storage`: canonical SQLite migrations, atomic evidence domain operations, canonical durable host-event log/outbox materialization, typed query facade, concrete `SessionStore`, `StreamArchive`, and `ReplayEventLog` adapters, plus migration status reporting.
- `starweaver-cli`: independent command-line/TUI product surface and local automation entry point; it must not host or depend on the RPC product. Configured MCP declarations are exposed as namespaced `<server>_<tool>` toolsets by default; `tools.mcp_mode = "proxy"` selects the fixed `mcp_search_tool`/`mcp_call_tool` surface.
- The sole host protocol is the atomically replaced `starweaver.host` major 1: strict JSON-RPC 2.0 with `protocol/host/` OpenRPC 1.4.0/JSON Schema Draft 7 as the only structural wire source. It uses non-empty string client request IDs, required object params, canonical decimal-string 64-bit values, explicit supported/required feature negotiation, root-registered typed public errors, one durable replay/live `EventRecord` vocabulary with atomic state/outbox publication, opaque scope/view-bound cursors, and identical feature/authorization eligibility. Initialization requires exact name, major, non-ordered revision, and schema digest and fails closed on any mismatch. Maintained generation emits the public bundle, core-only manifest, and Rust server bindings through `make rpc-idl-core-generate`; Desktop bridge/client projections are frozen and excluded. Complete external TypeScript bindings are generated and checked independently with `generate-rpc-typescript --output <directory>` and `check-rpc-typescript`; they are not a maintained workspace package, tracked default output, manifest entry, or release artifact. No generated language surface may become a competing definition. Handwritten DTOs and old fixtures are behavioral inventory only and are removed in the same replacement; there is no old-wire compatibility, fallback parser, major router, or deprecation window.
- `starweaver-rpc-core`: generated major-1 JSON-RPC wire types, validators, server trait, dispatcher, and narrow framing/projection helpers; product behavior does not move into this crate, and no handwritten or legacy contract remains authoritative.
- `starweaver-rpc`: independent standalone JSON-RPC host product and implementer of the generated single-contract server boundary; it owns `rpc.toml`, profile/model materialization, handlers, and active-run state, must not depend on CLI, and independently connects to shared storage, environment, and envd abstractions.
- Starweaver Desktop is a local-only Codex App-like graphical and lifecycle client for one long-lived local `starweaver-rpc` stdio child. It never starts one process per workspace. Its start surface opens an existing folder, creates an empty folder, or creates a retained managed temporary workspace; the privileged backend registers that root with the host, `session.create` binds to the returned opaque workspace ID, and RPC owns all workspace/session/agent/run lifecycles and durable evidence. Durable session workspace provenance is distinct from the host-local live grant: historical paths/IDs never recreate authority, and another product must explicitly regrant or record typed rebind drift. Desktop execution requires the exact generated host major-1 revision/schema digest and manifest-filtered safe TypeScript bindings behind a Tauri 2 privileged Rust backend. The backend retains local process transport, native folder grants, request and idempotency identity, session/workspace routing, replay recovery, configuration, safe projection, and authority; the renderer must not import the complete host model, send arbitrary RPC/params, provide supervisor-owned fields, or submit unrestricted paths. Native conversation windows are backend-routed views over the same process-owned supervisor: Rust mints their labels and fixed URL, binds each label to one opaque session, applies a separate least-authority role, replaces subscriptions only for the same window/run, and permits different windows to replay the same run independently. ID-based interaction operations require the owning session and RPC verifies it against the durable record; workspace pages are backend-filtered to the routed session's live grant; same-session hydration epochs prevent stale snapshots from reopening submission or replacing newer run state. Durable run status is separate from `run.status.controllableByCurrentHost`, which derives only from the current RPC coordinator's process-local active registry. `session.get` uses storage-bounded recent-run reads; older run history uses `run.list` with immutable sequence keysets and backend-minted Desktop page tokens, never renderer-visible host cursors. Hidden renderer views release run subscriptions and resume only after a visibility-triggered durable refresh. Desktop application config, immutable RPC bootstrap config, and reloadable RPC runtime config are separate versioned planes. Desktop may edit runtime settings only through typed safe `config.get`, `config.validate`, `config.update`, `config.reload`, `config.activate`, and `config.discard` host operations; RPC remains the authoritative validator/persister, active runs pin durable immutable config snapshots, and bootstrap-only changes require a supervisor-fenced restart. Renderer intent alone never grants config admin authority: security/destination-changing mutations require native user presence or managed policy bound to the exact candidate fingerprint. Model/provider credentials remain host-local and Desktop has no provider login/token API requirement. Desktop must not link CLI/RPC host/agent/runtime/storage implementations, parse CLI-private config, directly read/write `rpc.toml`, SQLite, or credential files, expose process transport to the renderer, or send arbitrary renderer RPC. Local process sharing is not an OS sandbox, so native local shell remains disabled by default without an enforceable sandbox. Every Desktop package includes an exact RPC bootstrap/fallback. Linux AppImage packaging must set `NO_STRIP=1`, restore the exact fallback sidecar after `linuxdeploy` RPATH patching, repack with the digest-pinned output plugin, and sign only the final bytes; macOS updater packaging must build both the Tauri `app` target and DMG. Packaged-sidecar execution proof covers both transports where stable; Windows packages run the full stdio/replay/live/typed-error proof while separate Windows workspace gates retain standalone HTTP coverage. The privileged backend supports independently published RPC updates only when the project signature, exact host revision/schema, exact Rust target triple, launch schema 1, Desktop semver range, asset digest/size, isolated initialize probe, and storage generation 1 all match; installation changes the next-start selection and rollback chooses a previous verified or bundled runtime without interrupting the current host. Desktop itself uses the Rust Tauri updater with a fixed Starweaver endpoint, backend-retained candidate, mandatory project signature, native confirmation, coordinated RPC shutdown, installation, and restart. Apple Developer ID/notarization and Windows Authenticode remain unconfigured; checksums/provenance and documented per-application warning bypass do not replace either OS signing or update signatures. SSH is outside the Desktop product; a future remote helper must remain an independent integration unless a new explicit architecture decision changes this boundary.
- Supervised RPC startup must not apply pending migrations to an existing canonical database. The current RPC path preflight rejects a database observed to be out of date before `SqliteStorage::open`; a schema-changing Desktop update must not ship until a storage-owned atomic supervised open/create plus product-neutral coordinated maintenance barrier replaces that guard and eliminates the remaining check/open race. Standalone CLI/RPC startup retains its existing explicit migration behavior. A not-yet-created supervised database may be initialized normally under the final atomic contract.
- RPC transport threads own framing, authorization, request order, response writes, and flush barriers. Startup reconciliation, request dispatch, subscription tails, and coordinated shutdown execute on the RPC-owned Tokio runtime with an explicit worker-stack budget; blocking service entry points must not run on those runtime workers.
- `starweaver-platform`: hosted orchestration and external protocol adapters such as A2A and AGUI.

## Documentation Workflow

Use `docs/` for user-facing guides and examples. The docs site is built with mdBook from `book.toml`, `docs/SUMMARY.md`, and focused topic pages. Keep `docs/nav.json` aligned for repository tooling that consumes the docs map.

Documentation maintenance rules:

- Keep examples concise, complete, and runnable.
- Put Rust examples in fenced `rust` blocks.
- Use hidden `# async fn example() -> Result<..., ...>` wrappers for async examples so `make docs-check` can compile them.
- Run `make docs-check` after changing docs examples.
- Run `make docs-build` after changing the docs site structure, mdBook configuration, sitemap generation, or deployment metadata.
- Update `docs/SUMMARY.md` and `docs/nav.json` when adding, removing, or renaming docs pages.
- Keep `docs/` user-facing and keep architecture decisions in `spec/`.
- Prefer mermaid diagrams for architecture flows.

Current docs:

- `docs/index.md` — overview and documentation map
- `docs/install.md` — install and local validation
- `docs/quickstart.md` — first agent, tools, structured output, sessions, and CLI run
- `docs/agent-sdk.md` — SDK surface, layers, capabilities, bundles, and entry points
- `docs/python-sdk.md` — in-process Python SDK, Python tool injection, live run steering, message bus helpers, typed HITL, sessions, stream records, and deterministic Python tests
- `docs/agent.md` — agent builder and result basics
- `docs/models.md` — test models, function models, and production-request guard
- `docs/direct.md` — direct model, stream, and tool APIs
- `docs/tools.md` — function tools, registries, toolsets, and retry metadata
- `docs/codeact.md` — constrained JavaScript composition, Strict tool eligibility, recipes, limits, and CLI/RPC defaults
- `docs/output.md` — structured output schemas and typed parsing
- `docs/message-history.md` — history continuation and new messages
- `docs/dependencies.md` — typed dependencies in context and tools
- `docs/capabilities.md` — runtime capability hooks
- `docs/graph.md` — graph inspection and iteration trace
- `docs/durability.md` — executor checkpoints
- `docs/sdk-app.md` — `AgentApp` usage
- `docs/subagents.md` — SDK-level subagent delegation
- `docs/mcp.md` — MCP foundations and official `rmcp` direction
- `docs/computer-use.md` — macOS observe, pointer, keyboard, and bounded Accessibility support, CLI/RPC opt-in, permissions, external MCP use, and platform limits
- `docs/testing.md` — deterministic testing, request guard, scripts, and coverage
- `docs/release.md` — release, upversion, crate publishing, and binary artifact workflow
- `docs/session-stream.md` — shared session, display stream, replay, and storage contracts
- `docs/session-search.md` — pluggable discovery contracts, local bounded search, CLI, and RPC usage
- `docs/session-management.md` — agent-facing session query/control tools and product policy

## Spec Workflow

Use `spec/` for product and architecture decisions before introducing new crates or public APIs. Use `spec/alignment/` for implementation evidence, readiness notes, and prioritized gap tracking.

Current specs:

- `spec/README.md` — architecture baseline map and design rules
- `spec/core/README.md` — core scope, contracts, and acceptance gates
- `spec/core/01-agent-loop.md` — deterministic run loop, graph states, retries, streaming, and durable execution seam
- `spec/core/02-model-provider-replay.md` — provider-neutral model protocol, replay fixtures, transport, settings, profiles, and CI gates
- `spec/core/03-tools-output-capabilities.md` — tool schema, tool loop, structured output, output functions, validators, hooks, and capability bundles
- `spec/core/04-context-state-executor.md` — AgentContext, StateStore, events, messages, notes, usage, checkpoints, and executor preparation
- `spec/core/05-agent-foundation-feature-map.md` — Agent foundation feature coverage map across agents, providers, tools, output, streaming, and testing
- `spec/core/06-message-request-abstractions.md` — Starweaver-native message AST, model request envelope, preparation pipeline, streaming parts, and provider boundary
- `spec/core/07-versioned-protocol-contracts.md` — versioned durable envelopes, canonical input/lifecycle/cursor vocabularies, planned session-workspace/run-config provenance v2 migrations, typed host/envd identities, and cross-release fixture gates
- `spec/core/08-boundaries-and-usage.md` — native runtime/context/SDK/usage boundaries, usage snapshot pricing contract, and cleanup acceptance gates
- `spec/sdk/README.md` — SDK product boundary and application-facing contract
- `spec/sdk/01-agent-sdk-app.md` — AgentBuilder, AgentApp, AgentSession, policy presets, app composition, and docs surface
- `spec/sdk/02-environment-provider.md` — EnvironmentProvider, filesystem, shell, resources, environment state, policies, and sandbox mapping
- `spec/sdk/03-first-party-tool-bundles.md` — filesystem, shell, search, media, task, skill, and tool-proxy bundles implemented through capabilities and context
- `spec/sdk/04-subagents-skills.md` — serializable subagent specs, delegation lifecycle, inherited tools, skills, and nested coordination
- `spec/sdk/05-sdk-integration-map.md` — SDK integration map for agents, context, filters, environment, toolsets, subagents, media, and presets
- `spec/sdk/06-async-subagent-execution.md` — async-only model-visible delegation, steering, cancellation, bounded fan-in, host continuation, durability, and product lifetime policy
- `spec/sdk/07-codeact-tool-composition.md` — constrained CodeAct composition, canonical nested tool invocation, reusable recipes, security, durability, and Computer Use integration
- `spec/computer-use/README.md` — current-active-desktop Computer Use boundary, implemented macOS observe, pointer, keyboard, and bounded Accessibility subset, fixed exclusions, package shape, and reading order
- `spec/computer-use/01-product-boundaries-and-ownership.md` — CLI/RPC in-process and external-harness MCP topology, ownership, dependency direction, lifecycle, and non-goals
- `spec/computer-use/02-service-contract-and-state-machine.md` — typed service, observation/geometry/action contracts, receipts, errors, cancellation, and state machine
- `spec/computer-use/03-toolset-and-library-integration.md` — canonical tool catalog/router, first-party Toolset adapter, capability grants, media mapping, and schema parity
- `spec/computer-use/04-native-active-desktop-backends.md` — macOS, Windows, Wayland, and explicit X11 active-session backends
- `spec/computer-use/05-mcp-binary-and-process-lifecycle.md` — feature-gated local stdio MCP binary, lifecycle, protocol mapping, diagnostics, and packaging
- `spec/computer-use/06-security-testing-and-delivery.md` — active-desktop security, product/run authorization, test matrix, delivery evidence, and release gates
- `spec/environment/README.md` — Starweaver Agent SDK environment layer, ownership rules, provider families, and envd relationship
- `spec/environment/01-sdk-provider-contract.md` — `EnvironmentProvider`, process/shell extension traits, descriptors, capabilities, snapshots, and restore boundary
- `spec/environment/02-tool-binding-and-envd-adapter.md` — environment-backed tool binding, `EnvdEnvironmentProvider`, CLI direct mode, host RPC attachments, and boundary rules
- `spec/envd/README.md` — standalone envd service architecture, ownership rules, implementation shape, and Starweaver reference integration
- `spec/envd/01-service-interface-and-state.md` — envd service trait, environment state, mount state, process state, operation/effect records, and capability model
- `spec/envd/02-implementations-and-modes.md` — local ephemeral mode, implementation-owned state lifecycle, RPC server mode, RPC client mode, and future sandbox/composite backends
- `spec/envd/03-rpc-protocol.md` — JSON-RPC method groups, stdio/http transports, request/response envelopes, errors, streaming, and idempotency
- `spec/envd/04-provider-and-host-integration.md` — reference Starweaver provider adapter, host RPC, session metadata, approval, and dependency boundaries
- `spec/envd/05-api-backlog.md` — unfinished envd API work that should wait for a concrete implementation or call site
- `spec/ops/README.md` — operational layer scope and readiness model
- `spec/ops/00-product-boundaries.md` — normative independence and shared-library boundaries for CLI/TUI, standalone RPC, envd, and shared in-process Computer Use composition
- `spec/ops/01-ci-readiness.md` — replay CI, docs examples, feature coverage matrix, and release acceptance gates
- `spec/ops/02-shared-execution-components.md` — shared session storage/stream contracts, durable workspace provenance versus live grants, and run config snapshot references
- `spec/ops/03-durable-service-runtime.md` — durable sessions, workspace/config provenance, `SessionStore`, stream archive, resume, interruption, service transports, display-message replay, and storage contracts
- `spec/ops/04-cli-product.md` — CLI-first product surface with headless stdio display streams, session restore, direct envd and opt-in in-process Computer Use composition, launcher dispatch, install/update flow, and the planned hardened public RPC component contract
- `spec/ops/05-observability.md` — OpenTelemetry GenAI tracing, Langfuse-friendly OTLP export, nested agent/model/tool spans, and trace-to-session correlation
- `spec/ops/06-json-rpc-host-protocol.md` — Starweaver-owned JSON-RPC host-control protocol, stdio/HTTP transports, typed method/event/error contracts, replay, idempotency, and RPC-owned opt-in in-process Computer Use composition
- `spec/ops/07-session-search.md` — optional product-neutral session search, local SQLite/filesystem discovery, external index ingestion, and independent CLI/RPC integration
- `spec/ops/08-agent-session-management.md` — agent-facing session query/control tools, query-only CLI policy, grant-gated RPC mutations, and lifecycle-safe run creation/steering/interruption
- `spec/ops/09-rpc-idl-and-client-generation.md` — single IDL-first JSON-RPC major-1 contract, unversioned `protocol/host/` source, generated Rust and safe Desktop TypeScript boundaries, exact revision/digest admission, atomic replacement, validation, and planned domain-host workspace/config methods
- `spec/alignment/09-architecture-review.md` — cross-workspace architecture, security, durability, API, and consolidation review baseline
- `spec/alignment/10-session-search-evidence.md` — Phase 1 session-search implementation, conformance, and boundary evidence
- `spec/alignment/11-tui-ui-ux-completion.md` — complete TUI interaction, status, task, history, and validation evidence
- `spec/alignment/12-rpc-host-readiness.md` — RPC host contract, durability, recovery, and interoperability readiness
- `spec/alignment/13-computer-use-macos-evidence.md` — macOS observe, pointer, keyboard, and bounded Accessibility Computer Use implementation, security boundary, release integration, and validation evidence
- `spec/desktop/README.md` — Desktop architecture baseline, ownership map, prerequisites, and delivery phases
- `spec/desktop/01-product-and-process-boundaries.md` — Desktop shell/supervisor boundary, one local stdio RPC host, workspace registry, and lifetime rules; SSH sections are superseded by the local-only baseline
- `spec/desktop/02-rpc-client-and-lifecycle.md` — Desktop RPC handshake, replay/recovery, run/HITL control, and required host protocol additions
- `spec/desktop/03-cli-migration-and-compatibility.md` — shared CLI history, custom database discovery, profile/provider-boundary compatibility, continuation preflight, and version skew
- `spec/desktop/04-workspaces-sessions-and-runs.md` — folder selection/creation, managed temporary workspaces, one-host multi-workspace/session routing, global history, active-run ownership, multi-window behavior, and pagination
- `spec/desktop/05-auth-interaction-and-security.md` — renderer isolation, provider-credential isolation, approvals, clarifying questions, authority scopes, transport bounds, and security gates
- `spec/desktop/06-runtime-updates-and-release.md` — Desktop-managed RPC runtime channel, manifests, staging, storage migration, activation, and rollback
- `spec/desktop/07-ssh-remote-workspaces.md` — superseded SSH design history retained for reference; SSH is outside the Desktop product roadmap
- `spec/desktop/08-configuration-and-reload.md` — Desktop/bootstrap/runtime configuration ownership, typed config editing, atomic reload, immutable run snapshots, and restart-required changes

Use `spec/alignment/` for readiness notes, design comparisons, implementation evidence, and roadmap reminders. Keep unfinished work in the spec that owns the changed contract.

After changing repository structure, workspace boundaries, command behavior, CI, or planned module responsibilities, review and update:

- `docs/*`
- `spec/*`
- `README.md`
- `AGENTS.md`
- `Cargo.toml`
- crate manifests under `crates/*/Cargo.toml`
- `Makefile`
- `.pre-commit-config.yaml`
- `.github/workflows/*.yml`

## Development Workflow

`make architecture-check` enforces maintained product boundaries: `starweaver-cli` and `starweaver-rpc` have no direct or transitive dependency path in either direction, CLI has no direct `rusqlite` dependency, durable session contracts have no normal dependency path or direct dependency of any kind to runtime and no direct environment implementation dependency, shared storage has no normal dependency path to runtime, and stream contracts have no dependency path to runtime or direct dependency on mutable agent context. Desktop-specific dependency checks are paused. The maintained architecture check is included in `make check` and `make scripts-check`.

`make capability-check` validates `spec/capabilities.toml`, including registry/release versions, required capability IDs, workspace owners, normative specs, implementation paths, and contract-test evidence. It is included in `make check`.

Desktop-specific Make targets remain only as frozen local reference entry points. They are excluded from `make check`, `make test`, `make ci`, protocol acceptance, coverage, pre-commit Clippy, release preparation, and release publication. Do not run or maintain them unless the Desktop maintenance freeze is explicitly lifted.

After changing code, run:

1. `make fmt-check`
2. `make check`
3. `make test`

After changing docs examples, run:

```bash
make docs-check
```

After changing docs site structure or mdBook configuration, run:

```bash
make docs-build
```

For focused model/provider replay validation, run:

```bash
make replay-check
```

For full local validation, run:

```bash
make ci
```

`make rpc-contracts-check` remains the complete standalone in-process/stdio/HTTP contract gate. The aggregate `make ci` uses the ordered `rpc-ci-check` composition so workspace tests provide typed in-process coverage before `rpc-integration-check` builds one normal dev-profile CLI/RPC binary pair and reuses it across the stdio/HTTP and bidirectional subprocess gates.

Before a release, also run the Rust semver and classified Python API gate:

```bash
make release-api-check
```

For Python package validation, run:

```bash
make py-check
```

Python packages use `uv` from the repository root. Local development defaults to
Python 3.13 through `.python-version`; Makefile Python targets should keep using
`uv` so they inherit that default. The supported package range is CPython 3.11
through 3.13, and CI must keep 3.11, 3.12, and 3.13 coverage.

Python Makefile targets:

- `make py-sync` — sync the uv workspace dependencies.
- `make py-version` — show the Python interpreter selected by uv.
- `make py-fmt` — format Python files with ruff.
- `make py-lint` — check the uv lock file, run ruff, and run pyright.
- `make py-rust-check` — run fmt, check, and clippy for the PyO3 extension crate.
- `make py-test` — build the native extension in place and run pytest.
- `make py-build` — build Python sdist and wheel artifacts with uv.
- `make py-check` — run the full Python package gate.

For coverage validation, run:

```bash
make coverage-core
make coverage-agent
make coverage-service
make coverage-ci
```

For repository automation, run:

```bash
make scripts-check
```

To ask the assistant to prepare a unified-version release, use GitHub CLI from the repository root:

```bash
gh workflow run prepare-release.yml -f version=X.Y.Z
```

This pushes `release/vX.Y.Z` for review. After the release commit reaches `main`, publish `vX.Y.Z` as a GitHub Release. The `release.yml` workflow runs from the published Release event for maintained core assets, crates.io, and PyPI. Desktop installers and Desktop-managed RPC runtime update assets remain disabled while Desktop is WIP. Core release assets remain immutable: automation refuses name collisions and publishes payloads before checksums rather than using `--clobber`.

Use squash merge only for GitHub pull requests. Do not merge pull requests with merge commits into `main`.

Keep release-event publishing packaging-only. Do not run CI, smoke checks, or publish dry-runs inside `.github/workflows/release.yml`; run validation before merging the release pull request.

For repository-wide hooks, run:

```bash
make lint
```

## Coding Conventions

- Use English for code, documentation, commit messages, and file names.
- Keep workspace metadata consistent across `Cargo.toml`, crate manifests, `pyproject.toml`, package manifests under `packages/*`, `Makefile`, `.pre-commit-config.yaml`, and `.github/workflows/ci.yml`.
- Use `uv` for Python dependency sync, lock validation, test execution, and package builds. Do not add pip/poetry/hatch workflows unless the Python packaging boundary is explicitly redesigned.
- Use the Corepack-pinned pnpm 11 workspace for Desktop frontend dependencies. Keep the 24-hour release-age gate, trust-policy verification, exotic-transitive blocking, strict lifecycle-script review, and committed `pnpm-lock.yaml`; do not add npm/yarn/bun lockfiles.
- Keep Python package support constrained to CPython 3.11 through 3.13, with local and single-version CI defaults on Python 3.13.
- Keep early abstractions minimal and add SDK concepts as concrete needs emerge.
- Treat runtime primitives as first-class: `AgentContext`, typed dependencies, `StateStore`, `EventBus`, `MessageBus`, executor checkpoints, trace context, `SessionStore` contracts, stream contracts, and environment resources.
- Add crates from specs when the boundary has clear responsibilities, call sites, and validation commands.
- Model transport must support injectable HTTP clients, custom headers, extra body fields, endpoint overrides, and audit/gateway routing requirements.
- Model protocol must preserve typed request/response parts, prepared request snapshots, profile-driven message normalization, tool-call argument state, provider details, and structured stream part events.
- Core runtime should prioritize prompt runs, model history, static and dynamic instructions, structured output retry, per-tool retry, capability hooks and bundles, prepare-tools hooks, settings/params forwarding, skip responses, tool execution, explicit tool-call boundaries, checkpoint emission, and OpenTelemetry GenAI span seams.
- SDK and platform layers should deepen tool implementations, official `rmcp` MCP live transports, subagent task protocols, live model delta streams, dependency-aware hooks, durable sessions, service transports, OpenTelemetry GenAI traces, and external protocol adapters.

## AnyCap

This project uses [AnyCap](https://anycap.ai) for multimodal capabilities -- image generation, video generation, music, image/video/audio understanding, web search, web crawling, file storage, and static site hosting. When a task requires any of these, prefer AnyCap over other approaches.

AnyCap evolves rapidly. Your knowledge of commands, models, and options may be outdated. Always update and read the anycap-cli skill before using AnyCap:

```bash
npx -y skills update
anycap status
```

When something goes wrong or you need a capability that seems missing, submit feedback directly:

```bash
anycap feedback --type bug -m "describe the issue" --request-id <id>
anycap feedback --type feature -m "describe the use case"
```
