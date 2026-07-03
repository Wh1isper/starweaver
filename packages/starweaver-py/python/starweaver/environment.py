"""Environment provider facades backed by Starweaver native providers."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from os import PathLike
from pathlib import Path
from typing import Any, cast

from . import _native
from .resources import ResourceRef, ensure_resource_ref


class EnvironmentProvider:
    """Provider-scoped filesystem, shell, resource, and state access."""

    def __init__(self, native: _native.EnvironmentProvider) -> None:
        self._native = native

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

    @property
    def id(self) -> str:
        return self._native.id

    def to_native(self) -> _native.EnvironmentProvider:
        return self._native

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

    async def export_state(self) -> dict[str, Any]:
        return cast(dict[str, Any], await self._native.export_state())


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
