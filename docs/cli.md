# CLI

`starweaver-cli` is the local headless product surface for Starweaver. It runs a prompt through the SDK runtime stream, projects runtime records into AGUI-compatible `DisplayMessage` events, persists replay evidence locally, and can print either human-readable text or display JSONL.

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
starweaver cli -p "hello"
sw cli -p "hello"
```

`starweaver cli ...` dispatches to the CLI product, and `starweaver <command> ...` dispatches to `starweaver-<command> ...` for future command families. The launcher resolves command binaries from the install directory first, then `PATH`.

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

Pass `--profile` a built-in name, a profile name from `.starweaver/profiles`, `.starweaver/agents`, or the global profile directory, or a YAML path directly:

```bash
starweaver-cli run -p "hello" --profile general
starweaver-cli run -p "hello" --profile .starweaver/profiles/research.yaml
```

A minimal profile looks like this:

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

Provider-backed model ids use these prefixes:

| Prefix                     | Protocol                |
| -------------------------- | ----------------------- |
| `openai:<model>`           | OpenAI Responses        |
| `openai-responses:<model>` | OpenAI Responses        |
| `openai-chat:<model>`      | OpenAI Chat Completions |
| `anthropic:<model>`        | Anthropic Messages      |
| `claude:<model>`           | Anthropic Messages      |
| `gemini:<model>`           | Gemini generateContent  |

Deterministic local model ids remain available for tests and offline validation: `local_echo`, `approval_model`, and `deferred_model`.

## Provider configuration

Initialize config, then export provider API keys:

```bash
starweaver-cli config init --global
export OPENAI_API_KEY=...
export ANTHROPIC_API_KEY=...
export GEMINI_API_KEY=...
```

Provider config stores environment variable names and gateway URLs. It avoids writing raw API keys into `config.toml`. Configuration examples live under `examples/cli/` and can be validated with `make cli-examples-check`.

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
```

Useful config commands:

```bash
starweaver-cli config get providers.openai.ready
starweaver-cli config get providers.openai.base_url
starweaver-cli config set --global providers.openai.base_url https://gateway.example/v1
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

## Human-in-the-loop policy

Headless HITL defaults to `deny`. The current policy is stored in display message metadata for auditability. Tool-return control flow with `approval_required` or `call_deferred` is projected into `APPROVAL_REQUESTED` and `APPROVAL_RESOLVED` display messages, persisted as approval/deferred records, and can place the run in `waiting` or `failed` status depending on policy:

```bash
starweaver-cli -p "hello" --hitl deny
starweaver-cli -p "hello" --hitl defer
starweaver-cli -p "hello" --hitl fail
starweaver-cli -p "hello" --hitl prompt
```

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

## Config

Resolution order is built-in defaults, global `config.toml`, project `config.toml`, `tools.toml` and `mcp.json` metadata, environment variables, then command flags. Supported environment overrides include `STARWEAVER_PROFILE`, `STARWEAVER_PROFILE_PATHS`, `STARWEAVER_SESSION_DB`, `STARWEAVER_FILE_STORE`, `STARWEAVER_WORKSPACE_ROOT`, `STARWEAVER_ENV_PROVIDER`, `STARWEAVER_FILES_POLICY`, `STARWEAVER_SHELL_ENABLED`, `STARWEAVER_OUTPUT`, `STARWEAVER_HITL`, `STARWEAVER_UPDATE_CHANNEL`, and `STARWEAVER_NO_AUTO_TRIM`.

Get resolved config values and persist project or global config overrides:

```bash
starweaver-cli config get trim.current_session_keep_recent_runs
starweaver-cli config get metadata.tools
starweaver-cli config set trim.current_session_keep_recent_runs 10
starweaver-cli config set --global general.default_profile general
```

## Shell completions

Generate shell completion scripts from the same clap schema used by the CLI:

```bash
starweaver-cli completion bash
starweaver-cli completion zsh
starweaver-cli completion fish
starweaver-cli completion powershell
starweaver-cli completion elvish
```
