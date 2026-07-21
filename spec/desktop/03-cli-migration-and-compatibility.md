# CLI Migration and Compatibility

Status: accepted architecture baseline; implementation planned

The Desktop migration goal is that existing CLI users see their durable history without export or duplication and can continue it safely. “Seamless” does not mean ignoring database overrides, materialization drift, workspace authority, authentication, or version skew.

## Shared Data Principle

When CLI and Desktop resolve the same canonical database in one execution domain, no data migration occurs. Local Desktop RPC children open the existing local database; an SSH-hosted RPC process independently opens the remote user's canonical database and shares it with remote CLI/RPC products. Desktop never merges or synchronizes local and remote canonical databases.

Shared durable data includes:

- sessions and runs;
- canonical input parts and output projections;
- raw and display stream records;
- replay cursors and snapshots;
- approvals and deferred records;
- checkpoints and resumable state;
- environment/materialization evidence;
- admission, continuation, and terminal evidence;
- source-product metadata.

Desktop-local window state, RPC client state, update state, and cached view models are not written into this database.

## Database Discovery

The Desktop backend first collects candidates without creating, opening for migration, or selecting an empty database:

1. an explicit user-selected path or managed deployment policy;
2. `STARWEAVER_SESSION_DB` when Desktop was launched with that environment;
3. an existing canonical database below the selected Starweaver config directory;
4. a one-time, read-only discovery of known CLI custom/legacy locations.

It then inspects viable candidates read-only and selects according to these rules:

- an explicit user/managed path wins after validation;
- an explicit environment path wins after validation;
- one non-empty canonical candidate is the default when no competing non-empty custom candidate exists;
- an absent or empty canonical candidate remains provisional until known-location discovery completes;
- multiple non-empty candidates, or an empty canonical candidate plus non-empty custom history, require a user choice;
- a new canonical database is created only after discovery and selection conclude.

Desktop always launches RPC with the resolved path explicitly. RPC does not parse CLI configuration and does not scan arbitrary user directories.

Discovery results are classified as:

- `canonical_existing`;
- `canonical_new`;
- `custom_cli`;
- `legacy_importable`;
- `multiple_candidates`;
- `unreadable`;
- `newer_schema`;
- `invalid`.

The UI must not show an empty new history as if it were the user’s old history when a known custom CLI database exists. It presents the selected path, record count summary, schema version, and last update time before a copy/import operation.

## Open In Place, Import, and Copy

- Canonical or explicitly shared databases are opened in place.
- Legacy database formats are first copied into a consistent SQLite backup/snapshot; the product-neutral importer may migrate and read that disposable snapshot, never the original source.
- Copying a live SQLite file with a filesystem copy is prohibited. A snapshot uses the SQLite backup API or an equivalent consistent storage operation.
- Import is idempotent and never rewrites source session/run IDs unless a documented collision strategy requires a new ID with provenance.
- The original source schema and bytes remain unchanged before, during, and after destination integrity checks. If a non-mutating legacy reader is implemented later, it must preserve the same invariant.
- Desktop never silently merges two independently active canonical databases.

## Configuration Migration

CLI and RPC remain separate configuration products. Desktop may offer a one-time migration adapter, but RPC must not depend on CLI config types.

The adapter reads only known, versioned CLI fields and produces a preview containing:

- candidate profiles and model identities;
- provider endpoint and non-secret transport settings;
- selected default profile;
- workspace/project hints;
- custom database location;
- skill, subagent, MCP, and tool configuration that has a compatible RPC representation;
- unsupported fields with an explanation.

The user confirms the preview before Desktop writes its product-owned profile model and materializes a supported version of the public RPC launch envelope. It never emits private `rpc.toml` fields. Environment variable names may be imported; environment variable values and inline secrets are not copied into ordinary config or launch-envelope files.

Unknown CLI fields are preserved in the source and ignored safely. A failed or partial import can be retried without changing the source configuration.

## OAuth Migration

The CLI and RPC should use the shared OAuth credential store through `starweaver-oauth` within one execution domain. Desktop does not copy access or refresh tokens. An SSH-hosted RPC uses the remote OAuth store and remote provider environment; local OAuth credentials are never forwarded.

On first launch:

- RPC reports safe provider authentication status;
- an existing valid Codex credential is reused;
- expired but refreshable credentials are refreshed by the OAuth provider layer;
- missing or invalid credentials start a Desktop-visible device/login flow;
- logout and account replacement are explicit user operations.

Desktop displays account/provider metadata allowed by the safe projection but never receives bearer or refresh tokens.

## Profile and Materialization Compatibility

A matching profile name does not prove that two products resolved the same agent. Continuation compares durable source evidence against the target RPC materialization, including:

- agent/spec digest;
- model/provider profile and settings digest;
- tool and capability policy;
- runtime binding digest;
- workspace root digest;
- environment attachment identities;
- protocol-relevant feature versions.

Desktop runs typed continuation preflight before resuming CLI history. Crossing local/remote execution domains, authenticated remote principals, database identities, or account/sandbox authority modes is materialization drift and never auto-switches.

| Outcome                     | Desktop behavior                                          |
| --------------------------- | --------------------------------------------------------- |
| Compatible                  | Continue with preserve semantics                          |
| Switch required             | Show sanitized drift and require confirmation             |
| Blocked                     | Keep history readable and explain the missing requirement |
| Waiting resolution required | Route the user to the durable interaction first           |
| Foreign active owner        | Observe durable status; do not steal control              |

Cross-workspace or changed-authority continuations never auto-switch.

## Foreign Active CLI Runs

A Desktop process cannot assume control of an in-memory run owned by CLI.

For a foreign active run Desktop may:

- display durable status and persisted output;
- poll or replay newly persisted evidence;
- notify the user when terminal evidence appears;
- recover only after the owner lease expires and shared reconciliation allows it.

It may not:

- send steer or interrupt to a process-local CLI control channel;
- infer ownership from `RunStatus::Running` alone;
- start a competing continuation while admission is held;
- describe non-durable token output as recoverable.

A future explicit cross-product handoff protocol requires its own fencing and is outside the v1 scope.

## Version Compatibility

Desktop shell, bundled RPC runtime, CLI, host protocol, and storage schema may have different versions. Compatibility is determined by declared ranges and fixtures, not by string equality alone.

The release matrix must cover at least:

| Desktop runtime    | CLI writer                | Required validation                                                 |
| ------------------ | ------------------------- | ------------------------------------------------------------------- |
| Current            | Current                   | Read, search, replay, create, continue                              |
| Current            | Previous supported        | Open/migrate, read, continue                                        |
| Previous supported | Current                   | Read when schema range permits; otherwise fail with update required |
| Staged next        | Current database snapshot | Preflight and migration dry run                                     |

Every CLI, RPC, and Desktop runtime path that can apply pending migrations participates in the same product-neutral maintenance barrier. A standalone CLI or RPC startup cannot bypass the barrier and silently migrate the database while Desktop children are active.

Remote component updates use the same compatibility declarations and maintenance barrier against the remote canonical database; they do not inspect or migrate the local database.

Every storage release declares:

- current schema generation;
- minimum readable generation;
- maximum readable generation;
- minimum writable generation;
- maximum writable generation;
- whether opening can apply a migration;
- whether the prior supported runtime can read the result.

Unknown future migrations fail closed with a structured update-required result. Desktop must never offer “create a new database” as an automatic recovery from a newer-schema error.

## Migration and Update Coordination

A runtime that may migrate shared storage is activated only through the quiesced update flow in `06-runtime-updates-and-release.md`.

- All Desktop-owned RPC children are drained before the real migration.
- The updater first publishes a fenced maintenance drain cutoff that rejects new admission while allowing pre-cutoff owners to finalize, then promotes it to exclusive migration ownership only after every eligible owner is gone.
- The updater creates a consistent backup before an irreversible migration.
- CLI cannot be forcibly stopped by Desktop; the updater detects active foreign admissions and non-participating older processes, then delays migration or asks the user to close active CLI work.
- Database migration and runtime pointer activation form one recoverable update transaction.
- Rollback is allowed only when the old runtime declares the migrated schema readable/writable or the database is safely restored from the pre-migration backup without discarding later writes.

## UX Requirements

On first launch, Desktop reports one of these concise outcomes:

- existing history connected;
- no existing history found;
- custom CLI history found and awaiting selection;
- history available but workspace missing;
- history available but profile/authentication setup required;
- history available but continuation requires a runtime switch;
- database requires a newer Desktop runtime;
- migration blocked by an active foreign process.

A session remains browsable even when its original workspace, profile, provider, or runtime is unavailable.

## Acceptance Gates

- Default CLI history appears in Desktop without copy/export.
- Candidate collection inspects custom history before selecting or creating an empty canonical store, so discovery cannot create a misleading parallel database.
- Legacy import is idempotent and provenance-preserving, and byte/schema checks prove the original source database remains unchanged.
- Local and remote OAuth reuse is tested without token projection into the renderer or credential forwarding between execution domains.
- Current-version CLI/RPC bidirectional subprocess interoperability remains green.
- N/N-1 database and protocol fixtures run in CI using released binaries or immutable fixtures, including the shared maintenance barrier.
- Preserve, switch, blocked, waiting, and foreign-owner continuation outcomes have typed contract tests.
- Live-file snapshot, migration interruption, newer-schema rejection, and safe backup recovery are tested.
