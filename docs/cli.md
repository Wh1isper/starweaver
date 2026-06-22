# CLI

`starweaver-cli` is the local headless product surface for Starweaver. It runs prompts through the SDK runtime stream, projects runtime records into AGUI-named `DisplayMessage` events, persists replay evidence locally, and prints either human-readable text or display JSONL.

## Launchers

Installations include four local entry points:

| Binary           | Role                         |
| ---------------- | ---------------------------- |
| `starweaver`     | product launcher             |
| `sw`             | short alias for `starweaver` |
| `starweaver-cli` | local agent CLI command      |
| `starweaver-rpc` | standalone JSON-RPC host     |

Launcher dispatch keeps product commands easy to discover:

```bash
sw
sw --help
sw -p "hello"
sw run "hello"
sw session list
starweaver version
starweaver doctor
starweaver update
starweaver update cli
starweaver cli
starweaver cli -p "hello"
starweaver rpc stdio
```

`sw` and `starweaver` print launcher help with no arguments. Prompt flags and common CLI commands such as `run`, `session`, `config`, `auth`, and `tools` dispatch directly to the local CLI product, so `sw -p "hello"` and `sw session list` are shortcuts for `sw cli -p "hello"` and `sw cli session list`. Unknown command families still dispatch to `starweaver-<command> ...`; the launcher resolves command binaries from the install directory first, then `PATH`. From a checkout, `make cli` runs the same product path as `sw cli`: in a terminal it opens the interactive TUI, and prompt arguments can be passed with `make cli -- -p "hello"`.

## Updates

`starweaver update` updates the installed CLI component through the installer workflow. The direct CLI command supports the same target selection:

```bash
starweaver update
starweaver update cli
starweaver update --dry-run
starweaver update --force
starweaver-cli update
```

Update first compares the current CLI package version with the selected release. It returns
`status=up-to-date` without downloading assets when the latest release is not newer. Set
`STARWEAVER_VERSION` to install a pinned release; pinned releases only skip when the selected version
matches the current version, so explicit rollbacks remain possible. Use `--force` or
`STARWEAVER_UPDATE_FORCE=1` to reinstall the selected release.

The CLI also performs a short background release lookup and caches the result in `~/.starweaver/update-check.json`. Human-readable commands append a compact hint when the cache reports a newer release:

```text
Update available: starweaver 0.0.1 -> 0.0.2. Run `starweaver update`.
```

Scripted environments can disable the hint and background lookup:

```bash
export STARWEAVER_UPDATE_CHECK=0
```

## Profiles

A run uses an AgentSpec profile. Built-in profiles are available immediately:

| Profile     | Model id                      | Purpose                        |
| ----------- | ----------------------------- | ------------------------------ |
| `general`   | `local_echo`                  | deterministic local validation |
| `coding`    | `openai:gpt-5`                | coding workflows               |
| `research`  | `anthropic:claude-sonnet-4-5` | research workflows             |
| `workspace` | `local_echo`                  | environment tool validation    |

List and inspect profiles:

```bash
starweaver-cli profile list
starweaver-cli profile show coding
```

Pass `--profile` a built-in name, a config-backed model profile, a profile name from `.starweaver/profiles`, `.starweaver/agents`, or the global profile directory, or a YAML path directly:

```bash
starweaver-cli run -p "hello" --profile general
starweaver-cli run -p "hello" --profile default_model
starweaver-cli run -p "hello" --profile codex
starweaver-cli run -p "hello" --profile .starweaver/profiles/research.yaml
```

A minimal file-backed profile looks like this:

```yaml
name: research
instructions:
  - You are a concise research assistant.
model:
  model_id: anthropic:claude-sonnet-4-5
  settings_preset: anthropic_default
toolsets:
  - environment
```

Config-backed model profiles are declared in `config.toml`:

```toml
[general]
default_profile = "default_model"
model = "openai-responses:gpt-5"
model_settings = "openai_responses_high"
model_cfg = "gpt5_270k"
max_requests = 1000

[model_profiles.codex]
label = "Codex OAuth"
model = "oauth@codex:gpt-5"
model_settings = "openai_responses_high"
model_cfg = "gpt5_270k"
```

OAuth-backed Codex profiles use the `oauth@codex:<model>` model id and read credentials from `~/.starweaver/auth.json` by default. Use `STARWEAVER_OAUTH_AUTH_FILE` or `--auth-file` on auth commands for an alternate file.

```bash
starweaver-cli auth login codex
starweaver-cli auth status codex
starweaver-cli auth refresh codex
starweaver-cli auth doctor
starweaver-cli auth logout codex
```

Provider-backed model ids use these prefixes:

| Model id pattern                     | Protocol                          |
| ------------------------------------ | --------------------------------- |
| `openai:<model>`                     | OpenAI Responses                  |
| `openai-responses:<model>`           | OpenAI Responses                  |
| `openai-chat:<model>`                | OpenAI Chat Completions           |
| `anthropic:<model>`                  | Anthropic Messages                |
| `claude:<model>`                     | Anthropic Messages                |
| `gemini:<model>`                     | Gemini generateContent            |
| `google-vertex:<model>`              | Gemini generateContent            |
| `<gateway>@openai-responses:<model>` | gateway-routed OpenAI Responses   |
| `<gateway>@openai-chat:<model>`      | gateway-routed OpenAI Chat        |
| `<gateway>@google-vertex:<model>`    | gateway-routed Gemini             |
| `oauth@codex:<model>`                | Codex OAuth over OpenAI Responses |

Deterministic local model ids remain available for tests and offline validation: `local_echo`, `approval_model`, and `deferred_model`.

## Provider configuration

Initialize config, then export provider API keys:

```bash
starweaver-cli config init --global
export OPENAI_API_KEY=...
export ANTHROPIC_API_KEY=...
export GEMINI_API_KEY=...
```

Provider config stores environment variable names and gateway URLs. It keeps raw API keys in the shell or secret manager. Configuration examples live under `examples/cli/` and can be validated with `make cli-examples-check`.

```toml
[providers.openai]
enabled = true
api_key_env = "OPENAI_API_KEY"
base_url = "https://api.openai.com/v1"

[providers.anthropic]
enabled = true
api_key_env = "ANTHROPIC_API_KEY"
base_url = "https://api.anthropic.com/v1"

[providers.gemini]
enabled = true
api_key_env = "GEMINI_API_KEY"
base_url = "https://generativelanguage.googleapis.com/v1beta"

[providers.codex]
base_url = "https://chatgpt.com/backend-api/codex"
max_tokens_parameter = "omit"

[oauth_refresh]
enabled = true
interval_seconds = 1800
failure_retry_seconds = 60
refresh_on_startup = true
```

`[oauth_refresh]` controls the background refresh supervisor used by CLI, TUI, and RPC runs whenever configured model profiles include `oauth@provider:<model>` ids.

Gateway model ids use the gateway name as a provider config key. `homelab@openai-responses:gpt-5` reads `[providers.homelab]`, falls back to `HOMELAB_API_KEY` for credentials, and can explicitly select gateway-specific request mappings:

```toml
[providers.homelab]
base_url = "https://gateway.example/v1"
max_tokens_parameter = "omit"
```

Provider presets treat a `base_url` with no path as a provider API host and insert the provider API root before the endpoint (`/v1` for OpenAI and Anthropic, `/v1beta` for Gemini). A `base_url` with a path is treated as a gateway mount point, so Starweaver appends only the provider endpoint. Set `endpoint_path` when the gateway needs a fully custom route.

```bash
export HOMELAB_API_KEY=...
starweaver-cli run -p "hello" --profile gateway
```

Useful config commands:

```bash
starweaver-cli config get general.model
starweaver-cli config get model.profiles
starweaver-cli config get providers.openai.ready
starweaver-cli config get providers.openai.base_url
starweaver-cli config get providers.homelab.max_tokens_parameter
starweaver-cli config get oauth_refresh.enabled
starweaver-cli config get oauth_refresh.interval_seconds
starweaver-cli config set --global providers.homelab.base_url https://gateway.example/v1
starweaver-cli config set --global providers.homelab.max_tokens_parameter omit
starweaver-cli diagnostics
```

Provider environment overrides:

```bash
STARWEAVER_OPENAI_BASE_URL=https://gateway.example/v1
STARWEAVER_ANTHROPIC_BASE_URL=https://gateway.example/anthropic
STARWEAVER_GEMINI_BASE_URL=https://gateway.example/gemini
STARWEAVER_OPENAI_API_KEY_ENV=MY_OPENAI_KEY
STARWEAVER_ANTHROPIC_API_KEY_ENV=MY_ANTHROPIC_KEY
STARWEAVER_GEMINI_API_KEY_ENV=MY_GEMINI_KEY
```

## Setup

Use `setup` to create the config skeleton, tool policy file, MCP config file, and skill/subagent directories:

```bash
starweaver-cli setup
starweaver-cli setup --global
starweaver-cli setup --project
starweaver-cli setup --force
```

`setup` prints one JSON object per created or existing item. With no scope or `--global`, it initializes the global `~/.starweaver` config skeleton. Use `--project` to initialize `pwd/.starweaver`; project setup also writes `.starweaver/.gitignore` entries for `state.json`, SQLite files, and the local file store.

## OAuth-backed Codex models

Codex OAuth profiles use `oauth@codex:<model>` and the OpenAI Responses protocol. Starweaver reads credentials from `~/.starweaver/auth.json` or the explicit path in `STARWEAVER_OAUTH_AUTH_FILE`:

```toml
[model_profiles.codex]
label = "Codex OAuth"
model = "oauth@codex:gpt-5"
model_settings = "openai_responses_high"
model_cfg = "gpt5_270k"
```

The OAuth transport attaches Codex request headers, adds session and thread metadata, sets Responses `store=false`, and refreshes the access token once after a `401` response when a refresh token is present. `[providers.codex]` controls the Codex base URL, endpoint path, and max-token parameter mapping.

CLI, TUI, and RPC runs also start a proactive refresh supervisor when `[oauth_refresh].enabled = true` and configured model profiles reference OAuth-backed models. The supervisor refreshes on startup by default, repeats every `interval_seconds`, and uses `failure_retry_seconds` after the latest attempt fails.

Auth inspection commands read and update the local auth store without printing secrets:

```bash
starweaver-cli auth status
starweaver-cli auth status codex
starweaver-cli auth logout codex
```

## Tools, MCP, skills, and subagents

Built-in and config-backed profiles attach the default first-party CLI tool catalog: filesystem, shell, host operations, task operations, skills, and CLI control-flow probes. CLI tools execute without approval by default; add explicit tool names, toolset ids, or `"*"` to opt back into approval gating:

```toml
[tools]
need_approval = []
# need_approval = ["shell", "write", "edit", "multi_edit", "delete", "move"]
```

Filesystem and shell execution policy is resolved from `[environment]` in `config.toml`; `tools.toml` controls tool-level approval gates.

Inspect the effective catalog and policy:

```bash
starweaver-cli tools list
starweaver-cli tools doctor
```

MCP servers are read from global and project `mcp.json`. Declared tools are exposed through `McpToolset`; calls defer to the host MCP runtime with server, transport, exposed tool name, and arguments recorded in the deferred-call metadata. Profiles can also validate MCP server names through `mcp_servers` in `AgentSpec`.

```json
{
  "servers": {
    "docs": {
      "transport": "stdio",
      "command": "npx",
      "args": ["-y", "@example/docs-mcp"],
      "tools": [
        {
          "name": "lookup",
          "description": "Look up documentation by query.",
          "parameters": {"type": "object"}
        }
      ]
    }
  }
}
```

Inspect MCP configuration:

```bash
starweaver-cli mcp list
starweaver-cli mcp show docs
starweaver-cli mcp doctor
```

Skills are loaded from `SKILL.md` packages in configured directories and exposed through the `skills` toolset. By default, the CLI checks `~/.starweaver/skills`, shared Agent Skills packages in `~/.agents/skills`, and project `.starweaver/skills`. These directories are also added to the local environment's allowed paths so model-facing skill paths can be opened with filesystem tools. Subagent markdown files are loaded from configured directories and registered in the CLI AgentSpec registry.

```toml
[skills]
# Overrides the default skill directories.
dirs = ["~/.starweaver/skills", "~/.agents/skills"]
# Adds extra skill directories while keeping defaults.
additional_dirs = [".starweaver/skills"]

[subagents]
dirs = ["~/.starweaver/subagents"]
additional_dirs = [".starweaver/subagents"]
disabled = []
disabled_builtins = []
```

Environment overrides use path-list syntax for directories and comma-list syntax for disabled subagents:

```bash
STARWEAVER_SKILL_DIRS=~/.starweaver/skills:.starweaver/skills
STARWEAVER_SUBAGENT_DIRS=~/.starweaver/subagents:.starweaver/subagents
STARWEAVER_DISABLED_SUBAGENTS=debugger,searcher
```

Catalog commands print JSONL for lists, raw file content for `show`, and compact key-value doctor output:

```bash
starweaver-cli skill list
starweaver-cli skill show cli-config
starweaver-cli skill doctor
starweaver-cli subagent list
starweaver-cli subagent show debugger
starweaver-cli subagent doctor
```

## Headless runs

Run a prompt with either shorthand or the `run` subcommand:

```bash
starweaver-cli -p "summarize this repository"
starweaver-cli run -p "summarize this repository"
sw -p "summarize this repository"
sw run -p "summarize this repository"
```

Output modes:

| Mode            | Flag                     | Output contract                                      |
| --------------- | ------------------------ | ---------------------------------------------------- |
| `text`          | `--output text`          | assistant text and compact tool/status lines         |
| `display-jsonl` | `--output display-jsonl` | one Starweaver `DisplayMessage` JSON object per line |
| `silent`        | `--output silent`        | persisted records and compact final status           |

Examples:

```bash
starweaver-cli run -p "hello" --output text
starweaver-cli run -p "hello" --output display-jsonl
starweaver-cli run -p "hello" --output silent
```

`display-jsonl` is the stable automation and replay format. `text` is the human-readable terminal default for initialized user configs.

## Session selectors

Every prompt-backed command appends a new run under a session.

```bash
starweaver-cli -p "start a session" --new-session
starweaver-cli -p "continue work" --continue
starweaver-cli -p "append here" --session <session-id>
starweaver-cli run -p "branch" --branch-from <run-id>
starweaver-cli run -p "restore from run" --run <run-id>
```

The parser rejects ambiguous selectors such as `--session` with `--new-session`, and `--session` with `--continue`.

## Replay and restore

Display messages are persisted and used as the replay source. Each completed or waiting run also exports `AgentSession::export_state()` so `--continue`, `--run`, and `--branch-from` can restore context before appending the next run:

```bash
starweaver-cli session list
starweaver-cli session show <session-id>
starweaver-cli session replay <session-id>
starweaver-cli session replay <session-id> --run <run-id>
starweaver-cli session replay <session-id> --after <sequence>
starweaver-cli session replay <session-id> --output text
```

Run-scoped replay emits the same display JSONL that the initial headless run emitted when `--output display-jsonl` is selected.

## TUI snapshot and human-in-the-loop policy

The CLI TUI opens a Codex-style inline terminal viewport when stdin and stdout are TTYs. On a fresh machine, it renders a bordered session header card, implemented startup shortcuts, a no-border bottom composer with `› Ask Starweaver to do anything`, and a compact footer with `? for shortcuts` plus right-aligned context. Type a prompt and press Enter or Tab to start a background run; pressing Tab while a run is active queues the draft for the next run. Runtime stream records update the scrollback while the input area stays responsive. Assistant output follows Codex-style terminal Markdown rendering: raw assistant Markdown is parsed with `pulldown-cmark`, reflowed at the current viewport width, and styled for headings, lists, blockquotes, fenced code, inline code, emphasis, strong text, links, and horizontal rules.

```bash
sw tui
make cli
```

Interactive slash commands:

| Command            | Action                                              |
| ------------------ | --------------------------------------------------- |
| `/help`            | Print command help into the transcript              |
| `/clear`           | Clear output                                        |
| `/cost`            | Show usage/context summary                          |
| `/model`           | Open the in-TUI model profile selector              |
| `/model <profile>` | Select a model profile directly for future TUI runs |
| `/session`         | Open the in-TUI session selector                    |
| `/session <id>`    | Reload an exact session id or unique id prefix      |
| `/goal <task>`     | Run toward a verified goal                          |
| `/paste-image`     | Attach image data from the system clipboard         |
| `!<command>`       | Run a shell command and show output inline          |

`/goal <task>` submits one runtime goal run. Goal progress is managed by runtime output validation: incomplete output emits `goal_iteration` and retries the model inside the same run, while verified completion or the iteration ceiling emits `goal_complete`. Configure the ceiling with `general.max_goal_iterations` in `config.toml`; the default is `10`.

Use `Ctrl+V` or `/paste-image` to attach an image currently stored in the system clipboard. The TUI inserts a visible placeholder such as `[Attached image 1: image/png 24KB]` into the composer, but submission strips that generated placeholder and sends the image as inline binary `ContentPart::Binary` content with the first model request. Clipboard image paste currently supports Linux clipboard providers through `wl-paste` on Wayland or `xclip` on X11.

The `/model` selector is embedded in the TUI. Use `Up` / `Down` to move, `Enter` to select, and `Esc` to cancel. The selector shows only user-facing profiles and expands the highlighted profile with its model id, settings preset, config preset, context window, and source so long model ids are easier to inspect. The TUI selected model is client state stored in `~/.starweaver/tui/state.json`. It does not mutate `~/.starweaver/config.toml`; shared config still owns the profile definitions and provider settings. Model selection is only allowed while no run is active.

The `/session` selector uses the same embedded picker style. Use `/session` to view recent local sessions, move with `Up` / `Down`, press `Enter` to reload the highlighted session, or `Esc` to cancel. Use `/session <id>` to reload directly; exact ids and unique id prefixes are supported. Reloading replaces the TUI transcript with persisted display replay, updates the current session pointer, restores the session profile when available, and the next message continues from the loaded history. Session selection is only allowed while no run is active.

Config-backed slash commands are declared in global or project `config.toml` under `[commands.<name>]`. They work in the TUI, headless `run`/`-p`, and JSON-RPC prompt runs. Invoking `/name optional instruction` expands the configured prompt before submission. In the TUI, the transcript shows the expanded prompt directly as the user message, matching the actual prompt sent to the agent. If instruction text is provided and the prompt has no `{instruction}`, `{{instruction}}`, `{args}`, or `{{args}}` placeholder, Starweaver appends `User instruction: <instruction>`. Built-in slash commands such as `/help`, `/model`, `/session`, `/goal`, `/paste-image`, `/clear`, and `/cost` remain reserved and cannot be overridden.

```toml
[commands.review]
description = "Review the current changes"
aliases = ["rv"]
prompt = """
Review the working tree changes for correctness, safety, and missing tests.
"""

[commands.test]
description = "Run targeted tests"
prompt = "Run tests for {{instruction}} and report failures with fixes."
```

```bash
starweaver-cli -p "/review staged diff" --output text
starweaver-cli run "/rv src/service.rs"
```

Interactive keys:

| Key                     | Action                                             |
| ----------------------- | -------------------------------------------------- |
| `Enter`                 | Submit message                                     |
| `Tab`                   | Submit message, or queue while running             |
| `Ctrl-O`                | Insert a newline                                   |
| `?`                     | Show or hide shortcut overlay from an empty prompt |
| `Shift-Tab`             | Toggle ACT/PLAN mode                               |
| `Ctrl-R`                | Recall the previous prompt                         |
| `Up` / `Down`           | Browse prompt history                              |
| `PageUp` / `PageDown`   | Scroll transcript                                  |
| Mouse wheel             | Scroll transcript                                  |
| `Ctrl-Up` / `Ctrl-Down` | Scroll transcript one line                         |
| `Ctrl-L`                | Jump to the live bottom                            |
| `Esc`                   | Enter transcript selection mode while idle         |
| `Ctrl-C`                | Request interruption during a run; exit while idle |
| `Ctrl-D`                | Exit                                               |
| `q`                     | Exit from an empty idle prompt                     |

The retained snapshot renderer remains available for scripts, tests, and display-message replay. It uses the same replay source as headless JSONL and session replay:

```bash
starweaver-cli tui --snapshot
starweaver-cli tui --session <session-id> --snapshot
starweaver-cli tui --session <session-id> --run <run-id> --snapshot
starweaver-cli tui --session <session-id> --output display-jsonl
```

Headless HITL defaults to `deny`. The current policy is stored in display message metadata for auditability. Tool-return control flow with `approval_required` or `call_deferred` is projected into `APPROVAL_REQUESTED` and `APPROVAL_RESOLVED` display messages, persisted as approval/deferred records, and can place the run in `waiting` or `failed` status depending on policy:

```bash
starweaver-cli -p "hello" --hitl deny
starweaver-cli -p "hello" --hitl defer
starweaver-cli -p "hello" --hitl fail
starweaver-cli -p "hello" --hitl prompt
```

Persisted approval records can be inspected and decided from the CLI:

```bash
starweaver-cli approval list
starweaver-cli approval show <approval-id>
starweaver-cli approval approve <approval-id> --reason "reviewed"
starweaver-cli approval reject <approval-id> --reason "blocked"
```

Persisted deferred tool calls can be inspected, completed, failed, and resumed through a continuation run:

```bash
starweaver-cli deferred list
starweaver-cli deferred show <deferred-id>
starweaver-cli deferred complete <deferred-id> --result '{"ok": true}'
starweaver-cli deferred fail <deferred-id> --error "worker failed"
starweaver-cli resume --session <session-id> --prompt "continue after review"
```

`resume` appends a continuation run from the waiting or head run state. Service-managed same-run checkpoint reload, interruption APIs, service transports, workflows, and schedules belong to future service adapters.

## JSON-RPC host service

`starweaver-rpc` and `starweaver-cli rpc` start the JSON-RPC 2.0 host service. `starweaver-rpc` is the dedicated Desktop/local host process with its own command-line entrypoint; `starweaver-cli rpc` exposes the same server for CLI-managed installs and launcher compatibility. TUI uses the same in-process runtime coordinator and local session store rather than launching through RPC.

```bash
starweaver rpc stdio
starweaver-rpc stdio
starweaver-rpc http --host 127.0.0.1 --port 8765
starweaver cli rpc
starweaver-cli rpc
starweaver-cli rpc stdio
starweaver-cli rpc http --host 127.0.0.1 --port 8765
```

The default `stdio` transport is newline-delimited JSON-RPC over stdin/stdout. It supports responses and live notifications on stdout, with diagnostics on stderr. JSON-RPC frame parsing, standard request validation, error envelopes, replay cursor parsing, and stream payload projection live in `starweaver-rpc-core` so the standalone RPC process and CLI adapter share the same protocol edge.

The `http` transport serves JSON-RPC request/response calls at `POST /rpc` on the configured host and port. It is useful for local host integrations that prefer HTTP. Live server notifications are not streamed over the unary HTTP endpoint; HTTP `initialize` responses advertise `liveDisplay: false` and `streamSubscribe: false`. HTTP clients should use `run.await`, `run.status`, or `stream.replay` to observe progress.

Example handshake and client model selection:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"tui"}}}
{"jsonrpc":"2.0","id":2,"method":"model.list","params":{"client":"tui"}}
{"jsonrpc":"2.0","id":3,"method":"model.select","params":{"client":"tui","profile":"coding"}}
{"jsonrpc":"2.0","id":4,"method":"run.start","params":{"client":"tui","prompt":"hello","newSession":true}}
{"jsonrpc":"2.0","id":5,"method":"stream.subscribe","params":{"sessionId":"session_...","runId":"run_...","subscriptionId":"sub_1"}}
{"jsonrpc":"2.0","id":6,"method":"stream.unsubscribe","params":{"subscriptionId":"sub_1"}}
{"jsonrpc":"2.0","id":7,"method":"shutdown","params":{}}
```

The `stream.subscribe` example is for notification-capable transports such as stdio. Unary HTTP clients should omit live subscriptions and use replay/status polling.

`model.select` writes `~/.starweaver/tui/state.json` or `~/.starweaver/desktop/state.json` depending on the `client` parameter. `run.prompt` and `run.start` use this priority for model selection: explicit `profile`/`modelProfile`, then selected profile for the supplied `client`, then the resolved config default profile.

`run.start` is non-blocking: it returns `sessionId`, `runId`, `status`, and `payloadFormat` after durable run creation and active-run registration. The runtime coordinator emits scoped Starweaver replay events; the RPC protocol edge maps those events into `run.started`, `run.output`, and `run.status` notifications. Stream payloads default to `agui`; pass `payloadFormat` or `stream.payloadFormat` as `display_message` to receive Starweaver `DisplayMessage` payloads instead. `run.prompt` remains the blocking method that returns the compact final JSON summary.

Use `stream.replay` for persisted output and `stream.subscribe` / `stream.unsubscribe` for explicit stream subscription lifecycle on notification-capable transports. `session.replay`, `session.output`, and `run.attach` remain product-shaped aliases over the same replay and active-run coordinator paths. Cursor semantics are scope-local: run output uses `run:<runId>` sequence values, while session output uses `session:<sessionId>` sequence values over the ordered session display feed. RPC clients may pass a full `cursor` object or the numeric `after` shorthand, which is interpreted within the requested scope. Active control methods are `run.cancel`, `run.steer`, `session.steer`, and `run.await`.

## Environment

The CLI attaches an environment provider to every `AgentSession`. The default is a local read-only provider rooted at the workspace. Config can select `local` or `virtual`, file policy (`read_only`, `read_write`, or `disabled`), workspace root, and shell enablement metadata:

```toml
[environment]
provider = "local"
workspace_root = "."
files_policy = "read_only"
shell_enabled = false
```

Profiles can request `environment`, `filesystem`, and `shell` toolsets. Local policy still governs actual file and shell behavior.

## Local storage

The CLI uses a local SQLite database and file store. Diagnostics print resolved paths, provider readiness, workspace policy, and WAL status:

```bash
starweaver-cli diagnostics
```

Each completed or waiting run stores raw runtime stream evidence, compact display archives, context state, and environment state under the configured file store:

```text
sessions/<session-id>/runs/<run-id>/raw.stream.json
sessions/<session-id>/runs/<run-id>/display.compact.json
sessions/<session-id>/runs/<run-id>/context.state.json
sessions/<session-id>/runs/<run-id>/environment.state.json
```

SQLite tables retain context states, environment snapshots, stream cursors, checkpoint refs, approvals, deferred tool records, file checksums, and compact run projections. `LocalSessionStore` and `LocalStreamArchive` adapt the CLI store to the shared `SessionStore` and `StreamArchive` contracts, exposing persisted session lifecycle and display output as `ReplayScope` / `ReplayCursor` windows so RPC, TUI, and headless replay consume the same durable contracts while the CLI storage path continues converging on shared storage adapters.

Trim retained run evidence with dry-run and age-filter support:

```bash
starweaver-cli session trim --session <session-id> --keep-runs 20 --dry-run
starweaver-cli session trim --session <session-id> --keep-runs 20 --older-than 7d
starweaver-cli session trim --all --keep-runs 20 --older-than 30d
```

Reset runtime state while preserving configuration:

```bash
starweaver-cli reset --yes
sw reset --yes
```

`reset` removes the resolved SQLite database, `state.json`, and file store. It leaves `config.toml`, `tools.toml`, `mcp.json`, `skills/`, and `subagents/` in place.

## Config

Global Starweaver configuration lives under `~/.starweaver`. `~/.starweaver/config.toml` stores shared defaults, provider settings, and model profile definitions. `~/.starweaver/tools.toml`, `~/.starweaver/mcp.json`, `~/.starweaver/skills`, and `~/.starweaver/subagents` are shared by CLI, TUI, Desktop, and RPC hosts. The CLI also discovers shared Agent Skills from `~/.agents/skills` by default. Frontend state is separate: TUI uses `~/.starweaver/tui/state.json`, Desktop uses `~/.starweaver/desktop/state.json`, and project runtime state keeps the current session pointer in `.starweaver/state.json`.

Resolution order is built-in defaults, global `config.toml`, project `config.toml`, `tools.toml` and `mcp.json` metadata, environment variables, then command flags. Supported environment overrides include `STARWEAVER_CONFIG_DIR`, `STARWEAVER_PROJECT_DIR`, `STARWEAVER_PROFILE`, `STARWEAVER_SKILL_DIRS`, `STARWEAVER_SUBAGENT_DIRS`, `STARWEAVER_DISABLED_SUBAGENTS`, `STARWEAVER_SESSION_DB`, `STARWEAVER_FILE_STORE`, `STARWEAVER_WORKSPACE_ROOT`, `STARWEAVER_ENV_PROVIDER`, `STARWEAVER_FILES_POLICY`, `STARWEAVER_SHELL_ENABLED`, `STARWEAVER_OUTPUT`, `STARWEAVER_HITL`, `STARWEAVER_MAX_GOAL_ITERATIONS`, `STARWEAVER_IMAGE_UNDERSTANDING_MODEL`, `STARWEAVER_VIDEO_UNDERSTANDING_MODEL`, `STARWEAVER_AUDIO_UNDERSTANDING_MODEL`, `STARWEAVER_UPDATE_CHANNEL`, `STARWEAVER_UPDATE_CHECK`, `STARWEAVER_UPDATE_DRY_RUN`, `STARWEAVER_UPDATE_FORCE`, `STARWEAVER_OAUTH_AUTH_FILE`, and `STARWEAVER_NO_AUTO_TRIM`.

Get resolved config values and persist project or global config overrides:

```bash
starweaver-cli config get trim.current_session_keep_recent_runs
starweaver-cli config get metadata.tools
starweaver-cli config get metadata.unmapped
starweaver-cli config set trim.current_session_keep_recent_runs 10
starweaver-cli config set --global general.default_profile general
```

Starweaver preserves recognized display, subagent, command, security, and max-request fields in unmapped metadata so configuration audits can map those sections into first-class Starweaver settings over time.

## Shell completions

Generate shell completion scripts from the same clap schema used by the CLI:

```bash
starweaver-cli completion bash
starweaver-cli completion zsh
starweaver-cli completion fish
starweaver-cli completion powershell
starweaver-cli completion elvish
```
