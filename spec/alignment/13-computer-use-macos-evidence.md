# macOS Computer Use Delivery Evidence

Status: implemented, provisional, observe-only

This note records the implemented subset of the broader contracts in `../computer-use/`. The normative current status is generated from `../capabilities.toml`; future platform and input work remains governed by the Computer Use specs.

## Delivered boundary

- `starweaver-computer-use` owns the typed service, canonical eight-tool catalog/router, state machine, deterministic fake, target selection, and feature-gated stdio MCP server.
- macOS uses a same-process native backend for the current interactive desktop, rejects root and root-owned/loginwindow console sessions, verifies that the process user owns the foreground `/dev/console` session, and fails closed when the session is inactive or locked. The provisional capability ceiling is observation; pointer and keyboard tools remain unavailable.
- Windows and Linux select an explicit unsupported backend and expose no Computer Use tools.
- `starweaver-agent` provides the opt-in `ComputerUseToolset`, grant-intersected filtered dependencies, method-limited handles, dynamic revocation, exact invocation identity, and immutable geometry-bound image projection.
- CLI and RPC compose the library in-process and never through MCP. Enabling product-level Computer Use automatically materializes the Toolset into every effective profile in that product. CLI then applies its observe-only process ceiling; RPC additionally requires a fresh principal-bound, expiring, revocable run admission, and generic `run` authority grants nothing.
- `starweaver-computer-use-mcp` is built only with `mcp-server`, serves stdio only, defaults to observe-only, and is released as separate macOS archives for external harnesses.

## Security posture

The released macOS implementation cannot synthesize pointer or keyboard input. Launch flags and product configuration are intersected with the compiled backend ceiling and cannot widen it. Production input remains blocked until a same-process `UserPresenceGuard`, emergency stop, signing evidence, and the input-specific release gates are satisfied. Observation itself remains provisional rather than production-ready while capture executables lack stable Developer ID signing/notarization and TCC continuity evidence.

Desktop screenshots are process-local, non-durable, geometry-bound evidence. Pre/post capture fingerprints discard bytes after lock, user/session, Screen Recording permission, or display-topology changes. Before acceptance, the service bounded-decodes backend bytes and verifies detected format, declared MIME, decoded dimensions, pixel limits, allocation limits, and geometry agreement without changing the retained bytes. Native operations run as owned supervisor tasks behind one shared serialized backend gate. Cancellation or timeout allows only a bounded cooperative cleanup grace; direct future abandonment or handler abort synchronously triggers the same poison-on-drop guard. If native work still does not terminate, the service permanently poisons that process-local backend lifecycle before it can be reused, clears capabilities and observations, returns `SessionUnavailable`, and forbids every later backend call or close. Actions reserve idempotent `DeliveryUncertain`/cleanup-failed evidence before native handoff. Only `NotRequired` and `Complete` confirm cleanup; `BestEffort` and `Failed` pause action control or fail shutdown. The observation ledger is age- and capacity-bounded and stores current layout generation explicitly. Generic media compression, splitting, upload, and understanding transforms do not alter accepted geometry media. The SDK revalidates image capability plus count, per-image/aggregate encoded-byte, and dimension hard limits before every model request, including after model switches. Canonical live history retains a bounded newest-first tail and removes complete stale media prompts plus duplicate private tool payloads while preserving retained bytes exactly. At the durability seam, `AgentCheckpoint::new` and full resumable-context export clone and project that live state: geometry-marked Computer Use screenshot content parts and the runtime-generated screenshot carrier are removed, while structured results and unrelated private metadata remain. Durable raw ToolReturn stream records apply the same exact-key projection. Checkpoint serialization/restoration fixtures prove the screenshot sentinel never enters the durable envelope and that ordinary/private metadata survives. A restored run must capture a fresh observation. RPC authority is not serialized and is revoked on connection close, expiry, replacement admission, run completion, or shutdown; revocation also cooperatively cancels in-flight observation.

GitHub archives are checksum-covered but are not Apple Developer ID signed or notarized. Default installation does not promote this provisional component: CLI/RPC Toolset authority remains default-denied, diagnostics identify the unsigned provisional ceiling, and Screen Recording permission/live capture certification remain attended, executable-identity-specific validation. A production-ready macOS observation claim is release-blocked until stable identity, notarization, and permission continuity are proven for the exact CLI, RPC, and MCP bytes.

## Contract and composition evidence

- Typed service and state-machine fixtures: `crates/starweaver-computer-use/tests/service_contract.rs`
- Canonical schema fixture parity: `crates/starweaver-computer-use/tests/catalog_contract.rs`
- MCP catalog and capability projection: `crates/starweaver-computer-use/src/mcp_server.rs`
- macOS backend tests: `crates/starweaver-computer-use/src/platform/macos.rs`
- Toolset/grant/media/revocation tests: `crates/starweaver-agent/src/bundles/computer_use/`
- Durable screenshot projection and restore contracts: `crates/starweaver-context/tests/checkpoint_contracts.rs` and `crates/starweaver-context/tests/context_state.rs`
- CLI configuration/profile/composition tests: `crates/starweaver-cli/src/computer_use.rs`, `crates/starweaver-cli/src/config.rs`, and `crates/starweaver-cli/src/profiles.rs`
- RPC auto-materialization, admission, expiry, revocation, and composition tests: `crates/starweaver-rpc/src/agent_catalog.rs`, `crates/starweaver-rpc/src/computer_use.rs`, and `crates/starweaver-rpc/src/coordinator.rs`
- Release and installer integration: `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `scripts/install.sh`, and `xtask/src/release.rs`

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

A native smoke additionally runs `starweaver-computer-use-mcp --doctor --json`. Live `computer_observe` requires an attended unlocked macOS session and Screen Recording permission for the exact executable identity; hosted CI does not claim that permission evidence.

## Remaining work

- production pointer and keyboard authority on every platform;
- Windows observation backend;
- Linux Wayland/X11 observation backend;
- production native user-presence and emergency-stop controls;
- Apple Developer ID signing/notarization and release-byte live permission certification;
- optional accessibility metadata.

These are future capability graduations, not hidden fallbacks in the provisional observe-only release.
