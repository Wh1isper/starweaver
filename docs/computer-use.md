# Computer Use

Starweaver Computer Use observes and operates the current local user's active interactive desktop
through an OS-native, process-local service. On macOS, the process user must own the foreground
`/dev/console` session; a different user, locked session, or inactive session fails closed.

| Platform | Observe current desktop                    | Optional accessibility snapshot         | Pointer input                                | Keyboard input                               |
| -------- | ------------------------------------------ | --------------------------------------- | -------------------------------------------- | -------------------------------------------- |
| macOS    | Available with Screen Recording permission | Available with Accessibility permission | Available with Accessibility/post permission | Available with Accessibility/post permission |
| Windows  | TBD, unavailable                           | TBD, unavailable                        | TBD, unavailable                             | TBD, unavailable                             |
| Linux    | TBD, unavailable                           | TBD, unavailable                        | TBD, unavailable                             | TBD, unavailable                             |

The capability is opt-in. It does not target a PID, window, application, remote host, hidden desktop,
or browser session. CLI and RPC link the library in-process; they never call the MCP binary. The
feature-gated MCP binary is only for non-Starweaver harnesses.

## macOS permissions

Pixel observation requires Screen Recording permission. Optional accessibility snapshots and native
pointer/keyboard event posting require Accessibility/post-event permission for the exact executable
identity that performs the operation. CLI, RPC, and the standalone MCP binary are distinct macOS TCC
identities; granting one does not grant the others. Inspect readiness without capturing pixels or
presenting permission UI:

```bash
starweaver-computer-use-mcp --doctor --json
```

Explicitly request the required Screen Recording and Accessibility/post-event permissions for the MCP executable:

```bash
starweaver-computer-use-mcp --request-permissions --json
```

This attended command calls the native Screen Recording, post-event, and Accessibility request APIs independently. Its immediate preflight/trust result is authoritative: displaying a prompt or opening System Settings is not a successful grant. Follow any reported remediation, then retry; macOS may require restarting the exact
executable after Screen Recording changes. `--doctor`, `computer_status`, MCP initialization, and
`tools/list` remain diagnostic-only and never present permission UI.

When Computer Use is enabled in CLI or RPC, trusted product startup policy allows a one-time attended Screen Recording request on the first desktop open and one attended Accessibility/post-event onboarding attempt on the first `computer_observe` with `include_accessibility = true`. Post-event and AX trust are requested and re-probed independently. The model argument only requests already host-authorized bounded metadata; it cannot widen policy or enable semantic actions.

## CLI in-process use

Enable the process ceiling in the CLI config:

```toml
[computer_use]
enabled = true
desktop_scope = "visible_desktop"
```

`desktop_scope` accepts `primary_display` or `visible_desktop`. `primary_display` means the operating system's primary display; it does not imply focus or pointer tracking. `visible_desktop` captures all displays in the current visible desktop layout.

Enabling this section automatically injects the `computer_use` Toolset into every resolved CLI profile. Profiles do not need to repeat it in their `toolsets`; an existing explicit selection remains accepted and is not duplicated. A profile may still select the desired model and instructions:

```yaml
name: macos_observer
instructions:
  - Observe the current desktop only when needed and describe what is visible.
model:
  model_id: openai:gpt-5
  settings_preset: openai_responses_medium
```

Run with that profile:

```bash
starweaver cli --profile examples/profiles/computer-use-macos.yaml \
  -p "Describe the current desktop"
```

The CLI keeps one process-local coordinator and its native service handle for the process lifetime.
One-shot commands and every TUI exit path use a bounded coordinated shutdown; a native cleanup failure
is returned as a command failure, or reported on stderr when unwinding leaves no result channel.
Enabling Computer Use is the user's product-level opt-in and makes the full canonical observe,
pointer, and keyboard family available when the macOS permissions above are granted. Maintained CLI
direct mode uses `InputApprovalPolicy::Never`: input tools do not add a per-call
`approval_required` pause. The native permissions and ordinary run cancellation remain authoritative.

## RPC in-process use

RPC requires Computer Use to be enabled in server configuration and the initiating caller/run to pass
the ordinary RPC transport authorization and run-admission checks. Enabling the feature automatically
injects the Toolset into every effective RPC profile. Profiles do not need to list `computer_use`
themselves, and there are no separate `stdio_observe`, `http_observe`, pointer, or keyboard principal
capabilities.

Example `rpc.toml`:

```toml
[computer_use]
enabled = true
desktop_scope = "primary_display"
grant_ttl_ms = 300000

[profiles.macos_observer]
model_id = "openai-responses:gpt-5"
```

A stdio admission is bound to its persistent connection and is revoked when that connection closes.
Unary HTTP admissions are bound to the credential fingerprint and process-start authorization
evidence, and expire at the configured TTL. In V1 this evidence is immutable for the RPC process;
standalone generation `0` is valid, while ordinary runtime profile reloads use separate per-run
snapshot generations and do not revoke an already admitted run. `grant_ttl_ms` must be between 1 and
900000\.

Every admitted run receives fresh process-local authorization for the full canonical Computer Use
family. Every tool call checks the exact run, caller fingerprint, authorization/admission generation,
expiry, and enabled configuration; every effect checks again at the serialized service fence.
Maintained RPC direct mode uses `InputApprovalPolicy::Never`, so there is no additional per-call HITL
approval for pointer or keyboard tools. Connection close, expiry, run completion, cancellation, and host shutdown cooperatively cancel in-flight work and revoke future calls. Once a call has crossed the dispatch boundary, cancellation preserves the router's canonical executed, partial, uncertain, or not-executed receipt rather than replacing it with a receipt-free admission error. Durable run/context records never serialize or restore this authority; resume performs normal fresh caller/run admission.

RPC always observes the RPC host's local active desktop, never the RPC client's desktop.

## MCP binary for external harnesses

The default macOS release install includes this external-harness binary together with the CLI:

```bash
curl -fsSL https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh | sh
```

Use `STARWEAVER_EXCLUDE_COMPONENTS=computer-use` on the installer process only when the separate MCP
binary should be omitted. Installing it does not enable the default-denied in-process Toolset in CLI
or RPC.

Or build it from source:

```bash
cargo build --release --locked \
  -p starweaver-computer-use \
  --features mcp-server \
  --bin starweaver-computer-use-mcp
```

Inspect immutable build identity:

```bash
starweaver-computer-use-mcp --version
```

Serve one local MCP client over stdio:

```bash
starweaver-computer-use-mcp --stdio
```

Standard output is reserved for MCP framing; diagnostics use standard error. The server advertises the
same canonical schemas and uses the same router as the in-process Toolset. Stdin EOF, terminal
transport failure, Unix `SIGINT`/`SIGTERM`, and MCP close converge on one checked cleanup path. One
20-second absolute deadline covers native cleanup and handler/transport completion; an exceeded
budget or unconfirmed mandatory cleanup produces a non-zero exit. If a native
capture does not stop within its bounded cancellation grace, or if its handler is directly aborted or
dropped, the process permanently disables that backend lifecycle before it can be reused, performs no
concurrent close or retry, reports unconfirmed cleanup, and exits rather than hanging indefinitely.
Only `not_required` and `complete` cleanup outcomes are treated as confirmed; `best_effort` and `failed` remain errors. The macOS backend atomically reserves native input so even direct backend callers cannot overlap two actions or close over an active action. It retains unconfirmed held-key/button state for checked close to retry, probes post-event permission before and after release delivery, and performs a synchronous best-effort release before preserving still-unconfirmed state when a direct future is abandoned.

Launching `starweaver-computer-use-mcp --stdio` is the user's explicit opt-in and advertises the full
canonical observe, pointer, and keyboard tool family on the supported macOS backend. There are no
`--allow-pointer` or `--allow-keyboard` release gates. Tool calls still require the same active, unlocked, same-user session checks and native permissions as the in-process paths. A dedicated macOS worker samples `CGSessionCopyCurrentDictionary` every 10 milliseconds and retains lock, console, user, audit, and lock-time history in the target-generation epoch. The backend also samples synchronously at each capture/input fence and during long actions, so a lock/unlock or switch/return round trip invalidates old observation coordinates even when no business operation observed the intermediate state.

GitHub Release archives are checksum-covered but are not Apple Developer ID signed or notarized.
Signing and notarization can improve TCC identity continuity and reduce Gatekeeper/OS warnings, but
they do not determine whether pointer or keyboard tools are available. If Gatekeeper quarantines the
standalone binary, inspect and verify the downloaded archive first, then apply any
organization-approved per-application exception. Such an exception does not replace code signing.

## Image and authority invariants

Computer Use screenshots are geometry-bound evidence. Starweaver keeps the exact captured bytes,
marks them as immutable, and excludes them from generic compression, splitting, upload, or
media-understanding transforms that would invalidate coordinates. Before each model request—including
after an active-model switch—the SDK requires image capability and enforces the model's image-count,
per-image/aggregate base64-byte, and dimension hard limits. If the newest observation cannot be
submitted unchanged, the request fails with an explicit Computer Use safety error.

To keep live model history bounded, Starweaver retains one newest-first tail of admitted
geometry-bound observations. It removes complete stale media prompts and their private tool-return
media payloads; it does not resize, recompress, or otherwise mutate retained screenshot bytes.

Screenshots are process-local and are deliberately removed at durable projection boundaries. Runtime
checkpoints, resumable context records, and durable raw stream records omit the geometry-bound data URL
and the generated screenshot carrier while retaining the bounded structured tool result, marker,
prompt, and unrelated private metadata. Projection clones the live state, so the current run's model
media preparation and history filtering continue to use the exact bytes. Restoring a checkpoint never
restores a screenshot or observation basis; the next Computer Use step must obtain a fresh observation.

Optional macOS accessibility output is a breadth-first snapshot of the focused application's AX tree.
Node count, depth, children per node, per-string bytes, total string bytes, capture deadline, and
per-message timeout are bounded by immutable host policy. Large child arrays are fetched only up to the
configured child limit, and native strings are converted into pre-bounded buffers. Output contains only
bounded role, name, value summary, state, and optional model-space bounds. Secure text and
`AXContainsProtectedContent` subtrees omit values; protection is inherited by descendants, and the
service rejects any protected node carrying a value. PIDs, native handles, application paths, and
unrestricted attributes are never exposed. Accessibility strings and pixels are untrusted,
prompt-injection-capable data, not instructions or authority. Native FFI is confined to three reviewed
macOS modules: `macos_accessibility` owns the `objc2`/Core Foundation retain-and-cast boundary, `macos_input` owns the sole audited unsafe `CGEventKeyboardSetUnicodeString` call, and `macos_session` owns the typed CoreGraphics session-dictionary cast and numeric conversion used by the continuous transition monitor.

Product-level Computer Use opt-in authorizes both pixel observation and the ability to request this
bounded Accessibility snapshot; `include_accessibility` remains an explicit per-call request and native
Accessibility permission remains independent. The configured desktop scope limits captured pixels.
Accessibility semantics intentionally describe the whole currently focused application rather than
pretending that an AX tree can be clipped to one display; bounds outside the captured model space are
omitted. Choose pixel-only calls (`include_accessibility: false`) when that broader semantic scope is not
wanted.

The service does not persist screenshots, accessibility content, desktop text, typed text, native
handles, permission tokens, or live authority. Lock, user switch, display-topology change, permission
loss, session replacement, or process mismatch fails closed.

## Validation

Focused local gates:

```bash
cargo test -p starweaver-computer-use --features mcp-server
cargo clippy -p starweaver-computer-use --all-targets --features mcp-server -- -D warnings
make computer-use-mcp-check
```

Native pixel observation requires an attended macOS session and Screen Recording permission; a live
accessibility snapshot additionally requires Accessibility permission. Automated CI builds and
validates both macOS release targets without claiming that hosted runners provide valid TCC grants for
live pixel or accessibility capture.
