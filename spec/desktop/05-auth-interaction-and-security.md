# Interaction, Authorization, and Transport Security

Status: accepted local-only architecture; renderer, stdio child, workspace authority, host-local OAuth, durable interaction controls, and privileged update channels implemented; enforceable sandbox controls planned; SSH removed from Desktop scope

Desktop introduces a privileged local UI around filesystem, shell, model, and durable-control capabilities. Its security boundary is the Desktop backend plus the execution-domain RPC host and its typed workspace registry, not the renderer.

## Threat Model

The design must account for:

- compromised or injected renderer content, including valid-looking attempts to redirect providers, broaden tools, or mutate runtime configuration;
- malformed JSON-RPC frames or notifications;
- a compromised model attempting unauthorized tool use;
- another local process reading credentials or connecting to an HTTP endpoint;
- SSH route spoofing, host-key replacement, malicious login-shell output, credential-prompt confusion, or remote bootstrap injection;
- symlink/path confusion across workspace roots and execution domains;
- stale or duplicated approval and clarification decisions;
- runtime update tampering or downgrade;
- crash/restart races that duplicate effects;
- diagnostics leaking tokens, SQL details, private paths, or model content.

Desktop does not claim isolation from an already fully compromised user account. It still applies least authority and avoids widening exposure across repositories or local processes.

## Renderer Boundary

The renderer receives a narrow application API. It cannot:

- spawn local or SSH-carried RPC/runtime processes;
- choose arbitrary runtime or OpenSSH binary paths;
- send arbitrary JSON-RPC methods;
- read environment variables, provider credential files, RPC transport-token files, or SQLite;
- select unrestricted workspace paths without a native backend grant flow;
- decide authorization scopes;
- install or activate runtime updates;
- access raw stderr, SSH prompts, provisioning channels, or internal error chains.

All external links, file reveals, shell actions, SSH credential prompts, and authority/destination-changing configuration mutations pass through explicit backend commands and platform validation. A closed config DTO is not user authorization: the backend requires native user presence or a managed allowlist bound to the exact candidate fingerprint before adding host admin authority, as defined in `08-configuration-and-reload.md`.

## Model Provider Credential Boundary

Each RPC host resolves model-provider configuration and credentials inside its own execution domain. It may internally use environment variables, API-key stores, `starweaver-oauth`, or `starweaver-oauth-provider`, but those are RPC/model implementation details rather than a Desktop product contract.

Desktop and its renderer have no provider `status`, `login`, `refresh`, `logout`, account-selection, token-transfer, or credential-migration API. They never receive access tokens, refresh tokens, provider account metadata, authorization headers, raw JWTs, login codes, provider environment values, or credential-file contents. Missing or invalid provider configuration appears only as a typed, secret-free host execution/materialization error with an external recovery hint.

Provider credential persistence, refresh coordination, and account selection remain host-owned. A local host never forwards them to a remote host, and Desktop never synchronizes them across execution domains or exposes them to workspace/session state.

## Client Capability Negotiation

The backend declares supported interaction capabilities during initialize, including:

- approvals;
- deferred tools;
- clarifying questions;
- rich tool events;
- native notifications;
- file/diff presentation;
- external-link confirmation.

RPC intersects client capabilities with server policy. A capability is enabled only when both sides support it. HTTP clients negotiate independently; a global `rpc.toml` flag is not proof that every connected client can resolve an interaction.

## Approval Model

An approval decision is separate from permission to execute or resume a run.

HTTP scope rules must preserve least privilege:

- `approval` is required to discover or inspect approval, clarification, and deferred records and to decide or resolve them;
- `run` is required to start, resume, steer, interrupt, or otherwise cause execution;
- `run.resume` requires `run` authority and may additionally require `approval` when the caller also submits the decision;
- one scope never implicitly grants another unless the protocol explicitly defines a composite credential;
- stdio inherits authority from the Desktop backend process but applies the same semantic checks in service code where practical.

An approval includes durable identity, expected revision/fence, actor, reason, normalized decision metadata, and idempotency key. Desktop discovers it through the durable paginated host query, loads its bounded complete argument projection only when selected, and disables approval when the projection is incomplete. The selected session/workspace context and requested action remain visible; richer capability-grant and risk presentation is added when those fields become part of the safe host projection.

## Event Authorization

Method-level authority for `events.replay` or `events.subscribe` is not sufficient by itself. The canonical closed `x-starweaver-event-classes` IDL registry binds every generated `HostEvent` variant to its exact schema, required feature, and authorization scopes; generator lint proves it is complete against both `HostEvent.oneOf` and all event profiles. Rust and complete TypeScript metadata are generated from that registry. The Desktop backend and server consume the generated admission facts rather than maintaining a renderer-controlled or handwritten competing table, and evaluate them against the typed `EventView` and caller authority identically for replay catch-up and live delivery.

A versioned event-view profile is admitted only when the caller is authorized for every event class it selects. Events outside that profile belong to a separate logical projection stream and do not consume its cursor positions or pagination, so there is no per-record silent authorization filtering. Approval/deferred profiles require `approval`; execution-changing or run-control profiles require `run`; no generic read/event scope silently grants either. Authority or negotiated-feature changes invalidate the view and close active subscriptions with a typed, non-disclosing reason.

## Clarifying Questions

Clarifying questions require a typed answer contract. Marking an approval `approved` without persisting normalized answers is invalid. The host summary retains the original one-to-four questions with header, options, and `multiSelect`; `clarification.resolve` submits closed answer items keyed by exact question text with explicit selected-option labels and optional free text, plus an optional whole-request response. Resolution is fenced by `expectedRevision` and a supervisor-owned idempotency key.

The ordinary `approval.decide` path rejects `ask_user_question` records in both the RPC adapter and the atomic storage mutation, so a caller cannot bypass typed answer validation by reusing the clarification ID as an approval ID.

The resolution path must:

1. load the durable question/approval and verify tool identity and pending state;
2. validate selected options and free-text answers against the original schema;
3. normalize answers through shared user-input preprocessing;
4. atomically persist decision metadata or approved override arguments;
5. return a durable decision receipt;
6. resume only through normal fenced continuation admission;
7. expose the sanitized answer to the model/tool result exactly once.

The implemented Desktop path discovers approval-backed questions after reconnect, resolves typed answers as a durable fenced mutation, and invokes `run.resume` as a separate durable operation. A crash or lost response between those operations leaves the decision saved and the run waiting; the Inbox detects that durable state and presents an explicit resume action rather than fabricating atomicity. The release gate retains a question-to-answer-to-resume E2E test.

## Deferred Tools

Deferred resolution follows the same durable discipline:

- list unresolved records after reconnect with kind and state predicates applied in storage before keyset pagination;
- validate expected revision/fence;
- resolve with a stable idempotency key;
- persist normalized result/error without raw secrets;
- resume through the normal waiting-run continuation path;
- prevent one window from replacing another window’s terminal decision.

## Workspace and Tool Authority

Each execution-domain host owns a registry of explicitly granted workspaces. Every session is durably bound to exactly one workspace identity, and every run/tool/environment context receives only that session's live grant; no process-global default root or sibling workspace handle is injected. Local filesystem tools use the selected grant intersected with host policy and cannot infer authority from historical session metadata. Remote paths are canonicalized by the remote RPC and never by local filesystem APIs. Concurrent cross-workspace materialization tests must prove that one session cannot acquire another session's root or resources.

A canonical root and process boundary do not restrict a native shell running as the user. Public local shell-enabled profiles must use an enforceable sandboxed environment/process provider that confines filesystem, process, and inherited-resource effects to the granted workspace/resources. When no supported sandbox is available, native local shell is disabled by default.

An explicitly granted SSH execution domain uses a different accepted default: native remote shell may be enabled with the full authenticated remote account's authority. The target grant must show that this permits access outside the selected workspace, and the UI must label it `remote account authority`, not sandboxed. A dedicated account, container, VM, or proved remote sandbox is required when repository containment is desired. Managed policy may disable this default.

Desktop displays effective authority before a sensitive decision:

- workspace root or safe display path;
- read-only/read-write mode;
- shell/process permission;
- external environment attachments;
- network/media/browser capabilities when present;
- requested tool and risk class.

Changing workspace, environment, tool grants, or model/provider during continuation is materialization drift and requires the policy in `03-cli-migration-and-compatibility.md`.

## RPC Framing and Process Security

The local stdio and SSH-tunneled private-endpoint transports must have an inbound byte limit before allocating a complete frame. An unbounded `lines()` decoder is not sufficient for the Desktop boundary.

Required controls:

- bounded incremental frame decoding;
- advertised maximum request/notification/result sizes;
- bounded outbound queue and slow-consumer policy, with reserved capacity/priority for responses and subscription terminal control frames;
- generation-safe subscriptions so an old tail cannot delete a newly reused subscription ID;
- no inherited stdin/stdout handles beyond the intended local host;
- clean environment allowlist rather than forwarding all Desktop environment variables;
- bounded stderr capture with secret scrubbing;
- no shell interpolation in local host launch arguments;
- SSH remote commands are fixed backend-owned templates; workspace paths, provider/profile values, launch envelopes, and renderer text travel only in bounded typed frames;
- login-shell output is bounded until an exact nonce-bound bootstrap marker; marker-following stdout accepts only the finite typed workspace-resolution and endpoint frames, never becomes RPC stdout, and any unexpected output fails the bootstrap;
- if `subscription.closed` cannot flush within its terminal-control deadline, close the transport so recovery starts from the client's last applied opaque cursor;
- successful unsubscribe response flush is a barrier after which no event from that subscription generation may be written; and
- process-tree or SSH-channel termination on forced shutdown.

The Desktop backend avoids long blocking calls on the command connection. The generated contract uses non-blocking `run.start` plus durable subscription/replay and does not expose unbounded `run.await`; concurrent dispatch still requires an ordered response writer, and long environment probes cannot block responsive stop/steer/shutdown behavior.

## RPC Runtime Safety Prerequisites

The local Desktop path now has periodic ordinary-run lease reconciliation, typed atomic clarification answers, run-authority enforcement for `run.resume`, generation-safe subscription removal, blocking client-state I/O isolation, safe public subscription errors, bounded stdio framing, and storage-bounded paginated interaction discovery. These are protocol and host safety requirements rather than optional UI polish, and their focused plus aggregate gates remain release prerequisites.

## SSH Transport Security

System OpenSSH transport, host-key verification, askpass mediation, effective-config restrictions, provisioning isolation, account-authority disclosure, private endpoint forwarding, and reconnect behavior are normative in `07-ssh-remote-workspaces.md`.

Agent, X11, dynamic forwarding, arbitrary local/remote forwarding, renderer-authored commands, multiplexing, and environment forwarding are disabled. V1 permits only one backend-constructed local or stream-local forward to the exact owner-private RPC endpoint returned by the nonce-bound bootstrap. Every forwarded connection completes a mandatory random per-launch endpoint-authenticator preface; a local stream socket also requires current-user ownership, restrictive mode/ACL, no-follow creation, and safe stale-socket handling, while a local TCP fallback binds only an OS-assigned loopback port. It invokes OpenSSH only with a generated least-authority configuration produced by a non-executing allowlist importer; it rejects `Match exec`, command/helper/provider loaders, `SetEnv`/`SendEnv`, recursive `Include`, and other non-allowlisted directives before OpenSSH parses them. The local SSH agent may authenticate but is never forwarded. Unknown keys require explicit native confirmation; changed keys fail closed. Remote component installation runs on a separate bounded provisioning channel and uses the signed, exact-version public Starweaver installer contract rather than renderer-authored commands or unpinned `curl | sh`.

## HTTP and Future Transports

Desktop v1 does not expose standalone RPC HTTP to the renderer. Every SSH-forwarded endpoint uses a per-launch bootstrap authenticator; the loopback fallback is an SSH-confined private transport, not the public HTTP integration profile. If HTTP is independently enabled for another local client:

- bind remains loopback-only unless an authenticated TLS reverse proxy owns exposure;
- bearer credentials have narrow scopes and constant-time comparison;
- browser use requires an explicit CORS/preflight policy rather than relying on `Origin` validation alone;
- token files must have platform-appropriate owner/ACL validation, including Windows DACL handling;
- live notification support must be explicitly negotiated;
- Desktop does not silently downgrade local host stdio or SSH private-endpoint transport to standalone HTTP.

Local daemon mode, WebSocket, and general-purpose named-pipe/socket transports require separate specifications. The narrowly scoped remote Unix-socket/loopback endpoint and SSH tunnel are specified in `07-ssh-remote-workspaces.md`.

## Diagnostics and Privacy

Renderer-visible diagnostics use stable codes and sanitized messages. Raw errors go only to bounded local tracing/logging configured by the user.

Secret scrub tests include sentinels in:

- provider errors;
- provider authentication or configuration failures;
- SQLite/replay errors;
- environment endpoint metadata;
- host stderr;
- subscription failure notifications;
- active generated `host.event` deliveries whose `delivery.record.event.kind` is `run_status_changed` or a typed diagnostic variant;
- updater URLs and headers.

Diagnostic export requires explicit user action, previews included files, excludes provider credential files, RPC transport-token files, and the session database by default, and redacts home/workspace paths when possible.

## Update Security

Runtime update trust, checksums, staged activation, downgrade policy, and rollback are specified in `06-runtime-updates-and-release.md`. The renderer cannot override a failed signature/checksum or force activation of an incompatible binary.

## Acceptance Gates

- The Desktop-required operation manifest contains no provider credential lifecycle methods; local and remote provider secrets never enter frontend IPC, Desktop logs, or cross-domain forwarding.
- Config update, reload commit, activate, and discard cannot gain mutation authority from renderer intent alone; native confirmation or managed policy is bound to the exact domain, revision, candidate fingerprint, changed categories, and one operation. Side-effect-free validate/dry-run remains available to derive that fingerprint under bounded config-inspection authority.
- Missing provider configuration is projected only as a typed secret-free host error and does not cause Desktop to start a login flow.
- Question → typed answer → durable decision → resume → model-visible result passes RPC subprocess E2E.
- Approval-only HTTP credentials receive authorization failure for `run.resume` and all execution methods.
- Renderer compromise tests cannot send arbitrary RPC or read credentials/storage.
- Local sandboxed shell tests reject absolute-path, parent-traversal, symlink, subprocess, and sibling-root escapes; unsandboxed native local shell remains disabled by default.
- Remote native shell tests and UI fixtures consistently disclose full authenticated-account authority and never claim workspace containment.
- Path-checked filesystem traversal and changed-authority continuation fail closed independently of shell policy.
- Oversized local stdio or SSH-private-endpoint frames, slow consumers, duplicate subscription IDs, and immediate resubscribe remain bounded and deterministic.
- Blocking state-file locks do not stall run heartbeat or shutdown workers.
- Live and terminal errors pass secret-sentinel projection tests.
- SSH host-key, askpass, static-config allowlist/denylist, exact private-endpoint forwarding, environment disablement, login-shell bootstrap, command-injection, execution-host exclusivity, provisioning, and partition/reconnect tests pass before remote public readiness.
- Windows standalone HTTP transport-token ACL policy is implemented before that independent transport is publicly supported on Windows.
