#!/usr/bin/env python3
"""Import sanitized model cassette JSON files into replay fixture files.

The importer accepts either a single cassette object or a list of cassette objects.
Each cassette object must include:
- provider
- name
- model
- history
- expected_provider_request

It may include replay fields:
- provider_response + expected_response
- provider_response + expected_error

The output layout matches crates/starweaver-model/tests/fixtures/<provider>/<name>.json.
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Any

VALID_PROVIDERS = {"openai_chat", "openai_responses", "anthropic", "gemini", "bedrock"}
VALID_NAME = re.compile(r"^[a-z0-9][a-z0-9_\-]*$")

REQUIRED_FIELDS = {"provider", "name", "model", "history", "expected_provider_request"}
OPTIONAL_FIELDS = {
    "settings",
    "request_parameters",
    "tools",
    "native_tools",
    "provider_response",
    "expected_response",
    "expected_error",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cassette", type=Path, help="Sanitized cassette JSON file")
    parser.add_argument(
        "--fixtures-root",
        type=Path,
        default=Path("crates/starweaver-model/tests/fixtures"),
        help="Fixture root directory",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Validate and print target paths without writing files",
    )
    return parser.parse_args()


def load_entries(path: Path) -> list[dict[str, Any]]:
    data = json.loads(path.read_text())
    if isinstance(data, list):
        entries = data
    elif isinstance(data, dict):
        entries = [data]
    else:
        raise ValueError("cassette root must be an object or array")
    for index, entry in enumerate(entries):
        if not isinstance(entry, dict):
            raise ValueError(f"cassette entry {index} must be an object")
    return entries


def validate_entry(entry: dict[str, Any]) -> None:
    missing = sorted(REQUIRED_FIELDS - entry.keys())
    if missing:
        raise ValueError(f"missing required fields: {', '.join(missing)}")
    unknown = sorted(set(entry) - REQUIRED_FIELDS - OPTIONAL_FIELDS)
    if unknown:
        raise ValueError(f"unknown fields: {', '.join(unknown)}")
    provider = entry["provider"]
    if provider not in VALID_PROVIDERS:
        raise ValueError(f"unknown provider: {provider}")
    name = entry["name"]
    if not isinstance(name, str) or not VALID_NAME.match(name):
        raise ValueError(f"invalid fixture name: {name!r}")
    if ("expected_response" in entry) == ("expected_error" in entry):
        raise ValueError("entry must include exactly one of expected_response or expected_error")
    if "provider_response" not in entry:
        raise ValueError("entry must include provider_response for replay/error fixtures")


def fixture_body(entry: dict[str, Any]) -> dict[str, Any]:
    body: dict[str, Any] = {}
    for key in [
        "model",
        "history",
        "settings",
        "request_parameters",
        "tools",
        "native_tools",
        "expected_provider_request",
        "provider_response",
        "expected_response",
        "expected_error",
    ]:
        if key in entry:
            body[key] = entry[key]
    return body


def main() -> int:
    args = parse_args()
    entries = load_entries(args.cassette)
    targets: list[Path] = []
    for entry in entries:
        validate_entry(entry)
        target = args.fixtures_root / entry["provider"] / f"{entry['name']}.json"
        targets.append(target)
        if not args.dry_run:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(json.dumps(fixture_body(entry), indent=2, sort_keys=False) + "\n")
    for target in targets:
        print(target)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
