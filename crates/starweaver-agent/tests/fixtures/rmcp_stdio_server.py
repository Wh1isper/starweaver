#!/usr/bin/env python3
import json
import sys


def send(message):
    sys.stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
    sys.stdout.flush()


for line in sys.stdin:
    if not line.strip():
        continue
    message = json.loads(line)
    method = message.get("method")
    request_id = message.get("id")
    if method == "initialize":
        send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {
                        "tools": {},
                        "resources": {},
                        "prompts": {},
                        "sampling": {},
                    },
                    "serverInfo": {"name": "starweaver-fixture", "version": "0.0.1"},
                    "instructions": "Use fixture MCP tools.",
                },
            }
        )
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "tools": [
                        {
                            "name": "lookup",
                            "description": "Look up fixture data.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {"query": {"type": "string"}},
                                "required": ["query"],
                            },
                        },
                        {
                            "name": "dangerous",
                            "description": "Execute an approval-gated fixture action.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {"path": {"type": "string"}},
                                "required": ["path"],
                            },
                        },
                        {
                            "name": "slow",
                            "description": "Start a deferred fixture task.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {"job": {"type": "string"}},
                                "required": ["job"],
                            },
                            "execution": {"taskSupport": "required"},
                        },
                    ]
                },
            }
        )
    elif method == "resources/list":
        send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "resources": [
                        {
                            "uri": "resource://fixture/docs",
                            "name": "Fixture Docs",
                            "description": "Fixture documentation.",
                            "mimeType": "text/markdown",
                        }
                    ]
                },
            }
        )
    elif method == "prompts/list":
        send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "prompts": [
                        {
                            "name": "summarize",
                            "description": "Summarize fixture docs.",
                            "arguments": [
                                {
                                    "name": "topic",
                                    "description": "Topic to summarize.",
                                    "required": False,
                                }
                            ],
                        }
                    ]
                },
            }
        )
    elif method == "tools/call":
        params = message.get("params") or {}
        name = params.get("name")
        arguments = params.get("arguments") or {}
        if name == "dangerous":
            structured = {
                "executed": True,
                "path": arguments.get("path"),
            }
            text = "dangerous executed"
        elif name == "slow":
            structured = {
                "answer": "slow task should be deferred",
                "job": arguments.get("job"),
            }
            text = "slow task should be deferred"
        else:
            structured = {
                "answer": "fixture result",
                "query": arguments.get("query"),
            }
            text = "fixture result"
        send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "content": [{"type": "text", "text": text}],
                    "structuredContent": structured,
                    "isError": False,
                },
            }
        )
    else:
        if request_id is not None:
            send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {"code": -32601, "message": f"unknown method {method}"},
                }
            )
