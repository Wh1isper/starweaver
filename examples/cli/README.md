# CLI configuration examples

These examples show current Starweaver CLI config shapes for global defaults, project defaults, provider gateways, tool metadata, MCP metadata, skills, subagents, and model profiles.

Use them as starting points:

```bash
mkdir -p ~/.starweaver .starweaver
cp examples/cli/global-config.toml ~/.starweaver/config.toml
cp examples/cli/project-config.toml .starweaver/config.toml
```

Validate examples from the repository root:

```bash
make cli-examples-check
```

Provider examples store environment variable names. Export API keys in the shell:

```bash
export OPENAI_API_KEY=...
export ANTHROPIC_API_KEY=...
export GEMINI_API_KEY=...
export GOOGLE_API_KEY=...
```

Gateway model ids use the form `<gateway>@<protocol>:<model>`. Starweaver resolves gateway config from `[providers.<gateway>]` and falls back to uppercased environment variables for credentials:

```toml
[providers.homelab]
base_url = "https://gateway.example/v1"
max_tokens_parameter = "omit"
```

```bash
export HOMELAB_API_KEY=...
starweaver-cli run -p "hello" --profile gateway
```

OAuth-backed Codex models use `oauth@codex:<model>` and read credentials from `~/.starweaver/auth.json` or `STARWEAVER_OAUTH_AUTH_FILE`. The `[oauth_refresh]` section enables proactive background refresh for configured OAuth profiles during CLI, TUI, and RPC runs.

```bash
starweaver-cli auth status codex
starweaver-cli config get oauth_refresh.enabled
starweaver-cli run -p "hello" --profile codex
```

Use setup to create the local project catalog files:

```bash
starweaver-cli setup --project
starweaver-cli tools list
starweaver-cli mcp list
```

Skills and subagents are loaded from configured directories. Default skill discovery includes `~/.starweaver/skills`, shared Agent Skills in `~/.agents/skills`, and project `.starweaver/skills`:

```toml
[skills]
additional_dirs = ["./custom-skills"]

[subagents]
additional_dirs = ["~/.starweaver/subagents"]
disabled = []
```

Update checks run through a short background GitHub release lookup with a local cache. Disable checks for scripted environments:

```bash
export STARWEAVER_UPDATE_CHECK=0
```
