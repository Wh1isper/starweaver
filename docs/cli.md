# CLI

`starweaver-cli` is the local headless product surface for Starweaver. It runs prompts through the SDK runtime stream, projects runtime records into AGUI-compatible `DisplayMessage` events, persists replay evidence locally, and prints either human-readable text or display JSONL.

## Launchers

Installations include three CLI entry points:

| Binary           | Role                         |
| ---------------- | ---------------------------- |
| `starweaver`     | product launcher             |
| `sw`             | short alias for `starweaver` |
| `starweaver-cli` | local agent CLI command      |

Launcher dispatch keeps product commands easy to discover:

```bash
starweaver version
starweaver doctor
starweaver update
starweaver update cli
starweaver update claw
starweaver cli
starweaver cli -p "hello"
sw cli
sw cli -p "hello"
```

`starweaver cli ...` dispatches to the CLI product. `starweaver <command> ...` dispatches to `starweaver-<command> ...` for future command families. The launcher resolves command binaries from the install directory first, then `PATH`. From a checkout, `make cli` runs the same product path as `sw cli`: no arguments render the TUI welcome/snapshot, and prompt arguments can be passed with `make cli -- -p "hello"`.

## Updates

`starweaver update` updates the installed CLI component through the installer workflow. `starweaver update claw` updates the Claw service binary. The direct CLI command supports the same target selection:

```bash
starweaver update
starweaver update cli
starweaver update claw
starweaver-cli update
starweaver-cli update claw
```

The CLI also performs a short background release lookup and caches the result in `~/.starweaver/update-check.json`. Human-readable commands append a compact hint when the cache reports a newer release:

```text
Update available: starweaver 0.1.0 -> 0.2.0. Run `starweaver update`.
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
```

Gateway model ids use the gateway name as a provider config key. `homelab@openai-responses:gpt-5` reads `[providers.homelab]`, falls back to `HOMELAB_API_KEY` for credentials, and can explicitly select gateway-specific request mappings:

```toml
[providers.homelab]
base_url = "https://gateway.example/v1"
max_tokens_parameter = "omit"
```

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

Auth inspection commands read and update the local auth store without printing secrets:

```bash
starweaver-cli auth status
starweaver-cli auth status codex
starweaver-cli auth logout codex
```

## Tools, MCP, skills, and subagents

Built-in and config-backed profiles attach the default first-party CLI tool catalog: filesystem, shell, host operations, task operations, skills, and CLI control-flow probes. The tool policy file marks approval-gated tools and toolsets:

```toml
[tools]
need_approval = ["shell", "write", "edit", "multi_edit", "delete", "move"]
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

Skills are loaded from `SKILL.md` packages in configured directories and exposed through the `skills` toolset. Subagent markdown files are loaded from configured directories and registered in the CLI AgentSpec registry.

```toml
[skills]
dirs = ["~/.starweaver/skills"]
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
starweaver cli -p "summarize this repository"
sw cli -p "summarize this repository"
```

Output modes:

| Mode            | Flag                     | Output contract                                           |
| --------------- | ------------------------ | --------------------------------------------------------- |
| `text`          | `--output text`          | assistant text and compact tool/status lines              |
| `display-jsonl` | `--output display-jsonl` | one AGUI-compatible `DisplayMessage` JSON object per line |
| `silent`        | `--output silent`        | persisted records and compact final status                |

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

The CLI TUI MVP renders a retained terminal snapshot from persisted `DisplayMessage` records. It uses the same replay source as headless JSONL and session replay. On a fresh machine, it renders a welcome/setup state and waits until the first prompt run before creating runtime session state:

```bash
starweaver-cli tui
starweaver-cli tui --session <session-id>
starweaver-cli tui --session <session-id> --run <run-id>
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

`resume` appends a continuation run from the waiting or head run state. Claw remains the owner for service-managed same-run checkpoint reload, interruption APIs, SSE transports, workflows, and schedules.

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

SQLite tables retain context states, environment snapshots, stream cursors, checkpoint refs, approvals, deferred tool records, file checksums, and compact run projections.

Trim retained run evidence with dry-run and age-filter support:

```bash
starweaver-cli session trim --session <session-id> --keep-runs 20 --dry-run
starweaver-cli session trim --session <session-id> --keep-runs 20 --older-than 7d
starweaver-cli session trim --all --keep-runs 20 --older-than 30d
```

Reset runtime state while preserving configuration:

```bash
starweaver-cli reset --yes
sw cli reset --yes
```

`reset` removes the resolved SQLite database, `state.json`, and file store. It leaves `config.toml`, `tools.toml`, `mcp.json`, `skills/`, and `subagents/` in place.

## Config

Resolution order is built-in defaults, global `config.toml`, project `config.toml`, `tools.toml` and `mcp.json` metadata, environment variables, then command flags. Supported environment overrides include `STARWEAVER_PROFILE`, `STARWEAVER_PROFILE_PATHS`, `STARWEAVER_SKILL_DIRS`, `STARWEAVER_SUBAGENT_DIRS`, `STARWEAVER_DISABLED_SUBAGENTS`, `STARWEAVER_SESSION_DB`, `STARWEAVER_FILE_STORE`, `STARWEAVER_WORKSPACE_ROOT`, `STARWEAVER_ENV_PROVIDER`, `STARWEAVER_FILES_POLICY`, `STARWEAVER_SHELL_ENABLED`, `STARWEAVER_OUTPUT`, `STARWEAVER_HITL`, `STARWEAVER_UPDATE_CHANNEL`, `STARWEAVER_UPDATE_CHECK`, `STARWEAVER_OAUTH_AUTH_FILE`, and `STARWEAVER_NO_AUTO_TRIM`.

Get resolved config values and persist project or global config overrides:

```bash
starweaver-cli config get trim.current_session_keep_recent_runs
starweaver-cli config get metadata.tools
starweaver-cli config get metadata.compatibility
starweaver-cli config set trim.current_session_keep_recent_runs 10
starweaver-cli config set --global general.default_profile general
```

Starweaver preserves recognized display, browser, subagent, command, security, and max-request fields in compatibility metadata so configuration audits can map those sections into first-class Starweaver settings over time.

## Shell completions

Generate shell completion scripts from the same clap schema used by the CLI:

```bash
starweaver-cli completion bash
starweaver-cli completion zsh
starweaver-cli completion fish
starweaver-cli completion powershell
starweaver-cli completion elvish
```
