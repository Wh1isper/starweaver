# Workspaces, Sessions, and Runs

Status: accepted architecture baseline; implementation planned

This document defines how Desktop presents one shared history while preserving workspace-scoped execution authority and process-local run ownership.

## Workspace Identity

A Desktop workspace is identified by:

- a canonical local path or an explicit non-local environment attachment identity;
- a stable Desktop workspace ID derived from the canonical identity, not from a display name;
- a user-visible name and optional repository metadata;
- the RPC runtime/config identity used for its child;
- availability and authorization state.

Canonicalization occurs in the privileged Desktop backend. The renderer receives display-safe paths or names according to user settings. Symlink and case-normalization behavior must follow platform filesystem semantics and be tested on each supported platform.

A path is not granted merely because it appears in old session metadata. Opening a historical workspace requires that the path still exists and that the user or managed policy grants Desktop authority to it.

## Child Registry

The supervisor registry maps a workspace key to one child state:

```text
canonical workspace identity -> child entry with runtime and launch-config generations
```

A child entry tracks:

- process and connection state;
- negotiated capabilities and compatibility;
- active run targets owned by the child;
- subscriptions and acknowledged cursors;
- unresolved interactions;
- last activity and drain state;
- crash/restart budget;
- update generation.

The supervisor serializes child creation for one key. Two windows that open the same workspace reuse the same child and backend event stream.

## Global History Without Broad Workspace Authority

Session discovery is global to the selected shared database, but execution authority is workspace-scoped. Desktop must not solve this by granting a child access to the user home directory or by reading SQLite in the UI/backend.

The preferred host addition is a least-authority catalog/control mode. It exposes bounded session/search/replay, explicitly authorized session metadata management, profile discovery, OAuth operations, diagnostics, and migration status while denying model/tool run admission and local/environment effects. The Desktop supervisor may keep one catalog RPC child rooted in an empty Desktop-owned directory.

Catalog/control authorization remains granular: storage reads, session mutations, auth changes, and migration preparation do not imply run authority. Until this mode exists, an already open workspace child may serve global read queries. If no child is open, Desktop may require a workspace selection before querying history; this is acceptable only for an internal pilot, not the public migration experience.

Catalog mode is a host authorization profile, not a separate storage implementation or Desktop-specific database service.

## Session Presentation

Desktop shows one logical history across CLI and all Desktop workspace children. Session lists are obtained through bounded RPC pagination and can be filtered by:

- workspace identity/display name;
- source product;
- profile/model summary;
- status;
- updated time;
- text search;
- availability and continuation readiness.

The list does not imply that the current child can control every active run. Each row separately projects durable status, process ownership when known, workspace availability, and continuation preflight state.

A session is still viewable when:

- its workspace was removed or moved;
- its original profile no longer exists;
- its OAuth account is logged out;
- its source runtime is unavailable;
- its last run has a foreign active owner.

Unavailable execution dependencies affect continuation, not historical readability.

## Session-to-Workspace Routing

When a user opens a session, the backend:

1. loads the safe session projection through RPC;
2. resolves durable workspace evidence;
3. matches an already granted canonical workspace;
4. if unavailable, opens history-only mode and offers explicit locate/rebind actions;
5. if available, starts or reuses the workspace child;
6. performs continuation preflight only when the user requests a new run;
7. routes live control to the child that admitted the new run.

Locating a moved workspace does not mutate historical evidence. A continuation under the new root records target materialization and workspace drift through normal switch semantics.

## Run Ownership

Run status and run control are separate concepts.

- Durable storage is authoritative for persisted lifecycle evidence.
- The child-local active registry is authoritative for process-local steer, interrupt, live environment mutation, and finalizer control.
- Admission leases and fences prevent competing ownership.
- The Desktop backend records which child admitted each Desktop-started run.
- A child restart does not regain control solely because durable status says `Running`.

The host must reconcile ordinary expired run admissions periodically, not only once at startup. This is a Phase 0 prerequisite: if a host restarts before a foreign lease expires, it must still terminalize or recover the orphan after expiry while remaining online. Status, await, subscription, or a dedicated periodic reconciler may trigger the fenced storage operation, but recovery cannot depend on a later mutation to the same session.

## Foreign Runs

A run may be owned by:

- the current workspace child;
- another Desktop child or Desktop instance;
- a CLI process;
- an external RPC host;
- no live process after an expired lease.

Desktop behavior:

| Ownership                 | Read/replay           | Steer/interrupt        | Continue                                       |
| ------------------------- | --------------------- | ---------------------- | ---------------------------------------------- |
| Current child             | Yes                   | Yes                    | Subject to admission/materialization           |
| Other known Desktop child | Yes                   | Routed to owner child  | Subject to owner and admission                 |
| CLI/external process      | Durable evidence only | No                     | Blocked while admission is active              |
| Expired/orphaned          | After reconciliation  | No old control channel | Allowed only after terminal/recovered evidence |

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

- Two windows for one workspace reuse one child and do not duplicate a run.
- Two workspace children share history but cannot access each other’s local roots.
- Global history remains available through a least-authority RPC path.
- Missing and moved workspaces preserve history-only access and require explicit rebind.
- CLI-owned active runs remain observable but uncontrollable.
- A host restarted before lease expiry automatically reconciles the ordinary run after expiry.
- Large-session tests prove bounded database reads, wire pages, and renderer memory.
- Case, symlink, moved-root, worktree, and duplicate-window routing tests pass on supported platforms.
