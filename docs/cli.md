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
Update available: starweaver X.Y.Z -> A.B.C. Run `starweaver update`.
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
model_cfg = "gpt5_350k"
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

| Model id pattern                        | Protocol                                                                          |
| --------------------------------------- | --------------------------------------------------------------------------------- |
| `openai:<model>`                        | OpenAI Responses                                                                  |
| `openai-responses:<model>`              | OpenAI Responses                                                                  |
| `openai-responses-ws:<model>`           | OpenAI Responses, WebSocket-preferred streaming with HTTP fallback                |
| `openai-chat:<model>`                   | OpenAI Chat Completions                                                           |
| `anthropic:<model>`                     | Anthropic Messages                                                                |
| `claude:<model>`                        | Anthropic Messages                                                                |
| `gemini:<model>`                        | Gemini generateContent                                                            |
| `google:<model>`                        | Gemini generateContent                                                            |
| `google-gla:<model>`                    | Gemini generateContent                                                            |
| `google-cloud:<model>`                  | Google Cloud Gemini                                                               |
| `google-vertex:<model>`                 | Google Cloud Gemini                                                               |
| `<gateway>@openai-responses:<model>`    | gateway-routed OpenAI Responses                                                   |
| `<gateway>@openai-responses-ws:<model>` | gateway-routed OpenAI Responses, WebSocket-preferred streaming with HTTP fallback |
| `<gateway>@openai-chat:<model>`         | gateway-routed OpenAI Chat                                                        |
| `<gateway>@google:<model>`              | gateway-routed Gemini                                                             |
| `<gateway>@google-cloud:<model>`        | gateway-routed Google Cloud                                                       |
| `oauth@codex:<model>`                   | Codex OAuth over OpenAI Responses                                                 |

Deterministic local model ids remain available for tests and offline validation: `local_echo`, `approval_model`, and `deferred_model`.

Use `openai-responses-ws:<model>` only when you want to opt in to Responses WebSocket streaming. Starweaver will prefer WebSocket for streaming requests and automatically fall back to HTTP server-sent events for retryable pre-event WebSocket failures. Plain `openai-responses:<model>` remains HTTP-first because most Responses-compatible endpoints do not support WebSocket streaming.

## Provider configuration

Initialize config, then export provider API keys:

```bash
starweaver-cli config init --global
export OPENAI_API_KEY=...
export ANTHROPIC_API_KEY=...
export GEMINI_API_KEY=...
export GOOGLE_API_KEY=...
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

[providers.google-cloud]
enabled = true
api_key_env = "GOOGLE_API_KEY"
auth_token_env = "GOOGLE_CLOUD_ACCESS_TOKEN"
base_url = "https://aiplatform.googleapis.com"

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

Provider presets treat a `base_url` with no path as a provider API host and insert the provider API root before the endpoint (`/v1` for OpenAI and Anthropic, `/v1beta` for Gemini, `/v1beta1` for Google Cloud). A `base_url` with a path is treated as a gateway mount point, so Starweaver appends only the provider endpoint. Set `endpoint_path` when the gateway needs a fully custom route.

`google:<model>` and `google-gla:<model>` use the Gemini API config from `[providers.gemini]`, so Gemini-compatible gateways can be reached by overriding `base_url` or `endpoint_path`. `google-cloud:<model>` and `google-vertex:<model>` use `[providers.google-cloud]`. With only `api_key_env`, Google Cloud uses Vertex AI Express Mode and sends the key as `x-goog-api-key`. When `project` is set, Starweaver uses `auth_token_env` as a bearer access token and builds `projects/{project}/locations/{location}/publishers/google/models/{model}:generateContent`; `location` defaults to `us-central1` for that project-scoped path.

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
STARWEAVER_GOOGLE_CLOUD_BASE_URL=https://gateway.example/google-cloud
STARWEAVER_OPENAI_API_KEY_ENV=MY_OPENAI_KEY
STARWEAVER_ANTHROPIC_API_KEY_ENV=MY_ANTHROPIC_KEY
STARWEAVER_GEMINI_API_KEY_ENV=MY_GEMINI_KEY
STARWEAVER_GOOGLE_CLOUD_API_KEY_ENV=MY_GOOGLE_KEY
STARWEAVER_GOOGLE_CLOUD_AUTH_TOKEN_ENV=MY_GOOGLE_CLOUD_TOKEN
STARWEAVER_GOOGLE_CLOUD_PROJECT=my-project
STARWEAVER_GOOGLE_CLOUD_LOCATION=us-central1
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
model_cfg = "gpt5_350k"
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

Built-in and config-backed profiles attach the default first-party CLI tool catalog: filesystem, shell, context management, host I/O, task operations, clarifying user input, skills, and CLI control-flow probes. CLI tools execute without approval by default; add explicit tool names, toolset ids such as `context` or `host_io`, or `"*"` to opt back into approval gating. `ask_user_question` is an exception: it always waits for user input because waiting is the tool's purpose, not an optional security policy.

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

Explicitly activate loaded skills by placing consecutive `/skill-name` or `@skill-name` tokens at the start of a prompt. Multiple skills preserve user order; the first is treated as the primary workflow and later skills as supporting workflows. Their complete `SKILL.md` bodies are injected as system guidance for that run, while the user request stored in durable input excludes the prefixes. Built-in and configured slash commands take precedence over a same-named skill. An unknown marker token leaves the prompt unchanged.

```bash
starweaver-cli -p "/lark-cli /building-agent create an agent"
starweaver-cli -p "@lark-cli @building-agent create an agent"
```

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
starweaver-cli session search "literal text" --source run_input --output json
starweaver-cli session search --profile coding --after <opaque-cursor>
starweaver-cli session show <session-id>
starweaver-cli session replay <session-id>
starweaver-cli session replay <session-id> --run <run-id>
starweaver-cli session replay <session-id> --after <sequence>
starweaver-cli session replay <session-id> --output text
```

Run-scoped replay emits the same display JSONL that the initial headless run emitted when `--output display-jsonl` is selected.

Restored headless runs default to `--continuation-mode preserve`, which requires the source and continuation to have the same safe agent materialization fingerprint. TUI-created continuations use the same `preserve` default rather than silently accepting profile, tool, policy, or environment drift. `compatible` permits only an AgentSpec revision with otherwise equivalent model, tool, policy, and environment bindings. `switch` deliberately accepts all reported drift and is available through the explicit headless option. Accepted drift is printed before normal output; materialization evidence is built from a credential-free semantic projection and never hashes arbitrary provider headers, bodies, metadata, or wrapper params.

Session search is read-only and does not select or resume a hit. Its query is case-insensitive literal text, its cursor is opaque and query-bound, and every result preserves coverage warnings. See [Session Search](session-search.md) for projection, pagination, and local display-mirror safety details.

## TUI snapshot and human-in-the-loop policy

The CLI TUI opens a full-screen terminal viewport when stdin and stdout are TTYs. An explicit `--interactive` request fails with a product-level error when either stream is not a TTY. On a fresh machine, the TUI renders a bordered session header card, startup shortcuts, a semantic status area, and a no-border composer below it. The status area includes session-scoped estimated cost and elapsed time and wraps onto additional rows when the terminal is narrow. When height permits, one blank row remains below the composer so the input is visually lifted without reversing the status/input hierarchy. Type a prompt and press Enter to start a background run. Tab completes the selected slash-command candidate; it never changes Enter behavior. `Ctrl+O` inserts a newline. While an agent run is active, Enter sends the draft as steering input. Steering accepted before the final guard is reinjected into the same run; steering that arrives after admission closes is explicitly queued as the next continuation instead of being dropped. Main-agent steering feedback reflows at the current terminal width; delegated subagent steering remains in raw durable evidence but is not rendered in live or restored TUI output. `Ctrl+C` cooperatively cancels only the foreground main-agent run. Repeated interrupts keep that cancellation pending and do not cancel async subagents or task work; explicit host shutdown still performs bounded background cleanup. Runtime stream records update the scrollback while input stays responsive, and scrolling away from the bottom keeps the viewport stable until `Ctrl+L` restores live following. Assistant output uses terminal Markdown rendering: raw assistant Markdown is parsed with `pulldown-cmark`, reflowed at the current viewport width, and styled for headings, lists, blockquotes, fenced code, inline code, emphasis, strong text, links, and horizontal rules.

```bash
sw tui
make cli
```

Interactive slash commands:

| Command            | Action                                              |
| ------------------ | --------------------------------------------------- |
| `/help`            | Print command help into the transcript              |
| `/clear`           | Detach the session and start a fresh context        |
| `/cost`            | Show usage/context summary                          |
| `/display [mode]`  | Switch transcript display mode                      |
| `/model`           | Open the in-TUI model profile selector              |
| `/model <profile>` | Select a model profile directly for future TUI runs |
| `/session`         | Open the in-TUI session selector                    |
| `/session <id>`    | Reload an exact session id or unique id prefix      |
| `/tasks`           | Toggle the expandable task list                     |
| `/goal <task>`     | Run toward a verified goal                          |
| `/paste-image`     | Attach image data from the system clipboard         |
| `/<skill> [task]`  | Explicitly activate a loaded skill; prefixes chain  |
| `@<skill> [task]`  | Alias for explicit skill activation                 |
| `!<command>`       | Run a shell command and show output inline          |

`/clear` succeeds only after the current-session pointer is cleared. It then removes the transcript, workspace prompt recall, attachments, task/subagent panels, HITL state, goal state, session usage/cost aggregation, elapsed-time origin, and other conversation-scoped UI state. It preserves the selected profile, display policy, and process-level provider affinity. Background subagents remain owned by the detached durable session's supervisor scope, so their completions cannot wake the fresh context; reloading that session can resume its pending delivery.

Typing a leading `/` opens bounded completion for built-ins, config-backed commands and aliases, and loaded skills. `Up`/`Down` or `Shift-Tab` moves the selection, Tab completes it, Enter executes it, and Esc dismisses it. `/display`, `/model`, and `/session` also complete known arguments. Near-miss reserved commands return a suggestion without consuming a valid skill name.

The status area keeps question, approval, errors, waiting/running state, and the currently executable action ahead of metadata. It always attempts to show current-session estimated cost, current-run elapsed time, and context usage, wrapping semantic segments instead of silently dropping them on narrow terminals. Run elapsed time uses `Xs`, `XmYYs`, or `XhYYmZZs`, freezes when the run completes, fails, is cancelled, or waits, and resets when the next run starts. The status area remains above the composer; an adaptive bottom spacer lifts the input by one row on normal-height terminals and disappears when compact layouts need the space. Successful WebSocket-to-HTTP model fallback remains available in durable diagnostics but is hidden from the normal TUI; a transport failure that terminates the run remains visible as an error. Task activity expands automatically when the first active task snapshot arrives; `F2` toggles it between the full read-only list and a one-line summary, while `/tasks` remains the command equivalent. The task panel does not capture Esc, arrows, PageUp/PageDown, or Enter and has no detail-selection mode. The minimized summary does not carry a permanent `/tasks` hint. Question and approval modals hide passive task UI. Press `?` from an empty composer or F1 to open transient help without writing to the transcript; `/help` remains the persistent transcript form.

Prompt recall is stored per workspace under `~/.starweaver/tui/prompt-history/`. Files are private, atomically replaced, and bounded to 100 prompts, 16 KiB per prompt, and 256 KiB total. Generated attachment placeholders and attachment-only submissions are not persisted. Use `Ctrl+P`/`Ctrl+N` or empty-composer `Up`/`Down` for sequential recall and `Ctrl+R` for incremental reverse search. Enter accepts a search result without submitting it; Esc preserves the original draft. `/clear` removes the current workspace's recall file as part of its documented context reset.

`!<command>` starts a background process through the selected `EnvironmentProvider`; it does not invoke an untracked host process directly. The TUI stays responsive while the command runs, preserves composer drafts, bounds captured output through the provider, and shows the terminal process snapshot inline. Agent runs and bang-shell processes are mutually exclusive. Press `Ctrl+C` to request process-tree cleanup; a second `Ctrl+C` restores the terminal and waits for bounded cleanup before exit. Cleanup failures include the provider process id and explicitly warn when manual cleanup may be required.

`/goal <task>` submits one runtime goal run. Goal progress is managed by runtime output validation: incomplete output emits `goal_iteration` and retries the model inside the same run, while verified completion or the iteration ceiling emits `goal_complete`. Configure the ceiling with `general.max_goal_iterations` in `config.toml`; the default is `10`.

`/display [mode]` switches the live transcript projection without dropping underlying stream evidence. Modes are `normal`, `concise`, and `debug`. `normal` shows assistant text, thinking, tool calls, and formatted tool returns. The live transcript is event-order preserving: text and thinking are appended only to the active tail segment, non-model events such as tools or context updates close that segment, and projection may only fold adjacent compatible activity. Consecutive thinking segments render without an empty spacer, while transitions to text, tools, or system events retain section separation. `concise` keeps assistant text and thinking streaming normally while semantically summarizing and folding tool activity: adjacent successful read/search/list calls are grouped as `Exploring` / `Explored`, shell commands render as `Running` / `Ran`, file mutations render as one-line edit/write summaries, task tools render as compact task activity, and generic tools render as `Calling` / `Called`. The footer's active tool label uses the same semantic summary line as the concise transcript instead of raw tool payloads. Ordinary successful tool result bodies are suppressed in `concise`; tool failures, approval-required calls, and deferred calls show bounded summary details instead of full result payloads. Only subagent output, summaries, and compactions keep their existing full display style in `concise`; goal events remain visible as important run-state events. Subagent returns are rendered as full Markdown in both `normal` and `concise`. `debug` keeps the full normal transcript plus diagnostic identifiers such as tool call ids, subagent ids, summary categories, and visibility states. Interactive TUI defaults to `concise`. A `/display <mode>` change is saved in the TUI client state and reused by later TUI sessions. Startup mode priority is `--render-mode`, saved TUI client state, then `[tui].render_mode`. Set an explicit startup mode with `sw tui --render-mode concise` or in config:

```toml
[tui]
render_mode = "concise"
```

Use `config get/set tui.render_mode` for scripted changes:

```bash
starweaver-cli config get tui.render_mode
starweaver-cli config set --global tui.render_mode concise
```

Use `Ctrl+V` or `/paste-image` to attach an image currently stored in the system clipboard. The TUI inserts a visible placeholder such as `[Attached image 1: image/png 24KB]` into the composer, but submission strips that generated placeholder and sends the image as inline binary `ContentPart::Binary` content with the first model request. Clipboard image paste currently supports Linux clipboard providers through `wl-paste` on Wayland or `xclip` on X11.

The `/model` selector is embedded in the TUI. Use `Up` / `Down` to move, `Enter` to select, and `Esc` to cancel. The selector shows only user-facing profiles and expands the highlighted profile with its model id, settings preset, config preset, context window, and source so long model ids are easier to inspect. The TUI selected model is client state stored in `~/.starweaver/tui/state.json`. It does not mutate `~/.starweaver/config.toml`; shared config still owns the profile definitions and provider settings. Model selection is only allowed while no run is active.

The `/session` selector uses the same embedded picker style. Use `/session` to view recent local sessions, move with `Up` / `Down`, press `Enter` to reload the highlighted session, or `Esc` to cancel. Use `/session <id>` to reload directly; exact ids and unique id prefixes are supported. Reloading replaces the TUI transcript with persisted display replay, updates the current session pointer, restores the session profile when available, and the next message continues from the loaded history. Session selection is only allowed while no run is active.

Config-backed slash commands are declared in global or project `config.toml` under `[commands.<name>]`. They work in the TUI, headless `run`/`-p`, and JSON-RPC prompt runs. Invoking `/name optional instruction` expands the configured prompt before submission. In the TUI, the transcript shows the expanded prompt directly as the user message, matching the actual prompt sent to the agent. If instruction text is provided and the prompt has no `{instruction}`, `{{instruction}}`, `{args}`, or `{{args}}` placeholder, Starweaver appends `User instruction: <instruction>`. Built-in slash commands such as `/help`, `/model`, `/session`, `/goal`, `/paste-image`, `/clear`, and `/cost` remain reserved and cannot be overridden. Config-backed commands are resolved before explicit skill prefixes, so a command and skill sharing a name invoke the command.

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

| Key                      | Action                                                        |
| ------------------------ | ------------------------------------------------------------- |
| `Enter`                  | Send, steer, or confirm the active modal                      |
| `Tab` / `Shift-Tab`      | Complete or move through slash-command candidates             |
| `Ctrl-O`                 | Insert a newline                                              |
| `Ctrl-P` / `Ctrl-N`      | Browse prompt history                                         |
| `Ctrl-R`                 | Search workspace prompt history                               |
| `Up` / `Down`            | Move visually; recall history from an empty single-line draft |
| `Alt-Up` / `Alt-Down`    | Scroll hidden composer lines                                  |
| `Alt-Left` / `Alt-Right` | Move by word                                                  |
| `Ctrl-A` / `Ctrl-E`      | Move to visual line start/end                                 |
| `PageUp` / `PageDown`    | Scroll transcript                                             |
| Mouse wheel              | Scroll transcript                                             |
| `Ctrl-L`                 | Jump to the live bottom                                       |
| `?` / `F1`               | Open transient contextual help                                |
| `F2`                     | Toggle the read-only task panel                               |
| `Esc`                    | Close the active modal or enter transcript selection          |
| `A` / `Y`                | Approve the displayed durable approval                        |
| `R` / `N`                | Reject the displayed durable approval                         |
| `Ctrl-C`                 | Interrupt activity, clear a draft, or exit when empty         |
| `Ctrl-D`                 | Exit only while idle with an empty composer                   |
| `Ctrl-U`                 | Clear the composer                                            |

When a run waits for approval, the TUI binds the panel to the persisted `ApprovalRecord` and displays approval details while keeping raw durable identifiers in debug-oriented output. Ordinary approvals accept only unmodified `A`/`Y` or `R`/`N`. An `ask_user_question` request with `request.kind = "clarifying_questions"` opens a typed question modal for one to four questions. `Up`/`Down` moves choices, Space toggles multi-select choices, Enter confirms and advances, Tab/Shift-Tab moves between questions, and `E` opens free-form editing with `Ctrl+O` for newlines. The modal renders headers, descriptions, and the selected option preview, then persists canonical question-keyed `ClarifyingQuestionAnswers`. `Esc` reloads the session and reconciles durable state; its refresh target remains available after a transient load failure, including deferred-only waits without an approval panel. The service always verifies the durable session's active run and restore lineage before accepting a prompt. An unresolved `Waiting` source or an active continuation from that source blocks ordinary admission. A prompt submitted during a state-change race remains queued: it starts after external reconciliation, or retries after a pre-start continuation failure. Resolving the final record acquires an exclusive preflight `HitlResumeClaim` before allocating the continuation run, marks it started before model or tool execution, and atomically consumes the waiting source run with continuation evidence before any retained prompt starts. This prevents another TUI or headless client from publishing or executing a competing continuation. Deferred-only waits remain visible until their durable results are completed.

The retained snapshot renderer remains available for scripts, tests, and display-message replay. It uses the same replay source as headless JSONL and session replay. Interactive render-mode projection applies to live TUI sessions; snapshot output replays stored display messages directly.

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

`resume` appends a continuation run from the waiting or head run state. A `Waiting` source uses the exclusive HITL claim path; a terminal head continues normally without creating a resume claim. Pending approval or deferred records still block the waiting path before a continuation run is allocated. Service-managed same-run checkpoint reload, interruption APIs, service transports, workflows, and schedules belong to future service adapters.

## JSON-RPC host service

`starweaver-rpc` is the standalone JSON-RPC 2.0 host product. It owns its configuration, handlers, coordinator, and transports and does not depend on `starweaver-cli`; the CLI/TUI does not act as an RPC frontend.

```bash
starweaver-rpc stdio
starweaver-rpc http --host 127.0.0.1 --port 8765
```

RPC disables `ask_user_question` by default. Enable it only for a frontend that supports both durable HITL handling and dedicated clarifying-question rendering/input:

```toml
[client_capabilities]
hitl = true
clarifying_questions = true
```

`clarifying_questions = true` without `hitl = true` is rejected. When both are true, RPC adds the opt-in `user_input` toolset to every materialized profile; this declaration is a host/client compatibility promise, not a model permission shortcut.

The default `stdio` transport is newline-delimited JSON-RPC over stdin/stdout. It supports responses and live notifications on stdout, with diagnostics on stderr. JSON-RPC frame parsing, standard request validation, error envelopes, replay cursor parsing, and stream payload projection live in `starweaver-rpc-core` so the standalone RPC process and CLI adapter share the same protocol edge.

The `http` transport serves authenticated JSON-RPC request/response calls at `POST /rpc` on a loopback host. Every request, including `/health`, requires `Authorization: Bearer <token>`. Set a token of at least 32 non-whitespace bytes with `STARWEAVER_RPC_TOKEN`, configure `server.http_auth.token_env`/`token_file` in `rpc.toml`, or let first startup generate the private `$state_dir/http-token` file (mode 0600 on Unix). The token itself is never printed.

`server.http_auth.scopes` (or `STARWEAVER_RPC_SCOPES`, comma-separated) grants any subset of `read`, `run`, `approval`, `admin`, and `shutdown`; scopes do not imply one another. HTTP requires JSON bodies to use `Content-Type: application/json`, validates `Host`, and rejects browser `Origin` headers unless listed in `server.http_auth.allowed_origins`. Live notifications are not streamed over unary HTTP; use `run.await`, `run.status`, or `stream.replay`.

Example handshake and client model selection:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"tui"}}}
{"jsonrpc":"2.0","id":2,"method":"model.list","params":{"clientStateScope":"tui"}}
{"jsonrpc":"2.0","id":3,"method":"model.select","params":{"clientStateScope":"tui","profile":"coding"}}
{"jsonrpc":"2.0","id":4,"method":"run.start","params":{"clientStateScope":"tui","prompt":"hello"}}
{"jsonrpc":"2.0","id":5,"method":"stream.subscribe","params":{"sessionId":"session_...","runId":"run_...","subscriptionId":"sub_1"}}
{"jsonrpc":"2.0","id":6,"method":"stream.unsubscribe","params":{"subscriptionId":"sub_1"}}
{"jsonrpc":"2.0","id":7,"method":"shutdown","params":{}}
```

The `stream.subscribe` example is for notification-capable transports such as stdio. Unary HTTP clients should omit live subscriptions and use replay/status polling.

`model.select` writes the selected profile into RPC-owned `$state_dir/state.json`, keyed by the validated `clientStateScope`; legacy `client` is accepted as an alias, but conflicting values are rejected. Omitting the scope consistently uses the `rpc` scope for selection, inspection, and later runs. Updates use locked read-modify-write and atomic replacement, so current-session and other scope selections are preserved. `run.prompt` and `run.start` resolve model profiles in this order: explicit `profile`/`modelProfile`, scoped selection, then the RPC config default.

`run.start` is non-blocking: it returns `sessionId`, `runId`, `status`, and `payloadFormat` after durable run creation and active-run registration. On stdio, subscriptions emit `subscription.ready` only after the subscribe response is flushed, followed by ordered canonical `stream.event` frames and terminal `run.status`; unary HTTP remains replay-only. Each connection permits at most 32 subscriptions and only one subscription per run. A terminal subscription drains all retained pages before closing. Stdio response flush and shutdown waits are deadline-bounded so an unread external host pipe cannot block server shutdown indefinitely.

Use `stream.replay` for persisted output and stdio `stream.subscribe` / `stream.unsubscribe` for connection-owned live tails. `session.replay`, `session.output`, and `run.attach` remain compatibility aliases over the same replay-event cursor family. RPC also wraps CLI/legacy display-only evidence into that outer family, allowing external host to replay CLI runs without cursor migration. The first replay of each run durably records whether that run uses canonical replay events or display messages; later evidence cannot switch cursor spaces between pages.

A CLI-owned run currently commits raw/display evidence atomically when it completes, fails, is cancelled, or enters a durable waiting state. Therefore RPC can replay or subscribe to the complete retained backlog, but it does not observe token/display deltas while that CLI process is still executing the run. Real-time external host observation of an active terminal-owned run requires a future incremental publication contract; clients must not infer that behavior from `stream.subscribe`.

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

The CLI and RPC resolve the same canonical local session database, `$STARWEAVER_CONFIG_DIR/starweaver.sqlite` by default. Without that override, both products use the same shared platform resolver: `$HOME/.starweaver` on Unix and the native Windows profile (`USERPROFILE`, then `HOMEDRIVE` + `HOMEPATH`, with `HOME` compatibility fallback) on Windows. If no platform user profile can be resolved, startup fails and asks for `STARWEAVER_CONFIG_DIR` instead of silently creating a process-relative machine database. This keeps a native external host/RPC process and a terminal CLI on the same database even when their working directories differ. `STARWEAVER_SESSION_DB` overrides it for both products (`STARWEAVER_STORE` remains a compatibility alias).

Legacy workspace databases are never discovered or imported merely by opening the canonical store. Import one user-approved source explicitly and safely retry the same command:

```bash
starweaver-cli storage import-legacy \
  --source /workspace/.starweaver/starweaver.sqlite \
  --workspace /workspace \
  --output json
```

The equivalent typed RPC administration method is `storage.importLegacy` with `sourcePath` and `workspace`; HTTP requires the `admin` scope. Import remains incremental and idempotent, including evidence appended by an older CLI after an earlier import, and records per-session workspace/source provenance. For sessions proven to belong to that exact legacy source, later imports also synchronize same-key mutable session, run, approval, deferred, context, environment, and snapshot records so queued or waiting state can advance to terminal state. Independently created canonical session-ID collisions continue to win, process-control ownership is never imported, and a physically deleted imported session is protected by a source-specific durable import tombstone so a later import cannot recreate it from the legacy database. Project-local state keeps UX pointers and blobs, not an independent session database. CLI implicit continuation, fallback selection, default listing, trimming, and retention are restricted to the normalized current workspace; an invalid project current-session pointer is ignored rather than granting cross-workspace authority. Explicit session IDs remain the cross-workspace boundary.

CLI and RPC runs share the same durable one-active-run admission and fencing generation. CLI renews its lease during execution, retries transient heartbeat failures, and cooperatively cancels before the retained lease can expire if ownership cannot be refreshed. Session deletion is a durable tombstone operation: active runs and live background-subagent ownership fence deletion, while repeated delete requests retry idempotent cleanup of CLI-owned compatibility blobs.

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

`reset` tombstones sessions belonging to the current normalized workspace, removes its project `state.json` and CLI-owned file store, and preserves the shared canonical SQLite database plus sessions from other workspaces. It leaves `config.toml`, `tools.toml`, `mcp.json`, `skills/`, and `subagents/` in place.

## Config

Global Starweaver configuration lives under `~/.starweaver`. `~/.starweaver/config.toml` stores shared defaults, provider settings, and model profile definitions. `~/.starweaver/tools.toml`, `~/.starweaver/mcp.json`, `~/.starweaver/skills`, and `~/.starweaver/subagents` are shared by CLI, TUI, SDK applications, and RPC hosts. The CLI also discovers shared Agent Skills from `~/.agents/skills` by default. Product state is separate: TUI uses `~/.starweaver/tui/state.json`, RPC uses its configured state directory, and project runtime state keeps the current session pointer in `.starweaver/state.json`.

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
