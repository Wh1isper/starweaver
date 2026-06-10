//! Local `SQLite` and file-store persistence for CLI sessions.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use serde_json::Value;
use starweaver_agent::ResumableState;
use starweaver_core::{CheckpointId, ConversationId, RunId, SessionId};
use starweaver_environment::EnvironmentState;
use starweaver_model::{
    ModelMessage, ModelRequest, ModelRequestPart, ModelResponsePart, ToolReturnPart,
};
use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord};
use starweaver_session::{
    ApprovalDecision, ApprovalRecord, ApprovalStatus, CheckpointRef, DeferredToolRecord,
    EnvironmentStateRef, ExecutionStatus, InputPart, RunRecord, RunStatus, SessionRecord,
    SessionStatus, StreamCursorRef,
};
use starweaver_stream::{DisplayMessage, ReplaySnapshot};
use uuid::Uuid;

use crate::{config::CliConfig, error::io_error, CliError, CliResult};

/// Local `SQLite` and file-store handle.
pub struct LocalStore {
    conn: Connection,
    file_store_path: PathBuf,
}

struct FileRefRecord {
    ref_id: String,
    relative_path: String,
    byte_size: i64,
    checksum: String,
    content_type: String,
    created_at: String,
}

/// Durable artifacts captured when a CLI run finishes or waits.
pub struct RunArtifacts {
    /// Final context state.
    pub state: ResumableState,
    /// Environment state snapshot.
    pub environment_state: Option<EnvironmentState>,
    /// Raw runtime records.
    pub raw_records: Vec<AgentStreamRecord>,
    /// Display messages.
    pub display_messages: Vec<DisplayMessage>,
    /// Compact display snapshot.
    pub display_snapshot: ReplaySnapshot,
    /// Approval records.
    pub approvals: Vec<ApprovalRecord>,
    /// Deferred tool records.
    pub deferred_tools: Vec<DeferredToolRecord>,
    /// Terminal status selected by HITL and runtime policy.
    pub status: RunStatus,
}

/// Session summary row.
#[derive(Clone, Debug, Serialize)]
pub struct SessionSummary {
    /// Session id.
    pub session_id: String,
    /// Title.
    pub title: Option<String>,
    /// Profile.
    pub profile: Option<String>,
    /// Status.
    pub status: String,
    /// Head run id.
    pub head_run_id: Option<String>,
    /// Head successful run id.
    pub head_success_run_id: Option<String>,
    /// Active run id.
    pub active_run_id: Option<String>,
    /// Run count.
    pub run_count: usize,
    /// Last output preview.
    pub last_output_preview: Option<String>,
    /// Creation time.
    pub created_at: String,
    /// Last update time.
    pub updated_at: String,
}

/// Run summary row.
#[derive(Clone, Debug, Serialize)]
pub struct RunSummary {
    /// Run id.
    pub run_id: String,
    /// Sequence number.
    pub sequence_no: usize,
    /// Run status.
    pub status: String,
    /// Restore source run id.
    pub restore_from_run_id: Option<String>,
    /// Output preview.
    pub output_preview: Option<String>,
    /// Creation time.
    pub created_at: String,
    /// Last update time.
    pub updated_at: String,
}

/// Trim report.
#[derive(Clone, Debug, Default, Serialize)]
pub struct TrimReport {
    /// Sessions scanned.
    pub sessions_scanned: usize,
    /// Runs selected for trimming.
    pub runs_to_trim: usize,
    /// Runs trimmed.
    pub runs_trimmed: usize,
    /// Bytes reclaimed from file store.
    pub bytes_reclaimed: u64,
    /// Dry-run flag.
    pub dry_run: bool,
}

impl LocalStore {
    /// Open a local store and initialize schema.
    pub fn open(config: &CliConfig) -> CliResult<Self> {
        crate::config::ensure_config_dirs(config)?;
        let conn = Connection::open(&config.database_path)?;
        conn.busy_timeout(Duration::from_secs(10))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let store = Self {
            conn,
            file_store_path: config.file_store_path.clone(),
        };
        store.init_schema()?;
        Ok(store)
    }

    #[allow(clippy::too_many_lines)]
    fn init_schema(&self) -> CliResult<()> {
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
            ",
        )?;
        add_column_if_missing(&self.conn, "file_refs", "checksum", "TEXT")?;
        add_column_if_missing(&self.conn, "file_refs", "content_type", "TEXT")?;
        Ok(())
    }

    /// Create or load a session.
    pub fn create_session(
        &mut self,
        profile: &str,
        title: Option<String>,
    ) -> CliResult<SessionRecord> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let session_id = SessionId::from_string(format!("session_{}", Uuid::new_v4()));
        let mut session = SessionRecord::new(session_id);
        session.profile = Some(profile.to_string());
        session.title = title;
        upsert_session_tx(&tx, &session)?;
        tx.commit()?;
        Ok(session)
    }

    /// Load a session.
    pub fn load_session(&self, session_id: &str) -> CliResult<SessionRecord> {
        self.conn
            .query_row(
                "SELECT record_json FROM sessions WHERE session_id = ?1",
                [session_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|json| serde_json::from_str(&json).map_err(CliError::from))
            .transpose()?
            .ok_or_else(|| CliError::NotFound(session_id.to_string()))
    }

    /// Resolve a session id or unique session id prefix.
    pub fn resolve_session_prefix(&self, session_id_or_prefix: &str) -> CliResult<String> {
        if self.load_session(session_id_or_prefix).is_ok() {
            return Ok(session_id_or_prefix.to_string());
        }
        let mut stmt = self.conn.prepare(
            "SELECT session_id FROM sessions WHERE session_id LIKE ?1 ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([format!("{session_id_or_prefix}%")], |row| {
            row.get::<_, String>(0)
        })?;
        let matches = rows.collect::<Result<Vec<_>, _>>()?;
        match matches.as_slice() {
            [session_id] => Ok(session_id.clone()),
            [] => Err(CliError::NotFound(session_id_or_prefix.to_string())),
            _ => Err(CliError::Usage(format!(
                "session prefix '{session_id_or_prefix}' is ambiguous"
            ))),
        }
    }

    /// Delete one session and its retained evidence.
    pub fn delete_session(&mut self, session_id: &str) -> CliResult<bool> {
        self.load_session(session_id)?;
        let path = self.file_store_path.join("sessions").join(session_id);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for table in [
            "display_messages",
            "raw_stream_records",
            "context_states",
            "environment_states",
            "stream_cursors",
            "checkpoints",
            "approvals",
            "deferred_tools",
            "file_refs",
            "runs",
            "sessions",
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE session_id = ?1"),
                params![session_id],
            )?;
        }
        tx.commit()?;
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|error| io_error(&path, error))?;
        }
        Ok(true)
    }

    /// Load a run.
    pub fn load_run(&self, session_id: &str, run_id: &str) -> CliResult<RunRecord> {
        self.conn
            .query_row(
                "SELECT record_json FROM runs WHERE session_id = ?1 AND run_id = ?2",
                params![session_id, run_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|json| serde_json::from_str(&json).map_err(CliError::from))
            .transpose()?
            .ok_or_else(|| CliError::NotFound(run_id.to_string()))
    }

    /// Latest active session.
    pub fn latest_session(&self) -> CliResult<Option<SessionRecord>> {
        self.conn
            .query_row(
                "SELECT record_json FROM sessions WHERE status = 'active' ORDER BY updated_at DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|json| serde_json::from_str(&json).map_err(CliError::from))
            .transpose()
    }

    /// Append a new queued run atomically and update session pointers.
    pub fn append_run(
        &mut self,
        session_id: &str,
        prompt: String,
        restore_from_run_id: Option<String>,
        profile: &str,
    ) -> CliResult<RunRecord> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut session = load_session_tx(&tx, session_id)?;
        let sequence_no = next_sequence_tx(&tx, session_id)?;
        let run_id = RunId::new();
        let mut run = RunRecord::new(session.session_id.clone(), run_id, ConversationId::new());
        run.sequence_no = sequence_no;
        run.restore_from_run_id = restore_from_run_id.map(RunId::from_string);
        run.trigger_type = Some("cli".to_string());
        run.profile = Some(profile.to_string());
        run.input = vec![InputPart::text(prompt)];
        session.head_run_id = Some(run.run_id.clone());
        session.active_run_id = Some(run.run_id.clone());
        session.updated_at = Utc::now();
        upsert_run_tx(&tx, &run)?;
        upsert_session_tx(&tx, &session)?;
        tx.commit()?;
        Ok(run)
    }

    /// Complete or pause a run, persist display messages, archive stream blobs, and update pointers.
    pub fn complete_run(
        &mut self,
        run: &mut RunRecord,
        output: String,
        artifacts: RunArtifacts,
    ) -> CliResult<Vec<DisplayMessage>> {
        let raw_ref = self.write_run_blob(run, "raw.stream.json", &artifacts.raw_records)?;
        let display_ref =
            self.write_run_blob(run, "display.compact.json", &artifacts.display_snapshot)?;
        let state_ref = self.write_run_blob(run, "context.state.json", &artifacts.state)?;
        let env_ref = artifacts
            .environment_state
            .as_ref()
            .map(|state| self.write_run_blob(run, "environment.state.json", state))
            .transpose()?;
        let checkpoint_refs = checkpoint_refs(run, &artifacts.raw_records);
        let latest_checkpoint = checkpoint_refs.last().cloned();
        let raw_cursor = StreamCursorRef::new(
            "raw_runtime",
            format!("run:{}", run.run_id.as_str()),
            artifacts
                .raw_records
                .last()
                .map_or(0, |record| record.sequence),
        );
        let display_cursor = StreamCursorRef::new(
            "display",
            format!("run:{}", run.run_id.as_str()),
            artifacts
                .display_messages
                .last()
                .map_or(0, |message| message.sequence),
        );
        let environment_ref =
            artifacts
                .environment_state
                .as_ref()
                .map(|state| EnvironmentStateRef {
                    provider: state.provider_id.clone(),
                    reference: format!(
                        "sessions/{}/runs/{}/environment.state.json",
                        run.session_id.as_str(),
                        run.run_id.as_str()
                    ),
                    revision: Some(format!("{}", state.files.len() + state.resources.len())),
                    metadata: state.metadata.clone(),
                });
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut session = load_session_tx(&tx, run.session_id.as_str())?;
        run.status = artifacts.status;
        run.output_preview = Some(output);
        run.updated_at = Utc::now();
        run.latest_checkpoint = latest_checkpoint;
        run.environment_state.clone_from(&environment_ref);
        run.stream_cursors = vec![raw_cursor.clone(), display_cursor.clone()];
        session.state = artifacts.state.clone();
        session.environment_state = environment_ref;
        session.stream_cursors.clone_from(&run.stream_cursors);
        session.profile.clone_from(&run.profile);
        session.head_run_id = Some(run.run_id.clone());
        if artifacts.status == RunStatus::Completed {
            session.head_success_run_id = Some(run.run_id.clone());
        }
        if session.active_run_id.as_ref() == Some(&run.run_id)
            && artifacts.status != RunStatus::Waiting
        {
            session.active_run_id = None;
        }
        session.updated_at = run.updated_at;
        upsert_run_tx(&tx, run)?;
        upsert_session_tx(&tx, &session)?;
        insert_raw_stream_records_tx(&tx, run, &artifacts.raw_records)?;
        insert_display_messages_tx(&tx, &artifacts.display_messages)?;
        insert_file_ref_tx(&tx, run, &raw_ref)?;
        insert_file_ref_tx(&tx, run, &display_ref)?;
        insert_file_ref_tx(&tx, run, &state_ref)?;
        if let Some(env_ref) = env_ref {
            insert_file_ref_tx(&tx, run, &env_ref)?;
        }
        insert_context_state_tx(&tx, run, &artifacts.state)?;
        if let Some(environment_state) = artifacts.environment_state.as_ref() {
            insert_environment_state_tx(&tx, run, environment_state)?;
        }
        insert_stream_cursor_tx(&tx, run, &raw_cursor)?;
        insert_stream_cursor_tx(&tx, run, &display_cursor)?;
        insert_checkpoint_refs_tx(&tx, run, &checkpoint_refs)?;
        insert_approval_records_tx(&tx, &artifacts.approvals)?;
        insert_deferred_tool_records_tx(&tx, &artifacts.deferred_tools)?;
        tx.commit()?;
        Ok(artifacts.display_messages)
    }

    /// Fail a run atomically.
    pub fn fail_run(&mut self, run: &mut RunRecord, message: String) -> CliResult<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut session = load_session_tx(&tx, run.session_id.as_str())?;
        run.status = RunStatus::Failed;
        run.output_preview = Some(message);
        run.updated_at = Utc::now();
        session.head_run_id = Some(run.run_id.clone());
        if session.active_run_id.as_ref() == Some(&run.run_id) {
            session.active_run_id = None;
        }
        session.updated_at = run.updated_at;
        upsert_run_tx(&tx, run)?;
        upsert_session_tx(&tx, &session)?;
        tx.commit()?;
        Ok(())
    }

    /// Fail a run and persist terminal display evidence.
    pub fn fail_run_with_messages(
        &mut self,
        run: &mut RunRecord,
        message: String,
        messages: &[DisplayMessage],
    ) -> CliResult<()> {
        let display_ref = self.write_run_blob(run, "display.compact.json", &messages)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut session = load_session_tx(&tx, run.session_id.as_str())?;
        run.status = RunStatus::Failed;
        run.output_preview = Some(message);
        run.updated_at = Utc::now();
        session.head_run_id = Some(run.run_id.clone());
        if session.active_run_id.as_ref() == Some(&run.run_id) {
            session.active_run_id = None;
        }
        session.updated_at = run.updated_at;
        upsert_run_tx(&tx, run)?;
        upsert_session_tx(&tx, &session)?;
        insert_display_messages_tx(&tx, messages)?;
        insert_file_ref_tx(&tx, run, &display_ref)?;
        tx.commit()?;
        Ok(())
    }

    /// Load the latest saved state for a run selected as continuation source.
    pub fn load_restore_state(
        &self,
        session_id: &str,
        run_id: Option<&str>,
    ) -> CliResult<Option<ResumableState>> {
        let Some(run_id) = run_id else {
            return Ok(Some(self.load_session(session_id)?.state));
        };
        let mut state = self
            .conn
            .query_row(
                "SELECT state_json FROM context_states WHERE session_id = ?1 AND run_id = ?2",
                params![session_id, run_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|json| serde_json::from_str(&json).map_err(CliError::from))
            .transpose()?;
        if let Some(state) = state.as_mut() {
            self.inject_resolved_hitl_tool_returns(session_id, run_id, state)?;
        }
        Ok(state)
    }

    fn inject_resolved_hitl_tool_returns(
        &self,
        session_id: &str,
        run_id: &str,
        state: &mut ResumableState,
    ) -> CliResult<()> {
        let mut existing_returns = existing_resume_tool_return_ids(&state.message_history);
        let tool_call_order = latest_tool_call_order(&state.message_history);
        let latest_tool_call_ids = tool_call_order.iter().cloned().collect::<BTreeSet<_>>();
        let approvals = self.list_approvals(Some(session_id), Some(run_id))?;
        let deferred_tools = self.list_deferred_tools(Some(session_id), Some(run_id))?;
        let pending_approvals = approvals
            .iter()
            .filter(|approval| {
                approval.status == ApprovalStatus::Pending
                    && !existing_returns.contains(&approval.action_id)
            })
            .map(|approval| approval.approval_id.clone())
            .collect::<Vec<_>>();
        let pending_deferred = deferred_tools
            .iter()
            .filter(|deferred| {
                deferred_status_is_unresolved(deferred.status)
                    && !existing_returns.contains(&deferred.tool_call_id)
            })
            .map(|deferred| deferred.deferred_id.clone())
            .collect::<Vec<_>>();
        if !pending_approvals.is_empty() || !pending_deferred.is_empty() {
            return Err(pending_hitl_resume_error(
                run_id,
                &pending_approvals,
                &pending_deferred,
            ));
        }

        let mut resolved = Vec::<(String, ModelRequestPart)>::new();
        for tool_return in self.list_run_tool_returns(session_id, run_id)? {
            if !latest_tool_call_ids.contains(&tool_return.tool_call_id)
                || tool_return_control_flow(&tool_return).is_some()
                || existing_returns.contains(&tool_return.tool_call_id)
            {
                continue;
            }
            existing_returns.insert(tool_return.tool_call_id.clone());
            resolved.push((
                tool_return.tool_call_id.clone(),
                ModelRequestPart::ToolReturn(tool_return),
            ));
        }
        for approval in approvals {
            if existing_returns.contains(&approval.action_id) {
                continue;
            }
            if let Some(tool_return) = approval_tool_return(&approval) {
                existing_returns.insert(approval.action_id.clone());
                resolved.push((
                    approval.action_id.clone(),
                    ModelRequestPart::ToolReturn(tool_return),
                ));
            }
        }
        for deferred in deferred_tools {
            if existing_returns.contains(&deferred.tool_call_id) {
                continue;
            }
            if let Some(tool_return) = deferred_tool_return(&deferred) {
                existing_returns.insert(deferred.tool_call_id.clone());
                resolved.push((
                    deferred.tool_call_id.clone(),
                    ModelRequestPart::ToolReturn(tool_return),
                ));
            }
        }
        if resolved.is_empty() {
            return Ok(());
        }
        resolved.sort_by_key(|(tool_call_id, _)| {
            tool_call_order
                .iter()
                .position(|known| known == tool_call_id)
                .unwrap_or(usize::MAX)
        });
        let mut metadata = serde_json::Map::new();
        metadata.insert(
            "starweaver.resume.hitl_results".to_string(),
            serde_json::json!(true),
        );
        metadata.insert(
            "starweaver.resume.source_run_id".to_string(),
            serde_json::json!(run_id),
        );
        state
            .message_history
            .push(ModelMessage::Request(ModelRequest {
                parts: resolved.into_iter().map(|(_, part)| part).collect(),
                timestamp: Some(Utc::now()),
                instructions: None,
                run_id: Some(RunId::from_string(run_id)),
                conversation_id: state.conversation_id.clone(),
                metadata,
            }));
        Ok(())
    }

    fn list_run_tool_returns(
        &self,
        session_id: &str,
        run_id: &str,
    ) -> CliResult<Vec<ToolReturnPart>> {
        let mut stmt = self.conn.prepare(
            r"
            SELECT record_json FROM raw_stream_records
            WHERE session_id = ?1 AND run_id = ?2 AND kind = 'tool_return'
            ORDER BY sequence_no
            ",
        )?;
        let rows = stmt.query_map(params![session_id, run_id], |row| row.get::<_, String>(0))?;
        let mut tool_returns = Vec::new();
        for json in rows.collect::<Result<Vec<_>, _>>()? {
            let record: AgentStreamRecord = serde_json::from_str(&json)?;
            if let AgentStreamEvent::ToolReturn { tool_return, .. } = record.event {
                tool_returns.push(tool_return);
            }
        }
        Ok(tool_returns)
    }

    /// List session summaries.
    pub fn list_sessions(&self, limit: usize) -> CliResult<Vec<SessionSummary>> {
        let mut stmt = self.conn.prepare(
            r"
            SELECT s.session_id, s.title, s.profile, s.status, s.head_run_id, s.head_success_run_id,
                   s.active_run_id, s.created_at, s.updated_at, COUNT(r.run_id),
                   (SELECT output_preview FROM runs lr WHERE lr.session_id = s.session_id ORDER BY lr.sequence_no DESC LIMIT 1)
            FROM sessions s
            LEFT JOIN runs r ON r.session_id = s.session_id
            GROUP BY s.session_id
            ORDER BY s.updated_at DESC
            LIMIT ?1
            ",
        )?;
        let rows = stmt.query_map([usize_to_i64(limit)?], |row| {
            Ok(SessionSummary {
                session_id: row.get(0)?,
                title: row.get(1)?,
                profile: row.get(2)?,
                status: row.get(3)?,
                head_run_id: row.get(4)?,
                head_success_run_id: row.get(5)?,
                active_run_id: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
                run_count: i64_to_usize(row.get::<_, i64>(9)?)?,
                last_output_preview: row.get(10)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(CliError::from)
    }

    /// List run summaries.
    pub fn list_runs(&self, session_id: &str, limit: usize) -> CliResult<Vec<RunSummary>> {
        let mut stmt = self.conn.prepare(
            r"
            SELECT run_id, sequence_no, status, restore_from_run_id, output_preview, created_at, updated_at
            FROM runs
            WHERE session_id = ?1
            ORDER BY sequence_no DESC
            LIMIT ?2
            ",
        )?;
        let rows = stmt.query_map(params![session_id, usize_to_i64(limit)?], |row| {
            Ok(RunSummary {
                run_id: row.get(0)?,
                sequence_no: i64_to_usize(row.get::<_, i64>(1)?)?,
                status: row.get(2)?,
                restore_from_run_id: row.get(3)?,
                output_preview: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;
        let mut runs = rows.collect::<Result<Vec<_>, _>>()?;
        runs.sort_by_key(|run| run.sequence_no);
        Ok(runs)
    }

    /// Replay display messages for a session or run.
    pub fn replay_display(
        &self,
        session_id: &str,
        run_id: Option<&str>,
        after: Option<usize>,
    ) -> CliResult<Vec<DisplayMessage>> {
        let after = after.map_or(-1_i64, |value| i64::try_from(value).unwrap_or(i64::MAX));
        let sql = if run_id.is_some() {
            r"
            SELECT dm.message_json
            FROM display_messages dm
            JOIN runs r ON r.session_id = dm.session_id AND r.run_id = dm.run_id
            WHERE dm.session_id = ?1 AND dm.run_id = ?2 AND dm.sequence_no > ?3
            ORDER BY r.sequence_no, dm.sequence_no
            "
        } else {
            r"
            SELECT dm.message_json
            FROM display_messages dm
            JOIN runs r ON r.session_id = dm.session_id AND r.run_id = dm.run_id
            WHERE dm.session_id = ?1 AND dm.sequence_no > ?2
            ORDER BY r.sequence_no, dm.sequence_no
            "
        };
        let mut stmt = self.conn.prepare(sql)?;
        let mapped = if let Some(run_id) = run_id {
            stmt.query_map(params![session_id, run_id, after], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![session_id, after], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        mapped
            .into_iter()
            .map(|json| serde_json::from_str(&json).map_err(CliError::from))
            .collect()
    }

    /// Trim old runs for selected sessions.
    pub fn trim(
        &mut self,
        sessions: Vec<String>,
        keep_runs: usize,
        dry_run: bool,
    ) -> CliResult<TrimReport> {
        self.trim_with_age(sessions, keep_runs, None, dry_run)
    }

    /// Trim old runs for selected sessions and optional age horizon.
    pub fn trim_with_age(
        &mut self,
        sessions: Vec<String>,
        keep_runs: usize,
        older_than: Option<chrono::Duration>,
        dry_run: bool,
    ) -> CliResult<TrimReport> {
        let mut report = TrimReport {
            dry_run,
            ..TrimReport::default()
        };
        report.sessions_scanned = sessions.len();
        for session_id in sessions {
            let trim_runs = self.trim_candidates(&session_id, keep_runs, older_than)?;
            report.runs_to_trim += trim_runs.len();
            for run_id in trim_runs {
                let bytes = self.run_file_bytes(&session_id, &run_id)?;
                report.bytes_reclaimed = report.bytes_reclaimed.saturating_add(bytes);
                if !dry_run {
                    self.delete_run(&session_id, &run_id)?;
                    self.remove_run_files(&session_id, &run_id)?;
                    report.runs_trimmed += 1;
                }
            }
        }
        Ok(report)
    }

    /// Return all session ids.
    pub fn all_session_ids(&self) -> CliResult<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT session_id FROM sessions ORDER BY updated_at DESC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(CliError::from)
    }

    fn trim_candidates(
        &self,
        session_id: &str,
        keep_runs: usize,
        older_than: Option<chrono::Duration>,
    ) -> CliResult<Vec<String>> {
        let cutoff = older_than.map(|duration| (Utc::now() - duration).to_rfc3339());
        let mut stmt = self.conn.prepare(
            r"
            SELECT r.run_id
            FROM runs r
            JOIN sessions s ON s.session_id = r.session_id
            WHERE r.session_id = ?1
              AND r.sequence_no <= (
                  SELECT COALESCE(MAX(sequence_no), 0) FROM runs WHERE session_id = ?1
              ) - ?2
              AND (?3 IS NULL OR r.updated_at < ?3)
              AND (s.active_run_id IS NULL OR r.run_id != s.active_run_id)
            ORDER BY r.sequence_no
            ",
        )?;
        let rows = stmt.query_map(
            params![session_id, usize_to_i64(keep_runs)?, cutoff],
            |row| row.get::<_, String>(0),
        )?;
        rows.collect::<Result<Vec<_>, _>>().map_err(CliError::from)
    }

    fn delete_run(&mut self, session_id: &str, run_id: &str) -> CliResult<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for table in [
            "display_messages",
            "raw_stream_records",
            "context_states",
            "environment_states",
            "stream_cursors",
            "checkpoints",
            "approvals",
            "deferred_tools",
            "file_refs",
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE session_id = ?1 AND run_id = ?2"),
                params![session_id, run_id],
            )?;
        }
        tx.execute(
            "DELETE FROM runs WHERE session_id = ?1 AND run_id = ?2",
            params![session_id, run_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn write_run_blob<T: Serialize>(
        &self,
        run: &RunRecord,
        name: &str,
        value: &T,
    ) -> CliResult<FileRefRecord> {
        let relative = PathBuf::from("sessions")
            .join(run.session_id.as_str())
            .join("runs")
            .join(run.run_id.as_str())
            .join(name);
        let path = self.file_store_path.join(&relative);
        atomic_write_json(&path, value)?;
        let data = fs::read(&path).map_err(|error| io_error(&path, error))?;
        let bytes = data.len();
        Ok(FileRefRecord {
            ref_id: format!(
                "{}:{}:{}",
                run.session_id.as_str(),
                run.run_id.as_str(),
                name
            ),
            relative_path: relative.to_string_lossy().to_string(),
            byte_size: i64::try_from(bytes)
                .map_err(|error| CliError::Storage(error.to_string()))?,
            checksum: cheap_checksum(&data),
            content_type: "application/json".to_string(),
            created_at: Utc::now().to_rfc3339(),
        })
    }

    fn run_file_bytes(&self, session_id: &str, run_id: &str) -> CliResult<u64> {
        let mut stmt = self.conn.prepare("SELECT COALESCE(SUM(byte_size), 0) FROM file_refs WHERE session_id = ?1 AND run_id = ?2")?;
        let bytes = stmt.query_row(params![session_id, run_id], |row| row.get::<_, i64>(0))?;
        Ok(u64::try_from(bytes).unwrap_or(0))
    }

    fn remove_run_files(&self, session_id: &str, run_id: &str) -> CliResult<()> {
        let path = self
            .file_store_path
            .join("sessions")
            .join(session_id)
            .join("runs")
            .join(run_id);
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|error| io_error(&path, error))?;
        }
        Ok(())
    }

    /// List persisted approval records.
    pub fn list_approvals(
        &self,
        session_id: Option<&str>,
        run_id: Option<&str>,
    ) -> CliResult<Vec<ApprovalRecord>> {
        let mut stmt = self.conn.prepare(
            r"
            SELECT record_json FROM approvals
            WHERE (?1 IS NULL OR session_id = ?1)
              AND (?2 IS NULL OR run_id = ?2)
            ORDER BY updated_at DESC, created_at DESC
            ",
        )?;
        let rows = stmt.query_map(params![session_id, run_id], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|json| serde_json::from_str(&json).map_err(CliError::from))
            .collect()
    }

    /// Load one approval record.
    pub fn load_approval(&self, approval_id: &str) -> CliResult<ApprovalRecord> {
        self.conn
            .query_row(
                "SELECT record_json FROM approvals WHERE approval_id = ?1",
                [approval_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|json| serde_json::from_str(&json).map_err(CliError::from))
            .transpose()?
            .ok_or_else(|| CliError::NotFound(approval_id.to_string()))
    }

    /// Record an approval decision.
    pub fn decide_approval(
        &mut self,
        approval_id: &str,
        status: ApprovalStatus,
        reason: Option<String>,
    ) -> CliResult<ApprovalRecord> {
        let mut approval = self.load_approval(approval_id)?;
        approval.status = status;
        approval.decision = Some(ApprovalDecision {
            status,
            decided_by: Some("starweaver-cli".to_string()),
            decided_at: Utc::now(),
            reason,
            metadata: serde_json::Map::default(),
        });
        approval.updated_at = Utc::now();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        insert_approval_records_tx(&tx, &[approval.clone()])?;
        tx.commit()?;
        Ok(approval)
    }

    /// List persisted deferred tool records.
    pub fn list_deferred_tools(
        &self,
        session_id: Option<&str>,
        run_id: Option<&str>,
    ) -> CliResult<Vec<DeferredToolRecord>> {
        let mut stmt = self.conn.prepare(
            r"
            SELECT record_json FROM deferred_tools
            WHERE (?1 IS NULL OR session_id = ?1)
              AND (?2 IS NULL OR run_id = ?2)
            ORDER BY updated_at DESC, created_at DESC
            ",
        )?;
        let rows = stmt.query_map(params![session_id, run_id], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|json| serde_json::from_str(&json).map_err(CliError::from))
            .collect()
    }

    /// Load one deferred tool record.
    pub fn load_deferred_tool(&self, deferred_id: &str) -> CliResult<DeferredToolRecord> {
        self.conn
            .query_row(
                "SELECT record_json FROM deferred_tools WHERE deferred_id = ?1",
                [deferred_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|json| serde_json::from_str(&json).map_err(CliError::from))
            .transpose()?
            .ok_or_else(|| CliError::NotFound(deferred_id.to_string()))
    }

    /// Complete one deferred tool record.
    pub fn complete_deferred_tool(
        &mut self,
        deferred_id: &str,
        response: Value,
    ) -> CliResult<DeferredToolRecord> {
        self.update_deferred_tool(deferred_id, ExecutionStatus::Completed, response)
    }

    /// Fail one deferred tool record.
    pub fn fail_deferred_tool(
        &mut self,
        deferred_id: &str,
        error: &str,
    ) -> CliResult<DeferredToolRecord> {
        self.update_deferred_tool(
            deferred_id,
            ExecutionStatus::Failed,
            serde_json::json!({"error": error}),
        )
    }

    fn update_deferred_tool(
        &mut self,
        deferred_id: &str,
        status: ExecutionStatus,
        response: Value,
    ) -> CliResult<DeferredToolRecord> {
        let mut deferred = self.load_deferred_tool(deferred_id)?;
        deferred.status = status;
        deferred.response = response;
        deferred.updated_at = Utc::now();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        insert_deferred_tool_records_tx(&tx, &[deferred.clone()])?;
        tx.commit()?;
        Ok(deferred)
    }
}

fn existing_resume_tool_return_ids(history: &[ModelMessage]) -> BTreeSet<String> {
    let Some(last_tool_response_index) = history.iter().rposition(|message| match message {
        ModelMessage::Response(response) => response
            .parts
            .iter()
            .any(|part| matches!(part, ModelResponsePart::ToolCall(_))),
        ModelMessage::Request(_) => false,
    }) else {
        return BTreeSet::new();
    };
    history
        .iter()
        .skip(last_tool_response_index.saturating_add(1))
        .filter_map(|message| match message {
            ModelMessage::Request(request) => Some(&request.parts),
            ModelMessage::Response(_) => None,
        })
        .flat_map(|parts| parts.iter())
        .filter_map(|part| match part {
            ModelRequestPart::ToolReturn(tool_return) => Some(tool_return.tool_call_id.clone()),
            _ => None,
        })
        .collect()
}

fn latest_tool_call_order(history: &[ModelMessage]) -> Vec<String> {
    history
        .iter()
        .rev()
        .find_map(|message| match message {
            ModelMessage::Response(response) => Some(
                response
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        ModelResponsePart::ToolCall(call) => Some(call.id.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            ),
            ModelMessage::Request(_) => None,
        })
        .unwrap_or_default()
}

fn tool_return_control_flow(tool_return: &ToolReturnPart) -> Option<&str> {
    tool_return
        .metadata
        .get("control_flow")
        .and_then(serde_json::Value::as_str)
}

const fn deferred_status_is_unresolved(status: ExecutionStatus) -> bool {
    matches!(
        status,
        ExecutionStatus::Pending | ExecutionStatus::Running | ExecutionStatus::Waiting
    )
}

fn pending_hitl_resume_error(
    run_id: &str,
    pending_approvals: &[String],
    pending_deferred: &[String],
) -> CliError {
    let mut details = Vec::new();
    if !pending_approvals.is_empty() {
        details.push(format!("approvals={}", pending_approvals.join(",")));
    }
    if !pending_deferred.is_empty() {
        details.push(format!("deferred_tools={}", pending_deferred.join(",")));
    }
    CliError::Usage(format!(
        "cannot resume run {run_id}: pending HITL decisions remain ({})",
        details.join("; ")
    ))
}

fn approval_tool_return(record: &ApprovalRecord) -> Option<ToolReturnPart> {
    let decision = record.decision.as_ref();
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "control_flow_resolution".to_string(),
        serde_json::json!("approval"),
    );
    metadata.insert(
        "approval_id".to_string(),
        serde_json::json!(record.approval_id),
    );
    metadata.insert(
        "approval_status".to_string(),
        serde_json::json!(record.status),
    );
    if let Some(decision) = decision {
        metadata.insert("decision".to_string(), serde_json::json!(decision));
    }
    match record.status {
        ApprovalStatus::Approved => {
            let mut content = serde_json::json!({
                "approved": true,
                "approval_id": record.approval_id,
                "tool_name": record.action_name,
                "request": record.request,
            });
            if let Some(reason) = decision.and_then(|decision| decision.reason.as_ref()) {
                content["reason"] = serde_json::json!(reason);
            }
            Some(
                ToolReturnPart::new(
                    record.action_id.clone(),
                    record.action_name.clone(),
                    content,
                )
                .with_metadata(metadata),
            )
        }
        ApprovalStatus::Denied | ApprovalStatus::Expired | ApprovalStatus::Cancelled => {
            let reason = decision
                .and_then(|decision| decision.reason.clone())
                .unwrap_or_else(|| format!("approval {}", approval_status_name(record.status)));
            Some(
                ToolReturnPart::new(
                    record.action_id.clone(),
                    record.action_name.clone(),
                    serde_json::json!({
                        "approved": false,
                        "approval_id": record.approval_id,
                        "tool_name": record.action_name,
                        "reason": reason,
                    }),
                )
                .with_error(true)
                .with_metadata(metadata),
            )
        }
        ApprovalStatus::Pending => None,
    }
}

fn deferred_tool_return(record: &DeferredToolRecord) -> Option<ToolReturnPart> {
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "control_flow_resolution".to_string(),
        serde_json::json!("deferred"),
    );
    metadata.insert(
        "deferred_id".to_string(),
        serde_json::json!(record.deferred_id),
    );
    metadata.insert(
        "deferred_status".to_string(),
        serde_json::json!(record.status),
    );
    match record.status {
        ExecutionStatus::Completed => Some(
            ToolReturnPart::new(
                record.tool_call_id.clone(),
                record.tool_name.clone(),
                record.response.clone(),
            )
            .with_metadata(metadata),
        ),
        ExecutionStatus::Failed | ExecutionStatus::Cancelled => Some(
            ToolReturnPart::new(
                record.tool_call_id.clone(),
                record.tool_name.clone(),
                if record.response.is_null() {
                    serde_json::json!({"error": deferred_status_name(record.status)})
                } else {
                    record.response.clone()
                },
            )
            .with_error(true)
            .with_metadata(metadata),
        ),
        ExecutionStatus::Pending | ExecutionStatus::Running | ExecutionStatus::Waiting => None,
    }
}

const fn approval_status_name(status: ApprovalStatus) -> &'static str {
    match status {
        ApprovalStatus::Pending => "pending",
        ApprovalStatus::Approved => "approved",
        ApprovalStatus::Denied => "denied",
        ApprovalStatus::Expired => "expired",
        ApprovalStatus::Cancelled => "cancelled",
    }
}

const fn deferred_status_name(status: ExecutionStatus) -> &'static str {
    match status {
        ExecutionStatus::Pending => "pending",
        ExecutionStatus::Running => "running",
        ExecutionStatus::Waiting => "waiting",
        ExecutionStatus::Completed => "completed",
        ExecutionStatus::Failed => "failed",
        ExecutionStatus::Cancelled => "cancelled",
    }
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> CliResult<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !columns.iter().any(|existing| existing == column) {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

fn load_session_tx(tx: &rusqlite::Transaction<'_>, session_id: &str) -> CliResult<SessionRecord> {
    tx.query_row(
        "SELECT record_json FROM sessions WHERE session_id = ?1",
        [session_id],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .map(|json| serde_json::from_str(&json).map_err(CliError::from))
    .transpose()?
    .ok_or_else(|| CliError::NotFound(session_id.to_string()))
}

fn next_sequence_tx(tx: &rusqlite::Transaction<'_>, session_id: &str) -> CliResult<usize> {
    let value = tx.query_row(
        "SELECT COALESCE(MAX(sequence_no), 0) + 1 FROM runs WHERE session_id = ?1",
        [session_id],
        |row| row.get::<_, i64>(0),
    )?;
    usize::try_from(value).map_err(|error| CliError::Storage(error.to_string()))
}

fn upsert_session_tx(tx: &rusqlite::Transaction<'_>, session: &SessionRecord) -> CliResult<()> {
    tx.execute(
        r"
        INSERT INTO sessions (session_id, status, profile, title, head_run_id, head_success_run_id, active_run_id, created_at, updated_at, record_json)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        ON CONFLICT(session_id) DO UPDATE SET
            status = excluded.status,
            profile = excluded.profile,
            title = excluded.title,
            head_run_id = excluded.head_run_id,
            head_success_run_id = excluded.head_success_run_id,
            active_run_id = excluded.active_run_id,
            updated_at = excluded.updated_at,
            record_json = excluded.record_json
        ",
        params![
            session.session_id.as_str(),
            session_status(session.status),
            session.profile.as_deref(),
            session.title.as_deref(),
            session.head_run_id.as_ref().map(RunId::as_str),
            session.head_success_run_id.as_ref().map(RunId::as_str),
            session.active_run_id.as_ref().map(RunId::as_str),
            session.created_at.to_rfc3339(),
            session.updated_at.to_rfc3339(),
            serde_json::to_string(session)?,
        ],
    )?;
    Ok(())
}

fn upsert_run_tx(tx: &rusqlite::Transaction<'_>, run: &RunRecord) -> CliResult<()> {
    tx.execute(
        r"
        INSERT INTO runs (session_id, run_id, sequence_no, status, restore_from_run_id, output_preview, created_at, updated_at, record_json)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ON CONFLICT(session_id, run_id) DO UPDATE SET
            sequence_no = excluded.sequence_no,
            status = excluded.status,
            restore_from_run_id = excluded.restore_from_run_id,
            output_preview = excluded.output_preview,
            updated_at = excluded.updated_at,
            record_json = excluded.record_json
        ",
        params![
            run.session_id.as_str(),
            run.run_id.as_str(),
            usize_to_i64(run.sequence_no)?,
            run_status(run.status),
            run.restore_from_run_id.as_ref().map(RunId::as_str),
            run.output_preview.as_deref(),
            run.created_at.to_rfc3339(),
            run.updated_at.to_rfc3339(),
            serde_json::to_string(run)?,
        ],
    )?;
    Ok(())
}

fn insert_raw_stream_records_tx(
    tx: &rusqlite::Transaction<'_>,
    run: &RunRecord,
    records: &[AgentStreamRecord],
) -> CliResult<()> {
    for record in records {
        tx.execute(
            r"
            INSERT OR REPLACE INTO raw_stream_records (session_id, run_id, sequence_no, kind, created_at, record_json)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ",
            params![
                run.session_id.as_str(),
                run.run_id.as_str(),
                usize_to_i64(record.sequence)?,
                raw_stream_kind(&record.event),
                Utc::now().to_rfc3339(),
                serde_json::to_string(record)?,
            ],
        )?;
    }
    Ok(())
}

fn insert_display_messages_tx(
    tx: &rusqlite::Transaction<'_>,
    messages: &[DisplayMessage],
) -> CliResult<()> {
    for message in messages {
        tx.execute(
            r"
            INSERT OR REPLACE INTO display_messages (session_id, run_id, sequence_no, kind, created_at, message_json)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ",
            params![
                message.session_id.as_str(),
                message.run_id.as_str(),
                usize_to_i64(message.sequence)?,
                format!("{:?}", message.kind).to_lowercase(),
                message.timestamp.to_rfc3339(),
                serde_json::to_string(message)?,
            ],
        )?;
    }
    Ok(())
}

fn insert_context_state_tx(
    tx: &rusqlite::Transaction<'_>,
    run: &RunRecord,
    state: &ResumableState,
) -> CliResult<()> {
    tx.execute(
        "INSERT OR REPLACE INTO context_states (session_id, run_id, state_json, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![
            run.session_id.as_str(),
            run.run_id.as_str(),
            serde_json::to_string(state)?,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

fn insert_environment_state_tx(
    tx: &rusqlite::Transaction<'_>,
    run: &RunRecord,
    state: &EnvironmentState,
) -> CliResult<()> {
    let ref_id = format!(
        "{}:{}:environment",
        run.session_id.as_str(),
        run.run_id.as_str()
    );
    tx.execute(
        "INSERT OR REPLACE INTO environment_states (ref_id, session_id, run_id, provider, state_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            ref_id,
            run.session_id.as_str(),
            run.run_id.as_str(),
            state.provider_id,
            serde_json::to_string(state)?,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

fn insert_stream_cursor_tx(
    tx: &rusqlite::Transaction<'_>,
    run: &RunRecord,
    cursor: &StreamCursorRef,
) -> CliResult<()> {
    tx.execute(
        "INSERT OR REPLACE INTO stream_cursors (session_id, run_id, family, scope, sequence_no, cursor_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            run.session_id.as_str(),
            run.run_id.as_str(),
            cursor.family,
            cursor.scope,
            usize_to_i64(cursor.sequence)?,
            serde_json::to_string(cursor)?,
            cursor.created_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

fn insert_checkpoint_refs_tx(
    tx: &rusqlite::Transaction<'_>,
    run: &RunRecord,
    checkpoints: &[CheckpointRef],
) -> CliResult<()> {
    for checkpoint in checkpoints {
        tx.execute(
            "INSERT OR REPLACE INTO checkpoints (checkpoint_id, session_id, run_id, sequence_no, node, checkpoint_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                checkpoint.checkpoint_id.as_str(),
                run.session_id.as_str(),
                run.run_id.as_str(),
                usize_to_i64(checkpoint.sequence)?,
                checkpoint.node,
                serde_json::to_string(checkpoint)?,
                checkpoint.created_at.to_rfc3339(),
            ],
        )?;
    }
    Ok(())
}

fn insert_approval_records_tx(
    tx: &rusqlite::Transaction<'_>,
    approvals: &[ApprovalRecord],
) -> CliResult<()> {
    for approval in approvals {
        tx.execute(
            "INSERT OR REPLACE INTO approvals (approval_id, session_id, run_id, action_id, action_name, status, record_json, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                approval.approval_id,
                approval.session_id.as_str(),
                approval.run_id.as_str(),
                approval.action_id,
                approval.action_name,
                format!("{:?}", approval.status).to_lowercase(),
                serde_json::to_string(approval)?,
                approval.created_at.to_rfc3339(),
                approval.updated_at.to_rfc3339(),
            ],
        )?;
    }
    Ok(())
}

fn insert_deferred_tool_records_tx(
    tx: &rusqlite::Transaction<'_>,
    deferred_tools: &[DeferredToolRecord],
) -> CliResult<()> {
    for deferred in deferred_tools {
        tx.execute(
            "INSERT OR REPLACE INTO deferred_tools (deferred_id, session_id, run_id, tool_call_id, tool_name, status, record_json, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                deferred.deferred_id,
                deferred.session_id.as_str(),
                deferred.run_id.as_str(),
                deferred.tool_call_id,
                deferred.tool_name,
                format!("{:?}", deferred.status).to_lowercase(),
                serde_json::to_string(deferred)?,
                deferred.created_at.to_rfc3339(),
                deferred.updated_at.to_rfc3339(),
            ],
        )?;
    }
    Ok(())
}

fn insert_file_ref_tx(
    tx: &rusqlite::Transaction<'_>,
    run: &RunRecord,
    file_ref: &FileRefRecord,
) -> CliResult<()> {
    tx.execute(
        "INSERT OR REPLACE INTO file_refs (ref_id, session_id, run_id, relative_path, byte_size, checksum, content_type, created_at, trimmed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
        params![
            file_ref.ref_id,
            run.session_id.as_str(),
            run.run_id.as_str(),
            file_ref.relative_path,
            file_ref.byte_size,
            file_ref.checksum,
            file_ref.content_type,
            file_ref.created_at,
        ],
    )?;
    Ok(())
}

fn checkpoint_refs(run: &RunRecord, records: &[AgentStreamRecord]) -> Vec<CheckpointRef> {
    records
        .iter()
        .filter_map(|record| match &record.event {
            AgentStreamEvent::Checkpoint { node, step } => Some(CheckpointRef {
                checkpoint_id: CheckpointId::from_string(format!(
                    "ckpt_{}_{}",
                    run.run_id.as_str(),
                    record.sequence
                )),
                run_id: run.run_id.clone(),
                sequence: record.sequence,
                node: format!("{node:?}"),
                storage_ref: Some(format!(
                    "sessions/{}/runs/{}/checkpoints/{}.json",
                    run.session_id.as_str(),
                    run.run_id.as_str(),
                    record.sequence
                )),
                stream_cursor: Some(record.sequence),
                created_at: Utc::now(),
                metadata: serde_json::json!({"step": step})
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            }),
            _ => None,
        })
        .collect()
}

const fn raw_stream_kind(event: &AgentStreamEvent) -> &'static str {
    match event {
        AgentStreamEvent::RunStart { .. } => "run_start",
        AgentStreamEvent::NodeStart { .. } => "node_start",
        AgentStreamEvent::NodeComplete { .. } => "node_complete",
        AgentStreamEvent::Custom { .. } => "custom",
        AgentStreamEvent::ModelRequest { .. } => "model_request",
        AgentStreamEvent::ModelStream { .. } => "model_stream",
        AgentStreamEvent::ModelResponse { .. } => "model_response",
        AgentStreamEvent::Checkpoint { .. } => "checkpoint",
        AgentStreamEvent::Suspended { .. } => "suspended",
        AgentStreamEvent::ToolCall { .. } => "tool_call",
        AgentStreamEvent::ToolReturn { .. } => "tool_return",
        AgentStreamEvent::OutputRetry { .. } => "output_retry",
        AgentStreamEvent::SteeringGuard { .. } => "steering_guard",
        AgentStreamEvent::RunComplete { .. } => "run_complete",
        AgentStreamEvent::RunFailed { .. } => "run_failed",
    }
}

const fn session_status(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Active => "active",
        SessionStatus::Archived => "archived",
        SessionStatus::Failed => "failed",
    }
}

const fn run_status(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Queued => "queued",
        RunStatus::Running => "running",
        RunStatus::Waiting => "waiting",
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
    }
}

fn usize_to_i64(value: usize) -> CliResult<i64> {
    i64::try_from(value).map_err(|error| CliError::Storage(error.to_string()))
}

fn i64_to_usize(value: i64) -> rusqlite::Result<usize> {
    usize::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> CliResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| CliError::Storage("missing parent path".to_string()))?;
    fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    let temp = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    fs::write(&temp, serde_json::to_vec_pretty(value)?).map_err(|error| io_error(&temp, error))?;
    fs::rename(&temp, path).map_err(|error| io_error(path, error))?;
    Ok(())
}

fn cheap_checksum(data: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("fnv64:{hash:016x}")
}
