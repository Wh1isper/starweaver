# ya-mono Parity and Migration Plan

This spec records the Starweaver parity target against `Wh1isper/ya-mono` commit `788c926`. It covers SDK filters, media handling, YA Claw backend/frontend behavior, CLI parity with `yaacli`, shared infrastructure refactors, and planned design-debt cleanup.

## Target Outcome

Starweaver should support a direct migration path for current `ya-mono` users:

- `ya-agent-sdk` application behavior maps to Starweaver SDK capabilities and history processors.
- `yaacli` users can move config, sessions, skills, subagents, model profiles, MCP config, and workflows into Starweaver CLI with predictable behavior.
- `ya-claw` users can migrate SQLite or PostgreSQL-backed service data through Starweaver-owned migrations.
- The Claw web console can run against Starweaver Claw with the same API behavior and event contracts.
- Storage, stream replay, SSE, and migration code are shared between CLI and Claw through reusable crates.

## Reference Scope

Audited reference areas from `ya-mono`:

- `packages/ya-agent-sdk/ya_agent_sdk/filters/*`
- `packages/ya-agent-sdk/ya_agent_sdk/media.py`
- `packages/ya-agent-sdk/ya_agent_sdk/utils.py`
- `packages/yaacli/yaacli/*`
- `packages/ya-claw/ya_claw/api/*`
- `packages/ya-claw/ya_claw/controller/*`
- `packages/ya-claw/ya_claw/execution/*`
- `packages/ya-claw/ya_claw/orm/tables.py`
- `apps/ya-claw-web/src/*`

## SDK Filter Parity

Starweaver has the runtime substrate for parity: `HistoryProcessor`, `AgentCapability`, `CapabilityBundle`, context-aware hooks, stream observers, and SDK bundle registration. The parity target is an explicit filter catalog implemented as named SDK capability/history-processor bundles.

| ya-agent-sdk filter        | Starweaver target                                                                        | Current coverage                       | Acceptance evidence                                              |
| -------------------------- | ---------------------------------------------------------------------------------------- | -------------------------------------- | ---------------------------------------------------------------- |
| `auto_load_files`          | auto-load files capability that reads configured paths and appends focused request parts | planned                                | processor tests over virtual and local providers                 |
| `background_shell`         | background process result injection from process-capable providers                       | partial process substrate              | completed-process injection tests with truncation files          |
| `bus_message`              | message bus injection before runtime instruction injection                               | partial context bus substrate          | retry-safe consume-once tests                                    |
| `cold_start`               | low-cost idle-start trimming and tool-return truncation                                  | planned                                | idle-window history tests                                        |
| `environment_instructions` | environment summary and workspace policy instruction injection                           | partial environment provider substrate | environment instruction snapshot tests                           |
| `handoff`                  | restored-history handoff processor with keep tags and steering parts                     | planned                                | restore, handoff, and auto-load ordering tests                   |
| `image`                    | image/video preflight processors                                                         | partial media URL support              | binary image validation, split, compress, GIF, count-limit tests |
| `media_upload`             | media uploader capability and resource/S3 upload adapters                                | planned                                | upload-to-url replacement tests and failure fallback tests       |
| `model_switch`             | model/profile switch normalization                                                       | partial profile presets                | profile switch event and capability update tests                 |
| `reasoning_normalize`      | provider-aware reasoning/thinking cleanup and synthesis                                  | partial provider reasoning mapping     | cross-provider history normalization tests                       |
| `runtime_instructions`     | request-bound runtime context injection                                                  | partial static/dynamic instructions    | reinjection after compact/handoff tests                          |
| `system_prompt`            | system prompt reinjection                                                                | landed                                 | existing reinjection tests plus processor order tests            |
| `tool_args`                | truncated tool-call argument repair                                                      | planned                                | malformed/truncated JSON argument tests                          |
| compact filter             | cache-friendly compact capability over current agent                                     | partial runtime compaction records     | token-threshold compact, keep-tag, event tests                   |
| capability filter          | provider capability based media/document filtering                                       | partial media capability hook          | unsupported media replacement tests                              |

Required processor order:

```text
cold_start -> capability -> image/video preflight -> media_upload -> compact -> handoff -> auto_load_files -> background_shell -> bus_message -> environment_instructions -> runtime_instructions -> system_prompt -> tool_args -> reasoning_normalize
```

Hosts can disable individual processors through policy presets, but the default SDK preset should use this order for ya-mono behavioral parity.

## Media and Image Handling Parity

The ya-mono media fixes address provider rejection, over-limit payloads, and broken image artifacts. Starweaver currently represents user media as URL/file content parts and has TUI image placeholders. Binary image parts, media preflight processing, and upload-to-URL replacement are the main parity gaps.

Required Starweaver media model updates:

- Add canonical request content variants for binary media and resource-backed media references:
  - `ContentPart::Binary { data, media_type }`
  - `ContentPart::ResourceRef { uri, media_type, kind, metadata }`
  - optional `ContentPart::DataUrl { data_url, media_type }` for provider adapters that accept data URLs.
- Detect actual image type from content bytes before provider mapping.
- Normalize declared media type with content detection to avoid provider media-type mismatch.
- Compute raw-byte budgets from base64 encoded API limits before compression.
- Compress oversized static images using progressive JPEG quality reduction and dimension halving.
- Composite alpha images onto a white background before JPEG conversion.
- Preserve animated media as original bytes and route it through GIF support filtering or upload-to-URL behavior.
- Split tall screenshots into overlapping vertical segments before compression/upload.
- Validate binary images and replace corrupted payloads with system reminders.
- Limit latest images/videos by model profile limits.
- Support nested media in tool returns and mapping-like structures, then update tool-return payloads safely.
- Add S3/resource-store upload adapters that run after local image processing.
- Use URL upload for large video payloads by default when the model supports video URLs.
- Keep image upload configurable because some providers handle binary images more reliably than image URLs.

Media acceptance gates:

- tests for PNG/JPEG/GIF/WebP detection and declared-type correction
- tests for base64-size budget compression
- tests for alpha compositing snapshots or byte-size/format assertions
- tests for animated GIF retention plus GIF support filtering
- tests for tall screenshot splitting order and overlap
- tests for corrupt image replacement
- tests for image/video count limits preserving newest media
- tests for binary-to-URL upload with success and adapter failure fallback
- provider replay fixtures for binary image and URL media mapping across OpenAI Chat, OpenAI Responses, Anthropic, Gemini, and Bedrock where supported

## YA Claw Backend API Parity

Starweaver Claw currently exposes a broad Axum route set and includes route compatibility tests. The service needs exact behavior parity for the ya-claw API surface, including colon-action endpoints, response envelopes, state transitions, SSE replay, and storage-backed side effects.

### API Surface Target

| Area                      | Required endpoints                                                                                              |
| ------------------------- | --------------------------------------------------------------------------------------------------------------- |
| health/info/notifications | `/healthz`, `/api/v1/healthz`, `/api/v1/claw/info`, `/api/v1/claw/notifications`, `/api/v1/notifications`       |
| profiles                  | list, get, put, delete, seed                                                                                    |
| workspace                 | runtime, resolve, session workspace, session sandbox, prepare sandbox, stop sandbox                             |
| sessions                  | create, stream create, list, get, submit, create run, stream run, turns, steer, interrupt, cancel, fork, events |
| session memory            | extract and summarize actions                                                                                   |
| async tasks               | list, spawn, get, steer, cancel                                                                                 |
| runs                      | create, stream create, get, trace, steer, respond interaction, interrupt, cancel, events                        |
| schedules                 | list, create, get, patch, delete, pause, resume, trigger, fires                                                 |
| heartbeat                 | config, status, fires, trigger                                                                                  |
| workflows                 | definitions CRUD, archive, trigger, runs, run events, cancel, node steer, agent hidden endpoints                |
| bridges                   | inbound messages, inbound actions, conversations, events, adapter ingress                                       |
| agency                    | config, status, fires, bootstrap, source-session submit, clear                                                  |

### Behavior Parity Requirements

- Register exact colon-action routes such as `/{id}:trigger`, `/{id}:archive`, `/{id}:pause`, `/{id}:resume`, `sandbox:prepare`, `runs:stream`, `sessions:stream`, and memory actions.
- Preserve slash compatibility routes as aliases for client convenience.
- Keep response models aligned with the web client `types.ts` and existing ya-claw API names.
- Implement service-managed same-run approval/deferred interaction responses through `/runs/{run_id}/interactions/{interaction_id}:respond`.
- Implement session memory actions with durable `session_memory_states` rows.
- Implement async task state transitions and child session/run linkage.
- Implement bridge dedupe, normalized event storage, deferred input linkage, and HITL bridge messages.
- Implement agency fires, bootstrap, source-session submit, clear, and pending fire dispatch semantics.
- Implement notification SSE with replay, live tail, and terminal markers through shared stream contracts.
- Implement cancellation and interruption as state transitions plus coordinator signals.
- Add route-contract tests generated from the ya-claw FastAPI route list and ya-claw-web API client calls.

## Storage and Migration Target

Starweaver can own migrations directly and sunset Alembic for migrated users. The migration path should accept a ya-claw database and produce a Starweaver database that preserves sessions, runs, profiles, schedules, workflows, bridges, HITL, memory, async tasks, replay evidence, and runtime metadata.

### Shared Storage Crate Direction

Introduce `starweaver-storage` as a small operational crate for shared SQLite migrations and adapters.

Ownership:

- SQLite connection management for shared session and stream adapters.
- Starweaver migration registry and migration runner.
- Schema metadata table: `starweaver_schema_migrations`.
- Importers for legacy ya-claw SQLite and legacy yaacli session folders.
- Adapter implementations for `SessionStore`, `StreamArchive`, and `ReplayEventLog` that are shared by CLI and Claw.

Product crates keep product-owned orchestration:

- `starweaver-cli`: command parsing, config, terminal renderers, local defaults.
- `starweaver-claw`: HTTP APIs, coordinator, workspace provider, workflow dispatch, schedules, bridge adapters.

### Migration Phases

1. Lock the current Starweaver SQLite schema as `starweaver.storage.v1`.
2. Add a schema metadata endpoint and CLI command to print migration status.
3. Add importer tests that load fixture ya-claw SQLite databases covering profiles, sessions/runs, workflows, schedules, bridges, HITL, async tasks, and memory state.
4. Convert ya-claw ORM table rows into Starweaver durable records with stable ids and preserved JSON payloads.
5. Generate replay/display records from available ya-claw event history where raw Starweaver stream records are absent.
6. Add dry-run migration reports with counts, warnings, and unsupported payload markers.
7. Add backup-before-migrate and rollback guidance for operators.
8. Add PostgreSQL after SQLite importer and schema are stable.

## Frontend Parity

The Starweaver Claw web source mirrors the ya-claw-web structure and feature directories. Functional parity depends on backend behavior, static asset build integration, and API/stream contract tests.

Frontend acceptance gates:

- run web unit tests for API client, workflow page, session history, and AGUI reducer
- run production web build and verify embedded assets through `starweaver-claw`
- add service-backed smoke tests for overview, sessions, run events, profiles, schedules, workflows, bridges, heartbeat, agency, and settings pages
- validate SSE reconnect and replay behavior in `useRunEventStream`
- validate auth header handling and connection gate behavior
- validate colon-action API paths from the web API client against Starweaver service tests

## CLI Parity with yaacli

Starweaver CLI has a strong headless and session foundation. The parity work focuses on interactive behavior, slash commands, worktree workflow, config semantics, media clipboard handling, and asset seeding.

| yaacli feature                  | Starweaver CLI status          | Target                                                                                                                    |
| ------------------------------- | ------------------------------ | ------------------------------------------------------------------------------------------------------------------------- |
| `-p/--prompt` headless          | landed                         | keep output parity with display JSONL and text modes                                                                      |
| `--session/-s` restore          | partial via `--session`        | add `-s` alias and yaacli session-folder importer                                                                         |
| `--profile` / `--model-profile` | partial via profile            | add model-profile alias and profile resolution parity                                                                     |
| `--worker`                      | planned                        | disable sync delegate subagents and emit worker metadata                                                                  |
| `--worktree`, `--branch`        | planned                        | create/resume project-scoped git worktrees under global config                                                            |
| setup wizard                    | partial setup command          | add interactive provider/model/env setup and asset seeding                                                                |
| built-in skills/subagents copy  | partial catalog inspection     | copy missing bundled assets on startup or setup with user override preservation                                           |
| config files                    | partial                        | align global/project precedence, `config.toml`, `tools.toml`, `mcp.json`, env loading, shell env isolation                |
| model profiles                  | partial                        | add switchable profile catalog and labels for TUI/status                                                                  |
| custom slash commands           | planned                        | load `[commands]` prompt definitions with mode hints                                                                      |
| interactive TUI                 | partial retained snapshot      | implement prompt composer, streaming panes, task panels, status bar, model switch, mode switch                            |
| slash commands                  | planned                        | `/help`, `/config`, `/mode`, `/act`, `/plan`, `/loop`, `/tasks`, `/session`, `/dump`, `/load`, `/clear`, `/cost`, `/exit` |
| media clipboard                 | placeholder only               | attach actual binary images and route through media preflight processors                                                  |
| S3 media config                 | planned                        | add `[media.s3]` and upload hooks                                                                                         |
| shell review                    | planned                        | add model-backed shell risk review and approval/defer policy                                                              |
| OAuth refresh                   | partial auth commands          | add proactive refresh settings and startup refresh                                                                        |
| session folders                 | different local SQLite model   | add importer/exporter and compatibility commands                                                                          |
| session trim                    | landed richer Starweaver model | add yaacli retention config compatibility                                                                                 |
| worktree resume hints           | planned                        | print branch/path resume guidance                                                                                         |
| fatal error diagnostics         | partial diagnostics            | add common issue hints and log path output                                                                                |

## Refactor Plan and Design Debt

### Current Refactor Pressure

- `starweaver-claw/src/service.rs` is a large route, DTO, compatibility, and test module.
- `starweaver-claw/src/storage.rs` combines migrations, SQLite adapters, import logic, replay event storage, and tests.
- CLI local storage and Claw storage share concepts with separate implementations.
- SSE framing exists in the service layer, while replay and transport semantics belong in `starweaver-stream`.
- The web API contract and service DTOs currently rely on broad JSON payloads in several places.

### Planned Refactors

01. Split Claw HTTP code into `api/routes`, `api/dto`, `api/extractors`, `api/sse`, and `api/compat` modules.
02. Move reusable SSE replay framing into `starweaver-stream` as `SseReplayTransport` behind an HTTP feature.
03. Move SQLite session/replay adapters and migrations into `starweaver-storage`, then add StreamArchive and importers as follow-up slices.
04. Keep product-specific tables and controllers in Claw, while shared evidence storage lives in the storage crate.
05. Add typed API DTOs for every ya-claw-web response model and retire broad `Value` passthroughs endpoint by endpoint.
06. Split migration SQL into versioned files or Rust modules with checksums.
07. Add an OpenAPI or schema snapshot for route contracts once DTOs stabilize.
08. Add service contract tests generated from route inventory and web client API methods.
09. Split CLI TUI into interaction, composer, renderer, slash-command, media, and session-controller modules.
10. Add migration/import commands before changing persisted schema again.

## Validation Gates

```bash
make fmt-check
make check
make test
make docs-check
make replay-check
make coverage-ci
```

Focused gates to add:

```bash
cargo test -p starweaver-agent --test media_filters --locked
cargo test -p starweaver-model --test multimodal_mapping --locked
cargo test -p starweaver-claw --test api_contract --locked
cargo test -p starweaver-claw --test migration_ya_claw --locked
cargo test -p starweaver-storage --test sqlite_migrations --locked
cargo test -p starweaver-cli --test yaacli_parity --locked
npm --prefix crates/starweaver-claw/web test
npm --prefix crates/starweaver-claw/web run build
```

## Objective Coverage Matrix

| User requirement                                     | Evidence in this spec                                                                                         | Follow-up artifact                                                      |
| ---------------------------------------------------- | ------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------- |
| 1. SDK filter migration status                       | `SDK Filter Parity` table and ordered default processor target                                                | `memos/implementation-todo.md` N2.6 SDK filter parity work              |
| 2. ya-agent-sdk image fix comparison                 | `Media and Image Handling Parity` and media acceptance gates                                                  | `memos/implementation-todo.md` N2.6 media/image fix parity work         |
| 3. ya-claw backend/API/migration/frontend parity     | `YA Claw Backend API Parity`, `Storage and Migration Target`, `Frontend Parity`, and route inventory appendix | `spec/ops/03-durable-service-runtime.md` compatibility section          |
| 4. CLI vs yaacli detailed gap                        | `CLI Parity with yaacli` and detailed CLI gap appendix                                                        | `spec/ops/04-cli-product.md` yaacli parity gate                         |
| 5. Repository refactors and shared SSE/storage reuse | `Refactor Plan and Design Debt` and shared infrastructure sections                                            | `spec/ops/02-shared-execution-components.md` shared storage direction   |
| 6. Design compromises and planned refactors          | `Current Refactor Pressure`, `Planned Refactors`, and design debt list                                        | `memos/implementation-todo.md` N2.6 shared infrastructure refactor work |

## Appendix A: Audited ya-claw Route Inventory

This route inventory comes from the current ya-mono FastAPI routers and ya-claw-web API client calls. Starweaver service tests should cover every path here, including colon-action spelling.

### Root and frontend

| Method | Path                | Purpose                                          |
| ------ | ------------------- | ------------------------------------------------ |
| GET    | `/`                 | root or frontend index depending deployment mode |
| GET    | `/{full_path:path}` | frontend SPA fallback                            |

### Claw info and notifications

| Method | Path                         | Purpose                                                            |
| ------ | ---------------------------- | ------------------------------------------------------------------ |
| GET    | `/api/v1/claw/info`          | service metadata, feature flags, workspace backend, build metadata |
| GET    | `/api/v1/claw/notifications` | notification SSE stream                                            |
| GET    | `/api/v1/notifications`      | compatibility notification SSE stream                              |
| GET    | `/healthz`                   | service health                                                     |
| GET    | `/api/v1/healthz`            | versioned service health                                           |

### Profiles

| Method | Path                              | Purpose                   |
| ------ | --------------------------------- | ------------------------- |
| GET    | `/api/v1/profiles`                | list profiles             |
| GET    | `/api/v1/profiles/{profile_name}` | get profile detail        |
| PUT    | `/api/v1/profiles/{profile_name}` | upsert profile            |
| DELETE | `/api/v1/profiles/{profile_name}` | delete profile            |
| POST   | `/api/v1/profiles/seed`           | seed configured profiles  |
| POST   | `/api/v1/profiles:seed`           | seed compatibility action |

### Workspace and sandbox

| Method | Path                                            | Purpose                           |
| ------ | ----------------------------------------------- | --------------------------------- |
| GET    | `/api/v1/workspace/runtime`                     | workspace provider runtime status |
| POST   | `/api/v1/workspace:resolve`                     | resolve workspace binding         |
| GET    | `/api/v1/sessions/{session_id}/workspace`       | session workspace state           |
| GET    | `/api/v1/sessions/{session_id}/sandbox`         | session sandbox state             |
| POST   | `/api/v1/sessions/{session_id}/sandbox:prepare` | prepare session sandbox           |
| POST   | `/api/v1/sessions/{session_id}/sandbox:stop`    | stop session sandbox              |

### Sessions

| Method | Path                                        | Purpose                          |
| ------ | ------------------------------------------- | -------------------------------- |
| POST   | `/api/v1/sessions`                          | create session                   |
| POST   | `/api/v1/sessions:stream`                   | create session and stream events |
| GET    | `/api/v1/sessions`                          | list sessions                    |
| GET    | `/api/v1/sessions/{session_id}`             | get session detail               |
| GET    | `/api/v1/sessions/{session_id}/turns`       | list session turns               |
| POST   | `/api/v1/sessions/{session_id}/submit`      | submit input to session          |
| POST   | `/api/v1/sessions/{session_id}/runs`        | create run in session            |
| POST   | `/api/v1/sessions/{session_id}/runs:stream` | create run and stream events     |
| POST   | `/api/v1/sessions/{session_id}/steer`       | steer active run                 |
| POST   | `/api/v1/sessions/{session_id}/interrupt`   | interrupt active run             |
| POST   | `/api/v1/sessions/{session_id}/cancel`      | cancel active run                |
| POST   | `/api/v1/sessions/{session_id}/fork`        | fork session                     |
| GET    | `/api/v1/sessions/{session_id}/events`      | stream session events            |

### Session memory and async tasks

| Method | Path                                                                 | Purpose                  |
| ------ | -------------------------------------------------------------------- | ------------------------ |
| POST   | `/api/v1/sessions/{session_id}/memory:extract`                       | extract session memory   |
| POST   | `/api/v1/sessions/{session_id}/memory:summarize`                     | summarize session memory |
| GET    | `/api/v1/sessions/{session_id}/async-tasks`                          | list session async tasks |
| POST   | `/api/v1/sessions/{session_id}/async-tasks:spawn`                    | spawn session async task |
| GET    | `/api/v1/sessions/{session_id}/async-tasks/{task_id_or_name}`        | get async task           |
| POST   | `/api/v1/sessions/{session_id}/async-tasks/{task_id_or_name}:steer`  | steer async task         |
| POST   | `/api/v1/sessions/{session_id}/async-tasks/{task_id_or_name}:cancel` | cancel async task        |

### Runs and HITL

| Method | Path                                                          | Purpose                                  |
| ------ | ------------------------------------------------------------- | ---------------------------------------- |
| POST   | `/api/v1/runs`                                                | create run                               |
| POST   | `/api/v1/runs:stream`                                         | create run and stream events             |
| GET    | `/api/v1/runs/{run_id}`                                       | get run detail                           |
| GET    | `/api/v1/runs/{run_id}/trace`                                 | get run trace                            |
| POST   | `/api/v1/runs/{run_id}/steer`                                 | steer run                                |
| POST   | `/api/v1/runs/{run_id}/interactions/{interaction_id}:respond` | respond to approval/deferred interaction |
| POST   | `/api/v1/runs/{run_id}/interrupt`                             | interrupt run                            |
| POST   | `/api/v1/runs/{run_id}/cancel`                                | cancel run                               |
| GET    | `/api/v1/runs/{run_id}/events`                                | stream run events                        |

### Workflows

| Method | Path                                                            | Purpose                                |
| ------ | --------------------------------------------------------------- | -------------------------------------- |
| GET    | `/api/v1/workflows`                                             | list workflow definitions              |
| POST   | `/api/v1/workflows`                                             | create workflow definition             |
| GET    | `/api/v1/workflows/{workflow_id}`                               | get workflow definition                |
| PATCH  | `/api/v1/workflows/{workflow_id}`                               | update workflow definition             |
| POST   | `/api/v1/workflows/{workflow_id}:archive`                       | archive workflow definition            |
| POST   | `/api/v1/workflows/{workflow_id}:trigger`                       | trigger workflow                       |
| GET    | `/api/v1/workflow-runs`                                         | list workflow runs                     |
| GET    | `/api/v1/workflow-runs/{workflow_run_id}`                       | get workflow run                       |
| GET    | `/api/v1/workflow-runs/{workflow_run_id}/events`                | list workflow events                   |
| POST   | `/api/v1/workflow-runs/{workflow_run_id}/cancel`                | cancel workflow run                    |
| POST   | `/api/v1/workflow-runs/{workflow_run_id}/nodes/{node_id}/steer` | steer workflow node                    |
| POST   | `/api/v1/agent/workflows`                                       | hidden agent workflow create endpoint  |
| POST   | `/api/v1/agent/workflows/{workflow_id}:trigger`                 | hidden agent workflow trigger endpoint |

### Schedules and heartbeat

| Method | Path                                      | Purpose              |
| ------ | ----------------------------------------- | -------------------- |
| GET    | `/api/v1/schedules`                       | list schedules       |
| POST   | `/api/v1/schedules`                       | create schedule      |
| GET    | `/api/v1/schedules/{schedule_id}`         | get schedule         |
| PATCH  | `/api/v1/schedules/{schedule_id}`         | update schedule      |
| DELETE | `/api/v1/schedules/{schedule_id}`         | delete schedule      |
| POST   | `/api/v1/schedules/{schedule_id}:pause`   | pause schedule       |
| POST   | `/api/v1/schedules/{schedule_id}:resume`  | resume schedule      |
| POST   | `/api/v1/schedules/{schedule_id}:trigger` | trigger schedule     |
| GET    | `/api/v1/schedules/{schedule_id}/fires`   | list schedule fires  |
| GET    | `/api/v1/heartbeat/config`                | heartbeat config     |
| GET    | `/api/v1/heartbeat/status`                | heartbeat status     |
| GET    | `/api/v1/heartbeat/fires`                 | list heartbeat fires |
| POST   | `/api/v1/heartbeat:trigger`               | trigger heartbeat    |

### Bridges and agency

| Method | Path                                   | Purpose                       |
| ------ | -------------------------------------- | ----------------------------- |
| POST   | `/api/v1/bridges/inbound/messages`     | ingest bridge message         |
| POST   | `/api/v1/bridges/inbound/actions`      | ingest bridge action          |
| GET    | `/api/v1/bridges/conversations`        | list bridge conversations     |
| GET    | `/api/v1/bridges/events`               | list bridge events            |
| POST   | `/api/v1/bridges/{adapter}/events`     | adapter-specific ingress      |
| GET    | `/api/v1/agency/config`                | agency config                 |
| GET    | `/api/v1/agency/status`                | agency status                 |
| GET    | `/api/v1/agency/fires`                 | list agency fires             |
| POST   | `/api/v1/agency:bootstrap`             | bootstrap agency              |
| POST   | `/api/v1/agency/source-session:submit` | submit source session handoff |
| POST   | `/api/v1/agency:clear`                 | clear agency state            |

## Appendix B: Detailed yaacli Gap Inventory

### Startup and configuration

| Area                | yaacli behavior                                                                                                    | Starweaver work                                                         |
| ------------------- | ------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------- |
| runtime preparation | load `.env`, ensure config dir, copy built-in subagents and skills, run setup wizard when unconfigured             | add startup asset seeding and interactive setup flow                    |
| config precedence   | project-level files override global files by file family                                                           | add parity tests for `config.toml`, `tools.toml`, `mcp.json` precedence |
| env injection       | `[env]` updates CLI process env, `[shell_env]` updates shell subprocess env, `include_os_env` controls inheritance | extend resolver and shell provider policy tests                         |
| model profiles      | profile id resolves model, model settings, model config, label                                                     | add model profile catalog and status/TUI switching                      |
| media config        | `[media.s3]` config creates S3 hook for video upload and optional media upload policies                            | add Starweaver media uploader config and SDK adapter wiring             |
| security review     | model-backed shell review with risk threshold and approval behavior                                                | add shell review capability and tests                                   |
| OAuth refresh       | proactive refresh settings and startup refresh                                                                     | extend auth service and status output                                   |

### Top-level CLI flags and commands

| Feature                         | yaacli behavior                                                     | Starweaver work                                                                      |
| ------------------------------- | ------------------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| `-p/--prompt`                   | headless prompt run, NDJSON display events, saved session artifacts | preserve current Starweaver headless mode and add yaacli output compatibility checks |
| `-s/--session`                  | restore saved session by exact or prefix id                         | add `-s` alias and yaacli session import lookup                                      |
| `--profile` / `--model-profile` | select model profile id                                             | add alias and model profile status evidence                                          |
| `--worker`                      | headless worker mode without synchronous delegate subagents         | add delegation-disable mode                                                          |
| `--worktree` / `--branch`       | create or resume git worktree under global config                   | add worktree manager and resume hints                                                |
| `sessions list/show/delete`     | file-backed saved session management                                | add import/export and compatibility commands over Starweaver store                   |

### Interactive TUI and slash commands

| Command or surface       | yaacli behavior                                          | Starweaver work                                          |
| ------------------------ | -------------------------------------------------------- | -------------------------------------------------------- |
| `/help`                  | list built-in and custom commands                        | slash-command registry                                   |
| `/config`                | show or edit active config                               | TUI config panel/action                                  |
| `/mode`, `/act`, `/plan` | switch operating mode                                    | mode state and prompt decoration                         |
| `/loop`                  | autonomous goal loop                                     | loop controller over session runs                        |
| `/tasks`                 | show background tasks and processes                      | task/process panel over process provider and async tasks |
| `/session`               | list and restore sessions                                | TUI session browser                                      |
| `/dump`                  | dump session artifacts to folder                         | exporter over session/stream records                     |
| `/load`                  | load session artifacts from folder                       | importer over legacy/current artifact folders            |
| `/clear`                 | clear conversation display/history                       | session state reset action                               |
| `/cost`                  | show usage/cost                                          | usage ledger panel                                       |
| `/exit`                  | exit TUI                                                 | shutdown action with resume hints                        |
| custom commands          | config-defined prompts with optional mode                | command config loader and registry tests                 |
| composer                 | prompt input, paste image, steering messages, status bar | interactive composer and media attachment pipeline       |

### Session artifacts and migration

| yaacli artifact                  | Migration target                                          |
| -------------------------------- | --------------------------------------------------------- |
| `metadata.json`                  | `SessionRecord` metadata and imported source marker       |
| `turns/*/messages.json`          | run input/output history and replay reconstruction source |
| `turns/*/state.json`             | `ResumableState` import into session state snapshot       |
| `turns/*/display_messages.jsonl` | `StreamArchive` display messages                          |
| session retention config         | Starweaver trim policy compatibility layer                |

## Appendix C: Detailed Refactor Inventory

| Current module                          | Pressure                                                                     | Target split                                                                                   |
| --------------------------------------- | ---------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| `crates/starweaver-claw/src/service.rs` | routes, DTOs, compatibility, SSE, tests, and frontend fallback in one module | `api/routes`, `api/dto`, `api/extractors`, `api/sse`, `api/compat`, focused tests              |
| `crates/starweaver-claw/src/storage.rs` | migration SQL, SQLite store, replay log, import logic, tests in one module   | migration registry, schema modules, session adapter, stream adapter, replay adapter, importers |
| CLI local store and Claw store          | duplicated SQLite/session/replay concepts                                    | `starweaver-storage` with product-level adapter selection                                      |
| service SSE                             | Axum-edge framing mixed with service handlers                                | `starweaver-stream::SseReplayTransport` plus service adapter                                   |
| web API DTOs                            | broad JSON passthroughs in several service methods                           | typed DTOs aligned with web `types.ts` and schema snapshots                                    |
| media handling                          | split across CLI placeholders, SDK URL tools, and model mapping              | SDK media preflight pipeline plus provider mapping fixtures                                    |
| CLI TUI                                 | renderer/state/composer concerns concentrated in TUI modules                 | slash commands, composer/media, renderer, session controller, task/process panels              |
