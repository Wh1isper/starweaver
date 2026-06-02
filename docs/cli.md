# CLI

`starweaver-cli` is the local headless product surface for Starweaver. It runs a prompt through the SDK runtime stream, projects runtime stream records into AGUI-compatible `DisplayMessage` events, persists replay evidence locally, and prints display JSONL by default.

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

Display messages are persisted and used as the replay source:

```bash
starweaver-cli session list
starweaver-cli session show <session-id>
starweaver-cli session replay <session-id>
starweaver-cli session replay <session-id> --run <run-id>
starweaver-cli session replay <session-id> --after <sequence>
```

Run-scoped replay emits the same display JSONL that the initial headless run emitted.

## Human-in-the-loop policy

Headless HITL defaults to `deny`. The current policy is stored in display message metadata for auditability:

```bash
starweaver-cli -p "hello" --hitl deny
starweaver-cli -p "hello" --hitl defer
starweaver-cli -p "hello" --hitl fail
starweaver-cli -p "hello" --hitl prompt
```

## Local storage

The CLI uses a local SQLite database and file store. Diagnostics print the resolved paths:

```bash
starweaver-cli diagnostics
```

Each completed run stores raw runtime stream evidence and compact display archives under the configured file store:

```text
sessions/<session-id>/runs/<run-id>/raw.stream.json
sessions/<session-id>/runs/<run-id>/display.compact.json
```

Trim retained run evidence with dry-run support:

```bash
starweaver-cli session trim --session <session-id> --keep-runs 20 --dry-run
starweaver-cli session trim --session <session-id> --keep-runs 20
```

## Config

Get resolved config values and persist project config overrides:

```bash
starweaver-cli config get trim.current_session_keep_recent_runs
starweaver-cli config set trim.current_session_keep_recent_runs 10
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
