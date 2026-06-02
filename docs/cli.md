# CLI

`starweaver-cli` is the local headless product surface for Starweaver. It runs a prompt through the SDK runtime stream, projects runtime stream records into AGUI-compatible `DisplayMessage` events, persists replay evidence locally, and prints display JSONL by default.

## Profiles

A run uses an AgentSpec profile. The built-in `general` profile uses the deterministic local echo model for offline validation. Pass `--profile` a profile name from `.starweaver/profiles`, `.starweaver/agents`, or the global profile directory, or pass a YAML path directly:

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
  model_id: local_echo
toolsets:
  - environment
```

The CLI registry supplies deterministic local model ids for validation (`local_echo`, `approval_model`, and `deferred_model`) and environment toolsets.

## Headless runs

Run a prompt with either the shorthand or the `run` subcommand:

```bash
starweaver-cli -p "summarize this repository"
starweaver-cli run -p "summarize this repository"
starweaver cli -p "summarize this repository"
sw cli -p "summarize this repository"
```

The default output is one `DisplayMessage` JSON object per line:

```bash
starweaver-cli run -p "hello" --output display-jsonl
```

Use silent mode when automation only needs ids and completion status:

```bash
starweaver-cli run -p "hello" --output silent
```

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
```

Run-scoped replay emits the same display JSONL that the initial headless run emitted.

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

## Local storage

The CLI uses a local SQLite database and file store. Diagnostics print the resolved paths:

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

SQLite tables also retain context states, environment snapshots, stream cursors, checkpoint refs, approvals, deferred tool records, file checksums, and compact run projections.

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

## Launcher commands

`starweaver` and `sw` are product launchers. They dispatch `cli` in-process and keep builtin product commands reserved:

```bash
starweaver version
starweaver doctor
starweaver update
starweaver cli -p "hello"
sw cli -p "hello"
```
