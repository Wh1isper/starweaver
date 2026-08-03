# macOS Computer Use Delivery Evidence

Status: implemented macOS observe, pointer, and keyboard support

This note records the implemented macOS subset of the broader contracts in `../computer-use/`. The normative current status is generated from `../capabilities.toml`; future Windows/Linux work remains governed by the Computer Use specs.

## Delivered boundary

- `starweaver-computer-use` owns the typed service, canonical eight-tool catalog/router, state machine, deterministic fake, target selection, and feature-gated stdio MCP server.
- macOS uses a same-process native backend for the current interactive desktop, rejects root and root-owned/loginwindow console sessions, verifies that the process user owns the foreground `/dev/console` session, and fails closed when the session is inactive or locked. Screen Recording gates pixels; Accessibility/post-event permission gates optional focused-application AX snapshots and high-level CGEvent pointer/keyboard synthesis.
- Windows and Linux select an explicit unsupported backend and expose no Computer Use tools.
- `starweaver-agent` provides the opt-in `ComputerUseToolset`, grant-intersected filtered dependencies, method-limited handles, dynamic revocation, exact invocation identity, and immutable geometry-bound image projection.
- CLI and RPC compose the library in-process and never through MCP. Enabling product-level Computer Use automatically materializes the full canonical Toolset into every effective profile. Both maintained products use `InputApprovalPolicy::Never`; RPC requires ordinary caller/run admission, expiry, cancellation, and revocation without transport- or input-specific principal capabilities.
- `starweaver-computer-use-mcp` is built only with `mcp-server`, serves stdio only, and advertises the full canonical family when launched. It is released as separate macOS archives for external harnesses. Stdio never prompts implicitly; the attended top-level `--request-permissions` mode independently invokes Screen Recording, CoreGraphics post-event, and AX trust request paths and reports their immediate authoritative result.

## Security posture

The macOS implementation synthesizes bounded pointer and keyboard input through complete high-level CGEvent sequences. CLI/RPC trusted startup policy allows at most one attended Screen Recording prompt on first open and one Accessibility/post-event onboarding attempt on the first accessibility-enabled observation; post-event and AX trust are requested and re-probed independently, while status and doctor are always passive. MCP stdio disables both implicit prompt paths. Prompt display is never treated as a grant, and input calls fail unless current Accessibility/post-event permission is usable.

The AX collector starts at `AXFocusedApplication`, traverses breadth-first, and enforces node, depth, children-per-node, per-string, total-string, capture-deadline, and native message-time budgets. It bounds native child batches and string conversion before Rust-side materialization. It returns bounded role/name/value/state and optional CGRect-derived model bounds, uses `AXContainsProtectedContent` plus secure-text subroles, inherits protection through descendants, and omits protected values. The service independently rejects malformed protected-value, tree, string-budget, and geometry output. PID, handles, paths, and unrestricted attributes remain excluded. `DesktopSurfaceScope` bounds pixels; the separately requested semantic projection explicitly covers the whole focused application and omits model bounds outside captured geometry. The narrow documented `macos_accessibility` module owns `objc2`/Core Foundation retain-and-cast safety, `macos_input` owns the sole audited unsafe `CGEventKeyboardSetUnicodeString` call, and `macos_session` owns the typed CoreGraphics session-dictionary cast and numeric conversion used by continuous transition sampling; unsafe Rust remains denied outside those reviewed macOS modules. Accessibility and screenshot content are untrusted and non-durable.

Explicit product enablement or standalone MCP launch grants the full canonical family at the product boundary. There is no input-specific `UserPresenceGuard`, emergency stop, `--allow-pointer`/`--allow-keyboard`, signing/notarization, per-input principal, or maintained CLI/RPC per-call approval gate. Native permissions, active same-user unlocked-session checks, current run/lifecycle authorization, observation basis, serialization, idempotency, cancellation cleanup, and revocation remain mandatory.

Desktop screenshots are process-local, non-durable, geometry-bound evidence. The macOS target generation includes the foreground console session UUID, audit identity, available lock marker, and an epoch maintained by a dedicated 10-millisecond `CGSessionCopyCurrentDictionary` sampler that retains lock, console, user, audit, and lock-time history. Pre/post capture fingerprints discard bytes after lock, user/session, Screen Recording permission, or display-topology changes, and synchronous plus periodic fences stop long input actions after the same transitions. The retained worker history detects lock/unlock and switch/return round trips even when no business operation sampled the intermediate state. Before acceptance, the service bounded-decodes backend bytes and verifies detected format, declared MIME, decoded dimensions, pixel limits, allocation limits, and geometry agreement without changing the retained bytes. Native operations run as owned supervisor tasks behind one shared serialized backend gate. Cancellation or timeout allows only a bounded cooperative cleanup grace; direct future abandonment or handler abort synchronously triggers the same poison-on-drop guard. If native work still does not terminate, the service permanently poisons that process-local backend lifecycle before it can be reused, clears capabilities and observations, returns `SessionUnavailable`, and forbids every later backend call or close. Actions reserve idempotent `DeliveryUncertain`/cleanup-failed evidence before native handoff. Only `NotRequired` and `Complete` confirm cleanup; `BestEffort` and `Failed` pause action control or fail shutdown. The backend atomically reserves active input, rejects overlapping direct execute calls, and refuses close while direct input is active. It retains unresolved held keys/buttons for checked close to retry and probes post-event authority before and after release delivery. An execution guard performs synchronous best-effort cleanup and preserves unresolved state when a direct action future is abandoned. RPC admission revocation after dispatch preserves the router's canonical effect receipt instead of replacing it with receipt-free policy denial. The observation ledger is age- and capacity-bounded and stores current layout generation explicitly. Generic media compression, splitting, upload, and understanding transforms do not alter accepted geometry media. The SDK revalidates image capability plus count, per-image/aggregate encoded-byte, and dimension hard limits before every model request, including after model switches. Canonical live history retains a bounded newest-first tail and removes complete stale media prompts plus duplicate private tool payloads while preserving retained bytes exactly. At the durability seam, `AgentCheckpoint::new` and full resumable-context export clone and project that live state: geometry-marked Computer Use screenshot content parts and the runtime-generated screenshot carrier are removed, while structured results and unrelated private metadata remain. Durable raw ToolReturn stream records apply the same exact-key projection. Checkpoint serialization/restoration fixtures prove the screenshot sentinel never enters the durable envelope and that ordinary/private metadata survives. A restored run must capture a fresh observation. RPC authority is not serialized and is revoked on connection close, expiry, replacement admission, run completion, or shutdown; revocation also cooperatively cancels in-flight observation.

GitHub archives are checksum-covered but are not Apple Developer ID signed or notarized. CLI/RPC Toolset composition remains default-off, while launching the standalone MCP server is explicit opt-in. Signing/notarization affects Gatekeeper warnings, stable TCC identity, and permission continuity across updates; it does not remove input tools when the current executable has the required native permissions.

## Contract and composition evidence

- Typed service and state-machine fixtures: `crates/starweaver-computer-use/tests/service_contract.rs`
- Canonical schema fixture parity: `crates/starweaver-computer-use/tests/catalog_contract.rs`
- MCP catalog and capability projection: `crates/starweaver-computer-use/src/mcp_server.rs`
- macOS backend, native input, session-transition, and AX boundary tests: `crates/starweaver-computer-use/src/platform/macos.rs`, `crates/starweaver-computer-use/src/platform/macos_input.rs`, `crates/starweaver-computer-use/src/platform/macos_session.rs`, and `crates/starweaver-computer-use/src/platform/macos_accessibility.rs`
- Toolset/grant/media/revocation tests: `crates/starweaver-agent/src/bundles/computer_use/`
- Durable screenshot projection and restore contracts: `crates/starweaver-context/tests/checkpoint_contracts.rs` and `crates/starweaver-context/tests/context_state.rs`
- CLI configuration/profile/composition tests: `crates/starweaver-cli/src/computer_use.rs`, `crates/starweaver-cli/src/config.rs`, and `crates/starweaver-cli/src/profiles.rs`
- RPC auto-materialization, admission, expiry, revocation, and composition tests: `crates/starweaver-rpc/src/agent_catalog.rs`, `crates/starweaver-rpc/src/computer_use.rs`, and `crates/starweaver-rpc/src/coordinator.rs`
- Release and installer integration: `.github/workflows/ci.yml`, `.github/workflows/release-components.yml`, `.github/workflows/release-computer-use.yml`, `scripts/install.sh`, and `xtask/src/release.rs`

## Validation gates

The delivery is accepted only while these commands pass:

```bash
make fmt-check
make check
make test
make docs-check
make docs-build
make scripts-check
make computer-use-mcp-check
cargo test -p starweaver-computer-use --all-targets --features mcp-server
cargo clippy -p starweaver-computer-use --all-targets --features mcp-server -- -D warnings
```

A native smoke additionally runs `starweaver-computer-use-mcp --doctor --json`; attended onboarding uses `--request-permissions --json`. Live pixel observation requires an unlocked session and Screen Recording permission for the exact executable identity, while accessibility-enabled observation additionally requires Accessibility permission. Hosted CI does not claim either live TCC grant.

Attended local validation on an authorized Apple Silicon macOS host confirmed the complete effective grant and a `ready_control` session at a 1710×1112 logical primary-display geometry. A real MCP `computer_move_pointer` call reached the exact transformed center point `(855, 556)` with `effect_status = executed`, seven posted native events, and no cleanup requirement. In disposable unsaved TextEdit documents, native text entry produced `replace me`; a deliberately reverse-ordered `[A, Meta]` chord selected it as Command+A; and a subsequent Unicode text action produced exactly `Starweaver native input ✓`. Follow-up regression validation delivered an input beginning with a newline and containing tab, carriage-return/newline, checkmark, and astral emoji with all controls and Unicode present; its receipt was `executed`, 16 native events, and `cleanup = not_required`. A maximum-policy 8,192-scalar ASCII action produced exactly 8,192 characters with an `executed` receipt, 820 native events, and no cleanup requirement. Documents were closed without saving. CoreGraphics text delivery isolates leading line-feed, carriage-return, and tab controls as canonical keys, preserves UTF-16 surrogate pairs, and applies bounded adaptive inter-part pacing. This evidence covers live move, text, multiline/control text, maximum-size text, Unicode, and modifier-chord delivery. Click, drag, and scroll remain covered by deterministic translation, geometry, ordering, cancellation, and cleanup tests rather than an automated live GUI mutation.

## Remaining work

- Windows observe/pointer/keyboard backend;
- Linux Wayland/X11 observe/pointer/keyboard backend;
- Apple Developer ID signing/notarization for improved identity continuity and OS-warning behavior;
- Windows/Linux and cross-platform accessibility parity.

These are future capability graduations, not hidden fallbacks in the implemented macOS release.
