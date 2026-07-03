"""Durable session-store facades over JSON-compatible Starweaver records."""

from __future__ import annotations

import copy
import json
import uuid
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from datetime import UTC, datetime
from os import PathLike
from pathlib import Path
from typing import Any

from .agent import AgentSession, SessionArchive
from .errors import StateError

JsonObject = dict[str, Any]


def _now() -> str:
    return datetime.now(UTC).isoformat()


def _copy(value: Mapping[str, Any]) -> JsonObject:
    return copy.deepcopy(dict(value))


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
                "status": "active",
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
        input_parts: Sequence[Mapping[str, Any]] = (),
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
        now = _now()
        return cls(
            {
                "session_id": session_id,
                "run_id": str(run_id),
                "conversation_id": str(conversation_id),
                "input": [dict(part) for part in input_parts],
                "status": getattr(result, "status", "completed"),
                "output_preview": getattr(result, "output", None),
                "structured_output": getattr(result, "structured_output", None),
                "stream_cursors": [],
                "sequence_no": sequence_no,
                "created_at": now,
                "updated_at": now,
                "metadata": dict(metadata or {}),
            }
        )

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
    latest_checkpoint: CheckpointRef | None = None
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
                self.latest_checkpoint.to_dict() if self.latest_checkpoint is not None else None
            ),
            "stream_records": [record.to_dict() for record in self.stream_records],
            "approvals": [record.to_dict() for record in self.approvals],
            "deferred_tools": [record.to_dict() for record in self.deferred_tools],
            "stream_cursors": copy.deepcopy(self.stream_cursors),
        }


class SessionStore:
    """Abstract Python session-store facade."""

    async def save_session(self, record: SessionRecord | Mapping[str, Any]) -> None:
        raise NotImplementedError

    async def load_session(self, session_id: str) -> SessionRecord:
        raise NotImplementedError

    async def list_sessions(self) -> list[SessionRecord]:
        raise NotImplementedError

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

    async def append_run(self, record: RunRecord | Mapping[str, Any]) -> None:
        raise NotImplementedError

    async def load_run(self, session_id: str, run_id: str) -> RunRecord:
        raise NotImplementedError

    async def list_runs(self, session_id: str) -> list[RunRecord]:
        raise NotImplementedError

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
        if run is not None:
            records = await self.replay_stream_records(session_id, run.run_id)
            approvals = await self.load_approvals(session_id, run.run_id)
            deferred = await self.load_deferred_tools(session_id, run.run_id)
        return SessionResumeSnapshot(
            state=session.state,
            session=session,
            run=run,
            latest_checkpoint=None,
            stream_records=records,
            approvals=approvals,
            deferred_tools=deferred,
            stream_cursors=list(session_raw.get("stream_cursors") or []),
        )

    async def save_archive(self, archive: SessionArchive | Mapping[str, Any]) -> None:
        archive = (
            archive if isinstance(archive, SessionArchive) else SessionArchive.from_dict(archive)
        )
        record = SessionRecord.from_state(archive.state)
        await self.save_session(record)

    async def load_archive(self, session_id: str) -> SessionArchive:
        record = await self.load_session(session_id)
        return SessionArchive.from_state(record.state, mode="full")

    async def save_current_session(self, session: AgentSession) -> SessionRecord:
        archive = SessionArchive.from_session(session, mode="full")
        record = SessionRecord.from_state(archive.state)
        await self.save_session(record)
        return record


class InMemorySessionStore(SessionStore):
    """Deterministic in-process JSON session store."""

    def __init__(self) -> None:
        self._sessions: dict[str, JsonObject] = {}
        self._runs: dict[tuple[str, str], JsonObject] = {}
        self._streams: dict[tuple[str, str], list[JsonObject]] = {}
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

    async def list_sessions(self) -> list[SessionRecord]:
        return [SessionRecord(_copy(record)) for record in self._sessions.values()]

    async def append_run(self, record: RunRecord | Mapping[str, Any]) -> None:
        record = _ensure_run_record(record)
        self._runs[(record.session_id, record.run_id)] = record.to_dict()
        if record.session_id in self._sessions:
            session = self._sessions[record.session_id]
            session["head_run_id"] = record.run_id
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
            "streams": {
                f"{sid}:{rid}": copy.deepcopy(records)
                for (sid, rid), records in self._streams.items()
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
        store._streams = _keyed_record_lists(raw.get("streams"))
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
            self._streams = loaded._streams
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

    async def append_stream_records(
        self,
        session_id: str,
        run_id: str,
        records: Sequence[StreamRecord | Mapping[str, Any]],
    ) -> None:
        await super().append_stream_records(session_id, run_id, records)
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


def _ensure_session_record(record: SessionRecord | Mapping[str, Any]) -> SessionRecord:
    return record if isinstance(record, SessionRecord) else SessionRecord(_copy(record))


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
