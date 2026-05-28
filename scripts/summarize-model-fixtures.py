#!/usr/bin/env python3
"""Generate a provider replay fixture summary.

The summary is deterministic JSON so CI and reviewers can compare fixture counts
and coverage tags without opening every fixture file.
"""

from __future__ import annotations

import argparse
import json
from collections import Counter
from pathlib import Path
from typing import Any

PROVIDERS = ["openai_chat", "openai_responses", "anthropic", "gemini", "bedrock"]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--fixtures-root",
        type=Path,
        default=Path("crates/starweaver-model/tests/fixtures"),
        help="Fixture root directory",
    )
    parser.add_argument("--output", type=Path, help="Output path, defaults to stdout")
    return parser.parse_args()


def fixture_kind(value: dict[str, Any]) -> str:
    if "expected_error" in value:
        return "error"
    if "provider_response" in value:
        return "replay"
    return "request"


def main() -> int:
    args = parse_args()
    summary: dict[str, Any] = {"providers": {}, "total": 0, "kinds": {}}
    total_kinds: Counter[str] = Counter()
    for provider in PROVIDERS:
        provider_dir = args.fixtures_root / provider
        files = sorted(provider_dir.glob("*.json"))
        kind_counts: Counter[str] = Counter()
        names: list[str] = []
        for path in files:
            value = json.loads(path.read_text())
            kind_counts[fixture_kind(value)] += 1
            names.append(path.stem)
        total_kinds.update(kind_counts)
        summary["providers"][provider] = {
            "count": len(files),
            "kinds": dict(sorted(kind_counts.items())),
            "fixtures": names,
        }
        summary["total"] += len(files)
    summary["kinds"] = dict(sorted(total_kinds.items()))
    output = json.dumps(summary, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(output)
    else:
        print(output, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
