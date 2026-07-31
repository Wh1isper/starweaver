# Standalone RPC example

Run `starweaver-rpc` with this directory's configuration:

```bash
STARWEAVER_RPC_CONFIG=examples/rpc/rpc.toml starweaver-rpc --stdio
```

The MCP path is relative to `rpc.toml`; the stdio server `cwd` is relative to `mcp.json`. Replace the placeholder `docs-mcp` command with an installed MCP server. Keep credentials in process environment variables or an external secret manager rather than in `mcp.json`.

Computer Use remains disabled in the example. On an attended macOS host, enable `[computer_use]`; otherwise authorized callers/runs then receive the full observe, pointer, and keyboard family. Enabling the server automatically injects the Toolset into every effective profile, including `macos_observer`; profiles do not list it themselves. There is no transport-specific observation authority or per-input principal, and maintained RPC input adds no per-call HITL approval. Native Screen Recording and Accessibility/post-event permission remains required. RPC operates the host machine rather than the client's desktop. See [Computer Use](../../docs/computer-use.md).
