//! SQLite storage adapters for Claw.

use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use starweaver_context::ResumableState;
use starweaver_core::{Metadata, RunId, SessionId};
use starweaver_runtime::{AgentCheckpoint, AgentStreamRecord};
use starweaver_session::{
    ApprovalRecord, ApprovalStatus, CompactRunTrace, CompactSessionTrace, DeferredToolRecord,
    EnvironmentStateRef, RunRecord, RunStatus, SessionFilter, SessionRecord, SessionResumeSnapshot,
    SessionStatus, SessionStore, SessionStoreError, SessionStoreResult, StreamCursorRef,
};
use starweaver_stream::{
    InMemoryReplayEventLog, ReplayCursor, ReplayError, ReplayEvent, ReplayEventKind,
    ReplayEventLog, ReplayResult, ReplayScope, ReplaySnapshot, ReplaySubscription,
};

const SQLITE_SCHEMA_MIGRATION_TABLE: &str = "starweaver_schema_migrations";

#[derive(Clone, Copy, Debug)]
struct SqliteMigration {
    id: &'static str,
    description: &'static str,
    sql: &'static str,
}

const SQLITE_MIGRATIONS: &[SqliteMigration] = &[
    SqliteMigration {
        id: "20260605_000001_session_stream_store",
        description: "create durable session, run, checkpoint, stream, approval, deferred tool, and replay tables",
        sql: r"
            CREATE TABLE IF NOT EXISTS session_records (
                session_id TEXT PRIMARY KEY,
                record TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS run_records (
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                record TEXT NOT NULL,
                sequence_no INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (session_id, run_id)
            );
            CREATE INDEX IF NOT EXISTS ix_claw_runs_session_sequence
                ON run_records(session_id, sequence_no);

            CREATE TABLE IF NOT EXISTS checkpoints (
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                sequence_no INTEGER NOT NULL,
                checkpoint_id TEXT NOT NULL,
                record TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (session_id, run_id, sequence_no, checkpoint_id)
            );

            CREATE TABLE IF NOT EXISTS stream_records (
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                sequence_no INTEGER NOT NULL,
                record TEXT NOT NULL,
                PRIMARY KEY (session_id, run_id, sequence_no)
            );

            CREATE TABLE IF NOT EXISTS approvals (
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                approval_id TEXT NOT NULL,
                record TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (session_id, run_id, approval_id)
            );

            CREATE TABLE IF NOT EXISTS deferred_tools (
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                deferred_id TEXT NOT NULL,
                record TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (session_id, run_id, deferred_id)
            );

            CREATE TABLE IF NOT EXISTS replay_events (
                scope TEXT NOT NULL,
                sequence_no INTEGER NOT NULL,
                record TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (scope, sequence_no)
            );
            CREATE INDEX IF NOT EXISTS ix_replay_events_scope_sequence
                ON replay_events(scope, sequence_no);

            CREATE TABLE IF NOT EXISTS replay_snapshots (
                scope TEXT PRIMARY KEY,
                record TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
        ",
    },
    SqliteMigration {
        id: "20260605_000002_latest_backend_schema",
        description: "create the latest Starweaver Claw backend resource schema and import compatible Python Claw records",

        sql: r"
            CREATE TABLE IF NOT EXISTS session_records (
                session_id TEXT PRIMARY KEY,
                record TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS run_records (
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                record TEXT NOT NULL,
                sequence_no INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (session_id, run_id)
            );
            CREATE INDEX IF NOT EXISTS ix_claw_runs_session_sequence
                ON run_records(session_id, sequence_no);

            CREATE TABLE IF NOT EXISTS checkpoints (
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                sequence_no INTEGER NOT NULL,
                checkpoint_id TEXT NOT NULL,
                record TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (session_id, run_id, sequence_no, checkpoint_id)
            );

            CREATE TABLE IF NOT EXISTS stream_records (
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                sequence_no INTEGER NOT NULL,
                record TEXT NOT NULL,
                PRIMARY KEY (session_id, run_id, sequence_no)
            );

            CREATE TABLE IF NOT EXISTS approvals (
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                approval_id TEXT NOT NULL,
                record TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (session_id, run_id, approval_id)
            );

            CREATE TABLE IF NOT EXISTS deferred_tools (
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                deferred_id TEXT NOT NULL,
                record TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (session_id, run_id, deferred_id)
            );

            CREATE TABLE IF NOT EXISTS replay_events (
                scope TEXT NOT NULL,
                sequence_no INTEGER NOT NULL,
                record TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (scope, sequence_no)
            );
            CREATE INDEX IF NOT EXISTS ix_replay_events_scope_sequence
                ON replay_events(scope, sequence_no);

            CREATE TABLE IF NOT EXISTS replay_snapshots (
                scope TEXT PRIMARY KEY,
                record TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS profiles (
                name TEXT PRIMARY KEY,
                model TEXT NOT NULL,
                model_settings_preset TEXT,
                model_settings_override TEXT,
                model_config_preset TEXT,
                model_config_override TEXT,
                system_prompt TEXT,
                builtin_toolsets TEXT NOT NULL DEFAULT '[]',
                subagents TEXT NOT NULL DEFAULT '[]',
                include_builtin_subagents INTEGER NOT NULL DEFAULT 0,
                unified_subagents INTEGER NOT NULL DEFAULT 0,
                need_user_approve_tools TEXT NOT NULL DEFAULT '[]',
                need_user_approve_mcps TEXT NOT NULL DEFAULT '[]',
                enabled_mcps TEXT NOT NULL DEFAULT '[]',
                disabled_mcps TEXT NOT NULL DEFAULT '[]',
                mcp_servers TEXT NOT NULL DEFAULT '{}',
                workspace_backend_hint TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                source_type TEXT,
                source_version TEXT,
                source_checksum TEXT,
                created_at TEXT,
                updated_at TEXT
            );

            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                parent_session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL,
                profile_name TEXT,
                session_type TEXT NOT NULL DEFAULT 'conversation',
                source_session_id TEXT,
                metadata TEXT NOT NULL DEFAULT '{}',
                head_run_id TEXT,
                head_success_run_id TEXT,
                active_run_id TEXT,
                created_at TEXT,
                updated_at TEXT
            );
            CREATE INDEX IF NOT EXISTS ix_sessions_session_type_updated ON sessions(session_type, updated_at);
            CREATE INDEX IF NOT EXISTS ix_sessions_source_session ON sessions(source_session_id);
            CREATE UNIQUE INDEX IF NOT EXISTS ix_sessions_type_source_unique ON sessions(session_type, source_session_id);

            CREATE TABLE IF NOT EXISTS runs (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                sequence_no INTEGER NOT NULL,
                restore_from_run_id TEXT,
                status TEXT DEFAULT 'queued',
                trigger_type TEXT DEFAULT 'api',
                profile_name TEXT,
                input_parts TEXT NOT NULL DEFAULT '[]',
                metadata TEXT NOT NULL DEFAULT '{}',
                output_text TEXT,
                error_message TEXT,
                termination_reason TEXT,
                created_at TEXT,
                started_at TEXT,
                finished_at TEXT,
                committed_at TEXT,
                claimed_by TEXT,
                claimed_at TEXT
            );
            CREATE INDEX IF NOT EXISTS ix_runs_session_id ON runs(session_id);
            CREATE INDEX IF NOT EXISTS ix_runs_session_created_at ON runs(session_id, created_at);
            CREATE UNIQUE INDEX IF NOT EXISTS ix_runs_session_sequence_no ON runs(session_id, sequence_no);

            CREATE TABLE IF NOT EXISTS bridge_conversations (
                id TEXT PRIMARY KEY,
                adapter TEXT NOT NULL,
                tenant_key TEXT NOT NULL DEFAULT 'default',
                external_chat_id TEXT NOT NULL,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                profile_name TEXT,
                metadata TEXT NOT NULL DEFAULT '{}',
                created_at TEXT,
                updated_at TEXT,
                last_event_at TEXT,
                UNIQUE(adapter, tenant_key, external_chat_id)
            );
            CREATE INDEX IF NOT EXISTS ix_bridge_conversations_session_id ON bridge_conversations(session_id);

            CREATE TABLE IF NOT EXISTS bridge_events (
                id TEXT PRIMARY KEY,
                adapter TEXT NOT NULL,
                tenant_key TEXT NOT NULL DEFAULT 'default',
                event_id TEXT NOT NULL,
                external_message_id TEXT,
                external_chat_id TEXT,
                conversation_id TEXT,
                session_id TEXT,
                run_id TEXT,
                event_type TEXT NOT NULL,
                status TEXT DEFAULT 'received',
                raw_event TEXT NOT NULL DEFAULT '{}',
                normalized_event TEXT NOT NULL DEFAULT '{}',
                error_message TEXT,
                created_at TEXT,
                updated_at TEXT,
                UNIQUE(adapter, tenant_key, event_id),
                UNIQUE(adapter, tenant_key, external_message_id)
            );
            CREATE INDEX IF NOT EXISTS ix_bridge_events_chat_created_at ON bridge_events(external_chat_id, created_at);
            CREATE INDEX IF NOT EXISTS ix_bridge_events_session_id ON bridge_events(session_id);
            CREATE INDEX IF NOT EXISTS ix_bridge_events_run_id ON bridge_events(run_id);

            CREATE TABLE IF NOT EXISTS runtime_instances (
                id TEXT PRIMARY KEY,
                hostname TEXT,
                process_id INTEGER,
                status TEXT DEFAULT 'active',
                metadata TEXT NOT NULL DEFAULT '{}',
                started_at TEXT,
                heartbeat_at TEXT,
                stopped_at TEXT
            );

            CREATE TABLE IF NOT EXISTS session_memory_states (
                source_session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                memory_session_id TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                last_extracted_sequence_no INTEGER NOT NULL DEFAULT 0,
                turns_since_extract INTEGER NOT NULL DEFAULT 0,
                extract_count INTEGER NOT NULL DEFAULT 0,
                extracts_since_summary INTEGER NOT NULL DEFAULT 0,
                pending_extract INTEGER NOT NULL DEFAULT 0,
                pending_summary INTEGER NOT NULL DEFAULT 0,
                last_extract_run_id TEXT,
                last_summary_run_id TEXT,
                metadata TEXT NOT NULL DEFAULT '{}',
                created_at TEXT,
                updated_at TEXT
            );

            CREATE TABLE IF NOT EXISTS schedules (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT,
                status TEXT DEFAULT 'active',
                owner_kind TEXT DEFAULT 'api',
                owner_session_id TEXT,
                owner_run_id TEXT,
                profile_name TEXT,
                trigger_kind TEXT NOT NULL DEFAULT 'cron',
                cron_expr TEXT,
                run_at TEXT,
                timezone TEXT DEFAULT 'UTC',
                next_fire_at TEXT,
                execution_mode TEXT DEFAULT 'isolate_session',
                target_session_id TEXT,
                source_session_id TEXT,
                on_active TEXT DEFAULT 'queue',
                input_parts_template TEXT NOT NULL DEFAULT '[]',
                workflow_id TEXT,
                workflow_inputs_template TEXT,
                metadata TEXT NOT NULL DEFAULT '{}',
                last_fire_at TEXT,
                last_fire_id TEXT,
                last_session_id TEXT,
                last_run_id TEXT,
                last_workflow_run_id TEXT,
                fire_count INTEGER DEFAULT 0,
                failure_count INTEGER DEFAULT 0,
                created_at TEXT,
                updated_at TEXT
            );
            CREATE INDEX IF NOT EXISTS ix_schedules_due ON schedules(status, next_fire_at);
            CREATE INDEX IF NOT EXISTS ix_schedules_trigger_kind ON schedules(trigger_kind);
            CREATE INDEX IF NOT EXISTS ix_schedules_owner_session ON schedules(owner_session_id);
            CREATE INDEX IF NOT EXISTS ix_schedules_target_session ON schedules(target_session_id);
            CREATE INDEX IF NOT EXISTS ix_schedules_source_session ON schedules(source_session_id);

            CREATE TABLE IF NOT EXISTS schedule_fires (
                id TEXT PRIMARY KEY,
                schedule_id TEXT NOT NULL REFERENCES schedules(id) ON DELETE CASCADE,
                scheduled_at TEXT NOT NULL,
                fired_at TEXT,
                status TEXT DEFAULT 'pending',
                dedupe_key TEXT NOT NULL,
                target_session_id TEXT,
                source_session_id TEXT,
                created_session_id TEXT,
                run_id TEXT,
                active_run_id TEXT,
                workflow_run_id TEXT,
                input_parts TEXT NOT NULL DEFAULT '[]',
                error_message TEXT,
                metadata TEXT NOT NULL DEFAULT '{}',
                created_at TEXT,
                updated_at TEXT,
                UNIQUE(schedule_id, dedupe_key)
            );
            CREATE INDEX IF NOT EXISTS ix_schedule_fires_schedule_created ON schedule_fires(schedule_id, created_at);
            CREATE INDEX IF NOT EXISTS ix_schedule_fires_status_scheduled ON schedule_fires(status, scheduled_at);
            CREATE INDEX IF NOT EXISTS ix_schedule_fires_run ON schedule_fires(run_id);

            CREATE TABLE IF NOT EXISTS heartbeat_fires (
                id TEXT PRIMARY KEY,
                scheduled_at TEXT NOT NULL,
                fired_at TEXT,
                status TEXT DEFAULT 'pending',
                dedupe_key TEXT NOT NULL UNIQUE,
                session_id TEXT,
                run_id TEXT,
                error_message TEXT,
                metadata TEXT NOT NULL DEFAULT '{}',
                created_at TEXT,
                updated_at TEXT
            );
            CREATE INDEX IF NOT EXISTS ix_heartbeat_fires_status_scheduled ON heartbeat_fires(status, scheduled_at);
            CREATE INDEX IF NOT EXISTS ix_heartbeat_fires_run ON heartbeat_fires(run_id);

            CREATE TABLE IF NOT EXISTS agency_fires (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                status TEXT DEFAULT 'pending',
                scheduled_at TEXT NOT NULL,
                fired_at TEXT,
                dedupe_key TEXT NOT NULL UNIQUE,
                source_session_id TEXT,
                source_run_id TEXT,
                agency_session_id TEXT,
                run_id TEXT,
                active_run_id TEXT,
                priority INTEGER DEFAULT 100,
                payload TEXT NOT NULL DEFAULT '{}',
                error_message TEXT,
                created_at TEXT,
                updated_at TEXT,
                consumed_at TEXT
            );
            CREATE INDEX IF NOT EXISTS ix_agency_fires_status_scheduled ON agency_fires(status, scheduled_at);
            CREATE INDEX IF NOT EXISTS ix_agency_fires_kind_created ON agency_fires(kind, created_at);
            CREATE INDEX IF NOT EXISTS ix_agency_fires_run ON agency_fires(run_id);
            CREATE INDEX IF NOT EXISTS ix_agency_fires_source ON agency_fires(source_session_id);

            CREATE TABLE IF NOT EXISTS session_async_tasks (
                id TEXT PRIMARY KEY,
                parent_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                parent_run_id TEXT,
                parent_agent_id TEXT NOT NULL DEFAULT 'main',
                task_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                task_run_id TEXT,
                subagent_name TEXT NOT NULL,
                name TEXT NOT NULL,
                status TEXT DEFAULT 'queued',
                wake_policy TEXT DEFAULT 'steer_or_run',
                input_parts TEXT NOT NULL DEFAULT '[]',
                result_run_id TEXT,
                error_message TEXT,
                metadata TEXT NOT NULL DEFAULT '{}',
                created_at TEXT,
                updated_at TEXT,
                completed_at TEXT,
                UNIQUE(parent_session_id, name)
            );
            CREATE INDEX IF NOT EXISTS ix_session_async_tasks_parent_status ON session_async_tasks(parent_session_id, status);
            CREATE INDEX IF NOT EXISTS ix_session_async_tasks_task_session ON session_async_tasks(task_session_id);
            CREATE INDEX IF NOT EXISTS ix_session_async_tasks_task_run ON session_async_tasks(task_run_id);
            CREATE INDEX IF NOT EXISTS ix_session_async_tasks_name ON session_async_tasks(parent_session_id, name);

            CREATE TABLE IF NOT EXISTS hitl_batches (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
                status TEXT DEFAULT 'pending',
                current_interaction_id TEXT,
                deferred_requests TEXT,
                created_at TEXT,
                updated_at TEXT,
                completed_at TEXT
            );
            CREATE INDEX IF NOT EXISTS ix_hitl_batches_run_status ON hitl_batches(run_id, status);
            CREATE INDEX IF NOT EXISTS ix_hitl_batches_session_status ON hitl_batches(session_id, status);

            CREATE TABLE IF NOT EXISTS hitl_interactions (
                id TEXT PRIMARY KEY,
                batch_id TEXT NOT NULL REFERENCES hitl_batches(id) ON DELETE CASCADE,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
                interaction_id TEXT NOT NULL,
                tool_call_id TEXT NOT NULL,
                tool_name TEXT,
                kind TEXT DEFAULT 'approval',
                sequence_no INTEGER NOT NULL,
                total_count INTEGER NOT NULL,
                status TEXT DEFAULT 'pending',
                title TEXT NOT NULL,
                description TEXT,
                arguments_preview TEXT,
                metadata TEXT NOT NULL DEFAULT '{}',
                response TEXT,
                created_at TEXT,
                updated_at TEXT,
                resolved_at TEXT,
                UNIQUE(batch_id, interaction_id)
            );
            CREATE INDEX IF NOT EXISTS ix_hitl_interactions_run_status ON hitl_interactions(run_id, status);
            CREATE INDEX IF NOT EXISTS ix_hitl_interactions_batch_sequence ON hitl_interactions(batch_id, sequence_no);

            CREATE TABLE IF NOT EXISTS hitl_deferred_inputs (
                id TEXT PRIMARY KEY,
                batch_id TEXT NOT NULL REFERENCES hitl_batches(id) ON DELETE CASCADE,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
                conversation_id TEXT,
                adapter TEXT NOT NULL,
                tenant_key TEXT NOT NULL DEFAULT 'default',
                external_event_id TEXT NOT NULL,
                external_message_id TEXT,
                external_chat_id TEXT,
                sequence_no INTEGER NOT NULL,
                input_parts TEXT NOT NULL DEFAULT '[]',
                source_metadata TEXT NOT NULL DEFAULT '{}',
                status TEXT DEFAULT 'pending',
                created_at TEXT,
                updated_at TEXT,
                consumed_at TEXT,
                UNIQUE(adapter, tenant_key, external_event_id),
                UNIQUE(adapter, tenant_key, external_message_id)
            );
            CREATE INDEX IF NOT EXISTS ix_hitl_deferred_inputs_batch_sequence ON hitl_deferred_inputs(batch_id, sequence_no);
            CREATE INDEX IF NOT EXISTS ix_hitl_deferred_inputs_run_status ON hitl_deferred_inputs(run_id, status);

            CREATE TABLE IF NOT EXISTS bridge_hitl_messages (
                id TEXT PRIMARY KEY,
                adapter TEXT NOT NULL,
                tenant_key TEXT NOT NULL DEFAULT 'default',
                external_chat_id TEXT NOT NULL,
                external_message_id TEXT NOT NULL,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
                batch_id TEXT REFERENCES hitl_batches(id) ON DELETE SET NULL,
                interaction_id TEXT,
                status TEXT DEFAULT 'active',
                created_at TEXT,
                updated_at TEXT,
                completed_at TEXT,
                UNIQUE(adapter, tenant_key, external_message_id)
            );
            CREATE INDEX IF NOT EXISTS ix_bridge_hitl_messages_run ON bridge_hitl_messages(run_id);
            CREATE INDEX IF NOT EXISTS ix_bridge_hitl_messages_batch ON bridge_hitl_messages(batch_id);
            CREATE INDEX IF NOT EXISTS ix_bridge_hitl_messages_chat_status ON bridge_hitl_messages(adapter, tenant_key, external_chat_id, status);

            CREATE TABLE IF NOT EXISTS workflow_definitions (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT,
                status TEXT DEFAULT 'active',
                definition_version INTEGER DEFAULT 1,
                schema_version TEXT DEFAULT 'starweaver-claw.workflow.v1',
                owner_kind TEXT DEFAULT 'api',
                owner_session_id TEXT,
                owner_run_id TEXT,
                scope TEXT DEFAULT 'global',
                tags TEXT NOT NULL DEFAULT '[]',
                when_to_use TEXT,
                argument_hint TEXT,
                input_schema TEXT NOT NULL DEFAULT '{}',
                definition TEXT NOT NULL DEFAULT '{}',
                metadata TEXT NOT NULL DEFAULT '{}',
                created_at TEXT,
                updated_at TEXT,
                archived_at TEXT
            );
            CREATE INDEX IF NOT EXISTS ix_workflow_definitions_status_updated ON workflow_definitions(status, updated_at);
            CREATE INDEX IF NOT EXISTS ix_workflow_definitions_owner_session ON workflow_definitions(owner_session_id);
            CREATE INDEX IF NOT EXISTS ix_workflow_definitions_scope_status ON workflow_definitions(scope, status);

            CREATE TABLE IF NOT EXISTS workflow_runs (
                id TEXT PRIMARY KEY,
                workflow_id TEXT NOT NULL REFERENCES workflow_definitions(id) ON DELETE CASCADE,
                workflow_version INTEGER NOT NULL,
                definition_snapshot TEXT NOT NULL DEFAULT '{}',
                status TEXT DEFAULT 'queued',
                trigger_kind TEXT DEFAULT 'api',
                supervisor_session_id TEXT,
                supervisor_run_id TEXT,
                profile_name TEXT,
                workspace TEXT,
                inputs TEXT NOT NULL DEFAULT '{}',
                result TEXT,
                error_message TEXT,
                current_node_ids TEXT NOT NULL DEFAULT '[]',
                metadata TEXT NOT NULL DEFAULT '{}',
                created_at TEXT,
                started_at TEXT,
                finished_at TEXT,
                updated_at TEXT
            );
            CREATE INDEX IF NOT EXISTS ix_workflow_runs_workflow_created ON workflow_runs(workflow_id, created_at);
            CREATE INDEX IF NOT EXISTS ix_workflow_runs_status_updated ON workflow_runs(status, updated_at);
            CREATE INDEX IF NOT EXISTS ix_workflow_runs_supervisor_session ON workflow_runs(supervisor_session_id);

            CREATE TABLE IF NOT EXISTS workflow_node_runs (
                id TEXT PRIMARY KEY,
                workflow_run_id TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
                node_id TEXT NOT NULL,
                attempt_no INTEGER DEFAULT 1,
                status TEXT DEFAULT 'pending',
                profile_name TEXT,
                session_id TEXT,
                run_id TEXT,
                input_parts TEXT NOT NULL DEFAULT '[]',
                output_text TEXT,
                output_json TEXT,
                error_message TEXT,
                needs TEXT NOT NULL DEFAULT '[]',
                metadata TEXT NOT NULL DEFAULT '{}',
                started_at TEXT,
                finished_at TEXT,
                updated_at TEXT
            );
            CREATE INDEX IF NOT EXISTS ix_workflow_node_runs_workflow_node ON workflow_node_runs(workflow_run_id, node_id);
            CREATE INDEX IF NOT EXISTS ix_workflow_node_runs_run ON workflow_node_runs(run_id);

            CREATE TABLE IF NOT EXISTS workflow_events (
                id TEXT PRIMARY KEY,
                workflow_run_id TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
                node_run_id TEXT,
                source_kind TEXT DEFAULT 'workflow',
                event_type TEXT NOT NULL,
                payload TEXT NOT NULL DEFAULT '{}',
                created_at TEXT
            );
            CREATE INDEX IF NOT EXISTS ix_workflow_events_run_created ON workflow_events(workflow_run_id, created_at);
            CREATE INDEX IF NOT EXISTS ix_workflow_events_node ON workflow_events(node_run_id);
        ",
    },
    SqliteMigration {
        id: "20260605_000003_align_workflow_indexes",
        description: "align workflow indexes with the latest Starweaver Claw backend schema",
        sql: r"
            CREATE INDEX IF NOT EXISTS ix_workflow_definitions_status_updated ON workflow_definitions(status, updated_at);
            CREATE INDEX IF NOT EXISTS ix_workflow_definitions_owner_session ON workflow_definitions(owner_session_id);
            CREATE INDEX IF NOT EXISTS ix_workflow_definitions_scope_status ON workflow_definitions(scope, status);
            CREATE INDEX IF NOT EXISTS ix_workflow_runs_status_updated ON workflow_runs(status, updated_at);
            CREATE INDEX IF NOT EXISTS ix_workflow_runs_supervisor_session ON workflow_runs(supervisor_session_id);
            CREATE INDEX IF NOT EXISTS ix_workflow_node_runs_workflow_node ON workflow_node_runs(workflow_run_id, node_id);
            CREATE INDEX IF NOT EXISTS ix_workflow_node_runs_run ON workflow_node_runs(run_id);
            CREATE INDEX IF NOT EXISTS ix_workflow_events_node ON workflow_events(node_run_id);
        ",
    },
];

/// Run all pending SQLite schema migrations for a database file.
///
/// # Errors
///
/// Returns a store error when SQLite cannot open the database or apply a migration.
pub fn migrate_sqlite_database(path: impl AsRef<Path>) -> SessionStoreResult<Vec<&'static str>> {
    let mut connection = Connection::open(path).map_err(sql_error)?;
    apply_sqlite_migrations(&mut connection)
}

fn apply_sqlite_migrations(connection: &mut Connection) -> SessionStoreResult<Vec<&'static str>> {
    connection
        .execute_batch(
            r"
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;
            ",
        )
        .map_err(sql_error)?;
    connection
        .execute(
            &format!(
                "CREATE TABLE IF NOT EXISTS {SQLITE_SCHEMA_MIGRATION_TABLE} (
                    id TEXT PRIMARY KEY,
                    description TEXT NOT NULL,
                    applied_at TEXT NOT NULL
                )"
            ),
            [],
        )
        .map_err(sql_error)?;
    let applied = load_applied_migrations(connection)?;
    let transaction = connection.transaction().map_err(sql_error)?;
    let mut newly_applied = Vec::new();
    for migration in SQLITE_MIGRATIONS {
        if applied.contains(migration.id) {
            continue;
        }
        if migration.id == "20260605_000002_latest_backend_schema" {
            preserve_legacy_store_tables(&transaction)?;
        }
        transaction
            .execute_batch(migration.sql)
            .map_err(sql_error)?;
        if migration.id == "20260605_000002_latest_backend_schema" {
            import_latest_backend_records(&transaction)?;
        }
        transaction
            .execute(
                &format!(
                    "INSERT INTO {SQLITE_SCHEMA_MIGRATION_TABLE} (id, description, applied_at)
                     VALUES (?1, ?2, ?3)"
                ),
                params![migration.id, migration.description, Utc::now().to_rfc3339()],
            )
            .map_err(sql_error)?;
        newly_applied.push(migration.id);
    }
    transaction.commit().map_err(sql_error)?;
    Ok(newly_applied)
}

fn load_applied_migrations(connection: &Connection) -> SessionStoreResult<BTreeSet<String>> {
    let mut statement = connection
        .prepare(&format!("SELECT id FROM {SQLITE_SCHEMA_MIGRATION_TABLE}"))
        .map_err(sql_error)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(sql_error)?;
    let mut applied = BTreeSet::new();
    for row in rows {
        applied.insert(row.map_err(sql_error)?);
    }
    Ok(applied)
}

fn preserve_legacy_store_tables(connection: &Connection) -> SessionStoreResult<()> {
    if has_legacy_session_record_table(connection)? {
        connection
            .execute("ALTER TABLE sessions RENAME TO legacy_session_records", [])
            .map_err(sql_error)?;
    }
    if has_legacy_run_record_table(connection)? {
        connection
            .execute("ALTER TABLE runs RENAME TO legacy_run_records", [])
            .map_err(sql_error)?;
    }
    Ok(())
}

fn import_latest_backend_records(connection: &Connection) -> SessionStoreResult<()> {
    import_legacy_store_records(connection)?;
    if has_backend_session_table(connection)? && has_backend_run_table(connection)? {
        import_backend_sessions(connection)?;
        import_backend_runs(connection)?;
    }
    Ok(())
}

fn import_legacy_store_records(connection: &Connection) -> SessionStoreResult<()> {
    if table_exists(connection, "legacy_session_records")? {
        connection
            .execute(
                "INSERT OR IGNORE INTO session_records (session_id, record, created_at, updated_at)
                 SELECT session_id, record, created_at, updated_at FROM legacy_session_records",
                [],
            )
            .map_err(sql_error)?;
    }
    if table_exists(connection, "legacy_run_records")? {
        connection
            .execute(
                "INSERT OR IGNORE INTO run_records
                 (session_id, run_id, record, sequence_no, created_at, updated_at)
                 SELECT session_id, run_id, record, sequence_no, created_at, updated_at FROM legacy_run_records",
                [],
            )
            .map_err(sql_error)?;
    }
    Ok(())
}

fn import_backend_sessions(connection: &Connection) -> SessionStoreResult<()> {
    let mut statement = connection
        .prepare(
            "SELECT id, parent_session_id, profile_name, session_type, source_session_id, metadata,
                    head_run_id, head_success_run_id, active_run_id, created_at, updated_at
             FROM sessions",
        )
        .map_err(sql_error)?;
    let rows = statement
        .query_map([], |row| {
            let session_id = row.get::<_, String>(0)?;
            let parent_session_id = row.get::<_, Option<String>>(1)?;
            let profile = row.get::<_, Option<String>>(2)?;
            let session_type = row.get::<_, Option<String>>(3)?;
            let source_session_id = row.get::<_, Option<String>>(4)?;
            let metadata = row.get::<_, Option<String>>(5)?;
            let head_run_id = row.get::<_, Option<String>>(6)?;
            let head_success_run_id = row.get::<_, Option<String>>(7)?;
            let active_run_id = row.get::<_, Option<String>>(8)?;
            let created_at = row.get::<_, Option<String>>(9)?;
            let updated_at = row.get::<_, Option<String>>(10)?;
            Ok((
                session_id,
                parent_session_id,
                profile,
                session_type,
                source_session_id,
                metadata,
                head_run_id,
                head_success_run_id,
                active_run_id,
                created_at,
                updated_at,
            ))
        })
        .map_err(sql_error)?;
    for row in rows {
        let (
            session_id,
            parent_session_id,
            profile,
            session_type,
            source_session_id,
            metadata,
            head_run_id,
            head_success_run_id,
            active_run_id,
            created_at,
            updated_at,
        ) = row.map_err(sql_error)?;
        let created_at = parse_optional_datetime(created_at.as_deref()).unwrap_or_else(Utc::now);
        let updated_at = parse_optional_datetime(updated_at.as_deref()).unwrap_or(created_at);
        let mut metadata_value = parse_json_object(metadata.as_deref());
        insert_string_metadata(&mut metadata_value, "legacy_session_type", session_type);
        insert_string_metadata(
            &mut metadata_value,
            "legacy_source_session_id",
            source_session_id,
        );
        let record = json!({
            "session_id": session_id,
            "profile": profile,
            "status": "active",
            "state": default_resumable_state(),
            "parent_session_id": parent_session_id,
            "head_run_id": head_run_id,
            "head_success_run_id": head_success_run_id,
            "active_run_id": active_run_id,
            "created_at": created_at,
            "updated_at": updated_at,
            "metadata": metadata_value,
        });
        connection
            .execute(
                "INSERT OR IGNORE INTO session_records (session_id, record, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    session_id,
                    record.to_string(),
                    created_at.to_rfc3339(),
                    updated_at.to_rfc3339(),
                ],
            )
            .map_err(sql_error)?;
    }
    Ok(())
}

fn import_backend_runs(connection: &Connection) -> SessionStoreResult<()> {
    let mut statement = connection
        .prepare(
            "SELECT id, session_id, sequence_no, restore_from_run_id, status, trigger_type,
                    profile_name, input_parts, metadata, output_text, error_message,
                    termination_reason, created_at
             FROM runs",
        )
        .map_err(sql_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
            ))
        })
        .map_err(sql_error)?;
    for row in rows {
        let (
            run_id,
            session_id,
            sequence_no,
            restore_from_run_id,
            status,
            trigger_type,
            profile,
            input_parts,
            metadata,
            output_text,
            error_message,
            termination_reason,
            created_at,
        ) = row.map_err(sql_error)?;
        let created_at = parse_optional_datetime(created_at.as_deref()).unwrap_or_else(Utc::now);
        let updated_at = created_at;
        let mut metadata_value = parse_json_object(metadata.as_deref());
        insert_string_metadata(&mut metadata_value, "legacy_error_message", error_message);
        insert_string_metadata(
            &mut metadata_value,
            "legacy_termination_reason",
            termination_reason,
        );
        let sequence_no_usize = usize::try_from(sequence_no).unwrap_or(0);
        let record = json!({
            "session_id": session_id,
            "run_id": run_id,
            "conversation_id": format!("conv_{}", run_id),
            "input": parse_input_parts(input_parts.as_deref()),
            "status": map_backend_run_status(status.as_deref()),
            "output_preview": output_text,
            "structured_output": Value::Null,
            "sequence_no": sequence_no_usize,
            "restore_from_run_id": restore_from_run_id,
            "trigger_type": trigger_type,
            "profile": profile,
            "created_at": created_at,
            "updated_at": updated_at,
            "metadata": metadata_value,
        });
        connection
            .execute(
                "INSERT OR IGNORE INTO run_records
                 (session_id, run_id, record, sequence_no, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    session_id,
                    run_id,
                    record.to_string(),
                    sequence_no,
                    created_at.to_rfc3339(),
                    updated_at.to_rfc3339(),
                ],
            )
            .map_err(sql_error)?;
    }
    Ok(())
}

fn has_backend_session_table(connection: &Connection) -> SessionStoreResult<bool> {
    Ok(table_exists(connection, "sessions")? && table_has_column(connection, "sessions", "id")?)
}

fn has_backend_run_table(connection: &Connection) -> SessionStoreResult<bool> {
    Ok(table_exists(connection, "runs")?
        && table_has_column(connection, "runs", "id")?
        && table_has_column(connection, "runs", "input_parts")?)
}

fn has_legacy_session_record_table(connection: &Connection) -> SessionStoreResult<bool> {
    Ok(table_exists(connection, "sessions")?
        && table_has_column(connection, "sessions", "session_id")?
        && table_has_column(connection, "sessions", "record")?)
}

fn has_legacy_run_record_table(connection: &Connection) -> SessionStoreResult<bool> {
    Ok(table_exists(connection, "runs")?
        && table_has_column(connection, "runs", "run_id")?
        && table_has_column(connection, "runs", "record")?)
}

fn table_exists(connection: &Connection, table: &str) -> SessionStoreResult<bool> {
    let count = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![table],
            |row| row.get::<_, i64>(0),
        )
        .map_err(sql_error)?;
    Ok(count > 0)
}

fn table_has_column(
    connection: &Connection,
    table: &str,
    column: &str,
) -> SessionStoreResult<bool> {
    let pragma = format!("PRAGMA table_info({table})");
    let mut statement = connection.prepare(&pragma).map_err(sql_error)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(sql_error)?;
    for row in rows {
        if row.map_err(sql_error)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn default_resumable_state() -> Value {
    json!({ "agent_id": "main" })
}

fn parse_input_parts(payload: Option<&str>) -> Value {
    let Some(payload) = payload else {
        return Value::Array(Vec::new());
    };
    let Ok(value) = serde_json::from_str::<Value>(payload) else {
        return Value::Array(Vec::new());
    };
    let Some(parts) = value.as_array() else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        parts
            .iter()
            .map(|part| {
                let Some(text) = part.get("text").and_then(Value::as_str) else {
                    return part.clone();
                };
                json!({ "kind": "text", "text": text })
            })
            .collect(),
    )
}

fn parse_json_object(payload: Option<&str>) -> Value {
    payload
        .and_then(|payload| serde_json::from_str::<Value>(payload).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

fn insert_string_metadata(metadata: &mut Value, key: &str, value: Option<String>) {
    if let (Some(object), Some(value)) = (metadata.as_object_mut(), value) {
        object.insert(key.to_string(), Value::String(value));
    }
}

fn parse_optional_datetime(value: Option<&str>) -> Option<chrono::DateTime<Utc>> {
    value
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn map_backend_run_status(status: Option<&str>) -> &'static str {
    match status {
        Some("running") => "running",
        Some("completed") => "completed",
        Some("failed") => "failed",
        Some("cancelled") => "cancelled",
        _ => "queued",
    }
}

/// SQLite-backed durable session store.
#[derive(Clone, Debug)]
pub struct SqliteSessionStore {
    connection: Arc<Mutex<Connection>>,
}

impl SqliteSessionStore {
    /// Open or create a SQLite session store.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite cannot open or initialize the database.
    pub fn open(path: impl AsRef<Path>) -> SessionStoreResult<Self> {
        let mut connection = Connection::open(path).map_err(sql_error)?;
        apply_sqlite_migrations(&mut connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    /// Open an in-memory SQLite session store.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite cannot initialize the database.
    pub fn in_memory() -> SessionStoreResult<Self> {
        let mut connection = Connection::open_in_memory().map_err(sql_error)?;
        apply_sqlite_migrations(&mut connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    fn lock(&self) -> SessionStoreResult<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|error| SessionStoreError::Failed(error.to_string()))
    }
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn save_session(&self, mut session: SessionRecord) -> SessionStoreResult<()> {
        session.updated_at = Utc::now();
        let connection = self.lock()?;
        save_session_record(&connection, &session)
    }

    async fn load_session(&self, session_id: &SessionId) -> SessionStoreResult<SessionRecord> {
        let connection = self.lock()?;
        load_session_record(&connection, session_id)
    }

    async fn list_sessions(&self, filter: SessionFilter) -> SessionStoreResult<Vec<SessionRecord>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare("SELECT record FROM session_records ORDER BY updated_at DESC")
            .map_err(sql_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(sql_error)?;
        let mut sessions = Vec::new();
        for row in rows {
            let session = deserialize::<SessionRecord>(&row.map_err(sql_error)?)?;
            if filter.status.is_some_and(|status| session.status != status) {
                continue;
            }
            if filter
                .profile
                .as_ref()
                .is_some_and(|profile| session.profile.as_ref() != Some(profile))
            {
                continue;
            }
            if filter
                .workspace
                .as_ref()
                .is_some_and(|workspace| session.workspace.as_ref() != Some(workspace))
            {
                continue;
            }
            sessions.push(session);
            if filter.limit.is_some_and(|limit| sessions.len() >= limit) {
                break;
            }
        }
        Ok(sessions)
    }

    async fn update_session_status(
        &self,
        session_id: &SessionId,
        status: SessionStatus,
    ) -> SessionStoreResult<()> {
        let connection = self.lock()?;
        let mut session = load_session_record(&connection, session_id)?;
        session.status = status;
        session.updated_at = Utc::now();
        save_session_record(&connection, &session)
    }

    async fn save_context_state(
        &self,
        session_id: &SessionId,
        state: ResumableState,
    ) -> SessionStoreResult<()> {
        let connection = self.lock()?;
        let mut session = load_session_record(&connection, session_id)?;
        session.state = state;
        session.updated_at = Utc::now();
        save_session_record(&connection, &session)
    }

    async fn save_environment_state(
        &self,
        session_id: &SessionId,
        environment_state: EnvironmentStateRef,
    ) -> SessionStoreResult<()> {
        let connection = self.lock()?;
        let mut session = load_session_record(&connection, session_id)?;
        session.environment_state = Some(environment_state);
        session.updated_at = Utc::now();
        save_session_record(&connection, &session)
    }

    async fn append_run(&self, mut run: RunRecord) -> SessionStoreResult<()> {
        run.updated_at = Utc::now();
        let connection = self.lock()?;
        let mut session = load_session_record(&connection, &run.session_id)?;
        save_run_record(&connection, &run)?;
        apply_run_to_session(&mut session, &run);
        save_session_record(&connection, &session)
    }

    async fn load_run(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<RunRecord> {
        let connection = self.lock()?;
        load_run_record(&connection, session_id, run_id)
    }

    async fn list_runs(&self, session_id: &SessionId) -> SessionStoreResult<Vec<RunRecord>> {
        let connection = self.lock()?;
        list_run_records(&connection, session_id)
    }

    async fn update_run_status(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        status: RunStatus,
        output_preview: Option<String>,
    ) -> SessionStoreResult<()> {
        let connection = self.lock()?;
        let mut run = load_run_record(&connection, session_id, run_id)?;
        run.status = status;
        run.output_preview = output_preview;
        run.updated_at = Utc::now();
        save_run_record(&connection, &run)?;
        let mut session = load_session_record(&connection, session_id)?;
        apply_run_to_session(&mut session, &run);
        save_session_record(&connection, &session)
    }

    async fn append_checkpoint(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()> {
        let connection = self.lock()?;
        let key = run_key_label(session_id, &checkpoint.run_id);
        let mut run = load_run_record(&connection, session_id, &checkpoint.run_id)
            .map_err(|_| SessionStoreError::NotFound(key.clone()))?;
        let created_at = Utc::now();
        let payload = serialize(&checkpoint)?;
        connection
            .execute(
                "INSERT OR REPLACE INTO checkpoints
                 (session_id, run_id, sequence_no, checkpoint_id, record, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    session_id.as_str(),
                    checkpoint.run_id.as_str(),
                    i64::try_from(checkpoint.run_step).map_err(int_error)?,
                    checkpoint.checkpoint_id.as_str(),
                    payload,
                    created_at.to_rfc3339(),
                ],
            )
            .map_err(sql_error)?;
        run.latest_checkpoint = Some(starweaver_session::CheckpointRef {
            checkpoint_id: checkpoint.checkpoint_id,
            run_id: checkpoint.run_id,
            sequence: checkpoint.run_step,
            node: format!("{:?}", checkpoint.node),
            storage_ref: None,
            stream_cursor: checkpoint.resume.cursor.stream_cursor,
            created_at,
            metadata: checkpoint.metadata,
        });
        run.updated_at = created_at;
        save_run_record(&connection, &run)
    }

    async fn load_checkpoints(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentCheckpoint>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT record FROM checkpoints
                 WHERE session_id = ?1 AND run_id = ?2
                 ORDER BY sequence_no ASC, checkpoint_id ASC",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map(params![session_id.as_str(), run_id.as_str()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(sql_error)?;
        collect_json_rows(rows)
    }

    async fn append_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        records: Vec<AgentStreamRecord>,
    ) -> SessionStoreResult<()> {
        let connection = self.lock()?;
        let mut run = load_run_record(&connection, session_id, run_id)?;
        for record in records {
            connection
                .execute(
                    "INSERT OR REPLACE INTO stream_records
                     (session_id, run_id, sequence_no, record)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        session_id.as_str(),
                        run_id.as_str(),
                        i64::try_from(record.sequence).map_err(int_error)?,
                        serialize(&record)?,
                    ],
                )
                .map_err(sql_error)?;
        }
        let latest_sequence = latest_stream_sequence(&connection, session_id, run_id)?;
        if let Some(sequence) = latest_sequence {
            let cursor =
                StreamCursorRef::new("raw_runtime", format!("run:{}", run_id.as_str()), sequence);
            run.stream_cursors
                .retain(|existing| existing.family != cursor.family);
            run.stream_cursors.push(cursor.clone());
            run.updated_at = Utc::now();
            save_run_record(&connection, &run)?;
            let mut session = load_session_record(&connection, session_id)?;
            session.stream_cursors.retain(|existing| {
                existing.family != cursor.family || existing.scope != cursor.scope
            });
            session.stream_cursors.push(cursor);
            session.updated_at = run.updated_at;
            save_session_record(&connection, &session)?;
        }
        Ok(())
    }

    async fn replay_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentStreamRecord>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT record FROM stream_records
                 WHERE session_id = ?1 AND run_id = ?2
                 ORDER BY sequence_no ASC",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map(params![session_id.as_str(), run_id.as_str()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(sql_error)?;
        collect_json_rows(rows)
    }

    async fn save_stream_cursor(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        cursor: StreamCursorRef,
    ) -> SessionStoreResult<()> {
        let connection = self.lock()?;
        let mut run = load_run_record(&connection, session_id, run_id)?;
        run.stream_cursors
            .retain(|existing| existing.family != cursor.family || existing.scope != cursor.scope);
        run.stream_cursors.push(cursor.clone());
        run.updated_at = Utc::now();
        save_run_record(&connection, &run)?;

        let mut session = load_session_record(&connection, session_id)?;
        session
            .stream_cursors
            .retain(|existing| existing.family != cursor.family || existing.scope != cursor.scope);
        session.stream_cursors.push(cursor);
        session.updated_at = run.updated_at;
        save_session_record(&connection, &session)
    }

    async fn append_approval(&self, approval: ApprovalRecord) -> SessionStoreResult<()> {
        let connection = self.lock()?;
        let _run = load_run_record(&connection, &approval.session_id, &approval.run_id)?;
        connection
            .execute(
                "INSERT OR REPLACE INTO approvals
                 (session_id, run_id, approval_id, record, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    approval.session_id.as_str(),
                    approval.run_id.as_str(),
                    approval.approval_id,
                    serialize(&approval)?,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    async fn load_approvals(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<ApprovalRecord>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT record FROM approvals
                 WHERE session_id = ?1 AND run_id = ?2
                 ORDER BY updated_at ASC, approval_id ASC",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map(params![session_id.as_str(), run_id.as_str()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(sql_error)?;
        collect_json_rows(rows)
    }

    async fn append_deferred_tool(&self, record: DeferredToolRecord) -> SessionStoreResult<()> {
        let connection = self.lock()?;
        let _run = load_run_record(&connection, &record.session_id, &record.run_id)?;
        connection
            .execute(
                "INSERT OR REPLACE INTO deferred_tools
                 (session_id, run_id, deferred_id, record, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    record.session_id.as_str(),
                    record.run_id.as_str(),
                    record.deferred_id,
                    serialize(&record)?,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    async fn load_deferred_tools(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<DeferredToolRecord>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT record FROM deferred_tools
                 WHERE session_id = ?1 AND run_id = ?2
                 ORDER BY updated_at ASC, deferred_id ASC",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map(params![session_id.as_str(), run_id.as_str()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(sql_error)?;
        collect_json_rows(rows)
    }

    async fn resume_snapshot(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<SessionResumeSnapshot> {
        let session = self.load_session(session_id).await?;
        let run = self.load_run(session_id, run_id).await?;
        let latest_checkpoint = self.latest_checkpoint(session_id, run_id).await?;
        let after_sequence = latest_checkpoint
            .as_ref()
            .and_then(|checkpoint| checkpoint.resume.cursor.stream_cursor);
        let stream_records = self
            .replay_stream_records_after(session_id, run_id, after_sequence)
            .await?;
        let approvals = self.load_approvals(session_id, run_id).await?;
        let deferred_tools = self.load_deferred_tools(session_id, run_id).await?;
        let environment_state = run
            .environment_state
            .clone()
            .or_else(|| session.environment_state.clone());
        let mut stream_cursors = session.stream_cursors.clone();
        stream_cursors.extend(run.stream_cursors.clone());
        Ok(SessionResumeSnapshot {
            state: session.state.clone(),
            session,
            run,
            environment_state,
            latest_checkpoint,
            stream_records,
            approvals,
            deferred_tools,
            stream_cursors,
        })
    }

    async fn compact_run_trace(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<CompactRunTrace> {
        let connection = self.lock()?;
        let run = load_run_record(&connection, session_id, run_id)?;
        let checkpoints = load_checkpoint_ids(&connection, session_id, run_id)?;
        let stream_cursor = latest_stream_sequence(&connection, session_id, run_id)?;
        let approvals = count_pending_approvals(&connection, session_id, run_id)?;
        let deferred_tools = count_deferred_tools(&connection, session_id, run_id)?;
        Ok(CompactRunTrace {
            session_id: Some(session_id.clone()),
            run_id: Some(run_id.clone()),
            status: run.status,
            checkpoints: checkpoints.clone(),
            approvals,
            deferred_tools,
            latest_checkpoint: checkpoints.last().cloned(),
            stream_cursor,
            stream_cursors: run.stream_cursors.clone(),
            output_preview: run.output_preview.clone(),
            trace_context: run.trace_context.clone(),
            updated_at: Some(run.updated_at),
            metadata: run.metadata.clone(),
        })
    }

    async fn compact_session_trace(
        &self,
        session_id: &SessionId,
    ) -> SessionStoreResult<CompactSessionTrace> {
        let connection = self.lock()?;
        let session = load_session_record(&connection, session_id)?;
        let runs = list_run_records(&connection, session_id)?;
        let latest_run = runs.last();
        Ok(CompactSessionTrace {
            session_id: session.session_id.clone(),
            title: session.title.clone(),
            workspace: session.workspace.clone(),
            profile: session.profile.clone(),
            status: session.status,
            runs: runs.len(),
            latest_run_id: latest_run.map(|run| run.run_id.clone()),
            last_output_preview: latest_run.and_then(|run| run.output_preview.clone()),
            stream_cursors: session.stream_cursors.clone(),
            trace_context: session.trace_context.clone(),
            created_at: session.created_at,
            updated_at: session.updated_at,
            metadata: session.metadata.clone(),
        })
    }
}

fn save_session_record(connection: &Connection, session: &SessionRecord) -> SessionStoreResult<()> {
    connection
        .execute(
            "INSERT OR REPLACE INTO session_records (session_id, record, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                session.session_id.as_str(),
                serialize(session)?,
                session.created_at.to_rfc3339(),
                session.updated_at.to_rfc3339(),
            ],
        )
        .map_err(sql_error)?;
    Ok(())
}

fn load_session_record(
    connection: &Connection,
    session_id: &SessionId,
) -> SessionStoreResult<SessionRecord> {
    let payload = connection
        .query_row(
            "SELECT record FROM session_records WHERE session_id = ?1",
            params![session_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sql_error)?
        .ok_or_else(|| SessionStoreError::NotFound(session_id.as_str().to_string()))?;
    deserialize(&payload)
}

fn save_run_record(connection: &Connection, run: &RunRecord) -> SessionStoreResult<()> {
    connection
        .execute(
            "INSERT OR REPLACE INTO run_records
             (session_id, run_id, record, sequence_no, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                run.session_id.as_str(),
                run.run_id.as_str(),
                serialize(run)?,
                i64::try_from(run.sequence_no).map_err(int_error)?,
                run.created_at.to_rfc3339(),
                run.updated_at.to_rfc3339(),
            ],
        )
        .map_err(sql_error)?;
    Ok(())
}

fn load_run_record(
    connection: &Connection,
    session_id: &SessionId,
    run_id: &RunId,
) -> SessionStoreResult<RunRecord> {
    let payload = connection
        .query_row(
            "SELECT record FROM run_records WHERE session_id = ?1 AND run_id = ?2",
            params![session_id.as_str(), run_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sql_error)?
        .ok_or_else(|| SessionStoreError::NotFound(run_key_label(session_id, run_id)))?;
    deserialize(&payload)
}

fn list_run_records(
    connection: &Connection,
    session_id: &SessionId,
) -> SessionStoreResult<Vec<RunRecord>> {
    let mut statement = connection
        .prepare("SELECT record FROM run_records WHERE session_id = ?1 ORDER BY sequence_no ASC")
        .map_err(sql_error)?;
    let rows = statement
        .query_map(params![session_id.as_str()], |row| row.get::<_, String>(0))
        .map_err(sql_error)?;
    collect_json_rows(rows)
}

fn apply_run_to_session(session: &mut SessionRecord, run: &RunRecord) {
    session.head_run_id = Some(run.run_id.clone());
    match run.status {
        RunStatus::Queued | RunStatus::Running | RunStatus::Waiting => {
            session.active_run_id = Some(run.run_id.clone());
        }
        RunStatus::Completed => {
            session.head_success_run_id = Some(run.run_id.clone());
            if session.active_run_id.as_ref() == Some(&run.run_id) {
                session.active_run_id = None;
            }
        }
        RunStatus::Failed | RunStatus::Cancelled => {
            if session.active_run_id.as_ref() == Some(&run.run_id) {
                session.active_run_id = None;
            }
        }
    }
    session.updated_at = run.updated_at;
}

fn latest_stream_sequence(
    connection: &Connection,
    session_id: &SessionId,
    run_id: &RunId,
) -> SessionStoreResult<Option<usize>> {
    let value = connection
        .query_row(
            "SELECT MAX(sequence_no) FROM stream_records WHERE session_id = ?1 AND run_id = ?2",
            params![session_id.as_str(), run_id.as_str()],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(sql_error)?;
    value
        .map(|sequence| usize::try_from(sequence).map_err(int_error))
        .transpose()
}

fn load_checkpoint_ids(
    connection: &Connection,
    session_id: &SessionId,
    run_id: &RunId,
) -> SessionStoreResult<Vec<starweaver_core::CheckpointId>> {
    let mut statement = connection
        .prepare(
            "SELECT record FROM checkpoints
             WHERE session_id = ?1 AND run_id = ?2
             ORDER BY sequence_no ASC, checkpoint_id ASC",
        )
        .map_err(sql_error)?;
    let rows = statement
        .query_map(params![session_id.as_str(), run_id.as_str()], |row| {
            row.get::<_, String>(0)
        })
        .map_err(sql_error)?;
    let checkpoints = collect_json_rows::<AgentCheckpoint>(rows)?;
    Ok(checkpoints
        .into_iter()
        .map(|checkpoint| checkpoint.checkpoint_id)
        .collect())
}

fn count_pending_approvals(
    connection: &Connection,
    session_id: &SessionId,
    run_id: &RunId,
) -> SessionStoreResult<usize> {
    let approvals = {
        let mut statement = connection
            .prepare("SELECT record FROM approvals WHERE session_id = ?1 AND run_id = ?2")
            .map_err(sql_error)?;
        let rows = statement
            .query_map(params![session_id.as_str(), run_id.as_str()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(sql_error)?;
        collect_json_rows::<ApprovalRecord>(rows)?
    };
    Ok(approvals
        .iter()
        .filter(|approval| approval.status == ApprovalStatus::Pending)
        .count())
}

fn count_deferred_tools(
    connection: &Connection,
    session_id: &SessionId,
    run_id: &RunId,
) -> SessionStoreResult<usize> {
    let count = connection
        .query_row(
            "SELECT COUNT(*) FROM deferred_tools WHERE session_id = ?1 AND run_id = ?2",
            params![session_id.as_str(), run_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(sql_error)?;
    usize::try_from(count).map_err(int_error)
}

fn collect_json_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<String>>,
) -> SessionStoreResult<Vec<T>>
where
    T: serde::de::DeserializeOwned,
{
    let mut values = Vec::new();
    for row in rows {
        values.push(deserialize(&row.map_err(sql_error)?)?);
    }
    Ok(values)
}

fn serialize<T>(value: &T) -> SessionStoreResult<String>
where
    T: serde::Serialize,
{
    serde_json::to_string(value).map_err(|error| SessionStoreError::Failed(error.to_string()))
}

fn deserialize<T>(payload: &str) -> SessionStoreResult<T>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str(payload).map_err(|error| SessionStoreError::Failed(error.to_string()))
}

fn sql_error(error: rusqlite::Error) -> SessionStoreError {
    SessionStoreError::Failed(error.to_string())
}

fn int_error(error: impl std::fmt::Display) -> SessionStoreError {
    SessionStoreError::Failed(error.to_string())
}

fn run_key_label(session_id: &SessionId, run_id: &RunId) -> String {
    format!("{}:{}", session_id.as_str(), run_id.as_str())
}

#[cfg(test)]
mod tests {
    use starweaver_core::ConversationId;

    use super::*;

    #[test]
    fn sqlite_migrations_are_idempotent() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let database_path = tempdir.path().join("claw.sqlite3");
        let first = migrate_sqlite_database(&database_path).expect("first migration");
        assert_eq!(
            first,
            vec![
                "20260605_000001_session_stream_store",
                "20260605_000002_latest_backend_schema",
                "20260605_000003_align_workflow_indexes",
            ]
        );
        let second = migrate_sqlite_database(&database_path).expect("second migration");
        assert!(second.is_empty());

        let connection = Connection::open(database_path).expect("open migrated database");
        assert_table_exists(&connection, "session_records");
        for table in LATEST_BACKEND_TABLES {
            assert_table_exists(&connection, table);
        }
        for index in LATEST_BACKEND_INDEXES {
            assert_index_exists(&connection, index);
        }
    }

    const LATEST_BACKEND_TABLES: &[&str] = &[
        "profiles",
        "sessions",
        "runs",
        "bridge_conversations",
        "bridge_events",
        "runtime_instances",
        "session_memory_states",
        "schedules",
        "schedule_fires",
        "heartbeat_fires",
        "agency_fires",
        "session_async_tasks",
        "hitl_batches",
        "hitl_interactions",
        "hitl_deferred_inputs",
        "bridge_hitl_messages",
        "workflow_definitions",
        "workflow_runs",
        "workflow_node_runs",
        "workflow_events",
    ];

    const LATEST_BACKEND_INDEXES: &[&str] = &[
        "ix_runs_session_id",
        "ix_runs_session_created_at",
        "ix_runs_session_sequence_no",
        "ix_bridge_conversations_session_id",
        "ix_bridge_events_chat_created_at",
        "ix_bridge_events_session_id",
        "ix_bridge_events_run_id",
        "ix_sessions_session_type_updated",
        "ix_sessions_source_session",
        "ix_sessions_type_source_unique",
        "ix_schedules_due",
        "ix_schedules_owner_session",
        "ix_schedules_target_session",
        "ix_schedules_source_session",
        "ix_schedules_trigger_kind",
        "ix_schedule_fires_schedule_created",
        "ix_schedule_fires_status_scheduled",
        "ix_schedule_fires_run",
        "ix_heartbeat_fires_status_scheduled",
        "ix_heartbeat_fires_run",
        "ix_agency_fires_status_scheduled",
        "ix_agency_fires_kind_created",
        "ix_agency_fires_run",
        "ix_agency_fires_source",
        "ix_session_async_tasks_parent_status",
        "ix_session_async_tasks_task_session",
        "ix_session_async_tasks_task_run",
        "ix_session_async_tasks_name",
        "ix_hitl_batches_run_status",
        "ix_hitl_batches_session_status",
        "ix_hitl_interactions_run_status",
        "ix_hitl_interactions_batch_sequence",
        "ix_hitl_deferred_inputs_batch_sequence",
        "ix_hitl_deferred_inputs_run_status",
        "ix_bridge_hitl_messages_run",
        "ix_bridge_hitl_messages_batch",
        "ix_bridge_hitl_messages_chat_status",
        "ix_workflow_definitions_status_updated",
        "ix_workflow_definitions_owner_session",
        "ix_workflow_definitions_scope_status",
        "ix_workflow_runs_workflow_created",
        "ix_workflow_runs_status_updated",
        "ix_workflow_runs_supervisor_session",
        "ix_workflow_node_runs_workflow_node",
        "ix_workflow_node_runs_run",
        "ix_workflow_events_run_created",
        "ix_workflow_events_node",
    ];

    fn assert_table_exists(connection: &Connection, table: &str) {
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                params![table],
                |row| row.get(0),
            )
            .expect("count table");
        assert_eq!(count, 1, "missing table {table}");
    }

    fn assert_index_exists(connection: &Connection, index: &str) {
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                params![index],
                |row| row.get(0),
            )
            .expect("count index");
        assert_eq!(count, 1, "missing index {index}");
    }

    #[tokio::test]
    async fn sqlite_migrations_import_latest_backend_sessions_and_runs() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let database_path = tempdir.path().join("python-claw.sqlite3");
        let connection = Connection::open(&database_path).expect("open seed database");
        connection
            .execute_batch(
                r#"
                CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    parent_session_id TEXT,
                    profile_name TEXT,
                    session_type TEXT,
                    source_session_id TEXT,
                    metadata TEXT NOT NULL,
                    head_run_id TEXT,
                    head_success_run_id TEXT,
                    active_run_id TEXT,
                    created_at TEXT,
                    updated_at TEXT
                );
                CREATE TABLE runs (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    sequence_no INTEGER NOT NULL,
                    restore_from_run_id TEXT,
                    status TEXT,
                    trigger_type TEXT,
                    profile_name TEXT,
                    input_parts TEXT NOT NULL,
                    metadata TEXT NOT NULL,
                    output_text TEXT,
                    error_message TEXT,
                    termination_reason TEXT,
                    created_at TEXT
                );
                INSERT INTO sessions (
                    id, profile_name, session_type, metadata, head_run_id,
                    head_success_run_id, created_at, updated_at
                ) VALUES (
                    'session_import', 'default', 'conversation', '{"topic":"import"}',
                    'run_import', 'run_import', '2026-06-05T00:00:00Z', '2026-06-05T00:01:00Z'
                );
                INSERT INTO runs (
                    id, session_id, sequence_no, status, trigger_type, profile_name,
                    input_parts, metadata, output_text, created_at
                ) VALUES (
                    'run_import', 'session_import', 7, 'completed', 'api', 'default',
                    '[{"type":"text","text":"hello"}]', '{"source":"python"}',
                    'done', '2026-06-05T00:00:30Z'
                );
                "#,
            )
            .expect("seed latest backend tables");
        drop(connection);

        migrate_sqlite_database(&database_path).expect("migrate latest backend database");
        let store = SqliteSessionStore::open(&database_path).expect("open migrated store");
        let session_id = SessionId::from_string("session_import");
        let session = store
            .load_session(&session_id)
            .await
            .expect("load imported session");
        assert_eq!(session.profile.as_deref(), Some("default"));
        assert_eq!(
            session.head_success_run_id.as_ref().map(RunId::as_str),
            Some("run_import")
        );
        let runs = store
            .list_runs(&session_id)
            .await
            .expect("list imported runs");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].sequence_no, 7);
        assert_eq!(runs[0].status, RunStatus::Completed);
        assert_eq!(runs[0].output_preview.as_deref(), Some("done"));
    }

    #[tokio::test]
    async fn sqlite_migrations_preserve_legacy_store_records() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let database_path = tempdir.path().join("legacy-store.sqlite3");
        let connection = Connection::open(&database_path).expect("open seed database");
        let session_id = SessionId::from_string("session_legacy");
        let run_id = RunId::from_string("run_legacy");
        let session = SessionRecord::new(session_id.clone());
        let mut run = RunRecord::new(session_id.clone(), run_id.clone(), ConversationId::new());
        run.sequence_no = 3;
        connection
            .execute_batch(
                r"
                CREATE TABLE sessions (
                    session_id TEXT PRIMARY KEY,
                    record TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                CREATE TABLE runs (
                    session_id TEXT NOT NULL,
                    run_id TEXT NOT NULL,
                    record TEXT NOT NULL,
                    sequence_no INTEGER NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY (session_id, run_id)
                );
                ",
            )
            .expect("create legacy store tables");
        connection
            .execute(
                "INSERT INTO sessions (session_id, record, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
                params![
                    session_id.as_str(),
                    serde_json::to_string(&session).expect("session json"),
                    session.created_at.to_rfc3339(),
                    session.updated_at.to_rfc3339(),
                ],
            )
            .expect("insert legacy session");
        connection
            .execute(
                "INSERT INTO runs (session_id, run_id, record, sequence_no, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    session_id.as_str(),
                    run_id.as_str(),
                    serde_json::to_string(&run).expect("run json"),
                    i64::try_from(run.sequence_no).expect("sequence fits"),
                    run.created_at.to_rfc3339(),
                    run.updated_at.to_rfc3339(),
                ],
            )
            .expect("insert legacy run");
        drop(connection);

        migrate_sqlite_database(&database_path).expect("migrate legacy store database");
        let store = SqliteSessionStore::open(&database_path).expect("open migrated store");
        let loaded = store
            .load_session(&session_id)
            .await
            .expect("load migrated legacy session");
        assert_eq!(loaded.session_id, session_id);
        let runs = store
            .list_runs(&session_id)
            .await
            .expect("list migrated legacy runs");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id, run_id);
    }

    #[tokio::test]
    async fn sqlite_store_round_trips_session_and_run() {
        let store = SqliteSessionStore::in_memory().expect("sqlite store");
        let session_id = SessionId::from_string("session_test");
        store
            .save_session(SessionRecord::new(session_id.clone()))
            .await
            .expect("save session");
        let run_id = RunId::from_string("run_test");
        let mut run = RunRecord::new(session_id.clone(), run_id.clone(), ConversationId::new());
        run.sequence_no = 1;
        store.append_run(run).await.expect("append run");

        let session = store.load_session(&session_id).await.expect("load session");
        assert_eq!(session.active_run_id.as_ref(), Some(&run_id));
        let runs = store.list_runs(&session_id).await.expect("list runs");
        assert_eq!(runs.len(), 1);
    }
}

/// SQLite-backed replay event log with in-process live subscriptions.
#[derive(Clone, Debug)]
pub struct SqliteReplayEventLog {
    connection: Arc<Mutex<Connection>>,
    live: InMemoryReplayEventLog,
}

impl SqliteReplayEventLog {
    /// Open or create a SQLite replay event log.
    ///
    /// # Errors
    ///
    /// Returns a replay error when SQLite cannot open or initialize the database.
    pub fn open(path: impl AsRef<Path>) -> ReplayResult<Self> {
        let mut connection = Connection::open(path).map_err(replay_sql_error)?;
        apply_sqlite_migrations(&mut connection).map_err(session_to_replay_error)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            live: InMemoryReplayEventLog::new(),
        })
    }

    /// Open an in-memory SQLite replay event log.
    ///
    /// # Errors
    ///
    /// Returns a replay error when SQLite cannot initialize the database.
    pub fn in_memory() -> ReplayResult<Self> {
        let mut connection = Connection::open_in_memory().map_err(replay_sql_error)?;
        apply_sqlite_migrations(&mut connection).map_err(session_to_replay_error)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            live: InMemoryReplayEventLog::new(),
        })
    }

    fn replay_lock(&self) -> ReplayResult<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|error| ReplayError::Failed(error.to_string()))
    }
}

#[async_trait]
impl ReplayEventLog for SqliteReplayEventLog {
    async fn append(&self, scope: ReplayScope, mut event: ReplayEvent) -> ReplayResult<()> {
        event.scope = scope.clone();
        {
            let connection = self.replay_lock()?;
            connection
                .execute(
                    "INSERT OR IGNORE INTO replay_events (scope, sequence_no, record, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        scope.as_str(),
                        i64::try_from(event.sequence)
                            .map_err(|error| ReplayError::Failed(error.to_string()))?,
                        serde_json::to_string(&event)
                            .map_err(|error| ReplayError::Failed(error.to_string()))?,
                        event.timestamp.to_rfc3339(),
                    ],
                )
                .map_err(replay_sql_error)?;
        }
        self.live.append(scope, event).await
    }

    async fn replay_after(
        &self,
        scope: &ReplayScope,
        cursor: Option<ReplayCursor>,
        limit: Option<usize>,
    ) -> ReplayResult<Vec<ReplayEvent>> {
        if let Some(cursor) = cursor.as_ref() {
            cursor.validate_scope(scope)?;
        }
        let after = cursor.map_or(0, |cursor| cursor.sequence.saturating_add(1));
        let limit = limit.unwrap_or(1000);
        let connection = self.replay_lock()?;
        let mut statement = connection
            .prepare(
                "SELECT record FROM replay_events
                 WHERE scope = ?1 AND sequence_no >= ?2
                 ORDER BY sequence_no ASC
                 LIMIT ?3",
            )
            .map_err(replay_sql_error)?;
        let rows = statement
            .query_map(
                params![
                    scope.as_str(),
                    i64::try_from(after).map_err(|error| ReplayError::Failed(error.to_string()))?,
                    i64::try_from(limit).map_err(|error| ReplayError::Failed(error.to_string()))?,
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(replay_sql_error)?;
        let mut events = Vec::new();
        for row in rows {
            let payload = row.map_err(replay_sql_error)?;
            events.push(
                serde_json::from_str::<ReplayEvent>(&payload)
                    .map_err(|error| ReplayError::Failed(error.to_string()))?,
            );
        }
        Ok(events)
    }

    async fn subscribe(
        &self,
        scope: ReplayScope,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<ReplaySubscription> {
        self.live.subscribe(scope, cursor).await
    }

    async fn compact_snapshot(&self, scope: &ReplayScope) -> ReplayResult<ReplaySnapshot> {
        let snapshot_payload = {
            let connection = self.replay_lock()?;
            connection
                .query_row(
                    "SELECT record FROM replay_snapshots WHERE scope = ?1",
                    params![scope.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(replay_sql_error)?
        };
        if let Some(payload) = snapshot_payload {
            return serde_json::from_str::<ReplaySnapshot>(&payload)
                .map_err(|error| ReplayError::Failed(error.to_string()));
        }
        let events = self.replay_after(scope, None, None).await?;
        let display_messages = events
            .iter()
            .filter_map(|event| match &event.event {
                ReplayEventKind::DisplayMessage(message) => Some((**message).clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let cursor = events
            .last()
            .map(|event| ReplayCursor::new(scope.clone(), event.sequence));
        Ok(ReplaySnapshot {
            scope: Some(scope.clone()),
            revision: events.len(),
            cursor,
            display_messages,
            metadata: Metadata::default(),
        })
    }
}

impl SqliteReplayEventLog {
    /// Persist a compact snapshot.
    ///
    /// # Errors
    ///
    /// Returns a replay error when SQLite write or JSON encoding fails.
    pub fn save_snapshot(&self, scope: ReplayScope, snapshot: ReplaySnapshot) -> ReplayResult<()> {
        let connection = self.replay_lock()?;
        connection
            .execute(
                "INSERT OR REPLACE INTO replay_snapshots (scope, record, updated_at)
                 VALUES (?1, ?2, ?3)",
                params![
                    scope.as_str(),
                    serde_json::to_string(&snapshot)
                        .map_err(|error| ReplayError::Failed(error.to_string()))?,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(replay_sql_error)?;
        Ok(())
    }
}

fn session_to_replay_error(error: SessionStoreError) -> ReplayError {
    ReplayError::Failed(error.to_string())
}

fn replay_sql_error(error: rusqlite::Error) -> ReplayError {
    ReplayError::Failed(error.to_string())
}

#[cfg(test)]
mod replay_tests {
    use super::*;

    #[tokio::test]
    async fn sqlite_replay_log_round_trips_events() {
        let log = SqliteReplayEventLog::in_memory().expect("replay log");
        let scope = ReplayScope::run("run_test");
        log.append(
            scope.clone(),
            ReplayEvent::new(scope.clone(), 1, ReplayEventKind::Heartbeat),
        )
        .await
        .expect("append event");
        let events = log.replay_after(&scope, None, None).await.expect("replay");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sequence, 1);
    }
}
