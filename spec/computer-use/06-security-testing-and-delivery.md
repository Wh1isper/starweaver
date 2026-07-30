# Computer Use Security, Testing, and Delivery

Status: **Accepted release gates; macOS observation subset implemented**
Scope: attended control of the current active interactive desktop
Depends on: [`01-product-boundaries-and-ownership.md`](01-product-boundaries-and-ownership.md), [`02-service-contract-and-state-machine.md`](02-service-contract-and-state-machine.md), [`03-toolset-and-library-integration.md`](03-toolset-and-library-integration.md), [`04-native-active-desktop-backends.md`](04-native-active-desktop-backends.md), [`05-mcp-binary-and-process-lifecycle.md`](05-mcp-binary-and-process-lifecycle.md)

## 1. Purpose

Computer Use combines screen observation with synthetic pointer and keyboard input in the user's real active desktop. This is authority comparable to attended remote control. This document defines the minimum security, privacy, testing, and release contract for the typed library, Starweaver Toolset adapter, and MCP binary.

The terms **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** are normative.

Security requirements apply equally to the CLI/RPC in-process Toolset path and the non-Starweaver harness path through `starweaver-computer-use-mcp`. An adapter cannot weaken the service contract because its caller is a CLI/RPC-hosted agent or an MCP harness.

## 2. Security invariants

The following invariants are mandatory:

01. The screen, accessibility metadata, model output, tool caller, and MCP client are untrusted input.
02. The process operates only the current local user's active, unlocked, visible interactive desktop.
03. Tool arguments cannot select another process, window, user, session, seat, display server, native handle, or remote endpoint.
04. Build support, OS permission, startup policy, user-presence state, current-session state, and observation basis are all required before an effect.
05. Observation, pointer, and keyboard are separate capabilities. One does not imply another.
06. Every pointer or keyboard action cites one opaque current-session `observation_id`; the service resolves and revalidates its full geometry/frame basis immediately before native input.
07. High-level actions are balanced; no public raw key/button-down authority persists across calls.
08. The service and MCP binary persist no screenshots, text input, key sequences, portal tokens, or live desktop authority by default.
09. Lock, secure desktop, session switch, seat loss, permission loss, portal close, physical takeover, cancellation, or emergency stop invalidates queued input.
10. Protected, elevated, or unsupported surfaces fail closed. No fallback elevates, widens scope, switches mechanism, or crosses a session boundary.
11. A production input-capable build requires an attended, same-process `UserPresenceGuard` accepted for that OS support target.
12. A model or MCP client can never resume input authority after user takeover by calling a tool.
13. Browser/CDP, remote/VM, unattended, helper/daemon/service, privileged, network MCP, and provider-native paths remain absent.

## 3. Threat model

### 3.1 Protected assets

Assets include:

- pixels visible on the current desktop;
- text inferred from pixels or optional accessibility data;
- current focus and display topology;
- pointer/keyboard input authority;
- permission and portal session handles;
- the user's ability to see, interrupt, and retake control;
- tool schemas, policy, and capability grants;
- process/signing identity and package integrity; and
- diagnostics that may reveal application or session state.

### 3.2 Adversaries and failure sources

The design assumes possible malicious or malformed behavior from:

- text or UI content shown on screen, including prompt injection;
- model-generated coordinates, text, key chords, scroll deltas, and stale observations;
- an MCP client that sends arbitrary calls, races cancellation, or floods the server;
- CLI/RPC host configuration or an MCP launch configuration that misconfigures policy;
- another same-user process that changes focus, display topology, session state, or UI contents;
- applications that reject or reinterpret synthetic input;
- protected/elevated UI outside the process's authority;
- compromised or buggy native libraries and platform APIs;
- physical user actions racing synthetic actions;
- malformed images, accessibility trees, and platform metadata;
- process crash, hard kill, OS sleep, lock, switch, and device removal; and
- packaging/signing/update mistakes that change permission identity or load untrusted libraries.

The baseline does not claim protection from a malicious process already running as the same OS user with equivalent desktop authority. Process separation over stdio is a product boundary, not an OS sandbox.

### 3.3 Trust boundaries

```mermaid
flowchart LR
    caller[Untrusted model or MCP caller]
    adapter[Toolset or MCP adapter]
    router[Canonical tool router]
    policy[Startup policy and capability grant]
    service[ComputerUseService]
    presence[UserPresenceGuard]
    backend[Native backend]
    os[Current interactive desktop]
    user[Attended user]

    caller -->|untrusted typed request| adapter
    adapter --> router
    policy --> router
    router --> service
    presence --> service
    service --> backend
    backend --> os
    user -->|physical input, stop, resume| presence
    os -->|untrusted pixels and metadata| backend
```

Only the service/native backend owns OS handles. The router owns canonical request validation. Startup policy and user presence are independent gates. Neither screen content nor caller intent is authority.

## 4. Startup policy and capability grants

### 4.1 Three capabilities

The canonical authority classes are:

- `observe`: status and current-desktop screenshots;
- `pointer`: click, move, drag, and scroll; and
- `keyboard`: text and key/chord input.

Pointer and keyboard require observe because every successful effect returns a fresh post-action observation. They remain independently granted.

For the MCP binary, launch policy is the maximum grant. Default `--stdio` grants observe only. Pointer and keyboard require explicit host/user-controlled options.

For the Starweaver adapter, CLI or RPC installs per-tool `ToolCapabilityGrant` values and named typed handles as specified by `03-toolset-and-library-integration.md`. The model cannot attach a handle or modify a grant.

CLI effective authority is additionally bounded by CLI-owned process configuration and per-tool grants. Enabling `[computer_use]` automatically materializes the Toolset into every effective CLI profile; profiles do not create or widen native authority. RPC effective authority is additionally bounded by RPC-owned server configuration and an admitted initiating-caller grant. Enabling RPC Computer Use likewise materializes the Toolset into every effective RPC profile, but merely reaching RPC over stdio or HTTP does not attach an authorized handle or grant input. The generic RPC `run` scope grants none of `computer.observe`, `computer.pointer`, or `computer.keyboard`; these are separate principal capabilities, default denied, and pointer/keyboard also require observe. When enabled, effects occur on the RPC host's current local desktop, never on the RPC client's machine.

RPC binds a process-local admitted grant to each `run_id`, principal fingerprint, authorization generation, expiry, immutable config/profile ceiling, and effective capability set. Every Computer Use call checks it, and every effect checks again before entering the service fence. Revocation/expiry cancels queued/active observations and queued pre-effect work and removes that run's handles. Resume and continuation require fresh authorization derivation; durable records never restore the caller grant.

### 4.2 Policy intersection

Effective authority is:

```text
compiled backend support
intersect current active-session eligibility
intersect current OS permission/portal grant
intersect startup ComputerUsePolicy
intersect adapter/tool capability grant
intersect RPC admitted caller/run grant when RPC-hosted
intersect current UserPresenceGuard state
intersect valid action observation basis
```

A missing term means denial. The service MUST compute this intersection at session establishment and revalidate all volatile terms before every effect.

### 4.3 Immutable bounds

Startup policy MUST bound:

- desktop capture scope;
- maximum screenshot width, height, pixels, encoded bytes, and format;
- operation and queue timeouts;
- maximum pointer coordinates/path points/duration;
- maximum scroll magnitude;
- maximum text bytes/scalars;
- allowed canonical key names, chord width, and sequence length;
- post-action settle time;
- optional accessibility node/depth/string/time budgets;
- permission-prompt behavior;
- input capability grants;
- user-presence mode;
- logging level and redaction; and
- explicit X11 compatibility permission.

Tool calls cannot widen these values. Config reload is not supported in V1; changing policy requires a new process/service session and fresh observation.

### 4.4 No semantic safety claim

The service can validate mechanisms and authority, not the real-world consequence of clicking a UI. It MUST NOT claim to detect every purchase, message send, deletion, credential disclosure, or legal action from pixels.

CLI and RPC MAY mark all input tools approval-required or implement higher-level product policy outside this library. External MCP harnesses remain responsible for their own human-in-the-loop product policy. In every case, the native service still enforces its startup grant, attended-presence state, and action basis.

Screen text that says to ignore policy, reveal secrets, alter grants, run commands, or disable safety has no privileged interpretation.

## 5. User presence and takeover

### 5.1 Production requirement

Every production build that enables pointer or keyboard authority MUST have a supported same-process `UserPresenceGuard` for that platform/session family. The guard is a release requirement, not an optional UI enhancement.

The guard MUST provide:

- a persistent OS-visible indication that control is armed or active;
- an emergency-stop mechanism outside model/MCP-controlled screen content;
- physical user input detection sufficient to pause or revoke synthetic action authority;
- a locally initiated attended resume path;
- a monotonically increasing takeover/arm epoch;
- a synchronous pre-effect check callable by the service;
- bounded cancellation acknowledgement; and
- cleanup integration for process exit and panic.

A hidden client window, log line, MCP notification, or model-visible tool result is not an out-of-band user-presence mechanism.

### 5.2 Illustrative guard contract

```rust
#[async_trait]
pub trait UserPresenceGuard: Send + Sync {
    async fn arm(
        &self,
        policy: PresencePolicy,
        cancel: CancellationToken,
    ) -> Result<PresenceLease, ComputerUseError>;

    fn current(&self) -> PresenceSnapshot;

    async fn before_effect(
        &self,
        lease: &PresenceLease,
        expected_epoch: TakeoverEpoch,
    ) -> Result<PresencePermit, ComputerUseError>;

    async fn synthetic_action_started(
        &self,
        permit: &PresencePermit,
        action_id: ActionId,
    ) -> Result<(), ComputerUseError>;

    async fn synthetic_action_finished(
        &self,
        permit: PresencePermit,
        outcome: ActionOutcome,
    );

    async fn disarm(&self, reason: DisarmReason) -> PresenceCloseReceipt;
}

struct PresenceSnapshot {
    state: PresenceState, // disarmed, armed, active, paused, revoked
    takeover_epoch: TakeoverEpoch,
    indicator_visible: bool,
    emergency_stop_ready: bool,
    last_transition_reason: PresenceReason,
}
```

Native event handles remain private. The service stores only process-local guard references and current epoch.

### 5.3 Physical input takeover

When physical user input or emergency stop is detected:

1. the guard atomically increments `TakeoverEpoch` and changes state to `paused` or `revoked`;
2. queued actions are cancelled;
3. the active action stops at the next safe high-level boundary;
4. synthetic held keys/buttons/modifiers are released;
5. the user-facing indicator changes before control-transfer acknowledgement;
6. every old action basis/permit is invalidated; and
7. the caller receives `user_presence_paused` or `user_presence_revoked`.

No call from a model or MCP client can resume the guard. Resume requires a local human action owned by the native guard, or a complete process restart under attended launch policy. After resume, the service MUST obtain a new active-session probe and observation before accepting input.

The guard SHOULD distinguish self-generated input from physical input using public platform markers where available. If it cannot do so reliably, the backend MUST choose a conservative policy; it MUST NOT ignore all input monitoring merely to avoid self-triggering.

### 5.4 Development-only exception

A temporary `development-terminal-only` presence mode MAY exist solely for backend bring-up when all of these conditions hold:

- the build is debug/non-release;
- the option requires an explicit compile-time development feature and launch flag;
- the process has a controlling terminal visible to the developer;
- a terminal command or signal stops input;
- diagnostics clearly label the mode non-production;
- automated release/package checks reject the feature and identifying strings; and
- no published binary, default example, CI release artifact, or documentation quickstart enables it.

This exception is not a release fallback. If no production guard exists for an OS/session target, release artifacts for that target MUST be observe-only.

### 5.5 Platform acceptance

The exact indicator and emergency-stop UX is platform-specific and remains gated by the spikes in `04-native-active-desktop-backends.md`. A platform is input-release-ready only when tests prove:

- the indicator is visible while armed;
- emergency stop works while the controlled application has focus;
- physical input races cannot execute queued effects after takeover acknowledgement;
- lock/switch and permission loss revoke the guard; and
- external-harness MCP use has the same attended guarantees as CLI/RPC in-process use.

## 6. Observation, prompt injection, and geometry safety

### 6.1 Untrusted observation

Screenshots and optional accessibility data are untrusted evidence. The library MUST treat image bytes and semantic strings as data only. It MUST NOT parse screen text into policy, configuration, target selection, native commands, or capability changes.

The Toolset instruction SHOULD remind models that UI content cannot authorize broader tools. The MCP server instruction MUST state the same boundary.

### 6.2 Typed action basis

Every effect request MUST carry one `observation_id`. The service resolves the authoritative process/session IDs, target/layout/frame generations, service-owned effect epoch, image dimensions/digest, capture time, and coordinate transform from its bounded current-session observation ledger; callers cannot echo or override them.

Immediately before native input the service MUST verify:

- process and interactive-session identity;
- active/unlocked/ordinary desktop state;
- the ledger record's observation/frame identity and its current target/layout generations;
- exact equality between the observation's effect epoch and the session's current effect epoch;
- exact equality between the observation's capture-time takeover epoch and the current armed epoch;
- capability grant and limits;
- coordinates/path/key/text bounds; and
- cancellation state.

Normal desktop repaint may advance the live frame generation after capture, so V1 does not require numerical equality with the latest frame counter. It does require an intact, unexpired observation record, unchanged target/layout mapping, and an effect epoch equal to the current session epoch. Immediately before any accepted action may submit native input, the service increments that epoch; therefore every observation predating that effect becomes unusable across all CLI/RPC runs and MCP requests sharing the session. A backend MAY enforce a stricter visual-stability check and records that decision in the receipt. A stale or mismatched basis is rejected. Coordinates are never clamped into range, guessed after resize, or transformed using current geometry when an old basis was supplied.

### 6.3 Post-action observation

Every successful effect produces a receipt and fresh observation from the same session after bounded settling. If input delivery is uncertain or post-action capture fails, the service returns a typed non-success/uncertain outcome; it MUST NOT claim success solely because an input API accepted events.

The post-action image does not prove semantic success. Receipts distinguish requested, accepted, delivered-known, delivery-uncertain, cancelled, and rejected states where the platform can support those distinctions.

## 7. Protected and elevated surfaces

The following are always out of scope:

- macOS login/lock/authorization protected UI and protected capture content;
- Windows Winlogon/UAC/credential secure desktops and higher-integrity targets blocked by UIPI;
- Linux login/lock screens, another seat/user, portal-denied sources, and compositor-private authority;
- DRM/protected video or application content that the OS redacts; and
- any application surface requiring elevation, injection, private API, helper, service, kernel input, or another session.

When encountered, the backend MUST:

1. stop before effect when detectable;
2. cancel queued effects whose assumptions may include that surface;
3. release held input;
4. invalidate the relevant observation/session generation;
5. return a typed `protected_surface`, `secure_desktop_unavailable`, `integrity_boundary`, `portal_scope_mismatch`, or equivalent safe error; and
6. require fresh attended recovery and observation.

It MUST NOT fall back to another capture API, X11/XWayland, `/dev/uinput`, Accessibility/UIA/AT-SPI actions, clipboard insertion, application scripting, process injection, or elevation to bypass the boundary.

## 8. Keyboard, text, pointer, and clipboard rules

### 8.1 Balanced high-level actions

V1 exposes no public key-down, key-up, button-down, or button-up tools. A high-level click, drag, key sequence, or chord owns its complete native press/release sequence.

Each action implementation MUST use an RAII or equivalent cleanup guard recording synthetic held state. The guard attempts release on:

- normal completion;
- validation failure after partial preparation;
- cancellation and timeout;
- backend error;
- session/presence revocation;
- panic crossing a safe boundary; and
- graceful process shutdown.

Hard process kill cannot guarantee async cleanup, so held intervals MUST be minimal.

### 8.2 Text input

Typed text is highly sensitive. The service MUST NOT log, persist, hash for telemetry, or echo full text in receipts. Safe receipts include only bounded counts and status.

V1 MUST NOT silently use the system clipboard to type text. Platform-native Unicode synthesis or an explicitly supported non-clipboard mechanism is required. If a character cannot be represented safely on a backend, the action returns `unsupported_text_input`; it does not substitute, drop, or paste through the clipboard.

Passwords and secrets cannot be reliably detected. Higher-level callers are responsible for deciding whether to type sensitive values. The library's zero-retention and redaction rules still apply.

### 8.3 Key and pointer limits

Canonical key names are a closed allowlist. The service MUST reject unknown/native key codes, over-wide chords, excessive sequences, and system-reserved combinations prohibited by policy.

Pointer coordinates, drag paths, duration, click count, and scroll deltas are bounded. Effects outside the observation surface or on a changed geometry are rejected rather than clamped.

## 9. Screenshot and semantic-data privacy

### 9.1 Zero retention baseline

The Computer Use library and MCP binary have a zero-retention baseline:

- screenshots are held in memory only long enough to normalize, encode, map, and write the current result;
- no screenshot directory, cache, history, thumbnail, replay store, crash attachment, or automatic evidence archive exists;
- structured results contain geometry and IDs, not base64 images;
- images are not written to temporary files;
- optional accessibility trees and strings are not persisted;
- typed text and key sequences are not retained after the action completes; and
- process restart restores no observation or authority.

CLI, RPC, or an external MCP harness may retain model/tool history according to its own policy. That is outside the library/MCP binary retention guarantee and SHOULD be disclosed by the calling product or harness.

### 9.2 Memory handling

The implementation MUST bound raw and encoded frame buffers before allocation where possible. It SHOULD reuse bounded pools only within the live process and SHOULD overwrite or release sensitive buffers promptly when practical. Rust memory zeroization is not a guarantee against all allocator/OS copies; documentation MUST not claim cryptographic erasure.

Core dumps and crash reporters can capture pixels or text. Production packaging SHOULD disable content-rich automatic crash attachments and document OS-level core-dump policy. Panic diagnostics MUST not format image/text DTOs.

### 9.3 Optional semantic observation

Accessibility metadata is independently gated and bounded. It MUST exclude or redact known secure/password values where platform APIs identify them, cap node/depth/string/time/total bytes, and return truncation metadata. It MUST not be included merely because pixel capture is authorized.

## 10. MCP and adapter security

### 10.1 MCP caller

Stdio limits transport reachability but does not make tool arguments trusted. The server MUST validate every call through the canonical router and must not deserialize arbitrary native extensions.

The MCP process inherits environment and working directory from its parent, but tool results and logs MUST not expose them. Tool calls have no filesystem, shell, subprocess, network, or config mutation authority.

### 10.2 Standard-stream safety

In server mode stdout contains only MCP framing. All diagnostic and native-library output is redirected to stderr or disabled. Fault-injection tests cover permission prompts, panic hooks, tracing initialization, and shutdown.

### 10.3 Denial of service

The router and server MUST bound:

- concurrent/queued calls;
- JSON depth and input size;
- schema/text/path/key limits;
- image dimensions, raw pixels, encoded bytes, and encode time, while geometry-bound observation images remain immutable after basis creation;
- accessibility traversal;
- operation/queue/shutdown timeouts; and
- diagnostic/log volume.

A flooding client cannot create unbounded sessions, portal dialogs, capture streams, frame buffers, or cancellation entries.

## 11. Cancellation, idempotency, and ambiguous effects

### 11.1 Cancellation fence

Cancellation is checked before queue admission, after queue wake, before native effect, at safe points during long drag/key sequences, before post-action capture, and during image encoding. A cancelled action is never retried automatically.

Cancellation response is not complete until synthetic held-state cleanup reaches a terminal best-effort result. Cleanup outcome appears in the safe receipt/error.

### 11.2 Idempotency

Observations and status probes MAY be retried when policy permits. Input effects MUST NOT be retried by the service or MCP adapter after an ambiguous transport/backend failure.

Canonical input adapters supply an operation/idempotency identity in the invocation envelope rather than asking the model/MCP tool arguments to manufacture one. Repeating an ID with the exact canonical request digest returns the recorded bounded receipt only while that process-local cache is valid. Reusing an ID with different content is rejected. The cache MUST NOT survive process restart and MUST NOT retain screenshot bytes or text bodies.

### 11.3 Delivery uncertainty

Some OS input APIs report submission, not application consumption. The service distinguishes API acceptance from semantic effect. If cancellation, session change, or backend failure occurs after possible native submission, the result is `delivery_uncertain` with no automatic retry. A fresh observation is required.

## 12. Redacted observability

### 12.1 Allowed fields

Local structured logs and metrics MAY include:

- backend/platform and safe version;
- canonical tool/action class;
- process-local correlation IDs;
- capability class;
- permission/status/error code;
- operation phase and bounded duration;
- image dimensions and encoded byte count;
- generation mismatch category;
- cancellation/takeover/cleanup outcome; and
- package/signing classification without user identity.

### 12.2 Forbidden fields

Logs, traces, metrics, panic messages, and crash attachments MUST NOT contain:

- image bytes or screenshot-derived text;
- full image/content hashes by default;
- typed text, key values, or clipboard data;
- accessibility names/values/descriptions;
- window/application titles;
- usernames, home directories, process command lines, or environment variables;
- native handles, portal restore tokens, D-Bus paths carrying authority, PipeWire/EIS FDs, or X authorization;
- MCP raw arguments/results; or
- model prompts/responses.

Telemetry export is absent by default. Adding network telemetry requires a later privacy/security decision.

## 13. Test architecture

Testing is layered so most invariants are deterministic and platform tests focus on native behavior.

```mermaid
flowchart BT
    unit[Pure unit and property tests]
    schema[Canonical schema and adapter parity]
    fake[Deterministic fake service/toolset/MCP]
    native[Platform current-desktop integration]
    permission[Permission, lock, takeover, and failure tests]
    package[Installed package and release-artifact tests]

    unit --> schema
    schema --> fake
    fake --> native
    native --> permission
    permission --> package
```

### 13.1 Pure unit and property tests

Required tests cover:

- service-state transitions and forbidden transitions;
- policy intersection and default denial;
- capability independence;
- observation/action basis validation;
- affine transform round trips, crop/scale/rotation/mixed-DPI, negative desktop origins, and bounds;
- independent process/session/backend/geometry/frame/effect/presence generations;
- stale, future, wrong-process, wrong-session, wrong-effect-epoch, and wrong-scope bases;
- run A observe → run B effect → run A queued action rejection under one shared RPC coordinator;
- generic RPC `run` authorization exposes no Computer Use handle, observe-only cannot widen to input, and untrusted profile selection cannot exceed principal grants;
- RPC grant revocation cancels queued/active observation and wins against queued pre-effect work, grants do not bleed across principals/runs, and resume/continuation without fresh authorization restores no handle;
- action limits and canonical key validation;
- balanced input cleanup state machine;
- cancellation at every safe boundary;
- idempotency same/different digest behavior;
- error classification and safe projection;
- zero-retention result shape; and
- log redaction.

Property tests MUST prove that arbitrary valid model points round-trip within documented tolerance and arbitrary invalid points never become valid by clamping or overflow.

### 13.2 Canonical schema tests

Checked-in fixtures cover every tool input, structured output, content class, error, catalog metadata, and version.

Tests compare normalized projections for:

- direct canonical catalog;
- Starweaver `ToolDefinition`/Toolset adapter; and
- MCP `tools/list` declarations.

A tool name/schema/description/capability mismatch fails CI. Unknown fields, integer/float edge cases, Unicode limits, oversized arrays, and JSON depth are tested.

### 13.3 Deterministic fake tests

The fake backend simulates:

- active and inactive sessions;
- permissions granted/denied/revoked/restart-required;
- multi-display and geometry changes;
- blank/protected/redacted frames;
- accepted/rejected/uncertain input;
- physical takeover and emergency stop;
- lock/session switch/portal close;
- cancellation during every action phase;
- held-state cleanup success/failure;
- image encoding limits; and
- optional accessibility truncation.

The same scenario suite runs through typed library calls, the shared CLI/RPC Toolset adapter, and in-process MCP server mapping. Separate composition fixtures prove both CLI and RPC attach one process-level coordinator without routing through MCP.

### 13.4 MCP tests

In-process and subprocess tests required by `05-mcp-binary-and-process-lifecycle.md` cover protocol negotiation, catalog filtering, image/structured content, errors, request cancellation, bounded concurrency, EOF/signal shutdown, stdout purity, stderr redaction, and negative surface/transport checks.

Tests MUST use clients/fixtures matching the resolved workspace `rmcp` version and at least one independently maintained MCP client or protocol fixture before release.

### 13.5 Platform integration tests

Tests run in dedicated attended CI/manual lanes; they MUST NOT automate a developer's ordinary desktop. Each platform uses a controlled test user/session with deterministic test windows and no sensitive data.

Common scenarios:

- observe known color/geometry patterns;
- click/hover/drag/scroll and verify test-app state;
- type supported Unicode and key chords into a controlled test app;
- cancel at every native action boundary;
- resize, rotate, scale, add/remove display where feasible;
- lock, unlock, switch session/seat, sleep/wake, and disconnect events;
- physical input and emergency-stop races;
- protected/elevated UI refusal;
- permission denial/revocation;
- process termination and held-state cleanup; and
- packaged artifact identity.

Assertions use test-application state and typed receipts, not screenshots of arbitrary user applications.

## 14. Platform-specific security matrix

### 14.1 macOS

Required evidence:

- public ScreenCaptureKit/CoreGraphics/AX/CGEvent baseline;
- stable TCC identity for actual signed package;
- Screen Recording and Accessibility denial/onboarding/restart behavior;
- active console and lock/Fast User Switching failure;
- Retina/mixed-scale/multi-display geometry;
- protected/redacted frame handling;
- event-tap or alternative same-process physical-input detection;
- native indicator/emergency stop while another app has focus;
- same-process Swift shim integrity if selected; and
- update continuity without helper/XPC/private API.

Input release is blocked until Developer ID/signing/notarization policy is accepted for the actual artifact. Ad-hoc builds are development-only.

### 14.2 Windows

Required evidence:

- accepted WGC versus Desktop Duplication decision and supported OS matrix;
- Per-Monitor V2 DPI and topology correctness;
- medium-integrity current-session process;
- WTS/input-desktop/lock/console-RDP transition validation;
- UIPI/higher-integrity and secure desktop refusal;
- `SendInput` rejection/uncertainty/cleanup semantics;
- physical-input detection and synthetic marker handling;
- native indicator/emergency stop;
- Authenticode/installer or accepted publisher identity; and
- actual package DLL search-path protection.

No input release may depend on `uiAccess`, elevation, service, injection, or secure-desktop switching.

### 14.3 Linux Wayland

Required evidence per supported compositor/desktop family:

- portal ScreenCast + PipeWire capture;
- RemoteDesktop/EIS input capability and portal version floor;
- logical/pixel/stream coordinate correctness;
- D-Bus/portal/PipeWire/EIS loss cleanup;
- portal cancellation and scope mismatch;
- lock/seat/session transition failure;
- physical takeover detection where available;
- persistent compositor or same-process native control indicator/emergency stop; and
- package-native library discovery and provenance.

If a desktop family cannot provide the required production `UserPresenceGuard`, its release artifact is observe-only.

### 14.4 Linux X11

X11 tests run only with explicit compatibility policy. They prove current-user/current-display binding, the declared desktop/session-manager lock and user-switch signals, XShm/XTest behavior, broad-authority disclosure, physical takeover, cleanup, and no Wayland-denial fallback. X11 input remains disabled where active/unlocked state cannot be proven independently of the X connection. No test or package may require `uinput`, evdev reads, root, input-group membership, setuid, or another user's X authority.

## 15. Permission and package tests

Testing only a development executable is insufficient. For each release platform, CI/manual evidence MUST cover the bytes users install. Evidence applies independently to every input-capable `starweaver-cli`, `starweaver-rpc`, and `starweaver-computer-use-mcp` executable identity; passing the MCP artifact does not qualify CLI/RPC, and passing one Starweaver product does not transfer OS permission/signing evidence to another:

- initial install and first permission request;
- denial and remediation;
- process restart;
- in-place signed/package update;
- path or identity change behavior;
- uninstall/reinstall where permission identity may persist;
- rollback if supported by packaging;
- dependency/library integrity and search paths;
- checksum/provenance verification;
- direct in-process Toolset activation, permission diagnostics, coordinator shutdown, and no-MCP-loopback smoke for CLI/RPC artifacts; and
- `--doctor`, `--request-permissions`, `--version`, and stdio smoke for the MCP artifact.

The test plan MUST record which permission behavior is OS-owned and cannot be reset safely in ordinary CI. Such cases require documented disposable test users/VMs for packaging verification only; this does not add VM control to the product.

## 16. Architecture and dependency gates

Implementation changes MUST add repository checks proving:

- `starweaver-computer-use` has no normal dependency on agent, model, runtime, environment, RPC, CLI, session/storage/stream, or graphical-product crates;
- `starweaver-agent` depends on the library with `default-features = false` and without `mcp-server`;
- CLI and RPC dependency/feature graphs use the in-process Toolset/library and never enable the Computer Use `mcp-server` feature or launch the binary for built-in Computer Use;
- RPC Computer Use cannot build/enable until authorization/config contracts implement default-denied `computer.observe`, `computer.pointer`, and `computer.keyboard`, immutable run admission, generation/expiry checks, revocation, and fresh resume admission;
- default library builds contain no `rmcp` server dependency;
- only the feature-gated binary/server module imports `rmcp` server APIs;
- no network server dependency or listening socket path is introduced;
- no browser/CDP/provider-native symbols appear in implementation modules;
- no helper/service/daemon/uinput/elevation path exists;
- public API and schema changes pass release API checks;
- the package inherits the workspace unsafe-Rust prohibition, and every native call is provided through an accepted sound safe dependency/wrapper; any unavoidable unsafe FFI requires a separate architecture/security decision before implementation; and
- each graduated subset is registered in `spec/capabilities.toml` only after implementation and evidence exist; unregistered platform/input capability remains planned.

Existing repository gates such as `make architecture-check`, `make capability-check`, `make fmt-check`, `make check`, `make test`, `make scripts-check`, and `make release-api-check` remain applicable. Implementation batches MUST add focused commands rather than weaken aggregate gates.

## 17. Delivery phases

Current graduation: Phase 0 is implemented; Phase 1 and Phase 2 are implemented for macOS observation only. Windows/Linux observation and Phases 3–5 remain planned. The generated capability status and `../alignment/13-computer-use-macos-evidence.md` are authoritative for the delivered subset.

### Phase 0: contract and deterministic core

Deliver:

- accepted spec set;
- package/API dependency design;
- canonical typed DTOs/catalog/router;
- policy/state machine/error taxonomy;
- RPC caller-principal/run-admission grant contract and negative conformance fixtures;
- deterministic fake backend;
- JSON Schema fixtures; and
- unit/property/redaction tests.

Gate: no native input, MCP binary, or Starweaver adapter lands before contract fixtures pass.

### Phase 1: observation-only native spikes

For macOS, Windows, Wayland, and explicit X11:

- prove same-process current-session capture;
- prove geometry and image limits;
- prove permission/session diagnostics;
- prove lock/switch/protected-content failure; and
- decide target API/dependency/package options.

Gate: each supported backend passes observation-only platform and package-identity evidence. Unsupported targets remain unavailable rather than using a fallback.

### Phase 2: observation adapters

Deliver:

- production library observation path;
- `computer_status` and `computer_observe` Starweaver Toolset adapter;
- CLI and RPC in-process observation composition with one coordinator per product process and explicit grants;
- feature-gated MCP server/binary for non-Starweaver harnesses with default observation-only catalog;
- image/structured-result parity; and
- stdio conformance and zero-retention tests.

Gate: observation-only artifacts may be released independently after privacy, package, and MCP gates pass.

### Phase 3: native input and presence prototypes

Deliver per platform:

- high-level pointer/keyboard backend;
- balanced cleanup/cancellation/idempotency;
- physical takeover detection;
- production candidate native indicator/emergency stop/local resume;
- protected/elevated refusal; and
- controlled test application integration.

Gate: prototypes remain development-only. `development-terminal-only` cannot enter release builds.

### Phase 4: input-capable release

Enable pointer and/or keyboard per platform only after:

- production `UserPresenceGuard` acceptance;
- takeover/cancellation latency tests;
- packaged signing/identity tests;
- permission/onboarding UX validation;
- actual release-artifact integration tests;
- security/code review with no unresolved P0/P1 findings; and
- capability registry evidence.

Capabilities MAY graduate independently. A platform may release observe-only, observe+pointer, or full observe+pointer+keyboard according to evidence.

### Phase 5: optional accessibility metadata

Optional bounded AX/UIA/AT-SPI observation is a separate graduation lane. It MUST NOT delay or destabilize the pixel baseline and MUST not add semantic action tools without a new spec.

## 18. Release gates

### 18.1 Universal release gate

No artifact is released unless:

- all fixed exclusions remain true;
- package dependency checks pass;
- canonical schema/adapters are in parity;
- Computer Use library/MCP-owned screenshot, accessibility, and input buffers follow the zero-retention baseline; CLI/RPC or harness history retention is separately disclosed and never restores native authority; logs are redacted; and the model/MCP client receives the exact geometry-bound image bytes and dimensions recorded by the observation;
- default MCP launch is observe-only;
- stdio framing and cancellation pass;
- native session/permission failures are typed and fail closed;
- RPC generic `run` authorization and profile selection cannot obtain Computer Use without dedicated admitted caller capabilities, and resume/continuation restore no grant;
- protected/elevated surfaces have no fallback;
- actual package bytes pass smoke/provenance checks; and
- documentation clearly distinguishes implemented capability from planned platform support.

### 18.2 Input-specific gate

Pointer or keyboard is enabled in a release only when:

- launch grants are explicit and separate;
- RPC-hosted input additionally has current `computer.observe` plus `computer.pointer` or `computer.keyboard` caller admission bound to the run and checked for revocation/expiry before effect;
- a production same-process `UserPresenceGuard` exists;
- emergency stop is out-of-band and works under controlled focus;
- physical takeover invalidates queued work before acknowledgement;
- local-only resume and fresh observation are enforced;
- held-state cleanup passes fault injection;
- ambiguous delivery is never auto-retried;
- input text/key data is absent from Computer Use-owned logs and persistence; any CLI/RPC or harness history retention follows its separately disclosed product policy and cannot restore authority;
- secure/locked/higher-integrity/session-switch tests pass; and
- target-specific signing/package policy is satisfied.

A failed input gate reduces the artifact to observe-only; it MUST NOT be waived by a warning flag in a release build.

### 18.3 MCP-specific gate

The MCP artifact additionally requires:

- server/cancellation conformance against the resolved workspace `rmcp` version;
- no resources/prompts/sampling/tasks/network capability;
- deterministic launch-policy-filtered `tools/list` that remains stable for the V1 connection;
- structured+image mapping parity;
- stdout purity under fault injection;
- stdin EOF/signal shutdown cleanup; and
- no separate MCP crate, agent/runtime dependency, or generic server framework introduced without new evidence.

## 19. Incident and recovery behavior

If an invariant violation is detected at runtime, the process MUST prefer loss of capability over continuation:

1. disarm user presence;
2. cancel queued/current effects;
3. release held input;
4. close native capture/input authority when safety is uncertain;
5. invalidate observations and generations;
6. return or log only a redacted stable diagnostic code; and
7. require process restart or explicit attended recovery according to error class.

The library and MCP binary do not upload incident data. A user MAY manually provide redacted diagnostics from `--doctor`.

Permission identity change, repeated protected-frame misclassification, cleanup failure, stdout framing corruption, or post-takeover action is release-blocking and should disable input in the affected build/platform until corrected.

## 20. Independent review requirements

Before Phase 4, an independent architecture/security review MUST examine:

- crate and feature dependency direction;
- native API/public-private boundary;
- action basis and geometry correctness;
- user-presence/takeover races;
- cancellation and held-input cleanup;
- MCP protocol and stdout isolation;
- permission/signing/package identity;
- screenshot/text retention and logs;
- elevated/protected-surface refusal; and
- absence of hidden helper, browser, remote, unattended, or network paths.

Findings are severity-classified. P0/P1 findings block release. Supported lower-severity findings are fixed or explicitly accepted with rationale; uncertainty is documented as a decision rather than asserted away.

## 21. Open decisions

The following decisions require prototype evidence and maintainer discussion:

01. The exact native `UserPresenceGuard` indicator, emergency stop, physical-input detector, and local resume UX per OS.
02. Maximum allowed takeover-to-cancellation acknowledgement latency per action/backend.
03. Whether primary-display or normalized visible-desktop is the default scope.
04. macOS pure Rust versus same-process Swift shim and app-bundle versus standalone signed identity.
05. Windows WGC versus Desktop Duplication and supported Windows version floor.
06. Wayland compositor/portal/EIS support matrix and whether some targets remain observe-only.
07. The explicit X11 support/release policy and per-target session-manager/lock-state contract.
08. Whether optional accessibility metadata ships in the first observation release.
09. Exact screenshot dimensions/byte defaults and PNG versus JPEG policy.
10. Whether `starweaver-core` cancellation/identity primitives are reused.
11. The independent MCP client conformance set.
12. Release artifact lane and native publisher-signing readiness.
13. Whether required native APIs can be reached under the workspace unsafe-Rust prohibition or require an explicitly reviewed safe-wrapper/package-boundary revision.

Open decisions do not permit weaker defaults. Until decided and proven, the narrower or observe-only behavior applies.

## 22. Final acceptance criteria

The spec set is implementation-ready when:

- service/tool/MCP/native/security contracts agree on one process and one current active desktop;
- canonical capability names and tool schemas are unambiguous;
- startup grants and default observation-only MCP behavior are fixed;
- `UserPresenceGuard` is a required input-release dependency with a clearly non-release development exception;
- observation basis, post-action image, cancellation, idempotency, and cleanup semantics are testable;
- zero retention and redacted observability are explicit;
- each platform has a bounded spike and package-evidence plan;
- delivery phases allow observation to ship without prematurely enabling input;
- all browser, provider-native, remote, VM, unattended, locked, helper, daemon, privileged, network-MCP, and graphical-product-owned paths remain excluded; and
- maintainers have concrete open decisions to discuss rather than implicit implementation guesses.
