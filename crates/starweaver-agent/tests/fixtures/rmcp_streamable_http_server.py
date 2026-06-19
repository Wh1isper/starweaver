#!/usr/bin/env python3
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def response_for(message):
    method = message.get("method")
    request_id = message.get("id")
    if request_id is None:
        return None
    if method == "initialize":
        return {
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
                "serverInfo": {"name": "starweaver-http-fixture", "version": "0.0.1"},
                "instructions": "Use HTTP fixture MCP tools.",
            },
        }
    if method == "tools/list":
        return {
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "tools": [
                    {
                        "name": "lookup",
                        "description": "Look up HTTP fixture data.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"query": {"type": "string"}},
                            "required": ["query"],
                        },
                    }
                ]
            },
        }
    if method == "resources/list":
        return {
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "resources": [
                    {
                        "uri": "resource://fixture/http-docs",
                        "name": "HTTP Fixture Docs",
                        "description": "HTTP fixture documentation.",
                        "mimeType": "text/markdown",
                    }
                ]
            },
        }
    if method == "prompts/list":
        return {
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "prompts": [
                    {
                        "name": "summarize",
                        "description": "Summarize HTTP fixture docs.",
                        "arguments": [
                            {
                                "name": "topic",
                                "description": "Topic to summarize.",
                                "required": False,
                            }
                        ],
                    }
                ],
            },
        }
    if method == "tools/call":
        params = message.get("params") or {}
        arguments = params.get("arguments") or {}
        return {
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "content": [{"type": "text", "text": "http fixture result"}],
                "structuredContent": {
                    "answer": "http fixture result",
                    "query": arguments.get("query"),
                },
                "isError": False,
            },
        }
    return {
        "jsonrpc": "2.0",
        "id": request_id,
        "error": {"code": -32601, "message": f"unknown method {method}"},
    }


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        body = self.rfile.read(length)
        message = json.loads(body)
        response = response_for(message)
        if response is None:
            self.send_response(202)
            self.end_headers()
            return
        payload = json.dumps(response, separators=(",", ":")).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_DELETE(self):
        self.send_response(202)
        self.end_headers()

    def log_message(self, _format, *_args):
        return


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
host, port = server.server_address
sys.stdout.write(f"http://{host}:{port}/mcp\n")
sys.stdout.flush()
server.serve_forever()
