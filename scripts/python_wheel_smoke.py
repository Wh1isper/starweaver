from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def main() -> None:
    args = parse_args()
    wheel = (args.wheel or newest_wheel(args.dist_dir)).resolve()
    uv = shutil.which("uv")
    if uv is None:
        raise RuntimeError("uv is required to run the Python wheel smoke")

    repository_root = Path(__file__).resolve().parents[1]
    examples = [
        repository_root / "examples" / "python" / "claw_like_runtime.py",
        repository_root / "examples" / "python" / "claw_product_runtime.py",
    ]
    for example in examples:
        if not example.exists():
            raise RuntimeError(f"missing smoke example: {example}")

    with tempfile.TemporaryDirectory(prefix="starweaver-wheel-smoke-") as directory:
        workdir = Path(directory)
        venv = workdir / ".venv"
        run([uv, "venv", str(venv), "--python", sys.executable], cwd=workdir)
        python = venv_python(venv)
        run([uv, "pip", "install", "--python", str(python), "pydantic>=2.0"], cwd=workdir)
        run(
            [uv, "pip", "install", "--python", str(python), "--no-deps", str(wheel)],
            cwd=workdir,
        )
        run([str(python), "-c", deterministic_smoke_code()], cwd=workdir)
        for example in examples:
            run([str(python), str(example)], cwd=workdir)

    print(f"Python wheel smoke passed: {wheel}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Smoke-test a built starweaver wheel")
    parser.add_argument(
        "dist_dir",
        nargs="?",
        default="dist/python",
        type=Path,
        help="Directory containing built Python distributions",
    )
    parser.add_argument(
        "--wheel",
        type=Path,
        help="Specific wheel path to install instead of selecting the newest starweaver wheel",
    )
    return parser.parse_args()


def newest_wheel(dist_dir: Path) -> Path:
    wheels = sorted(
        dist_dir.glob("starweaver-*.whl"),
        key=lambda path: path.stat().st_mtime,
        reverse=True,
    )
    if not wheels:
        raise RuntimeError(f"no starweaver wheel found in {dist_dir}")
    return wheels[0]


def venv_python(venv: Path) -> Path:
    if os.name == "nt":
        return venv / "Scripts" / "python.exe"
    return venv / "bin" / "python"


def run(command: list[str], *, cwd: Path) -> None:
    env = dict(os.environ)
    env.pop("PYTHONPATH", None)
    env.pop("CONDA_PREFIX", None)
    env.pop("CONDA_DEFAULT_ENV", None)
    env["PYTHONNOUSERSITE"] = "1"
    subprocess.run(command, check=True, cwd=cwd, env=env)  # noqa: S603


def deterministic_smoke_code() -> str:
    return r"""
import asyncio
import starweaver
from starweaver import create_agent, tool
from starweaver.testing import TestModel


@tool
async def add(left: int, right: int) -> dict[str, int]:
    return {"total": left + right}


async def main() -> None:
    model = TestModel.responses(
        [
            TestModel.tool_call_response(
                [{"id": "call_add", "name": "add", "arguments": {"left": 2, "right": 3}}]
            ),
            {"text": "wheel-ok"},
        ]
    )
    result = await create_agent(model=model, tools=[add]).run("Add two numbers")
    assert result.output == "wheel-ok"
    assert starweaver.__version__ == starweaver.version()


asyncio.run(main())
"""


if __name__ == "__main__":
    main()
