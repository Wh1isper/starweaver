# Tools, Toolsets, HITL, and MCP

## Scope

This document tracks only remaining tool, toolset, MCP, and HITL gaps.

## Tool Metadata Evidence

- Rust-native builders and typed helper APIs are the primary tool registration surface.
- `ToolDefinition.metadata` should be extended only for additional provider-neutral policy contracts that need stable SDK helpers.

## HITL And Deferred Resume Evidence

- SDK durable-store resume covers function-tool approvals and deferred tool results.
- Host-backed live MCP durable-store resume covers approval records, deferred records, resumed execution, and resumed model history through `runtime_durable_store_resumes_live_mcp_approval_and_deferred_records`.
- Protocol-level `rmcp` stdio durable-store resume covers approval records, required-task deferred records, resumed execution, and resumed model history through `runtime_durable_store_resumes_rmcp_stdio_approval_and_deferred_records`.

## MCP Gaps

- The older standalone SSE MCP transport is not exposed by `rmcp` 1.7. A product that still needs standalone SSE must provide a separate host adapter instead of extending `RmcpLiveMcpClient`.
- MCP roots, logging, completions, notifications, and host-owned task workers remain product-level host contracts. The current alignment target covers tool discovery/call, resource/prompt discovery, required-task deferred records, lifecycle evidence, and streamable HTTP session reinitialization.

Required direction:

- Add standalone SSE only through a host adapter if a product explicitly adopts that older protocol.
- Add roots, logging, completions, notifications, and task-worker contracts only with host ownership, UI, replay, and security policies.

Current evidence:

- Live MCP initialization and cleanup emit lifecycle reports with MCP identity metadata.
- MCP resource, prompt, sampling, and subscription discovery is represented in `McpToolsetConfig` and live lifecycle metadata.
- `rmcp_stdio_client_discovers_executes_and_closes_fixture_server` proves the built-in `RmcpLiveMcpClient` discovers tools, resources, prompts, and server instructions over stdio, executes a tool through `rmcp`, preserves MCP metadata on the resulting `ToolResult`, and closes the child-process transport.
- `rmcp_streamable_http_client_discovers_executes_and_closes_fixture_server` proves the built-in `RmcpLiveMcpClient` discovers and executes tools over streamable HTTP while preserving MCP metadata.
- `rmcp_streamable_http_client_reinitializes_after_expired_session` proves the streamable HTTP path transparently reinitializes an expired MCP session and retries the original tool call through `rmcp`.
- `runtime_durable_store_resumes_rmcp_stdio_approval_and_deferred_records` proves protocol-level `rmcp` stdio approval and required-task deferred records persist, resume, and produce resumed model history.
- Live MCP discovered tool calls execute through `LiveMcpClient::call_tool` when the host provides a call implementation, and successful `ToolResult` values carry MCP server, transport, and tool metadata.
- Discovery-only live MCP clients still defer discovered tool calls with `mcp_tool_call` metadata including server id, tool name, and arguments.
- Approval-required live MCP tool calls persist MCP tool metadata in durable approval records, execute after approval, and preserve MCP result metadata in resumed model history.
- Discovery-only deferred live MCP tool calls persist `mcp_tool_call` durable records and resume with host-supplied deferred results.
