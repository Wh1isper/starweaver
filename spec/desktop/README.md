# Starweaver Desktop

Status: accepted architecture baseline; cross-platform shell foundation implemented; RPC integration gated

Starweaver Desktop is a native client for local and SSH-hosted standalone Starweaver RPC hosts. It provides a Codex App-like graphical experience without embedding a second agent runtime, copying durable history between execution domains, or turning the CLI into a backend service.

The Desktop product consumes the same versioned host protocol, durable session records, stream contracts, and canonical SQLite storage used by the independent CLI and RPC products. The Desktop shell owns windows, client-side state, local and SSH RPC process supervision, runtime updates, and platform integration. `starweaver-rpc` remains the only Desktop execution backend.

## Decision Summary

- Desktop is a separate product surface, not a mode of `starweaver-cli`.
- Tauri 2 is the default native shell and privileged backend framework; changing it requires an explicit spec amendment after an implementation spike.
- The UI never links `starweaver-runtime`, invokes CLI coordination, or reads SQLite directly.
- Desktop execution requires the IDL-first `starweaver.host` major-2 contract and manifest-filtered TypeScript bridge/client bindings. Its privileged Rust supervisor carries JSON-RPC over child-process stdio locally and over a supervised stdio stream carried by a system OpenSSH exec channel remotely.
- One Desktop backend supervisor manages optional least-authority catalog/control connections and at most one execution RPC process for each execution-domain/canonical-workspace pair.
- RPC processes in one execution domain share that domain's canonical session database but receive distinct workspace roots, process state directories, and versioned public launch envelopes.
- Closing a window does not implicitly terminate an active run. Explicit application quit performs coordinated RPC shutdown.
- Desktop uses origin-scoped RPC replay cursors to recover UI state after a renderer, window, local child, or SSH connection restart.
- Existing CLI history is opened in place within its local or remote execution domain. Copy/import is used only for an explicitly selected non-canonical legacy or custom database; Desktop does not merge domains.
- Cross-product continuation must perform typed materialization preflight; incompatible history is never silently resumed under a different runtime binding.
- RPC gains OAuth and client-capability protocol support instead of exposing credentials to the UI.
- The Desktop supervisor owns a runtime update channel for `starweaver-rpc`. A runtime can be updated independently of the shell only when protocol and storage compatibility gates pass.
- SSH targets are separate execution domains: their RPC, storage, OAuth, model requests, filesystem, and shell remain remote. Desktop uses a hardened public Starweaver component-update contract to provision the remote RPC through the remote user's login shell.
- Remote native shell uses the authenticated remote account's full authority by default and is never represented as workspace-contained. Local native shell remains disabled by default without an enforceable sandbox.
- The accepted major-2 OpenRPC/JSON Schema source generates the Rust server boundary and safe, manifest-filtered TypeScript Desktop bindings; neither generated language surface is a separate protocol source.
- CLI, RPC, and Desktop remain independent products. Shared behavior belongs in the existing product-neutral crates.

## Readiness Baseline

The existing repository provides enough foundation to begin Desktop specification and a constrained internal pilot. It is not yet ready for a public Desktop release:

- CLI and RPC resolve the same canonical database by default;
- both products use `starweaver-storage` migrations and session/stream adapters;
- current-version subprocess tests cover CLI-to-RPC and RPC-to-CLI history and continuation;
- RPC provides typed initialize, session/run control, stream replay/subscription, HITL, environment attachment, startup reconciliation, and bounded shutdown;
- release archives already contain `starweaver-rpc` and publish SHA-256 checksums.

The Desktop release gate still requires the IDL-first host major, generated Rust and safe TypeScript boundaries, OAuth parity, custom database discovery, a public versioned RPC launch envelope, typed continuation preflight, explicit client capability negotiation, receipt-backed mutation recovery, enforceable local shell sandboxing, cross-version storage tests, and a transactional runtime updater. It also requires periodic ordinary-run lease reconciliation, typed clarification answers, correct continuation authorization, atomic state/outbox/event publication, scope/view-bound cursors, feature/authorization-consistent replay and live delivery, gap-free durable subscriptions, bounded stdio framing with terminal-control recovery, non-blocking state I/O, typed safe error projection, and storage-backed pagination.

SSH release readiness additionally requires the system-OpenSSH process boundary, host-key/askpass mediation, static configuration allowlisting, stable remote execution-domain identity, login-shell supervised bootstrap, cross-client execution-host exclusivity, origin-scoped history, partition reconciliation, and the hardened exact-version RPC component installer/update contract in `07-ssh-remote-workspaces.md`. These are planned requirements, not descriptions of current implementation.

## Foundation Implementation Evidence

The repository now contains a non-executing Desktop foundation under `apps/starweaver-desktop/`:

- Tauri 2 Rust shell plus React/TypeScript/Vite renderer in the shared Cargo and pnpm workspaces;
- Linux x86_64, macOS x86_64/ARM64, and Windows x64 target registry and native CI build matrix;
- pnpm 11 lockfile, package-age/trust verification, exotic-transitive blocking, and explicit `esbuild` lifecycle approval;
- application-owned single-instance transports that carry only a fixed activation frame: authenticated session D-Bus on Linux, an advisory-lock-elected private current-user peer-checked socket on macOS, and a peer-verified local named pipe discovered through a random rendezvous in private per-user application data on Windows;
- single-instance activation that focuses the primary window without reading or transmitting secondary process arguments or working directory;
- process-owned activation generation that survives renderer reloads;
- generated `get_desktop_status`, `subscribe_desktop_activation`, and token-scoped `unsubscribe_desktop_activation` permissions with a typed backend-to-renderer channel and no general event-listener permission;
- explicit production CSP, frozen IPC prototype, no opener/filesystem/shell/process/HTTP plugin, and a renderer bridge import boundary;
- architecture checks that prevent Desktop from linking CLI, RPC host, agent, runtime, or storage implementations;
- frontend, Rust, target-registry, security-boundary, current-platform no-bundle build commands, and a native macOS primary/secondary process smoke.

The managed runtime remains explicitly `not_configured`. This scaffold does not weaken Phase 0: local execution must not locate a binary through `PATH`, read private CLI configuration, emit `rpc.toml`, open storage, or launch `starweaver-rpc` until the public launch-envelope, negotiation, authorization, and compatibility prerequisites are implemented. Future SSH startup uses the separately specified probe, exact managed runtime selector, and login-shell bootstrap rather than an unverified `PATH` fallback.

## Target Product Shape

The host IDL, generated bindings, RPC supervision, and execution connections below remain planned. The implemented Desktop foundation is still disconnected from RPC.

```mermaid
flowchart TD
    user[Desktop user]
    shell[Desktop shell and renderer]
    supervisor[Desktop backend supervisor]
    updater[Desktop runtime and component manager]
    local_rpc[Local RPC child]
    ssh[System OpenSSH process]
    remote_rpc[Remote RPC process]
    host_idl[Planned host major-2 OpenRPC and JSON Schema IDL]
    ts_client[Planned major-2 manifest-filtered TypeScript bridge and client]
    rpc_core[Target generated major-2 Rust host bindings and bootstrap protocols]
    local_store[(Local SQLite and OAuth)]
    remote_store[(Remote SQLite and OAuth)]
    local_runtime[Local runtime and workspace]
    remote_runtime[Remote runtime, filesystem, and shell]
    cli[Independent local or remote CLI]

    user --> shell
    host_idl --> ts_client
    host_idl --> rpc_core
    ts_client --> shell
    shell --> supervisor
    shell --> updater
    supervisor -->|direct stdio JSON-RPC| local_rpc
    supervisor -->|fixed SSH exec channel| ssh
    ssh -->|login-shell supervised stdio| remote_rpc
    updater -->|local verified runtime| local_rpc
    updater -->|public remote component contract| ssh
    rpc_core --> local_rpc
    rpc_core --> remote_rpc
    local_rpc --> local_store
    local_rpc --> local_runtime
    remote_rpc --> remote_store
    remote_rpc --> remote_runtime
    cli --> local_store
    cli --> remote_store
```

## Target Ownership Map

| Concern                                                                   | Owner                                                  |
| ------------------------------------------------------------------------- | ------------------------------------------------------ |
| Windows, navigation, renderer state, notifications, shortcuts             | Desktop shell                                          |
| Workspace-to-child routing, process lifecycle, restart, update activation | Desktop backend supervisor                             |
| Runtime download, verification, version selection, rollback state         | Desktop runtime update manager                         |
| Host method/notification/error wire structure and generated bindings      | host OpenRPC/JSON Schema IDL and repository generators |
| JSON-RPC server behavior, authorization, and live subscriptions           | `starweaver-rpc` and `starweaver-rpc-core`             |
| Agent/model/tool execution                                                | `starweaver-agent` and `starweaver-runtime`            |
| Session/run/replay contracts                                              | `starweaver-session` and `starweaver-stream`           |
| SQLite schema, migrations, atomic evidence operations                     | `starweaver-storage`                                   |
| OAuth credential storage and provider construction                        | `starweaver-oauth` and `starweaver-oauth-provider`     |
| Local and envd-backed workspace authority                                 | `starweaver-environment` and envd crates               |
| SSH process transport, host trust, remote routing, and prompt mediation   | Desktop backend supervisor                             |
| Public RPC component install/update contract and shared updater mechanics | Starweaver installer/update path                       |
| CLI commands and TUI coordination                                         | `starweaver-cli` only                                  |

No Desktop crate should become a shared protocol or storage owner. Under the accepted target, the repository-level major-2 host IDL owns reusable wire structure; generated Rust bindings live in `starweaver-rpc-core`, IDL-derived safe TypeScript bindings are consumed by Desktop, and reusable storage and runtime contracts remain in their existing owning crates. Current handwritten major-1 DTOs remain isolated implementation history and do not define the Desktop bridge.

## Spec Map

- `01-product-and-process-boundaries.md` — product ownership, process topology, launch configuration, workspace/sandbox boundaries, and lifecycle.
- `02-rpc-client-and-lifecycle.md` — Desktop client contract, connection state machine, replay, HITL, and required RPC additions.
- `03-cli-migration-and-compatibility.md` — shared history, custom database discovery, profile migration, continuation preflight, and version skew.
- `04-workspaces-sessions-and-runs.md` — workspace routing, session presentation, active-run ownership, and multi-window behavior.
- `05-auth-interaction-and-security.md` — OAuth, approvals, clarifying questions, capability negotiation, and local security.
- `06-runtime-updates-and-release.md` — update channels, runtime bundles, compatibility manifests, transactional activation, and rollback.
- `07-ssh-remote-workspaces.md` — SSH execution domains, login-shell RPC bootstrap, remote authority, provisioning, updates, and reconnect.

## Non-Goals

The first Desktop implementation does not:

- replace or wrap the CLI/TUI;
- expose SQLite records directly to frontend code;
- use a broad home-directory workspace root;
- attach to the in-memory control channel of an already running CLI process;
- require HTTP, WebSocket, a local daemon, or a cloud account;
- synchronize remote session databases into the local canonical database;
- promise transparent continuation when materialization evidence differs;
- connect the shell foundation to an RPC runtime before the Phase 0 launch, negotiation, authorization, and compatibility contracts are implemented.

## Delivery Phases

### Phase 0: protocol and release prerequisites

- canonical host major-2 OpenRPC/JSON Schema IDL plus generated Rust server, complete TypeScript protocol, and manifest-filtered safe Desktop bridge/client parity;
- RPC OAuth parity and safe auth methods;
- client capability negotiation and typed clarification answers;
- public versioned RPC launch-envelope schema and compatibility fixtures;
- receipt-backed idempotency and uncertain-outcome recovery for effectful RPC mutations;
- enforceable local shell sandbox provider, with native unsandboxed local shell disabled by default;
- continuation preflight;
- periodic ordinary-run admission reconciliation and corrected `run.resume` authorization;
- generation-safe subscriptions, bounded stdio framing, non-blocking state I/O, safe live errors, and bounded pagination;
- runtime/storage compatibility metadata and a cross-product two-phase database maintenance barrier;
- dedicated, verified runtime update artifact;
- current/previous CLI and RPC interoperability matrix.

### Phase 1: single-workspace Desktop

- one workspace window, one execution RPC child, and the least-authority catalog/control path when needed;
- canonical session history and replay;
- prompt, steer, interrupt, approval, deferred, and clarifying-question flows;
- runtime staging and restart-safe activation;
- history-only behavior for unavailable workspaces.

### Phase 2: multi-workspace supervisor

- one execution child per canonical workspace;
- shared local database with workspace-scoped coordination and sandboxed local shell/effect authority where enabled;
- multi-window routing and notifications;
- safe child reuse, idle retirement, and crash recovery.

### Phase 3: release hardening

- platform signing/notarization;
- stable/preview channels and pinning;
- N/N-1 compatibility gates;
- updater fault injection and rollback tests;
- installer, auto-update, and recovery documentation.

### Parallel SSH track after Phase 0

This track may proceed alongside local Phases 1–3; it is not deferred until after local release hardening.

- system OpenSSH transport with backend-owned host-key and authentication prompts;
- stable remote execution-domain identity separated from mutable host-key/runtime evidence and remotely canonicalized workspaces;
- login-shell supervised RPC bootstrap, cross-client workspace execution-host locks, and partition/reconnect reconciliation;
- hardened public RPC component bootstrap/update contract shared with `sw`/CLI;
- remote catalog/history routing and explicit full-account shell authority.

## Acceptance Direction

Desktop implementation may begin after Phase 0 contracts have named owners and fixtures. A public Desktop release additionally requires:

- no direct Desktop UI dependency on runtime, storage implementation, or CLI crates;
- no direct CLI/RPC/Desktop product dependency cycle;
- local shell-enabled profiles use an enforceable sandbox, while native unsandboxed local shell is disabled by default;
- remote account-scoped shell is clearly distinguished from a sandbox and enabled only for an explicitly granted SSH execution domain;
- stdio and SSH-supervised protocol corpus coverage using the shipped runtime binary;
- bidirectional CLI/Desktop history and continuation tests;
- current/previous runtime and storage compatibility tests;
- updater download, verification, activation, crash, and rollback tests;
- platform packaging and code-signing checks;
- user-facing migration, update, recovery, and data-location documentation.
