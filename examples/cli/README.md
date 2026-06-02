# CLI configuration examples

These examples show current Starweaver CLI config shapes for global defaults, project defaults, provider gateways, tool metadata, and MCP metadata.

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
