"""Projection helpers for canonical Starweaver stream records."""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from typing import Any

from .agent import StreamEvent


class StreamAdapter:
    """Convert canonical stream events into application-facing projections."""

    def __init__(self, events: Iterable[StreamEvent | Mapping[str, Any]] = ()) -> None:
        self.events = [
            event if isinstance(event, StreamEvent) else StreamEvent(event) for event in events
        ]

    def records(self) -> list[dict[str, Any]]:
        return [event.raw for event in self.events]

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
        return [event.usage for event in self.events if event.usage is not None]

    def terminal(self) -> StreamEvent | None:
        for event in reversed(self.events):
            if event.is_terminal:
                return event
        return None

    @staticmethod
    def records_from(events: Iterable[StreamEvent | Mapping[str, Any]]) -> list[dict[str, Any]]:
        return StreamAdapter(events).records()

    @staticmethod
    def text_from(events: Iterable[StreamEvent | Mapping[str, Any]]) -> str:
        return StreamAdapter(events).text()
