# Starweaver Desktop

Starweaver Desktop is the Tauri 2 native shell for Starweaver. The current implementation is a
cross-platform foundation: it establishes the privileged Rust boundary, typed renderer bridge,
process-owned application state, single-instance activation, target registry, and native build
matrix. It intentionally does not launch `starweaver-rpc` until the public launch-envelope and
compatibility contracts in `spec/desktop/` are implemented.

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
- The main window can invoke only the generated `get_desktop_status`,
  `subscribe_desktop_activation`, and token-scoped `unsubscribe_desktop_activation` commands.
  Activation arrives over a typed application-owned IPC channel; the renderer has no general Tauri
  event-listener permission.
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

The status screen reports the managed runtime as `not_configured`. Production code must not search
`PATH`, read private CLI configuration, generate `rpc.toml`, or launch an unverified host as a
temporary fallback. RPC supervision begins only through the public versioned launch envelope and
compatibility handshake specified under `spec/desktop/`.
