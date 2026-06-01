//! Local `SQLite` and file-store persistence for CLI sessions.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use serde_json::json;
use starweaver_core::{ConversationId, RunId, SessionId};
use starweaver_session::{InputPart, RunRecord, RunStatus, SessionRecord, SessionStatus};
use starweaver_stream::{DisplayMessage, DisplayMessageKind};
use uuid::Uuid;

use crate::{config::CliConfig, error::io_error, CliError, CliResult};

/// Local `SQLite` and file-store handle.
pub struct LocalStore {
    conn: Connection,
    file_store_path: PathBuf,
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
            CREATE TABLE IF NOT EXISTS file_refs (
                ref_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                relative_path TEXT NOT NULL,
                byte_size INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                trimmed_at TEXT
            );
            ",
        )?;
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

    /// Complete a run atomically, persist display messages, and update head pointers.
    pub fn complete_run(
        &mut self,
        run: &mut RunRecord,
        output: String,
    ) -> CliResult<Vec<DisplayMessage>> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut session = load_session_tx(&tx, run.session_id.as_str())?;
        run.status = RunStatus::Completed;
        run.output_preview = Some(output.clone());
        run.updated_at = Utc::now();
        session.head_run_id = Some(run.run_id.clone());
        session.head_success_run_id = Some(run.run_id.clone());
        if session.active_run_id.as_ref() == Some(&run.run_id) {
            session.active_run_id = None;
        }
        session.updated_at = run.updated_at;
        let messages = vec![
            DisplayMessage::new(
                0,
                run.session_id.clone(),
                run.run_id.clone(),
                DisplayMessageKind::RunStarted,
            )
            .with_payload(json!({"source": "cli"}))
            .with_preview("run started"),
            DisplayMessage::new(
                1,
                run.session_id.clone(),
                run.run_id.clone(),
                DisplayMessageKind::AssistantTextStart,
            )
            .with_payload(json!({
                "message_id": "final",
                "role": "assistant",
            })),
            DisplayMessage::new(
                2,
                run.session_id.clone(),
                run.run_id.clone(),
                DisplayMessageKind::AssistantTextDelta,
            )
            .with_payload(json!({
                "message_id": "final",
                "delta": output,
            }))
            .with_preview(output.clone()),
            DisplayMessage::new(
                3,
                run.session_id.clone(),
                run.run_id.clone(),
                DisplayMessageKind::AssistantTextEnd,
            )
            .with_payload(json!({"message_id": "final"})),
            DisplayMessage::new(
                4,
                run.session_id.clone(),
                run.run_id.clone(),
                DisplayMessageKind::RunCompleted,
            )
            .with_payload(json!({"output": output}))
            .with_preview(output),
        ];
        upsert_run_tx(&tx, run)?;
        upsert_session_tx(&tx, &session)?;
        insert_display_messages_tx(&tx, &messages)?;
        tx.commit()?;
        self.write_run_blob(run, "display.compact.json", &messages)?;
        Ok(messages)
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
            "SELECT message_json FROM display_messages WHERE session_id = ?1 AND run_id = ?2 AND sequence_no > ?3 ORDER BY run_id, sequence_no"
        } else {
            "SELECT message_json FROM display_messages WHERE session_id = ?1 AND sequence_no > ?3 ORDER BY run_id, sequence_no"
        };
        let mut stmt = self.conn.prepare(sql)?;
        let mapped = if let Some(run_id) = run_id {
            stmt.query_map(params![session_id, run_id, after], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![session_id, "", after], |row| {
                row.get::<_, String>(0)
            })?
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
        let mut report = TrimReport {
            dry_run,
            ..TrimReport::default()
        };
        report.sessions_scanned = sessions.len();
        for session_id in sessions {
            let trim_runs = self.trim_candidates(&session_id, keep_runs)?;
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

    fn trim_candidates(&self, session_id: &str, keep_runs: usize) -> CliResult<Vec<String>> {
        let mut stmt = self.conn.prepare(
            r"
            SELECT r.run_id
            FROM runs r
            JOIN sessions s ON s.session_id = r.session_id
            WHERE r.session_id = ?1
              AND r.sequence_no <= (
                  SELECT COALESCE(MAX(sequence_no), 0) FROM runs WHERE session_id = ?1
              ) - ?2
              AND (s.head_success_run_id IS NULL OR r.run_id != s.head_success_run_id)
              AND (s.active_run_id IS NULL OR r.run_id != s.active_run_id)
            ORDER BY r.sequence_no
            ",
        )?;
        let rows = stmt.query_map(params![session_id, usize_to_i64(keep_runs)?], |row| {
            row.get::<_, String>(0)
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(CliError::from)
    }

    fn delete_run(&mut self, session_id: &str, run_id: &str) -> CliResult<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM display_messages WHERE session_id = ?1 AND run_id = ?2",
            params![session_id, run_id],
        )?;
        tx.execute(
            "DELETE FROM file_refs WHERE session_id = ?1 AND run_id = ?2",
            params![session_id, run_id],
        )?;
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
    ) -> CliResult<()> {
        let relative = PathBuf::from("sessions")
            .join(run.session_id.as_str())
            .join("runs")
            .join(run.run_id.as_str())
            .join(name);
        let path = self.file_store_path.join(&relative);
        atomic_write_json(&path, value)?;
        let bytes = fs::metadata(&path)
            .map_err(|error| io_error(&path, error))?
            .len();
        self.conn.execute(
            "INSERT OR REPLACE INTO file_refs (ref_id, session_id, run_id, relative_path, byte_size, created_at, trimmed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
            params![
                format!("{}:{}:{}", run.session_id.as_str(), run.run_id.as_str(), name),
                run.session_id.as_str(),
                run.run_id.as_str(),
                relative.to_string_lossy().to_string(),
                i64::try_from(bytes).map_err(|error| CliError::Storage(error.to_string()))?,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
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
    let temp = path.with_extension("tmp");
    fs::write(&temp, serde_json::to_vec_pretty(value)?).map_err(|error| io_error(&temp, error))?;
    fs::rename(&temp, path).map_err(|error| io_error(path, error))?;
    Ok(())
}
