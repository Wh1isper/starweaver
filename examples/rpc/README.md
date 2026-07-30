# Standalone RPC example

Run `starweaver-rpc` with this directory's configuration:

```bash
STARWEAVER_RPC_CONFIG=examples/rpc/rpc.toml starweaver-rpc --stdio
```

The MCP path is relative to `rpc.toml`; the stdio server `cwd` is relative to `mcp.json`. Replace the placeholder `docs-mcp` command with an installed MCP server. Keep credentials in process environment variables or an external secret manager rather than in `mcp.json`.

Computer Use remains disabled in the example. On an attended macOS host, enable `[computer_use]` and grant the initiating transport observation authority. Enabling the server automatically injects the Toolset into every effective profile, including `macos_observer`; profiles do not list it themselves. Generic RPC `run` authority still grants nothing, and RPC observes the host machine rather than the client's desktop. See [Computer Use](../../docs/computer-use.md).
