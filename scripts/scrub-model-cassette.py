#!/usr/bin/env python3
"""Scrub secrets and unstable provider values from model cassette JSON.

The scrubber recursively redacts credential-like keys and normalizes volatile ids,
timestamps, and request metadata while preserving provider request/response shape.
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Any

SECRET_KEYS = {
    "api_key",
    "apikey",
    "authorization",
    "cookie",
    "password",
    "secret",
    "token",
    "x-api-key",
}
VOLATILE_KEYS = {
    "created",
    "created_at",
    "request_id",
    "timestamp",
}
ID_PATTERNS = [
    (re.compile(r"chatcmpl-[A-Za-z0-9_\-]+"), "chatcmpl_REDACTED"),
    (re.compile(r"resp_[A-Za-z0-9_\-]+"), "resp_REDACTED"),
    (re.compile(r"msg_[A-Za-z0-9_\-]+"), "msg_REDACTED"),
    (re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"), "uuid_REDACTED"),
]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cassette", type=Path, help="Cassette JSON file to scrub")
    parser.add_argument("--output", type=Path, help="Output path, defaults to stdout")
    return parser.parse_args()


def scrub_string(value: str) -> str:
    scrubbed = value
    for pattern, replacement in ID_PATTERNS:
        scrubbed = pattern.sub(replacement, scrubbed)
    return scrubbed


def scrub(value: Any, key: str | None = None) -> Any:
    key_lower = key.lower() if key else ""
    if key_lower in SECRET_KEYS:
        return "REDACTED"
    if key_lower in VOLATILE_KEYS:
        return "NORMALIZED"
    if isinstance(value, dict):
        return {item_key: scrub(item_value, item_key) for item_key, item_value in value.items()}
    if isinstance(value, list):
        return [scrub(item) for item in value]
    if isinstance(value, str):
        return scrub_string(value)
    return value


def main() -> int:
    args = parse_args()
    data = json.loads(args.cassette.read_text())
    output = json.dumps(scrub(data), indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(output)
    else:
        print(output, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
