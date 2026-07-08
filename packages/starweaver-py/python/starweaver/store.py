"""Durable session-store facades over JSON-compatible Starweaver records."""

from __future__ import annotations

import asyncio
import copy
import json
import uuid
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from datetime import UTC, datetime
from enum import StrEnum
from os import PathLike
from pathlib import Path
from typing import Any
from urllib.parse import unquote, urlparse

from . import _native
from .agent import AgentSession, SessionArchive
from .errors import StateError
from .resources import ResourceRef, ensure_resource_ref

JsonObject = dict[str, Any]
SESSION_ARCHIVE_REQUIRED_TOOLSET_IDS_KEY = "starweaver.required_toolset_ids"


class SessionStatus(StrEnum):
    """Canonical durable session status values."""

    ACTIVE = "active"
    ARCHIVED = "archived"
    FAILED = "failed"

    @classmethod
    def from_value(cls, value: object) -> SessionStatus:
        try:
            return cls(str(value))
        except ValueError as error:
            raise ValueError(f"unknown session status: {value!r}") from error


class RunStatus(StrEnum):
    """Canonical durable run status values."""

    QUEUED = "queued"
    RUNNING = "running"
    WAITING = "waiting"
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELLED = "cancelled"

    @classmethod
    def from_value(cls, value: object) -> RunStatus:
        try:
            return cls(str(value))
        except ValueError as error:
            raise ValueError(f"unknown run status: {value!r}") from error


class ExecutionStatus(StrEnum):
    """Canonical execution status for approval, deferred, and archive records."""

    PENDING = "pending"
    RUNNING = "running"
    WAITING = "waiting"
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELLED = "cancelled"

    @classmethod
    def from_value(cls, value: object) -> ExecutionStatus:
        try:
            return cls(str(value))
        except ValueError as error:
            raise ValueError(f"unknown execution status: {value!r}") from error


def _sqlite_database_path(path: str | PathLike[str]) -> Path:
    if not isinstance(path, str):
        return Path(path)
    if path.startswith("sqlite:///"):
        database = unquote(path[len("sqlite:///") :])
        if not database:
            raise ValueError("sqlite URL must include a database path")
        return Path(database)
    if path.startswith("sqlite://"):
        raise ValueError("sqlite URL must use sqlite:///path")
    parsed = urlparse(path)
    if parsed.scheme == "file":
        if parsed.netloc not in {"", "localhost"}:
            raise ValueError("file URL must reference a local path")
        if parsed.params or parsed.query or parsed.fragment:
            raise ValueError("sqlite file URL must not include params, query, or fragment")
        return Path(unquote(parsed.path))
    if parsed.scheme:
        raise ValueError(f"unsupported sqlite database URL scheme: {parsed.scheme}")
    return Path(path)


def _now() -> str:
    return datetime.now(UTC).isoformat()


def _copy(value: Mapping[str, Any]) -> JsonObject:
    return copy.deepcopy(dict(value))


def _mapping_field(
    raw: Mapping[str, Any],
    key: str,
    *,
    default: Mapping[str, Any] | None = None,
) -> JsonObject:
    value = raw.get(key, default)
    if not isinstance(value, Mapping):
        raise StateError(f"{key} must be an object")
    return _copy(value)


def _optional_mapping_field(raw: Mapping[str, Any], key: str) -> JsonObject | None:
    value = raw.get(key)
    if value is None:
        return None
    if not isinstance(value, Mapping):
        raise StateError(f"{key} must be an object")
    return _copy(value)


def _json_object_list(value: object, key: str) -> list[JsonObject]:
    if value is None:
        return []
    if not isinstance(value, Sequence) or isinstance(value, (str, bytes, bytearray)):
        raise StateError(f"{key} must be a list")
    items: list[JsonObject] = []
    for item in value:
        if not isinstance(item, Mapping):
            raise StateError(f"{key} entries must be objects")
        items.append(_copy(item))
    return items


def _jsonify(value: object) -> object:
    to_dict = getattr(value, "to_dict", None)
    if callable(to_dict):
        return _jsonify(to_dict())
    if isinstance(value, Mapping):
        return {str(key): _jsonify(item) for key, item in value.items()}
    if isinstance(value, Sequence) and not isinstance(value, (str, bytes, bytearray)):
        return [_jsonify(item) for item in value]
    return value


@dataclass(frozen=True)
class InputPart:
    """Typed wrapper over a canonical durable input part JSON object."""

    raw: JsonObject

    @classmethod
    def from_raw(cls, raw: Mapping[str, Any]) -> InputPart:
        return cls(_copy(raw))

    @classmethod
    def text(
        cls,
        text: str,
        *,
        metadata: Mapping[str, Any] | None = None,
    ) -> InputPart:
        return cls(_with_metadata({"kind": "text", "text": text}, metadata))

    @classmethod
    def url(
        cls,
        url: str,
        *,
        metadata: Mapping[str, Any] | None = None,
    ) -> InputPart:
        return cls(_with_metadata({"kind": "url", "url": url}, metadata))

    @classmethod
    def file(
        cls,
        uri: str | ResourceRef | Mapping[str, Any],
        *,
        media_type: str | None = None,
        name: str | None = None,
        metadata: Mapping[str, Any] | None = None,
    ) -> InputPart:
        uri, resource_metadata = _input_resource(uri, metadata)
        if media_type is None:
            media_type = _metadata_str(resource_metadata, "media_type")
        if name is None:
            name = _metadata_str(resource_metadata, "name")
        file_ref: JsonObject = {"uri": uri}
        if media_type is not None:
            file_ref["media_type"] = media_type
        if name is not None:
            file_ref["name"] = name
        return cls(_with_metadata({"kind": "file", "file": file_ref}, resource_metadata))

    @classmethod
    def binary(
        cls,
        uri: str | ResourceRef | Mapping[str, Any],
        *,
        media_type: str | None = None,
        bytes: int | None = None,  # noqa: A002
        metadata: Mapping[str, Any] | None = None,
    ) -> InputPart:
        uri, resource_metadata = _input_resource(uri, metadata)
        if media_type is None:
            media_type = _metadata_str(resource_metadata, "media_type")
        byte_count = bytes
        if byte_count is None:
            byte_count = _metadata_int(resource_metadata, "bytes")
        binary_ref: JsonObject = {"uri": uri}
        if media_type is not None:
            binary_ref["media_type"] = media_type
        if byte_count is not None:
            binary_ref["bytes"] = byte_count
        return cls(_with_metadata({"kind": "binary", "binary": binary_ref}, resource_metadata))

    @classmethod
    def mode(
        cls,
        mode: str,
        *,
        config: Any = None,
        metadata: Mapping[str, Any] | None = None,
    ) -> InputPart:
        payload: JsonObject = {"kind": "mode", "mode": mode}
        if config is not None:
            payload["config"] = _jsonify(config)
        return cls(_with_metadata(payload, metadata))

    @classmethod
    def command(
        cls,
        command: str,
        args: Sequence[str] = (),
        *,
        payload: Any = None,
        metadata: Mapping[str, Any] | None = None,
    ) -> InputPart:
        data: JsonObject = {"kind": "command", "command": command}
        if args:
            data["args"] = [str(arg) for arg in args]
        if payload is not None:
            data["payload"] = _jsonify(payload)
        return cls(_with_metadata(data, metadata))

    @property
    def kind(self) -> str:
        return str(self.raw.get("kind") or "")

    @property
    def metadata(self) -> JsonObject:
        value = self.raw.get("metadata")
        return _copy(value) if isinstance(value, Mapping) else {}

    def to_dict(self) -> JsonObject:
        return _copy(self.raw)


@dataclass(frozen=True)
class SessionRecord:
    """Typed wrapper over a canonical session record JSON object."""

    raw: JsonObject

    @classmethod
    def from_state(
        cls,
        state: Mapping[str, Any],
        *,
        title: str | None = None,
        workspace: str | None = None,
        profile: str | None = None,
        metadata: Mapping[str, Any] | None = None,
    ) -> SessionRecord:
        now = _now()
        session_id = state.get("session_id") or state.get("conversation_id") or uuid.uuid4().hex
        return cls(
            {
                "session_id": str(session_id),
                "title": title,
                "workspace": workspace,
                "profile": profile,
                "status": SessionStatus.ACTIVE.value,
                "state": _copy(state),
                "stream_cursors": [],
                "created_at": now,
                "updated_at": now,
                "metadata": dict(metadata or {}),
            }
        )

    @property
    def session_id(self) -> str:
        return str(self.raw["session_id"])

    @property
    def state(self) -> JsonObject:
        return _copy(self.raw.get("state") or {})

    def to_dict(self) -> JsonObject:
        return _copy(self.raw)


@dataclass(frozen=True)
class RunRecord:
    """Typed wrapper over a canonical run record JSON object."""

    raw: JsonObject

    @classmethod
    def from_result(
        cls,
        session_id: str,
        result: Any,
        *,
        input_parts: Sequence[InputPart | Mapping[str, Any]] = (),
        sequence_no: int = 0,
        metadata: Mapping[str, Any] | None = None,
    ) -> RunRecord:
        raw_state = getattr(result, "raw_state", None) or getattr(result, "raw_run_state", None)
        if not isinstance(raw_state, Mapping):
            raise StateError("run result must expose raw_state")
        run_id = raw_state.get("run_id")
        conversation_id = raw_state.get("conversation_id")
        if run_id is None or conversation_id is None:
            raise StateError("run state must include run_id and conversation_id")
        raw: JsonObject = {
            "session_id": session_id,
            "run_id": str(run_id),
            "conversation_id": str(conversation_id),
            "input": [_input_part_dict(part) for part in input_parts],
            "status": _run_status_value(getattr(result, "status", RunStatus.COMPLETED)),
            "output_preview": getattr(result, "output", None),
            "structured_output": getattr(result, "structured_output", None),
            "stream_cursors": [],
            "sequence_no": sequence_no,
            "created_at": _now(),
            "updated_at": _now(),
            "metadata": dict(metadata or {}),
        }
        parent_run_id = raw_state.get("parent_run_id")
        if parent_run_id is not None:
            raw["parent_run_id"] = str(parent_run_id)
        parent_task_id = raw_state.get("parent_task_id")
        if parent_task_id is not None:
            raw["parent_task_id"] = str(parent_task_id)
        return cls(raw)

    @property
    def session_id(self) -> str:
        return str(self.raw["session_id"])

    @property
    def run_id(self) -> str:
        return str(self.raw["run_id"])

    def to_dict(self) -> JsonObject:
        return _copy(self.raw)


@dataclass(frozen=True)
class StreamRecord:
    """Typed wrapper over one raw stream record."""

    raw: JsonObject

    @property
    def sequence(self) -> int:
        value = self.raw.get("sequence", 0)
        return int(value) if isinstance(value, int | float | str) else 0

    def to_dict(self) -> JsonObject:
        return _copy(self.raw)


@dataclass(frozen=True)
class CheckpointRef:
    """Typed wrapper over checkpoint JSON evidence."""

    raw: JsonObject

    def to_dict(self) -> JsonObject:
        return _copy(self.raw)


@dataclass(frozen=True)
class ApprovalRecord:
    """Typed wrapper over approval record JSON."""

    raw: JsonObject

    def to_dict(self) -> JsonObject:
        return _copy(self.raw)


@dataclass(frozen=True)
class DeferredToolRecord:
    """Typed wrapper over deferred tool record JSON."""

    raw: JsonObject

    def to_dict(self) -> JsonObject:
        return _copy(self.raw)


@dataclass(frozen=True)
class SessionResumeSnapshot:
    """Store resume snapshot with typed wrappers and raw payloads."""

    state: JsonObject
    session: SessionRecord
    run: RunRecord | None = None
    latest_checkpoint: JsonObject | None = None
    stream_records: list[StreamRecord] = field(default_factory=list)
    approvals: list[ApprovalRecord] = field(default_factory=list)
    deferred_tools: list[DeferredToolRecord] = field(default_factory=list)
    stream_cursors: list[JsonObject] = field(default_factory=list)

    def to_dict(self) -> JsonObject:
        return {
            "state": _copy(self.state),
            "session": self.session.to_dict(),
            "run": self.run.to_dict() if self.run is not None else None,
            "latest_checkpoint": (
                _copy(self.latest_checkpoint) if self.latest_checkpoint is not None else None
            ),
            "stream_records": [record.to_dict() for record in self.stream_records],
            "approvals": [record.to_dict() for record in self.approvals],
            "deferred_tools": [record.to_dict() for record in self.deferred_tools],
            "stream_cursors": copy.deepcopy(self.stream_cursors),
        }


class SessionStore:
    """Abstract Python session-store facade."""

    def to_native(self) -> _native.PythonSessionStore | _native.SqliteSessionStore:
        """Adapt this Python store into Starweaver's native SessionStore trait."""

        return _native.PythonSessionStore(self, asyncio.get_running_loop())

    async def save_session(self, record: SessionRecord | Mapping[str, Any]) -> None:
        raise NotImplementedError

    async def load_session(self, session_id: str) -> SessionRecord:
        raise NotImplementedError

    async def list_sessions(
        self,
        filter: Mapping[str, Any] | None = None,  # noqa: A002
    ) -> list[SessionRecord]:
        raise NotImplementedError

    async def update_session_status(self, session_id: str, status: SessionStatus | str) -> None:
        record = await self.load_session(session_id)
        raw = record.to_dict()
        raw["status"] = _session_status_value(status)
        raw["updated_at"] = _now()
        await self.save_session(SessionRecord(raw))

    async def save_context_state(
        self,
        session_id: str,
        state: Mapping[str, Any],
    ) -> None:
        record = await self.load_session(session_id)
        raw = record.to_dict()
        raw["state"] = _copy(state)
        raw["updated_at"] = _now()
        await self.save_session(SessionRecord(raw))

    async def save_environment_state(
        self,
        session_id: str,
        environment_state: Mapping[str, Any],
    ) -> None:
        record = await self.load_session(session_id)
        raw = record.to_dict()
        raw["environment_state"] = _copy(environment_state)
        raw["updated_at"] = _now()
        await self.save_session(SessionRecord(raw))

    async def append_run(self, record: RunRecord | Mapping[str, Any]) -> None:
        raise NotImplementedError

    async def load_run(self, session_id: str, run_id: str) -> RunRecord:
        raise NotImplementedError

    async def list_runs(self, session_id: str) -> list[RunRecord]:
        raise NotImplementedError

    async def update_run_status(
        self,
        session_id: str,
        run_id: str,
        status: RunStatus | str,
        output_preview: str | None = None,
    ) -> None:
        record = await self.load_run(session_id, run_id)
        raw = record.to_dict()
        raw["status"] = _run_status_value(status)
        if output_preview is not None:
            raw["output_preview"] = output_preview
        raw["updated_at"] = _now()
        await self.append_run(RunRecord(raw))

    async def append_checkpoint(
        self,
        session_id: str,
        checkpoint: Mapping[str, Any],
    ) -> None:
        raise NotImplementedError

    async def load_checkpoints(
        self,
        session_id: str,
        run_id: str,
    ) -> list[JsonObject]:
        raise NotImplementedError

    async def latest_checkpoint(
        self,
        session_id: str,
        run_id: str,
    ) -> JsonObject | None:
        checkpoints = await self.load_checkpoints(session_id, run_id)
        return checkpoints[-1] if checkpoints else None

    async def append_stream_records(
        self,
        session_id: str,
        run_id: str,
        records: Sequence[StreamRecord | Mapping[str, Any]],
    ) -> None:
        raise NotImplementedError

    async def replay_stream_records(
        self,
        session_id: str,
        run_id: str,
        after_sequence: int | None = None,
    ) -> list[StreamRecord]:
        raise NotImplementedError

    async def save_stream_cursor(
        self,
        session_id: str,
        run_id: str,
        cursor: Mapping[str, Any],
    ) -> None:
        raise NotImplementedError

    async def append_approval(self, record: ApprovalRecord | Mapping[str, Any]) -> None:
        raise NotImplementedError

    async def load_approvals(self, session_id: str, run_id: str) -> list[ApprovalRecord]:
        raise NotImplementedError

    async def append_deferred_tool(
        self,
        record: DeferredToolRecord | Mapping[str, Any],
    ) -> None:
        raise NotImplementedError

    async def load_deferred_tools(
        self,
        session_id: str,
        run_id: str,
    ) -> list[DeferredToolRecord]:
        raise NotImplementedError

    async def resume_snapshot(
        self,
        session_id: str,
        run_id: str | None = None,
    ) -> SessionResumeSnapshot:
        session = await self.load_session(session_id)
        session_raw = session.to_dict()
        selected_run_id = (
            run_id or session_raw.get("active_run_id") or session_raw.get("head_run_id")
        )
        run = await self.load_run(session_id, str(selected_run_id)) if selected_run_id else None
        records: list[StreamRecord] = []
        approvals: list[ApprovalRecord] = []
        deferred: list[DeferredToolRecord] = []
        latest_checkpoint: JsonObject | None = None
        if run is not None:
            latest_checkpoint = await self.latest_checkpoint(session_id, run.run_id)
            records = await self.replay_stream_records(session_id, run.run_id)
            approvals = await self.load_approvals(session_id, run.run_id)
            deferred = await self.load_deferred_tools(session_id, run.run_id)
        return SessionResumeSnapshot(
            state=session.state,
            session=session,
            run=run,
            latest_checkpoint=latest_checkpoint,
            stream_records=records,
            approvals=approvals,
            deferred_tools=deferred,
            stream_cursors=_json_object_list(session_raw.get("stream_cursors"), "stream_cursors"),
        )

    async def compact_run_trace(self, session_id: str, run_id: str) -> JsonObject:
        run = await self.load_run(session_id, run_id)
        run_raw = run.to_dict()
        checkpoints = await self.load_checkpoints(session_id, run_id)
        approvals = await self.load_approvals(session_id, run_id)
        deferred_tools = await self.load_deferred_tools(session_id, run_id)
        stream_cursors = _json_object_list(run_raw.get("stream_cursors"), "stream_cursors")
        latest_checkpoint = checkpoints[-1] if checkpoints else None
        latest_checkpoint_id = (
            str(latest_checkpoint.get("checkpoint_id"))
            if isinstance(latest_checkpoint, Mapping)
            else None
        )
        stream_cursor = None
        if latest_checkpoint is not None:
            resume = latest_checkpoint.get("resume")
            if isinstance(resume, Mapping):
                cursor = resume.get("cursor")
                if isinstance(cursor, Mapping):
                    stream_cursor = cursor.get("stream_cursor")
        if stream_cursor is None and stream_cursors:
            stream_cursor = stream_cursors[-1].get("sequence")
        return {
            "session_id": session_id,
            "run_id": run_id,
            "status": str(run_raw.get("status") or "queued"),
            "checkpoints": [
                str(checkpoint.get("checkpoint_id"))
                for checkpoint in checkpoints
                if isinstance(checkpoint.get("checkpoint_id"), str)
            ],
            "approvals": len(approvals),
            "deferred_tools": len(deferred_tools),
            "latest_checkpoint": latest_checkpoint_id,
            "stream_cursor": stream_cursor,
            "stream_cursors": stream_cursors,
            "output_preview": run_raw.get("output_preview"),
            "trace_context": _mapping_field(run_raw, "trace_context", default={}),
            "updated_at": run_raw.get("updated_at"),
            "metadata": _mapping_field(run_raw, "metadata", default={}),
        }

    async def compact_session_trace(self, session_id: str) -> JsonObject:
        session = await self.load_session(session_id)
        session_raw = session.to_dict()
        runs = await self.list_runs(session_id)
        latest_run_id = session_raw.get("head_run_id")
        last_output_preview = None
        if latest_run_id is not None:
            try:
                last_output_preview = (
                    (await self.load_run(session_id, str(latest_run_id)))
                    .to_dict()
                    .get("output_preview")
                )
            except StateError:
                last_output_preview = None
        return {
            "session_id": session_id,
            "title": session_raw.get("title"),
            "workspace": session_raw.get("workspace"),
            "profile": session_raw.get("profile"),
            "status": str(session_raw.get("status") or "active"),
            "runs": len(runs),
            "latest_run_id": latest_run_id,
            "last_output_preview": last_output_preview,
            "stream_cursors": _json_object_list(
                session_raw.get("stream_cursors"),
                "stream_cursors",
            ),
            "trace_context": _mapping_field(session_raw, "trace_context", default={}),
            "created_at": str(session_raw.get("created_at") or _now()),
            "updated_at": str(session_raw.get("updated_at") or _now()),
            "metadata": _mapping_field(session_raw, "metadata", default={}),
        }

    async def save_archive(self, archive: SessionArchive | Mapping[str, Any]) -> None:
        archive = (
            archive if isinstance(archive, SessionArchive) else SessionArchive.from_dict(archive)
        )
        metadata: dict[str, Any] = {}
        if archive.required_toolset_ids:
            metadata[SESSION_ARCHIVE_REQUIRED_TOOLSET_IDS_KEY] = list(archive.required_toolset_ids)
        record = SessionRecord.from_state(archive.state, metadata=metadata)
        await self.save_session(record)

    async def load_archive(self, session_id: str) -> SessionArchive:
        record = await self.load_session(session_id)
        metadata = record.to_dict().get("metadata")
        required_toolset_ids: object = None
        if isinstance(metadata, Mapping):
            required_toolset_ids = metadata.get(SESSION_ARCHIVE_REQUIRED_TOOLSET_IDS_KEY)
        return SessionArchive.from_state(
            record.state,
            mode="full",
            required_toolset_ids=required_toolset_ids,
        )

    async def save_current_session(self, session: AgentSession) -> SessionRecord:
        archive = SessionArchive.from_session(session, mode="full")
        metadata: dict[str, Any] = {}
        if archive.required_toolset_ids:
            metadata[SESSION_ARCHIVE_REQUIRED_TOOLSET_IDS_KEY] = list(archive.required_toolset_ids)
        record = SessionRecord.from_state(archive.state, metadata=metadata)
        await self.save_session(record)
        return record


class InMemorySessionStore(SessionStore):
    """Deterministic in-process JSON session store."""

    def __init__(self) -> None:
        self._sessions: dict[str, JsonObject] = {}
        self._runs: dict[tuple[str, str], JsonObject] = {}
        self._checkpoints: dict[tuple[str, str], list[JsonObject]] = {}
        self._streams: dict[tuple[str, str], list[JsonObject]] = {}
        self._stream_cursors: dict[tuple[str, str], list[JsonObject]] = {}
        self._approvals: dict[tuple[str, str], list[JsonObject]] = {}
        self._deferred: dict[tuple[str, str], list[JsonObject]] = {}

    async def save_session(self, record: SessionRecord | Mapping[str, Any]) -> None:
        record = _ensure_session_record(record)
        self._sessions[record.session_id] = record.to_dict()

    async def load_session(self, session_id: str) -> SessionRecord:
        try:
            return SessionRecord(_copy(self._sessions[session_id]))
        except KeyError as error:
            raise StateError(f"unknown session: {session_id}") from error

    async def list_sessions(
        self,
        filter: Mapping[str, Any] | None = None,  # noqa: A002
    ) -> list[SessionRecord]:
        raw_filter = dict(filter or {})
        records = list(self._sessions.values())
        if status := raw_filter.get("status"):
            records = [record for record in records if record.get("status") == status]
        if profile := raw_filter.get("profile"):
            records = [record for record in records if record.get("profile") == profile]
        if workspace := raw_filter.get("workspace"):
            records = [record for record in records if record.get("workspace") == workspace]
        limit = raw_filter.get("limit")
        if isinstance(limit, int):
            records = records[:limit]
        return [SessionRecord(_copy(record)) for record in records]

    async def append_run(self, record: RunRecord | Mapping[str, Any]) -> None:
        record = _ensure_run_record(record)
        self._runs[(record.session_id, record.run_id)] = record.to_dict()
        if record.session_id in self._sessions:
            session = self._sessions[record.session_id]
            session["head_run_id"] = record.run_id
            session["active_run_id"] = (
                None if record.raw.get("status") == "completed" else record.run_id
            )
            if record.raw.get("status") == "completed":
                session["head_success_run_id"] = record.run_id
            session["updated_at"] = _now()

    async def load_run(self, session_id: str, run_id: str) -> RunRecord:
        try:
            return RunRecord(_copy(self._runs[(session_id, run_id)]))
        except KeyError as error:
            raise StateError(f"unknown run: {session_id}:{run_id}") from error

    async def list_runs(self, session_id: str) -> list[RunRecord]:
        return [
            RunRecord(_copy(record))
            for (record_session_id, _), record in self._runs.items()
            if record_session_id == session_id
        ]

    async def append_checkpoint(
        self,
        session_id: str,
        checkpoint: Mapping[str, Any],
    ) -> None:
        run_id = checkpoint.get("run_id")
        if run_id is None:
            raise StateError("checkpoint must include run_id")
        key = (session_id, str(run_id))
        checkpoints = self._checkpoints.setdefault(key, [])
        checkpoints.append(_copy(checkpoint))
        checkpoints.sort(key=lambda record: int(record.get("run_step", 0)))
        if key in self._runs:
            latest = checkpoints[-1]
            run = self._runs[key]
            run["latest_checkpoint"] = {
                "checkpoint_id": latest.get("checkpoint_id"),
                "run_id": latest.get("run_id"),
                "sequence": len(checkpoints) - 1,
                "node": latest.get("node"),
                "stream_cursor": (
                    latest.get("resume", {}).get("cursor", {}).get("stream_cursor")
                    if isinstance(latest.get("resume"), Mapping)
                    else None
                ),
                "created_at": _now(),
                "metadata": {},
            }
            run["updated_at"] = _now()

    async def load_checkpoints(
        self,
        session_id: str,
        run_id: str,
    ) -> list[JsonObject]:
        return [_copy(record) for record in self._checkpoints.get((session_id, run_id), [])]

    async def append_stream_records(
        self,
        session_id: str,
        run_id: str,
        records: Sequence[StreamRecord | Mapping[str, Any]],
    ) -> None:
        stream = self._streams.setdefault((session_id, run_id), [])
        stream.extend(_ensure_stream_record(record).to_dict() for record in records)
        stream.sort(key=lambda record: int(record.get("sequence", 0)))

    async def replay_stream_records(
        self,
        session_id: str,
        run_id: str,
        after_sequence: int | None = None,
    ) -> list[StreamRecord]:
        records = self._streams.get((session_id, run_id), [])
        return [
            StreamRecord(_copy(record))
            for record in records
            if after_sequence is None or int(record.get("sequence", 0)) > after_sequence
        ]

    async def save_stream_cursor(
        self,
        session_id: str,
        run_id: str,
        cursor: Mapping[str, Any],
    ) -> None:
        cursor_record = _copy(cursor)
        self._stream_cursors.setdefault((session_id, run_id), []).append(cursor_record)
        if session_id in self._sessions:
            session = self._sessions[session_id]
            session.setdefault("stream_cursors", []).append(cursor_record)
            session["updated_at"] = _now()
        if (session_id, run_id) in self._runs:
            run = self._runs[(session_id, run_id)]
            run.setdefault("stream_cursors", []).append(cursor_record)
            run["updated_at"] = _now()

    async def append_approval(self, record: ApprovalRecord | Mapping[str, Any]) -> None:
        record = _ensure_approval_record(record)
        key = _record_key(record.raw)
        self._approvals.setdefault(key, []).append(record.to_dict())

    async def load_approvals(self, session_id: str, run_id: str) -> list[ApprovalRecord]:
        return [
            ApprovalRecord(_copy(record))
            for record in self._approvals.get((session_id, run_id), [])
        ]

    async def append_deferred_tool(
        self,
        record: DeferredToolRecord | Mapping[str, Any],
    ) -> None:
        record = _ensure_deferred_record(record)
        key = _record_key(record.raw)
        self._deferred.setdefault(key, []).append(record.to_dict())

    async def load_deferred_tools(
        self,
        session_id: str,
        run_id: str,
    ) -> list[DeferredToolRecord]:
        return [
            DeferredToolRecord(_copy(record))
            for record in self._deferred.get((session_id, run_id), [])
        ]

    def to_dict(self) -> JsonObject:
        return {
            "sessions": copy.deepcopy(self._sessions),
            "runs": {f"{sid}:{rid}": _copy(record) for (sid, rid), record in self._runs.items()},
            "checkpoints": {
                f"{sid}:{rid}": copy.deepcopy(records)
                for (sid, rid), records in self._checkpoints.items()
            },
            "streams": {
                f"{sid}:{rid}": copy.deepcopy(records)
                for (sid, rid), records in self._streams.items()
            },
            "stream_cursors": {
                f"{sid}:{rid}": copy.deepcopy(records)
                for (sid, rid), records in self._stream_cursors.items()
            },
            "approvals": {
                f"{sid}:{rid}": copy.deepcopy(records)
                for (sid, rid), records in self._approvals.items()
            },
            "deferred": {
                f"{sid}:{rid}": copy.deepcopy(records)
                for (sid, rid), records in self._deferred.items()
            },
        }

    @classmethod
    def from_dict(cls, raw: Mapping[str, Any]) -> InMemorySessionStore:
        store = cls()
        store._sessions = {
            str(session_id): _copy(record)
            for session_id, record in dict(raw.get("sessions") or {}).items()
            if isinstance(record, Mapping)
        }
        store._runs = _keyed_records(raw.get("runs"))
        store._checkpoints = _keyed_record_lists(raw.get("checkpoints"))
        store._streams = _keyed_record_lists(raw.get("streams"))
        store._stream_cursors = _keyed_record_lists(raw.get("stream_cursors"))
        store._approvals = _keyed_record_lists(raw.get("approvals"))
        store._deferred = _keyed_record_lists(raw.get("deferred"))
        return store


class JsonSessionStore(InMemorySessionStore):
    """Single-file JSON session store for local development and tests."""

    def __init__(self, path: str | PathLike[str]) -> None:
        self.path = Path(path)
        if self.path.exists():
            loaded = InMemorySessionStore.from_dict(
                json.loads(self.path.read_text(encoding="utf-8"))
            )
            self._sessions = loaded._sessions
            self._runs = loaded._runs
            self._checkpoints = loaded._checkpoints
            self._streams = loaded._streams
            self._stream_cursors = loaded._stream_cursors
            self._approvals = loaded._approvals
            self._deferred = loaded._deferred
        else:
            super().__init__()

    async def flush(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_text(
            json.dumps(self.to_dict(), indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )

    async def save_session(self, record: SessionRecord | Mapping[str, Any]) -> None:
        await super().save_session(record)
        await self.flush()

    async def save_context_state(self, session_id: str, state: Mapping[str, Any]) -> None:
        await super().save_context_state(session_id, state)
        await self.flush()

    async def append_run(self, record: RunRecord | Mapping[str, Any]) -> None:
        await super().append_run(record)
        await self.flush()

    async def append_checkpoint(
        self,
        session_id: str,
        checkpoint: Mapping[str, Any],
    ) -> None:
        await super().append_checkpoint(session_id, checkpoint)
        await self.flush()

    async def append_stream_records(
        self,
        session_id: str,
        run_id: str,
        records: Sequence[StreamRecord | Mapping[str, Any]],
    ) -> None:
        await super().append_stream_records(session_id, run_id, records)
        await self.flush()

    async def save_stream_cursor(
        self,
        session_id: str,
        run_id: str,
        cursor: Mapping[str, Any],
    ) -> None:
        await super().save_stream_cursor(session_id, run_id, cursor)
        await self.flush()

    async def append_approval(self, record: ApprovalRecord | Mapping[str, Any]) -> None:
        await super().append_approval(record)
        await self.flush()

    async def append_deferred_tool(
        self,
        record: DeferredToolRecord | Mapping[str, Any],
    ) -> None:
        await super().append_deferred_tool(record)
        await self.flush()


class SqliteSessionStore(SessionStore):
    """Native SQLite session store backed by `starweaver-storage` migrations."""

    def __init__(
        self,
        path: str | PathLike[str],
        *,
        _native_store: _native.SqliteSessionStore | None = None,
    ) -> None:
        self.path = _sqlite_database_path(path)
        self._native = _native_store or _native.SqliteSessionStore(str(self.path))

    def to_native(self) -> _native.SqliteSessionStore:
        return self._native

    @classmethod
    def open(cls, path: str | PathLike[str]) -> SqliteSessionStore:
        return cls(path)

    @classmethod
    def in_memory(cls) -> SqliteSessionStore:
        instance = cls.__new__(cls)
        instance.path = Path(":memory:")
        instance._native = _native.SqliteSessionStore.in_memory()
        return instance

    @staticmethod
    def migrate(path: str | PathLike[str]) -> list[str]:
        return list(_native.SqliteSessionStore.migrate(str(_sqlite_database_path(path))))

    @staticmethod
    def migration_status(path: str | PathLike[str]) -> dict[str, Any]:
        return dict(_native.SqliteSessionStore.migration_status(str(_sqlite_database_path(path))))

    async def save_session(self, record: SessionRecord | Mapping[str, Any]) -> None:
        await self._native.save_session(_ensure_session_record(record).to_dict())

    async def load_session(self, session_id: str) -> SessionRecord:
        return SessionRecord(await self._native.load_session(session_id))

    async def list_sessions(
        self,
        filter: Mapping[str, Any] | None = None,  # noqa: A002
    ) -> list[SessionRecord]:
        return [SessionRecord(record) for record in await self._native.list_sessions(filter)]

    async def save_context_state(self, session_id: str, state: Mapping[str, Any]) -> None:
        await self._native.save_context_state(session_id, _copy(state))

    async def save_environment_state(
        self,
        session_id: str,
        environment_state: Mapping[str, Any],
    ) -> None:
        await self._native.save_environment_state(session_id, _copy(environment_state))

    async def append_run(self, record: RunRecord | Mapping[str, Any]) -> None:
        await self._native.append_run(_ensure_run_record(record).to_dict())

    async def load_run(self, session_id: str, run_id: str) -> RunRecord:
        return RunRecord(await self._native.load_run(session_id, run_id))

    async def list_runs(self, session_id: str) -> list[RunRecord]:
        return [RunRecord(record) for record in await self._native.list_runs(session_id)]

    async def update_run_status(
        self,
        session_id: str,
        run_id: str,
        status: RunStatus | str,
        output_preview: str | None = None,
    ) -> None:
        await self._native.update_run_status(
            session_id,
            run_id,
            _run_status_value(status),
            output_preview,
        )

    async def append_checkpoint(
        self,
        session_id: str,
        checkpoint: Mapping[str, Any],
    ) -> None:
        await self._native.append_checkpoint(session_id, _copy(checkpoint))

    async def load_checkpoints(
        self,
        session_id: str,
        run_id: str,
    ) -> list[JsonObject]:
        return [dict(record) for record in await self._native.load_checkpoints(session_id, run_id)]

    async def append_stream_records(
        self,
        session_id: str,
        run_id: str,
        records: Sequence[StreamRecord | Mapping[str, Any]],
    ) -> None:
        await self._native.append_stream_records(
            session_id,
            run_id,
            [_ensure_stream_record(record).to_dict() for record in records],
        )

    async def replay_stream_records(
        self,
        session_id: str,
        run_id: str,
        after_sequence: int | None = None,
    ) -> list[StreamRecord]:
        return [
            StreamRecord(record)
            for record in await self._native.replay_stream_records(
                session_id,
                run_id,
                after_sequence,
            )
        ]

    async def save_stream_cursor(
        self,
        session_id: str,
        run_id: str,
        cursor: Mapping[str, Any],
    ) -> None:
        await self._native.save_stream_cursor(session_id, run_id, _copy(cursor))

    async def append_approval(self, record: ApprovalRecord | Mapping[str, Any]) -> None:
        await self._native.append_approval(_ensure_approval_record(record).to_dict())

    async def load_approvals(self, session_id: str, run_id: str) -> list[ApprovalRecord]:
        return [
            ApprovalRecord(record)
            for record in await self._native.load_approvals(session_id, run_id)
        ]

    async def append_deferred_tool(
        self,
        record: DeferredToolRecord | Mapping[str, Any],
    ) -> None:
        await self._native.append_deferred_tool(_ensure_deferred_record(record).to_dict())

    async def load_deferred_tools(
        self,
        session_id: str,
        run_id: str,
    ) -> list[DeferredToolRecord]:
        return [
            DeferredToolRecord(record)
            for record in await self._native.load_deferred_tools(session_id, run_id)
        ]

    async def resume_snapshot(
        self,
        session_id: str,
        run_id: str | None = None,
    ) -> SessionResumeSnapshot:
        session = await self.load_session(session_id)
        session_raw = session.to_dict()
        selected_run_id = (
            run_id or session_raw.get("active_run_id") or session_raw.get("head_run_id")
        )
        if selected_run_id is None:
            return SessionResumeSnapshot(
                state=session.state,
                session=session,
                stream_cursors=_json_object_list(
                    session_raw.get("stream_cursors"),
                    "stream_cursors",
                ),
            )
        raw = await self._native.resume_snapshot(session_id, str(selected_run_id))
        return SessionResumeSnapshot(
            state=_mapping_field(raw, "state", default={}),
            session=SessionRecord(_mapping_field(raw, "session")),
            run=RunRecord(_mapping_field(raw, "run")),
            latest_checkpoint=_optional_mapping_field(raw, "latest_checkpoint"),
            stream_records=[
                StreamRecord(record)
                for record in _json_object_list(raw.get("stream_records"), "stream_records")
            ],
            approvals=[
                ApprovalRecord(record)
                for record in _json_object_list(raw.get("approvals"), "approvals")
            ],
            deferred_tools=[
                DeferredToolRecord(record)
                for record in _json_object_list(raw.get("deferred_tools"), "deferred_tools")
            ],
            stream_cursors=_json_object_list(raw.get("stream_cursors"), "stream_cursors"),
        )

    async def compact_run_trace(self, session_id: str, run_id: str) -> JsonObject:
        return dict(await self._native.compact_run_trace(session_id, run_id))

    async def compact_session_trace(self, session_id: str) -> JsonObject:
        return dict(await self._native.compact_session_trace(session_id))


class SqliteReplayEventLog:
    """Native SQLite replay event log backed by `starweaver-storage`."""

    def __init__(
        self,
        path: str | PathLike[str],
        *,
        _native_log: _native.SqliteReplayEventLog | None = None,
    ) -> None:
        self.path = _sqlite_database_path(path)
        self._native = _native_log or _native.SqliteReplayEventLog(str(self.path))

    @classmethod
    def open(cls, path: str | PathLike[str]) -> SqliteReplayEventLog:
        return cls(path)

    @classmethod
    def in_memory(cls) -> SqliteReplayEventLog:
        instance = cls.__new__(cls)
        instance.path = Path(":memory:")
        instance._native = _native.SqliteReplayEventLog.in_memory()
        return instance

    async def append(self, scope: str, event: Mapping[str, Any]) -> None:
        await self._native.append(scope, _copy(event))

    async def replay_after(
        self,
        scope: str,
        cursor: Mapping[str, Any] | None = None,
        limit: int | None = None,
    ) -> list[JsonObject]:
        return [
            _copy(record)
            for record in await self._native.replay_after(
                scope,
                _copy(cursor) if cursor is not None else None,
                limit,
            )
        ]

    async def compact_snapshot(self, scope: str) -> JsonObject:
        return _copy(await self._native.compact_snapshot(scope))

    async def save_snapshot(self, scope: str, snapshot: Mapping[str, Any]) -> None:
        await self._native.save_snapshot(scope, _copy(snapshot))


class SqliteStreamArchive:
    """Native SQLite stream archive backed by `starweaver-storage`."""

    def __init__(
        self,
        path: str | PathLike[str],
        *,
        _native_archive: _native.SqliteStreamArchive | None = None,
    ) -> None:
        self.path = _sqlite_database_path(path)
        self._native = _native_archive or _native.SqliteStreamArchive(str(self.path))

    @classmethod
    def open(cls, path: str | PathLike[str]) -> SqliteStreamArchive:
        return cls(path)

    @classmethod
    def in_memory(cls) -> SqliteStreamArchive:
        instance = cls.__new__(cls)
        instance.path = Path(":memory:")
        instance._native = _native.SqliteStreamArchive.in_memory()
        return instance

    async def append_raw_records(
        self,
        session_id: str,
        run_id: str,
        records: Sequence[StreamRecord | Mapping[str, Any]],
    ) -> None:
        await self._native.append_raw_records(
            session_id,
            run_id,
            [_ensure_stream_record(record).to_dict() for record in records],
        )

    async def replay_raw_after(
        self,
        session_id: str,
        run_id: str,
        cursor: Mapping[str, Any] | None = None,
    ) -> list[StreamRecord]:
        return [
            StreamRecord(record)
            for record in await self._native.replay_raw_after(
                session_id,
                run_id,
                _copy(cursor) if cursor is not None else None,
            )
        ]

    async def append_display_messages(
        self,
        scope: str,
        messages: Sequence[Mapping[str, Any]],
    ) -> None:
        await self._native.append_display_messages(
            scope,
            [_copy(message) for message in messages],
        )

    async def replay_display_after(
        self,
        scope: str,
        cursor: Mapping[str, Any] | None = None,
    ) -> list[JsonObject]:
        return [
            _copy(message)
            for message in await self._native.replay_display_after(
                scope,
                _copy(cursor) if cursor is not None else None,
            )
        ]

    async def append_snapshot(self, scope: str, snapshot: Mapping[str, Any]) -> None:
        await self._native.append_snapshot(scope, _copy(snapshot))

    async def latest_snapshot(self, scope: str) -> JsonObject | None:
        raw = await self._native.latest_snapshot(scope)
        return _copy(raw) if raw is not None else None

    async def cursor_range(self, scope: str) -> JsonObject | None:
        raw = await self._native.cursor_range(scope)
        return _copy(raw) if raw is not None else None


def _ensure_session_record(record: SessionRecord | Mapping[str, Any]) -> SessionRecord:
    return record if isinstance(record, SessionRecord) else SessionRecord(_copy(record))


def _with_metadata(
    payload: Mapping[str, Any],
    metadata: Mapping[str, Any] | None,
) -> JsonObject:
    result = _copy(payload)
    if metadata is not None:
        result["metadata"] = _copy(metadata)
    return result


def _input_resource(
    value: str | ResourceRef | Mapping[str, Any],
    metadata: Mapping[str, Any] | None,
) -> tuple[str, Mapping[str, Any] | None]:
    if isinstance(value, str):
        return value, metadata
    ref = ensure_resource_ref(value)
    merged = dict(ref.metadata)
    if metadata is not None:
        merged.update(metadata)
    return ref.uri, merged or None


def _metadata_str(metadata: Mapping[str, Any] | None, key: str) -> str | None:
    if metadata is None:
        return None
    value = metadata.get(key)
    return value if isinstance(value, str) and value else None


def _metadata_int(metadata: Mapping[str, Any] | None, key: str) -> int | None:
    if metadata is None:
        return None
    value = metadata.get(key)
    return value if isinstance(value, int) and not isinstance(value, bool) else None


def _input_part_dict(part: InputPart | Mapping[str, Any]) -> JsonObject:
    if isinstance(part, InputPart):
        return part.to_dict()
    raw = _copy(part)
    if not isinstance(raw.get("kind"), str) or not raw["kind"]:
        raise StateError("input part must include kind")
    return raw


def _session_status_value(status: SessionStatus | str) -> str:
    if isinstance(status, SessionStatus):
        return status.value
    return SessionStatus.from_value(status).value


def _run_status_value(status: RunStatus | str) -> str:
    if isinstance(status, RunStatus):
        return status.value
    return RunStatus.from_value(status).value


def _ensure_run_record(record: RunRecord | Mapping[str, Any]) -> RunRecord:
    return record if isinstance(record, RunRecord) else RunRecord(_copy(record))


def _ensure_stream_record(record: StreamRecord | Mapping[str, Any]) -> StreamRecord:
    return record if isinstance(record, StreamRecord) else StreamRecord(_copy(record))


def _ensure_approval_record(record: ApprovalRecord | Mapping[str, Any]) -> ApprovalRecord:
    return record if isinstance(record, ApprovalRecord) else ApprovalRecord(_copy(record))


def _ensure_deferred_record(
    record: DeferredToolRecord | Mapping[str, Any],
) -> DeferredToolRecord:
    return record if isinstance(record, DeferredToolRecord) else DeferredToolRecord(_copy(record))


def _record_key(record: Mapping[str, Any]) -> tuple[str, str]:
    session_id = record.get("session_id")
    run_id = record.get("run_id")
    if session_id is None or run_id is None:
        raise StateError("record must include session_id and run_id")
    return str(session_id), str(run_id)


def _split_key(key: str) -> tuple[str, str]:
    session_id, sep, run_id = key.partition(":")
    if not sep:
        raise StateError(f"invalid store key: {key}")
    return session_id, run_id


def _keyed_records(raw: Any) -> dict[tuple[str, str], JsonObject]:
    if not isinstance(raw, Mapping):
        return {}
    return {
        _split_key(str(key)): _copy(record)
        for key, record in raw.items()
        if isinstance(record, Mapping)
    }


def _keyed_record_lists(raw: Any) -> dict[tuple[str, str], list[JsonObject]]:
    if not isinstance(raw, Mapping):
        return {}
    result: dict[tuple[str, str], list[JsonObject]] = {}
    for key, records in raw.items():
        if not isinstance(records, Sequence) or isinstance(records, (str, bytes, bytearray)):
            continue
        result[_split_key(str(key))] = [
            _copy(record) for record in records if isinstance(record, Mapping)
        ]
    return result
