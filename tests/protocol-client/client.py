#!/usr/bin/env python3
"""Bundle-only independent client proof for the public starweaver.host contract."""

from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, NoReturn

FEATURES = [
    "clarifications",
    "diagnostics.safe",
    "environment.attachments",
    "environment.mounts",
    "events.replay",
    "events.subscribe",
    "hitl",
    "host.shutdown",
    "profiles",
    "runs",
    "session.fork",
    "session.search",
    "sessions",
    "steering",
]


def fail(message: str) -> NoReturn:
    raise RuntimeError(message)


def load_bundle(path: Path) -> tuple[dict[str, Any], dict[str, Any]]:
    bundle = json.loads(path.read_text(encoding="utf-8"))
    if bundle.get("openrpc") != "1.4.0":
        fail("public bundle is not OpenRPC 1.4.0")
    protocol = bundle.get("x-starweaver-protocol")
    if not isinstance(protocol, dict) or protocol.get("name") != "starweaver.host":
        fail("public bundle has no starweaver.host identity")
    methods = {entry.get("name") for entry in bundle.get("methods", [])}
    required = {
        "initialize",
        "session.list",
        "session.create",
        "session.get",
        "run.start",
        "run.status",
        "events.replay",
        "events.subscribe",
        "events.unsubscribe",
        "shutdown",
    }
    if not required.issubset(methods):
        fail(f"public bundle is missing methods: {sorted(required - methods)}")
    return bundle, protocol


def request(request_id: str, method: str, params: dict[str, Any]) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}


def initialize(protocol: dict[str, Any], request_id: str) -> dict[str, Any]:
    return request(
        request_id,
        "initialize",
        {
            "clientInfo": {"name": "independent-python-proof", "version": "1.0.0"},
            "protocol": protocol,
            "requiredFeatures": [],
            "supportedFeatures": FEATURES,
        },
    )


class StdioClient:
    def __init__(self, process: subprocess.Popen[str]) -> None:
        self.process = process
        self.notifications: list[dict[str, Any]] = []

    def call(self, frame: dict[str, Any]) -> dict[str, Any]:
        if self.process.stdin is None or self.process.stdout is None:
            fail("stdio pipes are unavailable")
        self.process.stdin.write(json.dumps(frame, separators=(",", ":")) + "\n")
        self.process.stdin.flush()
        while True:
            line = self.process.stdout.readline()
            if not line:
                stderr = self.process.stderr.read() if self.process.stderr else ""
                fail(f"stdio host exited before response: {stderr}")
            decoded = json.loads(line)
            if "id" not in decoded:
                self.notifications.append(decoded)
                continue
            if decoded.get("id") != frame["id"]:
                fail("stdio response correlation failed")
            return decoded

    def next_notification(self, method: str, deadline: float) -> dict[str, Any]:
        while time.monotonic() < deadline:
            for index, notification in enumerate(self.notifications):
                if notification.get("method") == method:
                    return self.notifications.pop(index)
            if self.process.stdout is None:
                fail("stdio stdout is unavailable")
            line = self.process.stdout.readline()
            if not line:
                fail("stdio host exited while waiting for notification")
            decoded = json.loads(line)
            if "id" in decoded:
                fail("unexpected response while waiting for notification")
            self.notifications.append(decoded)
        fail(f"timed out waiting for {method}")


def expect_result(response: dict[str, Any], context: str) -> dict[str, Any]:
    result = response.get("result")
    if not isinstance(result, dict):
        fail(f"{context} failed: {response}")
    return result


def expect_typed_error(response: dict[str, Any], expected_kind: str) -> None:
    error = response.get("error")
    if not isinstance(error, dict) or not isinstance(error.get("code"), int):
        fail(f"expected typed error, got {response}")
    data = error.get("data")
    if not isinstance(data, dict) or data.get("kind") != expected_kind:
        fail(f"expected {expected_kind} error data, got {response}")
    for field in ("retryable", "reconciliationRequired"):
        if not isinstance(data.get(field), bool):
            fail(f"typed error omitted {field}")


def event_view(session_id: str) -> dict[str, Any]:
    return {
        "scope": {"kind": "session", "sessionId": session_id},
        "profile": "operations.v1",
        "optionalFeatures": [],
    }


def run_stdio(
    rpc: Path,
    protocol: dict[str, Any],
    config_dir: Path,
    workspace: Path,
    store: Path,
) -> None:
    environment = os.environ.copy()
    environment["STARWEAVER_CONFIG_DIR"] = str(config_dir)
    process = subprocess.Popen(
        [str(rpc), "--store", str(store), "stdio"],
        cwd=workspace,
        env=environment,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        bufsize=1,
    )
    client = StdioClient(process)
    try:
        initialized = expect_result(client.call(initialize(protocol, "independent-init")), "initialize")
        if initialized.get("protocol") != protocol:
            fail("stdio initialize identity differs from the public bundle")

        expect_result(
            client.call(request("session-list", "session.list", {"limit": 10})),
            "session.list",
        )
        missing = client.call(
            request(
                "session-missing",
                "session.get",
                {"sessionId": "missing-independent-session", "runLimit": 10},
            )
        )
        expect_typed_error(missing, "not_found")

        created = expect_result(
            client.call(
                request(
                    "session-create",
                    "session.create",
                    {
                        "idempotencyKey": "independent-session-create",
                        "deferredTools": [],
                        "profile": "general",
                        "title": "Independent protocol proof",
                    },
                )
            ),
            "session.create",
        )
        session_id = created["session"]["sessionId"]
        expect_result(
            client.call(
                request(
                    "session-get",
                    "session.get",
                    {"sessionId": session_id, "runLimit": 10},
                )
            ),
            "session.get",
        )

        view = event_view(session_id)
        replay = expect_result(
            client.call(
                request("replay-before", "events.replay", {"view": view, "limit": 100})
            ),
            "events.replay",
        )
        cursor = replay["nextCursor"]
        subscription = expect_result(
            client.call(
                request(
                    "subscribe",
                    "events.subscribe",
                    {"view": view, "cursor": cursor},
                )
            ),
            "events.subscribe",
        )
        subscription_id = subscription["subscriptionId"]

        started = expect_result(
            client.call(
                request(
                    "run-start",
                    "run.start",
                    {
                        "sessionId": session_id,
                        "input": [{"kind": "text", "text": "independent client proof"}],
                        "idempotencyKey": "independent-run-start",
                        "continuationMode": "preserve",
                        "environmentAttachments": [],
                        "profile": "general",
                    },
                )
            ),
            "run.start",
        )
        run_id = started["run"]["runId"]
        notification = client.next_notification("host.event", time.monotonic() + 20)
        params = notification.get("params", {})
        if params.get("subscriptionId") != subscription_id:
            fail("host.event used the wrong subscription identity")
        event_cursor = params["delivery"]["cursor"]

        terminal = None
        for index in range(100):
            status = expect_result(
                client.call(
                    request(
                        f"run-status-{index}",
                        "run.status",
                        {"sessionId": session_id, "runId": run_id},
                    )
                ),
                "run.status",
            )
            terminal = status["run"]["status"]
            if terminal in {"completed", "failed", "cancelled"}:
                break
            time.sleep(0.05)
        if terminal != "completed":
            fail(f"independent run did not complete: {terminal}")

        expect_result(
            client.call(
                request(
                    "unsubscribe",
                    "events.unsubscribe",
                    {"subscriptionId": subscription_id},
                )
            ),
            "events.unsubscribe",
        )
        reconnect = expect_result(
            client.call(
                request(
                    "replay-reconnect",
                    "events.replay",
                    {"view": view, "limit": 100, "cursor": event_cursor},
                )
            ),
            "events.replay reconnect",
        )
        if not isinstance(reconnect.get("deliveries"), list):
            fail("reconnect replay returned no delivery list")
        expect_result(
            client.call(request("shutdown", "shutdown", {"deadlineMs": 2_000})),
            "shutdown",
        )
        process.wait(timeout=20)
        if process.returncode != 0:
            fail(f"stdio host exited with {process.returncode}")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=10)


def reserve_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def http_call(port: int, token: str, frame: dict[str, Any]) -> dict[str, Any]:
    body = json.dumps(frame, separators=(",", ":")).encode()
    request_value = urllib.request.Request(
        f"http://127.0.0.1:{port}/rpc",
        data=body,
        method="POST",
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "Host": f"127.0.0.1:{port}",
        },
    )
    with urllib.request.urlopen(request_value, timeout=5) as response:
        return json.loads(response.read())


def run_http(
    rpc: Path,
    protocol: dict[str, Any],
    config_dir: Path,
    workspace: Path,
    store: Path,
) -> None:
    token = "independent-http-token-0123456789abcdef0123456789abcdef"
    port = reserve_port()
    environment = os.environ.copy()
    environment.update(
        {
            "STARWEAVER_CONFIG_DIR": str(config_dir),
            "STARWEAVER_INDEPENDENT_CLIENT_TOKEN": token,
        }
    )
    process = subprocess.Popen(
        [
            str(rpc),
            "--store",
            str(store),
            "http",
            "--host",
            "127.0.0.1",
            "--port",
            str(port),
        ],
        cwd=workspace,
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
    )
    try:
        init = initialize(protocol, "independent-http-init")
        deadline = time.monotonic() + 20
        while True:
            try:
                response = http_call(port, token, init)
                break
            except (OSError, urllib.error.URLError):
                if process.poll() is not None:
                    fail(f"HTTP host exited before readiness: {process.stderr.read()}")
                if time.monotonic() >= deadline:
                    fail("HTTP host did not become ready")
                time.sleep(0.05)
        initialized = expect_result(response, "HTTP initialize")
        if initialized.get("protocol") != protocol:
            fail("HTTP initialize identity differs from the public bundle")
        expect_result(
            http_call(port, token, request("http-list", "session.list", {"limit": 1})),
            "HTTP session.list",
        )
        expect_result(
            http_call(port, token, request("http-shutdown", "shutdown", {"deadlineMs": 2_000})),
            "HTTP shutdown",
        )
        process.wait(timeout=20)
        if process.returncode != 0:
            fail(f"HTTP host exited with {process.returncode}")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=10)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--rpc-binary", type=Path, required=True)
    parser.add_argument("--bundle", type=Path, required=True)
    args = parser.parse_args()
    rpc = args.rpc_binary.resolve(strict=True)
    _, protocol = load_bundle(args.bundle.resolve(strict=True))

    with tempfile.TemporaryDirectory(prefix="starweaver-independent-client-") as directory:
        root = Path(directory)
        config_dir = root / "config"
        workspace = root / "workspace"
        config_dir.mkdir()
        workspace.mkdir()
        (config_dir / "rpc.toml").write_text(
            "\n".join(
                [
                    "[server]",
                    f'workspace_root = {json.dumps(str(workspace))}',
                    'default_profile = "general"',
                    "",
                    "[server.http_auth]",
                    'token_env = "STARWEAVER_INDEPENDENT_CLIENT_TOKEN"',
                    'scopes = ["read", "run", "approval", "admin", "shutdown"]',
                    "",
                    "[profiles.general]",
                    'model_id = "local_echo"',
                    "",
                ]
            ),
            encoding="utf-8",
        )
        run_stdio(rpc, protocol, config_dir, workspace, root / "stdio.sqlite")
        run_http(rpc, protocol, config_dir, workspace, root / "http.sqlite")

    print("independent bundle-only client passed stdio, replay/live, typed-error, and HTTP proof")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        print(f"independent protocol client failed: {error}", file=sys.stderr)
        sys.exit(1)
