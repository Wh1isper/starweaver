# Session Search

Starweaver can discover persisted sessions through the optional, read-only `SessionSearchProvider` contract. Search is separate from `SessionStore`: a hit is a discovery projection, not restore state, proof of resumability, or mutation authority.

## CLI

The local CLI installs the bounded local provider for its selected SQLite store:

```bash
sw session search "oauth refresh"
sw session search "tool failed" --workspace . --status active
sw session search "summary" --source display --limit 20
sw session search --profile coding --output json
```

The query is a case-insensitive literal. It is never interpreted as a regular expression, shell expression, or command option. `--source` is repeatable and accepts:

- `session_metadata`
- `run_input`
- `run_output_preview`
- `display_message` (or `display`)

`--granularity` accepts `session`, `run`, or `occurrence`. Use the opaque cursor returned by one page without inspecting or changing it:

```bash
sw session search "refresh" --limit 20 --after 'ssc1.…' --output json
```

Human output includes the composite session/run identity, title, update time, source, snippet, coverage warnings, and the next cursor. JSON output has the stable shared shape:

```json
{
  "hits": [],
  "nextCursor": null,
  "coverage": {
    "state": "complete",
    "searchedSources": ["session_metadata"],
    "unavailableSources": [],
    "generation": "local-v1-…"
  }
}
```

Search never changes the current session and never resumes a run. Pass a returned session id to `session show`, `session replay`, or an explicit continuation command.

## Local provider and coverage

`starweaver-storage::LocalSessionSearchProvider` reads canonical session/run records from the injected `SessionStore`. It searches only these default projections:

- session title;
- textual canonical `InputPart` values;
- bounded run output preview;
- public assistant/terminal previews parsed from validated `display.compact.json` compatibility mirrors.

It does **not** search arbitrary metadata, resumable state, checkpoints, environment state, inline binary, resource URI query strings, internal/diagnostic display messages, tool arguments/results, or raw JSON payloads.

Display mirrors are currently best effort. Missing, malformed, oversized, symlinked, or root-escaping files produce `partial` coverage and safe warnings. They do not hide canonical metadata/input/output hits or make a session non-resumable. Local scans bound candidate sessions/runs, files, per-file bytes, aggregate bytes, page size, query bytes, and snippet bytes.

Coverage values are `complete`, `partial`, `eventually_consistent`, and `degraded`. A non-complete empty result is not equivalent to “no matches.”

## Standalone RPC

The standalone RPC product independently constructs its provider from `rpc.toml`; it does not read CLI configuration or call CLI handlers:

```toml
[server]
database_path = "starweaver.sqlite3"

[server.session_search]
enabled = true
backend = "local"
# Optional compatibility mirrors belonging to the same database namespace:
# display_root = "rpc-display"
max_query_bytes = 4096
max_page_size = 100
max_display_files = 1000
max_total_display_bytes = 67108864
max_display_hits = 10000
scan_timeout_ms = 2000
```

`initialize` advertises the `session.search` protocol feature and safe provider capabilities only when a provider is installed. The method requires the HTTP `read` scope. Authorization scope is derived from the authenticated server/database context and is never accepted from request params.

```json
{
  "jsonrpc": "2.0",
  "id": "search_1",
  "method": "session.search",
  "params": {
    "query": "oauth refresh",
    "filters": {
      "status": ["active"],
      "workspace": "/workspace/project"
    },
    "sources": ["session_metadata", "run_input"],
    "granularity": "session",
    "limit": 20,
    "cursor": null
  }
}
```

Malformed and wrong-query cursors map to invalid params. Unsupported modes, sources, filters, or granularities map to `unsupported_feature`. An installed provider that cannot serve a request maps to `session search unavailable`. With search disabled, `session.search` returns `unsupported_feature` rather than an empty successful page.

## Application contracts

Applications can inject any `SessionSearchProvider` beside their `SessionStore`. The shared contract includes typed capabilities, scope, filters, provenance-rich locations, bounded plain-text snippets with highlight offsets, coverage/freshness, and safe error categories.

`SessionSearchIndexWriter` and versioned `SessionSearchMutation` provide an optional conformance seam for materialized or external indexes. The repository includes writer contracts and delayed-write/tombstone conformance evidence, but no external production backend or dependency. SQLite FTS/materialized indexing and external/stateless indexing remain follow-on phases.

Opaque cursors bind the normalized query, host authorization scope, provider, generation, and page position. Reusing a cursor with another query, scope, provider, or generation fails with `invalid_cursor`; providers never silently restart at page one.
