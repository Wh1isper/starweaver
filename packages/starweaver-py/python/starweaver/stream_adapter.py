"""Projection helpers for canonical Starweaver stream records."""

from __future__ import annotations

import json
from collections.abc import Iterable, Mapping
from typing import Any

from . import _native
from .agent import StreamEvent
from .observability import UsageSnapshot
from .toolset import ToolsetLifecycleReport


class StreamAdapter:
    """Convert canonical stream events into application-facing projections."""

    def __init__(self, events: Iterable[StreamEvent | Mapping[str, Any]] = ()) -> None:
        self.events = [
            event if isinstance(event, StreamEvent) else StreamEvent(event) for event in events
        ]

    def records(self) -> list[dict[str, Any]]:
        return [event.raw for event in self.events]

    def ordered_records(self) -> list[dict[str, Any]]:
        indexed = list(enumerate(self.events))
        indexed.sort(key=lambda item: (_event_sequence(item[1], item[0]), item[0]))
        return [event.raw for _, event in indexed]

    def text(self) -> str:
        return "".join(event.text_delta or "" for event in self.events)

    def text_deltas(self) -> list[str]:
        return [event.text_delta for event in self.events if event.text_delta is not None]

    def tool_events(self) -> list[dict[str, Any]]:
        events: list[dict[str, Any]] = []
        for event in self.events:
            if event.tool_call is not None:
                events.append({"kind": "tool_call", **event.tool_call})
            if event.tool_return is not None:
                events.append({"kind": "tool_return", **event.tool_return})
        return events

    def sideband_events(self, *, kind: str | None = None) -> list[dict[str, Any]]:
        events = [event.sideband for event in self.events if event.sideband is not None]
        if kind is not None:
            events = [event for event in events if event.get("kind") == kind]
        return events

    def usage_snapshots(self) -> list[dict[str, Any]]:
        snapshots: list[dict[str, Any]] = []
        for event in self.events:
            if event.usage_snapshot is not None:
                snapshots.append(event.usage_snapshot.to_dict())
            elif event.usage is not None:
                snapshots.append(event.usage)
        return snapshots

    def typed_usage_snapshots(self) -> list[UsageSnapshot]:
        snapshots: list[UsageSnapshot] = []
        for event in self.events:
            snapshot = event.usage_snapshot
            if snapshot is not None:
                snapshots.append(snapshot)
        return snapshots

    def toolset_lifecycle_reports(self) -> list[ToolsetLifecycleReport]:
        reports: list[ToolsetLifecycleReport] = []
        for event in self.events:
            report = event.toolset_lifecycle_report
            if report is not None:
                reports.append(report)
        return reports

    def terminal(self) -> StreamEvent | None:
        for event in reversed(self.events):
            if event.is_terminal:
                return event
        return None

    def cursor_range(self, *, scope: str | None = None) -> dict[str, Any] | None:
        if not self.events:
            return None
        ordered = [
            _cursor(scope or _infer_scope(self.events), _event_sequence(event, index))
            for index, event in enumerate(self.events)
        ]
        ordered.sort(key=lambda cursor: int(cursor["sequence"]))
        return {"first": ordered[0], "last": ordered[-1]}

    def replay_window(
        self,
        *,
        after_sequence: int | None = None,
        limit: int | None = None,
    ) -> list[dict[str, Any]]:
        records = [
            event.raw
            for index, event in enumerate(self.events)
            if after_sequence is None or _event_sequence(event, index) > after_sequence
        ]
        records.sort(key=lambda record: _raw_sequence(record))
        return records[:limit] if limit is not None else records

    def replay_buffer(
        self,
        *,
        session_id: str,
        run_id: str | None = None,
        scope: str | None = None,
    ) -> dict[str, Any]:
        display_messages = self.display_messages(session_id=session_id, run_id=run_id)
        terminal = self.terminal()
        return {
            "raw_records": self.ordered_records(),
            "display_messages": display_messages,
            "cursor_range": self.cursor_range(scope=scope),
            "text": self.text(),
            "terminal": terminal.raw if terminal is not None else None,
        }

    def display_messages(
        self,
        *,
        session_id: str,
        run_id: str | None = None,
    ) -> list[dict[str, Any]]:
        resolved_run_id = run_id or _infer_run_id(self.events)
        if not resolved_run_id:
            raise ValueError("run_id is required when stream records do not include one")
        ordered = sorted(
            enumerate(self.events),
            key=lambda item: (_event_sequence(item[1], item[0]), item[0]),
        )
        messages: list[dict[str, Any]] = []
        for index, event in ordered:
            if _is_unknown_extension(event):
                # Unknown extension kinds remain lossless host events. Known canonical kinds are
                # always decoded by Rust, so malformed canonical payloads propagate as errors.
                messages.append(
                    _display_message(
                        event,
                        sequence=_event_sequence(event, index),
                        session_id=session_id,
                        run_id=event.run_id or resolved_run_id,
                    )
                )
                continue
            projected = _native.project_stream_records_to_display(
                [event.raw],
                session_id,
                event.run_id or resolved_run_id,
            )
            messages.extend(dict(message) for message in projected)
        return messages

    def agui_events(
        self,
        *,
        session_id: str,
        run_id: str | None = None,
    ) -> list[dict[str, Any]]:
        return [
            _display_to_agui(message)
            for message in self.display_messages(
                session_id=session_id,
                run_id=run_id,
            )
        ]

    def agui_jsonl(self, *, session_id: str, run_id: str | None = None) -> str:
        return "".join(
            json.dumps(event, separators=(",", ":"), sort_keys=True) + "\n"
            for event in self.agui_events(session_id=session_id, run_id=run_id)
        )

    def sse_frames(self, *, scope: str | None = None) -> list[dict[str, Any]]:
        resolved_scope = scope or _infer_scope(self.events)
        frames: list[dict[str, Any]] = []
        for index, event in sorted(
            enumerate(self.events),
            key=lambda item: (_event_sequence(item[1], item[0]), item[0]),
        ):
            sequence = _event_sequence(event, index)
            frames.append(
                {
                    "id": str(sequence),
                    "event": "raw",
                    "data": event.raw,
                    "cursor": _cursor(resolved_scope, sequence),
                }
            )
        return frames

    def sse_text(self, *, scope: str | None = None) -> str:
        return "".join(_sse_frame_to_text(frame) for frame in self.sse_frames(scope=scope))

    @staticmethod
    def records_from(events: Iterable[StreamEvent | Mapping[str, Any]]) -> list[dict[str, Any]]:
        return StreamAdapter(events).records()

    @staticmethod
    def text_from(events: Iterable[StreamEvent | Mapping[str, Any]]) -> str:
        return StreamAdapter(events).text()


_CANONICAL_STREAM_KINDS = frozenset(
    {
        "run_start",
        "node_start",
        "node_complete",
        "custom",
        "model_request",
        "model_stream",
        "model_response",
        "checkpoint",
        "suspended",
        "tool_call",
        "tool_return",
        "output_retry",
        "steering_guard",
        "run_complete",
        "run_cancelled",
        "run_failed",
    }
)


def _is_unknown_extension(event: StreamEvent) -> bool:
    return event.kind not in _CANONICAL_STREAM_KINDS


def _event_sequence(event: StreamEvent, fallback: int) -> int:
    sequence = event.raw.get("sequence", fallback)
    return int(sequence) if isinstance(sequence, int | float | str) else fallback


def _raw_sequence(record: Mapping[str, Any]) -> int:
    sequence = record.get("sequence", 0)
    return int(sequence) if isinstance(sequence, int | float | str) else 0


def _event_timestamp(event: StreamEvent) -> str:
    raw_timestamp = event.raw.get("timestamp")
    if isinstance(raw_timestamp, str):
        return raw_timestamp
    payload_timestamp = _event_payload(event).get("timestamp")
    if isinstance(payload_timestamp, str):
        return payload_timestamp
    return "1970-01-01T00:00:00+00:00"


def _event_payload(event: StreamEvent) -> dict[str, Any]:
    payload = event.raw.get("event")
    return dict(payload) if isinstance(payload, Mapping) else {}


def _nested_event(event: StreamEvent) -> dict[str, Any]:
    nested = _event_payload(event).get("event")
    return dict(nested) if isinstance(nested, Mapping) else {}


def _infer_run_id(events: Iterable[StreamEvent]) -> str | None:
    for event in events:
        if event.run_id:
            return event.run_id
    return None


def _infer_scope(events: Iterable[StreamEvent]) -> str:
    run_id = _infer_run_id(events)
    return f"run:{run_id}" if run_id is not None else "stream"


def _cursor(scope: str, sequence: int) -> dict[str, Any]:
    return {"scope": scope, "sequence": sequence}


def _display_message(
    event: StreamEvent,
    *,
    sequence: int,
    session_id: str,
    run_id: str,
) -> dict[str, Any]:
    kind = _display_kind(event)
    payload = _display_payload(event)
    preview = _display_preview(kind, payload, event)
    message: dict[str, Any] = {
        "schema": "starweaver.display.v1",
        "sequence": sequence,
        "session_id": session_id,
        "run_id": run_id,
        "timestamp": _event_timestamp(event),
        "type": kind,
        "payload": payload,
        "visibility": "public",
        "metadata": {"stream_kind": event.kind},
    }
    if preview is not None:
        message["preview"] = preview
    return message


def _display_kind(event: StreamEvent) -> str:
    if event.kind == "run_start":
        return "RUN_STARTED"
    if event.kind == "run_complete":
        return "RUN_FINISHED"
    if event.kind == "run_failed":
        return "RUN_ERROR"
    if event.kind == "model_stream" and event.text_delta is not None:
        return "TEXT_MESSAGE_CONTENT"
    if event.kind == "tool_call":
        return "TOOL_CALL_START"
    if event.kind == "tool_return":
        return "TOOL_CALL_RESULT"
    if event.kind == "custom":
        return _custom_display_kind(event.sideband_kind)
    return "HOST_EVENT"


def _custom_display_kind(kind: str | None) -> str:
    if not kind:
        return "HOST_EVENT"
    mapping = {
        "toolset_initialized": "TOOLSET_INITIALIZED",
        "toolset_unavailable": "TOOLSET_UNAVAILABLE",
        "toolset_failed": "TOOLSET_FAILED",
        "toolset_refreshed": "TOOLSET_REFRESHED",
        "toolset_closed": "TOOLSET_CLOSED",
        "approval_requested": "APPROVAL_REQUESTED",
        "approval_resolved": "APPROVAL_RESOLVED",
        "hitl_resolved": "HITL_RESOLVED",
        "hitl_diagnostic": "HITL_DIAGNOSTIC",
        "subagent_started": "SUBAGENT_STARTED",
        "subagent_completed": "SUBAGENT_COMPLETED",
        "subagent_failed": "SUBAGENT_FAILED",
        "steering_submitted": "STEERING_SUBMITTED",
        "steering_received": "STEERING_RECEIVED",
        "task_snapshot": "TASK_SNAPSHOT",
        "task_event": "TASK_EVENT",
        "note_event": "NOTE_EVENT",
        "file_event": "FILE_EVENT",
        "media_event": "MEDIA_EVENT",
    }
    return mapping.get(kind, "HOST_EVENT")


def _display_payload(event: StreamEvent) -> dict[str, Any]:
    if event.kind == "model_stream" and event.text_delta is not None:
        return {"text_delta": event.text_delta}
    if event.tool_call is not None:
        return event.tool_call
    if event.tool_return is not None:
        return event.tool_return
    if event.sideband is not None:
        return event.sideband
    payload = _event_payload(event)
    nested = _nested_event(event)
    if nested:
        return {**payload, "event": nested}
    return payload if payload else dict(event.raw)


def _display_preview(kind: str, payload: Mapping[str, Any], event: StreamEvent) -> str | None:
    if kind == "TEXT_MESSAGE_CONTENT":
        return event.text_delta
    if kind in {"TOOL_CALL_START", "TOOL_CALL_RESULT"}:
        name = payload.get("name") or payload.get("tool_name")
        return str(name) if name is not None else None
    if kind == "RUN_STARTED":
        return "run started"
    if kind == "RUN_FINISHED":
        return "run completed"
    if kind == "RUN_ERROR":
        return str(payload.get("error") or payload.get("message") or "run failed")
    return None


def _display_to_agui(message: Mapping[str, Any]) -> dict[str, Any]:
    try:
        return dict(_native.display_to_agui(dict(message)))
    except (TypeError, ValueError, RuntimeError):
        return _display_to_agui_fallback(message)


def _display_to_agui_fallback(message: Mapping[str, Any]) -> dict[str, Any]:
    payload = dict(message.get("payload") or {})
    payload["timestamp"] = message["timestamp"]
    if message.get("preview") is not None:
        payload["preview"] = message["preview"]
    return {
        "type": str(message["type"]),
        "id": str(message["sequence"]),
        "sequence": int(message["sequence"]),
        "session_id": str(message["session_id"]),
        "run_id": str(message["run_id"]),
        "payload": payload,
    }


def _sse_frame_to_text(frame: Mapping[str, Any]) -> str:
    lines = [
        f"id: {frame['id']}",
        f"event: {frame['event']}",
        f"data: {json.dumps(frame['data'], separators=(',', ':'), sort_keys=True)}",
    ]
    return "\n".join(lines) + "\n\n"
