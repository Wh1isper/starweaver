#!/usr/bin/env python3
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


state = {
    "current_session": None,
    "expired_once": False,
    "initialize_count": 0,
}


def initialize_response(request_id):
    state["initialize_count"] += 1
    state["current_session"] = f"session-{state['initialize_count']}"
    return {
        "jsonrpc": "2.0",
        "id": request_id,
        "result": {
            "protocolVersion": "2025-11-25",
            "capabilities": {
                "tools": {},
                "resources": {},
                "prompts": {},
            },
            "serverInfo": {"name": "starweaver-reconnect-fixture", "version": "0.1.0"},
            "instructions": "Use reconnect fixture MCP tools.",
        },
    }


def response_for(message):
    method = message.get("method")
    request_id = message.get("id")
    if request_id is None:
        return None
    if method == "initialize":
        return initialize_response(request_id)
    if method == "tools/list":
        return {
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "tools": [
                    {
                        "name": "lookup",
                        "description": "Look up reconnect fixture data.",
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
        return {"jsonrpc": "2.0", "id": request_id, "result": {"resources": []}}
    if method == "prompts/list":
        return {"jsonrpc": "2.0", "id": request_id, "result": {"prompts": []}}
    if method == "tools/call":
        params = message.get("params") or {}
        arguments = params.get("arguments") or {}
        return {
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "content": [{"type": "text", "text": "reconnected result"}],
                "structuredContent": {
                    "answer": "reconnected result",
                    "query": arguments.get("query"),
                    "session": state["current_session"],
                    "initialize_count": state["initialize_count"],
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
        method = message.get("method")
        session = self.headers.get("Mcp-Session-Id")

        if method != "initialize" and message.get("id") is not None:
            if session != state["current_session"]:
                self.send_response(404)
                self.end_headers()
                return
            if method == "tools/call" and not state["expired_once"]:
                state["expired_once"] = True
                state["current_session"] = None
                self.send_response(404)
                self.end_headers()
                return

        response = response_for(message)
        if response is None:
            self.send_response(202)
            self.end_headers()
            return
        payload = json.dumps(response, separators=(",", ":")).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        if state["current_session"] is not None:
            self.send_header("Mcp-Session-Id", state["current_session"])
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self):
        self.send_response(405)
        self.end_headers()

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
