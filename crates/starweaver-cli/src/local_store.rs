//! CLI persistence facade over shared `SQLite` storage and product-owned JSON blobs.

use std::{collections::BTreeSet, fs, path::Path, path::PathBuf, sync::Arc};

use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use starweaver_agent::ResumableState;
use starweaver_core::{ConversationId, RunId, SessionId};
use starweaver_environment::EnvironmentState;
use starweaver_model::{ModelMessage, ModelRequest, ModelRequestPart, ToolReturnPart};
use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord};
use starweaver_session::{
    ApprovalRecord, ApprovalStatus, DeferredToolRecord, EnvironmentStateRef, ExecutionStatus,
    InputPart, RunRecord, RunStatus, SessionRecord, SessionSearchError, SessionSearchPage,
    SessionSearchProvider, SessionSearchQuery, SessionSearchScope, SessionStatus,
    SessionStoreError, StreamCursorRef,
};
use starweaver_storage::{LocalSessionSearchProvider, RunEvidenceCommit, SqliteStorage};
use starweaver_stream::{DisplayMessage, ReplayCursor, ReplayScope, ReplaySnapshot};
use uuid::Uuid;

use crate::{CliError, CliResult, config::CliConfig, error::io_error};

mod archive;
mod hitl;
mod replay;
mod session_store;

pub use archive::LocalStreamArchive;
use hitl::{
    approval_tool_return, deferred_status_is_unresolved, deferred_tool_return,
    existing_resume_tool_return_ids, latest_tool_call_order, pending_hitl_resume_error,
    tool_return_control_flow,
};
pub use replay::DisplayReplayWindow;
pub use session_store::LocalSessionStore;

/// Local product facade backed by the workspace-wide canonical `SQLite` schema.
pub struct LocalStore {
    storage: SqliteStorage,
    file_store_path: PathBuf,
    search_scope: SessionSearchScope,
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
    /// Open canonical shared storage and the CLI-owned blob directory.
    pub fn open(config: &CliConfig) -> CliResult<Self> {
        crate::config::ensure_config_dirs(config)?;
        Ok(Self {
            storage: SqliteStorage::open(&config.database_path).map_err(storage_error)?,
            file_store_path: config.file_store_path.clone(),
            search_scope: SessionSearchScope::local(
                config.database_path.to_string_lossy().into_owned(),
            ),
        })
    }

    /// Create a session.
    pub fn create_session(
        &mut self,
        profile: &str,
        title: Option<String>,
    ) -> CliResult<SessionRecord> {
        self.storage
            .create_session(Some(profile.to_string()), title)
            .map_err(storage_error)
    }

    /// Load a session.
    pub fn load_session(&self, session_id: &str) -> CliResult<SessionRecord> {
        self.storage
            .load_session(&SessionId::from_string(session_id))
            .map_err(storage_error)
    }

    /// Resolve an exact session id or unique prefix.
    pub fn resolve_session_prefix(&self, value: &str) -> CliResult<String> {
        self.storage
            .resolve_session_prefix(value)
            .map(|session_id| session_id.as_str().to_string())
            .map_err(|error| match error {
                SessionStoreError::NotFound(_) => CliError::NotFound(value.to_string()),
                SessionStoreError::Failed(message) if message.contains("ambiguous") => {
                    CliError::Usage(message)
                }
                other => storage_error(other),
            })
    }

    /// Delete one session and its shared durable evidence plus CLI-owned blobs.
    pub fn delete_session(&mut self, session_id: &str) -> CliResult<bool> {
        let session_id = SessionId::from_string(session_id);
        let deleted = self
            .storage
            .delete_session(&session_id)
            .map_err(storage_error)?;
        if deleted {
            let path = self
                .file_store_path
                .join("sessions")
                .join(session_id.as_str());
            if path.exists() {
                fs::remove_dir_all(&path).map_err(|error| io_error(&path, error))?;
            }
        }
        Ok(deleted)
    }

    /// Load a run.
    pub fn load_run(&self, session_id: &str, run_id: &str) -> CliResult<RunRecord> {
        self.storage
            .load_run(
                &SessionId::from_string(session_id),
                &RunId::from_string(run_id),
            )
            .map_err(storage_error)
    }

    /// Latest active session.
    pub fn latest_session(&self) -> CliResult<Option<SessionRecord>> {
        Ok(self
            .storage
            .list_sessions()
            .map_err(storage_error)?
            .into_iter()
            .find(|session| session.status == SessionStatus::Active))
    }

    /// Append a queued run and update session pointers atomically.
    pub fn append_run(
        &mut self,
        session_id: &str,
        prompt: String,
        restore_from_run_id: Option<String>,
        profile: &str,
    ) -> CliResult<RunRecord> {
        let session = self.load_session(session_id)?;
        let mut run = RunRecord::new(session.session_id, RunId::new(), ConversationId::new());
        run.restore_from_run_id = restore_from_run_id.map(RunId::from_string);
        run.trigger_type = Some("cli".to_string());
        run.profile = Some(profile.to_string());
        run.input = vec![InputPart::text(prompt)];
        self.storage.begin_run(run).map_err(storage_error)
    }

    /// Complete or pause a run and atomically commit shared durable evidence.
    pub fn complete_run(
        &mut self,
        run: &mut RunRecord,
        output: String,
        artifacts: RunArtifacts,
    ) -> CliResult<Vec<DisplayMessage>> {
        let scope = ReplayScope::run(run.run_id.as_str());
        let mut display_snapshot = artifacts.display_snapshot.clone();
        if display_snapshot.scope.is_none() {
            display_snapshot.scope = Some(scope.clone());
        }
        let raw_cursor = artifacts.raw_records.last().map(|record| {
            StreamCursorRef::new(ReplayCursor::raw_runtime(scope.clone(), record.sequence))
        });
        let display_cursor = artifacts.display_messages.last().map(|message| {
            StreamCursorRef::new(ReplayCursor::display(scope.clone(), message.sequence))
        });
        let environment_ref =
            artifacts
                .environment_state
                .as_ref()
                .map(|state| EnvironmentStateRef {
                    provider: state.provider_id.clone(),
                    reference: format!(
                        "sqlite:run_environment_records/{}/{}",
                        run.session_id.as_str(),
                        run.run_id.as_str()
                    ),
                    revision: Some(format!("{}", state.files.len() + state.resources.len())),
                    metadata: state.metadata.clone(),
                });
        run.status = artifacts.status;
        run.output_preview = Some(output);
        run.updated_at = Utc::now();
        run.environment_state.clone_from(&environment_ref);
        run.stream_cursors = raw_cursor.into_iter().chain(display_cursor).collect();

        let mut commit = RunEvidenceCommit::new(run.clone(), artifacts.state.clone());
        commit.environment_state = artifacts
            .environment_state
            .as_ref()
            .map(EnvironmentState::to_json);
        commit.stream_records.clone_from(&artifacts.raw_records);
        commit.approvals.clone_from(&artifacts.approvals);
        commit.deferred_tools.clone_from(&artifacts.deferred_tools);
        commit.stream_cursors.clone_from(&run.stream_cursors);
        commit
            .display_messages
            .clone_from(&artifacts.display_messages);
        commit.display_snapshot = Some(display_snapshot.clone());
        *run = self
            .storage
            .commit_run_evidence(commit)
            .map_err(storage_error)?;

        // Compatibility mirrors are best-effort and are written only after the canonical SQLite
        // evidence commit. A mirror failure must not turn a durably completed run into a reported
        // failure: no canonical record points at these mutable files, and search already treats
        // them as optional compatibility evidence.
        let _ = self.write_run_blob(run, "raw.stream.json", &artifacts.raw_records);
        let _ = self.write_run_blob(run, "display.compact.json", &display_snapshot);
        let _ = self.write_run_blob(run, "context.state.json", &artifacts.state);
        if let Some(environment_state) = artifacts.environment_state.as_ref() {
            let _ =
                self.write_run_blob(run, "environment.state.json", &environment_state.to_json());
        }
        Ok(artifacts.display_messages)
    }

    /// Fail a run atomically.
    pub fn fail_run(&mut self, run: &mut RunRecord, message: String) -> CliResult<()> {
        self.fail_run_with_messages(run, message, &[])
    }

    /// Fail a run and persist terminal display evidence.
    pub fn fail_run_with_messages(
        &mut self,
        run: &mut RunRecord,
        message: String,
        messages: &[DisplayMessage],
    ) -> CliResult<()> {
        let session = self.load_session(run.session_id.as_str())?;
        run.status = RunStatus::Failed;
        run.output_preview = Some(message);
        run.updated_at = Utc::now();
        let mut commit = RunEvidenceCommit::new(run.clone(), session.state);
        commit.display_messages = messages.to_vec();
        *run = self
            .storage
            .commit_run_evidence(commit)
            .map_err(storage_error)?;
        self.write_run_blob(run, "display.compact.json", &messages)?;
        Ok(())
    }

    /// Load the latest saved state for a continuation source.
    pub fn load_restore_state(
        &self,
        session_id: &str,
        run_id: Option<&str>,
    ) -> CliResult<Option<ResumableState>> {
        let Some(run_id) = run_id else {
            return Ok(Some(self.load_session(session_id)?.state));
        };
        let mut state = self
            .storage
            .load_run_context(
                &SessionId::from_string(session_id),
                &RunId::from_string(run_id),
            )
            .map_err(storage_error)?;
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
        let records = self
            .storage
            .load_stream_records(
                &SessionId::from_string(session_id),
                &RunId::from_string(run_id),
            )
            .map_err(storage_error)?;
        Ok(records
            .into_iter()
            .filter_map(|record| match record.event {
                AgentStreamEvent::ToolReturn { tool_return, .. } => Some(tool_return),
                _ => None,
            })
            .collect())
    }

    /// List session summaries.
    pub fn list_sessions(&self, limit: usize) -> CliResult<Vec<SessionSummary>> {
        self.storage
            .list_sessions()
            .map_err(storage_error)?
            .into_iter()
            .take(limit)
            .map(|session| {
                let runs = self
                    .storage
                    .list_runs(&session.session_id)
                    .map_err(storage_error)?;
                Ok(SessionSummary {
                    session_id: session.session_id.as_str().to_string(),
                    title: session.title,
                    profile: session.profile,
                    status: session_status_name(session.status).to_string(),
                    head_run_id: session.head_run_id.map(|id| id.as_str().to_string()),
                    head_success_run_id: session
                        .head_success_run_id
                        .map(|id| id.as_str().to_string()),
                    active_run_id: session.active_run_id.map(|id| id.as_str().to_string()),
                    run_count: runs.len(),
                    last_output_preview: runs.last().and_then(|run| run.output_preview.clone()),
                    created_at: session.created_at.to_rfc3339(),
                    updated_at: session.updated_at.to_rfc3339(),
                })
            })
            .collect()
    }

    /// Search canonical local sessions and approved compatibility display projections.
    pub fn search_sessions(&self, query: SessionSearchQuery) -> CliResult<SessionSearchPage> {
        let provider = LocalSessionSearchProvider::new(
            Arc::new(self.storage.session_store()),
            &self.search_scope,
        )
        .with_display_root(self.file_store_path.clone());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| CliError::Run(error.to_string()))?;
        runtime
            .block_on(provider.search(&self.search_scope, query))
            .map_err(search_error)
    }

    /// List canonical run records in session sequence order.
    pub fn list_run_records(&self, session_id: &str) -> CliResult<Vec<RunRecord>> {
        self.storage
            .list_runs(&SessionId::from_string(session_id))
            .map_err(storage_error)
    }

    /// List run summaries in sequence order, retaining the newest `limit` runs.
    pub fn list_runs(&self, session_id: &str, limit: usize) -> CliResult<Vec<RunSummary>> {
        let mut runs = self
            .storage
            .list_runs(&SessionId::from_string(session_id))
            .map_err(storage_error)?;
        if runs.len() > limit {
            runs.drain(..runs.len() - limit);
        }
        Ok(runs.into_iter().map(run_summary).collect())
    }

    /// Replay display messages for a session or run.
    pub fn replay_display(
        &self,
        session_id: &str,
        run_id: Option<&str>,
        after: Option<usize>,
    ) -> CliResult<Vec<DisplayMessage>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = run_id.map(RunId::from_string);
        self.storage
            .load_display_messages(&session_id, run_id.as_ref(), after)
            .map_err(storage_error)
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
            sessions_scanned: sessions.len(),
            dry_run,
            ..TrimReport::default()
        };
        let cutoff = older_than.map(|duration| Utc::now() - duration);
        for session_id in sessions {
            let session_id = SessionId::from_string(session_id);
            let session = self
                .storage
                .load_session(&session_id)
                .map_err(storage_error)?;
            let runs = self.storage.list_runs(&session_id).map_err(storage_error)?;
            let keep_from = runs.len().saturating_sub(keep_runs);
            let candidates = runs
                .into_iter()
                .take(keep_from)
                .filter(|run| session.active_run_id.as_ref() != Some(&run.run_id))
                .filter(|run| cutoff.is_none_or(|cutoff| run.updated_at < cutoff))
                .collect::<Vec<_>>();
            report.runs_to_trim += candidates.len();
            for run in &candidates {
                report.bytes_reclaimed = report
                    .bytes_reclaimed
                    .saturating_add(self.run_file_bytes(session_id.as_str(), run.run_id.as_str())?);
            }
            if !dry_run && !candidates.is_empty() {
                let run_ids = candidates
                    .iter()
                    .map(|run| run.run_id.clone())
                    .collect::<Vec<_>>();
                report.runs_trimmed += self
                    .storage
                    .prune_runs(&session_id, &run_ids)
                    .map_err(storage_error)?;
                for run_id in run_ids {
                    self.remove_run_files(session_id.as_str(), run_id.as_str())?;
                }
            }
        }
        Ok(report)
    }

    /// Return all session ids.
    pub fn all_session_ids(&self) -> CliResult<Vec<String>> {
        Ok(self
            .storage
            .list_sessions()
            .map_err(storage_error)?
            .into_iter()
            .map(|session| session.session_id.as_str().to_string())
            .collect())
    }

    fn write_run_blob<T: Serialize>(
        &self,
        run: &RunRecord,
        name: &str,
        value: &T,
    ) -> CliResult<()> {
        let path = self
            .file_store_path
            .join("sessions")
            .join(run.session_id.as_str())
            .join("runs")
            .join(run.run_id.as_str())
            .join(name);
        atomic_write_json(&path, value)
    }

    fn run_file_bytes(&self, session_id: &str, run_id: &str) -> CliResult<u64> {
        directory_bytes(
            &self
                .file_store_path
                .join("sessions")
                .join(session_id)
                .join("runs")
                .join(run_id),
        )
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
        let session_id = session_id.map(SessionId::from_string);
        let run_id = run_id.map(RunId::from_string);
        self.storage
            .list_approvals(session_id.as_ref(), run_id.as_ref())
            .map_err(storage_error)
    }

    /// Load one approval record.
    pub fn load_approval(&self, approval_id: &str) -> CliResult<ApprovalRecord> {
        self.storage
            .load_approval(approval_id)
            .map_err(storage_error)
    }

    /// Record an approval decision.
    pub fn decide_approval(
        &mut self,
        approval_id: &str,
        status: ApprovalStatus,
        reason: Option<String>,
    ) -> CliResult<ApprovalRecord> {
        self.storage
            .decide_approval(
                approval_id,
                status,
                Some("starweaver-cli".to_string()),
                reason,
            )
            .map_err(storage_error)
    }

    /// List persisted deferred tool records.
    pub fn list_deferred_tools(
        &self,
        session_id: Option<&str>,
        run_id: Option<&str>,
    ) -> CliResult<Vec<DeferredToolRecord>> {
        let session_id = session_id.map(SessionId::from_string);
        let run_id = run_id.map(RunId::from_string);
        self.storage
            .list_deferred_tools(session_id.as_ref(), run_id.as_ref())
            .map_err(storage_error)
    }

    /// Load one deferred tool record.
    pub fn load_deferred_tool(&self, deferred_id: &str) -> CliResult<DeferredToolRecord> {
        self.storage
            .load_deferred_tool(deferred_id)
            .map_err(storage_error)
    }

    /// Complete one deferred tool record.
    pub fn complete_deferred_tool(
        &mut self,
        deferred_id: &str,
        response: Value,
    ) -> CliResult<DeferredToolRecord> {
        self.storage
            .resolve_deferred_tool(deferred_id, ExecutionStatus::Completed, response)
            .map_err(storage_error)
    }

    /// Fail one deferred tool record.
    pub fn fail_deferred_tool(
        &mut self,
        deferred_id: &str,
        error: &str,
    ) -> CliResult<DeferredToolRecord> {
        self.storage
            .resolve_deferred_tool(
                deferred_id,
                ExecutionStatus::Failed,
                serde_json::json!({"error": error}),
            )
            .map_err(storage_error)
    }
}

fn run_summary(run: RunRecord) -> RunSummary {
    RunSummary {
        run_id: run.run_id.as_str().to_string(),
        sequence_no: run.sequence_no,
        status: run_status_name(run.status).to_string(),
        restore_from_run_id: run
            .restore_from_run_id
            .map(|run_id| run_id.as_str().to_string()),
        output_preview: run.output_preview,
        created_at: run.created_at.to_rfc3339(),
        updated_at: run.updated_at.to_rfc3339(),
    }
}

const fn run_status_name(status: RunStatus) -> &'static str {
    status.as_str()
}

const fn session_status_name(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Active => "active",
        SessionStatus::Archived => "archived",
        SessionStatus::Failed => "failed",
        SessionStatus::Deleted => "deleted",
    }
}

fn storage_error(error: SessionStoreError) -> CliError {
    match error {
        SessionStoreError::NotFound(value) => CliError::NotFound(value),
        other => CliError::Storage(other.to_string()),
    }
}

fn search_error(error: SessionSearchError) -> CliError {
    match error {
        SessionSearchError::InvalidQuery(message) | SessionSearchError::InvalidCursor(message) => {
            CliError::Usage(message)
        }
        SessionSearchError::Unsupported(message) => CliError::Unsupported(message),
        SessionSearchError::Unavailable(message) | SessionSearchError::Failed(message) => {
            CliError::Storage(message)
        }
        SessionSearchError::PermissionDenied => {
            CliError::Storage("session search permission denied".to_string())
        }
    }
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> CliResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| CliError::Storage("missing parent path".to_string()))?;
    fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    let temp = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    fs::write(&temp, serde_json::to_vec_pretty(value)?).map_err(|error| io_error(&temp, error))?;
    fs::rename(&temp, path).map_err(|error| io_error(path, error))
}

fn directory_bytes(path: &Path) -> CliResult<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0_u64;
    for entry in fs::read_dir(path).map_err(|error| io_error(path, error))? {
        let entry = entry.map_err(|error| io_error(path, error))?;
        let entry_path = entry.path();
        let metadata = entry
            .metadata()
            .map_err(|error| io_error(&entry_path, error))?;
        if metadata.is_dir() {
            total = total.saturating_add(directory_bytes(&entry_path)?);
        } else {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}
