use super::{LocalStore, db::add_column_if_missing};
use crate::CliResult;

impl LocalStore {
    #[allow(clippy::too_many_lines)]
    pub(super) fn init_schema(&self) -> CliResult<()> {
        self.conn.execute_batch(
            r"
            CREATE TABLE IF NOT EXISTS sessions (
                session_id TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                profile TEXT,
                title TEXT,
                head_run_id TEXT,
                head_success_run_id TEXT,
                active_run_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                record_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS runs (
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                sequence_no INTEGER NOT NULL,
                status TEXT NOT NULL,
                restore_from_run_id TEXT,
                output_preview TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                record_json TEXT NOT NULL,
                PRIMARY KEY (session_id, run_id),
                UNIQUE (session_id, sequence_no),
                FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_runs_session_sequence ON runs(session_id, sequence_no);
            CREATE TABLE IF NOT EXISTS display_messages (
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                sequence_no INTEGER NOT NULL,
                kind TEXT NOT NULL,
                created_at TEXT NOT NULL,
                message_json TEXT NOT NULL,
                PRIMARY KEY (session_id, run_id, sequence_no),
                FOREIGN KEY (session_id, run_id) REFERENCES runs(session_id, run_id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS raw_stream_records (
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                sequence_no INTEGER NOT NULL,
                kind TEXT NOT NULL,
                created_at TEXT NOT NULL,
                record_json TEXT NOT NULL,
                PRIMARY KEY (session_id, run_id, sequence_no),
                FOREIGN KEY (session_id, run_id) REFERENCES runs(session_id, run_id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS file_refs (
                ref_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                relative_path TEXT NOT NULL,
                byte_size INTEGER NOT NULL,
                checksum TEXT,
                content_type TEXT,
                created_at TEXT NOT NULL,
                trimmed_at TEXT
            );
            CREATE TABLE IF NOT EXISTS context_states (
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                state_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (session_id, run_id),
                FOREIGN KEY (session_id, run_id) REFERENCES runs(session_id, run_id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS environment_states (
                ref_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                provider TEXT NOT NULL,
                state_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY (session_id, run_id) REFERENCES runs(session_id, run_id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS stream_cursors (
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                family TEXT NOT NULL,
                scope TEXT NOT NULL,
                sequence_no INTEGER NOT NULL,
                cursor_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (session_id, run_id, family, scope),
                FOREIGN KEY (session_id, run_id) REFERENCES runs(session_id, run_id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS checkpoints (
                checkpoint_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                sequence_no INTEGER NOT NULL,
                node TEXT NOT NULL,
                checkpoint_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY (session_id, run_id) REFERENCES runs(session_id, run_id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS approvals (
                approval_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                action_id TEXT NOT NULL,
                action_name TEXT NOT NULL,
                status TEXT NOT NULL,
                record_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY (session_id, run_id) REFERENCES runs(session_id, run_id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS deferred_tools (
                deferred_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                tool_call_id TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                status TEXT NOT NULL,
                record_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY (session_id, run_id) REFERENCES runs(session_id, run_id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS replay_snapshots (
                scope TEXT PRIMARY KEY,
                snapshot_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            ",
        )?;
        add_column_if_missing(&self.conn, "file_refs", "checksum", "TEXT")?;
        add_column_if_missing(&self.conn, "file_refs", "content_type", "TEXT")?;
        Ok(())
    }
}
