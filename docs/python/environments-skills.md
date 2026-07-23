# Python Environments And Skills

Environment providers give Python agents controlled access to filesystem,
shell, resource, and resumable environment state. Skills are native Starweaver
skill packages discovered through an environment provider.

## Virtual Environments

`VirtualEnvironment(...)` and `EnvironmentProvider.virtual(...)` are
deterministic and useful for tests:

```python
from starweaver import VirtualEnvironment


environment = VirtualEnvironment(
    files={"README.md": "hello"},
    shell_outputs={"pwd": {"stdout": "/workspace\n", "exit_code": 0}},
)
```

`EnvironmentProvider.virtual(...)` remains available for code that prefers the
factory style.

The virtual provider can be passed to `create_agent(...)`, `agent.session(...)`,
or one `run(...)`.

## Local Environments

`LocalEnvironment(...)` and `EnvironmentProvider.local(...)` wrap a Rust local
provider with explicit read/write and shell policy:

```python
from pathlib import Path

from starweaver import LocalEnvironment


environment = LocalEnvironment(
    Path.cwd(),
    allowed_paths=[Path.cwd()],
    context_file_tree_roots=[Path.cwd()],
    writable=False,
    allow_shell=False,
)
```

`allowed_paths` controls tool authority. `context_file_tree_roots` controls
what may be rendered into prompt context. Prompt visibility does not imply
filesystem authority.

Use `render_context()` to preview the provider-supplied model-facing
environment context. This is useful for validating that `context_file_tree_roots`
does not expose auxiliary roots that are available through `allowed_paths` for
tool execution.

## Envd Environments

`EnvdEnvironment.from_local(...)`, `EnvdEnvironment.http(...)`, and
`EnvdEnvironment.stdio(...)` are semantic constructors over the same native envd
providers exposed by `EnvironmentProvider.envd_local(...)`,
`EnvironmentProvider.envd_http(...)`, and `EnvironmentProvider.envd_stdio(...)`:

```python
from starweaver import EnvdEnvironment, VirtualEnvironment


backing = VirtualEnvironment(files={"README.md": "hello"})
environment = EnvdEnvironment.from_local(backing, environment_id="product-workspace")
```

The `Environment` base name is a semantic facade over `EnvironmentProvider`.
These names do not move policy, state, process ownership, or envd protocol
handling into Python.

## Direct Environment Operations

Environment providers expose direct async methods for applications and tests:

```python
from starweaver import Environment


async def inspect_environment(environment: Environment) -> None:
    text = await environment.read_text("README.md")
    entries = await environment.list_with_options("", max_entries=20)
    assert text or entries
```

For application-facing ergonomics, use `environment.files` and
`environment.shell`. These facades still delegate to the same Rust provider
policy:

```python
from starweaver import Environment


async def inspect_with_facades(environment: Environment) -> None:
    text = await environment.files.read("README.md")
    typed_entries = await environment.files.list_dir_with_types("")
    shell_output = await environment.shell.execute("pwd")
    assert text or typed_entries or shell_output
```

`Shell.execute(...)` covers foreground commands. `Shell.start(...)`,
`wait_process(...)`, `write_stdin(...)`, `send_signal(...)`, and
`kill_process(...)` expose provider-owned background process snapshots where
the provider supports `ProcessShellProvider`:

```python
from starweaver import Environment


async def run_background(environment: Environment) -> None:
    process = await environment.shell.start("sleep 5")
    assert process.running
    await environment.shell.send_signal(process, "TERM")
    killed = await environment.shell.kill_process(process)
    assert killed.terminal
```

The handle is a `ShellProcess` snapshot. The Rust provider owns the live
process and returns updated snapshots; Python does not keep a parallel process
store.

## Python-Defined Providers

Subclass `PythonEnvironmentProvider` when product code already owns the
workspace, resource, or remote execution boundary. Convert it with
`EnvironmentProvider.from_python(...)` before attaching it to agents or
toolsets:

```python
from typing import Any

from starweaver import EnvironmentProvider, PythonEnvironmentProvider


class ProductEnvironment(PythonEnvironmentProvider):
    def __init__(self) -> None:
        super().__init__(id="product-workspace")
        self.files = {"README.md": "hello"}

    async def read_text(self, path: str) -> str:
        return self.files[path]

    async def write_text(self, path: str, content: str) -> None:
        self.files[path] = content

    async def stat(self, path: str) -> dict[str, Any]:
        return {"size": len(self.files[path]), "is_file": True, "is_dir": False}

    async def list(self, path: str = "") -> list[str]:
        return sorted(self.files)

    async def run_shell(self, command: dict[str, Any]) -> dict[str, Any]:
        return {"status": 0, "stdout": "", "stderr": "", "metadata": {}}


async def bind_product_environment() -> EnvironmentProvider:
    return EnvironmentProvider.from_python(ProductEnvironment())
```

The native bridge schedules sync or async Python callbacks on the captured
event loop and validates results against the Rust `EnvironmentProvider` trait.
Core callbacks cover `read_text`, `read_bytes`, `write_text`, `create_dir`,
`delete_path`, `move_path`, `copy_path`, `write_scratch_file`, `stat`, `list`,
`run_shell`, and `export_state`. Background process methods remain available
from native providers that implement the Rust `ProcessShellProvider` extension.

Scratch is provider-owned ephemeral storage, not a separate file-operator API.
`write_scratch_file()` returns a path accepted by the same provider's normal
file methods. Native local providers return an absolute path and release their
exclusive directory when the last provider handle is dropped. They prefer the
operating-system temporary directory and fall back to an exclusive
`<workspace>/.starweaver/tmp/<instance-id>` child only when OS-temp creation
fails. Fallback initialization creates `.starweaver/tmp/.gitignore` with `*`
when absent; the shared tmp root remains after instance cleanup, and startup
never scans or reclaims sibling instances. Virtual and the default Python
implementation use `.starweaver/scratch`. The optional
`scratch_namespace` constructor argument must be one safe path segment and is
validated instead of being silently ignored.

Use `WorkspaceBinding` and `VirtualMount` when an application needs multiple
providers behind one agent-facing environment namespace:

```python
from starweaver import EnvironmentProvider, VirtualMount, WorkspaceBinding


async def bind_workspace() -> None:
    workspace = EnvironmentProvider.virtual(id="workspace", files={"README.md": "workspace"})
    data = EnvironmentProvider.virtual(id="data", files={"table.csv": "x,y\n1,2\n"})
    environment = WorkspaceBinding(
        [
            VirtualMount("workspace", workspace, default=True, default_for_shell=True),
            VirtualMount("data", data, mode="read_only"),
        ],
        id="workspace-binding",
    ).environment()

    assert await environment.read_text("README.md") == "workspace"
    assert await environment.read_text("/environment/data/table.csv")
```

Routing, read-only enforcement, default shell selection, and process-id rebasing
come from Rust `CompositeEnvironmentProvider`.

Use envd-backed providers when an application needs the provider boundary to go
through the envd service contract. `envd_local(...)` wraps an existing provider
with an in-process `LocalEnvd` service; `envd_http(...)` and `envd_stdio(...)`
connect to a remote envd endpoint or child process:

```python
from starweaver import EnvironmentProvider


async def use_envd() -> None:
    backing = EnvironmentProvider.virtual(files={"README.md": "envd"})
    environment = EnvironmentProvider.envd_local(
        backing,
        environment_id="app-workspace",
        id="app-envd",
    )

    assert await environment.read_text("README.md") == "envd"
```

The envd adapter still exposes the same file, shell, process, context, and
state methods. Environment state includes envd metadata such as the envd
environment id, kind, store, state version, and operation/effect ids.

For model-facing access, attach first-party environment toolsets:

```python
from starweaver import create_agent, environment_toolsets
from starweaver.testing import TestModel


agent = create_agent(
    model=TestModel.text("ready"),
    environment=environment,
    toolsets=environment_toolsets(),
)
```

## Environment State

Environment handles are process-local. Durable Starweaver session state can
store environment state snapshots, but Python applications must reattach live
providers after restore:

```python
from starweaver import SessionArchive


archive = SessionArchive.from_session(session)
restored = agent.session_from_archive(archive, environment=environment)
```

Use `session.export_environment_state()` or `environment.export_state()` when a
host application needs to inspect provider state explicitly.

## Skills

`SkillRegistry.scan(...)` loads Starweaver `SKILL.md` packages from an
environment provider. It is not a Python-only skill format.

```python
from starweaver import EnvironmentProvider, SkillRegistry, SkillSourceScope


async def load_skills() -> SkillRegistry:
    environment = EnvironmentProvider.virtual(
        files={
            "skills/research/SKILL.md": """---
name: research
description: Research workflow
---
Read primary sources before answering.
"""
        }
    )
    return await SkillRegistry.scan(
        environment,
        SkillSourceScope(root="", directories=["skills"]),
    )
```

Pass the registry directly to `create_agent(...)`:

```python
agent = create_agent(model=model, environment=environment, skills=skills)
```

Use `skills.toolset()` when the model should search and activate skills through
the normal toolset composition path.
