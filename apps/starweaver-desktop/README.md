# Starweaver Desktop

Starweaver Desktop is the Tauri 2 native shell for Starweaver. The current implementation provides
the cross-platform shell foundation, a generated least-authority host bridge, and a consistency-
verified local `starweaver-rpc` supervisor. A normal source-tree application launch selects the adjacent same-build `starweaver-rpc` sidecar,
creates a private public launch envelope, and starts one long-lived local host. Every native package
contains that exact target sidecar as an immutable bootstrap/fallback. Release builds also support
project-signed independent RPC updates that activate on the next Desktop process start, previous or
bundled runtime rollback, and Tauri-signed whole-Desktop installation with native confirmation,
coordinated RPC shutdown, and restart. The source-tree bootstrap inherits trust from the developer
build boundary and uses the digest for immutable staging and time-of-check/time-of-use protection.

The implemented local topology is one long-lived RPC host per execution domain managing multiple
registered workspaces, sessions, and concurrent runs. Supervised launch is domain-only; explicit
workspace registration creates live authority, while durable session provenance never recreates a
grant. The host implements typed runtime config get/validate/update/reload/activate/discard and the
Desktop backend supplies native-confirmation-bound HMAC grants without exposing the key, token,
paths, or idempotency identity to the renderer. Desktop never edits `rpc.toml` directly. The local
product now includes the three-entry workspace start center, multiple registered workspaces,
incremental durable session and run-history pages, prompt/steer/interrupt controls, replayable incremental
public assistant text, backend-routed conversation windows, and a durable Interaction Inbox for
approvals, clarifying questions, and deferred results. SSH is intentionally
outside the Desktop product; a future independent helper may
integrate through the public host boundary without adding SSH authority to the renderer or backend.

## Supported Targets

`targets.toml` is the reviewed source of truth for the initial native matrix:

| Platform            | Rust target                | Planned bundles |
| ------------------- | -------------------------- | --------------- |
| Linux x86_64        | `x86_64-unknown-linux-gnu` | AppImage, deb   |
| macOS Intel         | `x86_64-apple-darwin`      | dmg             |
| macOS Apple Silicon | `aarch64-apple-darwin`     | dmg             |
| Windows x64         | `x86_64-pc-windows-msvc`   | NSIS            |

Linux ARM64 and Windows ARM64 are not advertised until both the Desktop shell and managed RPC
runtime are built and validated for those targets.

For release-package verification, unsigned platform warnings, installation, updates, and recovery,
see [Install and Update Starweaver Desktop](../../docs/desktop-install.md).

## Toolchain

- Rust follows the repository `rust-toolchain.toml` and shared `Cargo.lock`.
- Node follows the repository `.node-version`.
- pnpm is pinned by the root `packageManager` field and invoked through Corepack.
- `pnpm-workspace.yaml` keeps pnpm 11's 24-hour package-age gate, lockfile verification,
  no-trust-downgrade policy, exotic-transitive blocking, and explicit lifecycle-script approval.
  Only `esbuild` is allowed to execute an install script.

Enable Corepack once if your Node installation does not provide a `pnpm` shim:

```bash
corepack enable
```

On Debian or Ubuntu, install the native Tauri build dependencies:

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libappindicator3-dev \
  librsvg2-dev \
  patchelf \
  xdg-utils
```

Windows development uses the MSVC toolchain and WebView2. macOS development requires Xcode command
line tools.

## Development

Run commands from the repository root:

```bash
make rpc
make desktop
```

`make rpc` runs the standalone RPC host over stdio by default; pass `ARGS="http --port 8765"` for an HTTP development host. `make desktop` installs the locked frontend dependencies, builds the current development RPC binary, and launches Tauri with that exact absolute binary selected through a debug-only override. Set `DESKTOP_RPC_BINARY=/absolute/path/to/starweaver-rpc` to test another development build; release binaries ignore this override and retain verified bundled/managed selection.

Run the full validation gate separately when needed:

```bash
make desktop-check
```

Build the current platform without producing an installer:

```bash
make desktop-build
```

The complete frontend gate runs Biome formatting and linting, TypeScript checks, Vitest, and a Vite
production build. The Rust gate runs check, Clippy with warnings denied, and unit tests.

## Native Packaging and Update Keys

Build unsigned current-platform installers with their exact RPC sidecar. Linux packaging disables
`linuxdeploy` stripping, then restores the exact target RPC into the generated AppDir and repacks the
AppImage with a digest-pinned output plugin. Updater builds sign only the final repacked bytes:

```bash
make desktop-package
```

Automatic Desktop and independent RPC updates still require a free Tauri/minisign project key. This
is not Apple Developer ID, notarization, or Windows Authenticode signing. Generate the long-lived key
once on a trusted maintainer machine, outside the repository:

```bash
mkdir -p "$HOME/.config/starweaver-release"
corepack pnpm --filter @starweaver/desktop tauri signer generate \
  --write-keys "$HOME/.config/starweaver-release/updater.key"
```

The command writes the private key to `updater.key` and the public key to `updater.key.pub`. Read the
public value with `cat "$HOME/.config/starweaver-release/updater.key.pub"`. Do not commit either file
or copy the private key into an issue, log, build artifact, or repository variable. Configure GitHub:

- repository variable `STARWEAVER_UPDATE_PUBLIC_KEY`: the complete `updater.key.pub` contents;
- repository secret `TAURI_SIGNING_PRIVATE_KEY`: the complete private-key file contents;
- repository secret `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: the password, when one was chosen.

The public key is embedded into release Desktop binaries and verifies both Tauri updater artifacts
and detached runtime manifests. Development builds without it do not register the native updater
plugin, intentionally report both update channels as unconfigured, and continue using the bundled
sidecar. For a local updater-artifact build, expose the
same values only to the build process and run:

```bash
make desktop-package-updater
```

Key rotation is a release trust migration: existing clients trust the old embedded public key, so do
not replace the configured key without a reviewed transition plan and recovery release. OS publisher
signing remains deliberately unconfigured; user verification and per-application warning handling are
documented in [Install and Update Starweaver Desktop](../../docs/desktop-install.md).

## Branding Assets

`public/app-icon.png` is the canonical transparent Starweaver icon source. The renderer uses the
64-pixel derivative at `public/favicon.png`, while Tauri bundles the generated desktop assets under
`src-tauri/icons/` for Linux, macOS, Windows, and Microsoft Store packaging.

Regenerate the complete desktop icon family from the repository root with the pinned Tauri CLI.
The wrapper also refreshes `public/favicon.png` and removes the unused Android and iOS output trees:

```bash
corepack pnpm --filter @starweaver/desktop icons
```

## Authority Boundary

- `src/bridge/desktop.ts` is the only renderer module allowed to import Tauri APIs.
- The main window can invoke only the reviewed shell commands and the generated host capability:
  status, fixed managed-runtime retry, typed activation subscribe/unsubscribe, backend-owned
  conversation-window opening, manifest-filtered host-operation execute, acknowledgement and
  pending-handle discovery, typed run-event subscribe/unsubscribe, fixed runtime-update
  status/check/install/rollback, and fixed Desktop-update status/check/install.
- A conversation window receives a separate wildcard shell capability. Rust mints its native label
  and fixed application URL, retains `label -> sessionId`, focuses an existing window for the same
  session, and rechecks every host operation and event subscription against that route. ID-based
  interaction reads and mutations carry the owning session through the host contract and RPC verifies
  the durable record before projection or mutation. Workspace discovery is backend-filtered to the
  routed session's live workspace, so sibling workspace metadata never reaches a conversation
  renderer. The renderer cannot provide a label, URL, path, capability set, execution domain, or
  unrelated event scope. Activation and host events arrive over application-owned
  typed IPC channels; the renderer has no general Tauri event-listener permission.
- The renderer cannot invoke initialize, shutdown, replay, host subscribe/unsubscribe, or
  environment attach; provide request IDs, idempotency keys, execution domains, host subscription
  IDs, cursors, or delivery sequences; or submit free-form JSON-RPC.
- `run.start` accepts text input only. The manifest excludes public host resource URI variants from
  both generated bridge languages until a privileged backend grant flow can issue opaque resource
  handles, so renderer-provided file paths and URIs cannot reach the host.
- No renderer filesystem, shell, process, opener, HTTP, storage, provider-credential, or generic updater plugin is installed. Privileged Rust alone owns the fixed Tauri and RPC update clients.
- The renderer receives no secondary-launch arguments or working directory. The process-to-process
  activation protocol also carries only a fixed versioned signal: Linux uses the authenticated
  session D-Bus, macOS uses a current-user peer-checked socket in a private directory, and Windows
  uses a peer-verified local named pipe discovered through a random rendezvous in the user's private
  application-data directory.
- The Rust crate does not link CLI, RPC host, agent, runtime, or storage implementation crates.
- Process-owned state survives renderer reloads; a second launch focuses the primary window and
  advances a monotonic activation generation. Session hydration keeps run submission disabled until
  active-run ownership is authoritative, and same-session refresh epochs reject stale snapshots.
- Conversation history follows new output only while the reader remains near the bottom and offers an
  explicit **Jump to latest** control otherwise. Settings and interaction drawers are modal, trap focus,
  close on Escape, restore the opening control, and make the underlying surface inert.
- `session.get` reads only its storage-bounded newest run window. Older history uses newest-first
  `run.list` keyset pages represented in the renderer only by opaque, generation-bound Desktop page
  tokens. Hidden windows release run subscriptions and suspend status polling until visibility-driven
  durable refresh and reattachment.
- Closing the main window with the `quit` preference enters the same coordinated RPC shutdown barrier
  even when conversation windows remain open; `keep_running` still hides only the main window.

`make desktop-boundaries-check` validates these invariants and checks that the GitHub Actions native
matrix exactly matches `targets.toml`. The native macOS ARM job also launches primary and secondary
processes to smoke-test fixed-signal single-instance routing.

## Current Runtime State

At startup, the privileged backend fully revalidates and prefers a compatible managed
`starweaver-rpc` pointer when present, otherwise it resolves the adjacent bundled sidecar; it never
searches `PATH`. It materializes a deterministic private launch envelope for the canonical local
Starweaver database and the host-local `oauth@codex:gpt-5.6-sol` provider with the reviewed
`openai_responses_high` and `gpt5_350k` presets, keeps provider tokens outside Desktop, keeps native
local shell disabled, and starts the managed host asynchronously. Codex OAuth routing canonicalizes
long typed session affinity into a stable 64-byte-safe identifier; durable session identity remains
separate metadata and is never copied into provider headers verbatim.
The status screen exposes only safe lifecycle state, a bounded safe issue, and a stable diagnostic
category rather than raw process, transport, path, SQL, provider, or credential details. A failed
preparation or launch can be retried through a fixed command without accepting a path or arbitrary RPC
from the renderer.

The implemented backend supervisor accepts only an absolute managed executable with an exact SHA-256
digest and an absolute public launch envelope. Supervised startup preflights an existing canonical
database and refuses one observed to be out of date before storage open. A schema-changing RPC or Desktop update cannot be published until a storage-owned atomic open/create
and coordinated maintenance barrier replaces this path-level guard and removes the remaining
check/open race. Independent runtime manifests therefore require exact storage generation 1. It verifies source identity and permissions, copies
the exact bytes into a private immutable
per-child staging directory, re-verifies the staged identity/digest, clears the child environment,
uses a fixed allowlist, invokes the staged executable directly
without `PATH` or a shell, bounds stdio and stderr, performs the sole IDL-first
`starweaver.host` major-1 handshake with exact revision/schema/storage/launch compatibility, and
retains request correlation, host cursors, subscription sequencing, actor recovery, and coordinated
shutdown in Rust. Every fresh renderer subscription replays its run from durable origin so a new
in-memory transcript projection is complete; internal host-tail recovery resumes from the latest
renderer-applied acknowledgement cursor. Subscription identity includes the owning window label:
reload of the same window/run replaces only its previous tail, different windows may observe the same
run concurrently, and window destruction cancels only that window's tails. Incremental assistant text uses the closed
`transcript_changed` host event and is committed
atomically with its canonical replay record; reasoning, provider-native payloads, arbitrary display
metadata, and provider message IDs are not projected to the renderer. Local source-tree validation
has exercised live OAuth prompt completion, balanced incremental transcript replay, host restart,
and recovered input and output from the same canonical database. The generated client
acknowledges each event only after its synchronous or asynchronous handler succeeds. The generated
client separately exposes replay catch-up and terminal completion barriers: historical views close
after catch-up so bounded hydration advances through every returned run, while a backend-issued
scope token completes an active subscription when its Rust tail ends. Rust atomically persists that
opaque cursor and bounded event-ID deduplication state
without exposing either host cursor or subscription authority to the renderer. At cursor capacity,
inactive historical views are evicted while active subscriptions and pending acknowledgements remain
protected; a later visit to an evicted view safely replays from origin. Duplicate unsubscribe calls
share one terminal barrier. Terminal close and unsubscribe responses may cross safely through a
bounded generation-scoped recently closed ledger; only a retained duplicate close is ignored, while
unknown or stale notifications still fail closed. Host pagination cursors are represented only by
bounded opaque Desktop page tokens tied to the admitted operation, domain, and child generation. The
history UI fetches one session page initially and retains only the opaque next-page token for an
explicit “Load older conversations” action. Long conversations similarly expose “Load earlier runs”
without letting host cursors cross renderer IPC; sessions whose durable workspace is absent from the
live registry are grouped under history-only rather than being hidden. Each mutation uses
a generated logical operation instance whose renderer-safe operation body and backend-created
idempotency key binding are persisted before first send and reused across response loss, child
recovery, and Desktop restart; identical payloads with distinct operation IDs remain distinct
mutations. This includes `approval.decide`, `clarification.resolve`, `deferred.complete`,
`deferred.fail`, and `run.resume`. The renderer discovers pending interactions from durable paginated
host queries as well as live events, lazily loads bounded review details, and fails closed when an
approval or deferred payload exceeds the safe complete-detail projection. Interaction resolution and
run continuation remain separate durable operations: after a response-loss or crash gap, the Inbox
shows an explicit resume action for the still-waiting run instead of assuming continuation occurred.
An unresolved execution error carries the original invocation, and a fixed generated
command lists pending typed invocations after restart without exposing the binding or other
supervisor-owned fields. The product hook reconciles workspace registration, session creation, run
start, steering, and interruption from those exact invocations during renderer startup and exposes a
single explicit recovery retry rather than allocating a new operation. If only the acknowledgement
response is lost, a distinct generated error retains the known result and acknowledgement token and
retries acknowledgement rather than execution.
The Settings drawer reads catalog/profile readiness from the safe host projection, changes the default
profile only for new runs, and edits only typed runtime profile/provider fields. It validates before
update, recovers the exact `model.select` or `config.*` invocation after an uncertain outcome, and
keeps active runs pinned to their admitted snapshots. Provider credential and network readiness stay
host-local and are checked when a run starts. Source reload is previewed and candidate-bound. Restart-required runtime-config activation remains a
separate host-owned transaction and is never conflated with managed RPC binary selection. `run.status` separately projects
`controllableByCurrentHost` from the current coordinator's process-local active registry; Desktop
keeps foreign durable active runs readable but disables steer and interrupt instead of treating an
active status as proof of ownership.

Theme, density, and close behavior are separate Desktop application preferences. Rust owns their
fixed private versioned file under application-local data, canonical decimal revision fence,
idempotent mutation identity, same-directory atomic replacement, and last-known-good reload. The
renderer receives only closed typed get/update/reload commands and never a path or generic storage
API. Closing with `keep_running` hides the window without stopping the supervised host; a secondary
launch restores it. The explicit `quit` preference follows coordinated runtime shutdown.

## Local Release Visual Checklist

Before accepting a Desktop visual change, review the ready conversation, start center, Settings, and
Interaction Inbox in light and dark themes at a normal window and at `760 × 560`; also inspect the
`680 × 520` lower-bound layout where the host platform permits it. Verify keyboard-only open/close and
focus restoration, Escape behavior, visible focus, reduced motion, long labels, loading/error/recovery
copy, the unread-output jump control, and operation-specific progress. Platform review must include
macOS plus the native Linux and Windows CI build gates; absence of those local platforms must be
recorded rather than inferred from a browser screenshot.

The generated client acknowledges a validated successful result or conclusive non-retryable rejection
before Rust atomically compacts the record into a retired binding. A count- and byte-bounded recent
retired horizon preserves response-loss idempotency without letting successful operations fill the
ledger forever; pending outcomes are never pruned and fail closed at their separate bounds. Forced
termination uses an owned Unix
process group or Windows Job Object and waits for the complete process tree before replacement, while
a bounded cross-generation crash budget fails closed on a restart loop. It never reads CLI-private configuration or emits `rpc.toml`, and native
local shell remains disabled.
