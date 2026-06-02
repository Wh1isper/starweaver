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

OAuth-backed Codex models use `oauth@codex:<model>` and read credentials from `~/.starweaver/auth.json` or `STARWEAVER_OAUTH_AUTH_FILE`:

```bash
starweaver-cli run -p "hello" --profile codex
```

Skills and subagents are loaded from configured directories:

```toml
[skills]
dirs = ["~/.starweaver/skills"]

[subagents]
additional_dirs = ["~/.starweaver/subagents"]
disabled = []
```

Update checks run through a short background GitHub release lookup with a local cache. Disable checks for scripted environments:

```bash
export STARWEAVER_UPDATE_CHECK=0
```
