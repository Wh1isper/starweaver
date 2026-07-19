"""Durable session-store facades over JSON-compatible Starweaver records."""

from __future__ import annotations

import asyncio
import copy
import hashlib
import json
import os
import tempfile
import uuid
from collections.abc import Callable, Mapping, Sequence
from contextlib import suppress
from dataclasses import dataclass, field
from datetime import UTC, datetime
from enum import StrEnum
from os import PathLike
from pathlib import Path
from typing import Any, cast
from urllib.parse import unquote, urlparse

from . import _native
from .agent import AgentSession, SessionArchive
from .errors import StateError
from .resources import ResourceRef, ensure_resource_ref

JsonObject = dict[str, Any]
SESSION_ARCHIVE_REQUIRED_TOOLSET_IDS_KEY = "starweaver.required_toolset_ids"
SESSION_STORE_FORMAT = "starweaver.python.session_store"
SESSION_STORE_VERSION = 2
LEGACY_UNSEALED_EVIDENCE_DIGEST = "legacy-unsealed:v1"


def _encode_run_key(session_id: str, run_id: str) -> str:
    return json.dumps([session_id, run_id], ensure_ascii=False, separators=(",", ":"))


def _stream_publication_id(session_id: str, run_id: str) -> str:
    digest = hashlib.sha256()
    for component in (session_id, run_id):
        encoded = component.encode()
        digest.update(str(len(encoded)).encode())
        digest.update(b":")
        digest.update(encoded)
        digest.update(b";")
    return f"publication-sha256:{digest.hexdigest()}"


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


def _merge_stream_cursors(
    session_cursors: Sequence[JsonObject],
    run_cursors: Sequence[JsonObject],
) -> list[JsonObject]:
    merged: list[JsonObject] = []
    positions: dict[str, int] = {}
    for cursor in [*session_cursors, *run_cursors]:
        position = cursor.get("position")
        if not isinstance(position, Mapping):
            raise StateError("stream cursor must include a position object")
        identity = json.dumps(
            [position.get("family"), position.get("scope")],
            sort_keys=True,
            separators=(",", ":"),
        )
        copied = _copy(cursor)
        if identity in positions:
            merged[positions[identity]] = copied
        else:
            positions[identity] = len(merged)
            merged.append(copied)
    return merged


def _validate_approval_transition(
    existing: Mapping[str, Any],
    resolved: Mapping[str, Any],
) -> None:
    immutable_fields = (
        "approval_id",
        "session_id",
        "run_id",
        "action_id",
        "action_name",
        "request",
        "reviewed_arguments",
        "created_at",
        "trace_context",
        "metadata",
    )
    same_request = all(existing.get(field) == resolved.get(field) for field in immutable_fields)
    if (
        existing.get("status") != "pending"
        or resolved.get("status") == "pending"
        or resolved.get("decision") is None
        or not same_request
    ):
        raise StateError(f"approval transition conflict for {resolved.get('approval_id', '')}")


def _validate_deferred_transition(
    existing: Mapping[str, Any],
    resolved: Mapping[str, Any],
) -> None:
    immutable_fields = (
        "deferred_id",
        "session_id",
        "run_id",
        "tool_call_id",
        "tool_name",
        "request",
        "created_at",
        "trace_context",
    )
    same_request = all(existing.get(field) == resolved.get(field) for field in immutable_fields)
    if (
        existing.get("status") not in {"pending", "waiting"}
        or resolved.get("status") in {"pending", "running", "waiting"}
        or not same_request
    ):
        raise StateError(f"deferred tool transition conflict for {resolved.get('deferred_id', '')}")


def _ensure_unique_values(
    records: Sequence[Mapping[str, Any]],
    field: str,
    family: str,
) -> None:
    seen: set[str | int] = set()
    for record in records:
        value = record.get(field)
        if not isinstance(value, (str, int)) or value in seen:
            raise StateError(f"invalid or duplicate {family} {field}: {value}")
        seen.add(value)


def _validate_checkpoint_evidence(
    checkpoints: Sequence[Mapping[str, Any]],
    run: RunRecord,
) -> None:
    for checkpoint in checkpoints:
        state = checkpoint.get("state")
        resume = checkpoint.get("resume")
        if (
            checkpoint.get("run_id") != run.run_id
            or checkpoint.get("conversation_id") != run.conversation_id
            or not isinstance(state, Mapping)
            or state.get("run_id") != run.run_id
            or state.get("conversation_id") != run.conversation_id
            or not isinstance(resume, Mapping)
            or checkpoint.get("node") != resume.get("node")
            or checkpoint.get("run_step") != resume.get("run_step")
            or checkpoint.get("run_step") != state.get("run_step")
        ):
            raise StateError(f"checkpoint identity mismatch for run {run.run_id}")


def _validate_record_ownership(
    records: Sequence[Mapping[str, Any]],
    run: RunRecord,
    family: str,
) -> None:
    if any(
        item.get("session_id") != run.session_id or item.get("run_id") != run.run_id
        for item in records
    ):
        raise StateError(f"{family} identity mismatch for run {run.run_id}")


def _validate_display_session_ownership(
    records: Sequence[Mapping[str, Any]],
    run: RunRecord,
    family: str,
) -> None:
    if any(item.get("session_id") != run.session_id for item in records):
        raise StateError(f"{family} session identity mismatch for run {run.run_id}")


def _optional_metadata_id_matches(metadata: Mapping[str, Any], key: str, expected: str) -> bool:
    value = metadata.get(key)
    return value is None or value == expected


def _validate_related_run_evidence(
    updates: Sequence[Mapping[str, Any]],
    run: RunRecord,
) -> None:
    for update in updates:
        source_run_id = str(update.get("run_id") or "")
        approvals = _json_object_list(update.get("approvals"), "approvals")
        deferred = _json_object_list(update.get("deferred_tools"), "deferred_tools")
        wrong_approval_owner = any(
            item.get("session_id") != run.session_id or item.get("run_id") != source_run_id
            for item in approvals
        )
        wrong_deferred_owner = any(
            item.get("session_id") != run.session_id or item.get("run_id") != source_run_id
            for item in deferred
        )
        if wrong_approval_owner or wrong_deferred_owner:
            raise StateError(
                f"related run evidence identity mismatch for session {run.session_id} "
                f"and run {source_run_id}"
            )
        _ensure_unique_values(approvals, "approval_id", "approval")
        _ensure_unique_values(deferred, "deferred_id", "deferred tool")


def _is_nonnegative_int(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def _validated_stream_cursor_identity(
    cursor: Mapping[str, Any], run_id: str
) -> tuple[tuple[str, str], int]:
    position = cursor.get("position")
    if (
        not isinstance(position, Mapping)
        or not isinstance(position.get("family"), str)
        or not isinstance(position.get("scope"), str)
        or not _is_nonnegative_int(position.get("sequence"))
    ):
        raise StateError("stream cursor must include a valid family, scope, and sequence")
    identity = (position["family"], position["scope"])
    if identity[1] != f"run:{run_id}":
        raise StateError(f"stream cursor scope mismatch for run {run_id}")
    return identity, position["sequence"]


def _replace_stream_cursor(
    cursors: list[JsonObject], identity: tuple[str, str], cursor: JsonObject
) -> None:
    cursors[:] = [
        existing
        for existing in cursors
        if not isinstance(existing.get("position"), Mapping)
        or (
            existing["position"].get("family"),
            existing["position"].get("scope"),
        )
        != identity
    ]
    cursors.append(_copy(cursor))


def _validate_cursor_evidence(
    raw_commit: Mapping[str, Any],
    run: RunRecord,
) -> None:
    expected_scope = f"run:{run.run_id}"
    seen: set[tuple[str, str]] = set()
    for cursor_ref in _json_object_list(raw_commit.get("stream_cursors"), "stream_cursors"):
        position = cursor_ref.get("position")
        if not isinstance(position, Mapping):
            raise StateError("stream cursor must include a position object")
        family = position.get("family")
        scope = position.get("scope")
        identity = (str(family or ""), str(scope or ""))
        if family not in {"raw_runtime", "display", "replay_event"}:
            raise StateError(f"unknown stream cursor family for run {run.run_id}")
        if scope != expected_scope:
            raise StateError(f"stream cursor scope mismatch for run {run.run_id}")
        if not _is_nonnegative_int(position.get("sequence")):
            raise StateError(f"invalid stream cursor sequence for run {run.run_id}")
        if identity in seen:
            raise StateError(f"duplicate stream cursor family/scope for run {run.run_id}")
        seen.add(identity)

    snapshot = raw_commit.get("display_snapshot")
    if not isinstance(snapshot, Mapping):
        return
    snapshot_scope = snapshot.get("scope")
    if snapshot_scope != expected_scope:
        raise StateError(f"display snapshot scope mismatch for run {run.run_id}")
    cursor = snapshot.get("cursor")
    if cursor is not None and (
        not isinstance(cursor, Mapping)
        or cursor.get("family") != "display"
        or cursor.get("scope") != expected_scope
        or not _is_nonnegative_int(cursor.get("sequence"))
    ):
        raise StateError(f"display snapshot cursor mismatch for run {run.run_id}")


def _validate_environment_evidence(raw_commit: Mapping[str, Any]) -> None:
    environment = raw_commit.get("environment_state")
    if environment is None:
        return
    version = environment.get("version") if isinstance(environment, Mapping) else None
    if (
        not isinstance(environment, Mapping)
        or environment.get("schema") != "starweaver.environment.state"
        or not isinstance(version, int)
        or isinstance(version, bool)
        or version != 1
        or "payload" not in environment
    ):
        raise StateError("environment state must use starweaver.environment.state version 1")


def _validate_run_evidence_commit(
    raw_commit: Mapping[str, Any],
    raw_run: Mapping[str, Any],
    context_state: Mapping[str, Any],
    run: RunRecord,
) -> None:
    metadata = raw_run.get("metadata")
    if isinstance(metadata, Mapping) and "starweaver.run_evidence_sha256" in metadata:
        raise StateError(
            "reserved run metadata key starweaver.run_evidence_sha256 cannot be supplied by callers"
        )
    context_metadata = _mapping_field(context_state, "metadata", default={})
    context_binding_matches = (
        (
            context_state.get("conversation_id") is None
            or context_state.get("conversation_id") == run.conversation_id
        )
        and _optional_metadata_id_matches(
            context_metadata, "starweaver.durable_session_id", run.session_id
        )
        and _optional_metadata_id_matches(context_metadata, "starweaver.durable_run_id", run.run_id)
    )
    if not context_binding_matches:
        raise StateError(
            f"run evidence identity mismatch for session {run.session_id} and run {run.run_id}"
        )

    checkpoints = _json_object_list(raw_commit.get("checkpoints"), "checkpoints")
    approvals = _json_object_list(raw_commit.get("approvals"), "approvals")
    deferred = _json_object_list(raw_commit.get("deferred_tools"), "deferred_tools")
    streams = _json_object_list(raw_commit.get("stream_records"), "stream_records")
    displays = _json_object_list(raw_commit.get("display_messages"), "display_messages")
    replay_events = _json_object_list(raw_commit.get("replay_events"), "replay_events")
    scope = f"run:{run.run_id}"

    _validate_checkpoint_evidence(checkpoints, run)
    _validate_record_ownership(approvals, run, "approval")
    _validate_record_ownership(deferred, run, "deferred tool")
    _validate_display_session_ownership(displays, run, "display message")
    if any(item.get("scope") != scope for item in replay_events):
        raise StateError(f"replay event scope mismatch for run {run.run_id}")

    _ensure_unique_values(streams, "sequence", "raw stream")
    _ensure_unique_values(displays, "sequence", "display stream")
    _ensure_unique_values(replay_events, "sequence", "replay event")
    _ensure_unique_values(approvals, "approval_id", "approval")
    _ensure_unique_values(deferred, "deferred_id", "deferred tool")
    _validate_cursor_evidence(raw_commit, run)
    _validate_environment_evidence(raw_commit)

    snapshot = raw_commit.get("display_snapshot")
    if isinstance(snapshot, Mapping):
        snapshot_messages = _json_object_list(
            snapshot.get("display_messages"), "display_snapshot.display_messages"
        )
        _validate_display_session_ownership(snapshot_messages, run, "display snapshot")

    related_updates = _json_object_list(
        raw_commit.get("related_run_updates"), "related_run_updates"
    )
    _validate_related_run_evidence(related_updates, run)


def _require_existing_evidence_in_commit(
    existing: Sequence[Mapping[str, Any]],
    incoming: Sequence[Mapping[str, Any]],
    identity_field: str,
    family: str,
) -> None:
    incoming_by_id = {record.get(identity_field): record for record in incoming}
    for record in existing:
        identity = record.get(identity_field)
        if incoming_by_id.get(identity) != record:
            raise StateError(f"{family} conflict for id {identity}")


def _validate_existing_evidence_compatibility(
    store: InMemorySessionStore,
    key: tuple[str, str],
    raw_commit: Mapping[str, Any],
) -> None:
    families = (
        (store._checkpoints.get(key, []), "checkpoints", "checkpoint_id", "checkpoint"),
        (store._streams.get(key, []), "stream_records", "sequence", "stream record"),
        (store._approvals.get(key, []), "approvals", "approval_id", "approval"),
        (store._deferred.get(key, []), "deferred_tools", "deferred_id", "deferred tool"),
        (store._display_messages.get(key, []), "display_messages", "sequence", "display message"),
        (store._replay_events.get(key, []), "replay_events", "sequence", "replay event"),
    )
    for existing, commit_field, identity_field, family in families:
        incoming = _json_object_list(raw_commit.get(commit_field), commit_field)
        _require_existing_evidence_in_commit(existing, incoming, identity_field, family)

    existing_cursors = [
        *store._stream_cursors.get(key, []),
        *_json_object_list(
            store._sessions.get(key[0], {}).get("stream_cursors"),
            "stream_cursors",
        ),
    ]
    for incoming_cursor in _json_object_list(raw_commit.get("stream_cursors"), "stream_cursors"):
        incoming = incoming_cursor.get("position")
        if not isinstance(incoming, Mapping):
            raise StateError("stream cursor must include a position object")
        identity = (str(incoming.get("family") or ""), str(incoming.get("scope") or ""))
        for existing_cursor in existing_cursors:
            existing = existing_cursor.get("position")
            if not isinstance(existing, Mapping):
                raise StateError("stored stream cursor must include a position object")
            existing_identity = (
                str(existing.get("family") or ""),
                str(existing.get("scope") or ""),
            )
            if existing_identity != identity:
                continue
            old_sequence = existing.get("sequence")
            new_sequence = incoming.get("sequence")
            if not _is_nonnegative_int(old_sequence) or not _is_nonnegative_int(new_sequence):
                raise StateError(f"stream cursor regression for {identity[0]}:{identity[1]}")
            old_value = int(cast(int, old_sequence))
            new_value = int(cast(int, new_sequence))
            if new_value < old_value:
                raise StateError(f"stream cursor regression for {identity[0]}:{identity[1]}")


def _replace_record_by_id(
    records: list[JsonObject],
    replacement: Mapping[str, Any],
    id_field: str,
    *,
    validate: Callable[[Mapping[str, Any], Mapping[str, Any]], None] | None = None,
) -> None:
    identity = replacement.get(id_field)
    if not isinstance(identity, str) or not identity:
        raise StateError(f"{id_field} must be a non-empty string")
    for index, record in enumerate(records):
        if record.get(id_field) == identity:
            if validate is not None:
                validate(record, replacement)
            records[index] = _copy(replacement)
            return
    raise StateError(f"unknown {id_field}: {identity}")


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

    @property
    def conversation_id(self) -> str:
        return str(self.raw["conversation_id"])

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
    environment_state: JsonObject | None = None
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
            "environment_state": (
                _copy(self.environment_state) if self.environment_state is not None else None
            ),
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

    async def commit_run_evidence(self, commit: Mapping[str, Any]) -> RunRecord:
        """Atomically persist one complete run evidence bundle."""

        raise NotImplementedError

    async def claim_hitl_resume(self, claim: Mapping[str, Any]) -> None:
        """Acquire exclusive ownership of a waiting run before continuation effects."""

        raise NotImplementedError

    async def mark_hitl_resume_started(
        self,
        session_id: str,
        run_id: str,
        claim_id: str,
    ) -> None:
        """Fence a claim immediately before continuation execution starts."""

        raise NotImplementedError

    async def release_hitl_resume_claim(
        self,
        session_id: str,
        run_id: str,
        claim_id: str,
    ) -> None:
        """Release an unstarted continuation claim."""

        raise NotImplementedError

    async def pending_stream_publications(
        self,
        session_id: str,
    ) -> list[JsonObject]:
        """Return transactionally queued external stream publications."""

        raise NotImplementedError

    async def acknowledge_stream_publication(
        self,
        publication_id: str,
        target: str,
    ) -> None:
        """Acknowledge one external sink after complete idempotent delivery."""

        raise NotImplementedError

    async def commit_checkpoint(
        self,
        session_id: str,
        checkpoint: Mapping[str, Any],
    ) -> None:
        """Atomically bootstrap missing session/run records and persist a checkpoint."""

        raise NotImplementedError

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

    def _resume_state(
        self,
        session: SessionRecord,
        run: RunRecord | None,
    ) -> JsonObject:
        return session.state

    def _resume_environment_state(
        self,
        session: SessionRecord,
        run: RunRecord | None,
    ) -> JsonObject | None:
        run_environment = run.raw.get("environment_state") if run is not None else None
        if isinstance(run_environment, Mapping):
            return _copy(run_environment)
        session_environment = session.raw.get("environment_state")
        return _copy(session_environment) if isinstance(session_environment, Mapping) else None

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
        session_cursors = _json_object_list(session_raw.get("stream_cursors"), "stream_cursors")
        run_cursors = (
            _json_object_list(run.raw.get("stream_cursors"), "stream_cursors")
            if run is not None
            else []
        )
        return SessionResumeSnapshot(
            state=self._resume_state(session, run),
            session=session,
            run=run,
            environment_state=self._resume_environment_state(session, run),
            latest_checkpoint=latest_checkpoint,
            stream_records=records,
            approvals=approvals,
            deferred_tools=deferred,
            stream_cursors=_merge_stream_cursors(session_cursors, run_cursors),
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
        if stream_cursor is None:
            for cursor_ref in reversed(stream_cursors):
                position = cursor_ref.get("position")
                if isinstance(position, Mapping) and position.get("family") == "raw_runtime":
                    stream_cursor = position.get("sequence")
                    break
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
        self._run_context: dict[tuple[str, str], JsonObject] = {}
        self._run_environment: dict[tuple[str, str], JsonObject] = {}
        self._display_messages: dict[tuple[str, str], list[JsonObject]] = {}
        self._replay_events: dict[tuple[str, str], list[JsonObject]] = {}
        self._display_snapshots: dict[tuple[str, str], JsonObject] = {}
        self._evidence_digests: dict[tuple[str, str], str] = {}
        self._hitl_resume_claims: dict[tuple[str, str], JsonObject] = {}
        self._stream_publication_outbox: dict[str, JsonObject] = {}

    def _resume_state(
        self,
        session: SessionRecord,
        run: RunRecord | None,
    ) -> JsonObject:
        if run is None:
            return session.state
        return _copy(self._run_context.get((session.session_id, run.run_id), session.state))

    def _replace_state(self, other: InMemorySessionStore) -> None:
        self._sessions = other._sessions
        self._runs = other._runs
        self._checkpoints = other._checkpoints
        self._streams = other._streams
        self._stream_cursors = other._stream_cursors
        self._approvals = other._approvals
        self._deferred = other._deferred
        self._run_context = other._run_context
        self._run_environment = other._run_environment
        self._display_messages = other._display_messages
        self._replay_events = other._replay_events
        self._display_snapshots = other._display_snapshots
        self._evidence_digests = other._evidence_digests
        self._hitl_resume_claims = other._hitl_resume_claims
        self._stream_publication_outbox = other._stream_publication_outbox

    async def commit_checkpoint(
        self,
        session_id: str,
        checkpoint: Mapping[str, Any],
    ) -> None:
        staged = InMemorySessionStore.from_dict(self.to_dict())
        raw_checkpoint = _copy(checkpoint)
        run_id = str(raw_checkpoint.get("run_id") or "")
        conversation_id = str(raw_checkpoint.get("conversation_id") or "")
        if not run_id or not conversation_id:
            raise StateError("checkpoint must include run_id and conversation_id")
        if session_id not in staged._sessions:
            staged._sessions[session_id] = SessionRecord.from_state(
                {
                    "agent_id": "main",
                    "session_id": session_id,
                    "conversation_id": conversation_id,
                }
            ).to_dict()
        key = (session_id, run_id)
        if key not in staged._runs:
            now = _now()
            resume = raw_checkpoint.get("resume")
            lifecycle = resume.get("status") if isinstance(resume, Mapping) else "running"
            status = "running" if lifecycle in {"starting", "running"} else str(lifecycle)
            state = raw_checkpoint.get("state")
            state = state if isinstance(state, Mapping) else {}
            run: JsonObject = {
                "session_id": session_id,
                "run_id": run_id,
                "conversation_id": conversation_id,
                "status": status,
                "sequence_no": 0,
                "created_at": now,
                "updated_at": now,
                "metadata": {},
            }
            if state.get("parent_run_id") is not None:
                run["parent_run_id"] = str(state["parent_run_id"])
            if state.get("parent_task_id") is not None:
                run["parent_task_id"] = str(state["parent_task_id"])
            await staged.append_run(run)
        await staged.append_checkpoint(session_id, raw_checkpoint)
        self._replace_state(staged)

    def _require_resume_claim(
        self,
        source_key: tuple[str, str],
        update: Mapping[str, Any],
    ) -> None:
        source_run_id = source_key[1]
        claim_id = update.get("resume_claim_id")
        claim = self._hitl_resume_claims.get(source_key)
        if not isinstance(claim_id, str) or not claim_id:
            raise StateError(
                f"related run update {source_run_id} requires an exclusive resume claim"
            )
        if claim is None or claim.get("claim_id") != claim_id or claim.get("state") != "started":
            raise StateError(f"started resume claim conflict for related run {source_run_id}")

    def _stage_stream_publication(
        self,
        raw_commit: Mapping[str, Any],
        run: RunRecord,
        raw_run: Mapping[str, Any],
        snapshot: object,
    ) -> None:
        targets = raw_commit.get("publication_targets")
        if not isinstance(targets, Mapping) or not bool(
            targets.get("archive") or targets.get("replay")
        ):
            return
        publication_id = _stream_publication_id(run.session_id, run.run_id)
        publication: JsonObject = {
            "publication_id": publication_id,
            "session_id": run.session_id,
            "run_id": run.run_id,
            "stream_records": _json_object_list(raw_commit.get("stream_records"), "stream_records"),
            "display_messages": _json_object_list(
                raw_commit.get("display_messages"), "display_messages"
            ),
            "replay_events": _json_object_list(raw_commit.get("replay_events"), "replay_events"),
            "archive_pending": bool(targets.get("archive")),
            "replay_pending": bool(targets.get("replay")),
            "created_at": str(raw_run.get("updated_at") or _now()),
        }
        if isinstance(snapshot, Mapping):
            publication["display_snapshot"] = _copy(snapshot)
        self._stream_publication_outbox[publication_id] = publication

    def _apply_related_run_updates(
        self,
        session_id: str,
        updates: object,
    ) -> None:
        seen_run_ids: set[str] = set()
        for update in _json_object_list(updates, "related_run_updates"):
            source_run_id = str(update.get("run_id") or "")
            if not source_run_id or source_run_id in seen_run_ids:
                raise StateError(f"invalid or duplicate related run update: {source_run_id}")
            seen_run_ids.add(source_run_id)
            status = str(update.get("status") or "")
            if status not in {"completed", "failed", "cancelled"}:
                raise StateError(
                    f"related run update {source_run_id} must target a terminal status"
                )
            source_key = (session_id, source_run_id)
            self._require_resume_claim(source_key, update)
            source = self._runs.get(source_key)
            if source is None:
                raise StateError(f"unknown run: {session_id}:{source_run_id}")
            expected = str(update.get("expected_status") or "")
            if source.get("status") != expected:
                raise StateError(
                    f"related run {source_run_id} status conflict: expected {expected}, "
                    f"found {source.get('status')}"
                )
            source["status"] = status
            source["output_preview"] = update.get("output_preview")
            source["updated_at"] = _now()
            source_session = self._sessions[session_id]
            if source_session.get("active_run_id") == source_run_id:
                source_session["active_run_id"] = None
            if source["status"] == "completed":
                source_session["head_success_run_id"] = source_run_id
            for approval in _json_object_list(update.get("approvals"), "approvals"):
                _replace_record_by_id(
                    self._approvals.setdefault(source_key, []),
                    approval,
                    "approval_id",
                    validate=_validate_approval_transition,
                )
            for deferred in _json_object_list(
                update.get("deferred_tools"),
                "deferred_tools",
            ):
                _replace_record_by_id(
                    self._deferred.setdefault(source_key, []),
                    deferred,
                    "deferred_id",
                    validate=_validate_deferred_transition,
                )
            del self._hitl_resume_claims[source_key]

    async def commit_run_evidence(self, commit: Mapping[str, Any]) -> RunRecord:
        raw_commit = _copy(commit)
        raw_run = raw_commit.get("run")
        context_state = raw_commit.get("context_state")
        if not isinstance(raw_run, Mapping) or not isinstance(context_state, Mapping):
            raise StateError("run evidence must include run and context_state objects")
        run = _ensure_run_record(raw_run)
        _validate_run_evidence_commit(raw_commit, raw_run, context_state, run)
        related_updates = _json_object_list(
            raw_commit.get("related_run_updates"),
            "related_run_updates",
        )
        if related_updates:
            if len(related_updates) != 1:
                raise StateError("run evidence accepts exactly one related run update")
            source_run_id = str(related_updates[0].get("run_id") or "")
            if source_run_id == run.run_id or raw_run.get("restore_from_run_id") != source_run_id:
                raise StateError(
                    "related run update must match restore_from_run_id and differ from run_id"
                )
        key = (run.session_id, run.run_id)
        digest_payload = json.dumps(raw_commit, sort_keys=True, separators=(",", ":"))
        digest = hashlib.sha256(digest_payload.encode()).hexdigest()
        existing_digest = self._evidence_digests.get(key)
        if existing_digest is not None:
            if existing_digest != digest:
                raise StateError(f"run evidence conflict for {run.session_id}:{run.run_id}")
            return await self.load_run(run.session_id, run.run_id)
        if run.session_id not in self._sessions:
            raise StateError(f"unknown session: {run.session_id}")
        _validate_existing_evidence_compatibility(self, key, raw_commit)

        staged = InMemorySessionStore.from_dict(self.to_dict())
        staged._apply_related_run_updates(
            run.session_id,
            raw_commit.get("related_run_updates"),
        )
        await staged.append_run(run)
        staged._sessions[run.session_id]["state"] = _copy(context_state)
        staged._run_context[key] = _copy(context_state)
        environment_state = raw_commit.get("environment_state")
        if isinstance(environment_state, Mapping):
            staged._run_environment[key] = _copy(environment_state)
        staged._checkpoints[key] = _json_object_list(
            raw_commit.get("checkpoints"),
            "checkpoints",
        )
        staged._streams[key] = _json_object_list(
            raw_commit.get("stream_records"),
            "stream_records",
        )
        staged._approvals[key] = _json_object_list(
            raw_commit.get("approvals"),
            "approvals",
        )
        staged._deferred[key] = _json_object_list(
            raw_commit.get("deferred_tools"),
            "deferred_tools",
        )
        cursors = _json_object_list(raw_commit.get("stream_cursors"), "stream_cursors")
        staged._stream_cursors[key] = cursors
        staged._runs[key]["stream_cursors"] = copy.deepcopy(cursors)
        staged._sessions[run.session_id]["stream_cursors"] = copy.deepcopy(cursors)
        staged._display_messages[key] = _json_object_list(
            raw_commit.get("display_messages"),
            "display_messages",
        )
        staged._replay_events[key] = _json_object_list(
            raw_commit.get("replay_events"),
            "replay_events",
        )
        snapshot = raw_commit.get("display_snapshot")
        if isinstance(snapshot, Mapping):
            staged._display_snapshots[key] = _copy(snapshot)
        staged._stage_stream_publication(raw_commit, run, raw_run, snapshot)
        staged._evidence_digests[key] = digest
        self._replace_state(staged)
        return await self.load_run(run.session_id, run.run_id)

    async def claim_hitl_resume(self, claim: Mapping[str, Any]) -> None:
        raw = _copy(claim)
        raw.setdefault("state", "preflight")
        session_id = raw.get("session_id")
        run_id = raw.get("run_id")
        claim_id = raw.get("claim_id")
        if (
            not all(
                isinstance(value, str) and bool(value.strip())
                for value in (session_id, run_id, claim_id)
            )
            or raw.get("state") != "preflight"
        ):
            raise StateError("invalid HITL preflight claim")
        key = (str(session_id), str(run_id))
        run = self._runs.get(key)
        if run is None:
            raise StateError(f"unknown run: {session_id}:{run_id}")
        if run.get("status") != "waiting":
            raise StateError(f"run {run_id} is not waiting")
        existing = self._hitl_resume_claims.get(key)
        if existing is not None:
            if existing == raw:
                return
            raise StateError(f"run {run_id} already has an active resume claim")
        self._hitl_resume_claims[key] = raw

    async def mark_hitl_resume_started(
        self,
        session_id: str,
        run_id: str,
        claim_id: str,
    ) -> None:
        key = (session_id, run_id)
        claim = self._hitl_resume_claims.get(key)
        if claim is None:
            raise StateError(f"unknown resume claim for run {run_id}")
        if claim.get("claim_id") != claim_id:
            raise StateError(f"resume claim conflict for run {run_id}")
        state = claim.get("state")
        if state == "preflight":
            claim["state"] = "started"
        elif state != "started":
            raise StateError(f"invalid resume claim state for run {run_id}")

    async def release_hitl_resume_claim(
        self,
        session_id: str,
        run_id: str,
        claim_id: str,
    ) -> None:
        key = (session_id, run_id)
        existing = self._hitl_resume_claims.get(key)
        if existing is None:
            return
        if existing.get("claim_id") != claim_id:
            raise StateError(f"resume claim conflict for run {run_id}")
        if existing.get("state") != "preflight":
            raise StateError(f"started resume claim for run {run_id} cannot be released")
        del self._hitl_resume_claims[key]

    async def pending_stream_publications(
        self,
        session_id: str,
    ) -> list[JsonObject]:
        return [
            _copy(publication)
            for publication in self._stream_publication_outbox.values()
            if publication.get("session_id") == session_id
            and bool(publication.get("archive_pending") or publication.get("replay_pending"))
        ]

    async def acknowledge_stream_publication(
        self,
        publication_id: str,
        target: str,
    ) -> None:
        publication = self._stream_publication_outbox.get(publication_id)
        if publication is None:
            return
        target_value = str(target)
        if target_value == "archive":
            publication["archive_pending"] = False
        elif target_value == "replay":
            publication["replay_pending"] = False
        else:
            raise StateError(f"unknown stream publication target: {target_value}")
        if not publication.get("archive_pending") and not publication.get("replay_pending"):
            del self._stream_publication_outbox[publication_id]

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
        raw = record.to_dict()
        key = (record.session_id, record.run_id)
        persisted = self._runs.get(key)
        requested_sequence = int(raw.get("sequence_no", 0))
        if persisted is not None:
            persisted_sequence = int(persisted.get("sequence_no", 0))
            if requested_sequence not in {0, persisted_sequence}:
                raise StateError(
                    "run sequence is immutable for "
                    f"{record.session_id}:{record.run_id}: "
                    f"persisted {persisted_sequence}, received {requested_sequence}"
                )
            raw["sequence_no"] = persisted_sequence
        elif requested_sequence == 0:
            raw["sequence_no"] = (
                max(
                    (
                        int(candidate.get("sequence_no", 0))
                        for (session_id, _), candidate in self._runs.items()
                        if session_id == record.session_id
                    ),
                    default=0,
                )
                + 1
            )
        elif any(
            session_id == record.session_id
            and int(candidate.get("sequence_no", 0)) == requested_sequence
            for (session_id, _), candidate in self._runs.items()
        ):
            raise StateError(
                f"run sequence conflict for session {record.session_id} "
                f"at sequence {requested_sequence}"
            )
        self._runs[key] = raw
        if record.session_id in self._sessions:
            session = self._sessions[record.session_id]
            session["head_run_id"] = record.run_id
            status = str(raw.get("status") or "queued")
            if status in {"queued", "starting", "running", "waiting"}:
                session["active_run_id"] = record.run_id
            else:
                if status == "completed":
                    session["head_success_run_id"] = record.run_id
                if session.get("active_run_id") == record.run_id:
                    session["active_run_id"] = None
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
        raw_checkpoint = _copy(checkpoint)
        run_id = raw_checkpoint.get("run_id")
        checkpoint_id = raw_checkpoint.get("checkpoint_id")
        run_step = raw_checkpoint.get("run_step")
        if not isinstance(run_id, str) or not run_id:
            raise StateError("checkpoint must include a non-empty run_id")
        if not isinstance(checkpoint_id, str) or not checkpoint_id:
            raise StateError("checkpoint must include a non-empty checkpoint_id")
        if not isinstance(run_step, int) or isinstance(run_step, bool) or run_step < 0:
            raise StateError("checkpoint run_step must be a non-negative integer")
        key = (session_id, run_id)
        checkpoints = self._checkpoints.setdefault(key, [])
        for persisted in checkpoints:
            if persisted.get("checkpoint_id") == checkpoint_id:
                if persisted != raw_checkpoint:
                    raise StateError(
                        f"checkpoint conflict for {session_id}:{run_id}:{checkpoint_id}"
                    )
                return
        checkpoints.append(raw_checkpoint)
        checkpoints.sort(
            key=lambda record: (
                int(record.get("run_step", 0)),
                str(record.get("checkpoint_id") or ""),
            )
        )
        if key in self._runs:
            latest = checkpoints[-1]
            latest_resume = latest.get("resume")
            latest_cursor = (
                latest_resume.get("cursor") if isinstance(latest_resume, Mapping) else None
            )
            run = self._runs[key]
            run["latest_checkpoint"] = {
                "checkpoint_id": latest.get("checkpoint_id"),
                "run_id": latest.get("run_id"),
                "sequence": int(latest.get("run_step", 0)),
                "node": latest.get("node"),
                "stream_cursor": (
                    latest_cursor.get("stream_cursor")
                    if isinstance(latest_cursor, Mapping)
                    else None
                ),
                "created_at": _now(),
                "metadata": _mapping_field(latest, "metadata", default={}),
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
        by_sequence = {record.get("sequence"): record for record in stream}
        for record in records:
            incoming = _ensure_stream_record(record).to_dict()
            sequence = incoming.get("sequence")
            if not _is_nonnegative_int(sequence):
                raise StateError(
                    f"invalid stream record sequence for {session_id}:{run_id}: {sequence}"
                )
            existing = by_sequence.get(sequence)
            if existing is not None and existing != incoming:
                raise StateError(
                    f"stream record conflict for {session_id}:{run_id} at sequence {sequence}"
                )
            if existing is None:
                stream.append(incoming)
                by_sequence[sequence] = incoming
        stream.sort(key=lambda record: int(record["sequence"]))

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
        identity, sequence = _validated_stream_cursor_identity(cursor_record, run_id)
        cursors = self._stream_cursors.setdefault((session_id, run_id), [])
        for existing in cursors:
            existing_position = existing.get("position")
            if not isinstance(existing_position, Mapping):
                continue
            if (existing_position.get("family"), existing_position.get("scope")) != identity:
                continue
            existing_sequence = existing_position.get("sequence")
            if not isinstance(existing_sequence, int) or isinstance(existing_sequence, bool):
                raise StateError("stored stream cursor has an invalid sequence")
            if sequence < existing_sequence:
                raise StateError(f"stream cursor regressed for {identity[0]}:{identity[1]}")
            if sequence == existing_sequence and existing != cursor_record:
                raise StateError(f"stream cursor conflict for {identity[0]}:{identity[1]}")
        _replace_stream_cursor(cursors, identity, cursor_record)
        for record in (self._sessions.get(session_id), self._runs.get((session_id, run_id))):
            if record is not None:
                record_cursors = record.setdefault("stream_cursors", [])
                _replace_stream_cursor(record_cursors, identity, cursor_record)
                record["updated_at"] = _now()

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
            "format": SESSION_STORE_FORMAT,
            "version": SESSION_STORE_VERSION,
            "sessions": copy.deepcopy(self._sessions),
            "runs": {
                _encode_run_key(sid, rid): _copy(record)
                for (sid, rid), record in self._runs.items()
            },
            "checkpoints": {
                _encode_run_key(sid, rid): copy.deepcopy(records)
                for (sid, rid), records in self._checkpoints.items()
            },
            "streams": {
                _encode_run_key(sid, rid): copy.deepcopy(records)
                for (sid, rid), records in self._streams.items()
            },
            "stream_cursors": {
                _encode_run_key(sid, rid): copy.deepcopy(records)
                for (sid, rid), records in self._stream_cursors.items()
            },
            "approvals": {
                _encode_run_key(sid, rid): copy.deepcopy(records)
                for (sid, rid), records in self._approvals.items()
            },
            "deferred": {
                _encode_run_key(sid, rid): copy.deepcopy(records)
                for (sid, rid), records in self._deferred.items()
            },
            "run_context": {
                _encode_run_key(sid, rid): _copy(record)
                for (sid, rid), record in self._run_context.items()
            },
            "run_environment": {
                _encode_run_key(sid, rid): _copy(record)
                for (sid, rid), record in self._run_environment.items()
            },
            "display_messages": {
                _encode_run_key(sid, rid): copy.deepcopy(records)
                for (sid, rid), records in self._display_messages.items()
            },
            "replay_events": {
                _encode_run_key(sid, rid): copy.deepcopy(records)
                for (sid, rid), records in self._replay_events.items()
            },
            "display_snapshots": {
                _encode_run_key(sid, rid): _copy(record)
                for (sid, rid), record in self._display_snapshots.items()
            },
            "evidence_digests": {
                _encode_run_key(sid, rid): digest
                for (sid, rid), digest in self._evidence_digests.items()
            },
            "hitl_resume_claims": {
                _encode_run_key(sid, rid): _copy(record)
                for (sid, rid), record in self._hitl_resume_claims.items()
            },
            "stream_publication_outbox": copy.deepcopy(self._stream_publication_outbox),
        }

    @classmethod
    def from_dict(cls, raw: Mapping[str, Any]) -> InMemorySessionStore:
        store = cls()
        format_value = raw.get("format")
        version_value = raw.get("version")
        if format_value is None and version_value is None:
            is_legacy = True
        elif format_value != SESSION_STORE_FORMAT:
            raise StateError(f"unsupported session-store format: {format_value!r}")
        elif (
            not isinstance(version_value, int)
            or isinstance(version_value, bool)
            or version_value != SESSION_STORE_VERSION
        ):
            raise StateError(f"unsupported session-store version: {version_value!r}")
        else:
            is_legacy = False
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
        store._run_context = _keyed_records(raw.get("run_context"))
        store._run_environment = _keyed_records(raw.get("run_environment"))
        store._display_messages = _keyed_record_lists(raw.get("display_messages"))
        store._replay_events = _keyed_record_lists(raw.get("replay_events"))
        store._display_snapshots = _keyed_records(raw.get("display_snapshots"))
        store._hitl_resume_claims = _keyed_records(raw.get("hitl_resume_claims"))
        publications = raw.get("stream_publication_outbox")
        if isinstance(publications, Mapping):
            store._stream_publication_outbox = {
                str(publication_id): _copy(record)
                for publication_id, record in publications.items()
                if isinstance(record, Mapping)
            }
        digests = raw.get("evidence_digests")
        if isinstance(digests, Mapping):
            store._evidence_digests = {
                _split_key(str(key)): str(value)
                for key, value in digests.items()
                if isinstance(value, str)
            }
        if is_legacy:
            for key in store._runs:
                store._evidence_digests.setdefault(key, LEGACY_UNSEALED_EVIDENCE_DIGEST)
        return store


class JsonSessionStore(InMemorySessionStore):
    """Single-file JSON session store for local development and tests."""

    def __init__(self, path: str | PathLike[str]) -> None:
        self.path = Path(path)
        if self.path.exists():
            loaded = InMemorySessionStore.from_dict(
                json.loads(self.path.read_text(encoding="utf-8"))
            )
            self._replace_state(loaded)
        else:
            super().__init__()

    async def flush(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        payload = json.dumps(self.to_dict(), indent=2, sort_keys=True) + "\n"
        descriptor, temporary_path = tempfile.mkstemp(
            dir=self.path.parent,
            prefix=f".{self.path.name}.",
            suffix=".tmp",
        )
        try:
            with os.fdopen(descriptor, "w", encoding="utf-8") as temporary:
                temporary.write(payload)
                temporary.flush()
                os.fsync(temporary.fileno())
            os.replace(temporary_path, self.path)
        except BaseException:
            with suppress(FileNotFoundError):
                os.unlink(temporary_path)
            raise

    async def commit_checkpoint(
        self,
        session_id: str,
        checkpoint: Mapping[str, Any],
    ) -> None:
        previous = InMemorySessionStore.from_dict(self.to_dict())
        try:
            await super().commit_checkpoint(session_id, checkpoint)
            await self.flush()
        except BaseException:
            self._replace_state(previous)
            raise

    async def commit_run_evidence(self, commit: Mapping[str, Any]) -> RunRecord:
        previous = InMemorySessionStore.from_dict(self.to_dict())
        try:
            record = await super().commit_run_evidence(commit)
            await self.flush()
            return record
        except BaseException:
            self._replace_state(previous)
            raise

    async def claim_hitl_resume(self, claim: Mapping[str, Any]) -> None:
        del claim
        raise StateError(
            "JsonSessionStore does not support durable HITL resume claims; use SqliteSessionStore"
        )

    async def mark_hitl_resume_started(
        self,
        session_id: str,
        run_id: str,
        claim_id: str,
    ) -> None:
        del session_id, run_id, claim_id
        raise StateError(
            "JsonSessionStore does not support durable HITL resume claims; use SqliteSessionStore"
        )

    async def release_hitl_resume_claim(
        self,
        session_id: str,
        run_id: str,
        claim_id: str,
    ) -> None:
        del session_id, run_id, claim_id
        raise StateError(
            "JsonSessionStore does not support durable HITL resume claims; use SqliteSessionStore"
        )

    async def acknowledge_stream_publication(
        self,
        publication_id: str,
        target: str,
    ) -> None:
        previous = InMemorySessionStore.from_dict(self.to_dict())
        try:
            await super().acknowledge_stream_publication(publication_id, target)
            await self.flush()
        except BaseException:
            self._replace_state(previous)
            raise

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

    async def commit_run_evidence(self, commit: Mapping[str, Any]) -> RunRecord:
        return RunRecord(await self._native.commit_run_evidence(_copy(commit)))

    async def commit_checkpoint(
        self,
        session_id: str,
        checkpoint: Mapping[str, Any],
    ) -> None:
        await self._native.commit_checkpoint(session_id, _copy(checkpoint))

    async def claim_hitl_resume(self, claim: Mapping[str, Any]) -> None:
        raw = _copy(claim)
        raw.setdefault("state", "preflight")
        await self._native.claim_hitl_resume(raw)

    async def mark_hitl_resume_started(
        self,
        session_id: str,
        run_id: str,
        claim_id: str,
    ) -> None:
        await self._native.mark_hitl_resume_started(session_id, run_id, claim_id)

    async def release_hitl_resume_claim(
        self,
        session_id: str,
        run_id: str,
        claim_id: str,
    ) -> None:
        await self._native.release_hitl_resume_claim(session_id, run_id, claim_id)

    async def pending_stream_publications(
        self,
        session_id: str,
    ) -> list[JsonObject]:
        return [
            _copy(publication)
            for publication in await self._native.pending_stream_publications(session_id)
        ]

    async def acknowledge_stream_publication(
        self,
        publication_id: str,
        target: str,
    ) -> None:
        await self._native.acknowledge_stream_publication(publication_id, target)

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

    async def append_run_allocated(
        self,
        record: RunRecord | Mapping[str, Any],
    ) -> RunRecord:
        """Atomically allocate or preserve the run sequence and persist the record."""

        return RunRecord(
            await self._native.append_run_allocated(_ensure_run_record(record).to_dict())
        )

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
                environment_state=_optional_mapping_field(session_raw, "environment_state"),
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
            environment_state=_optional_mapping_field(raw, "environment_state"),
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
    try:
        decoded = json.loads(key)
    except json.JSONDecodeError:
        decoded = None
    if (
        isinstance(decoded, list)
        and len(decoded) == 2
        and all(isinstance(value, str) for value in decoded)
    ):
        return decoded[0], decoded[1]
    # Best-effort compatibility for v1 local-development files. V1 keys were ambiguous when IDs
    # contained ':', so every migrated run is legacy-sealed and cannot be rewritten.
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
