# Workspaces, Sessions, and Runs

Status: local workspace entry, multi-workspace history, backend-routed conversation windows, window-owned subscriptions, run-control ownership projection, storage-bounded session/run pagination, restart recovery, and D9 dogfood interaction polish implemented; user-guided visual review remains an acceptance gate

This document defines the local-only Codex App-like Desktop product surface: the user opens an existing folder, creates an empty folder, or starts without choosing a folder. Desktop registers that choice with its one long-lived local RPC host, then creates a session bound to the returned workspace identity. The RPC host, not Desktop, owns all workspaces, agents, sessions, runs, durable evidence, and workspace execution contexts. SSH and remote execution domains are outside the Desktop product.

## Workspace Entry Modes

The initial workspace picker offers three first-class actions:

1. **Open folder** — the native backend grants and canonicalizes an existing local folder.
2. **Create workspace** — the user chooses a parent and name; the authority-owning backend creates one empty folder, canonicalizes it, and opens it as the workspace.
3. **Start without a folder** — Desktop creates an empty managed temporary workspace and opens it through the same RPC launch path. It is not a separate agent mode.

A managed temporary workspace has a stable opaque workspace ID and a real directory root. To avoid losing agent-created files or making durable sessions impossible to resume, Desktop retains the directory while any retained session refers to it; cleanup is explicit or policy-driven and must first surface the affected sessions. Closing a window or restarting the RPC host never deletes it implicitly. Moving, copying, or rebinding a temporary workspace is outside the initial scope.

All three modes converge before session creation, while the one local host remains running:

1. the privileged backend obtains or atomically creates the folder under native user intent;
2. it canonicalizes the folder and submits a typed privileged workspace registration to the local RPC host;
3. RPC validates and records the grant and returns an opaque workspace identity; and
4. `session.create` binds the new session to that identity.

RPC stores the session/workspace binding and hosts all later agent/run operations for that session. Opening another folder changes only the workspace registry; it never starts another RPC process.

The renderer never sends an unrestricted path directly to the host. It sends a closed native workspace intent; the privileged backend owns folder selection or managed creation, constructs authority-bearing registration fields, and exposes only opaque workspace IDs to later renderer operations.

## Workspace Identity

A Desktop workspace has a durable provenance identity and a separate live authority grant. It is identified by:

- an execution-domain identity;
- a canonical local path;
- a stable Desktop workspace ID derived from the domain/canonical identity pair, not from a display name;
- a user-visible name and optional repository metadata;
- the RPC runtime/config generation used for new session/run materialization;
- availability and authorization state.

Local canonicalization occurs in the privileged Desktop backend. The renderer receives only display-safe labels and opaque identities. Symlink and case-normalization behavior follows the local platform semantics and is tested on each supported platform.

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
- last activity, drain state, and crash/restart budget.

RPC owns the authoritative live workspace-grant registry for its domain and persists it in owner-private host state separate from the canonical session database and static runtime config. A workspace entry is created only by an explicit privileged registration, contains the canonical root and authority metadata privately, and survives host restart; historical session metadata alone never recreates a grant. Protocol and renderer projections use opaque IDs and safe labels. Duplicate registration of the same canonical root is idempotent and returns the existing identity. Removing a workspace grant blocks new sessions/runs but must not invalidate retained history or interrupt active work without an explicit drain decision.

The supervisor serializes creation of the one local host. Multiple windows and workspaces reuse the same process-owned supervisor and stdio connection. Opening a folder or conversation window never starts another host.

## Global History Without Broad Workspace Authority

Session discovery is global within the local execution domain's canonical database. The always-available host serves bounded session/search/replay, session metadata management, profile/configuration discovery, diagnostics, and migration status. Desktop neither federates remote databases nor treats a cache as authoritative.

History access does not grant workspace effects. The host can query durable storage without injecting any workspace root into an agent or environment. Run admission requires a session bound to a currently registered and permitted workspace. Provider credential setup and refresh remain host concerns and are not Desktop catalog operations. Authorization remains granular: storage reads, session metadata mutations, configuration changes, and migration preparation do not automatically imply run authority.

## Session Presentation

Desktop shows one logical navigation surface across local CLI/Desktop history. The first bounded `session.list` page is rendered immediately; the frontend retains the opaque next-page token and loads older pages only on explicit demand. Session lists can be filtered by:

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

1. resolves the session through the process-owned local supervisor;
2. loads the safe session projection through the local RPC host;
3. resolves durable workspace evidence;
4. matches an already granted canonical workspace in the same domain;
5. if unavailable, groups the conversation under history-only mode without recreating authority;
6. if available, confirms that the domain host still has the workspace grant;
7. performs continuation preflight only when the user requests a new run;
8. routes live control to the domain host and run owner that admitted the new run.

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

Windows are views over process-owned backend state, not independent host clients.

- Only the main window may request another conversation window. The backend accepts one opaque session ID, mints the native label and fixed application URL, records `label -> sessionId`, and focuses the existing window rather than opening a duplicate for that session.
- Conversation windows receive a separate wildcard capability and the backend rechecks their route for host operations and event subscriptions; a renderer cannot choose a native label, URL, path, capability set, execution domain, or event scope outside its routed session. ID-based approval, clarification, and deferred operations carry a required session ID, and RPC compares it with the loaded durable record before projection or mutation. Workspace list pages are backend-filtered to the routed session's currently granted workspace, including an empty projection when that grant is unavailable; sibling workspace metadata is never released to the conversation renderer.
- Event subscription identity is `(window label, execution domain, session, run)`. Reloading the same window/run replaces only that tail after its completion barrier, while different windows may subscribe to the same run concurrently.
- Replay cursors, event acknowledgements, unsubscribe tokens, and teardown are window-owned. Destroying one conversation window cancels only its tails and never cancels the run or another window's subscription.
- Window-specific scroll position, selection, drafts, and panel layout stay client-local. Prompt submission remains disabled until `session.get` hydration and current-host controllability probes are authoritative, stays serialized by RPC's session admission boundary, and same-session refresh epochs prevent an older snapshot from replacing a newly admitted run.
- `run.status.controllableByCurrentHost` is derived from the current coordinator's process-local active registry. Durable `running` or `waiting` evidence remains readable when false, but Desktop disables steer and interrupt instead of assuming ownership.
- Approval or clarification decisions use expected revision/fence, explicit session ownership, and one durable idempotency key. A duplicate decision receives durable conflict/replay semantics rather than causing a second effect; pending ID-based mutations remain recoverable only from the matching conversation route.
- Closing the main window with `quit` requests application-level coordinated shutdown, so retained conversation windows cannot keep the RPC child alive without a restorable main window. `keep_running` hides only the main window.

## Empty and Temporary Workspaces

A newly created empty folder and a managed temporary workspace are ordinary workspace roots once opened. RPC does not need a special no-workspace session type, and agent tools always observe a concrete root.

Temporary workspace rules:

- the directory is atomically created below an owner-private Desktop-managed root; Unix ownership and `0700`-equivalent mode or a Windows current-user-only DACL are mandatory and creation fails closed when they cannot be verified;
- its identity is independent of a display path and remains stable across RPC restarts;
- creation/opening rejects symlinks, reparse points, pre-existing non-empty targets, and parent-directory substitution races;
- it is excluded from broad catalog scans and never treated as authority over its parent directory;
- closing a window, restarting the host, or restarting Desktop does not delete it;
- explicit cleanup checks for retained sessions and active runs before deletion;

## Worktrees and Related Roots

A worktree is a distinct canonical workspace unless an explicit repository grouping feature says otherwise. Repository grouping affects navigation only and must not merge environment authority or run identity.

A future workspace switch within one conversation still creates a materialization boundary. Desktop records the selected target and runs preflight; it does not rewrite the source run’s workspace.

## Pagination and Large Histories

Desktop uses storage-backed bounded pagination rather than loading all records and truncating in the RPC handler. `session.get` reads its newest run window through `SessionStore::list_recent_runs`, and `run.list` pages older runs by immutable session-local sequence. RPC cursors are opaque and bound to the storage, caller authority, operation, and session view; Desktop replaces them with generation-bound page tokens before crossing renderer IPC.

At minimum:

- session list/search uses opaque page tokens;
- run history supports newest-first bounded `run.list` pages, which the renderer reverses before prepending to its chronological conversation view;
- approval, deferred, and clarification lists are bounded;
- stream replay has cursor and byte/event limits;
- snapshots and large payloads use references where appropriate;
- the frontend virtualizes rendered history and can discard/reload old pages.

Page-size maxima are advertised during initialize and enforced by RPC. Hidden windows release run subscriptions and suspend ordinary status polling; visibility restoration refreshes durable state before attaching new live tails. Terminal transcript hydration remains concurrency-bounded.

## Acceptance Gates

- Open-folder, create-empty-folder, and start-without-folder flows register one canonical workspace identity before `session.create` and route through the same domain host.
- `session.create` requires one registered opaque workspace ID; RPC durably records the binding and renderer input cannot substitute a path.
- Managed temporary workspaces are atomically owner-private, reject path redirection, survive window/host restart, and are deleted only through explicit or visible policy-driven cleanup.
- One local RPC host manages multiple workspaces and sessions; opening another workspace never starts another local host.
- Two windows reuse the same backend connection, independently replay the same run, and do not duplicate a run.
- Concurrent runs in different local workspaces receive only their own registered roots and cannot obtain sibling workspace handles through host context or tool preparation.
- Authoritative local history remains available through the host without requiring any workspace grant to be open.
- Missing and moved workspaces preserve history-only access and require explicit rebind.
- CLI-owned active runs remain observable but uncontrollable.
- A host restarted before lease expiry automatically reconciles the ordinary run after expiry.
- Large-session tests prove bounded database reads, wire pages, and renderer memory.
- Local case, symlink, moved-root, worktree, and duplicate-window routing tests pass on supported platforms.
