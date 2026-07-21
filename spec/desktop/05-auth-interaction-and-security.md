# Authentication, Interaction, and Security

Status: accepted architecture baseline; implementation planned

Desktop introduces a privileged local UI around filesystem, shell, model, and durable-control capabilities. Its security boundary is the Desktop backend plus the workspace-scoped RPC child, not the renderer.

## Threat Model

The design must account for:

- compromised or injected renderer content;
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
- read environment variables, OAuth files, RPC token files, or SQLite;
- select unrestricted workspace paths without a native backend grant flow;
- decide authorization scopes;
- install or activate runtime updates;
- access raw stderr, SSH prompts, provisioning channels, or internal error chains.

All external links, file reveals, shell actions, and credential flows pass through explicit backend commands and platform validation.

## OAuth Contract

RPC owns provider authentication by using `starweaver-oauth` and `starweaver-oauth-provider` in its execution domain. A remote RPC uses remote credentials and provider environment; Desktop receives only typed safe projections and never forwards the local OAuth store over SSH.

Required methods or equivalent typed operations:

- `auth.status`;
- `auth.login.start`;
- `auth.login.poll` or progress notifications;
- `auth.refresh`;
- `auth.logout`;
- `auth.account.select` if multiple accounts become supported.

Safe status may include provider, account label allowed by the provider, expiry/refreshability, scopes summary, and required user action. It never includes access tokens, refresh tokens, authorization headers, raw JWTs, or auth-file contents.

Device/login URLs are opened only after validating scheme and provider identity. User-entered codes are treated as sensitive transient data and are not logged or persisted in renderer state.

Because multiple workspace children may share one OAuth store, credential access must be process-safe across refresh and write, not only atomic at the final file replacement. Refresh uses a provider/account generation check or an inter-process refresh lease so rotating refresh tokens cannot be consumed concurrently and overwritten by a stale child. A child that loses the refresh race reloads the durable credential before retrying; it never forwards tokens through the Desktop supervisor to synchronize peers.

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

- `approval` may inspect and decide approval/deferred records;
- `run` is required to start, resume, steer, interrupt, or otherwise cause execution;
- `run.resume` requires `run` authority and may additionally require `approval` when the caller also submits the decision;
- one scope never implicitly grants another unless the protocol explicitly defines a composite credential;
- stdio inherits authority from the Desktop backend process but applies the same semantic checks in service code where practical.

An approval includes durable identity, expected revision/fence, actor, reason, normalized decision metadata, and idempotency key. The backend shows the effective tool, arguments, workspace/environment, capability grant, and risk summary before submission.

## Clarifying Questions

Clarifying questions require a typed answer contract. Marking an approval `approved` without persisting normalized answers is invalid.

The resolution path must:

1. load the durable question/approval and verify tool identity and pending state;
2. validate selected options and free-text answers against the original schema;
3. normalize answers through shared user-input preprocessing;
4. atomically persist decision metadata or approved override arguments;
5. return a durable decision receipt;
6. resume only through normal fenced continuation admission;
7. expose the sanitized answer to the model/tool result exactly once.

RPC must provide a question-to-answer-to-resume E2E test. Until this path exists, Desktop and RPC must not advertise clarifying-question support.

## Deferred Tools

Deferred resolution follows the same durable discipline:

- list pending records after reconnect;
- validate expected revision/fence;
- resolve with a stable idempotency key;
- persist normalized result/error without raw secrets;
- resume through the normal waiting-run continuation path;
- prevent one window from replacing another window’s terminal decision.

## Workspace and Tool Authority

Each execution host receives one canonical workspace identity. Local filesystem tools use path/capability grants intersected with host policy and cannot infer authority from historical session metadata. Remote paths are canonicalized by the remote RPC and never by local filesystem APIs.

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

## Stdio Framing and Process Security

The local and SSH-carried stdio transports must have an inbound byte limit before allocating a complete line. An unbounded `lines()` decoder is not sufficient for the Desktop boundary.

Required controls:

- bounded incremental frame decoding;
- advertised maximum request/notification/result sizes;
- bounded outbound queue and slow-consumer policy;
- generation-safe subscriptions so an old tail cannot delete a newly reused subscription ID;
- no inherited stdin/stdout handles beyond the intended child;
- clean environment allowlist rather than forwarding all Desktop environment variables;
- bounded stderr capture with secret scrubbing;
- no shell interpolation in local child launch arguments;
- SSH remote commands are fixed backend-owned templates; workspace paths, provider/profile values, launch envelopes, and renderer text travel only in bounded typed frames;
- login-shell output is bounded and ignored until an exact nonce-bound RPC marker, after which stdout purity is strict;
- process-tree or SSH-channel termination on forced shutdown.

The Desktop backend avoids long blocking calls on the command connection. It uses non-blocking `run.start` plus subscription/replay. RPC should eventually support concurrent dispatch with an ordered response writer, but Desktop must not depend on `run.await`, blocking `run.prompt`, or long environment probes for responsive stop/steer/shutdown behavior.

## RPC Runtime Safety Prerequisites

Before Desktop public release, RPC must close these host-side gaps:

- ordinary expired run admissions are reconciled periodically after startup;
- clarifying answers are typed, validated, and atomically persisted;
- `run.resume` requires run authority and cannot be invoked by an approval-only credential;
- subscription registry removal is generation-safe across immediate unsubscribe/resubscribe;
- synchronous client-state file locks and I/O move off Tokio runtime workers with bounded waiting;
- live subscription/status errors pass through the same safe public error projection as request errors;
- stdio frame sizes are bounded before allocation;
- session/run/interaction list queries are storage-bounded and paginated.

These are implementation prerequisites discovered by the Desktop readiness review, not optional UI polish.

## SSH Transport Security

System OpenSSH transport, host-key verification, askpass mediation, effective-config restrictions, provisioning isolation, account-authority disclosure, and reconnect behavior are normative in `07-ssh-remote-workspaces.md`.

Agent, X11, port, socket, local/remote-command, multiplexing, and environment forwarding are disabled. V1 invokes OpenSSH only with a generated least-authority configuration produced by a non-executing allowlist importer; it rejects `Match exec`, command/helper/provider loaders, `SetEnv`/`SendEnv`, recursive `Include`, and other non-allowlisted directives before OpenSSH parses them. The local SSH agent may authenticate but is never forwarded. Unknown keys require explicit native confirmation; changed keys fail closed. Remote component installation runs on a separate bounded provisioning channel and uses the signed, exact-version public Starweaver installer contract rather than renderer-authored commands or unpinned `curl | sh`.

## HTTP and Future Transports

Desktop v1 does not expose RPC HTTP to the renderer. If HTTP is enabled for another local client:

- bind remains loopback-only unless an authenticated TLS reverse proxy owns exposure;
- bearer credentials have narrow scopes and constant-time comparison;
- browser use requires an explicit CORS/preflight policy rather than relying on `Origin` validation alone;
- token files must have platform-appropriate owner/ACL validation, including Windows DACL handling;
- live notification support must be explicitly negotiated;
- Desktop does not silently downgrade from stdio to HTTP.

Unix domain sockets, Windows named pipes, WebSocket, and daemon mode require separate transport specifications.

## Diagnostics and Privacy

Renderer-visible diagnostics use stable codes and sanitized messages. Raw errors go only to bounded local tracing/logging configured by the user.

Secret scrub tests include sentinels in:

- provider errors;
- OAuth failures;
- SQLite/replay errors;
- environment endpoint metadata;
- child stderr;
- subscription failure notifications;
- active `run.status` error projections;
- updater URLs and headers.

Diagnostic export requires explicit user action, previews included files, excludes OAuth/token files and the session database by default, and redacts home/workspace paths when possible.

## Update Security

Runtime update trust, checksums, staged activation, downgrade policy, and rollback are specified in `06-runtime-updates-and-release.md`. The renderer cannot override a failed signature/checksum or force activation of an incompatible binary.

## Acceptance Gates

- Existing local and remote OAuth credentials can be reused without token projection into frontend IPC or copying between execution domains.
- Full login, refresh, expiry, logout, restart, and concurrent multi-child refresh flows pass deterministic tests.
- Question → typed answer → durable decision → resume → model-visible result passes RPC subprocess E2E.
- Approval-only HTTP credentials receive authorization failure for `run.resume` and all execution methods.
- Renderer compromise tests cannot send arbitrary RPC or read credentials/storage.
- Local sandboxed shell tests reject absolute-path, parent-traversal, symlink, subprocess, and sibling-root escapes; unsandboxed native local shell remains disabled by default.
- Remote native shell tests and UI fixtures consistently disclose full authenticated-account authority and never claim workspace containment.
- Path-checked filesystem traversal and changed-authority continuation fail closed independently of shell policy.
- Oversized stdio frames, slow consumers, duplicate subscription IDs, and immediate resubscribe remain bounded and deterministic.
- Blocking state-file locks do not stall run heartbeat or shutdown workers.
- Live and terminal errors pass secret-sentinel projection tests.
- SSH host-key, askpass, static-config allowlist/denylist, forwarding/environment disablement, login-shell bootstrap, command-injection, execution-host exclusivity, provisioning, and partition/reconnect tests pass before remote public readiness.
- Windows token-file ACL policy is implemented before Windows public readiness.
