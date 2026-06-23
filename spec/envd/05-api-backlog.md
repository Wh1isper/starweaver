# Envd API Backlog

This backlog records envd API work that is intentionally not required for the
current local CLI/headless path. The current service trait is sufficient for:

- local ephemeral CLI runs through `LocalEnvd`
- Starweaver `EnvironmentProvider` adaptation
- stdio/http file, command, process, context, and snapshot RPC round trips
- one active envd attachment in the host runtime

Do not add these APIs just because the protocol could support them. Add them
when a concrete envd implementation, external runtime adapter, or host workflow
needs the contract and can provide tests.

## Backlog

| Area                      | Missing API                                                                                                   | Add when                                                                                                 | Acceptance gate                                                                                                   |
| ------------------------- | ------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| Request metadata          | A typed `EnvdRequestContext` carrying `request_id`, `idempotency_key`, `trace_context`, `actor`, and metadata | remote envd retries, audit logs, policy decisions, or cross-runtime trace propagation need stable fields | Local and RPC conformance tests prove metadata reaches operation/effect records without changing method semantics |
| Mutation preconditions    | `base_state_version` or resource-generation preconditions on mutating file/process methods                    | multiple clients can mutate the same environment concurrently                                            | stale writes return `state_conflict`, matching retries return the original result                                 |
| Lifecycle close/unload    | explicit `environment.close` or `environment.unload`                                                          | an implementation owns expensive or durable state that must be released through a public protocol        | direct service and RPC tests prove close/unload behavior and post-close errors                                    |
| Shell sessions            | `shell_session.create`, `attach`, `input`, `resize`, `close`, and output cursor methods                       | interactive PTY sessions are needed instead of foreground commands or background process snapshots       | PTY transcript/output cursor tests pass for at least one concrete backend                                         |
| Policy preflight          | `policy.describe`, `policy.check`, and approval decision methods                                              | external hosts need to preview policy or complete approval outside a direct tool call                    | denied, allowed, and approval-required paths map cleanly to Starweaver HITL records                               |
| Streaming output          | process/shell output cursors and subscription-capable transport profile                                       | polling snapshots are not enough for UX or remote execution latency                                      | cursor replay is deterministic and does not require stdio/http unary semantics to change                          |
| Health and diagnostics    | typed `envd.health` and `diagnostics.state` RPC methods beyond HTTP `GET /health`                             | orchestration or external clients need protocol-level health over every transport                        | stdio and HTTP expose the same health result shape                                                                |
| Multi-environment routing | composite or multi-mount environment refs in SDK/provider integration                                         | one run needs multiple active envd attachments                                                           | host RPC stops returning `UNSUPPORTED_FEATURE` for multiple refs and provider conformance tests cover routing     |

## Design Rules

- Add fields to request DTOs before adding transport-specific behavior.
- Keep `EnvdRpcClient` and direct `LocalEnvd` behavior equivalent.
- Keep lifecycle durability inside the concrete envd implementation; the
  protocol should expose close/unload only after there is a caller that needs
  it.
- Keep `starweaver-rpc-core` free of envd file, process, and mount DTOs.
