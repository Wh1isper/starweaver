# Computer Use

Starweaver Computer Use observes the current local user's active interactive desktop through an
OS-native, process-local service. On macOS, the process user must own the foreground `/dev/console`
session; a different user, locked session, or inactive session fails closed. The current provisional observe-only boundary is deliberately narrow:

| Platform | Observe current desktop                    | Pointer input                      | Keyboard input                     |
| -------- | ------------------------------------------ | ---------------------------------- | ---------------------------------- |
| macOS    | Available with Screen Recording permission | TBD, unavailable in release builds | TBD, unavailable in release builds |
| Windows  | TBD, unavailable                           | TBD, unavailable                   | TBD, unavailable                   |
| Linux    | TBD, unavailable                           | TBD, unavailable                   | TBD, unavailable                   |

The capability is opt-in. It does not target a PID, window, application, remote host, hidden desktop,
or browser session. CLI and RPC link the library in-process; they never call the MCP binary. The
feature-gated MCP binary is only for non-Starweaver harnesses.

## macOS permission

Observation requires Screen Recording permission for the exact executable identity that performs the
capture. Inspect readiness without capturing pixels:

```bash
starweaver-computer-use-mcp --doctor --json
```

Print onboarding guidance:

```bash
starweaver-computer-use-mcp --request-permissions --json
```

Follow the reported remediation in System Settings. macOS may require restarting the executable after
the permission changes. Status and diagnostic output contain no screenshots or desktop text.

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
Configuration cannot enable pointer or keyboard input in this release.

## RPC in-process use

RPC requires two independent authorization gates:

1. RPC server Computer Use is enabled; this automatically injects the Toolset into every effective RPC profile; and
2. the initiating transport principal has a separate observe capability.

Profiles do not need to list `computer_use` themselves. The automatic materialization does not grant caller authority.

Example `rpc.toml`:

```toml
[computer_use]
enabled = true
desktop_scope = "primary_display"
grant_ttl_ms = 300000
stdio_observe = true
http_observe = false

[profiles.macos_observer]
model_id = "openai-responses:gpt-5"
```

The generic HTTP `run` scope grants no Computer Use authority. `stdio_observe` and `http_observe` are
separate, default-denied principal capabilities. A stdio admission is also bound to its persistent
connection and is revoked when that connection closes. Unary HTTP admissions are bound to the
credential fingerprint and process-start authorization evidence, and expire at the configured TTL.
In V1 this evidence is immutable for the RPC process; standalone generation `0` is valid, while
ordinary runtime profile reloads use separate per-run snapshot generations and do not revoke an
already admitted run.
`grant_ttl_ms` must be between 1 and 900000.

Every admitted run receives a fresh process-local grant. Every tool call checks the exact run grant,
principal fingerprint, authorization generation, admission generation, expiry, and effective
observe-only ceiling. Connection close, expiry, run completion, and host shutdown cooperatively cancel
in-flight observation and revoke future calls. Durable run/context records never serialize or restore
this authority; resume derives a fresh grant from the initiating caller.

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
Only `not_required` and `complete` cleanup outcomes are treated as confirmed; `best_effort` and `failed`
remain errors.

`--allow-pointer` and `--allow-keyboard` cannot widen the compiled release ceiling. They currently
produce a diagnostic and the corresponding tools remain omitted.

Computer Use remains a provisional component, even when it is included by the default macOS
installer, because GitHub Release archives are checksum-covered but are not Apple Developer ID signed
or notarized. The CLI and RPC Toolset remains default-denied. A production-ready macOS capture claim
is blocked until Developer ID signing, notarization, stable TCC identity, and release-byte permission
continuity are proven. If Gatekeeper quarantines the standalone binary, inspect and verify the
downloaded archive first, then
apply any organization-approved per-application exception. Such an exception does not replace code
signing.

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

The service does not persist screenshots, desktop text, typed text, native handles, permission tokens,
or live authority. Lock, user switch, display-topology change, permission loss, session replacement,
or process mismatch fails closed.

## Validation

Focused local gates:

```bash
cargo test -p starweaver-computer-use --features mcp-server
cargo clippy -p starweaver-computer-use --all-targets --features mcp-server -- -D warnings
make computer-use-mcp-check
```

Native observation requires an attended macOS session and Screen Recording permission. Automated CI
builds and validates both macOS release targets without claiming that hosted runners provide a valid
permission grant for live pixel capture.
