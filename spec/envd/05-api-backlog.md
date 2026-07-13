# Envd API Backlog

This non-normative backlog records envd API work that is intentionally not required for the current local CLI/headless path. Current implementation status is generated from `../capabilities.toml` into [`../capability-status.md`](../capability-status.md); this backlog may describe only future additions and must not override that status view. The current service trait is sufficient for:

- local ephemeral CLI runs through `LocalEnvd`
- Starweaver `EnvironmentProvider` adaptation
- stdio/http file, command, process, context, and snapshot RPC round trips
- run-scoped envd attachments materialized by the Starweaver host attachment
  manager and SDK composite provider

Do not add these APIs just because the protocol could support them. Add them
when a concrete envd implementation, external runtime adapter, or host workflow
needs the contract and can provide tests.

## Backlog

| Area                   | Missing API                                                                                                   | Add when                                                                                                 | Acceptance gate                                                                                                   |
| ---------------------- | ------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| Request metadata       | A typed `EnvdRequestContext` carrying `request_id`, `idempotency_key`, `trace_context`, `actor`, and metadata | remote envd retries, audit logs, policy decisions, or cross-runtime trace propagation need stable fields | Local and RPC conformance tests prove metadata reaches operation/effect records without changing method semantics |
| Mutation preconditions | `base_state_version` or resource-generation preconditions on mutating file/process methods                    | multiple clients can mutate the same environment concurrently                                            | stale writes return `state_conflict`, matching retries return the original result                                 |
| Lifecycle close/unload | explicit `environment.close` or `environment.unload`                                                          | an implementation owns expensive or durable state that must be released through a public protocol        | direct service and RPC tests prove close/unload behavior and post-close errors                                    |
| Policy preflight       | `policy.describe`, `policy.check`, and approval decision methods                                              | external hosts need to preview policy or complete approval outside a direct tool call                    | denied, allowed, and approval-required paths map cleanly to Starweaver HITL records                               |
| Streaming output       | process/shell output cursors and subscription-capable transport profile                                       | polling snapshots are not enough for UX or remote execution latency                                      | cursor replay is deterministic and does not require stdio/http unary semantics to change                          |
| Health and diagnostics | typed `envd.health` and `diagnostics.state` RPC methods beyond HTTP `GET /health`                             | orchestration or external clients need protocol-level health over every transport                        | stdio and HTTP expose the same health result shape                                                                |

## Design Rules

- Add fields to request DTOs before adding transport-specific behavior.
- Keep `EnvdRpcClient` and direct `LocalEnvd` behavior equivalent.
- Keep lifecycle durability inside the concrete envd implementation; the
  protocol should expose close/unload only after there is a caller that needs
  it.
- Keep `starweaver-rpc-core` free of envd file, process, and mount DTOs.

Active-run host attachment changes are implemented by the RPC-owned attachment manager and runtime environment-handle refresh path; they are therefore intentionally absent from this future API backlog.
