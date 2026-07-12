#!/usr/bin/env python3
"""Validate the classified Starweaver Python top-level API snapshot."""

from __future__ import annotations

import ast
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
INIT = ROOT / "packages/starweaver-py/python/starweaver/__init__.py"
SNAPSHOT = ROOT / "packages/starweaver-py/tests/fixtures/api/top-level-v1.json"


def literal_string_collection(node: ast.AST) -> set[str]:
    value = ast.literal_eval(node)
    if not isinstance(value, (list, set, tuple, frozenset)) or not all(
        isinstance(item, str) for item in value
    ):
        raise ValueError("API declaration must contain only string literals")
    return set(value)


def declared_api() -> dict[str, str]:
    module = ast.parse(INIT.read_text(encoding="utf-8"), filename=str(INIT))
    exported: set[str] | None = None
    stable: set[str] | None = None
    extensions: set[str] = set()
    for statement in module.body:
        if isinstance(statement, ast.Assign):
            names = [target.id for target in statement.targets if isinstance(target, ast.Name)]
            if "__all__" in names:
                exported = literal_string_collection(statement.value)
            elif "STABLE_API" in names:
                if not isinstance(statement.value, ast.Call) or not statement.value.args:
                    raise ValueError("STABLE_API must be a literal frozenset")
                stable = literal_string_collection(statement.value.args[0])
        elif (
            isinstance(statement, ast.Expr)
            and isinstance(statement.value, ast.Call)
            and isinstance(statement.value.func, ast.Attribute)
            and isinstance(statement.value.func.value, ast.Name)
            and statement.value.func.value.id == "__all__"
            and statement.value.func.attr == "extend"
            and statement.value.args
        ):
            extensions.update(literal_string_collection(statement.value.args[0]))
    if exported is None or stable is None:
        raise ValueError("missing __all__ or STABLE_API declaration")
    exported.update(extensions)
    undeclared = stable - exported
    if undeclared:
        raise ValueError(f"stable names are not exported: {sorted(undeclared)}")
    return {name: "stable" if name in stable else "provisional" for name in sorted(exported)}


def main() -> int:
    actual = declared_api()
    expected = json.loads(SNAPSHOT.read_text(encoding="utf-8"))
    if actual != expected:
        added = sorted(set(actual) - set(expected))
        removed = sorted(set(expected) - set(actual))
        changed = sorted(
            name for name in set(actual) & set(expected) if actual[name] != expected[name]
        )
        print(f"Python API snapshot mismatch: added={added}, removed={removed}, changed={changed}")
        return 1
    stable_count = sum(value == "stable" for value in actual.values())
    print(
        f"Python API snapshot passed: {len(actual)} exports "
        f"({stable_count} stable, {len(actual) - stable_count} provisional)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
