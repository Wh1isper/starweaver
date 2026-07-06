"""Python agent, session, run, message, and HITL facades."""

from __future__ import annotations

import asyncio
import copy
import json
import uuid
from collections.abc import AsyncIterator, Callable, Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from os import PathLike
from pathlib import Path
from typing import Any, Literal, overload
from weakref import WeakSet

from . import _native
from .capability import CapabilityBundle, ensure_capability_bundle
from .environment import EnvironmentProvider, ensure_environment_provider
from .errors import StateError
from .media import MediaUploader, ensure_media_uploader
from .model import ModelSettings, RequestParams, ensure_model_settings, ensure_request_params
from .observability import TraceMetadata, Usage, UsageSnapshot
from .output import OutputPolicy, OutputSchema, ensure_output_policy, ensure_output_schema
from .runtime import RuntimeConfig, ensure_runtime_config
from .skills import SkillRegistry, ensure_skill_registry
from .subagent import Subagent, ensure_subagent
from .tool import BaseTool, Tool, ensure_tool
from .toolset import (
    AbstractToolset,
    Toolset,
    ToolsetContext,
    ToolsetFactory,
    ToolsetLifecycleReport,
    ensure_toolsets,
    toolset_factory,
    validate_toolsets_for_durability,
)

SESSION_ARCHIVE_FORMAT = "starweaver.session.archive"
SESSION_ARCHIVE_VERSION = 1


@dataclass(frozen=True)
class ControlReceipt:
    """Receipt returned after active-run control input is accepted."""

    id: str
    kind: Literal["message", "steering", "interrupt"]
    queued: bool
    run_id: str | None = None
    session_id: str | None = None

    @classmethod
    def from_raw(cls, raw: Mapping[str, Any]) -> ControlReceipt:
        return cls(
            id=str(raw["id"]),
            kind=raw["kind"],  # type: ignore[arg-type]
            queued=bool(raw.get("queued", False)),
            run_id=_optional_str(raw.get("run_id")),
            session_id=_optional_str(raw.get("session_id")),
        )


@dataclass(frozen=True)
class BusMessage:
    """Message-bus record exposed through Python."""

    content: Any
    id: str = field(default_factory=lambda: uuid.uuid4().hex)
    source: str = "application"
    target: str | None = None
    topic: str | None = None
    template: str | None = None
    metadata: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        metadata = dict(self.metadata)
        if self.topic is not None:
            existing_topic = metadata.get("starweaver.topic")
            if existing_topic is not None and str(existing_topic) != self.topic:
                raise ValueError("topic conflicts with metadata['starweaver.topic']")
            metadata["starweaver.topic"] = self.topic
        payload: dict[str, Any] = {
            "id": self.id,
            "content": self.content,
            "source": self.source,
            "metadata": metadata,
        }
        if self.target is not None:
            payload["target"] = self.target
        if self.template is not None:
            payload["template"] = self.template
        return payload

    @classmethod
    def from_raw(cls, raw: Mapping[str, Any]) -> BusMessage:
        metadata = dict(raw.get("metadata") or {})
        topic = raw.get("topic") or metadata.get("starweaver.topic")
        if (
            raw.get("topic") is not None
            and metadata.get("starweaver.topic") is not None
            and str(raw["topic"]) != str(metadata["starweaver.topic"])
        ):
            raise ValueError("topic conflicts with metadata['starweaver.topic']")
        return cls(
            id=str(raw["id"]),
            content=raw.get("content"),
            source=str(raw.get("source") or "application"),
            target=_optional_str(raw.get("target")),
            topic=_optional_str(topic),
            template=_optional_str(raw.get("template")),
            metadata=metadata,
        )


@dataclass(frozen=True)
class MessageDelivery:
    """Consistent result for message-bus writes."""

    message: BusMessage
    receipt: ControlReceipt | None = None

    @property
    def id(self) -> str:
        return self.message.id

    @property
    def active(self) -> bool:
        return self.receipt is not None

    @property
    def queued(self) -> bool:
        return bool(self.receipt and self.receipt.queued)

    @property
    def kind(self) -> Literal["message", "steering"]:
        if self.receipt is not None and self.receipt.kind == "steering":
            return "steering"
        return "steering" if self.message.topic == "steering" else "message"


@dataclass(frozen=True)
class RunStatusSnapshot:
    """Pollable run status snapshot."""

    run_status: str
    current_error: dict[str, Any] | None = None
    cancel_requested: bool = False
    dropped_events: int = 0
    receiver_closed: bool = False
    buffer_size: int | None = None
    drop_policy: str | None = None

    @classmethod
    def from_raw(cls, raw: Mapping[str, Any]) -> RunStatusSnapshot:
        return cls(
            run_status=str(raw.get("run_status", "unknown")),
            current_error=(
                dict(raw["current_error"])
                if isinstance(raw.get("current_error"), Mapping)
                else None
            ),
            cancel_requested=bool(raw.get("cancel_requested", False)),
            dropped_events=int(raw.get("dropped_events", 0)),
            receiver_closed=bool(raw.get("receiver_closed", False)),
            buffer_size=_optional_int(raw.get("buffer_size")),
            drop_policy=_optional_str(raw.get("drop_policy")),
        )

    def to_dict(self) -> dict[str, Any]:
        return {
            "run_status": self.run_status,
            "current_error": self.current_error,
            "cancel_requested": self.cancel_requested,
            "dropped_events": self.dropped_events,
            "receiver_closed": self.receiver_closed,
            "buffer_size": self.buffer_size,
            "drop_policy": self.drop_policy,
        }

    def __getitem__(self, key: str) -> Any:
        return self.to_dict()[key]

    def get(self, key: str, default: Any = None) -> Any:
        return self.to_dict().get(key, default)


@dataclass(frozen=True)
class ApprovalDecision:
    """Approval decision built from a pending approval helper."""

    id: str
    approved: bool
    decided_by: str | None = None
    reason: str | None = None
    override_arguments: Any | None = None
    metadata: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "approval_id": self.id,
            "approved": self.approved,
            "metadata": dict(self.metadata),
        }
        if self.decided_by is not None:
            payload["decided_by"] = self.decided_by
        if self.reason is not None:
            payload["reason"] = self.reason
        if self.override_arguments is not None:
            payload["override_arguments"] = self.override_arguments
        return payload


@dataclass(frozen=True)
class DeferredResult:
    """Deferred tool result built from a pending deferred helper."""

    id: str
    status: Literal["completed", "failed", "cancelled"]
    response: Any = None
    metadata: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        return {
            "deferred_id": self.id,
            "status": self.status,
            "response": self.response,
            "metadata": dict(self.metadata),
        }


@dataclass(frozen=True)
class PendingApproval:
    """Typed view over a pending approval record."""

    id: str
    tool_call_id: str
    tool_name: str
    arguments: dict[str, Any]
    metadata: dict[str, Any]
    raw: dict[str, Any]

    @classmethod
    def from_raw(cls, raw: Mapping[str, Any]) -> PendingApproval:
        return cls(
            id=str(raw.get("approval_id") or raw.get("tool_call_id") or raw["id"]),
            tool_call_id=str(raw.get("tool_call_id") or raw.get("id")),
            tool_name=str(raw.get("tool_name") or raw.get("name") or ""),
            arguments=dict(raw.get("arguments") or {}),
            metadata=dict(raw.get("metadata") or {}),
            raw=dict(raw),
        )

    def approve(
        self,
        *,
        decided_by: str | None = None,
        reason: str | None = None,
        override_arguments: Any | None = None,
        metadata: Mapping[str, Any] | None = None,
        **extra_metadata: Any,
    ) -> ApprovalDecision:
        return ApprovalDecision(
            id=self.id,
            approved=True,
            decided_by=decided_by,
            reason=reason,
            override_arguments=override_arguments,
            metadata=_merge_metadata(metadata, extra_metadata),
        )

    def deny(
        self,
        reason: str,
        *,
        decided_by: str | None = None,
        metadata: Mapping[str, Any] | None = None,
        **extra_metadata: Any,
    ) -> ApprovalDecision:
        return ApprovalDecision(
            id=self.id,
            approved=False,
            decided_by=decided_by,
            reason=reason,
            metadata=_merge_metadata(metadata, extra_metadata),
        )


@dataclass(frozen=True)
class PendingDeferred:
    """Typed view over a pending deferred tool record."""

    id: str
    tool_call_id: str
    tool_name: str
    metadata: dict[str, Any]
    raw: dict[str, Any]

    @classmethod
    def from_raw(cls, raw: Mapping[str, Any]) -> PendingDeferred:
        return cls(
            id=str(raw.get("deferred_id") or raw.get("tool_call_id") or raw["id"]),
            tool_call_id=str(raw.get("tool_call_id") or raw.get("id")),
            tool_name=str(raw.get("tool_name") or raw.get("name") or ""),
            metadata=dict(raw.get("metadata") or {}),
            raw=dict(raw),
        )

    def complete(
        self,
        value: Any,
        *,
        metadata: Mapping[str, Any] | None = None,
        **extra_metadata: Any,
    ) -> DeferredResult:
        return DeferredResult(
            id=self.id,
            status="completed",
            response=value,
            metadata=_merge_metadata(metadata, extra_metadata),
        )

    def fail(
        self,
        error: str,
        *,
        metadata: Mapping[str, Any] | None = None,
        **extra_metadata: Any,
    ) -> DeferredResult:
        return DeferredResult(
            id=self.id,
            status="failed",
            response={"error": error},
            metadata=_merge_metadata(metadata, extra_metadata),
        )

    def cancel(
        self,
        reason: str | None = None,
        *,
        metadata: Mapping[str, Any] | None = None,
        **extra_metadata: Any,
    ) -> DeferredResult:
        return DeferredResult(
            id=self.id,
            status="cancelled",
            response={"reason": reason} if reason is not None else None,
            metadata=_merge_metadata(metadata, extra_metadata),
        )


@dataclass(frozen=True)
class HitlSnapshot:
    """Typed HITL snapshot plus raw escape hatches."""

    approvals: list[PendingApproval]
    deferred: list[PendingDeferred]
    raw_approvals: list[dict[str, Any]]
    raw_deferred: list[dict[str, Any]]

    @property
    def pending_approvals(self) -> list[dict[str, Any]]:
        return self.raw_approvals

    @property
    def pending_deferred(self) -> list[dict[str, Any]]:
        return self.raw_deferred


@dataclass(frozen=True)
class SessionArchive:
    """JSON-compatible session archive for persistence and restore."""

    state: dict[str, Any]
    last_run_state: dict[str, Any] | None = None
    session_id: str | None = None
    run_id: str | None = None
    required_toolset_ids: tuple[str, ...] = ()
    mode: Literal["full", "curated"] = "full"
    version: int = SESSION_ARCHIVE_VERSION
    format: Literal["starweaver.session.archive"] = SESSION_ARCHIVE_FORMAT

    @classmethod
    def from_session(
        cls,
        session: AgentSession,
        *,
        mode: Literal["full", "curated"] = "full",
    ) -> SessionArchive:
        if mode == "full":
            state = session.export_full_state()
        elif mode == "curated":
            state = session.export_state("curated")
        else:
            raise ValueError("mode must be 'full' or 'curated'")
        return cls.from_state(
            state,
            mode=mode,
            last_run_state=session._last_hitl_state if mode == "full" else None,
            required_toolset_ids=_required_toolset_ids_for_archive(session._required_toolsets),
        )

    @classmethod
    def from_state(
        cls,
        state: Mapping[str, Any],
        *,
        mode: Literal["full", "curated"] = "full",
        version: int = SESSION_ARCHIVE_VERSION,
        last_run_state: Mapping[str, Any] | None = None,
        required_toolset_ids: Iterable[str] | None = None,
    ) -> SessionArchive:
        if mode not in {"full", "curated"}:
            raise ValueError("mode must be 'full' or 'curated'")
        if version != SESSION_ARCHIVE_VERSION:
            raise ValueError(f"unsupported session archive version: {version}")
        if mode == "curated" and last_run_state is not None:
            raise ValueError("last_run_state requires a full session archive")
        state_copy = copy.deepcopy(dict(state))
        return cls(
            state=state_copy,
            last_run_state=copy.deepcopy(dict(last_run_state))
            if last_run_state is not None
            else None,
            session_id=_optional_str(state_copy.get("session_id")),
            run_id=_optional_str(state_copy.get("run_id")),
            required_toolset_ids=_normalize_required_toolset_ids(required_toolset_ids),
            mode=mode,
            version=version,
            format=SESSION_ARCHIVE_FORMAT,
        )

    @classmethod
    def from_dict(cls, raw: Mapping[str, Any]) -> SessionArchive:
        archive_format = raw.get("format")
        if archive_format != SESSION_ARCHIVE_FORMAT:
            raise ValueError(f"session archive format must be {SESSION_ARCHIVE_FORMAT!r}")
        raw_version = raw.get("version")
        if not isinstance(raw_version, int):
            raise TypeError("session archive version must be an integer")
        if raw_version != SESSION_ARCHIVE_VERSION:
            raise ValueError(f"unsupported session archive version: {raw_version}")
        state = raw.get("state")
        if not isinstance(state, Mapping):
            raise TypeError("session archive must include a state mapping")
        mode = str(raw.get("mode") or "full")
        if mode not in {"full", "curated"}:
            raise ValueError("session archive mode must be 'full' or 'curated'")
        if mode == "curated" and raw.get("last_run_state") is not None:
            raise ValueError("last_run_state requires a full session archive")
        if raw.get("last_run_state") is not None and not isinstance(
            raw.get("last_run_state"), Mapping
        ):
            raise TypeError("session archive last_run_state must be a mapping")
        return cls(
            state=copy.deepcopy(dict(state)),
            last_run_state=(
                copy.deepcopy(dict(raw["last_run_state"]))
                if isinstance(raw.get("last_run_state"), Mapping)
                else None
            ),
            session_id=_optional_str(raw.get("session_id") or state.get("session_id")),
            run_id=_optional_str(raw.get("run_id") or state.get("run_id")),
            required_toolset_ids=_normalize_required_toolset_ids(raw.get("required_toolset_ids")),
            mode=mode,  # type: ignore[arg-type]
            version=raw_version,
            format=SESSION_ARCHIVE_FORMAT,
        )

    @classmethod
    def from_json(cls, data: str | bytes | bytearray) -> SessionArchive:
        raw = json.loads(data)
        if not isinstance(raw, Mapping):
            raise TypeError("session archive JSON must decode to an object")
        return cls.from_dict(raw)

    @classmethod
    def load(cls, path: str | PathLike[str]) -> SessionArchive:
        return cls.from_json(Path(path).read_text(encoding="utf-8"))

    def to_dict(self) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "format": self.format,
            "state": copy.deepcopy(self.state),
            "mode": self.mode,
            "version": self.version,
        }
        if self.session_id is not None:
            payload["session_id"] = self.session_id
        if self.run_id is not None:
            payload["run_id"] = self.run_id
        if self.required_toolset_ids:
            payload["required_toolset_ids"] = list(self.required_toolset_ids)
        if self.last_run_state is not None:
            payload["last_run_state"] = copy.deepcopy(self.last_run_state)
        return payload

    def to_json(self, **kwargs: Any) -> str:
        return json.dumps(self.to_dict(), **kwargs)

    def save(self, path: str | PathLike[str], **json_kwargs: Any) -> None:
        if "indent" not in json_kwargs:
            json_kwargs["indent"] = 2
        if "sort_keys" not in json_kwargs:
            json_kwargs["sort_keys"] = True
        Path(path).write_text(self.to_json(**json_kwargs) + "\n", encoding="utf-8")


class StreamEvent:
    """Python event wrapper backed by a canonical Starweaver stream record."""

    def __init__(self, native: _native.StreamEvent | Mapping[str, Any]) -> None:
        self._native = native if isinstance(native, _native.StreamEvent) else None
        self._raw = native.raw if isinstance(native, _native.StreamEvent) else dict(native)
        self._kind = (
            native.kind if isinstance(native, _native.StreamEvent) else _event_kind(self._raw)
        )

    @property
    def kind(self) -> str:
        return self._kind

    @property
    def raw(self) -> dict[str, Any]:
        return self._raw

    @property
    def run_id(self) -> str | None:
        return _optional_str(_event_payload(self.raw).get("run_id") or self.raw.get("run_id"))

    @property
    def step(self) -> int | None:
        return _optional_int(_event_payload(self.raw).get("step") or self.raw.get("step"))

    @property
    def sideband(self) -> dict[str, Any] | None:
        event = _event_payload(self.raw)
        if self.kind == "custom":
            sideband = event.get("event")
            return dict(sideband) if isinstance(sideband, Mapping) else None
        return dict(event) if self.kind == "steering_guard" else None

    @property
    def sideband_kind(self) -> str | None:
        sideband = self.sideband
        return _optional_str(sideband.get("kind")) if sideband is not None else None

    @property
    def sideband_payload(self) -> dict[str, Any] | None:
        sideband = self.sideband
        if sideband is None:
            return None
        payload = sideband.get("payload")
        return dict(payload) if isinstance(payload, Mapping) else None

    @property
    def text_delta(self) -> str | None:
        event = _event_payload(self.raw)
        nested = _nested_event_payload(self.raw)
        return _optional_str(
            event.get("text_delta")
            or event.get("delta")
            or nested.get("text")
            or nested.get("text_delta")
            or nested.get("delta")
        )

    @property
    def tool_call(self) -> dict[str, Any] | None:
        event = _event_payload(self.raw)
        return dict(event) if self.kind == "tool_call" else None

    @property
    def tool_return(self) -> dict[str, Any] | None:
        event = _event_payload(self.raw)
        return dict(event) if self.kind == "tool_return" else None

    @property
    def usage(self) -> dict[str, Any] | None:
        event = _event_payload(self.raw)
        usage = event.get("usage")
        return dict(usage) if isinstance(usage, Mapping) else None

    @property
    def usage_record(self) -> Usage | None:
        usage = self.usage
        return Usage(usage) if usage is not None else None

    @property
    def usage_snapshot(self) -> UsageSnapshot | None:
        if self.sideband_kind != "usage_snapshot":
            return None
        payload = self.sideband_payload
        return UsageSnapshot(payload) if payload is not None else None

    @property
    def toolset_lifecycle_report(self) -> ToolsetLifecycleReport | None:
        return ToolsetLifecycleReport.from_sideband(self.sideband)

    @property
    def approval(self) -> dict[str, Any] | None:
        event = _event_payload(self.raw)
        approval = event.get("approval")
        return dict(approval) if isinstance(approval, Mapping) else None

    @property
    def deferred(self) -> dict[str, Any] | None:
        event = _event_payload(self.raw)
        deferred = event.get("deferred")
        return dict(deferred) if isinstance(deferred, Mapping) else None

    @property
    def is_terminal(self) -> bool:
        return self.kind in {"run_complete", "run_failed", "suspended"}


class RunResult:
    """Python run result wrapper with typed helpers and raw fields."""

    def __init__(self, native: _native.RunResult) -> None:
        self._native = native

    @property
    def output(self) -> str:
        return self._native.output

    @property
    def structured_output(self) -> Any | None:
        return self._native.structured_output

    @property
    def messages(self) -> list[Any]:
        return self._native.messages

    @property
    def raw_state(self) -> dict[str, Any]:
        return self._native.raw_state

    @property
    def raw_run_state(self) -> dict[str, Any]:
        return self.raw_state

    @property
    def usage(self) -> Usage:
        return Usage(self.raw_state.get("usage") if isinstance(self.raw_state, Mapping) else None)

    @property
    def usage_snapshot(self) -> UsageSnapshot:
        return UsageSnapshot.from_state(self.raw_state)

    @property
    def trace(self) -> TraceMetadata:
        return TraceMetadata.from_state(self.raw_state)

    @property
    def trace_metadata(self) -> TraceMetadata:
        return self.trace

    @property
    def status(self) -> str:
        return self._native.status

    @property
    def is_waiting(self) -> bool:
        return self._native.is_waiting

    @property
    def needs_approval(self) -> bool:
        return self._native.needs_approval

    @property
    def pending_approvals(self) -> list[dict[str, Any]]:
        return self._native.pending_approvals

    @property
    def pending_deferred_tools(self) -> list[dict[str, Any]]:
        return self._native.pending_deferred_tools

    @property
    def pending_deferred(self) -> list[dict[str, Any]]:
        return self._native.pending_deferred

    @property
    def approvals(self) -> list[PendingApproval]:
        return [PendingApproval.from_raw(item) for item in self.pending_approvals]

    @property
    def deferred(self) -> list[PendingDeferred]:
        return [PendingDeferred.from_raw(item) for item in self.pending_deferred]

    @property
    def hitl(self) -> HitlSnapshot:
        return HitlSnapshot(
            approvals=self.approvals,
            deferred=self.deferred,
            raw_approvals=self.pending_approvals,
            raw_deferred=self.pending_deferred,
        )


class StreamRunResult:
    """Completed stream result wrapper."""

    def __init__(self, native: _native.StreamRunResult) -> None:
        self._native = native
        self._result = RunResult(native.result)
        self._events = [StreamEvent(event) for event in native.events]

    @classmethod
    def from_parts(
        cls,
        *,
        result: RunResult,
        events: Sequence[StreamEvent] = (),
    ) -> StreamRunResult:
        instance = cls.__new__(cls)
        instance._native = None
        instance._result = result
        instance._events = list(events)
        return instance

    @property
    def result(self) -> RunResult:
        return self._result

    @property
    def events(self) -> list[StreamEvent]:
        return self._events


class MessageBus:
    """Python facade over Starweaver message-bus operations."""

    def __init__(
        self,
        *,
        session: AgentSession | None = None,
        run: AgentRun | None = None,
    ) -> None:
        self._session = session
        self._run = run

    async def send(
        self,
        content: Any,
        *,
        topic: str | None = None,
        source: str = "application",
        target: str | None = None,
        id: str | None = None,  # noqa: A002
        template: str | None = None,
        metadata: Mapping[str, Any] | None = None,
    ) -> MessageDelivery:
        message = BusMessage(
            id=id or uuid.uuid4().hex,
            content=content,
            source=source,
            target=target,
            topic=topic,
            template=template,
            metadata=dict(metadata or {}),
        )
        if self._run is not None:
            receipt = await self._run.send_message(message)
            return MessageDelivery(message=message, receipt=receipt)
        if self._session is None:
            raise StateError("message bus is not bound to a run or session")
        active_run = self._session.active_run
        if active_run is not None:
            receipt = await active_run.send_message(message)
            return MessageDelivery(message=message, receipt=receipt)
        stored = BusMessage.from_raw(self._session._native.message_send(message.to_dict()))
        return MessageDelivery(message=stored)

    async def steer(self, text: str, **options: Any) -> MessageDelivery:
        when_idle = options.pop("when_idle", None)
        if when_idle is not None and when_idle != "queue":
            raise ValueError("when_idle must be 'queue' when provided")
        message_id = _optional_str(options.pop("id", None)) or uuid.uuid4().hex
        if options:
            unknown = ", ".join(sorted(options))
            raise TypeError(f"unexpected steering options: {unknown}")
        message = BusMessage(
            id=message_id,
            content=text,
            source="user",
            topic="steering",
        )
        if self._run is not None:
            receipt = await self._run.steer(text, id=message_id)
            return MessageDelivery(message=message, receipt=receipt)
        if self._session is None:
            raise StateError("message bus is not bound to a run or session")
        active_run = self._session.active_run
        if active_run is not None:
            receipt = await active_run.steer(text, id=message_id)
            return MessageDelivery(message=message, receipt=receipt)
        if when_idle == "queue":
            return await self.send(
                text,
                id=message_id,
                topic="steering",
                source="user",
            )
        raise StateError("no active run for session")

    def peek(self, agent_id: str | None = None) -> list[BusMessage]:
        if self._run is not None:
            raise StateError("active run message inspection requires a session snapshot")
        if self._session is None:
            raise StateError("message bus is not bound to a session")
        if self._session.active_run is not None:
            raise StateError("active session messages must be inspected after the run yields state")
        return [BusMessage.from_raw(item) for item in self._session._native.message_peek(agent_id)]

    def consume(self, agent_id: str | None = None) -> list[BusMessage]:
        if self._run is not None:
            raise StateError("active run message inspection requires a session snapshot")
        if self._session is None:
            raise StateError("message bus is not bound to a session")
        if self._session.active_run is not None:
            raise StateError("active session messages must be inspected after the run yields state")
        return [
            BusMessage.from_raw(item) for item in self._session._native.message_consume(agent_id)
        ]

    def subscribe(self, agent_id: str | None = None) -> None:
        if self._session is None:
            raise StateError("message bus is not bound to a session")
        self._session._native.message_subscribe(agent_id)

    def unsubscribe(self, agent_id: str | None = None) -> None:
        if self._session is None:
            raise StateError("message bus is not bound to a session")
        self._session._native.message_unsubscribe(agent_id)


class SessionHitl:
    """HITL helper bound to an AgentSession."""

    def __init__(self, session: AgentSession) -> None:
        self._session = session

    async def snapshot(self) -> HitlSnapshot:
        if self._session._last_result is not None and self._session._last_result.is_waiting:
            return self._session._last_result.hitl
        if self._session._last_hitl_state is not None:
            return _hitl_snapshot_from_run_state(self._session._last_hitl_state)
        raise StateError("session has no HITL result snapshot")

    async def resume(
        self,
        *,
        approvals: object | None = None,
        deferred_results: object | None = None,
    ) -> RunResult:
        return await self._session.resume_after_hitl(
            approvals=approvals,
            deferred_results=deferred_results,
        )

    async def resume_collected(
        self,
        *,
        approvals: object | None = None,
        deferred_results: object | None = None,
    ) -> RunResult:
        return await self.resume(approvals=approvals, deferred_results=deferred_results)

    async def resume_stream(
        self,
        *,
        approvals: object | None = None,
        deferred_results: object | None = None,
    ) -> AgentRun:
        return await self._session.resume_after_hitl_stream(
            approvals=approvals,
            deferred_results=deferred_results,
        )


class RunHitl:
    """HITL helper bound to an AgentRun."""

    def __init__(self, run: AgentRun) -> None:
        self._run = run

    async def snapshot(self) -> HitlSnapshot:
        if self._run._joined is not None:
            return self._run._joined.result.hitl
        if not self._run._suspended_seen:
            raise StateError("run.hitl.snapshot requires a suspended run")
        return (await self._run.result()).hitl

    async def resume_collected(
        self,
        *,
        approvals: object | None = None,
        deferred_results: object | None = None,
    ) -> RunResult:
        return await self._run._resume_hitl_collected(
            approvals=approvals,
            deferred_results=deferred_results,
        )

    async def resume(
        self,
        *,
        approvals: object | None = None,
        deferred_results: object | None = None,
    ) -> AgentRun:
        return await self._run._resume_hitl_stream(
            approvals=approvals,
            deferred_results=deferred_results,
        )


class AgentRuntime:
    """Owned Python runtime with optional durable session-store persistence."""

    def __init__(self, native: _native.AgentRuntime) -> None:
        self._native = native

    @property
    def durable_session_id(self) -> str | None:
        return self._native.durable_session_id

    async def run(self, prompt: str) -> RunResult:
        return RunResult(await self._native.run(prompt))

    async def run_stream(self, prompt: str) -> StreamRunResult:
        return StreamRunResult(await self._native.run_stream(prompt))

    def stream(self, prompt: str) -> AgentRun:
        return AgentRun(self._native.stream(prompt))

    def export_state(self) -> dict[str, Any]:
        return dict(self._native.export_state())

    def export_full_state(self) -> dict[str, Any]:
        return dict(self._native.export_full_state())

    def set_environment(
        self,
        environment: EnvironmentProvider | _native.EnvironmentProvider,
    ) -> None:
        native_environment = ensure_environment_provider(environment)
        if native_environment is None:
            raise TypeError("environment must not be None")
        self._native.set_environment(native_environment)

    async def export_environment_state(self) -> dict[str, Any] | None:
        raw = await self._native.export_environment_state()
        return dict(raw) if raw is not None else None

    async def resume_snapshot(self, session_id: str, run_id: str) -> dict[str, Any]:
        return dict(await self._native.resume_snapshot(session_id, run_id))

    async def resume_after_hitl_by_id(
        self,
        session_id: str,
        run_id: str,
        *,
        approvals: object | None = None,
        deferred_results: object | None = None,
    ) -> RunResult:
        return RunResult(
            await self._native.resume_after_hitl_by_id(
                session_id,
                run_id,
                _approval_payload(approvals),
                _deferred_payload(deferred_results),
            )
        )


class Agent:
    """Python facade over a native Starweaver agent."""

    def __init__(
        self,
        native: _native.Agent,
        *,
        profile_toolsets: Iterable[Toolset | AbstractToolset | Callable[[ToolsetContext], Any]]
        | None = None,
    ) -> None:
        self._native = native
        self._active_runs: WeakSet[AgentRun] = WeakSet()
        self._python_toolsets: list[
            Toolset | AbstractToolset | Callable[[ToolsetContext], Any]
        ] = []
        self._profile_toolsets: list[
            Toolset | AbstractToolset | Callable[[ToolsetContext], Any]
        ] = list(profile_toolsets or ())

    async def __aenter__(self) -> Agent:
        return self

    async def __aexit__(self, exc_type: object, exc: object, tb: object) -> None:
        active = [run for run in list(self._active_runs) if not run.is_finished]
        if exc_type is not None:
            for run in active:
                run.interrupt()
        for run in active:
            try:
                await run.join()
            except BaseException:
                if exc_type is None:
                    raise
        return None

    async def run(
        self,
        prompt: str,
        *,
        instructions: Iterable[str] | None = None,
        tools: Iterable[Tool | BaseTool | Callable[..., Any]] | None = None,
        replace_tools: bool = False,
        model_settings: ModelSettings | dict[str, Any] | None = None,
        request_params: RequestParams | dict[str, Any] | None = None,
        output_schema: OutputSchema | dict[str, Any] | None = None,
        output_policy: OutputPolicy | dict[str, Any] | None = None,
        trace_metadata: Mapping[str, Any] | None = None,
        toolsets: Iterable[Toolset | AbstractToolset | Callable[[ToolsetContext], Any]]
        | None = None,
        environment: EnvironmentProvider | _native.EnvironmentProvider | None = None,
    ) -> RunResult:
        return await self.run_stream(
            prompt,
            instructions=instructions,
            tools=tools,
            replace_tools=replace_tools,
            model_settings=model_settings,
            request_params=request_params,
            output_schema=output_schema,
            output_policy=output_policy,
            trace_metadata=trace_metadata,
            toolsets=toolsets,
            environment=environment,
        ).result()

    def run_stream(
        self,
        prompt: str,
        *,
        instructions: Iterable[str] | None = None,
        tools: Iterable[Tool | BaseTool | Callable[..., Any]] | None = None,
        replace_tools: bool = False,
        model_settings: ModelSettings | dict[str, Any] | None = None,
        request_params: RequestParams | dict[str, Any] | None = None,
        output_schema: OutputSchema | dict[str, Any] | None = None,
        output_policy: OutputPolicy | dict[str, Any] | None = None,
        trace_metadata: Mapping[str, Any] | None = None,
        toolsets: Iterable[Toolset | AbstractToolset | Callable[[ToolsetContext], Any]]
        | None = None,
        environment: EnvironmentProvider | _native.EnvironmentProvider | None = None,
    ) -> AgentRun:
        if output_schema is not None and output_policy is not None:
            raise ValueError("pass output_schema or output_policy, not both")
        native_tools = [ensure_tool(tool).to_native() for tool in tools or ()]
        run = AgentRun(
            self._native.stream(
                prompt,
                list(instructions or ()),
                native_tools,
                replace_tools,
                ensure_model_settings(model_settings),
                ensure_request_params(request_params),
                ensure_output_schema(output_schema),
                ensure_output_policy(output_policy),
                dict(trace_metadata) if trace_metadata is not None else None,
                ensure_toolsets([*self._python_toolsets, *(toolsets or ())]),
                ensure_environment_provider(environment),
            ),
            agent=self,
        )
        self._active_runs.add(run)
        return run

    @overload
    def toolset(
        self,
        factory: Callable[[ToolsetContext], Any],
        /,
        *,
        name: str | None = None,
        id: str | None = None,
        per_run_step: bool = True,
        max_retries: int | None = None,
        timeout_ms: int | None = None,
    ) -> ToolsetFactory: ...

    @overload
    def toolset(
        self,
        factory: None = None,
        /,
        *,
        name: str | None = None,
        id: str | None = None,
        per_run_step: bool = True,
        max_retries: int | None = None,
        timeout_ms: int | None = None,
    ) -> Callable[[Callable[[ToolsetContext], Any]], ToolsetFactory]: ...

    def toolset(
        self,
        factory: Callable[[ToolsetContext], Any] | None = None,
        /,
        *,
        name: str | None = None,
        id: str | None = None,  # noqa: A002
        per_run_step: bool = True,
        max_retries: int | None = None,
        timeout_ms: int | None = None,
    ) -> ToolsetFactory | Callable[[Callable[[ToolsetContext], Any]], ToolsetFactory]:
        """Register a context-aware toolset factory on this Python facade."""

        def wrap(inner: Callable[[ToolsetContext], Any]) -> ToolsetFactory:
            prepared = toolset_factory(
                inner,
                name=name,
                id=id,
                per_run_step=per_run_step,
                max_retries=max_retries,
                timeout_ms=timeout_ms,
            )
            self._python_toolsets.append(prepared)
            self._profile_toolsets.append(prepared)
            return prepared

        if factory is None:
            return wrap
        return wrap(factory)

    def session(
        self,
        state: dict[str, Any] | SessionArchive | None = None,
        *,
        environment: EnvironmentProvider | _native.EnvironmentProvider | None = None,
    ) -> AgentSession:
        if state is None:
            return self.new_session(environment=environment)
        if isinstance(state, SessionArchive):
            return self.session_from_archive(state, environment=environment)
        return self.session_from_state(state, environment=environment)

    def new_session(
        self,
        *,
        environment: EnvironmentProvider | _native.EnvironmentProvider | None = None,
    ) -> AgentSession:
        return AgentSession(
            self._native.new_session(ensure_environment_provider(environment)),
            default_toolsets=self._python_toolsets,
            required_toolsets=self._profile_toolsets,
        )

    def session_from_state(
        self,
        state: dict[str, Any],
        *,
        environment: EnvironmentProvider | _native.EnvironmentProvider | None = None,
    ) -> AgentSession:
        return AgentSession(
            self._native.session_from_state(state, ensure_environment_provider(environment)),
            default_toolsets=self._python_toolsets,
            required_toolsets=self._profile_toolsets,
        )

    def session_from_archive(
        self,
        archive: SessionArchive | Mapping[str, Any],
        *,
        environment: EnvironmentProvider | _native.EnvironmentProvider | None = None,
    ) -> AgentSession:
        archive = _ensure_session_archive(archive)
        _validate_archive_toolset_requirements(
            archive.required_toolset_ids,
            self._profile_toolsets,
        )
        session = self.session_from_state(archive.state, environment=environment)
        session._last_hitl_state = copy.deepcopy(archive.last_run_state)
        return session

    async def steer(self, text: str, **options: Any) -> ControlReceipt:
        active = [run for run in self._active_runs if not run.is_finished]
        if len(active) != 1:
            raise StateError("agent.steer requires exactly one direct active run")
        return await active[0].steer(text, **options)

    def _unregister_run(self, run: AgentRun) -> None:
        self._active_runs.discard(run)


class AgentSession:
    """Stateful Python facade over a Starweaver agent session."""

    def __init__(
        self,
        native: _native.AgentSession,
        *,
        default_toolsets: Iterable[Toolset | AbstractToolset | Callable[[ToolsetContext], Any]]
        | None = None,
        required_toolsets: Iterable[Toolset | AbstractToolset | Callable[[ToolsetContext], Any]]
        | None = None,
    ) -> None:
        self._native = native
        self._active_run: AgentRun | None = None
        self._last_result: RunResult | None = None
        self._last_hitl_state: dict[str, Any] | None = None
        self._default_toolsets = list(default_toolsets or ())
        self._required_toolsets = list(required_toolsets or ())

    async def __aenter__(self) -> AgentSession:
        return self

    async def __aexit__(self, exc_type: object, exc: object, tb: object) -> None:
        run = self.active_run
        if run is not None:
            if exc_type is not None:
                run.interrupt()
            try:
                await run.join()
            except BaseException:
                if exc_type is None:
                    raise
        return None

    async def run(
        self,
        prompt: str,
        *,
        instructions: Iterable[str] | None = None,
        tools: Iterable[Tool | BaseTool | Callable[..., Any]] | None = None,
        replace_tools: bool = False,
        model_settings: ModelSettings | dict[str, Any] | None = None,
        request_params: RequestParams | dict[str, Any] | None = None,
        output_schema: OutputSchema | dict[str, Any] | None = None,
        output_policy: OutputPolicy | dict[str, Any] | None = None,
        trace_metadata: Mapping[str, Any] | None = None,
        toolsets: Iterable[Toolset | AbstractToolset | Callable[[ToolsetContext], Any]]
        | None = None,
        environment: EnvironmentProvider | _native.EnvironmentProvider | None = None,
    ) -> RunResult:
        return await self.run_stream(
            prompt,
            instructions=instructions,
            tools=tools,
            replace_tools=replace_tools,
            model_settings=model_settings,
            request_params=request_params,
            output_schema=output_schema,
            output_policy=output_policy,
            trace_metadata=trace_metadata,
            toolsets=toolsets,
            environment=environment,
        ).result()

    def run_stream(
        self,
        prompt: str,
        *,
        instructions: Iterable[str] | None = None,
        tools: Iterable[Tool | BaseTool | Callable[..., Any]] | None = None,
        replace_tools: bool = False,
        model_settings: ModelSettings | dict[str, Any] | None = None,
        request_params: RequestParams | dict[str, Any] | None = None,
        output_schema: OutputSchema | dict[str, Any] | None = None,
        output_policy: OutputPolicy | dict[str, Any] | None = None,
        trace_metadata: Mapping[str, Any] | None = None,
        toolsets: Iterable[Toolset | AbstractToolset | Callable[[ToolsetContext], Any]]
        | None = None,
        environment: EnvironmentProvider | _native.EnvironmentProvider | None = None,
    ) -> AgentRun:
        if self.active_run is not None:
            raise StateError("session is busy")
        if output_schema is not None and output_policy is not None:
            raise ValueError("pass output_schema or output_policy, not both")
        native_tools = [ensure_tool(tool).to_native() for tool in tools or ()]
        run = AgentRun(
            self._native.stream(
                prompt,
                list(instructions or ()),
                native_tools,
                replace_tools,
                ensure_model_settings(model_settings),
                ensure_request_params(request_params),
                ensure_output_schema(output_schema),
                ensure_output_policy(output_policy),
                dict(trace_metadata) if trace_metadata is not None else None,
                ensure_toolsets([*self._default_toolsets, *(toolsets or ())]),
                ensure_environment_provider(environment),
            ),
            session=self,
        )
        self._active_run = run
        return run

    def export_state(self, mode: str = "curated") -> dict[str, Any]:
        if self.active_run is not None:
            raise StateError("session is busy")
        return _map_native_state_error(lambda: self._native.export_state(mode))

    def export_full_state(self) -> dict[str, Any]:
        return self.export_state("full")

    def set_environment(
        self,
        environment: EnvironmentProvider | _native.EnvironmentProvider,
    ) -> None:
        native_environment = ensure_environment_provider(environment)
        if native_environment is None:
            raise TypeError("environment must not be None")
        self._native.set_environment(native_environment)

    async def export_environment_state(self) -> dict[str, Any] | None:
        return await self._native.export_environment_state()

    def archive(self, *, mode: Literal["full", "curated"] = "full") -> SessionArchive:
        return SessionArchive.from_session(self, mode=mode)

    async def resume_after_hitl(
        self,
        *,
        approvals: object | None = None,
        deferred_results: object | None = None,
    ) -> RunResult:
        if self.active_run is not None:
            raise StateError("session is busy")
        approval_payload = _approval_payload(approvals)
        deferred_payload = _deferred_payload(deferred_results)
        if self._last_hitl_state is not None:
            result = RunResult(
                await self._native.resume_after_hitl_for_state(
                    self._last_hitl_state,
                    approval_payload,
                    deferred_payload,
                )
            )
        else:
            result = RunResult(
                await self._native.resume_after_hitl(approval_payload, deferred_payload)
            )
        self._set_last_result(result)
        return result

    async def resume_after_hitl_stream(
        self,
        *,
        approvals: object | None = None,
        deferred_results: object | None = None,
    ) -> AgentRun:
        if self.active_run is not None:
            raise StateError("session is busy")
        continuation = AgentRun(
            await self._native.resume_after_hitl_stream(
                _approval_payload(approvals),
                _deferred_payload(deferred_results),
            ),
            session=self,
        )
        self._active_run = continuation
        return continuation

    async def steer(self, text: str, **options: Any) -> ControlReceipt:
        when_idle = options.pop("when_idle", None)
        if when_idle is not None and when_idle != "queue":
            raise ValueError("when_idle must be 'queue' when provided")
        message_id = _optional_str(options.pop("id", None))
        if options:
            unknown = ", ".join(sorted(options))
            raise TypeError(f"unexpected steering options: {unknown}")
        run = self.active_run
        if run is not None:
            run_options = {"id": message_id} if message_id is not None else {}
            return await run.steer(text, **run_options)
        if when_idle != "queue":
            raise StateError("no active run for session")
        delivery = await self.messages.steer(
            text,
            when_idle="queue",
            id=message_id,
        )
        state = self.export_state("curated")
        return ControlReceipt(
            id=delivery.message.id,
            kind="steering",
            queued=True,
            session_id=_optional_str(state.get("session_id")),
        )

    def interrupt(self, reason: str | None = None) -> None:
        run = self.active_run
        if run is None:
            raise StateError("no active run for session")
        run.interrupt(reason)

    @property
    def messages(self) -> MessageBus:
        return MessageBus(session=self)

    @property
    def active_run(self) -> AgentRun | None:
        if self._active_run is not None and self._active_run.is_finished:
            self._active_run = None
        return self._active_run

    @property
    def hitl(self) -> SessionHitl:
        return SessionHitl(self)

    def _finish_run(self, run: AgentRun, result: RunResult | None = None) -> None:
        if self._active_run is run:
            self._active_run = None
        if result is not None:
            self._set_last_result(result)

    def _set_last_result(self, result: RunResult) -> None:
        self._last_result = result
        if result.is_waiting or result.hitl.approvals or result.hitl.deferred:
            self._last_hitl_state = copy.deepcopy(result.raw_run_state)
        else:
            self._last_hitl_state = None


class AgentRun:
    """Live stream handle for one agent run."""

    def __init__(
        self,
        native: _native.AgentStream,
        *,
        agent: Agent | None = None,
        session: AgentSession | None = None,
    ) -> None:
        self._native = native
        self._agent = agent
        self._session = session
        self._joined: StreamRunResult | None = None
        self._join_future: asyncio.Future[Any] | None = None
        self._finished = False
        self._detached = False
        self._terminal_event_seen = False
        self._suspended_seen = False
        self._last_hitl_state: dict[str, Any] | None = None

    async def __aenter__(self) -> AgentRun:
        return self

    async def __aexit__(self, exc_type: object, exc: object, tb: object) -> None:
        if self._detached:
            return None
        if exc_type is not None:
            self.interrupt()
        if self._joined is None:
            try:
                await self.join()
            except BaseException:
                if exc_type is None:
                    raise

    def __aiter__(self) -> AsyncIterator[StreamEvent]:
        return self

    async def __anext__(self) -> StreamEvent:
        event = await self.recv()
        if event is None:
            await self.join()
            raise StopAsyncIteration
        return event

    @property
    def is_finished(self) -> bool:
        return self._finished

    async def recv(self) -> StreamEvent | None:
        if self._detached:
            raise StateError("agent run has been detached")
        try:
            event = await self._native.recv()
        except asyncio.CancelledError:
            self.interrupt()
            raise
        if event is None:
            return None
        wrapped = StreamEvent(event)
        if wrapped.kind == "suspended":
            self._suspended_seen = True
        if wrapped.is_terminal:
            self._terminal_event_seen = True
        return wrapped

    def interrupt(self, reason: str | None = None) -> None:
        try:
            self._native.interrupt(reason)
        except TypeError:
            self._native.interrupt()

    def close_receiver(self) -> None:
        if self._detached:
            raise StateError("agent run has been detached")
        self._native.close_receiver()

    def detach(self) -> None:
        if self._joined is not None or self._finished:
            return
        self._native.close_receiver()
        self._detached = True
        if self._join_future is None:
            self._join_future = asyncio.ensure_future(self._native.join())
            self._join_future.add_done_callback(self._finalize_join_future)

    async def steer(self, text: str, **options: Any) -> ControlReceipt:
        if self._joined is not None or self._finished or self._terminal_event_seen:
            raise StateError("agent run has already completed")
        message_id = _optional_str(options.pop("id", None))
        if options:
            unknown = ", ".join(sorted(options))
            raise TypeError(f"unexpected steering options: {unknown}")
        receipt = await self._native.steer(text, message_id)
        return ControlReceipt.from_raw(receipt)

    async def send_message(self, message: BusMessage | Mapping[str, Any]) -> ControlReceipt:
        if self._joined is not None or self._finished or self._terminal_event_seen:
            raise StateError("agent run has already completed")
        bus_message = _ensure_bus_message(message)
        receipt = await self._native.send_message(bus_message.to_dict())
        return ControlReceipt.from_raw(receipt)

    @property
    def messages(self) -> MessageBus:
        return MessageBus(run=self)

    def hitl(self) -> RunHitl:
        return RunHitl(self)

    def status(self) -> RunStatusSnapshot:
        snapshot = RunStatusSnapshot.from_raw(self._native.status())
        if snapshot.run_status == "finished":
            self._terminal_event_seen = True
        return snapshot

    async def recoverable_state(self) -> dict[str, Any]:
        if self._detached:
            raise StateError("agent run has been detached")
        return await self._native.recoverable_state()

    async def join(self) -> StreamRunResult:
        if self._detached:
            raise StateError("agent run has been detached")
        if self._joined is not None:
            return self._joined
        if self._join_future is None:
            self._join_future = asyncio.ensure_future(self._native.join())
            self._join_future.add_done_callback(self._finalize_join_future)
        try:
            await asyncio.shield(self._join_future)
            if self._joined is None:
                self._finalize_join_future(self._join_future)
            if self._joined is None:
                raise StateError("stream completed without a result")
            return self._joined
        except asyncio.CancelledError:
            self.interrupt()
            raise
        except Exception:
            self._mark_finished(None)
            raise

    async def result(self) -> RunResult:
        return (await self.join()).result

    async def _resume_hitl_collected(
        self,
        *,
        approvals: object | None = None,
        deferred_results: object | None = None,
    ) -> RunResult:
        if self._session is not None:
            if self._joined is None:
                await self.join()
            result = await self._session.resume_after_hitl(
                approvals=approvals,
                deferred_results=deferred_results,
            )
            self._replace_joined_result(result)
            return result
        if self._last_hitl_state is None:
            if self._joined is None:
                await self.join()
            if self._joined is not None:
                self._set_hitl_state_from_result(self._joined.result)
        if self._last_hitl_state is None:
            raise StateError("run has no collected HITL state to resume")
        result = RunResult(
            await self._native.resume_after_hitl_for_state(
                self._last_hitl_state,
                _approval_payload(approvals),
                _deferred_payload(deferred_results),
            )
        )
        self._set_hitl_state_from_result(result)
        self._replace_joined_result(result)
        return result

    async def _resume_hitl_stream(
        self,
        *,
        approvals: object | None = None,
        deferred_results: object | None = None,
    ) -> AgentRun:
        if self._joined is None:
            await self.join()
        if self._session is not None:
            return await self._session.resume_after_hitl_stream(
                approvals=approvals,
                deferred_results=deferred_results,
            )
        continuation = AgentRun(
            await self._native.resume_after_hitl_stream(
                _approval_payload(approvals),
                _deferred_payload(deferred_results),
            ),
            agent=self._agent,
        )
        if self._agent is not None:
            self._agent._active_runs.add(continuation)
        return continuation

    def _replace_joined_result(self, result: RunResult) -> None:
        events = self._joined.events if self._joined is not None else []
        self._joined = StreamRunResult.from_parts(result=result, events=events)
        self._set_hitl_state_from_result(result)
        self._mark_finished(result)

    def _finalize_join_future(self, future: asyncio.Future[Any]) -> None:
        if self._joined is not None:
            return
        if future.cancelled():
            self._mark_finished(None)
            return
        try:
            joined = StreamRunResult(future.result())
        except BaseException:
            self._mark_finished(None)
            return
        self._joined = joined
        self._set_hitl_state_from_result(joined.result)
        self._mark_finished(joined.result)

    def _set_hitl_state_from_result(self, result: RunResult) -> None:
        if result.is_waiting or result.hitl.approvals or result.hitl.deferred:
            self._last_hitl_state = copy.deepcopy(result.raw_run_state)
        else:
            self._last_hitl_state = None

    def _mark_finished(self, result: RunResult | None) -> None:
        self._finished = True
        if self._session is not None:
            self._session._finish_run(self, result)
        if self._agent is not None:
            self._agent._unregister_run(self)


AgentStream = AgentRun


def create_agent(
    *,
    model: Any,
    tools: Iterable[Tool | BaseTool | Callable[..., Any]] | None = None,
    instructions: Iterable[str] | None = None,
    name: str | None = None,
    model_settings: ModelSettings | dict[str, Any] | None = None,
    request_params: RequestParams | dict[str, Any] | None = None,
    output_schema: OutputSchema | dict[str, Any] | None = None,
    output_policy: OutputPolicy | dict[str, Any] | None = None,
    subagents: Iterable[Subagent] | None = None,
    subagent_delegation_mode: str = "blocking",
    capability_bundles: Iterable[CapabilityBundle] | None = None,
    toolsets: Iterable[Toolset | AbstractToolset | Callable[[ToolsetContext], Any]] | None = None,
    approval_required_tools: Iterable[str] | None = None,
    runtime_config: RuntimeConfig | Mapping[str, Any] | None = None,
    skills: SkillRegistry | _native.SkillRegistry | None = None,
    environment: EnvironmentProvider | _native.EnvironmentProvider | None = None,
    media_uploader: MediaUploader | _native.MediaUploader | None = None,
) -> Agent:
    """Create a Python Starweaver agent."""

    if output_schema is not None and output_policy is not None:
        raise ValueError("pass output_schema or output_policy, not both")
    to_native = getattr(model, "to_native", None)
    native_model = to_native() if callable(to_native) else getattr(model, "_native", model)
    native_tools = [ensure_tool(tool).to_native() for tool in tools or ()]
    native_subagents = [ensure_subagent(subagent) for subagent in subagents or ()]
    native_bundles = [ensure_capability_bundle(bundle) for bundle in capability_bundles or ()]
    profile_toolsets = list(toolsets or ())
    return Agent(
        _native.Agent(
            native_model,
            native_tools,
            list(instructions or ()),
            name,
            ensure_model_settings(model_settings),
            ensure_request_params(request_params),
            ensure_output_schema(output_schema),
            ensure_output_policy(output_policy),
            native_subagents,
            subagent_delegation_mode,
            native_bundles,
            ensure_toolsets(profile_toolsets),
            list(approval_required_tools or ()),
            ensure_runtime_config(runtime_config),
            ensure_skill_registry(skills),
            ensure_environment_provider(environment),
            ensure_media_uploader(media_uploader),
        ),
        profile_toolsets=profile_toolsets,
    )


def create_agent_runtime(
    *,
    model: Any,
    tools: Iterable[Tool | BaseTool | Callable[..., Any]] | None = None,
    instructions: Iterable[str] | None = None,
    name: str | None = None,
    model_settings: ModelSettings | dict[str, Any] | None = None,
    request_params: RequestParams | dict[str, Any] | None = None,
    output_schema: OutputSchema | dict[str, Any] | None = None,
    output_policy: OutputPolicy | dict[str, Any] | None = None,
    subagents: Iterable[Subagent] | None = None,
    subagent_delegation_mode: str = "blocking",
    capability_bundles: Iterable[CapabilityBundle] | None = None,
    toolsets: Iterable[Toolset | AbstractToolset | Callable[[ToolsetContext], Any]] | None = None,
    approval_required_tools: Iterable[str] | None = None,
    runtime_config: RuntimeConfig | Mapping[str, Any] | None = None,
    skills: SkillRegistry | _native.SkillRegistry | None = None,
    environment: EnvironmentProvider | _native.EnvironmentProvider | None = None,
    media_uploader: MediaUploader | _native.MediaUploader | None = None,
    session_store: Any | None = None,
    durable_session_id: str | None = None,
    stream_archive: Any | None = None,
    replay_event_log: Any | None = None,
    state: Mapping[str, Any] | None = None,
) -> AgentRuntime:
    """Create an owned Starweaver runtime with optional durable storage."""

    if output_schema is not None and output_policy is not None:
        raise ValueError("pass output_schema or output_policy, not both")
    to_native = getattr(model, "to_native", None)
    native_model = to_native() if callable(to_native) else getattr(model, "_native", model)
    native_tools = [ensure_tool(tool).to_native() for tool in tools or ()]
    native_subagents = [ensure_subagent(subagent) for subagent in subagents or ()]
    native_bundles = [ensure_capability_bundle(bundle) for bundle in capability_bundles or ()]
    return AgentRuntime(
        _native.AgentRuntime(
            native_model,
            native_tools,
            list(instructions or ()),
            name,
            ensure_model_settings(model_settings),
            ensure_request_params(request_params),
            ensure_output_schema(output_schema),
            ensure_output_policy(output_policy),
            native_subagents,
            subagent_delegation_mode,
            native_bundles,
            ensure_toolsets(toolsets),
            list(approval_required_tools or ()),
            ensure_runtime_config(runtime_config),
            ensure_skill_registry(skills),
            ensure_environment_provider(environment),
            ensure_media_uploader(media_uploader),
            session_store,
            durable_session_id,
            stream_archive,
            replay_event_log,
            dict(state) if state is not None else None,
        )
    )


def _ensure_bus_message(message: BusMessage | Mapping[str, Any]) -> BusMessage:
    if isinstance(message, BusMessage):
        return message
    return BusMessage.from_raw(message)


def _ensure_session_archive(archive: SessionArchive | Mapping[str, Any]) -> SessionArchive:
    if isinstance(archive, SessionArchive):
        return archive
    return SessionArchive.from_dict(archive)


def _normalize_required_toolset_ids(values: object) -> tuple[str, ...]:
    if values is None:
        return ()
    if isinstance(values, str | bytes | bytearray) or not isinstance(values, Iterable):
        raise TypeError("required_toolset_ids must be an iterable of strings")
    ids: list[str] = []
    for value in values:
        if not isinstance(value, str):
            raise TypeError("required_toolset_ids entries must be strings")
        if not value.strip():
            raise ValueError("required_toolset_ids entries must not be empty")
        ids.append(value)
    duplicates = sorted({value for value in ids if ids.count(value) > 1})
    if duplicates:
        raise ValueError(f"required_toolset_ids contains duplicates: {duplicates}")
    return tuple(ids)


def _required_toolset_ids_for_archive(
    toolsets: Iterable[Toolset | AbstractToolset | Callable[[ToolsetContext], Any]],
) -> tuple[str, ...]:
    values = tuple(toolsets)
    if not values:
        return ()
    validation = validate_toolsets_for_durability(values)
    validation.require_serializable_dynamic_state()
    return tuple(identity.id for identity in validation.identities if identity.id is not None)


def _validate_archive_toolset_requirements(
    required_ids: Iterable[str],
    toolsets: Iterable[Toolset | AbstractToolset | Callable[[ToolsetContext], Any]],
) -> None:
    required = set(_normalize_required_toolset_ids(required_ids))
    if not required:
        return
    try:
        current_ids = set(_required_toolset_ids_for_archive(toolsets))
    except ValueError as error:
        raise StateError(f"current agent toolsets are not durable: {error}") from error
    missing = sorted(required - current_ids)
    if missing:
        details = ", ".join(missing)
        raise StateError(
            f"restored session requires toolset ids missing from current agent: {details}"
        )


def _approval_payload(approvals: object | None) -> object | None:
    if approvals is None:
        return approvals
    if isinstance(approvals, Mapping):
        if "approved" in approvals and ("approval_id" in approvals or "id" in approvals):
            decision_id = approvals.get("approval_id") or approvals.get("id")
            return {str(decision_id): dict(approvals)}
        return approvals
    if isinstance(approvals, ApprovalDecision):
        return {approvals.id: approvals.to_dict()}
    if isinstance(approvals, Sequence) and not isinstance(approvals, (str, bytes, bytearray)):
        payload: dict[str, Any] = {}
        for decision in approvals:
            if isinstance(decision, ApprovalDecision):
                payload[decision.id] = decision.to_dict()
            elif isinstance(decision, Mapping):
                decision_id = decision.get("id") or decision.get("approval_id")
                if decision_id is None:
                    raise StateError("approval mapping must include id or approval_id")
                payload[str(decision_id)] = dict(decision)
            else:
                raise TypeError("approvals must be a mapping or ApprovalDecision sequence")
        return payload
    raise TypeError("approvals must be a mapping or ApprovalDecision sequence")


def _deferred_payload(deferred_results: object | None) -> object | None:
    if deferred_results is None:
        return None
    if isinstance(deferred_results, Mapping):
        if "deferred_id" in deferred_results and "status" in deferred_results:
            return {"results": [dict(deferred_results)]}
        return deferred_results
    if isinstance(deferred_results, DeferredResult):
        return {"results": [deferred_results.to_dict()]}
    if isinstance(deferred_results, Sequence) and not isinstance(
        deferred_results, (str, bytes, bytearray)
    ):
        results = []
        for result in deferred_results:
            if isinstance(result, DeferredResult):
                results.append(result.to_dict())
            elif isinstance(result, Mapping):
                results.append(dict(result))
            else:
                raise TypeError("deferred_results must contain DeferredResult or mappings")
        return {"results": results}
    raise TypeError("deferred_results must be a mapping or DeferredResult sequence")


def _merge_metadata(
    metadata: Mapping[str, Any] | None,
    extra_metadata: Mapping[str, Any],
) -> dict[str, Any]:
    merged = dict(metadata or {})
    merged.update(extra_metadata)
    return merged


def _optional_str(value: object) -> str | None:
    return None if value is None else str(value)


def _optional_int(value: object) -> int | None:
    if value is None:
        return None
    if isinstance(value, int | float | str | bytes | bytearray):
        return int(value)
    raise TypeError(f"expected int-compatible value, got {type(value).__name__}")


def _event_payload(raw: Mapping[str, Any]) -> dict[str, Any]:
    event = raw.get("event")
    return dict(event) if isinstance(event, Mapping) else {}


def _nested_event_payload(raw: Mapping[str, Any]) -> dict[str, Any]:
    nested = _event_payload(raw).get("event")
    return dict(nested) if isinstance(nested, Mapping) else {}


def _event_kind(raw: Mapping[str, Any]) -> str:
    return str(_event_payload(raw).get("kind") or raw.get("kind") or "unknown")


def _hitl_snapshot_from_run_state(state: Mapping[str, Any]) -> HitlSnapshot:
    run_id = _optional_str(state.get("run_id")) or ""
    approvals = [
        _pending_tool_return_with_id(item, id_field="approval_id", prefix="approval", run_id=run_id)
        for item in _mapping_list(state.get("pending_approval_tool_returns"))
    ]
    deferred = [
        _pending_tool_return_with_id(item, id_field="deferred_id", prefix="deferred", run_id=run_id)
        for item in _mapping_list(state.get("deferred_tool_returns"))
    ]
    return HitlSnapshot(
        approvals=[PendingApproval.from_raw(item) for item in approvals],
        deferred=[PendingDeferred.from_raw(item) for item in deferred],
        raw_approvals=approvals,
        raw_deferred=deferred,
    )


def _pending_tool_return_with_id(
    item: Mapping[str, Any],
    *,
    id_field: str,
    prefix: str,
    run_id: str,
) -> dict[str, Any]:
    raw = dict(item)
    tool_call_id = _optional_str(raw.get("tool_call_id") or raw.get("id")) or ""
    raw.setdefault(id_field, f"{prefix}_{run_id}_{tool_call_id}" if run_id else tool_call_id)
    raw.setdefault("tool_name", raw.get("name") or "")
    return raw


def _mapping_list(value: object) -> list[dict[str, Any]]:
    if not isinstance(value, Sequence) or isinstance(value, (str, bytes, bytearray)):
        return []
    return [dict(item) for item in value if isinstance(item, Mapping)]


def _map_native_state_error(call: Callable[[], Any]) -> Any:
    try:
        return call()
    except RuntimeError as error:
        message = str(error)
        if (
            "session is busy" in message
            or "no active run for session" in message
            or "stream has already completed" in message
        ):
            raise StateError(message) from error
        raise
