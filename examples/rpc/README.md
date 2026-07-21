# Standalone RPC example

Run `starweaver-rpc` with this directory's configuration:

```bash
STARWEAVER_RPC_CONFIG=examples/rpc/rpc.toml starweaver-rpc --stdio
```

The MCP path is relative to `rpc.toml`; the stdio server `cwd` is relative to `mcp.json`. Replace the placeholder `docs-mcp` command with an installed MCP server. Keep credentials in process environment variables or an external secret manager rather than in `mcp.json`.
