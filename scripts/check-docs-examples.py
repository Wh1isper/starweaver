#!/usr/bin/env python3
"""Compile Rust examples from docs Markdown files."""

from __future__ import annotations

import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import textwrap

ROOT = pathlib.Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs"
FENCE = re.compile(r"```rust\n(.*?)\n```", re.DOTALL)


def function_name(path: pathlib.Path, index: int) -> str:
    stem = re.sub(r"[^a-zA-Z0-9]+", "_", path.stem).strip("_")
    return f"docs_{stem}_{index}"


def wrap_example(code: str, name: str) -> str:
    cleaned = textwrap.dedent(code).strip()
    is_async = "# async fn example()" in cleaned
    visible = "\n".join(line[2:] if line.startswith("# ") else line for line in cleaned.splitlines())
    if is_async:
        return f"#[tokio::test]\nasync fn {name}() {{\n{visible}\n    example().await.unwrap();\n}}\n"
    return f"#[test]\nfn {name}() {{\n{visible}\n}}\n"


def main() -> int:
    examples: list[str] = []
    for path in sorted(DOCS.glob("*.md")):
        for index, match in enumerate(FENCE.finditer(path.read_text()), start=1):
            examples.append(wrap_example(match.group(1), function_name(path, index)))

    if not examples:
        print("no rust examples found", file=sys.stderr)
        return 1

    tmp = pathlib.Path(tempfile.mkdtemp(prefix="starweaver-docs-"))
    try:
        (tmp / "src").mkdir()
        (tmp / "Cargo.toml").write_text(
            textwrap.dedent(
                f"""
                [package]
                name = "starweaver-docs-examples"
                version = "0.0.0"
                edition = "2021"

                [dependencies]
                async-trait = "0.1.89"
                serde = {{ version = "1.0.228", features = ["derive"] }}
                serde_json = "1.0.145"
                starweaver-agent = {{ path = "{ROOT / 'crates/starweaver-agent'}" }}
                starweaver-context = {{ path = "{ROOT / 'crates/starweaver-context'}" }}
                starweaver-model = {{ path = "{ROOT / 'crates/starweaver-model'}" }}
                starweaver-runtime = {{ path = "{ROOT / 'crates/starweaver-runtime'}" }}
                starweaver-tools = {{ path = "{ROOT / 'crates/starweaver-tools'}" }}
                tokio = {{ version = "1.48.0", features = ["macros", "rt-multi-thread"] }}
                """
            ).strip()
            + "\n"
        )
        (tmp / "src/lib.rs").write_text("\n".join(examples))
        return subprocess.run(["cargo", "test"], cwd=tmp, check=False).returncode
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
