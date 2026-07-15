# Session Search Implementation Evidence

Date: 2026-07-14

Status: Phase 1 implemented; durable local index and external production index remain follow-ons

Normative source: [`../ops/07-session-search.md`](../ops/07-session-search.md)

## Boundary evidence

| Responsibility                                                                                                 | Owner                 | Evidence                                                            |
| -------------------------------------------------------------------------------------------------------------- | --------------------- | ------------------------------------------------------------------- |
| Provider, scope, query/filter, result/location, coverage, capabilities, errors, opaque cursor, optional writer | `starweaver-session`  | `crates/starweaver-session/src/search.rs`                           |
| Canonical read-through records and bounded validated display mirrors                                           | `starweaver-storage`  | `crates/starweaver-storage/src/session_search.rs`                   |
| Independent local CLI composition and rendering                                                                | `starweaver-cli`      | `crates/starweaver-cli/src/args.rs`, `local_store.rs`, `service.rs` |
| Typed host DTO and conditional feature identity                                                                | `starweaver-rpc-core` | `crates/starweaver-rpc-core/src/lib.rs`                             |
| Independent RPC configuration, provider composition, read authorization, handler                               | `starweaver-rpc`      | `crates/starweaver-rpc/src/config.rs`, `auth.rs`, `service.rs`      |

The dependency gate continues to prohibit CLI/RPC product dependencies in either direction. Both products depend only on the lower shared contracts/storage implementation.

## Contract evidence

- Cursor tests authenticate the payload and reject wrong query/scope/provider/generation bindings.
- Writer fake tests enforce monotonic revisions so delayed upserts cannot resurrect tombstoned sessions.
- Local provider tests cover literal option-like/regex-like queries, canonical projection exclusion, public display policy, composite archive/source provenance, missing mirrors, byte/file bounds, and symlink escape.
- CLI tests cover human/JSON output and opaque pagination without changing session selection.
- RPC-core tests cover casing, DTO conversion, scope-authority rejection, and optional feature negotiation.
- RPC tests cover installed/disabled behavior, typed results, and read-scope authorization mapping.

## Deliberate follow-ons

- Phase 2 SQLite FTS/materialized index, transactional search mutation table/outbox, generation rebuild, and reconciliation.
- Phase 3 external production adapter and object-storage ingestion. The common writer seam exists, but no external dependency or backend is claimed.
- Authoritative display offload manifests. Current CLI mirrors remain best effort and therefore report partial coverage when absent or unsafe.
