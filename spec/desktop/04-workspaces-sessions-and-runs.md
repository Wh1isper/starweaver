# Workspaces, Sessions, and Runs

Status: accepted architecture baseline; implementation planned

This document defines the Codex App-like Desktop product surface: the user opens an existing folder, creates an empty folder, or starts without choosing a folder. Desktop registers that choice with the one RPC host for the execution domain, then creates a session bound to the returned workspace identity. The RPC host, not Desktop, owns all workspaces, agents, sessions, runs, durable evidence, and workspace execution contexts in that domain.

## Workspace Entry Modes

The initial workspace picker offers three first-class actions:

1. **Open folder** — the native backend grants and canonicalizes an existing local folder, or asks the remote RPC to canonicalize an existing remote folder.
2. **Create workspace** — the user chooses a parent and name; the authority-owning backend creates one empty folder, canonicalizes it, and opens it as the workspace.
3. **Start without a folder** — Desktop creates an empty managed temporary workspace and opens it through the same RPC launch path. It is not a separate agent mode.

A managed temporary workspace has a stable opaque workspace ID and a real directory root. To avoid losing agent-created files or making durable sessions impossible to resume, Desktop retains the directory while any retained session refers to it; cleanup is explicit or policy-driven and must first surface the affected sessions. Closing a window or restarting the RPC host never deletes it implicitly. Moving, copying, or rebinding a temporary workspace is outside the initial scope.

All three modes converge before session creation, while the one domain host remains running:

**Local:**

1. the privileged backend obtains or atomically creates the folder under native user intent;
2. it canonicalizes the folder and submits a typed privileged workspace registration to the local RPC host;
3. RPC validates and records the grant and returns an opaque workspace identity; and
4. `session.create` binds the new session to that identity.

**Remote:**

1. the backend sends a typed existing/create/temporary workspace intent through the established SSH-private RPC connection;
2. the remote RPC canonicalizes or safely creates the folder and registers the authority;
3. RPC returns an opaque canonical workspace identity; and
4. `session.create` binds the new session to that identity.

RPC stores the session/workspace binding and hosts all later agent/run operations for that session. Opening another folder changes only the workspace registry; it never starts another RPC process.

The renderer never sends an unrestricted path directly to the host. For local folders it sends an opaque native-picker grant to the backend; for remote folders it sends a constrained existing/create/temporary intent. The backend constructs authority-bearing workspace registration fields, and later renderer operations use only opaque workspace IDs.

## Workspace Identity

A Desktop workspace has a durable provenance identity and a separate live authority grant. It is identified by:

- an execution-domain identity;
- a canonical local path, a canonical remote workspace identity, or an explicit non-local environment attachment identity;
- a stable Desktop workspace ID derived from the domain/canonical identity pair, not from a display name;
- a user-visible name and optional repository metadata;
- the RPC runtime/config generation used for new session/run materialization;
- availability and authorization state.

Local canonicalization occurs in the privileged Desktop backend. SSH workspace canonicalization occurs only in the remote RPC after supervised bootstrap; a local filesystem API must never interpret the remote path. The renderer receives display-safe paths or names according to user settings. Symlink and case-normalization behavior follows the execution domain's platform semantics and is tested on each supported platform.

A path is not granted merely because it appears in old session metadata. Opening a historical workspace requires that the path still exists and that the user or managed policy grants Desktop authority to it.

## Domain Host and Workspace Registry

The supervisor registry maps one execution domain to one host process/connection:

```text
execution domain identity
    -> host entry with runtime, config, workspaces, sessions, and runs
```

A host entry tracks:

- process/connection state and negotiated compatibility;
- active runtime and configuration generations;
- registered opaque workspace identities and safe display projections;
- active run targets, subscriptions, acknowledged cursors, and unresolved interactions;
- last activity, drain state, crash/restart budget, and remote owner generation.

RPC owns the authoritative live workspace-grant registry for its domain and persists it in owner-private host state separate from the canonical session database and static runtime config. A workspace entry is created only by an explicit privileged registration, contains the canonical root and authority metadata privately, and survives host restart; historical session metadata alone never recreates a grant. Protocol and renderer projections use opaque IDs and safe labels. Duplicate registration of the same canonical root is idempotent and returns the existing identity. Removing a workspace grant blocks new sessions/runs but must not invalidate retained history or interrupt active work without an explicit drain decision.

The supervisor serializes creation of the one local host. Remote readiness additionally requires the shared execution-domain/database OS lock and fenced owner generation, so another Desktop client cannot create a competing domain host. Multiple windows and workspaces reuse the same process/connection and backend event stream. Identical displayed paths in local and remote domains remain unrelated identities.

## Global History Without Broad Workspace Authority

Session discovery is global within one execution domain's canonical database, not across all local and remote machines. The always-available domain host serves bounded session/search/replay, session metadata management, profile/configuration discovery, diagnostics, and migration status. Desktop presents a federated origin-scoped history by querying the local host and each connected remote domain; it does not merge databases or treat a disconnected cache as authoritative.

History access does not grant workspace effects. The host can query durable storage without injecting any workspace root into an agent or environment. Run admission requires a session bound to a currently registered and permitted workspace. Provider credential setup and refresh remain host concerns and are not Desktop catalog operations. Authorization remains granular: storage reads, session metadata mutations, configuration changes, and migration preparation do not automatically imply run authority.

## Session Presentation

Desktop shows one logical navigation surface across local CLI/Desktop history and connected remote execution domains. Every row retains its execution-domain origin, and queries/pagination remain per-origin before backend aggregation. Session lists are obtained through bounded RPC pagination and can be filtered by:

- workspace identity/display name;
- source product;
- profile/model summary;
- status;
- updated time;
- text search;
- availability and continuation readiness.

The list does not imply that the current host connection can control every active run. Each row separately projects durable status, process ownership when known, workspace availability, and continuation preflight state.

A session is still viewable when:

- its workspace was removed or moved;
- its original profile no longer exists;
- its configured model provider is unavailable or requires host-side configuration;
- its source runtime is unavailable;
- its last run has a foreign active owner.

Unavailable execution dependencies affect continuation, not historical readability.

## RPC-Hosted Sessions

A session is created and run through the one RPC host for its execution domain. `session.create` requires one opaque registered `workspaceId`; it never accepts a renderer-provided path. The host resolves the live grant, durably records typed workspace provenance on the session, and rejects unknown, removed, or unauthorized grants. The durable provenance ID remains useful for history and matching after the live grant disappears, but never recreates filesystem authority. `run.start`, continuation, HITL resolution, replay, and active control remain on that domain host or a compatible replacement generation.

Another CLI/RPC/Desktop product opening the same canonical database can read the provenance but may not have this host's private grant registry. It must explicitly register/regrant a canonical root that proves the same workspace identity or record a typed rebind/materialization switch; historical path text is never executed as authority.

Desktop is a UI and lifecycle client. It owns folder-picking UX, opaque workspace grants, host selection, subscriptions, drafts, and safe view state. It does not instantiate agents, interpret workspace paths as execution authority, or maintain a parallel session implementation.

## Session-to-Workspace Routing

When a user opens a session, the backend:

1. resolves and, when necessary, reconnects the session's execution domain;
2. loads the safe session projection through that domain's RPC;
3. resolves durable workspace evidence;
4. matches an already granted canonical workspace in the same domain;
5. if unavailable, opens history-only mode when authoritative remote access remains available and offers explicit locate/rebind actions;
6. if available, confirms that the domain host still has the workspace grant;
7. performs continuation preflight only when the user requests a new run;
8. routes live control to the domain host and run owner that admitted the new run.

A locally cached remote projection can support a clearly stale preview while disconnected, but not authoritative history-only mutation, continuation, or HITL resolution.

Locating a moved workspace does not mutate historical evidence. A continuation under the new root records target materialization and workspace drift through normal switch semantics.

## Run Ownership

Run status and run control are separate concepts.

- Durable storage is authoritative for persisted lifecycle evidence.
- The host-process-local active registry is authoritative for steer, interrupt, live environment mutation, and finalizer control.
- Admission leases and fences prevent competing ownership.
- The Desktop backend records which domain host generation admitted each Desktop-started run.
- A host restart does not regain control solely because durable status says `Running`.

The host must reconcile ordinary expired run admissions periodically, not only once at startup. This is a Phase 0 prerequisite: if a host restarts before a foreign lease expires, it must still terminalize or recover the orphan after expiry while remaining online. Status, await, subscription, or a dedicated periodic reconciler may trigger the fenced storage operation, but recovery cannot depend on a later mutation to the same session.

## Foreign Runs

A run may be owned by:

- the current domain host generation;
- another Desktop instance or foreign host generation;
- a CLI process;
- an external RPC host;
- no live process after an expired lease.

Desktop behavior:

| Ownership                | Read/replay           | Steer/interrupt         | Continue                                       |
| ------------------------ | --------------------- | ----------------------- | ---------------------------------------------- |
| Current host generation  | Yes                   | Yes                     | Subject to admission/materialization           |
| Other known/foreign host | Yes                   | Routed only if attached | Subject to owner and admission                 |
| CLI/external process     | Durable evidence only | No                      | Blocked while admission is active              |
| Expired/orphaned         | After reconciliation  | No old control channel  | Allowed only after terminal/recovered evidence |

Desktop does not silently kill a foreign process to obtain control.

## Multi-Window Behavior

Windows are views over backend state, not independent host clients.

- One backend subscription can fan out safe events to multiple windows.
- Window-specific scroll position, selection, drafts, and panel layout stay client-local.
- Prompt submission is serialized per session admission boundary.
- Steering from two windows receives one ordered backend sequence.
- Approval or clarification decisions use expected revision/fence and one durable idempotency key.
- Closing one window does not cancel another window’s run or subscription.
- A duplicate decision receives the durable existing result rather than causing a second effect.

## Empty and Temporary Workspaces

A newly created empty folder and a managed temporary workspace are ordinary workspace roots once opened. RPC does not need a special no-workspace session type, and agent tools always observe a concrete root.

Temporary workspace rules:

- the directory is atomically created below an owner-private Desktop-managed root; Unix ownership and `0700`-equivalent mode or a Windows current-user-only DACL are mandatory and creation fails closed when they cannot be verified;
- its identity is independent of a display path and remains stable across RPC restarts;
- creation/opening rejects symlinks, reparse points, pre-existing non-empty targets, and parent-directory substitution races;
- it is excluded from broad catalog scans and never treated as authority over its parent directory;
- closing a window, restarting the host, or restarting Desktop does not delete it;
- explicit cleanup checks for retained sessions and active runs before deletion;

For SSH, “start without a folder” creates an empty managed directory in a fixed remote Starweaver workspace area through a typed RPC/bootstrap operation. Desktop does not invent or interpolate a remote path in a shell command.

## Worktrees and Related Roots

A worktree is a distinct canonical workspace unless an explicit repository grouping feature says otherwise. Repository grouping affects navigation only and must not merge environment authority or run identity.

A future workspace switch within one conversation still creates a materialization boundary. Desktop records the selected target and runs preflight; it does not rewrite the source run’s workspace.

## Pagination and Large Histories

Public Desktop readiness requires storage-backed bounded pagination rather than loading all records and truncating in the RPC handler.

At minimum:

- session list/search uses opaque page tokens;
- run history supports newest-first bounded pages;
- approval, deferred, and clarification lists are bounded;
- stream replay has cursor and byte/event limits;
- snapshots and large payloads use references where appropriate;
- the frontend virtualizes rendered history and can discard/reload old pages.

Page-size maxima are advertised during initialize and enforced by RPC.

## Acceptance Gates

- Open-folder, create-empty-folder, and start-without-folder flows register one canonical workspace identity before `session.create` and route through the same domain host.
- `session.create` requires one registered opaque workspace ID; RPC durably records the binding and renderer input cannot substitute a path.
- Managed temporary workspaces are atomically owner-private, reject path redirection, survive window/host restart, and are deleted only through explicit or visible policy-driven cleanup.
- One local RPC host manages multiple workspaces and sessions; opening another workspace never starts another local host.
- Two windows reuse the same backend/domain connection and do not duplicate a run; two Desktop clients targeting one remote execution domain/database cannot both become execution-authorized.
- Concurrent runs in different local workspaces receive only their own registered roots and cannot obtain sibling workspace handles through host context or tool preparation.
- Remote workspaces share only their remote domain host and history; routing and cache keys cannot collide with local or other remote domains.
- Authoritative history remains available through the domain host without requiring any workspace to be open; disconnected remote cache is visibly stale and read-only.
- Missing and moved workspaces preserve history-only access and require explicit rebind.
- CLI-owned active runs remain observable but uncontrollable.
- A host restarted before lease expiry automatically reconciles the ordinary run after expiry.
- Large-session tests prove bounded database reads, wire pages, and renderer memory.
- Local and remote case, symlink, moved-root, worktree, execution-domain, and duplicate-window routing tests pass on supported platforms.
