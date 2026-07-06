"""Environment provider facades backed by Starweaver native providers."""

from __future__ import annotations

import asyncio
import uuid
from collections.abc import AsyncIterator, Mapping, Sequence
from dataclasses import asdict, dataclass, is_dataclass
from os import PathLike
from pathlib import Path
from signal import Signals
from typing import Any, Literal, cast

from . import _native
from .resources import ResourceRef, ensure_resource_ref


def _jsonify(value: Any) -> Any:
    if value is None or isinstance(value, str | int | float | bool):
        return value
    if isinstance(value, bytes):
        return value.decode("utf-8")
    if isinstance(value, Mapping):
        return {str(key): _jsonify(item) for key, item in value.items()}
    if isinstance(value, Sequence) and not isinstance(value, str | bytes | bytearray):
        return [_jsonify(item) for item in value]
    if is_dataclass(value) and not isinstance(value, type):
        return _jsonify(asdict(value))
    to_dict = getattr(value, "to_dict", None)
    if callable(to_dict):
        return _jsonify(to_dict())
    return value


@dataclass(frozen=True)
class ShellProcess:
    """Snapshot handle for a provider-owned background shell process."""

    raw: dict[str, Any]

    @classmethod
    def from_raw(cls, raw: Mapping[str, Any]) -> ShellProcess:
        return cls(dict(raw))

    @property
    def process_id(self) -> str:
        return str(self.raw["process_id"])

    @property
    def command(self) -> str:
        return str(self.raw.get("command") or "")

    @property
    def status(self) -> str:
        return str(self.raw.get("status") or "running")

    @property
    def stdout(self) -> str:
        return str(self.raw.get("stdout") or "")

    @property
    def stderr(self) -> str:
        return str(self.raw.get("stderr") or "")

    @property
    def return_code(self) -> int | None:
        value = self.raw.get("return_code")
        if value is None:
            return None
        return int(value)

    @property
    def metadata(self) -> dict[str, Any]:
        metadata = self.raw.get("metadata")
        return dict(metadata) if isinstance(metadata, Mapping) else {}

    @property
    def running(self) -> bool:
        return self.status == "running"

    @property
    def terminal(self) -> bool:
        return self.status in {"completed", "failed", "killed"}

    def to_dict(self) -> dict[str, Any]:
        return dict(self.raw)


@dataclass(frozen=True)
class VirtualPath:
    """Agent-facing POSIX-style path inside an environment binding."""

    path: str

    def __post_init__(self) -> None:
        object.__setattr__(self, "path", self.path.replace("\\", "/"))

    def join(self, *parts: str) -> VirtualPath:
        path = self.path.rstrip("/")
        for part in parts:
            normalized = part.replace("\\", "/").strip("/")
            path = normalized if not path else f"{path}/{normalized}"
        return VirtualPath(path)

    def as_posix(self) -> str:
        return self.path

    def __str__(self) -> str:
        return self.path


@dataclass(frozen=True)
class VirtualMount:
    """One provider mounted into a composite workspace binding."""

    id: str
    environment: EnvironmentProvider
    mode: Literal["read_write", "read_only"] = "read_write"
    default: bool = False
    default_for_shell: bool = False

    @property
    def root(self) -> VirtualPath:
        return VirtualPath(f"/environment/{self.id}")

    def to_dict(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "root": str(self.root),
            "mode": self.mode,
            "default": self.default,
            "default_for_shell": self.default_for_shell,
            "provider_id": self.environment.id,
        }


@dataclass(frozen=True)
class WorkspaceBinding:
    """Composite workspace binding over one or more environment providers."""

    mounts: tuple[VirtualMount, ...]
    id: str | None = None

    def __init__(
        self,
        mounts: Sequence[VirtualMount],
        *,
        id: str | None = None,  # noqa: A002
    ) -> None:
        object.__setattr__(self, "mounts", tuple(mounts))
        object.__setattr__(self, "id", id)

    def environment(self) -> EnvironmentProvider:
        return EnvironmentProvider.composite(self.mounts, id=self.id)

    def to_dict(self) -> dict[str, Any]:
        payload: dict[str, Any] = {"mounts": [mount.to_dict() for mount in self.mounts]}
        if self.id is not None:
            payload["id"] = self.id
        return payload


class PythonEnvironmentProvider:
    """Python-defined provider adapted into Starweaver's native environment trait."""

    id: str = "python"

    def __init__(self, *, id: str | None = None) -> None:  # noqa: A002
        if id is not None:
            self.id = id

    def to_native(self) -> _native.EnvironmentProvider:
        provider_id = self._provider_id()
        return _native.EnvironmentProvider.python_provider(
            self,
            asyncio.get_running_loop(),
            id=provider_id,
        )

    def as_environment_provider(self) -> EnvironmentProvider:
        return EnvironmentProvider(self.to_native())

    async def read_text(self, path: str) -> str:
        raise NotImplementedError("read_text must be implemented")

    async def read_bytes(self, path: str, offset: int = 0, length: int | None = None) -> bytes:
        data = (await self.read_text(path)).encode()
        end = None if length is None else offset + length
        return data[offset:end]

    async def write_text(self, path: str, content: str) -> None:
        raise NotImplementedError("write_text must be implemented")

    async def create_dir(self, path: str, parents: bool = True) -> None:
        raise NotImplementedError("create_dir must be implemented")

    async def delete_path(self, path: str, recursive: bool = False) -> None:
        raise NotImplementedError("delete_path must be implemented")

    async def move_path(self, src: str, dst: str, overwrite: bool = False) -> None:
        await self.copy_path(src, dst, overwrite=overwrite)
        await self.delete_path(src)

    async def copy_path(self, src: str, dst: str, overwrite: bool = False) -> None:
        _ = overwrite
        await self.write_text(dst, await self.read_text(src))

    async def write_tmp_file(self, filename: str, content: str | bytes) -> str:
        path = f".tmp/{filename.lstrip('/')}"
        text = content.decode("utf-8") if isinstance(content, bytes) else content
        await self.write_text(path, text)
        return path

    async def stat(self, path: str) -> dict[str, Any]:
        raise NotImplementedError("stat must be implemented")

    async def list(self, path: str = "") -> list[str]:
        raise NotImplementedError("list must be implemented")

    async def run_shell(self, command: Mapping[str, Any]) -> dict[str, Any]:
        raise NotImplementedError("run_shell must be implemented")

    async def render_context(self) -> str | None:
        return None

    async def export_state(self) -> dict[str, Any]:
        return {"provider_id": self._provider_id()}

    def _provider_id(self) -> str:
        provider_id = getattr(self, "id", None)
        if not isinstance(provider_id, str) or not provider_id.strip():
            raise ValueError("environment provider id must not be empty")
        return provider_id


class DockerEnvironmentProvider(PythonEnvironmentProvider):
    """Docker-backed workspace provider using a bind-mounted host workspace."""

    def __init__(
        self,
        image: str,
        workspace: str | PathLike[str],
        *,
        id: str | None = None,  # noqa: A002
        container_workspace: str = "/workspace",
        name: str | None = None,
        docker: str = "docker",
        command: Sequence[str] = ("sleep", "infinity"),
        env: Mapping[str, str] | None = None,
        network: str | None = None,
        user: str | None = None,
        auto_remove: bool = True,
    ) -> None:
        super().__init__(id=id or f"docker-{uuid.uuid4().hex[:8]}")
        self.image = image
        self.workspace = Path(workspace).resolve()
        self.container_workspace = container_workspace.rstrip("/") or "/workspace"
        self.name = name
        self.docker = docker
        self.command = tuple(command)
        self.env = dict(env or {})
        self.network = network
        self.user = user
        self.auto_remove = auto_remove
        self._container_id: str | None = None
        self._lock = asyncio.Lock()

    async def read_text(self, path: str) -> str:
        return self._host_path(path).read_text()

    async def read_bytes(self, path: str, offset: int = 0, length: int | None = None) -> bytes:
        data = self._host_path(path).read_bytes()
        end = None if length is None else offset + length
        return data[offset:end]

    async def write_text(self, path: str, content: str) -> None:
        target = self._host_path(path)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)

    async def create_dir(self, path: str, parents: bool = True) -> None:
        self._host_path(path).mkdir(parents=parents, exist_ok=parents)

    async def delete_path(self, path: str, recursive: bool = False) -> None:
        target = self._host_path(path)
        if target.is_dir():
            if not recursive:
                target.rmdir()
                return
            for child in sorted(target.rglob("*"), key=lambda item: len(item.parts), reverse=True):
                if child.is_dir():
                    child.rmdir()
                else:
                    child.unlink()
            target.rmdir()
            return
        target.unlink()

    async def move_path(self, src: str, dst: str, overwrite: bool = False) -> None:
        source = self._host_path(src)
        target = self._host_path(dst)
        if target.exists() and not overwrite:
            raise FileExistsError(dst)
        target.parent.mkdir(parents=True, exist_ok=True)
        source.replace(target)

    async def copy_path(self, src: str, dst: str, overwrite: bool = False) -> None:
        source = self._host_path(src)
        target = self._host_path(dst)
        if target.exists() and not overwrite:
            raise FileExistsError(dst)
        target.parent.mkdir(parents=True, exist_ok=True)
        if source.is_dir():
            await self.create_dir(dst, parents=True)
            for child in source.rglob("*"):
                rel = child.relative_to(source)
                child_target = target / rel
                if child.is_dir():
                    child_target.mkdir(parents=True, exist_ok=True)
                else:
                    child_target.parent.mkdir(parents=True, exist_ok=True)
                    child_target.write_bytes(child.read_bytes())
        else:
            target.write_bytes(source.read_bytes())

    async def stat(self, path: str) -> dict[str, Any]:
        target = self._host_path(path)
        stat = target.stat()
        return {
            "size": stat.st_size if target.is_file() else 0,
            "is_file": target.is_file(),
            "is_dir": target.is_dir(),
            "modified_unix_seconds": int(stat.st_mtime),
        }

    async def list(self, path: str = "") -> list[str]:
        target = self._host_path(path)
        return sorted(child.name for child in target.iterdir())

    async def run_shell(self, command: Mapping[str, Any]) -> dict[str, Any]:
        container_id = await self._ensure_container()
        shell_command = str(command.get("command") or "")
        timeout = command.get("timeout_seconds")
        cwd = self._container_cwd(command.get("cwd"))
        env = dict(self.env)
        extra_env = command.get("environment")
        if isinstance(extra_env, Mapping):
            env.update({str(key): str(value) for key, value in extra_env.items()})
        args = [self.docker, "exec", "-i", "-w", cwd]
        for key, value in env.items():
            args.extend(["-e", f"{key}={value}"])
        args.extend([container_id, "sh", "-lc", shell_command])
        status, stdout, stderr = await self._run(args, timeout=timeout)
        return {
            "status": status,
            "stdout": stdout,
            "stderr": stderr,
            "metadata": {"container_id": container_id},
        }

    async def render_context(self) -> str | None:
        return f"Docker workspace: {self.container_workspace}"

    async def export_state(self) -> dict[str, Any]:
        return {
            "provider_id": self._provider_id(),
            "metadata": {
                "kind": "docker",
                "image": self.image,
                "workspace": str(self.workspace),
                "container_workspace": self.container_workspace,
                "container_id": self._container_id,
            },
        }

    async def close(self) -> None:
        if self._container_id is None:
            return
        container_id = self._container_id
        self._container_id = None
        await self._run([self.docker, "rm", "-f", container_id])

    async def _ensure_container(self) -> str:
        if self._container_id is not None:
            return self._container_id
        async with self._lock:
            if self._container_id is not None:
                return self._container_id
            self.workspace.mkdir(parents=True, exist_ok=True)
            args = [self.docker, "run", "-d"]
            if self.auto_remove:
                args.append("--rm")
            if self.name:
                args.extend(["--name", self.name])
            args.extend(
                [
                    "-v",
                    f"{self.workspace}:{self.container_workspace}",
                    "-w",
                    self.container_workspace,
                ]
            )
            if self.network:
                args.extend(["--network", self.network])
            if self.user:
                args.extend(["--user", self.user])
            for key, value in self.env.items():
                args.extend(["-e", f"{key}={value}"])
            args.append(self.image)
            args.extend(self.command)
            status, stdout, stderr = await self._run(args)
            if status != 0:
                raise RuntimeError(f"docker run failed: {stderr or stdout}")
            self._container_id = stdout.strip()
            return self._container_id

    async def _run(
        self,
        args: Sequence[str],
        *,
        timeout: int | float | None = None,
    ) -> tuple[int, str, str]:
        process = await asyncio.create_subprocess_exec(
            *args,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        try:
            stdout, stderr = await asyncio.wait_for(process.communicate(), timeout=timeout)
        except TimeoutError:
            process.kill()
            stdout, stderr = await process.communicate()
            return 124, stdout.decode(), stderr.decode()
        return process.returncode or 0, stdout.decode(), stderr.decode()

    def _host_path(self, path: str) -> Path:
        path_text = str(path)
        if path_text == self.container_workspace:
            relative = Path()
        elif path_text.startswith(f"{self.container_workspace}/"):
            relative = Path(path_text.removeprefix(self.container_workspace).lstrip("/"))
        else:
            relative = Path(path_text.lstrip("/"))
        target = (self.workspace / relative).resolve()
        try:
            target.relative_to(self.workspace)
        except ValueError as error:
            raise ValueError(f"path escapes docker workspace: {path}") from error
        return target

    def _container_cwd(self, cwd: object) -> str:
        if not cwd:
            return self.container_workspace
        cwd_text = str(cwd)
        if cwd_text.startswith("/"):
            return cwd_text
        return f"{self.container_workspace}/{cwd_text.strip('/')}"


class EnvironmentProvider:
    """Provider-scoped filesystem, shell, resource, and state access."""

    def __init__(self, native: _native.EnvironmentProvider) -> None:
        self._native = native

    @classmethod
    def from_python(
        cls,
        provider: PythonEnvironmentProvider,
        *,
        id: str | None = None,  # noqa: A002
    ) -> EnvironmentProvider:
        return cls(
            _native.EnvironmentProvider.python_provider(
                provider,
                asyncio.get_running_loop(),
                id=id,
            )
        )

    @classmethod
    def virtual(
        cls,
        *,
        id: str = "virtual",  # noqa: A002
        files: Mapping[str, str] | None = None,
        resources: Sequence[ResourceRef | Mapping[str, Any]] | None = None,
        shell_outputs: Mapping[str, str | Mapping[str, Any]] | None = None,
        tmp_namespace: str | None = None,
    ) -> EnvironmentProvider:
        return cls(
            _native.EnvironmentProvider.virtual_provider(
                id,
                files=dict(files or {}),
                resources=[ensure_resource_ref(resource).to_dict() for resource in resources or ()],
                shell_outputs=dict(shell_outputs or {}),
                tmp_namespace=tmp_namespace,
            )
        )

    @classmethod
    def local(
        cls,
        root: str | PathLike[str],
        *,
        id: str | None = None,  # noqa: A002
        allowed_paths: Sequence[str | PathLike[str]] | None = None,
        context_file_tree_roots: Sequence[str | PathLike[str]] | None = None,
        writable: bool = False,
        allow_shell: bool = False,
        allowed_programs: Sequence[str] | None = None,
        tmp_namespace: str | None = None,
    ) -> EnvironmentProvider:
        return cls(
            _native.EnvironmentProvider.local(
                str(Path(root)),
                id=id,
                allowed_paths=[str(Path(path)) for path in allowed_paths or ()],
                context_file_tree_roots=[str(Path(path)) for path in context_file_tree_roots or ()],
                writable=writable,
                allow_shell=allow_shell,
                allowed_programs=list(allowed_programs or ()),
                tmp_namespace=tmp_namespace,
            )
        )

    @classmethod
    def docker(
        cls,
        image: str,
        workspace: str | PathLike[str],
        **kwargs: Any,
    ) -> EnvironmentProvider:
        return DockerEnvironmentProvider(image, workspace, **kwargs).as_environment_provider()

    @classmethod
    def composite(
        cls,
        mounts: Sequence[VirtualMount],
        *,
        id: str | None = None,  # noqa: A002
    ) -> EnvironmentProvider:
        return cls(
            _native.EnvironmentProvider.composite(
                [mount.id for mount in mounts],
                [mount.environment.to_native() for mount in mounts],
                id=id,
                modes=[mount.mode for mount in mounts],
                defaults=[mount.default for mount in mounts],
                default_for_shell=[mount.default_for_shell for mount in mounts],
            )
        )

    @classmethod
    def envd_local(
        cls,
        environment: EnvironmentProvider | _native.EnvironmentProvider,
        *,
        environment_id: str | None = None,
        id: str | None = None,  # noqa: A002
    ) -> EnvironmentProvider:
        native_environment = ensure_environment_provider(environment)
        if native_environment is None:
            raise TypeError("environment must be an EnvironmentProvider")
        return cls(
            _native.EnvironmentProvider.envd_local(
                native_environment,
                environment_id=environment_id,
                id=id,
            )
        )

    @classmethod
    def envd_http(
        cls,
        endpoint: str,
        *,
        environment_id: str = "env_cli_default",
        token: str | None = None,
        id: str | None = None,  # noqa: A002
    ) -> EnvironmentProvider:
        return cls(
            _native.EnvironmentProvider.envd_http(
                endpoint,
                environment_id=environment_id,
                token=token,
                id=id,
            )
        )

    @classmethod
    def envd_stdio(
        cls,
        program: str | PathLike[str],
        *,
        args: Sequence[str] | None = None,
        environment_id: str = "env_cli_default",
        id: str | None = None,  # noqa: A002
    ) -> EnvironmentProvider:
        return cls(
            _native.EnvironmentProvider.envd_stdio(
                str(Path(program)),
                args=list(args or ()),
                environment_id=environment_id,
                id=id,
            )
        )

    @property
    def id(self) -> str:
        return self._native.id

    def to_native(self) -> _native.EnvironmentProvider:
        return self._native

    @property
    def files(self) -> FileOperator:
        return FileOperator(self)

    @property
    def shell(self) -> Shell:
        return Shell(self)

    async def read_text(self, path: str) -> str:
        return cast(str, await self._native.read_text(path))

    async def read_bytes(
        self,
        path: str,
        *,
        offset: int = 0,
        length: int | None = None,
    ) -> bytes:
        return cast(bytes, await self._native.read_bytes(path, offset, length))

    async def write_text(self, path: str, content: str) -> None:
        await self._native.write_text(path, content)

    async def write_tmp_file(self, filename: str, content: str | bytes) -> str:
        return cast(str, await self._native.write_tmp_file(filename, content))

    async def create_dir(self, path: str, *, parents: bool = True) -> None:
        await self._native.create_dir(path, parents)

    async def delete_path(self, path: str, *, recursive: bool = False) -> None:
        await self._native.delete_path(path, recursive)

    async def move_path(self, src: str, dst: str, *, overwrite: bool = False) -> None:
        await self._native.move_path(src, dst, overwrite)

    async def copy_path(self, src: str, dst: str, *, overwrite: bool = False) -> None:
        await self._native.copy_path(src, dst, overwrite)

    async def list(self, path: str = "") -> list[str]:
        return cast(list[str], await self._native.list(path))

    async def list_with_options(
        self,
        path: str = "",
        *,
        max_entries: int = 0,
        ignore_patterns: Sequence[str] | None = None,
    ) -> dict[str, Any]:
        return cast(
            dict[str, Any],
            await self._native.list_with_options(
                path,
                max_entries=max_entries,
                ignore_patterns=list(ignore_patterns or ()),
            ),
        )

    async def stat(self, path: str) -> dict[str, Any]:
        return cast(dict[str, Any], await self._native.stat(path))

    async def glob(
        self,
        pattern: str,
        *,
        path: str = "",
        include_hidden: bool = False,
        include_ignored: bool = False,
        max_results: int = 500,
    ) -> list[dict[str, Any]]:
        return cast(
            list[dict[str, Any]],
            await self._native.glob(
                path,
                pattern,
                include_hidden=include_hidden,
                include_ignored=include_ignored,
                max_results=max_results,
            ),
        )

    async def grep(
        self,
        pattern: str,
        *,
        path: str = "",
        include: str | None = None,
        context_lines: int = 0,
        max_results: int = 100,
        max_matches_per_file: int = 20,
        max_files: int = 50,
        include_hidden: bool = False,
        include_ignored: bool = False,
    ) -> list[dict[str, Any]]:
        return cast(
            list[dict[str, Any]],
            await self._native.grep(
                path,
                pattern,
                include=include,
                context_lines=context_lines,
                max_results=max_results,
                max_matches_per_file=max_matches_per_file,
                max_files=max_files,
                include_hidden=include_hidden,
                include_ignored=include_ignored,
            ),
        )

    async def render_context(self) -> str | None:
        """Render provider-supplied model-facing environment context."""

        return cast(str | None, await self._native.render_context())

    async def run_shell(
        self,
        command: str,
        *,
        timeout_seconds: int | None = None,
        cwd: str | None = None,
        environment: Mapping[str, str] | None = None,
    ) -> dict[str, Any]:
        return cast(
            dict[str, Any],
            await self._native.run_shell(
                command,
                timeout_seconds=timeout_seconds,
                cwd=cwd,
                environment=dict(environment or {}),
            ),
        )

    async def start_process(
        self,
        command: str,
        *,
        timeout_seconds: int | None = None,
        cwd: str | None = None,
        environment: Mapping[str, str] | None = None,
    ) -> ShellProcess:
        return ShellProcess.from_raw(
            cast(
                dict[str, Any],
                await self._native.start_process(
                    command,
                    timeout_seconds=timeout_seconds,
                    cwd=cwd,
                    environment=dict(environment or {}),
                ),
            )
        )

    async def wait_process(
        self,
        process: ShellProcess | str,
        *,
        timeout_seconds: int = 0,
    ) -> ShellProcess:
        return ShellProcess.from_raw(
            cast(
                dict[str, Any],
                await self._native.wait_process(
                    _process_id(process),
                    timeout_seconds=timeout_seconds,
                ),
            )
        )

    async def list_processes(self) -> list[ShellProcess]:
        snapshots = cast(list[dict[str, Any]], await self._native.list_processes())
        return [ShellProcess.from_raw(snapshot) for snapshot in snapshots]

    async def input_process(
        self,
        process: ShellProcess | str,
        text: str,
        *,
        close_stdin: bool = False,
    ) -> ShellProcess:
        return ShellProcess.from_raw(
            cast(
                dict[str, Any],
                await self._native.input_process(
                    _process_id(process),
                    text,
                    close_stdin=close_stdin,
                ),
            )
        )

    async def signal_process(
        self,
        process: ShellProcess | str,
        signal: int | str,
    ) -> ShellProcess:
        return ShellProcess.from_raw(
            cast(
                dict[str, Any],
                await self._native.signal_process(_process_id(process), _signal_number(signal)),
            )
        )

    async def kill_process(self, process: ShellProcess | str) -> ShellProcess:
        return ShellProcess.from_raw(
            cast(dict[str, Any], await self._native.kill_process(_process_id(process)))
        )

    async def export_state(self) -> dict[str, Any]:
        return cast(dict[str, Any], await self._native.export_state())


class Environment(EnvironmentProvider):
    """Semantic base facade for Rust-owned environment providers."""


class VirtualEnvironment(Environment):
    """Deterministic in-memory environment backed by the native virtual provider."""

    def __init__(
        self,
        native: _native.EnvironmentProvider | None = None,
        *,
        id: str = "virtual",  # noqa: A002
        files: Mapping[str, str] | None = None,
        resources: Sequence[ResourceRef | Mapping[str, Any]] | None = None,
        shell_outputs: Mapping[str, str | Mapping[str, Any]] | None = None,
        tmp_namespace: str | None = None,
    ) -> None:
        super().__init__(
            native
            or _native.EnvironmentProvider.virtual_provider(
                id,
                files=dict(files or {}),
                resources=[ensure_resource_ref(resource).to_dict() for resource in resources or ()],
                shell_outputs=dict(shell_outputs or {}),
                tmp_namespace=tmp_namespace,
            )
        )


class LocalEnvironment(Environment):
    """Local filesystem environment backed by the native local provider."""

    def __init__(
        self,
        root: str | PathLike[str] | _native.EnvironmentProvider,
        *,
        id: str | None = None,  # noqa: A002
        allowed_paths: Sequence[str | PathLike[str]] | None = None,
        context_file_tree_roots: Sequence[str | PathLike[str]] | None = None,
        writable: bool = False,
        allow_shell: bool = False,
        allowed_programs: Sequence[str] | None = None,
        tmp_namespace: str | None = None,
    ) -> None:
        if isinstance(root, _native.EnvironmentProvider):
            super().__init__(root)
            return
        super().__init__(
            _native.EnvironmentProvider.local(
                str(Path(root)),
                id=id,
                allowed_paths=[str(Path(path)) for path in allowed_paths or ()],
                context_file_tree_roots=[str(Path(path)) for path in context_file_tree_roots or ()],
                writable=writable,
                allow_shell=allow_shell,
                allowed_programs=list(allowed_programs or ()),
                tmp_namespace=tmp_namespace,
            )
        )


class EnvdEnvironment(Environment):
    """Environment facade backed by local, HTTP, or stdio envd providers."""

    @classmethod
    def from_local(
        cls,
        environment: EnvironmentProvider | _native.EnvironmentProvider,
        *,
        environment_id: str | None = None,
        id: str | None = None,  # noqa: A002
    ) -> EnvdEnvironment:
        native_environment = ensure_environment_provider(environment)
        if native_environment is None:
            raise TypeError("environment must be an EnvironmentProvider")
        return cls(
            _native.EnvironmentProvider.envd_local(
                native_environment,
                environment_id=environment_id,
                id=id,
            )
        )

    @classmethod
    def http(
        cls,
        endpoint: str,
        *,
        environment_id: str = "env_cli_default",
        token: str | None = None,
        id: str | None = None,  # noqa: A002
    ) -> EnvdEnvironment:
        return cls(
            _native.EnvironmentProvider.envd_http(
                endpoint,
                environment_id=environment_id,
                token=token,
                id=id,
            )
        )

    @classmethod
    def stdio(
        cls,
        program: str | PathLike[str],
        *,
        args: Sequence[str] | None = None,
        environment_id: str = "env_cli_default",
        id: str | None = None,  # noqa: A002
    ) -> EnvdEnvironment:
        return cls(
            _native.EnvironmentProvider.envd_stdio(
                str(Path(program)),
                args=list(args or ()),
                environment_id=environment_id,
                id=id,
            )
        )


class FileOperator:
    """Application-facing file facade over an environment provider."""

    def __init__(self, environment: EnvironmentProvider) -> None:
        self.environment = environment

    async def read(self, path: str) -> str:
        return await self.environment.read_text(path)

    async def read_bytes(
        self,
        path: str,
        *,
        offset: int = 0,
        length: int | None = None,
    ) -> bytes:
        return await self.environment.read_bytes(path, offset=offset, length=length)

    async def write(self, path: str, content: str) -> None:
        await self.environment.write_text(path, content)

    async def list_with_options(
        self,
        path: str = "",
        *,
        max_entries: int = 0,
        ignore_patterns: Sequence[str] | None = None,
    ) -> dict[str, Any]:
        return await self.environment.list_with_options(
            path,
            max_entries=max_entries,
            ignore_patterns=ignore_patterns,
        )

    async def list_dir_with_types(
        self,
        path: str = "",
        *,
        max_entries: int = 0,
        ignore_patterns: Sequence[str] | None = None,
    ) -> list[dict[str, Any]]:
        listing = await self.list_with_options(
            path,
            max_entries=max_entries,
            ignore_patterns=ignore_patterns,
        )
        entries: list[dict[str, Any]] = []
        for name in listing.get("entries", []):
            child_path = _join_provider_path(path, str(name))
            stat = await self.environment.stat(child_path)
            entries.append({"name": str(name), "path": child_path, **stat})
        return entries

    async def walk_files(
        self,
        root: str = "",
        *,
        include_hidden: bool = False,
        include_ignored: bool = False,
        max_results: int = 500,
    ) -> AsyncIterator[dict[str, Any]]:
        matches = await self.environment.glob(
            "**/*",
            path=root,
            include_hidden=include_hidden,
            include_ignored=include_ignored,
            max_results=max_results,
        )
        for match in matches:
            path = str(match["path"])
            yield {"path": path, **await self.environment.stat(path)}

    async def truncate_to_tmp(
        self,
        content: str | bytes,
        *,
        suffix: str = ".txt",
    ) -> ResourceRef:
        filename = f"{uuid.uuid4().hex}{suffix}"
        path = await self.environment.write_tmp_file(filename, content)
        return ResourceRef.typed(path, kind="file", metadata={"path": path})


class Shell:
    """Application-facing foreground shell facade over an environment provider."""

    def __init__(self, environment: EnvironmentProvider) -> None:
        self.environment = environment

    async def execute(
        self,
        command: str,
        *,
        timeout_seconds: int | None = None,
        cwd: str | None = None,
        environment: Mapping[str, str] | None = None,
    ) -> dict[str, Any]:
        return await self.environment.run_shell(
            command,
            timeout_seconds=timeout_seconds,
            cwd=cwd,
            environment=environment,
        )

    async def start(
        self,
        command: str,
        *,
        timeout_seconds: int | None = None,
        cwd: str | None = None,
        environment: Mapping[str, str] | None = None,
    ) -> ShellProcess:
        return await self.environment.start_process(
            command,
            timeout_seconds=timeout_seconds,
            cwd=cwd,
            environment=environment,
        )

    async def wait_process(
        self,
        handle: ShellProcess | str,
        *,
        timeout_seconds: int = 0,
    ) -> ShellProcess:
        return await self.environment.wait_process(handle, timeout_seconds=timeout_seconds)

    async def list_processes(self) -> list[ShellProcess]:
        return await self.environment.list_processes()

    async def kill_process(self, handle: ShellProcess | str) -> ShellProcess:
        return await self.environment.kill_process(handle)

    async def write_stdin(
        self,
        handle: ShellProcess | str,
        data: str,
        *,
        close_stdin: bool = False,
    ) -> ShellProcess:
        return await self.environment.input_process(handle, data, close_stdin=close_stdin)

    async def send_signal(self, handle: ShellProcess | str, signal: int | str) -> ShellProcess:
        return await self.environment.signal_process(handle, signal)


def ensure_environment_provider(
    value: EnvironmentProvider | _native.EnvironmentProvider | None,
) -> _native.EnvironmentProvider | None:
    if value is None:
        return None
    to_native = getattr(value, "to_native", None)
    if callable(to_native):
        return cast(_native.EnvironmentProvider, to_native())
    if isinstance(value, _native.EnvironmentProvider):
        return value
    raise TypeError("environment must be an EnvironmentProvider")


def _join_provider_path(root: str, name: str) -> str:
    root = root.strip("/")
    return name if not root else f"{root}/{name}"


def _process_id(process: ShellProcess | str) -> str:
    return process.process_id if isinstance(process, ShellProcess) else str(process)


def _signal_number(signal: int | str) -> int:
    if isinstance(signal, int):
        return signal
    name = signal.upper()
    if not name.startswith("SIG"):
        name = f"SIG{name}"
    return Signals[name].value
