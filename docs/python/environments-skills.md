# Python Environments And Skills

Environment providers give Python agents controlled access to filesystem,
shell, resource, and resumable environment state. Skills are native Starweaver
skill packages discovered through an environment provider.

## Virtual Environments

`EnvironmentProvider.virtual(...)` is deterministic and useful for tests:

```python
from starweaver import EnvironmentProvider


environment = EnvironmentProvider.virtual(
    files={"README.md": "hello"},
    shell_outputs={"pwd": {"stdout": "/workspace\n", "exit_code": 0}},
)
```

The virtual provider can be passed to `create_agent(...)`, `agent.session(...)`,
or one `run(...)`.

## Local Environments

`EnvironmentProvider.local(...)` wraps a Rust local provider with explicit
read/write and shell policy:

```python
from pathlib import Path

from starweaver import EnvironmentProvider


environment = EnvironmentProvider.local(
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

## Direct Environment Operations

Environment providers expose direct async methods for applications and tests:

```python
async def inspect_environment(environment: EnvironmentProvider) -> None:
    text = await environment.read_text("README.md")
    entries = await environment.list_with_options("", max_entries=20)
    assert text or entries
```

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
