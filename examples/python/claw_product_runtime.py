from __future__ import annotations

import asyncio
import hashlib
import json
import re
import sqlite3
import tempfile
import uuid
from collections.abc import AsyncIterator, Awaitable, Callable, Mapping, Sequence
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, Literal, cast

from starweaver import (
    AbstractToolset,
    Agent,
    AgentRun,
    AgentRuntime,
    AgentSession,
    ApprovalDecision,
    ApprovalRequired,
    EnvironmentProvider,
    RunRecord,
    SessionArchive,
    SqliteReplayEventLog,
    SqliteSessionStore,
    SqliteStreamArchive,
    StreamAdapter,
    StreamEvent,
    Tool,
    ToolContext,
    ToolsetContext,
    ToolsetPreparation,
    VirtualMount,
    WorkspaceBinding,
    create_agent,
    create_agent_runtime,
    validate_toolset_ids,
)
from starweaver.testing import FunctionModel

JsonObject = dict[str, Any]
WorkspaceMountMode = Literal["read_write", "read_only"]


def _now() -> str:
    return "2026-01-01T00:00:00Z"


def _parse_time(value: Any) -> datetime | None:
    if not isinstance(value, str) or not value:
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None


def _new_id(prefix: str) -> str:
    return f"{prefix}_{uuid.uuid4().hex}"


def _encode(value: Any) -> str:
    return json.dumps(value, sort_keys=True)


def _decode(value: str | None, default: Any) -> Any:
    if value is None:
        return default
    return json.loads(value)


def _extract_marker(text: str, key: str) -> str:
    match = re.search(rf"{re.escape(key)}=([A-Za-z0-9_.:-]+)", text)
    if match is None:
        raise RuntimeError(f"missing marker: {key}")
    return match.group(1)


@dataclass(frozen=True)
class ProductRunView:
    run_id: str
    session_id: str
    status: str
    behavior: str
    output: str | None = None
    native_session_id: str | None = None
    native_run_id: str | None = None


def _run_view_dict(view: ProductRunView) -> JsonObject:
    return {
        "run_id": view.run_id,
        "session_id": view.session_id,
        "status": view.status,
        "behavior": view.behavior,
        "output": view.output,
        "native_session_id": view.native_session_id,
        "native_run_id": view.native_run_id,
    }


@dataclass(frozen=True)
class ProductProfile:
    name: str
    instructions: tuple[str, ...]
    model_factory: Callable[[], Any]
    toolset_factory: Callable[[], tuple[AbstractToolset, ...]]
    approval_required_tools: tuple[str, ...] = ()


@dataclass(frozen=True)
class WorkspaceMountSpec:
    id: str
    files: Mapping[str, str]
    mode: WorkspaceMountMode = "read_write"
    default: bool = False
    default_for_shell: bool = False

    @classmethod
    def from_mapping(cls, raw: Mapping[str, Any]) -> WorkspaceMountSpec:
        mount_id = str(raw.get("id") or "")
        if not mount_id:
            raise ValueError("workspace mount id must not be empty")
        raw_mode = str(raw.get("mode") or "read_write")
        if raw_mode not in {"read_write", "read_only"}:
            raise ValueError("workspace mount mode must be read_write or read_only")
        mode: WorkspaceMountMode = "read_only" if raw_mode == "read_only" else "read_write"
        raw_files = raw.get("files") or {}
        if not isinstance(raw_files, Mapping):
            raise TypeError("workspace mount files must be a mapping")
        return cls(
            id=mount_id,
            files={str(path): str(content) for path, content in raw_files.items()},
            mode=mode,
            default=bool(raw.get("default", False)),
            default_for_shell=bool(raw.get("default_for_shell", False)),
        )

    @property
    def root(self) -> str:
        return f"/environment/{self.id}"

    def to_dict(self) -> JsonObject:
        return {
            "id": self.id,
            "root": self.root,
            "mode": self.mode,
            "default": self.default,
            "default_for_shell": self.default_for_shell,
            "files": dict(self.files),
        }


@dataclass(frozen=True)
class SandboxSpec:
    backend: str = "virtual"
    lifecycle: str = "session"
    ttl_seconds: int | None = 3600

    @classmethod
    def from_mapping(cls, raw: Mapping[str, Any]) -> SandboxSpec:
        ttl_raw = raw.get("ttl_seconds", 3600)
        ttl_seconds = None if ttl_raw is None else int(ttl_raw)
        return cls(
            backend=str(raw.get("backend") or "virtual"),
            lifecycle=str(raw.get("lifecycle") or "session"),
            ttl_seconds=ttl_seconds,
        )

    def to_dict(self) -> JsonObject:
        payload: JsonObject = {"backend": self.backend, "lifecycle": self.lifecycle}
        if self.ttl_seconds is not None:
            payload["ttl_seconds"] = self.ttl_seconds
        return payload


@dataclass(frozen=True)
class ProductWorkspaceBinding:
    backend: str
    binding_id: str
    default_cwd: str
    mounts: tuple[WorkspaceMountSpec, ...]
    sandbox: SandboxSpec
    generation: int = 1

    @classmethod
    def default(cls) -> ProductWorkspaceBinding:
        return cls(
            backend="virtual",
            binding_id="claw-product-workspace",
            default_cwd="/environment/workspace",
            mounts=(
                WorkspaceMountSpec(
                    id="workspace",
                    files={
                        "README.md": "Claw product workspace\n",
                        "deploy/config.yaml": "service: api\n",
                    },
                    default=True,
                    default_for_shell=True,
                ),
                WorkspaceMountSpec(
                    id="data",
                    files={"release-notes.md": "staged rollout required\n"},
                    mode="read_only",
                ),
            ),
            sandbox=SandboxSpec(),
        )

    @classmethod
    def from_mapping(cls, raw: Mapping[str, Any] | None) -> ProductWorkspaceBinding:
        if not raw:
            return cls.default()
        mounts_raw = raw.get("mounts")
        if not isinstance(mounts_raw, list) or not mounts_raw:
            raise ValueError("workspace must include at least one mount")
        mounts = tuple(WorkspaceMountSpec.from_mapping(mount) for mount in mounts_raw)
        default_cwd = str(raw.get("default_cwd") or "")
        if not default_cwd:
            raise ValueError("workspace default_cwd must not be empty")
        backend = str(raw.get("backend") or "virtual")
        if backend != "virtual":
            raise ValueError("example product runtime only supports virtual workspaces")
        sandbox_raw = raw.get("sandbox") or {}
        if not isinstance(sandbox_raw, Mapping):
            raise TypeError("workspace sandbox must be a mapping")
        workspace = cls(
            backend=backend,
            binding_id=str(raw.get("binding_id") or "claw-product-workspace"),
            default_cwd=default_cwd,
            mounts=mounts,
            sandbox=SandboxSpec.from_mapping(sandbox_raw),
            generation=int(raw.get("generation") or 1),
        )
        workspace.validate()
        return workspace

    def validate(self) -> None:
        mount_ids = [mount.id for mount in self.mounts]
        if len(mount_ids) != len(set(mount_ids)):
            raise ValueError("workspace mount ids must be unique")
        default_mounts = [mount for mount in self.mounts if mount.default]
        if len(default_mounts) != 1:
            raise ValueError("workspace must have exactly one default mount")
        if not self.binding_id:
            raise ValueError("workspace binding_id must not be empty")
        if not any(
            self.default_cwd == mount.root or self.default_cwd.startswith(f"{mount.root}/")
            for mount in self.mounts
        ):
            raise ValueError("workspace default_cwd must be inside a mounted virtual path")

    def fingerprint(self) -> str:
        payload = {
            "backend": self.backend,
            "binding_id": self.binding_id,
            "default_cwd": self.default_cwd,
            "generation": self.generation,
            "mounts": [mount.to_dict() for mount in self.mounts],
            "sandbox": self.sandbox.to_dict(),
        }
        return hashlib.sha256(_encode(payload).encode("utf-8")).hexdigest()

    def to_dict(self) -> JsonObject:
        self.validate()
        payload: JsonObject = {
            "format": "claw.product.workspace",
            "backend": self.backend,
            "binding_id": self.binding_id,
            "default_cwd": self.default_cwd,
            "generation": self.generation,
            "fingerprint": self.fingerprint(),
            "mounts": [mount.to_dict() for mount in self.mounts],
            "sandbox": self.sandbox.to_dict(),
        }
        return payload


@dataclass(frozen=True)
class WorkspaceRuntime:
    binding: ProductWorkspaceBinding
    environment: EnvironmentProvider
    workspace_snapshot: JsonObject
    sandbox_status: JsonObject


@dataclass
class ActiveProductRun:
    run_id: str
    session_id: str
    agent: Agent
    session: AgentSession
    run: AgentRun
    events: list[StreamEvent]
    workspace_snapshot: JsonObject
    sandbox_status: JsonObject


class NotificationHub:
    def __init__(self, database: ProductDatabase) -> None:
        self._database = database
        self._queue: asyncio.Queue[JsonObject] = asyncio.Queue()

    async def publish(self, topic: str, payload: Mapping[str, Any]) -> JsonObject:
        record = self._database.append_notification(topic, dict(payload))
        await self._queue.put(record)
        return record

    async def next(self) -> JsonObject:
        return await self._queue.get()

    def replay(self, after_sequence: int = 0) -> list[JsonObject]:
        return self._database.notifications_after(after_sequence)


class WorkspaceFactory:
    def normalize(self, workspace: Mapping[str, Any] | None = None) -> ProductWorkspaceBinding:
        return ProductWorkspaceBinding.from_mapping(workspace)

    async def runtime_for(self, workspace: Mapping[str, Any]) -> WorkspaceRuntime:
        binding = ProductWorkspaceBinding.from_mapping(workspace)
        environment = self._environment(binding)
        environment_state = await environment.export_state()
        workspace_snapshot = binding.to_dict()
        workspace_snapshot["environment_state"] = environment_state
        sandbox_status = {
            **binding.sandbox.to_dict(),
            "status": "running",
            "started_at": _now(),
            "workspace_fingerprint": workspace_snapshot["fingerprint"],
        }
        return WorkspaceRuntime(
            binding=binding,
            environment=environment,
            workspace_snapshot=workspace_snapshot,
            sandbox_status=sandbox_status,
        )

    def _environment(self, binding: ProductWorkspaceBinding) -> EnvironmentProvider:
        mounts = [
            VirtualMount(
                mount.id,
                EnvironmentProvider.virtual(id=mount.id, files=mount.files),
                mode=mount.mode,
                default=mount.default,
                default_for_shell=mount.default_for_shell,
            )
            for mount in binding.mounts
        ]
        workspace = WorkspaceBinding(mounts, id=binding.binding_id)
        return workspace.environment()


class ProductDatabase:
    def __init__(self, path: str | Path) -> None:
        self.path = Path(path)
        self._connection = sqlite3.connect(self.path)
        self._connection.row_factory = sqlite3.Row

    def close(self) -> None:
        self._connection.close()

    def migrate(self) -> None:
        self._connection.executescript(
            """
            CREATE TABLE IF NOT EXISTS product_runtime_instances (
                id TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                started_at TEXT NOT NULL,
                stopped_at TEXT
            );

            CREATE TABLE IF NOT EXISTS product_profiles (
                name TEXT PRIMARY KEY,
                instructions_json TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS product_sessions (
                id TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                profile TEXT NOT NULL,
                active_run_id TEXT,
                native_session_id TEXT,
                workspace_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS product_runs (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                status TEXT NOT NULL,
                prompt TEXT NOT NULL,
                behavior TEXT NOT NULL,
                trigger_type TEXT NOT NULL,
                native_session_id TEXT,
                native_run_id TEXT,
                output TEXT,
                last_run_state_json TEXT,
                pending_hitl_json TEXT,
                workspace_snapshot_json TEXT,
                sandbox_status_json TEXT,
                claim_instance_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS product_async_tasks (
                id TEXT PRIMARY KEY,
                parent_session_id TEXT NOT NULL,
                status TEXT NOT NULL,
                prompt TEXT NOT NULL,
                worker_session_id TEXT,
                worker_run_id TEXT,
                transcript_json TEXT NOT NULL,
                result_json TEXT,
                wake_parent_json TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS product_schedules (
                id TEXT PRIMARY KEY,
                prompt TEXT NOT NULL,
                status TEXT NOT NULL,
                fire_count INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS product_schedule_fires (
                id TEXT PRIMARY KEY,
                schedule_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS product_heartbeat_fires (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS product_workflows (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                definition_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS product_workflow_runs (
                id TEXT PRIMARY KEY,
                workflow_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS product_workflow_node_runs (
                id TEXT PRIMARY KEY,
                workflow_run_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                status TEXT NOT NULL,
                output TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS product_memory_entries (
                id TEXT PRIMARY KEY,
                scope TEXT NOT NULL,
                source_run_id TEXT NOT NULL,
                content TEXT NOT NULL,
                summary TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS product_agency_sessions (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS product_agency_fires (
                id TEXT PRIMARY KEY,
                agency_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                status TEXT NOT NULL,
                output TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS product_bridge_conversations (
                id TEXT PRIMARY KEY,
                channel TEXT NOT NULL,
                external_id TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS product_bridge_events (
                id TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS product_bridge_hitl_messages (
                id TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                approval_id TEXT NOT NULL,
                status TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                decision_json TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS product_notifications (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                topic TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            """
        )
        self._ensure_column("product_runs", "workspace_snapshot_json", "TEXT")
        self._ensure_column("product_runs", "sandbox_status_json", "TEXT")
        self._ensure_column("product_async_tasks", "worker_session_id", "TEXT")
        self._ensure_column("product_async_tasks", "worker_run_id", "TEXT")
        self._ensure_column("product_async_tasks", "wake_parent_json", "TEXT")
        self._connection.commit()

    def _ensure_column(self, table: str, column: str, definition: str) -> None:
        try:
            self._connection.execute(f"ALTER TABLE {table} ADD COLUMN {column} {definition}")
        except sqlite3.OperationalError as error:
            if "duplicate column name" not in str(error).lower():
                raise

    def seed_profile(self, profile: ProductProfile) -> None:
        self._connection.execute(
            """
            INSERT INTO product_profiles (name, instructions_json)
            VALUES (?, ?)
            ON CONFLICT(name) DO UPDATE SET instructions_json = excluded.instructions_json
            """,
            (profile.name, _encode(list(profile.instructions))),
        )
        self._connection.commit()

    def register_runtime(self, runtime_id: str) -> None:
        self._connection.execute(
            """
            INSERT INTO product_runtime_instances (id, status, started_at)
            VALUES (?, 'ready', ?)
            ON CONFLICT(id) DO UPDATE SET status = 'ready', started_at = excluded.started_at
            """,
            (runtime_id, _now()),
        )
        self._connection.commit()

    def stop_runtime(self, runtime_id: str) -> None:
        self._connection.execute(
            """
            UPDATE product_runtime_instances
            SET status = 'stopped', stopped_at = ?
            WHERE id = ?
            """,
            (_now(), runtime_id),
        )
        self._connection.commit()

    def recover_orphan_running(self) -> int:
        cursor = self._connection.execute(
            """
            UPDATE product_runs
            SET status = 'queued',
                behavior = 'recovered',
                claim_instance_id = NULL,
                updated_at = ?
            WHERE status = 'running'
            """,
            (_now(),),
        )
        self._connection.execute(
            """
            UPDATE product_sessions
            SET status = 'idle', active_run_id = NULL, updated_at = ?
            WHERE active_run_id IN (
                SELECT id FROM product_runs WHERE status = 'queued' AND behavior = 'recovered'
            )
            """,
            (_now(),),
        )
        self._connection.commit()
        return cursor.rowcount

    def create_session(self, *, profile: str, workspace: Mapping[str, Any]) -> str:
        session_id = _new_id("product_session")
        self._connection.execute(
            """
            INSERT INTO product_sessions (
                id, status, profile, active_run_id, workspace_json, created_at, updated_at
            )
            VALUES (?, 'idle', ?, NULL, ?, ?, ?)
            """,
            (session_id, profile, _encode(dict(workspace)), _now(), _now()),
        )
        self._connection.commit()
        return session_id

    def active_run(self, session_id: str) -> sqlite3.Row | None:
        return self._connection.execute(
            """
            SELECT * FROM product_runs
            WHERE session_id = ? AND status IN ('queued', 'running', 'hitl')
            ORDER BY created_at DESC
            LIMIT 1
            """,
            (session_id,),
        ).fetchone()

    def create_run(
        self,
        session_id: str,
        prompt: str,
        *,
        trigger_type: str = "interactive",
    ) -> ProductRunView:
        session = self.session(session_id)
        run_id = _new_id("product_run")
        self._connection.execute(
            """
            INSERT INTO product_runs (
                id,
                session_id,
                status,
                prompt,
                behavior,
                trigger_type,
                workspace_snapshot_json,
                sandbox_status_json,
                created_at,
                updated_at
            )
            VALUES (?, ?, 'queued', ?, 'created', ?, ?, ?, ?, ?)
            """,
            (
                run_id,
                session_id,
                prompt,
                trigger_type,
                session["workspace_json"],
                _encode({"status": "queued"}),
                _now(),
                _now(),
            ),
        )
        self._connection.execute(
            """
            UPDATE product_sessions
            SET status = 'queued', active_run_id = ?, updated_at = ?
            WHERE id = ?
            """,
            (run_id, _now(), session_id),
        )
        self._connection.commit()
        return ProductRunView(run_id, session_id, "queued", "created")

    def merge_queued_input(self, run: sqlite3.Row, prompt: str) -> ProductRunView:
        merged = f"{run['prompt']}\n{prompt}"
        self._connection.execute(
            """
            UPDATE product_runs
            SET prompt = ?, behavior = 'merged', updated_at = ?
            WHERE id = ?
            """,
            (merged, _now(), run["id"]),
        )
        self._connection.commit()
        return ProductRunView(run["id"], run["session_id"], "queued", "merged")

    def claim_next_queued(self, runtime_id: str) -> sqlite3.Row | None:
        run = self._connection.execute(
            """
            SELECT * FROM product_runs
            WHERE status = 'queued'
            ORDER BY created_at ASC
            LIMIT 1
            """
        ).fetchone()
        if run is None:
            return None
        self._connection.execute(
            """
            UPDATE product_runs
            SET status = 'running',
                behavior = CASE WHEN behavior = 'recovered' THEN behavior ELSE 'claimed' END,
                claim_instance_id = ?,
                updated_at = ?
            WHERE id = ?
            """,
            (runtime_id, _now(), run["id"]),
        )
        self._connection.execute(
            """
            UPDATE product_sessions
            SET status = 'running', active_run_id = ?, updated_at = ?
            WHERE id = ?
            """,
            (run["id"], _now(), run["session_id"]),
        )
        self._connection.commit()
        return self.run(run["id"])

    def run(self, run_id: str) -> sqlite3.Row:
        row = self._connection.execute(
            "SELECT * FROM product_runs WHERE id = ?",
            (run_id,),
        ).fetchone()
        if row is None:
            raise KeyError(f"unknown product run: {run_id}")
        return row

    def session(self, session_id: str) -> sqlite3.Row:
        row = self._connection.execute(
            "SELECT * FROM product_sessions WHERE id = ?",
            (session_id,),
        ).fetchone()
        if row is None:
            raise KeyError(f"unknown product session: {session_id}")
        return row

    def update_run_status(
        self,
        run_id: str,
        status: str,
        *,
        behavior: str | None = None,
        native_session_id: str | None = None,
        native_run_id: str | None = None,
        output: str | None = None,
        last_run_state: Mapping[str, Any] | None = None,
        pending_hitl: Mapping[str, Any] | None = None,
        workspace_snapshot: Mapping[str, Any] | None = None,
        sandbox_status: Mapping[str, Any] | None = None,
    ) -> ProductRunView:
        run = self.run(run_id)
        next_behavior = behavior or run["behavior"]
        self._connection.execute(
            """
            UPDATE product_runs
            SET status = ?,
                behavior = ?,
                native_session_id = COALESCE(?, native_session_id),
                native_run_id = COALESCE(?, native_run_id),
                output = COALESCE(?, output),
                last_run_state_json = COALESCE(?, last_run_state_json),
                pending_hitl_json = COALESCE(?, pending_hitl_json),
                workspace_snapshot_json = COALESCE(?, workspace_snapshot_json),
                sandbox_status_json = COALESCE(?, sandbox_status_json),
                claim_instance_id = CASE
                    WHEN ? IN ('completed', 'failed', 'hitl') THEN NULL
                    ELSE claim_instance_id
                END,
                updated_at = ?
            WHERE id = ?
            """,
            (
                status,
                next_behavior,
                native_session_id,
                native_run_id,
                output,
                _encode(dict(last_run_state)) if last_run_state is not None else None,
                _encode(dict(pending_hitl)) if pending_hitl is not None else None,
                _encode(dict(workspace_snapshot)) if workspace_snapshot is not None else None,
                _encode(dict(sandbox_status)) if sandbox_status is not None else None,
                status,
                _now(),
                run_id,
            ),
        )
        session_status = "idle" if status == "completed" else status
        self._connection.execute(
            """
            UPDATE product_sessions
            SET status = ?,
                active_run_id = CASE WHEN ? = 'completed' THEN NULL ELSE ? END,
                native_session_id = COALESCE(?, native_session_id),
                updated_at = ?
            WHERE id = ?
            """,
            (
                session_status,
                status,
                run_id,
                native_session_id,
                _now(),
                run["session_id"],
            ),
        )
        self._connection.commit()
        updated = self.run(run_id)
        return ProductRunView(
            updated["id"],
            updated["session_id"],
            updated["status"],
            updated["behavior"],
            output=updated["output"],
            native_session_id=updated["native_session_id"],
            native_run_id=updated["native_run_id"],
        )

    def run_details(self, run_id: str) -> JsonObject:
        run = self.run(run_id)
        session = self.session(str(run["session_id"]))
        return {
            "run_id": run["id"],
            "session_id": run["session_id"],
            "status": run["status"],
            "behavior": run["behavior"],
            "workspace_snapshot": _decode(run["workspace_snapshot_json"], {}),
            "sandbox_status": _decode(run["sandbox_status_json"], {}),
            "session_workspace": _decode(session["workspace_json"], {}),
            "native_session_id": run["native_session_id"],
            "native_run_id": run["native_run_id"],
        }

    def cleanup_expired_sandboxes(
        self,
        *,
        ttl_seconds: int = 3600,
        now: str | None = None,
    ) -> JsonObject:
        if ttl_seconds < 0:
            raise ValueError("ttl_seconds must be non-negative")
        current = now or _now()
        current_time = _parse_time(current)
        cleaned: list[JsonObject] = []
        rows = self._connection.execute(
            """
            SELECT id, sandbox_status_json
            FROM product_runs
            WHERE sandbox_status_json IS NOT NULL
            ORDER BY updated_at ASC
            """
        ).fetchall()
        for row in rows:
            sandbox_status = _decode(row["sandbox_status_json"], {})
            if not isinstance(sandbox_status, Mapping):
                continue
            status = str(sandbox_status.get("status") or "")
            if status not in {"stopped", "failed", "interrupted"}:
                continue
            stopped_at = sandbox_status.get("stopped_at")
            stopped_time = _parse_time(stopped_at)
            if ttl_seconds > 0:
                if stopped_time is None or current_time is None:
                    continue
                if stopped_time + timedelta(seconds=ttl_seconds) > current_time:
                    continue
            updated = dict(sandbox_status)
            updated["status"] = "cleaned"
            updated["previous_status"] = status
            updated["cleaned_at"] = current
            updated["cleanup_reason"] = "ttl_expired"
            updated["ttl_seconds"] = ttl_seconds
            self._connection.execute(
                """
                UPDATE product_runs
                SET sandbox_status_json = ?, updated_at = ?
                WHERE id = ?
                """,
                (_encode(updated), current, row["id"]),
            )
            cleaned.append(
                {
                    "run_id": row["id"],
                    "previous_status": status,
                    "sandbox_status": updated,
                }
            )
        self._connection.commit()
        return {
            "ttl_seconds": ttl_seconds,
            "now": current,
            "cleaned": len(cleaned),
            "runs": cleaned,
        }

    def session_details(self, session_id: str) -> JsonObject:
        session = self.session(session_id)
        return {
            "session_id": session["id"],
            "status": session["status"],
            "profile": session["profile"],
            "active_run_id": session["active_run_id"],
            "native_session_id": session["native_session_id"],
            "workspace": _decode(session["workspace_json"], {}),
        }

    def runs_for_session(self, session_id: str) -> list[JsonObject]:
        rows = self._connection.execute(
            """
            SELECT *
            FROM product_runs
            WHERE session_id = ?
            ORDER BY created_at ASC
            """,
            (session_id,),
        ).fetchall()
        return [
            {
                "run_id": row["id"],
                "session_id": row["session_id"],
                "status": row["status"],
                "behavior": row["behavior"],
                "trigger_type": row["trigger_type"],
                "native_session_id": row["native_session_id"],
                "native_run_id": row["native_run_id"],
                "output": row["output"],
            }
            for row in rows
        ]

    def append_notification(self, topic: str, payload: Mapping[str, Any]) -> JsonObject:
        cursor = self._connection.execute(
            """
            INSERT INTO product_notifications (topic, payload_json, created_at)
            VALUES (?, ?, ?)
            """,
            (topic, _encode(dict(payload)), _now()),
        )
        self._connection.commit()
        sequence = cursor.lastrowid
        if sequence is None:
            raise RuntimeError("notification insert did not return a sequence")
        return {
            "sequence": int(sequence),
            "topic": topic,
            "payload": dict(payload),
            "created_at": _now(),
        }

    def notifications_after(self, after_sequence: int = 0) -> list[JsonObject]:
        rows = self._connection.execute(
            """
            SELECT * FROM product_notifications
            WHERE sequence > ?
            ORDER BY sequence ASC
            """,
            (after_sequence,),
        ).fetchall()
        return [
            {
                "sequence": int(row["sequence"]),
                "topic": row["topic"],
                "payload": _decode(row["payload_json"], {}),
                "created_at": row["created_at"],
            }
            for row in rows
        ]

    def counts(self) -> JsonObject:
        return {
            "product_sessions": int(
                self._connection.execute("SELECT COUNT(*) FROM product_sessions").fetchone()[0]
            ),
            "product_runs": int(
                self._connection.execute("SELECT COUNT(*) FROM product_runs").fetchone()[0]
            ),
            "product_profiles": int(
                self._connection.execute("SELECT COUNT(*) FROM product_profiles").fetchone()[0]
            ),
            "product_notifications": int(
                self._connection.execute("SELECT COUNT(*) FROM product_notifications").fetchone()[0]
            ),
            "product_runtime_instances": int(
                self._connection.execute(
                    "SELECT COUNT(*) FROM product_runtime_instances"
                ).fetchone()[0]
            ),
            "product_async_tasks": int(
                self._connection.execute("SELECT COUNT(*) FROM product_async_tasks").fetchone()[0]
            ),
            "product_schedules": int(
                self._connection.execute("SELECT COUNT(*) FROM product_schedules").fetchone()[0]
            ),
            "product_schedule_fires": int(
                self._connection.execute("SELECT COUNT(*) FROM product_schedule_fires").fetchone()[
                    0
                ]
            ),
            "product_heartbeat_fires": int(
                self._connection.execute("SELECT COUNT(*) FROM product_heartbeat_fires").fetchone()[
                    0
                ]
            ),
            "product_workflows": int(
                self._connection.execute("SELECT COUNT(*) FROM product_workflows").fetchone()[0]
            ),
            "product_workflow_runs": int(
                self._connection.execute("SELECT COUNT(*) FROM product_workflow_runs").fetchone()[0]
            ),
            "product_workflow_node_runs": int(
                self._connection.execute(
                    "SELECT COUNT(*) FROM product_workflow_node_runs"
                ).fetchone()[0]
            ),
            "product_memory_entries": int(
                self._connection.execute("SELECT COUNT(*) FROM product_memory_entries").fetchone()[
                    0
                ]
            ),
            "product_agency_sessions": int(
                self._connection.execute("SELECT COUNT(*) FROM product_agency_sessions").fetchone()[
                    0
                ]
            ),
            "product_agency_fires": int(
                self._connection.execute("SELECT COUNT(*) FROM product_agency_fires").fetchone()[0]
            ),
            "product_bridge_conversations": int(
                self._connection.execute(
                    "SELECT COUNT(*) FROM product_bridge_conversations"
                ).fetchone()[0]
            ),
            "product_bridge_events": int(
                self._connection.execute("SELECT COUNT(*) FROM product_bridge_events").fetchone()[0]
            ),
            "product_bridge_hitl_messages": int(
                self._connection.execute(
                    "SELECT COUNT(*) FROM product_bridge_hitl_messages"
                ).fetchone()[0]
            ),
        }

    def create_async_task(
        self,
        *,
        task_id: str | None,
        parent_session_id: str,
        prompt: str,
    ) -> JsonObject:
        task_id = task_id or _new_id("async_task")
        transcript = [{"at": _now(), "kind": "spawned", "content": prompt}]
        self._connection.execute(
            """
            INSERT INTO product_async_tasks (
                id,
                parent_session_id,
                status,
                prompt,
                transcript_json,
                created_at,
                updated_at
            )
            VALUES (?, ?, 'running', ?, ?, ?, ?)
            """,
            (task_id, parent_session_id, prompt, _encode(transcript), _now(), _now()),
        )
        self._connection.commit()
        return self.async_task(task_id)

    def async_task(self, task_id: str) -> JsonObject:
        row = self._connection.execute(
            "SELECT * FROM product_async_tasks WHERE id = ?",
            (task_id,),
        ).fetchone()
        if row is None:
            raise KeyError(f"unknown async task: {task_id}")
        return {
            "task_id": row["id"],
            "parent_session_id": row["parent_session_id"],
            "status": row["status"],
            "prompt": row["prompt"],
            "worker_session_id": row["worker_session_id"],
            "worker_run_id": row["worker_run_id"],
            "transcript": _decode(row["transcript_json"], []),
            "result": _decode(row["result_json"], None),
            "wake_parent": _decode(row["wake_parent_json"], None),
            "created_at": row["created_at"],
            "updated_at": row["updated_at"],
        }

    def attach_async_task_run(
        self,
        *,
        task_id: str,
        worker_session_id: str,
        worker_run_id: str,
    ) -> JsonObject:
        task = self.async_task(task_id)
        transcript = list(task["transcript"])
        transcript.append(
            {
                "at": _now(),
                "kind": "worker_started",
                "run_id": worker_run_id,
            }
        )
        self._connection.execute(
            """
            UPDATE product_async_tasks
            SET worker_session_id = ?,
                worker_run_id = ?,
                transcript_json = ?,
                updated_at = ?
            WHERE id = ?
            """,
            (worker_session_id, worker_run_id, _encode(transcript), _now(), task_id),
        )
        self._connection.commit()
        return self.async_task(task_id)

    def complete_async_task(
        self,
        *,
        task_id: str,
        status: str,
        output: str | None,
    ) -> JsonObject:
        task = self.async_task(task_id)
        transcript = list(task["transcript"])
        transcript.append(
            {
                "at": _now(),
                "kind": "completed",
                "status": status,
                "output": output,
            }
        )
        result = {
            "output": output,
            "status": status,
            "wake_parent": True,
            "worker_run_id": task["worker_run_id"],
        }
        wake_parent = {
            "parent_session_id": task["parent_session_id"],
            "task_id": task_id,
            "worker_run_id": task["worker_run_id"],
            "output": output,
        }
        self._connection.execute(
            """
            UPDATE product_async_tasks
            SET status = ?,
                transcript_json = ?,
                result_json = ?,
                wake_parent_json = ?,
                updated_at = ?
            WHERE id = ?
            """,
            (
                status,
                _encode(transcript),
                _encode(result),
                _encode(wake_parent),
                _now(),
                task_id,
            ),
        )
        self._connection.commit()
        return self.async_task(task_id)

    def steer_async_task(self, task_id: str, message: str) -> JsonObject:
        task = self.async_task(task_id)
        if task["status"] != "running":
            raise RuntimeError(f"async task is not running: {task['status']}")
        transcript = list(task["transcript"])
        transcript.append({"at": _now(), "kind": "steering", "content": message})
        result = {"last_instruction": message, "wake_parent": True}
        self._connection.execute(
            """
            UPDATE product_async_tasks
            SET transcript_json = ?, result_json = ?, updated_at = ?
            WHERE id = ?
            """,
            (_encode(transcript), _encode(result), _now(), task_id),
        )
        self._connection.commit()
        return self.async_task(task_id)

    def cancel_async_task(self, task_id: str, reason: str) -> JsonObject:
        task = self.async_task(task_id)
        transcript = list(task["transcript"])
        transcript.append({"at": _now(), "kind": "cancelled", "content": reason})
        result = {"cancelled": True, "reason": reason, "wake_parent": True}
        self._connection.execute(
            """
            UPDATE product_async_tasks
            SET status = 'cancelled',
                transcript_json = ?,
                result_json = ?,
                updated_at = ?
            WHERE id = ?
            """,
            (_encode(transcript), _encode(result), _now(), task_id),
        )
        self._connection.commit()
        return self.async_task(task_id)

    def upsert_schedule(self, schedule_id: str, prompt: str) -> JsonObject:
        self._connection.execute(
            """
            INSERT INTO product_schedules (id, prompt, status, fire_count, created_at, updated_at)
            VALUES (?, ?, 'active', 0, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                prompt = excluded.prompt,
                status = 'active',
                updated_at = excluded.updated_at
            """,
            (schedule_id, prompt, _now(), _now()),
        )
        self._connection.commit()
        return self.schedule(schedule_id)

    def schedule(self, schedule_id: str) -> JsonObject:
        row = self._connection.execute(
            "SELECT * FROM product_schedules WHERE id = ?",
            (schedule_id,),
        ).fetchone()
        if row is None:
            raise KeyError(f"unknown schedule: {schedule_id}")
        return {
            "schedule_id": row["id"],
            "prompt": row["prompt"],
            "status": row["status"],
            "fire_count": int(row["fire_count"]),
            "created_at": row["created_at"],
            "updated_at": row["updated_at"],
        }

    def active_schedules(self) -> list[JsonObject]:
        rows = self._connection.execute(
            """
            SELECT *
            FROM product_schedules
            WHERE status = 'active'
            ORDER BY created_at ASC, id ASC
            """
        ).fetchall()
        return [
            {
                "schedule_id": row["id"],
                "prompt": row["prompt"],
                "status": row["status"],
                "fire_count": int(row["fire_count"]),
                "created_at": row["created_at"],
                "updated_at": row["updated_at"],
            }
            for row in rows
        ]

    def create_schedule_fire(
        self,
        *,
        schedule_id: str,
        session_id: str,
        run_id: str,
    ) -> JsonObject:
        fire_id = _new_id("schedule_fire")
        self._connection.execute(
            """
            INSERT INTO product_schedule_fires (
                id, schedule_id, session_id, run_id, status, created_at, updated_at
            )
            VALUES (?, ?, ?, ?, 'running', ?, ?)
            """,
            (fire_id, schedule_id, session_id, run_id, _now(), _now()),
        )
        self._connection.execute(
            """
            UPDATE product_schedules
            SET fire_count = fire_count + 1, updated_at = ?
            WHERE id = ?
            """,
            (_now(), schedule_id),
        )
        self._connection.commit()
        return self.schedule_fire(fire_id)

    def schedule_fire(self, fire_id: str) -> JsonObject:
        row = self._connection.execute(
            "SELECT * FROM product_schedule_fires WHERE id = ?",
            (fire_id,),
        ).fetchone()
        if row is None:
            raise KeyError(f"unknown schedule fire: {fire_id}")
        return {
            "fire_id": row["id"],
            "schedule_id": row["schedule_id"],
            "session_id": row["session_id"],
            "run_id": row["run_id"],
            "status": row["status"],
            "created_at": row["created_at"],
            "updated_at": row["updated_at"],
        }

    def complete_schedule_fire(self, fire_id: str, status: str) -> JsonObject:
        self._connection.execute(
            """
            UPDATE product_schedule_fires
            SET status = ?, updated_at = ?
            WHERE id = ?
            """,
            (status, _now(), fire_id),
        )
        self._connection.commit()
        return self.schedule_fire(fire_id)

    def create_heartbeat_fire(self, *, session_id: str, run_id: str) -> JsonObject:
        fire_id = _new_id("heartbeat_fire")
        self._connection.execute(
            """
            INSERT INTO product_heartbeat_fires (
                id, session_id, run_id, status, created_at, updated_at
            )
            VALUES (?, ?, ?, 'running', ?, ?)
            """,
            (fire_id, session_id, run_id, _now(), _now()),
        )
        self._connection.commit()
        return self.heartbeat_fire(fire_id)

    def heartbeat_fire(self, fire_id: str) -> JsonObject:
        row = self._connection.execute(
            "SELECT * FROM product_heartbeat_fires WHERE id = ?",
            (fire_id,),
        ).fetchone()
        if row is None:
            raise KeyError(f"unknown heartbeat fire: {fire_id}")
        return {
            "fire_id": row["id"],
            "session_id": row["session_id"],
            "run_id": row["run_id"],
            "status": row["status"],
            "created_at": row["created_at"],
            "updated_at": row["updated_at"],
        }

    def complete_heartbeat_fire(self, fire_id: str, status: str) -> JsonObject:
        self._connection.execute(
            """
            UPDATE product_heartbeat_fires
            SET status = ?, updated_at = ?
            WHERE id = ?
            """,
            (status, _now(), fire_id),
        )
        self._connection.commit()
        return self.heartbeat_fire(fire_id)

    def upsert_workflow(
        self,
        *,
        workflow_id: str,
        name: str,
        nodes: Sequence[Mapping[str, Any]],
    ) -> JsonObject:
        self._connection.execute(
            """
            INSERT INTO product_workflows (id, name, definition_json, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                definition_json = excluded.definition_json,
                updated_at = excluded.updated_at
            """,
            (workflow_id, name, _encode({"nodes": list(nodes)}), _now(), _now()),
        )
        self._connection.commit()
        return self.workflow(workflow_id)

    def workflow(self, workflow_id: str) -> JsonObject:
        row = self._connection.execute(
            "SELECT * FROM product_workflows WHERE id = ?",
            (workflow_id,),
        ).fetchone()
        if row is None:
            raise KeyError(f"unknown workflow: {workflow_id}")
        return {
            "workflow_id": row["id"],
            "name": row["name"],
            "definition": _decode(row["definition_json"], {}),
            "created_at": row["created_at"],
            "updated_at": row["updated_at"],
        }

    def create_workflow_run(self, *, workflow_id: str, session_id: str) -> JsonObject:
        workflow_run_id = _new_id("workflow_run")
        self._connection.execute(
            """
            INSERT INTO product_workflow_runs (
                id, workflow_id, session_id, status, created_at, updated_at
            )
            VALUES (?, ?, ?, 'running', ?, ?)
            """,
            (workflow_run_id, workflow_id, session_id, _now(), _now()),
        )
        self._connection.commit()
        return self.workflow_run(workflow_run_id)

    def workflow_run(self, workflow_run_id: str) -> JsonObject:
        row = self._connection.execute(
            "SELECT * FROM product_workflow_runs WHERE id = ?",
            (workflow_run_id,),
        ).fetchone()
        if row is None:
            raise KeyError(f"unknown workflow run: {workflow_run_id}")
        node_rows = self._connection.execute(
            """
            SELECT *
            FROM product_workflow_node_runs
            WHERE workflow_run_id = ?
            ORDER BY created_at ASC, rowid ASC
            """,
            (workflow_run_id,),
        ).fetchall()
        return {
            "workflow_run_id": row["id"],
            "workflow_id": row["workflow_id"],
            "session_id": row["session_id"],
            "status": row["status"],
            "nodes": [
                {
                    "node_run_id": node["id"],
                    "node_id": node["node_id"],
                    "run_id": node["run_id"],
                    "status": node["status"],
                    "output": node["output"],
                }
                for node in node_rows
            ],
            "created_at": row["created_at"],
            "updated_at": row["updated_at"],
        }

    def create_workflow_node_run(
        self,
        *,
        workflow_run_id: str,
        node_id: str,
        run_id: str,
    ) -> JsonObject:
        node_run_id = _new_id("workflow_node")
        self._connection.execute(
            """
            INSERT INTO product_workflow_node_runs (
                id, workflow_run_id, node_id, run_id, status, created_at, updated_at
            )
            VALUES (?, ?, ?, ?, 'running', ?, ?)
            """,
            (node_run_id, workflow_run_id, node_id, run_id, _now(), _now()),
        )
        self._connection.commit()
        return self.workflow_run(workflow_run_id)

    def complete_workflow_node_run(
        self,
        *,
        node_run_id: str,
        status: str,
        output: str | None,
    ) -> None:
        self._connection.execute(
            """
            UPDATE product_workflow_node_runs
            SET status = ?, output = ?, updated_at = ?
            WHERE id = ?
            """,
            (status, output, _now(), node_run_id),
        )
        self._connection.commit()

    def complete_workflow_run(self, workflow_run_id: str, status: str) -> JsonObject:
        self._connection.execute(
            """
            UPDATE product_workflow_runs
            SET status = ?, updated_at = ?
            WHERE id = ?
            """,
            (status, _now(), workflow_run_id),
        )
        self._connection.commit()
        return self.workflow_run(workflow_run_id)

    def create_memory_entry(
        self,
        *,
        scope: str,
        source_run_id: str,
        content: str,
        summary: str,
    ) -> JsonObject:
        entry_id = _new_id("memory")
        self._connection.execute(
            """
            INSERT INTO product_memory_entries (
                id, scope, source_run_id, content, summary, created_at, updated_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?)
            """,
            (entry_id, scope, source_run_id, content, summary, _now(), _now()),
        )
        self._connection.commit()
        return self.memory_entry(entry_id)

    def memory_entry(self, entry_id: str) -> JsonObject:
        row = self._connection.execute(
            "SELECT * FROM product_memory_entries WHERE id = ?",
            (entry_id,),
        ).fetchone()
        if row is None:
            raise KeyError(f"unknown memory entry: {entry_id}")
        return {
            "entry_id": row["id"],
            "scope": row["scope"],
            "source_run_id": row["source_run_id"],
            "content": row["content"],
            "summary": row["summary"],
            "created_at": row["created_at"],
            "updated_at": row["updated_at"],
        }

    def create_agency_session(self, agency_id: str, session_id: str) -> JsonObject:
        self._connection.execute(
            """
            INSERT INTO product_agency_sessions (
                id, session_id, status, created_at, updated_at
            )
            VALUES (?, ?, 'active', ?, ?)
            """,
            (agency_id, session_id, _now(), _now()),
        )
        self._connection.commit()
        return self.agency_session(agency_id)

    def agency_session(self, agency_id: str) -> JsonObject:
        row = self._connection.execute(
            "SELECT * FROM product_agency_sessions WHERE id = ?",
            (agency_id,),
        ).fetchone()
        if row is None:
            raise KeyError(f"unknown agency session: {agency_id}")
        return {
            "agency_id": row["id"],
            "session_id": row["session_id"],
            "status": row["status"],
            "created_at": row["created_at"],
            "updated_at": row["updated_at"],
        }

    def create_agency_fire(
        self,
        *,
        agency_id: str,
        session_id: str,
        run_id: str,
    ) -> JsonObject:
        fire_id = _new_id("agency_fire")
        self._connection.execute(
            """
            INSERT INTO product_agency_fires (
                id, agency_id, session_id, run_id, status, created_at, updated_at
            )
            VALUES (?, ?, ?, ?, 'running', ?, ?)
            """,
            (fire_id, agency_id, session_id, run_id, _now(), _now()),
        )
        self._connection.commit()
        return self.agency_fire(fire_id)

    def agency_fire(self, fire_id: str) -> JsonObject:
        row = self._connection.execute(
            "SELECT * FROM product_agency_fires WHERE id = ?",
            (fire_id,),
        ).fetchone()
        if row is None:
            raise KeyError(f"unknown agency fire: {fire_id}")
        return {
            "fire_id": row["id"],
            "agency_id": row["agency_id"],
            "session_id": row["session_id"],
            "run_id": row["run_id"],
            "status": row["status"],
            "output": row["output"],
            "created_at": row["created_at"],
            "updated_at": row["updated_at"],
        }

    def complete_agency_fire(
        self,
        *,
        fire_id: str,
        status: str,
        output: str | None,
    ) -> JsonObject:
        self._connection.execute(
            """
            UPDATE product_agency_fires
            SET status = ?, output = ?, updated_at = ?
            WHERE id = ?
            """,
            (status, output, _now(), fire_id),
        )
        self._connection.commit()
        return self.agency_fire(fire_id)

    def upsert_bridge_conversation(
        self,
        *,
        channel: str,
        external_id: str,
    ) -> JsonObject:
        conversation_id = f"bridge_{channel}_{external_id}"
        self._connection.execute(
            """
            INSERT INTO product_bridge_conversations (
                id, channel, external_id, status, created_at, updated_at
            )
            VALUES (?, ?, ?, 'active', ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                status = 'active',
                updated_at = excluded.updated_at
            """,
            (conversation_id, channel, external_id, _now(), _now()),
        )
        self._connection.commit()
        return self.bridge_conversation(conversation_id)

    def bridge_conversation(self, conversation_id: str) -> JsonObject:
        row = self._connection.execute(
            "SELECT * FROM product_bridge_conversations WHERE id = ?",
            (conversation_id,),
        ).fetchone()
        if row is None:
            raise KeyError(f"unknown bridge conversation: {conversation_id}")
        return {
            "conversation_id": row["id"],
            "channel": row["channel"],
            "external_id": row["external_id"],
            "status": row["status"],
            "created_at": row["created_at"],
            "updated_at": row["updated_at"],
        }

    def append_bridge_event(
        self,
        *,
        conversation_id: str,
        kind: str,
        payload: Mapping[str, Any],
    ) -> JsonObject:
        event_id = _new_id("bridge_event")
        self._connection.execute(
            """
            INSERT INTO product_bridge_events (
                id, conversation_id, kind, payload_json, created_at
            )
            VALUES (?, ?, ?, ?, ?)
            """,
            (event_id, conversation_id, kind, _encode(dict(payload)), _now()),
        )
        self._connection.commit()
        return {
            "event_id": event_id,
            "conversation_id": conversation_id,
            "kind": kind,
            "payload": dict(payload),
            "created_at": _now(),
        }

    def create_bridge_hitl_message(
        self,
        *,
        conversation_id: str,
        run_id: str,
        approval_id: str,
        payload: Mapping[str, Any],
    ) -> JsonObject:
        message_id = _new_id("bridge_hitl")
        self._connection.execute(
            """
            INSERT INTO product_bridge_hitl_messages (
                id, conversation_id, run_id, approval_id, status, payload_json, created_at, updated_at
            )
            VALUES (?, ?, ?, ?, 'waiting', ?, ?, ?)
            """,
            (
                message_id,
                conversation_id,
                run_id,
                approval_id,
                _encode(dict(payload)),
                _now(),
                _now(),
            ),
        )
        self._connection.commit()
        return self.bridge_hitl_message(message_id)

    def bridge_hitl_message(self, message_id: str) -> JsonObject:
        row = self._connection.execute(
            "SELECT * FROM product_bridge_hitl_messages WHERE id = ?",
            (message_id,),
        ).fetchone()
        if row is None:
            raise KeyError(f"unknown bridge HITL message: {message_id}")
        return {
            "message_id": row["id"],
            "conversation_id": row["conversation_id"],
            "run_id": row["run_id"],
            "approval_id": row["approval_id"],
            "status": row["status"],
            "payload": _decode(row["payload_json"], {}),
            "decision": _decode(row["decision_json"], None),
            "created_at": row["created_at"],
            "updated_at": row["updated_at"],
        }

    def complete_bridge_hitl_message(
        self,
        *,
        message_id: str,
        decision: Mapping[str, Any],
    ) -> JsonObject:
        self._connection.execute(
            """
            UPDATE product_bridge_hitl_messages
            SET status = 'completed',
                decision_json = ?,
                updated_at = ?
            WHERE id = ?
            """,
            (_encode(dict(decision)), _now(), message_id),
        )
        self._connection.commit()
        return self.bridge_hitl_message(message_id)


class ProductController:
    def __init__(self, database: ProductDatabase) -> None:
        self._database = database
        self._trace_reader: Callable[[str], Awaitable[JsonObject]] | None = None
        self.operator_waiting = asyncio.Event()
        self.operator_release = asyncio.Event()
        self.deployments: list[str] = []

    def set_trace_reader(self, reader: Callable[[str], Awaitable[JsonObject]]) -> None:
        self._trace_reader = reader

    async def wait_for_operator(self) -> dict[str, bool]:
        self.operator_waiting.set()
        await self.operator_release.wait()
        return {"released": True}

    async def deploy_service(self, service: str) -> dict[str, str]:
        self.deployments.append(service)
        return {"service": service, "status": "deployed"}

    async def spawn_async_task(
        self,
        *,
        task_id: str | None,
        parent_session_id: str,
        prompt: str,
    ) -> JsonObject:
        return self._database.create_async_task(
            task_id=task_id,
            parent_session_id=parent_session_id,
            prompt=prompt,
        )

    async def inspect_async_task(self, task_id: str) -> JsonObject:
        return self._database.async_task(task_id)

    async def steer_async_task(self, task_id: str, message: str) -> JsonObject:
        return self._database.steer_async_task(task_id, message)

    async def cancel_async_task(self, task_id: str, reason: str) -> JsonObject:
        return self._database.cancel_async_task(task_id, reason)

    async def inspect_session(self, session_id: str) -> JsonObject:
        return self._database.session_details(session_id)

    async def list_session_runs(self, session_id: str) -> list[JsonObject]:
        return self._database.runs_for_session(session_id)

    async def inspect_run_trace(self, run_id: str) -> JsonObject:
        if self._trace_reader is None:
            raise RuntimeError("trace reader is not configured")
        return await self._trace_reader(run_id)

    async def inspect_schedule_fire(self, fire_id: str) -> JsonObject:
        return self._database.schedule_fire(fire_id)

    async def inspect_heartbeat_fire(self, fire_id: str) -> JsonObject:
        return self._database.heartbeat_fire(fire_id)

    async def inspect_workflow_run(self, workflow_run_id: str) -> JsonObject:
        return self._database.workflow_run(workflow_run_id)

    async def inspect_memory_entry(self, entry_id: str) -> JsonObject:
        return self._database.memory_entry(entry_id)

    async def inspect_agency_fire(self, fire_id: str) -> JsonObject:
        return self._database.agency_fire(fire_id)


class ProductSelfClient:
    def __init__(self, controller: ProductController) -> None:
        self._controller = controller

    async def wait_for_operator(self) -> dict[str, bool]:
        return await self._controller.wait_for_operator()

    async def deploy_service(self, service: str) -> dict[str, str]:
        return await self._controller.deploy_service(service)

    async def spawn_async_task(
        self,
        *,
        task_id: str | None,
        parent_session_id: str,
        prompt: str,
    ) -> JsonObject:
        return await self._controller.spawn_async_task(
            task_id=task_id,
            parent_session_id=parent_session_id,
            prompt=prompt,
        )

    async def inspect_async_task(self, task_id: str) -> JsonObject:
        return await self._controller.inspect_async_task(task_id)

    async def steer_async_task(self, task_id: str, message: str) -> JsonObject:
        return await self._controller.steer_async_task(task_id, message)

    async def cancel_async_task(self, task_id: str, reason: str) -> JsonObject:
        return await self._controller.cancel_async_task(task_id, reason)

    async def inspect_session(self, session_id: str) -> JsonObject:
        return await self._controller.inspect_session(session_id)

    async def list_session_runs(self, session_id: str) -> list[JsonObject]:
        return await self._controller.list_session_runs(session_id)

    async def inspect_run_trace(self, run_id: str) -> JsonObject:
        return await self._controller.inspect_run_trace(run_id)

    async def inspect_schedule_fire(self, fire_id: str) -> JsonObject:
        return await self._controller.inspect_schedule_fire(fire_id)

    async def inspect_heartbeat_fire(self, fire_id: str) -> JsonObject:
        return await self._controller.inspect_heartbeat_fire(fire_id)

    async def inspect_workflow_run(self, workflow_run_id: str) -> JsonObject:
        return await self._controller.inspect_workflow_run(workflow_run_id)

    async def inspect_memory_entry(self, entry_id: str) -> JsonObject:
        return await self._controller.inspect_memory_entry(entry_id)

    async def inspect_agency_fire(self, fire_id: str) -> JsonObject:
        return await self._controller.inspect_agency_fire(fire_id)


class ProductToolset(AbstractToolset):
    name = "claw.product.service"
    id = "claw.product.service"

    def __init__(self, client: ProductSelfClient) -> None:
        super().__init__()
        self._client = client

    async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation:
        return ToolsetPreparation(
            tools=[
                Tool(
                    self.wait_for_operator,
                    name="wait_for_operator",
                    description="Wait for product operator steering before continuing.",
                    parameters_schema={"type": "object", "properties": {}},
                    sequential=True,
                ),
                Tool(
                    self.deploy_service,
                    name="deploy_service",
                    description="Deploy a named service through the product controller.",
                    parameters_schema={
                        "type": "object",
                        "properties": {"service": {"type": "string"}},
                        "required": ["service"],
                    },
                    sequential=True,
                ),
            ],
        )

    async def wait_for_operator(
        self,
        ctx: ToolContext,
        args: dict[str, object],
    ) -> dict[str, bool]:
        del ctx, args
        return await self._client.wait_for_operator()

    async def deploy_service(
        self,
        ctx: ToolContext,
        args: dict[str, object],
    ) -> dict[str, str]:
        service = str(args["service"])
        if ctx.approval is None:
            raise ApprovalRequired(
                f"deploy {service}",
                metadata={"service": service, "risk": "medium"},
            )
        return await self._client.deploy_service(service)


class AsyncTaskToolset(AbstractToolset):
    name = "claw.product.async_tasks"
    id = "claw.product.async_tasks"

    def __init__(self, client: ProductSelfClient) -> None:
        super().__init__()
        self._client = client

    async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation:
        return ToolsetPreparation(
            tools=[
                Tool(
                    self.spawn_async_task,
                    name="spawn_async_task",
                    description="Create a durable async product subagent task.",
                    parameters_schema={
                        "type": "object",
                        "properties": {
                            "task_id": {"type": "string"},
                            "parent_session_id": {"type": "string"},
                            "prompt": {"type": "string"},
                        },
                        "required": ["parent_session_id", "prompt"],
                    },
                    sequential=True,
                ),
                Tool(
                    self.inspect_async_task,
                    name="inspect_async_task",
                    description="Inspect a durable async product task.",
                    parameters_schema={
                        "type": "object",
                        "properties": {"task_id": {"type": "string"}},
                        "required": ["task_id"],
                    },
                    sequential=True,
                ),
                Tool(
                    self.steer_async_task,
                    name="steer_async_task",
                    description="Send steering input to a durable async product task.",
                    parameters_schema={
                        "type": "object",
                        "properties": {
                            "task_id": {"type": "string"},
                            "message": {"type": "string"},
                        },
                        "required": ["task_id", "message"],
                    },
                    sequential=True,
                ),
                Tool(
                    self.cancel_async_task,
                    name="cancel_async_task",
                    description="Cancel a durable async product task.",
                    parameters_schema={
                        "type": "object",
                        "properties": {
                            "task_id": {"type": "string"},
                            "reason": {"type": "string"},
                        },
                        "required": ["task_id", "reason"],
                    },
                    sequential=True,
                ),
            ],
        )

    async def spawn_async_task(self, ctx: ToolContext, args: dict[str, object]) -> JsonObject:
        del ctx
        task_id = args.get("task_id")
        return await self._client.spawn_async_task(
            task_id=str(task_id) if task_id is not None else None,
            parent_session_id=str(args["parent_session_id"]),
            prompt=str(args["prompt"]),
        )

    async def inspect_async_task(self, ctx: ToolContext, args: dict[str, object]) -> JsonObject:
        del ctx
        return await self._client.inspect_async_task(str(args["task_id"]))

    async def steer_async_task(self, ctx: ToolContext, args: dict[str, object]) -> JsonObject:
        del ctx
        return await self._client.steer_async_task(
            str(args["task_id"]),
            str(args["message"]),
        )

    async def cancel_async_task(self, ctx: ToolContext, args: dict[str, object]) -> JsonObject:
        del ctx
        return await self._client.cancel_async_task(
            str(args["task_id"]),
            str(args["reason"]),
        )


class SessionTraceToolset(AbstractToolset):
    name = "claw.product.session_trace"
    id = "claw.product.session_trace"

    def __init__(self, client: ProductSelfClient) -> None:
        super().__init__()
        self._client = client

    async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation:
        return ToolsetPreparation(
            tools=[
                Tool(
                    self.inspect_session,
                    name="inspect_session",
                    description="Inspect a product session summary.",
                    parameters_schema={
                        "type": "object",
                        "properties": {"session_id": {"type": "string"}},
                        "required": ["session_id"],
                    },
                    sequential=True,
                ),
                Tool(
                    self.list_session_runs,
                    name="list_session_runs",
                    description="List product runs for a session.",
                    parameters_schema={
                        "type": "object",
                        "properties": {"session_id": {"type": "string"}},
                        "required": ["session_id"],
                    },
                    sequential=True,
                ),
                Tool(
                    self.inspect_run_trace,
                    name="inspect_run_trace",
                    description="Inspect canonical stream and replay evidence for a run.",
                    parameters_schema={
                        "type": "object",
                        "properties": {"run_id": {"type": "string"}},
                        "required": ["run_id"],
                    },
                    sequential=True,
                ),
            ],
        )

    async def inspect_session(self, ctx: ToolContext, args: dict[str, object]) -> JsonObject:
        del ctx
        return await self._client.inspect_session(str(args["session_id"]))

    async def list_session_runs(
        self, ctx: ToolContext, args: dict[str, object]
    ) -> list[JsonObject]:
        del ctx
        return await self._client.list_session_runs(str(args["session_id"]))

    async def inspect_run_trace(self, ctx: ToolContext, args: dict[str, object]) -> JsonObject:
        del ctx
        return await self._client.inspect_run_trace(str(args["run_id"]))


class ScheduleWorkflowToolset(AbstractToolset):
    name = "claw.product.schedule_workflow"
    id = "claw.product.schedule_workflow"

    def __init__(self, client: ProductSelfClient) -> None:
        super().__init__()
        self._client = client

    async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation:
        return ToolsetPreparation(
            tools=[
                Tool(
                    self.inspect_schedule_fire,
                    name="inspect_schedule_fire",
                    description="Inspect a product schedule fire record.",
                    parameters_schema={
                        "type": "object",
                        "properties": {"fire_id": {"type": "string"}},
                        "required": ["fire_id"],
                    },
                    sequential=True,
                ),
                Tool(
                    self.inspect_heartbeat_fire,
                    name="inspect_heartbeat_fire",
                    description="Inspect a product heartbeat fire record.",
                    parameters_schema={
                        "type": "object",
                        "properties": {"fire_id": {"type": "string"}},
                        "required": ["fire_id"],
                    },
                    sequential=True,
                ),
                Tool(
                    self.inspect_workflow_run,
                    name="inspect_workflow_run",
                    description="Inspect a product workflow run and node records.",
                    parameters_schema={
                        "type": "object",
                        "properties": {"workflow_run_id": {"type": "string"}},
                        "required": ["workflow_run_id"],
                    },
                    sequential=True,
                ),
            ],
        )

    async def inspect_schedule_fire(
        self,
        ctx: ToolContext,
        args: dict[str, object],
    ) -> JsonObject:
        del ctx
        return await self._client.inspect_schedule_fire(str(args["fire_id"]))

    async def inspect_heartbeat_fire(
        self,
        ctx: ToolContext,
        args: dict[str, object],
    ) -> JsonObject:
        del ctx
        return await self._client.inspect_heartbeat_fire(str(args["fire_id"]))

    async def inspect_workflow_run(
        self,
        ctx: ToolContext,
        args: dict[str, object],
    ) -> JsonObject:
        del ctx
        return await self._client.inspect_workflow_run(str(args["workflow_run_id"]))


class MemoryAgencyToolset(AbstractToolset):
    name = "claw.product.memory_agency"
    id = "claw.product.memory_agency"

    def __init__(self, client: ProductSelfClient) -> None:
        super().__init__()
        self._client = client

    async def prepare(self, ctx: ToolsetContext) -> ToolsetPreparation:
        return ToolsetPreparation(
            tools=[
                Tool(
                    self.inspect_memory_entry,
                    name="inspect_memory_entry",
                    description="Inspect a product workspace memory entry.",
                    parameters_schema={
                        "type": "object",
                        "properties": {"entry_id": {"type": "string"}},
                        "required": ["entry_id"],
                    },
                    sequential=True,
                ),
                Tool(
                    self.inspect_agency_fire,
                    name="inspect_agency_fire",
                    description="Inspect a product agency fire record.",
                    parameters_schema={
                        "type": "object",
                        "properties": {"fire_id": {"type": "string"}},
                        "required": ["fire_id"],
                    },
                    sequential=True,
                ),
            ],
        )

    async def inspect_memory_entry(
        self,
        ctx: ToolContext,
        args: dict[str, object],
    ) -> JsonObject:
        del ctx
        return await self._client.inspect_memory_entry(str(args["entry_id"]))

    async def inspect_agency_fire(
        self,
        ctx: ToolContext,
        args: dict[str, object],
    ) -> JsonObject:
        del ctx
        return await self._client.inspect_agency_fire(str(args["fire_id"]))


class ProfileResolver:
    def __init__(self, client: ProductSelfClient) -> None:
        self._client = client

    def resolve(self, name: str = "default") -> ProductProfile:
        if name != "default":
            raise KeyError(f"unknown profile: {name}")
        return ProductProfile(
            name="default",
            instructions=("You are running inside a Claw-like product service.",),
            model_factory=self._model,
            toolset_factory=lambda: (
                ProductToolset(self._client),
                AsyncTaskToolset(self._client),
                SessionTraceToolset(self._client),
                ScheduleWorkflowToolset(self._client),
                MemoryAgencyToolset(self._client),
            ),
        )

    def _model(self) -> FunctionModel:
        def respond(messages: list[object], info: dict[str, object]) -> dict[str, object]:
            params = cast(dict[str, Any], info["params"])
            tools = cast(list[dict[str, Any]], params["tools"])
            tool_names = {tool["name"] for tool in tools}
            self._validate_tools(tool_names)
            return self._response_for_rendered(str(messages))

        return FunctionModel(respond)

    def _validate_tools(self, tool_names: set[str]) -> None:
        required_tools = self._required_tool_names()
        if not required_tools.issubset(tool_names):
            raise RuntimeError(f"missing product tools: {sorted(tool_names)}")

    def _response_for_rendered(self, rendered: str) -> dict[str, object]:
        if "Manage async task" in rendered:
            return self._async_task_response(rendered)
        if "Spawn background async task" in rendered:
            return self._background_async_task_spawn_response(rendered)
        if "Inspect session trace" in rendered:
            return self._session_trace_response(rendered)
        if "Inspect schedule workflow" in rendered:
            return self._schedule_workflow_inspection_response(rendered)
        if "Inspect memory agency" in rendered:
            return self._memory_agency_inspection_response(rendered)
        simple_response = self._simple_response_for_rendered(rendered)
        if simple_response is not None:
            return simple_response
        return self._deployment_response(rendered)

    @staticmethod
    def _simple_response_for_rendered(rendered: str) -> dict[str, object] | None:
        if "Scheduled heartbeat" in rendered:
            return {"text": "scheduled run complete"}
        if "Heartbeat fire" in rendered:
            return {"text": "heartbeat run complete"}
        if "Workflow node" in rendered:
            return {"text": f"workflow node {_extract_marker(rendered, 'node_id')} complete"}
        if "Memory extraction" in rendered:
            return {"text": "memory entry extracted"}
        if "Agency fire" in rendered:
            return {"text": "agency fire complete"}
        if "Background async task" in rendered:
            return {"text": "background async task complete"}
        return None

    @staticmethod
    def _required_tool_names() -> set[str]:
        return {
            "wait_for_operator",
            "deploy_service",
            "spawn_async_task",
            "inspect_async_task",
            "steer_async_task",
            "cancel_async_task",
            "inspect_session",
            "list_session_runs",
            "inspect_run_trace",
            "inspect_schedule_fire",
            "inspect_heartbeat_fire",
            "inspect_workflow_run",
            "inspect_memory_entry",
            "inspect_agency_fire",
        }

    @staticmethod
    def _async_task_response(rendered: str) -> dict[str, object]:
        task_id = "async_task_demo"
        if "spawn_async_task" not in rendered:
            return {
                "tool_calls": [
                    {
                        "id": "call_spawn_async",
                        "name": "spawn_async_task",
                        "arguments": {
                            "task_id": task_id,
                            "parent_session_id": "product-session",
                            "prompt": "Summarize release risk.",
                        },
                    }
                ]
            }
        if "inspect_async_task" not in rendered:
            return {
                "tool_calls": [
                    {
                        "id": "call_inspect_async",
                        "name": "inspect_async_task",
                        "arguments": {"task_id": task_id},
                    }
                ]
            }
        if "steer_async_task" not in rendered:
            return {
                "tool_calls": [
                    {
                        "id": "call_steer_async",
                        "name": "steer_async_task",
                        "arguments": {
                            "task_id": task_id,
                            "message": "Prioritize staged rollout risk.",
                        },
                    }
                ]
            }
        if "cancel_async_task" not in rendered:
            return {
                "tool_calls": [
                    {
                        "id": "call_cancel_async",
                        "name": "cancel_async_task",
                        "arguments": {
                            "task_id": task_id,
                            "reason": "parent run completed inspection",
                        },
                    }
                ]
            }
        return {"text": "async task lifecycle complete"}

    @staticmethod
    def _background_async_task_spawn_response(rendered: str) -> dict[str, object]:
        task_id = "background_task_demo"
        if "spawn_async_task" not in rendered:
            return {
                "tool_calls": [
                    {
                        "id": "call_spawn_background_async",
                        "name": "spawn_async_task",
                        "arguments": {
                            "task_id": task_id,
                            "parent_session_id": "product-session",
                            "prompt": f"Background async task task_id={task_id}",
                        },
                    }
                ]
            }
        return {"text": "background async task spawned"}

    @staticmethod
    def _session_trace_response(rendered: str) -> dict[str, object]:
        session_id = _extract_marker(rendered, "session_id")
        run_id = _extract_marker(rendered, "run_id")
        if "inspect_session" not in rendered:
            return {
                "tool_calls": [
                    {
                        "id": "call_inspect_session",
                        "name": "inspect_session",
                        "arguments": {"session_id": session_id},
                    }
                ]
            }
        if "list_session_runs" not in rendered:
            return {
                "tool_calls": [
                    {
                        "id": "call_list_session_runs",
                        "name": "list_session_runs",
                        "arguments": {"session_id": session_id},
                    }
                ]
            }
        if "inspect_run_trace" not in rendered:
            return {
                "tool_calls": [
                    {
                        "id": "call_inspect_run_trace",
                        "name": "inspect_run_trace",
                        "arguments": {"run_id": run_id},
                    }
                ]
            }
        return {"text": "session trace inspected"}

    @staticmethod
    def _schedule_workflow_inspection_response(rendered: str) -> dict[str, object]:
        schedule_fire_id = _extract_marker(rendered, "schedule_fire_id")
        heartbeat_fire_id = _extract_marker(rendered, "heartbeat_fire_id")
        workflow_run_id = _extract_marker(rendered, "workflow_run_id")
        if "inspect_schedule_fire" not in rendered:
            return {
                "tool_calls": [
                    {
                        "id": "call_inspect_schedule_fire",
                        "name": "inspect_schedule_fire",
                        "arguments": {"fire_id": schedule_fire_id},
                    }
                ]
            }
        if "inspect_heartbeat_fire" not in rendered:
            return {
                "tool_calls": [
                    {
                        "id": "call_inspect_heartbeat_fire",
                        "name": "inspect_heartbeat_fire",
                        "arguments": {"fire_id": heartbeat_fire_id},
                    }
                ]
            }
        if "inspect_workflow_run" not in rendered:
            return {
                "tool_calls": [
                    {
                        "id": "call_inspect_workflow_run",
                        "name": "inspect_workflow_run",
                        "arguments": {"workflow_run_id": workflow_run_id},
                    }
                ]
            }
        return {"text": "schedule workflow inspected"}

    @staticmethod
    def _memory_agency_inspection_response(rendered: str) -> dict[str, object]:
        memory_entry_id = _extract_marker(rendered, "memory_entry_id")
        agency_fire_id = _extract_marker(rendered, "agency_fire_id")
        if "inspect_memory_entry" not in rendered:
            return {
                "tool_calls": [
                    {
                        "id": "call_inspect_memory_entry",
                        "name": "inspect_memory_entry",
                        "arguments": {"entry_id": memory_entry_id},
                    }
                ]
            }
        if "inspect_agency_fire" not in rendered:
            return {
                "tool_calls": [
                    {
                        "id": "call_inspect_agency_fire",
                        "name": "inspect_agency_fire",
                        "arguments": {"fire_id": agency_fire_id},
                    }
                ]
            }
        return {"text": "memory agency inspected"}

    @staticmethod
    def _deployment_response(rendered: str) -> dict[str, object]:
        if "wait_for_operator" not in rendered:
            return {
                "tool_calls": [
                    {
                        "id": "call_wait",
                        "name": "wait_for_operator",
                        "arguments": {},
                    }
                ]
            }
        if "deploy_service" not in rendered:
            if "Use staged rollout." not in rendered:
                raise RuntimeError("operator steering text was not delivered")
            return {
                "tool_calls": [
                    {
                        "id": "call_deploy",
                        "name": "deploy_service",
                        "arguments": {"service": "api"},
                    }
                ]
            }
        return {"text": "deployment complete"}


class RuntimeBuilder:
    def __init__(
        self,
        *,
        store: SqliteSessionStore,
        archive: SqliteStreamArchive,
        replay: SqliteReplayEventLog,
    ) -> None:
        self._store = store
        self._archive = archive
        self._replay = replay

    def build_agent(
        self,
        profile: ProductProfile,
        *,
        environment: EnvironmentProvider | None = None,
    ) -> Agent:
        toolsets = profile.toolset_factory()
        validate_toolset_ids(toolsets).raise_for_errors()
        return create_agent(
            model=profile.model_factory(),
            instructions=profile.instructions,
            toolsets=toolsets,
            approval_required_tools=profile.approval_required_tools,
            environment=environment,
        )

    def build_runtime(
        self,
        profile: ProductProfile,
        *,
        durable_session_id: str,
        environment: EnvironmentProvider | None = None,
        state: Mapping[str, Any] | None = None,
    ) -> AgentRuntime:
        toolsets = profile.toolset_factory()
        validate_toolset_ids(toolsets).raise_for_errors()
        return create_agent_runtime(
            model=profile.model_factory(),
            instructions=profile.instructions,
            toolsets=toolsets,
            approval_required_tools=profile.approval_required_tools,
            environment=environment,
            session_store=self._store,
            stream_archive=self._archive,
            replay_event_log=self._replay,
            durable_session_id=durable_session_id,
            state=state,
        )


class ClawProductRuntime:
    def __init__(self, database_path: str | Path) -> None:
        self.database_path = Path(database_path)
        self.native_database_path = self.database_path.with_name(
            f"{self.database_path.stem}.starweaver{self.database_path.suffix}"
        )
        self.database_path.parent.mkdir(parents=True, exist_ok=True)
        self.native_database_path.parent.mkdir(parents=True, exist_ok=True)
        self.runtime_id = _new_id("runtime")
        self.database = ProductDatabase(self.database_path)
        self.controller = ProductController(self.database)
        self.controller.set_trace_reader(self.trace_summary)
        self.self_client = ProductSelfClient(self.controller)
        self.notifications = NotificationHub(self.database)
        self.resolver = ProfileResolver(self.self_client)
        self.workspace_factory = WorkspaceFactory()
        self.store: SqliteSessionStore | None = None
        self.archive: SqliteStreamArchive | None = None
        self.replay: SqliteReplayEventLog | None = None
        self.runtime_builder: RuntimeBuilder | None = None
        self._active_runs: dict[str, ActiveProductRun] = {}
        self.dispatcher = ProductDispatcher(self)
        self.bridge = ProductBridge(self)
        self._ready = False
        self._startup_report: JsonObject = {
            "started_at": None,
            "steps": [],
            "startup_recovery_completed": False,
            "recovered_orphan_runs": 0,
        }

    async def start(self) -> None:
        if self._ready:
            return
        steps: list[str] = []
        steps.append("settings_loaded")
        steps.append("data_directories_ready")
        self.database.migrate()
        steps.append("product_database_migrated")
        SqliteSessionStore.migrate(self.native_database_path)
        self.store = SqliteSessionStore.open(self.native_database_path)
        self.archive = SqliteStreamArchive.open(self.native_database_path)
        self.replay = SqliteReplayEventLog.open(self.native_database_path)
        native_status = SqliteSessionStore.migration_status(self.native_database_path)
        steps.append("native_store_opened")
        self.runtime_builder = RuntimeBuilder(
            store=self.store,
            archive=self.archive,
            replay=self.replay,
        )
        steps.append("runtime_state_built")
        steps.append("workspace_provider_built")
        steps.append("profile_resolver_built")
        self.database.seed_profile(self.resolver.resolve())
        self.database.register_runtime(self.runtime_id)
        steps.append("runtime_registered")
        recovered = self.database.recover_orphan_running()
        steps.append("startup_recovery_completed")
        if recovered:
            await self.notifications.publish("runtime.recovered", {"runs": recovered})
        steps.append("supervisors_available")
        steps.append("service_ready")
        self._startup_report = {
            "started_at": _now(),
            "steps": steps,
            "startup_recovery_completed": True,
            "recovered_orphan_runs": recovered,
            "native_store_current": native_status["current"],
        }
        self._ready = True

    async def shutdown(self) -> None:
        await self.dispatcher.stop()
        for active in list(self._active_runs.values()):
            active.run.interrupt("product runtime shutdown")
        self.database.stop_runtime(self.runtime_id)
        self.database.close()
        self._ready = False

    def ready(self) -> JsonObject:
        return {
            "ready": self._ready,
            "runtime_id": self.runtime_id,
            "startup_recovery_completed": bool(
                self._startup_report.get("startup_recovery_completed")
            ),
            "supervisors": self.supervisor_status(),
        }

    def doctor(self) -> JsonObject:
        native_status = SqliteSessionStore.migration_status(self.native_database_path)
        return {
            "ready": self._ready,
            "runtime_id": self.runtime_id,
            "database": str(self.database_path),
            "native_database": str(self.native_database_path),
            "counts": self.database.counts(),
            "native_store_current": native_status["current"],
            "startup": dict(self._startup_report),
            "stores": {
                "product_database": str(self.database_path),
                "native_database": str(self.native_database_path),
                "native_store_current": native_status["current"],
            },
            "workspace_provider": {
                "backend": "virtual",
                "factory": type(self.workspace_factory).__name__,
            },
            "notification_hub": {
                "available": True,
                "notifications": len(self.notifications.replay()),
            },
            "supervisors": self.supervisor_status(),
            "bridge": {
                "available": True,
                "mode": "embedded_optional",
            },
        }

    def supervisor_status(self) -> JsonObject:
        return {
            "execution": {
                "kind": "manual_run_coordinator",
                "running": self._ready,
            },
            "dispatcher": {
                "kind": "schedule_heartbeat_dispatcher",
                "running": self.dispatcher.running,
            },
            "bridge": {
                "kind": "embedded_bridge_adapter",
                "available": True,
                "running": self._ready,
            },
        }

    async def create_session(
        self,
        *,
        profile: str = "default",
        workspace: Mapping[str, Any] | None = None,
    ) -> str:
        self._require_ready()
        workspace_binding = self.workspace_factory.normalize(workspace)
        session_id = self.database.create_session(
            profile=profile,
            workspace=workspace_binding.to_dict(),
        )
        await self.notifications.publish("session.created", {"session_id": session_id})
        return session_id

    def run_details(self, run_id: str) -> JsonObject:
        return self.database.run_details(run_id)

    def sandbox_status(self, run_id: str) -> JsonObject:
        details = self.run_details(run_id)
        return {
            "run_id": run_id,
            "status": details["sandbox_status"].get("status"),
            "sandbox_status": details["sandbox_status"],
            "workspace_snapshot": details["workspace_snapshot"],
        }

    async def cleanup_expired_sandboxes(
        self,
        *,
        ttl_seconds: int = 3600,
    ) -> JsonObject:
        self._require_ready()
        result = self.database.cleanup_expired_sandboxes(ttl_seconds=ttl_seconds)
        if result["cleaned"]:
            await self.notifications.publish("sandbox.cleanup", result)
        return result

    def async_task_details(self, task_id: str) -> JsonObject:
        return self.database.async_task(task_id)

    async def run_background_async_task(self, task_id: str) -> JsonObject:
        self._require_ready()
        task = self.database.async_task(task_id)
        if task["status"] != "running":
            raise RuntimeError(f"async task is not running: {task['status']}")
        worker_session_id = await self.create_session()
        queued = self.database.create_run(
            worker_session_id,
            str(task["prompt"]),
            trigger_type="async_task",
        )
        task = self.database.attach_async_task_run(
            task_id=task_id,
            worker_session_id=worker_session_id,
            worker_run_id=queued.run_id,
        )
        await self.notifications.publish(
            "async_task.worker_started",
            {"task_id": task_id, "run_id": queued.run_id},
        )
        completed = await self.run_next_queued()
        if completed is None or completed.run_id != queued.run_id:
            raise RuntimeError("async task worker run did not execute")
        completed_task = self.database.complete_async_task(
            task_id=task_id,
            status=completed.status,
            output=completed.output,
        )
        await self.notifications.publish(
            "async_task.parent_wake",
            completed_task["wake_parent"],
        )
        return {
            "task": completed_task,
            "worker_run": _run_view_dict(completed),
        }

    def schedule_fire_details(self, fire_id: str) -> JsonObject:
        return self.database.schedule_fire(fire_id)

    def heartbeat_fire_details(self, fire_id: str) -> JsonObject:
        return self.database.heartbeat_fire(fire_id)

    def workflow_run_details(self, workflow_run_id: str) -> JsonObject:
        return self.database.workflow_run(workflow_run_id)

    def memory_entry_details(self, entry_id: str) -> JsonObject:
        return self.database.memory_entry(entry_id)

    def agency_fire_details(self, fire_id: str) -> JsonObject:
        return self.database.agency_fire(fire_id)

    def upsert_schedule(self, schedule_id: str, prompt: str) -> JsonObject:
        self._require_ready()
        return self.database.upsert_schedule(schedule_id, prompt)

    async def trace_summary(self, run_id: str) -> JsonObject:
        details = self.database.run_details(run_id)
        ui_state = await self.replay_ui_state(run_id)
        return {
            "run_id": run_id,
            "session_id": details["session_id"],
            "status": details["status"],
            "behavior": details["behavior"],
            "native_session_id": details["native_session_id"],
            "native_run_id": details["native_run_id"],
            "raw_records": len(ui_state["raw_records"]),
            "display_messages": len(ui_state["display_messages"]),
            "replay_events": len(ui_state["replay_events"]),
            "tool_events": len(ui_state.get("tool_events", [])),
            "terminal": ui_state.get("terminal"),
        }

    async def dispatch_schedule_fire(
        self,
        *,
        schedule_id: str = "schedule_demo",
        prompt: str = "Scheduled heartbeat schedule_id=schedule_demo",
    ) -> JsonObject:
        self._require_ready()
        schedule = self.database.upsert_schedule(schedule_id, prompt)
        session_id = await self.create_session()
        queued = self.database.create_run(session_id, prompt, trigger_type="schedule")
        fire = self.database.create_schedule_fire(
            schedule_id=schedule_id,
            session_id=session_id,
            run_id=queued.run_id,
        )
        await self.notifications.publish("schedule.fire", {"fire_id": fire["fire_id"]})
        completed = await self.run_next_queued()
        if completed is None or completed.run_id != queued.run_id:
            raise RuntimeError("scheduled run did not execute")
        completed_fire = self.database.complete_schedule_fire(fire["fire_id"], completed.status)
        return {
            "schedule": schedule,
            "fire": completed_fire,
            "run": _run_view_dict(completed),
        }

    async def dispatch_heartbeat_fire(self) -> JsonObject:
        self._require_ready()
        session_id = await self.create_session()
        queued = self.database.create_run(
            session_id,
            "Heartbeat fire heartbeat_id=heartbeat_demo",
            trigger_type="heartbeat",
        )
        fire = self.database.create_heartbeat_fire(session_id=session_id, run_id=queued.run_id)
        await self.notifications.publish("heartbeat.fire", {"fire_id": fire["fire_id"]})
        completed = await self.run_next_queued()
        if completed is None or completed.run_id != queued.run_id:
            raise RuntimeError("heartbeat run did not execute")
        completed_fire = self.database.complete_heartbeat_fire(fire["fire_id"], completed.status)
        return {
            "fire": completed_fire,
            "run": _run_view_dict(completed),
        }

    async def dispatch_workflow(self, *, workflow_id: str = "workflow_demo") -> JsonObject:
        self._require_ready()
        nodes = [
            {"id": "plan", "prompt": "Workflow node node_id=plan"},
            {"id": "execute", "prompt": "Workflow node node_id=execute"},
        ]
        workflow = self.database.upsert_workflow(
            workflow_id=workflow_id,
            name="Release workflow",
            nodes=nodes,
        )
        session_id = await self.create_session()
        workflow_run = self.database.create_workflow_run(
            workflow_id=workflow_id,
            session_id=session_id,
        )
        await self.notifications.publish(
            "workflow.started",
            {"workflow_run_id": workflow_run["workflow_run_id"]},
        )
        for node in workflow["definition"]["nodes"]:
            queued = self.database.create_run(
                session_id,
                str(node["prompt"]),
                trigger_type="workflow",
            )
            workflow_run = self.database.create_workflow_node_run(
                workflow_run_id=workflow_run["workflow_run_id"],
                node_id=str(node["id"]),
                run_id=queued.run_id,
            )
            node_run_id = str(workflow_run["nodes"][-1]["node_run_id"])
            completed = await self.run_next_queued()
            if completed is None or completed.run_id != queued.run_id:
                raise RuntimeError("workflow node run did not execute")
            self.database.complete_workflow_node_run(
                node_run_id=node_run_id,
                status=completed.status,
                output=completed.output,
            )
        completed_workflow = self.database.complete_workflow_run(
            workflow_run["workflow_run_id"],
            "completed",
        )
        await self.notifications.publish(
            "workflow.completed",
            {"workflow_run_id": completed_workflow["workflow_run_id"]},
        )
        return {
            "workflow": workflow,
            "workflow_run": completed_workflow,
        }

    async def dispatch_memory_extraction(
        self,
        *,
        source_run_id: str,
        scope: str = "workspace",
    ) -> JsonObject:
        self._require_ready()
        session_id = await self.create_session()
        queued = self.database.create_run(
            session_id,
            f"Memory extraction source_run_id={source_run_id}",
            trigger_type="memory",
        )
        await self.notifications.publish(
            "memory.extraction_started",
            {"source_run_id": source_run_id, "run_id": queued.run_id},
        )
        completed = await self.run_next_queued()
        if completed is None or completed.run_id != queued.run_id:
            raise RuntimeError("memory extraction run did not execute")
        summary = completed.output or ""
        entry = self.database.create_memory_entry(
            scope=scope,
            source_run_id=source_run_id,
            content=f"source_run_id={source_run_id}; output={summary}",
            summary=summary,
        )
        await self.notifications.publish(
            "memory.entry_created",
            {"entry_id": entry["entry_id"], "source_run_id": source_run_id},
        )
        return {
            "entry": entry,
            "run": _run_view_dict(completed),
        }

    async def dispatch_agency_fire(
        self,
        *,
        agency_id: str = "agency_demo",
    ) -> JsonObject:
        self._require_ready()
        try:
            agency = self.database.agency_session(agency_id)
        except KeyError:
            session_id = await self.create_session()
            agency = self.database.create_agency_session(agency_id, session_id)
        queued = self.database.create_run(
            str(agency["session_id"]),
            f"Agency fire agency_id={agency_id}",
            trigger_type="agency",
        )
        fire = self.database.create_agency_fire(
            agency_id=agency_id,
            session_id=str(agency["session_id"]),
            run_id=queued.run_id,
        )
        await self.notifications.publish("agency.fire", {"fire_id": fire["fire_id"]})
        completed = await self.run_next_queued()
        if completed is None or completed.run_id != queued.run_id:
            raise RuntimeError("agency run did not execute")
        completed_fire = self.database.complete_agency_fire(
            fire_id=fire["fire_id"],
            status=completed.status,
            output=completed.output,
        )
        await self.notifications.publish(
            "agency.completed",
            {"fire_id": completed_fire["fire_id"], "agency_id": agency_id},
        )
        return {
            "agency": agency,
            "fire": completed_fire,
            "run": _run_view_dict(completed),
        }

    async def submit(self, session_id: str, text: str) -> ProductRunView:
        self._require_ready()
        active = self.database.active_run(session_id)
        if active is None:
            view = self.database.create_run(session_id, text)
            await self.notifications.publish("run.queued", {"run_id": view.run_id})
            return view
        if active["status"] == "queued":
            view = self.database.merge_queued_input(active, text)
            await self.notifications.publish("run.merged", {"run_id": view.run_id})
            return view
        if active["status"] == "running":
            active_run = self._active_runs.get(active["id"])
            if active_run is None:
                raise RuntimeError("running run is not owned by this runtime")
            receipt = await active_run.run.steer(text, id=_new_id("steer"))
            self.controller.operator_release.set()
            await self.notifications.publish(
                "run.steered",
                {"run_id": active["id"], "receipt": receipt.id},
            )
            return ProductRunView(active["id"], session_id, "running", "steered")
        return ProductRunView(active["id"], session_id, active["status"], "hitl_waiting")

    async def run_next_queued(self) -> ProductRunView | None:
        self._require_ready()
        run = self.database.claim_next_queued(self.runtime_id)
        if run is None:
            return None
        return await self._run_claimed(run)

    async def approve_next(self, run_id: str, *, decided_by: str = "operator") -> ProductRunView:
        self._require_ready()
        run = self.database.run(run_id)
        if run["status"] != "hitl":
            raise RuntimeError(f"run is not waiting for HITL: {run['status']}")
        if self.store is None or self.runtime_builder is None:
            raise RuntimeError("runtime is not started")
        native_session_id = str(run["native_session_id"])
        native_run_id = run["native_run_id"]
        if not isinstance(native_run_id, str) or not native_run_id:
            raise RuntimeError("run does not have a durable native run id")
        session_record = await self.store.load_session(native_session_id)
        last_run_state = _decode(run["last_run_state_json"], None)
        if not isinstance(last_run_state, Mapping):
            raise TypeError("run does not have restorable HITL state")
        restored_state = dict(session_record.state)
        restored_metadata = restored_state.get("metadata")
        metadata = dict(restored_metadata) if isinstance(restored_metadata, Mapping) else {}
        metadata["starweaver.durable_run_id"] = native_run_id
        restored_state["metadata"] = metadata
        archive = SessionArchive.from_state(
            restored_state,
            mode="full",
            last_run_state=last_run_state,
        )
        workspace_runtime = await self._workspace_for_run(run)
        profile = self.resolver.resolve(self.database.session(run["session_id"])["profile"])
        agent = self.runtime_builder.build_agent(
            profile,
            environment=workspace_runtime.environment,
        )
        session = agent.session_from_archive(archive, environment=workspace_runtime.environment)
        pending_hitl = _decode(run["pending_hitl_json"], {})
        approvals = pending_hitl.get("approvals", [])
        if not approvals:
            raise RuntimeError("run does not have pending approval records")
        decision = ApprovalDecision(
            id=str(approvals[0]["approval_id"]),
            approved=True,
            decided_by=decided_by,
        )
        result = await session.resume_after_hitl(approvals=[decision])
        stream_result = type("_SyntheticStreamResult", (), {"result": result, "events": []})()
        return await self._persist_terminal(
            run_id,
            session=session,
            stream_result=stream_result,
            behavior="resumed",
        )

    async def replay_ui_state(self, run_id: str) -> JsonObject:
        self._require_ready()
        run = self.database.run(run_id)
        native_session_id = run["native_session_id"]
        native_run_id = run["native_run_id"]
        if native_session_id is None or native_run_id is None:
            return {"run_id": run_id, "raw_records": [], "display_messages": []}
        if self.archive is None or self.replay is None:
            raise RuntimeError("runtime is not started")
        scope = f"run:{native_run_id}"
        raw_records = await self.archive.replay_raw_after(native_session_id, native_run_id)
        display_messages = await self.archive.replay_display_after(scope)
        replay_events = await self.replay.replay_after(scope)
        adapter = StreamAdapter(record.to_dict() for record in raw_records)
        terminal = adapter.terminal()
        return {
            "run_id": run_id,
            "native_session_id": native_session_id,
            "native_run_id": native_run_id,
            "raw_records": [record.to_dict() for record in raw_records],
            "display_messages": display_messages,
            "replay_events": replay_events,
            "tool_events": adapter.tool_events(),
            "sideband_events": adapter.sideband_events(),
            "terminal": terminal.kind if terminal is not None else None,
        }

    def force_orphan_running(self, run_id: str) -> None:
        self.database.update_run_status(run_id, "running", behavior="orphaned")

    def _require_ready(self) -> None:
        if not self._ready:
            raise RuntimeError("runtime is not started")

    async def _workspace_for_run(self, run: sqlite3.Row) -> WorkspaceRuntime:
        workspace_snapshot = _decode(run["workspace_snapshot_json"], None)
        if not isinstance(workspace_snapshot, Mapping):
            session = self.database.session(run["session_id"])
            workspace_snapshot = _decode(session["workspace_json"], {})
        return await self.workspace_factory.runtime_for(workspace_snapshot)

    def _sandbox_status(self, run_id: str, status: str) -> JsonObject:
        current = self.database.run_details(run_id)["sandbox_status"]
        if not isinstance(current, Mapping):
            current = {}
        updated = dict(current)
        updated["status"] = status
        updated["updated_at"] = _now()
        if status in {"stopped", "failed", "interrupted"}:
            updated["stopped_at"] = _now()
        return updated

    async def _run_claimed(self, run: sqlite3.Row) -> ProductRunView:
        if self.runtime_builder is None:
            raise RuntimeError("runtime is not started")
        workspace_runtime = await self._workspace_for_run(run)
        self.database.update_run_status(
            run["id"],
            "running",
            behavior=run["behavior"],
            workspace_snapshot=workspace_runtime.workspace_snapshot,
            sandbox_status=workspace_runtime.sandbox_status,
        )
        profile = self.resolver.resolve(self.database.session(run["session_id"])["profile"])
        agent = self.runtime_builder.build_agent(
            profile,
            environment=workspace_runtime.environment,
        )
        session = agent.session()
        active_run = session.run_stream(run["prompt"], environment=workspace_runtime.environment)
        active = ActiveProductRun(
            run_id=run["id"],
            session_id=run["session_id"],
            agent=agent,
            session=session,
            run=active_run,
            events=[],
            workspace_snapshot=workspace_runtime.workspace_snapshot,
            sandbox_status=workspace_runtime.sandbox_status,
        )
        self._active_runs[run["id"]] = active
        await self.notifications.publish("run.started", {"run_id": run["id"]})
        try:
            async for event in active_run:
                active.events.append(event)
                await self.notifications.publish(
                    "run.event",
                    {"run_id": run["id"], "kind": event.kind, "raw": event.raw},
                )
                if event.kind == "suspended":
                    stream_result = await active_run.join()
                    return await self._persist_suspended(
                        run["id"],
                        session=session,
                        stream_result=stream_result,
                    )
            stream_result = await active_run.join()
            return await self._persist_terminal(
                run["id"],
                session=session,
                stream_result=stream_result,
                behavior="completed",
            )
        except BaseException:
            self.database.update_run_status(
                run["id"],
                "failed",
                behavior="failed",
                sandbox_status=self._sandbox_status(run["id"], "failed"),
            )
            raise
        finally:
            self._active_runs.pop(run["id"], None)

    async def _persist_suspended(
        self,
        run_id: str,
        *,
        session: AgentSession,
        stream_result: Any,
    ) -> ProductRunView:
        if self.store is None:
            raise RuntimeError("runtime is not started")
        session_record = await self.store.save_current_session(session)
        run_record = RunRecord.from_result(
            session_record.session_id,
            stream_result.result,
            sequence_no=0,
        )
        run_record = await self._append_native_records(
            session_record.session_id,
            run_record,
            stream_result,
        )
        pending_hitl = {
            "approvals": stream_result.result.pending_approvals,
            "deferred": stream_result.result.pending_deferred,
        }
        view = self.database.update_run_status(
            run_id,
            "hitl",
            behavior="suspended",
            native_session_id=session_record.session_id,
            native_run_id=run_record.run_id,
            last_run_state=stream_result.result.raw_run_state,
            pending_hitl=pending_hitl,
            sandbox_status=self._sandbox_status(run_id, "suspended"),
        )
        await self.notifications.publish("run.hitl", {"run_id": run_id, "hitl": pending_hitl})
        return view

    async def _persist_terminal(
        self,
        run_id: str,
        *,
        session: AgentSession,
        stream_result: Any,
        behavior: str,
    ) -> ProductRunView:
        if self.store is None:
            raise RuntimeError("runtime is not started")
        session_record = await self.store.save_current_session(session)
        run_record = RunRecord.from_result(
            session_record.session_id,
            stream_result.result,
            sequence_no=0,
        )
        run_record = await self._append_native_records(
            session_record.session_id,
            run_record,
            stream_result,
        )
        existing = self.database.run(run_id)
        mapped_run_id = (
            run_record.run_id
            if stream_result.events
            else existing["native_run_id"] or run_record.run_id
        )
        view = self.database.update_run_status(
            run_id,
            "completed",
            behavior=behavior,
            native_session_id=session_record.session_id,
            native_run_id=mapped_run_id,
            output=stream_result.result.output,
            last_run_state=stream_result.result.raw_run_state,
            sandbox_status=self._sandbox_status(run_id, "stopped"),
        )
        await self.notifications.publish(
            "run.completed",
            {"run_id": run_id, "output": stream_result.result.output},
        )
        return view

    async def _append_native_records(
        self,
        native_session_id: str,
        run_record: RunRecord,
        stream_result: Any,
    ) -> RunRecord:
        if self.store is None or self.archive is None or self.replay is None:
            raise RuntimeError("runtime is not started")
        events = list(stream_result.events)
        run_record = await self.store.append_run_allocated(run_record)
        if events:
            raw_records = [event.raw for event in events]
            await self.store.append_stream_records(
                native_session_id,
                run_record.run_id,
                raw_records,
            )
            await self.archive.append_raw_records(
                native_session_id,
                run_record.run_id,
                raw_records,
            )
        scope = f"run:{run_record.run_id}"
        display_messages = [
            {
                "sequence": index,
                "session_id": native_session_id,
                "run_id": run_record.run_id,
                "timestamp": _now(),
                "type": "HOST_EVENT",
                "payload": event.raw,
                "preview": event.kind,
            }
            for index, event in enumerate(events)
        ]
        if display_messages:
            await self.archive.append_display_messages(scope, display_messages)
        await self.replay.append(
            scope,
            {
                "scope": scope,
                "sequence": 1,
                "timestamp": _now(),
                "event": {"kind": "heartbeat", "source": "product_runtime"},
            },
        )
        return run_record


class ProductDispatcher:
    def __init__(self, runtime: ClawProductRuntime, *, interval_seconds: float = 60.0) -> None:
        if interval_seconds <= 0:
            raise ValueError("interval_seconds must be positive")
        self._runtime = runtime
        self._interval_seconds = interval_seconds
        self._task: asyncio.Task[None] | None = None
        self._stop_event: asyncio.Event | None = None

    @property
    def running(self) -> bool:
        return self._task is not None and not self._task.done()

    async def run_once(self) -> JsonObject:
        self._runtime._require_ready()
        schedule_results: list[JsonObject] = []
        for schedule in self._runtime.database.active_schedules():
            schedule_results.append(
                await self._runtime.dispatch_schedule_fire(
                    schedule_id=str(schedule["schedule_id"]),
                    prompt=str(schedule["prompt"]),
                )
            )
        heartbeat_result = await self._runtime.dispatch_heartbeat_fire()
        return {
            "schedule_fires": len(schedule_results),
            "schedule_fire_ids": [result["fire"]["fire_id"] for result in schedule_results],
            "heartbeat_fired": True,
            "heartbeat_fire_id": heartbeat_result["fire"]["fire_id"],
        }

    def start(self, *, run_immediately: bool = True) -> JsonObject:
        self._runtime._require_ready()
        if self.running:
            return {"running": True, "started": False}
        self._stop_event = asyncio.Event()
        self._task = asyncio.create_task(self._run_loop(run_immediately=run_immediately))
        return {"running": True, "started": True, "run_immediately": run_immediately}

    async def stop(self) -> JsonObject:
        task = self._task
        stop_event = self._stop_event
        if task is None or stop_event is None:
            return {"running": False, "stopped": False}
        stop_event.set()
        await task
        self._task = None
        self._stop_event = None
        return {"running": False, "stopped": True}

    async def _run_loop(self, *, run_immediately: bool) -> None:
        if self._stop_event is None:
            raise RuntimeError("dispatcher was not started")
        if run_immediately:
            await self.run_once()
        while True:
            try:
                await asyncio.wait_for(
                    self._stop_event.wait(),
                    timeout=self._interval_seconds,
                )
                return
            except TimeoutError:
                await self.run_once()


class ProductApiAuthError(PermissionError):
    pass


@dataclass(frozen=True)
class ProductApiConfig:
    expected_authorization: str = "Bearer product-test-auth"
    auth_enabled: bool = True
    cors_origins: tuple[str, ...] = ("http://localhost",)


class ProductApi:
    def __init__(
        self,
        runtime: ClawProductRuntime,
        config: ProductApiConfig | None = None,
    ) -> None:
        self._runtime = runtime
        self._config = config or ProductApiConfig()

    def ready(self, *, authorization: str | None = None) -> JsonObject:
        self._authorize(authorization)
        return self._runtime.ready()

    def doctor(self, *, authorization: str | None = None) -> JsonObject:
        self._authorize(authorization)
        return self._runtime.doctor()

    def sandbox_status(
        self,
        run_id: str,
        *,
        authorization: str | None = None,
    ) -> JsonObject:
        self._authorize(authorization)
        return self._runtime.sandbox_status(run_id)

    async def cleanup_sandboxes(
        self,
        payload: Mapping[str, Any] | None = None,
        *,
        authorization: str | None = None,
    ) -> JsonObject:
        self._authorize(authorization)
        body = self._body(payload)
        ttl_seconds = int(body.get("ttl_seconds", 3600))
        return await self._runtime.cleanup_expired_sandboxes(ttl_seconds=ttl_seconds)

    async def create_session(
        self,
        payload: Mapping[str, Any] | None = None,
        *,
        authorization: str | None = None,
    ) -> JsonObject:
        self._authorize(authorization)
        body = self._body(payload)
        workspace = body.get("workspace")
        if workspace is not None and not isinstance(workspace, Mapping):
            raise TypeError("workspace must be a mapping")
        session_id = await self._runtime.create_session(
            profile=str(body.get("profile") or "default"),
            workspace=workspace,
        )
        return {
            "session_id": session_id,
            "status": "created",
        }

    async def submit(
        self,
        session_id: str,
        payload: Mapping[str, Any],
        *,
        authorization: str | None = None,
    ) -> JsonObject:
        self._authorize(authorization)
        body = self._body(payload)
        text = body.get("text")
        if not isinstance(text, str) or not text:
            raise ValueError("text must be a non-empty string")
        return _run_view_dict(await self._runtime.submit(session_id, text))

    def notification_sse(
        self,
        *,
        after_sequence: int = 0,
        authorization: str | None = None,
    ) -> list[JsonObject]:
        self._authorize(authorization)
        return [
            {
                "id": str(notification["sequence"]),
                "cursor": notification["sequence"],
                "event": notification["topic"],
                "data": {
                    "topic": notification["topic"],
                    "payload": notification["payload"],
                    "created_at": notification["created_at"],
                },
            }
            for notification in self._runtime.notifications.replay(after_sequence)
        ]

    async def run_sse(
        self,
        run_id: str,
        *,
        authorization: str | None = None,
    ) -> list[JsonObject]:
        self._authorize(authorization)
        ui_state = await self._runtime.replay_ui_state(run_id)
        events: list[JsonObject] = []
        for message in ui_state.get("display_messages", []):
            sequence = int(message.get("sequence", len(events)))
            events.append(
                {
                    "id": f"{run_id}:display:{sequence}",
                    "cursor": sequence,
                    "event": "run.display",
                    "data": message,
                }
            )
        for replay_event in ui_state.get("replay_events", []):
            sequence = int(replay_event.get("sequence", len(events)))
            event = replay_event.get("event", {})
            kind = event.get("kind", "unknown") if isinstance(event, Mapping) else "unknown"
            events.append(
                {
                    "id": f"{run_id}:replay:{sequence}",
                    "cursor": sequence,
                    "event": f"run.replay.{kind}",
                    "data": replay_event,
                }
            )
        return events

    def _authorize(self, authorization: str | None) -> JsonObject:
        if not self._config.auth_enabled:
            return {"authenticated": True, "mode": "disabled"}
        if authorization != self._config.expected_authorization:
            raise ProductApiAuthError("invalid bearer token")
        return {"authenticated": True, "scheme": "bearer"}

    @staticmethod
    def _body(payload: Mapping[str, Any] | None) -> Mapping[str, Any]:
        if payload is None:
            return {}
        if not isinstance(payload, Mapping):
            raise TypeError("payload must be a mapping")
        return payload


@dataclass(frozen=True)
class ProductServiceConfig:
    api: ProductApiConfig = field(default_factory=ProductApiConfig)
    static_fallback: str = "web/index.html"
    start_execution_supervisor: bool = True
    start_bridge_supervisor: bool = False


class ProductServiceApp:
    def __init__(
        self,
        database_path: str | Path,
        config: ProductServiceConfig | None = None,
    ) -> None:
        self.config = config or ProductServiceConfig()
        self.runtime = ClawProductRuntime(database_path)
        self.api = ProductApi(self.runtime, self.config.api)

    def factory_metadata(self) -> JsonObject:
        return {
            "factory": "fastapi_compatible_product_facade",
            "routes": self.route_map(),
            "middleware": self.middleware(),
            "cors": self.cors_policy(),
            "static": self.static_fallback(),
        }

    @asynccontextmanager
    async def lifespan(self) -> AsyncIterator[ProductServiceApp]:
        await self.start()
        try:
            yield self
        finally:
            await self.shutdown()

    async def start(self) -> JsonObject:
        await self.runtime.start()
        dispatcher = {"running": self.runtime.dispatcher.running, "started": False}
        if self.config.start_execution_supervisor:
            dispatcher = self.runtime.dispatcher.start(run_immediately=False)
        return {
            "ready": self.runtime.ready()["ready"],
            "startup": self.runtime.doctor()["startup"],
            "supervisors": {
                "dispatcher": dispatcher,
                "bridge": {
                    "enabled": self.config.start_bridge_supervisor,
                    "available": self.runtime.bridge is not None,
                },
            },
        }

    async def shutdown(self) -> JsonObject:
        was_ready = self.runtime.ready()["ready"]
        await self.runtime.shutdown()
        return {
            "was_ready": was_ready,
            "ready": self.runtime.ready()["ready"],
        }

    def ready(self) -> JsonObject:
        ready = self.runtime.ready()
        ready["service"] = self.factory_metadata()
        return ready

    def doctor(self) -> JsonObject:
        doctor = self.runtime.doctor()
        doctor["service"] = self.factory_metadata()
        doctor["api"] = {
            "auth": {
                "enabled": self.config.api.auth_enabled,
                "scheme": "bearer" if self.config.api.auth_enabled else "disabled",
            },
            "cors": self.cors_policy(),
            "routes": self.route_map(),
        }
        doctor["static"] = self.static_fallback()
        return doctor

    def migrate(self) -> JsonObject:
        self.runtime.database.migrate()
        SqliteSessionStore.migrate(self.runtime.native_database_path)
        native_status = SqliteSessionStore.migration_status(self.runtime.native_database_path)
        return {
            "product_database": str(self.runtime.database_path),
            "native_database": str(self.runtime.native_database_path),
            "native_store_current": native_status["current"],
        }

    def middleware(self) -> list[JsonObject]:
        return [
            {
                "name": "api_token",
                "enabled": self.config.api.auth_enabled,
                "scheme": "bearer",
            }
        ]

    def cors_policy(self) -> JsonObject:
        return {"origins": list(self.config.api.cors_origins)}

    def static_fallback(self) -> JsonObject:
        return {
            "enabled": bool(self.config.static_fallback),
            "fallback": self.config.static_fallback,
        }

    @staticmethod
    def route_map() -> list[str]:
        return [
            "GET /healthz",
            "GET /api/v1/claw/ready",
            "GET /api/v1/claw/doctor",
            "POST /api/v1/sessions",
            "POST /api/v1/sessions/{session_id}/submit",
            "GET /api/v1/runs/{run_id}/events",
            "GET /api/v1/notifications",
        ]


def create_product_service_app(
    database_path: str | Path,
    config: ProductServiceConfig | None = None,
) -> ProductServiceApp:
    return ProductServiceApp(database_path, config)


class ProductBridge:
    def __init__(self, runtime: ClawProductRuntime) -> None:
        self._runtime = runtime

    async def publish_hitl(
        self,
        run_id: str,
        *,
        channel: str = "lark",
        external_id: str = "release-room",
    ) -> JsonObject:
        self._runtime._require_ready()
        run = self._runtime.database.run(run_id)
        if run["status"] != "hitl":
            raise RuntimeError(f"run is not waiting for HITL: {run['status']}")
        pending_hitl = _decode(run["pending_hitl_json"], {})
        approvals = pending_hitl.get("approvals", [])
        if not approvals:
            raise RuntimeError("run does not have pending approval records")
        approval_id = str(approvals[0]["approval_id"])
        conversation = self._runtime.database.upsert_bridge_conversation(
            channel=channel,
            external_id=external_id,
        )
        payload = {
            "run_id": run_id,
            "approval_id": approval_id,
            "pending_hitl": pending_hitl,
        }
        message = self._runtime.database.create_bridge_hitl_message(
            conversation_id=str(conversation["conversation_id"]),
            run_id=run_id,
            approval_id=approval_id,
            payload=payload,
        )
        event = self._runtime.database.append_bridge_event(
            conversation_id=str(conversation["conversation_id"]),
            kind="hitl.requested",
            payload={"message_id": message["message_id"], "run_id": run_id},
        )
        await self._runtime.notifications.publish(
            "bridge.hitl_requested",
            {"message_id": message["message_id"], "run_id": run_id},
        )
        return {
            "conversation": conversation,
            "message": message,
            "event": event,
        }

    async def approve_hitl(
        self,
        message_id: str,
        *,
        decided_by: str = "bridge",
    ) -> JsonObject:
        self._runtime._require_ready()
        message = self._runtime.database.bridge_hitl_message(message_id)
        if message["status"] != "waiting":
            raise RuntimeError(f"bridge HITL message is not waiting: {message['status']}")
        completed = await self._runtime.approve_next(str(message["run_id"]), decided_by=decided_by)
        decision = {
            "approved": True,
            "approval_id": message["approval_id"],
            "decided_by": decided_by,
            "completed_run_id": completed.run_id,
        }
        updated_message = self._runtime.database.complete_bridge_hitl_message(
            message_id=message_id,
            decision=decision,
        )
        event = self._runtime.database.append_bridge_event(
            conversation_id=str(message["conversation_id"]),
            kind="hitl.approved",
            payload=decision,
        )
        await self._runtime.notifications.publish(
            "bridge.hitl_approved",
            {"message_id": message_id, "run_id": completed.run_id},
        )
        return {
            "message": updated_message,
            "event": event,
            "run": _run_view_dict(completed),
        }


async def _suspended_native_sequence(
    runtime: ClawProductRuntime,
    suspended: ProductRunView,
) -> int:
    native_session_id = suspended.native_session_id
    native_run_id = suspended.native_run_id
    if native_session_id is None or native_run_id is None:
        raise RuntimeError("suspended run did not persist native identity")
    if runtime.store is None:
        raise RuntimeError("runtime store is not started")
    native_run = await runtime.store.load_run(native_session_id, native_run_id)
    return int(native_run.to_dict()["sequence_no"])


async def _native_hitl_continuation_state(
    runtime: ClawProductRuntime,
    suspended: ProductRunView,
    completed: JsonObject,
    suspended_sequence: int,
) -> tuple[bool, bool]:
    native_session_id = suspended.native_session_id
    native_run_id = suspended.native_run_id
    if native_session_id is None or native_run_id is None:
        raise RuntimeError("suspended run did not persist native identity")
    if runtime.store is None:
        raise RuntimeError("restarted runtime store is not started")
    completed_native_run = await runtime.store.load_run(native_session_id, native_run_id)
    native_runs = await runtime.store.list_runs(native_session_id)
    completed_native_record = completed_native_run.to_dict()
    identity_preserved = (
        completed["native_run_id"] == native_run_id
        and len(native_runs) == 1
        and native_runs[0].run_id == native_run_id
    )
    sequence_preserved = (
        completed_native_record["status"] == "completed"
        and int(completed_native_record["sequence_no"]) == suspended_sequence
    )
    return identity_preserved, sequence_preserved


async def run_product_runtime_smoke(database_path: str | Path) -> JsonObject:
    runtime = ClawProductRuntime(database_path)
    await runtime.start()
    try:
        session_id = await runtime.create_session()
        queued = await runtime.submit(session_id, "Deploy api")
        run_task = asyncio.create_task(runtime.run_next_queued())
        await runtime.controller.operator_waiting.wait()
        steered = await runtime.submit(session_id, "Use staged rollout.")
        suspended = await run_task
        if suspended is None:
            raise RuntimeError("run did not execute")
        suspended_native_sequence = await _suspended_native_sequence(runtime, suspended)
        await runtime.shutdown()

        restarted = ClawProductRuntime(database_path)
        await restarted.start()
        try:
            bridge_hitl = await restarted.bridge.publish_hitl(queued.run_id)
            bridge_approval = await restarted.bridge.approve_hitl(
                str(bridge_hitl["message"]["message_id"])
            )
            completed = bridge_approval["run"]
            (
                native_run_identity_preserved,
                native_sequence_preserved,
            ) = await _native_hitl_continuation_state(
                restarted,
                suspended,
                completed,
                suspended_native_sequence,
            )
            ui_state = await restarted.replay_ui_state(queued.run_id)
            details = restarted.run_details(queued.run_id)
            workspace_snapshot = details["workspace_snapshot"]
            sandbox_status = details["sandbox_status"]
            async_session_id = await restarted.create_session()
            await restarted.submit(async_session_id, "Manage async task")
            async_completed = await restarted.run_next_queued()
            if async_completed is None:
                raise RuntimeError("async task run did not execute")
            async_task = restarted.async_task_details("async_task_demo")
            background_parent_session_id = await restarted.create_session()
            await restarted.submit(background_parent_session_id, "Spawn background async task")
            background_spawn_completed = await restarted.run_next_queued()
            if background_spawn_completed is None:
                raise RuntimeError("background async task spawn did not execute")
            background_result = await restarted.run_background_async_task("background_task_demo")
            trace_session_id = await restarted.create_session()
            await restarted.submit(
                trace_session_id,
                f"Inspect session trace session_id={session_id} run_id={queued.run_id}",
            )
            trace_completed = await restarted.run_next_queued()
            if trace_completed is None:
                raise RuntimeError("trace run did not execute")
            trace_summary = await restarted.trace_summary(queued.run_id)
            schedule_result = await restarted.dispatch_schedule_fire()
            heartbeat_result = await restarted.dispatch_heartbeat_fire()
            workflow_result = await restarted.dispatch_workflow()
            workflow_run_id = workflow_result["workflow_run"]["workflow_run_id"]
            memory_result = await restarted.dispatch_memory_extraction(
                source_run_id=queued.run_id,
            )
            agency_result = await restarted.dispatch_agency_fire()
            inspection_session_id = await restarted.create_session()
            await restarted.submit(
                inspection_session_id,
                "Inspect schedule workflow "
                f"schedule_fire_id={schedule_result['fire']['fire_id']} "
                f"heartbeat_fire_id={heartbeat_result['fire']['fire_id']} "
                f"workflow_run_id={workflow_run_id}",
            )
            inspection_completed = await restarted.run_next_queued()
            if inspection_completed is None:
                raise RuntimeError("schedule workflow inspection did not execute")
            memory_inspection_session_id = await restarted.create_session()
            await restarted.submit(
                memory_inspection_session_id,
                "Inspect memory agency "
                f"memory_entry_id={memory_result['entry']['entry_id']} "
                f"agency_fire_id={agency_result['fire']['fire_id']}",
            )
            memory_inspection_completed = await restarted.run_next_queued()
            if memory_inspection_completed is None:
                raise RuntimeError("memory agency inspection did not execute")
            api = ProductApi(
                restarted,
                ProductApiConfig(expected_authorization="Bearer product-api"),
            )
            api_auth_rejected = False
            try:
                api.ready(authorization="Bearer wrong")
            except ProductApiAuthError:
                api_auth_rejected = True
            api_ready = api.ready(authorization="Bearer product-api")
            api_session = await api.create_session(
                {"profile": "default"},
                authorization="Bearer product-api",
            )
            api_submitted = await api.submit(
                str(api_session["session_id"]),
                {"text": "Scheduled heartbeat api_session=api"},
                authorization="Bearer product-api",
            )
            api_completed = await restarted.run_next_queued()
            if api_completed is None or api_completed.run_id != api_submitted["run_id"]:
                raise RuntimeError("api-submitted run did not execute")
            api_notifications = api.notification_sse(
                after_sequence=0,
                authorization="Bearer product-api",
            )
            api_notifications_after_first = api.notification_sse(
                after_sequence=int(api_notifications[0]["cursor"]),
                authorization="Bearer product-api",
            )
            api_run_events = await api.run_sse(
                queued.run_id,
                authorization="Bearer product-api",
            )
            api_sandbox_status = api.sandbox_status(
                queued.run_id,
                authorization="Bearer product-api",
            )
            api_cleanup = await api.cleanup_sandboxes(
                {"ttl_seconds": 0},
                authorization="Bearer product-api",
            )
            api_sandbox_status_after_cleanup = api.sandbox_status(
                queued.run_id,
                authorization="Bearer product-api",
            )
            dispatcher_once = await restarted.dispatcher.run_once()
            dispatcher_started = restarted.dispatcher.start(run_immediately=False)
            dispatcher_stopped = await restarted.dispatcher.stop()
            return {
                "run_id": queued.run_id,
                "queued_behavior": queued.behavior,
                "steered_behavior": steered.behavior,
                "suspended_status": suspended.status,
                "completed_status": completed["status"],
                "native_run_identity_preserved_after_hitl": native_run_identity_preserved,
                "native_sequence_preserved_after_hitl": native_sequence_preserved,
                "output": completed["output"],
                "bridge_hitl_message_status": bridge_approval["message"]["status"],
                "bridge_hitl_approval_id_preserved": (
                    bridge_hitl["message"]["approval_id"]
                    == bridge_approval["message"]["approval_id"]
                ),
                "bridge_hitl_run_status": bridge_approval["run"]["status"],
                "async_run_status": async_completed.status,
                "async_run_output": async_completed.output,
                "async_task_status": async_task["status"],
                "async_task_transcript": len(async_task["transcript"]),
                "async_task_wake_parent": async_task["result"]["wake_parent"],
                "background_async_spawn_output": background_spawn_completed.output,
                "background_async_task_status": background_result["task"]["status"],
                "background_async_task_output": background_result["task"]["result"]["output"],
                "background_async_task_wake_parent": (
                    background_result["task"]["wake_parent"]["parent_session_id"]
                    == "product-session"
                ),
                "background_async_worker_output": background_result["worker_run"]["output"],
                "trace_run_status": trace_completed.status,
                "trace_run_output": trace_completed.output,
                "trace_summary_status": trace_summary["status"],
                "trace_summary_raw_records": trace_summary["raw_records"],
                "trace_summary_terminal": trace_summary["terminal"],
                "schedule_fire_status": schedule_result["fire"]["status"],
                "schedule_fire_id": schedule_result["fire"]["fire_id"],
                "schedule_run_output": schedule_result["run"]["output"],
                "heartbeat_fire_status": heartbeat_result["fire"]["status"],
                "heartbeat_fire_id": heartbeat_result["fire"]["fire_id"],
                "heartbeat_run_output": heartbeat_result["run"]["output"],
                "workflow_run_status": workflow_result["workflow_run"]["status"],
                "workflow_run_id": workflow_run_id,
                "workflow_node_count": len(workflow_result["workflow_run"]["nodes"]),
                "workflow_node_outputs": [
                    node["output"] for node in workflow_result["workflow_run"]["nodes"]
                ],
                "schedule_workflow_inspection_status": inspection_completed.status,
                "schedule_workflow_inspection_output": inspection_completed.output,
                "memory_entry_summary": memory_result["entry"]["summary"],
                "memory_entry_id": memory_result["entry"]["entry_id"],
                "memory_run_output": memory_result["run"]["output"],
                "agency_fire_status": agency_result["fire"]["status"],
                "agency_fire_id": agency_result["fire"]["fire_id"],
                "agency_run_output": agency_result["run"]["output"],
                "memory_agency_inspection_status": memory_inspection_completed.status,
                "memory_agency_inspection_output": memory_inspection_completed.output,
                "api_auth_rejected": api_auth_rejected,
                "api_ready": api_ready["ready"],
                "api_session_status": api_session["status"],
                "api_submit_status": api_submitted["status"],
                "api_run_output": api_completed.output,
                "api_notification_sse_events": len(api_notifications),
                "api_notification_sse_after_first": len(api_notifications_after_first),
                "api_run_sse_events": len(api_run_events),
                "api_run_sse_first_event": api_run_events[0]["event"],
                "api_sandbox_status": api_sandbox_status["status"],
                "api_sandbox_cleanup_cleaned": api_cleanup["cleaned"],
                "api_sandbox_cleanup_contains_run": any(
                    run["run_id"] == queued.run_id for run in api_cleanup["runs"]
                ),
                "api_sandbox_status_after_cleanup": api_sandbox_status_after_cleanup["status"],
                "dispatcher_schedule_fires": dispatcher_once["schedule_fires"],
                "dispatcher_heartbeat_fired": dispatcher_once["heartbeat_fired"],
                "dispatcher_loop_started": dispatcher_started["started"],
                "dispatcher_loop_stopped": dispatcher_stopped["stopped"],
                "workspace_backend": workspace_snapshot["backend"],
                "workspace_default_cwd": workspace_snapshot["default_cwd"],
                "workspace_fingerprint": workspace_snapshot["fingerprint"],
                "sandbox_status": sandbox_status["status"],
                "raw_records": len(ui_state["raw_records"]),
                "display_messages": len(ui_state["display_messages"]),
                "replay_events": len(ui_state["replay_events"]),
                "notifications": len(restarted.notifications.replay()),
                "ready": restarted.ready()["ready"],
            }
        finally:
            await restarted.shutdown()
    finally:
        if runtime.ready()["ready"]:
            await runtime.shutdown()


async def main() -> None:
    with tempfile.TemporaryDirectory() as directory:
        result = await run_product_runtime_smoke(Path(directory) / "claw-product.sqlite3")
    print(result)


if __name__ == "__main__":
    asyncio.run(main())
