# Starweaver Desktop

Starweaver Desktop is the Tauri 2 native shell for Starweaver. The current implementation provides
the cross-platform shell foundation plus a generated least-authority host bridge and a verified
local `starweaver-rpc` supervisor. Runtime selection and update activation are not yet wired, so a
normal application launch remains `unconfigured` until a trusted backend owner supplies an exact
managed runtime and public launch envelope.

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
make desktop-sync
make desktop-check
corepack pnpm desktop:dev
```

Build the current platform without producing an installer:

```bash
make desktop-build
```

The complete frontend gate runs Biome formatting and linting, TypeScript checks, Vitest, and a Vite
production build. The Rust gate runs check, Clippy with warnings denied, and unit tests.

## Authority Boundary

- `src/bridge/desktop.ts` is the only renderer module allowed to import Tauri APIs.
- The main window can invoke only the reviewed shell commands and the generated host capability:
  status, typed activation subscribe/unsubscribe, manifest-filtered host-operation execute,
  acknowledgement and pending-handle discovery, and typed run-event subscribe/unsubscribe.
  Activation and host events arrive over application-owned
  typed IPC channels; the renderer has no general Tauri event-listener permission.
- The renderer cannot invoke initialize, shutdown, replay, host subscribe/unsubscribe, or
  environment attach; provide request IDs, idempotency keys, execution domains, host subscription
  IDs, cursors, or delivery sequences; or submit free-form JSON-RPC.
- `run.start` accepts text input only. The manifest excludes public host resource URI variants from
  both generated bridge languages until a privileged backend grant flow can issue opaque resource
  handles, so renderer-provided file paths and URIs cannot reach the host.
- No filesystem, shell, process, opener, HTTP, storage, OAuth, or updater plugin is installed.
- The renderer receives no secondary-launch arguments or working directory. The process-to-process
  activation protocol also carries only a fixed versioned signal: Linux uses the authenticated
  session D-Bus, macOS uses a current-user peer-checked socket in a private directory, and Windows
  uses a peer-verified local named pipe discovered through a random rendezvous in the user's private
  application-data directory.
- The Rust crate does not link CLI, RPC host, agent, runtime, or storage implementation crates.
- Process-owned state survives renderer reloads; a second launch focuses the primary window and
  advances a monotonic activation generation.

`make desktop-boundaries-check` validates these invariants and checks that the GitHub Actions native
matrix exactly matches `targets.toml`. The native macOS ARM job also launches primary and secondary
processes to smoke-test fixed-signal single-instance routing.

## Current Runtime State

The status screen reports the managed runtime as `unconfigured` because runtime staging and update
selection are not yet connected to application startup. The implemented backend supervisor accepts
only an absolute managed executable with an exact SHA-256 digest and an absolute public launch
envelope. It verifies source identity and permissions, copies the exact bytes into a private immutable
per-child staging directory, re-verifies the staged identity/digest, clears the child environment,
uses a fixed allowlist, invokes the staged executable directly
without `PATH` or a shell, bounds stdio and stderr, performs the sole IDL-first
`starweaver.host` major-1 handshake with exact revision/schema/storage/launch compatibility, and
retains request correlation, host cursors, subscription sequencing, actor recovery, and coordinated
shutdown in Rust. Durable replay is streamed before live delivery from the last renderer-applied
cursor. The generated client acknowledges each event only after its synchronous or asynchronous
handler succeeds; Rust atomically persists that opaque cursor and bounded event-ID deduplication state
without exposing either host cursor or subscription authority to the renderer. At cursor capacity,
inactive historical views are evicted while active subscriptions and pending acknowledgements remain
protected; a later visit to an evicted view safely replays from origin. Duplicate unsubscribe calls
share one terminal barrier. Terminal close and unsubscribe responses may cross safely through a
bounded generation-scoped recently closed ledger; only a retained duplicate close is ignored, while
unknown or stale notifications still fail closed. Host pagination cursors are represented only by
bounded opaque Desktop page tokens tied to the admitted operation, domain, and child generation. Each mutation uses
a generated logical operation instance whose renderer-safe operation body and backend-created
idempotency key binding are persisted before first send and reused across response loss, child
recovery, and Desktop restart; identical payloads with distinct operation IDs remain distinct
mutations. An unresolved execution error carries the original invocation, and a fixed generated
command lists pending typed invocations after restart without exposing the binding or other
supervisor-owned fields. If only the acknowledgement response is lost, a distinct generated error
retains the known result and acknowledgement token and retries acknowledgement rather than execution.
The generated client acknowledges a validated successful result or conclusive non-retryable rejection
before Rust atomically compacts the record into a retired binding. A count- and byte-bounded recent
retired horizon preserves response-loss idempotency without letting successful operations fill the
ledger forever; pending outcomes are never pruned and fail closed at their separate bounds. Forced
termination uses an owned Unix
process group or Windows Job Object and waits for the complete process tree before replacement, while
a bounded cross-generation crash budget fails closed on a restart loop. It never reads CLI-private configuration or emits `rpc.toml`, and native
local shell remains disabled.
