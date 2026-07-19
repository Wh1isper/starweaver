//! CLI persistence facade over shared `SQLite` storage and product-owned JSON blobs.

use std::{fs, path::Path, path::PathBuf, sync::Arc};

use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use starweaver_context::{AgentCheckpoint, ResumableState};
use starweaver_core::{ConversationId, RunId, SessionId};
use starweaver_environment::EnvironmentState;
use starweaver_runtime::AgentStreamRecord;
use starweaver_session::{
    AcquireRunAdmission, ApprovalRecord, ApprovalStatus, ContinuationPreparationMode,
    DeferredToolRecord, EnvironmentStateRef, ExecutionStatus, InputPart, LOCAL_SESSION_NAMESPACE,
    PreparedContinuation, RelatedRunUpdate, RunAdmissionLease, RunRecord, RunStatus,
    RunTerminalError, SessionRecord, SessionSearchError, SessionSearchPage, SessionSearchProvider,
    SessionSearchQuery, SessionSearchScope, SessionStatus, SessionStore, SessionStoreError,
    StreamCursorRef,
};
use starweaver_storage::{
    LocalSessionSearchProvider, LocalStoreImportReport, RunEvidenceCommit, SqliteStorage,
};
use starweaver_stream::{DisplayMessage, ReplayCursor, ReplayScope, ReplaySnapshot};
use uuid::Uuid;

use crate::{CliError, CliResult, config::CliConfig, error::io_error};

mod archive;
mod replay;
mod session_store;

pub use archive::LocalStreamArchive;
pub use replay::DisplayReplayWindow;
pub use session_store::LocalSessionStore;

pub const HITL_RESUME_CLAIM_ID_METADATA_KEY: &str = "starweaver.cli.hitl_resume_claim_id";
pub const HITL_RESUME_SOURCE_RUN_ID_METADATA_KEY: &str = "starweaver.cli.hitl_resume_source_run_id";
pub const HITL_RESUME_PREFLIGHT_SOURCE_RUN_ID_METADATA_KEY: &str =
    "starweaver.cli.hitl_resume_preflight_source_run_id";

/// Local product facade backed by the workspace-wide canonical `SQLite` schema.
pub struct LocalStore {
    storage: SqliteStorage,
    file_store_path: PathBuf,
    workspace: String,
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
    /// Full runtime checkpoints captured at resumable boundaries.
    pub checkpoints: Vec<AgentCheckpoint>,
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
    /// Safe diagnostic when the selected terminal status failed or was cancelled.
    pub terminal_error: Option<RunTerminalError>,
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
        let storage = SqliteStorage::open(&config.database_path).map_err(storage_error)?;
        let workspace = fs::canonicalize(&config.workspace_root)
            .unwrap_or_else(|_| config.workspace_root.clone())
            .to_string_lossy()
            .into_owned();
        Ok(Self {
            storage,
            file_store_path: config.file_store_path.clone(),
            workspace,
            search_scope: SessionSearchScope::local(
                config.database_path.to_string_lossy().into_owned(),
            ),
        })
    }

    /// Explicitly import a project-local legacy database into canonical shared storage.
    pub fn import_legacy_database(
        &self,
        source_path: impl AsRef<Path>,
        workspace: impl AsRef<Path>,
    ) -> CliResult<LocalStoreImportReport> {
        self.storage
            .import_legacy_project_database(source_path, workspace)
            .map_err(storage_error)
    }

    /// Create a session.
    pub fn create_session(
        &mut self,
        profile: &str,
        title: Option<String>,
    ) -> CliResult<SessionRecord> {
        self.storage
            .create_session_for_product(
                Some(profile.to_string()),
                title,
                Some(self.workspace.clone()),
                Some("cli"),
            )
            .map_err(storage_error)
    }

    /// Load a session.
    pub fn load_session(&self, session_id: &str) -> CliResult<SessionRecord> {
        self.storage
            .load_session(&SessionId::from_string(session_id))
            .map_err(storage_error)
    }

    /// Load a session and require it to belong to the current workspace.
    pub fn load_workspace_session(&self, session_id: &str) -> CliResult<SessionRecord> {
        let session = self.load_session(session_id)?;
        if session.workspace.as_deref() != Some(self.workspace.as_str()) {
            return Err(CliError::NotFound(session_id.to_string()));
        }
        Ok(session)
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

    /// Tombstone one shared session and remove only CLI-owned compatibility blobs.
    pub fn delete_session(&mut self, session_id: &str) -> CliResult<bool> {
        let session_id = SessionId::from_string(session_id);
        let session = self
            .storage
            .load_session(&session_id)
            .map_err(storage_error)?;
        let newly_tombstoned = session.status != SessionStatus::Deleted;
        if newly_tombstoned {
            let fence_id = format!("cli-delete-{}", Uuid::new_v4());
            let idempotency_key = fence_id.clone();
            let command_fingerprint = format!("delete:{}", session_id.as_str());
            self.storage
                .acquire_session_deletion_fence(
                    &session_id,
                    session.revision,
                    &fence_id,
                    "cli",
                    &idempotency_key,
                    &command_fingerprint,
                )
                .map_err(storage_error)?;
            self.storage
                .tombstone_session(&session_id, &fence_id)
                .map_err(storage_error)?;
        }
        let path = self
            .file_store_path
            .join("sessions")
            .join(session_id.as_str());
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|error| io_error(&path, error))?;
        }
        Ok(newly_tombstoned)
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

    /// Latest active session in the current workspace.
    pub fn latest_session(&self) -> CliResult<Option<SessionRecord>> {
        Ok(self
            .workspace_sessions()?
            .into_iter()
            .find(|session| session.status == SessionStatus::Active))
    }

    /// Append a queued run without admission for fixture construction only.
    #[cfg(test)]
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
        run.trigger_type = Some("cli-test-fixture".to_string());
        run.profile = Some(profile.to_string());
        run.input = vec![InputPart::text(prompt)];
        self.storage.begin_run(run).map_err(storage_error)
    }

    /// Atomically admit a queued run and return its fenced lease.
    #[allow(clippy::too_many_arguments)]
    pub fn admit_run(
        &mut self,
        session_id: &str,
        prompt: String,
        restore_from_run_id: Option<String>,
        profile: &str,
        initial_metadata: serde_json::Map<String, Value>,
        host_instance_id: &str,
        replaces_waiting_run_id: Option<RunId>,
        hitl_resume_claim_id: Option<String>,
    ) -> CliResult<(RunRecord, RunAdmissionLease)> {
        let session = self.load_session(session_id)?;
        let mut run = RunRecord::new(session.session_id, RunId::new(), ConversationId::new());
        run.restore_from_run_id = restore_from_run_id.map(RunId::from_string);
        run.trigger_type = Some("cli".to_string());
        run.profile = Some(profile.to_string());
        run.input = vec![InputPart::text(prompt)];
        run.metadata = initial_metadata;
        let run_id = run.run_id.clone();
        let receipt = self
            .storage
            .acquire_run_admission(AcquireRunAdmission {
                run,
                namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
                host_instance_id: host_instance_id.to_string(),
                admission_id: format!("cli-admission-{}", Uuid::new_v4()),
                lease_expires_at: Utc::now() + chrono::Duration::seconds(30),
                idempotency_key: format!("cli-run-{}", run_id.as_str()),
                command_fingerprint: format!("cli-run-v1:{}", run_id.as_str()),
                replaces_waiting_run_id,
                hitl_resume_claim_id,
            })
            .map_err(storage_error)?;
        Ok((receipt.run, receipt.lease))
    }

    /// Extend a CLI-owned durable admission lease.
    pub fn heartbeat_run_admission(
        config: &CliConfig,
        lease: &RunAdmissionLease,
    ) -> CliResult<RunAdmissionLease> {
        SqliteStorage::open(&config.database_path)
            .map_err(storage_error)?
            .heartbeat_run_admission(lease, Utc::now() + chrono::Duration::seconds(30))
            .map_err(storage_error)
    }

    /// Release a CLI-owned durable admission lease.
    pub fn release_run_admission(&self, lease: &RunAdmissionLease) -> CliResult<()> {
        self.storage
            .release_run_admission(lease)
            .map_err(storage_error)
    }

    /// Complete or pause a fixture run without an admission lease.
    #[cfg(test)]
    pub fn complete_run(
        &mut self,
        run: &mut RunRecord,
        output: String,
        artifacts: RunArtifacts,
    ) -> CliResult<Vec<DisplayMessage>> {
        self.complete_run_with_admission(run, output, artifacts, None)
    }

    /// Complete or pause an admitted product run while its lease remains current.
    pub fn complete_run_fenced(
        &mut self,
        run: &mut RunRecord,
        output: String,
        artifacts: RunArtifacts,
        admission_lease: &RunAdmissionLease,
    ) -> CliResult<Vec<DisplayMessage>> {
        self.complete_run_with_admission(run, output, artifacts, Some(admission_lease))
    }

    fn complete_run_with_admission(
        &self,
        run: &mut RunRecord,
        output: String,
        artifacts: RunArtifacts,
        admission_lease: Option<&RunAdmissionLease>,
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
        run.terminal_error = artifacts.terminal_error.or_else(|| {
            (run.status == RunStatus::Failed)
                .then(|| RunTerminalError::new("cli_run_failed", output.clone()))
        });
        run.output_preview = match run.status {
            RunStatus::Failed | RunStatus::Cancelled => None,
            _ => Some(output),
        };
        run.updated_at = Utc::now();
        run.environment_state.clone_from(&environment_ref);
        run.stream_cursors = raw_cursor.into_iter().chain(display_cursor).collect();

        let mut commit = RunEvidenceCommit::new(run.clone(), artifacts.state.clone());
        commit.environment_state = artifacts
            .environment_state
            .as_ref()
            .map(EnvironmentState::to_json);
        commit.stream_records.clone_from(&artifacts.raw_records);
        commit.checkpoints.clone_from(&artifacts.checkpoints);
        commit.approvals.clone_from(&artifacts.approvals);
        commit.deferred_tools.clone_from(&artifacts.deferred_tools);
        commit.stream_cursors.clone_from(&run.stream_cursors);
        commit
            .display_messages
            .clone_from(&artifacts.display_messages);
        commit.display_snapshot = Some(display_snapshot.clone());
        let source_status = if run.status.is_terminal() {
            run.status
        } else {
            RunStatus::Completed
        };
        attach_hitl_resume_update(run, &mut commit, source_status)?;
        *run = match admission_lease {
            Some(lease) => self.storage.commit_run_evidence_fenced(lease, commit),
            None => self.storage.commit_run_evidence(commit),
        }
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

    /// Fail a fixture run atomically without an admission lease.
    #[cfg(test)]
    pub fn fail_run(&mut self, run: &mut RunRecord, message: String) -> CliResult<()> {
        self.fail_run_with_messages(run, message, &[])
    }

    /// Fail a fixture run and persist terminal display evidence without an admission lease.
    #[cfg(test)]
    pub fn fail_run_with_messages(
        &mut self,
        run: &mut RunRecord,
        message: String,
        messages: &[DisplayMessage],
    ) -> CliResult<()> {
        self.fail_run_with_messages_and_admission(run, message, messages, None)
    }

    /// Fail an admitted product run while its lease remains current.
    pub fn fail_run_with_messages_fenced(
        &mut self,
        run: &mut RunRecord,
        message: String,
        messages: &[DisplayMessage],
        admission_lease: &RunAdmissionLease,
    ) -> CliResult<()> {
        self.fail_run_with_messages_and_admission(run, message, messages, Some(admission_lease))
    }

    fn fail_run_with_messages_and_admission(
        &self,
        run: &mut RunRecord,
        message: String,
        messages: &[DisplayMessage],
        admission_lease: Option<&RunAdmissionLease>,
    ) -> CliResult<()> {
        let session = self.load_session(run.session_id.as_str())?;
        run.status = RunStatus::Failed;
        run.output_preview = None;
        run.terminal_error = Some(RunTerminalError::new("cli_run_failed", message));
        run.updated_at = Utc::now();
        let mut state = session.state;
        state.run_id = Some(run.run_id.clone());
        state.conversation_id = Some(run.conversation_id.clone());
        let mut commit = RunEvidenceCommit::new(run.clone(), state);
        commit.display_messages = messages.to_vec();
        attach_hitl_resume_update(run, &mut commit, RunStatus::Failed)?;
        *run = match admission_lease {
            Some(lease) => self.storage.commit_run_evidence_fenced(lease, commit),
            None => self.storage.commit_run_evidence(commit),
        }
        .map_err(storage_error)?;
        self.write_run_blob(run, "display.compact.json", &messages)?;
        Ok(())
    }

    /// Side-effect-free prepare a waiting HITL continuation from canonical evidence.
    pub fn prepare_waiting_continuation(
        &self,
        session_id: &str,
        run_id: &str,
    ) -> CliResult<PreparedContinuation> {
        let store = self.storage.session_store();
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| CliError::Run(error.to_string()))?;
        runtime
            .block_on(store.reconcile_expired_run_admissions(LOCAL_SESSION_NAMESPACE, Utc::now()))
            .map_err(storage_error)?;
        match runtime.block_on(store.prepare_continuation(
            &session_id,
            &run_id,
            ContinuationPreparationMode::WaitingHitl,
        )) {
            Ok(prepared) => Ok(prepared),
            Err(error) => {
                let mut pending = self
                    .list_approvals(Some(session_id.as_str()), Some(run_id.as_str()))?
                    .into_iter()
                    .filter(|approval| approval.status == ApprovalStatus::Pending)
                    .map(|approval| approval.approval_id)
                    .collect::<Vec<_>>();
                pending.extend(
                    self.list_deferred_tools(Some(session_id.as_str()), Some(run_id.as_str()))?
                        .into_iter()
                        .filter(|deferred| {
                            matches!(
                                deferred.status,
                                ExecutionStatus::Pending
                                    | ExecutionStatus::Running
                                    | ExecutionStatus::Waiting
                            )
                        })
                        .map(|deferred| deferred.deferred_id),
                );
                if pending.is_empty() {
                    Err(storage_error(error))
                } else {
                    Err(CliError::Run(format!(
                        "cannot resume run {} while HITL items are pending: {}",
                        run_id.as_str(),
                        pending.join(", ")
                    )))
                }
            }
        }
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
        self.storage
            .load_run_context(
                &SessionId::from_string(session_id),
                &RunId::from_string(run_id),
            )
            .map_err(storage_error)
    }

    /// List session summaries for the current workspace.
    pub fn list_sessions(&self, limit: usize) -> CliResult<Vec<SessionSummary>> {
        self.workspace_sessions()?
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

    /// Return all session ids in the current workspace.
    pub fn all_session_ids(&self) -> CliResult<Vec<String>> {
        Ok(self
            .workspace_sessions()?
            .into_iter()
            .map(|session| session.session_id.as_str().to_string())
            .collect())
    }

    fn workspace_sessions(&self) -> CliResult<Vec<SessionRecord>> {
        Ok(self
            .storage
            .list_sessions()
            .map_err(storage_error)?
            .into_iter()
            .filter(|session| {
                session.workspace.as_deref() == Some(self.workspace.as_str())
                    && session.status != SessionStatus::Deleted
            })
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

fn attach_hitl_resume_update(
    run: &RunRecord,
    commit: &mut RunEvidenceCommit,
    source_status: RunStatus,
) -> CliResult<()> {
    let claim_id = run
        .metadata
        .get(HITL_RESUME_CLAIM_ID_METADATA_KEY)
        .and_then(Value::as_str);
    let source_run_id = run
        .metadata
        .get(HITL_RESUME_SOURCE_RUN_ID_METADATA_KEY)
        .and_then(Value::as_str);
    match (claim_id, source_run_id) {
        (None, None) => Ok(()),
        (Some(claim_id), Some(source_run_id)) => {
            let mut update = RelatedRunUpdate::new(
                RunId::from_string(source_run_id),
                RunStatus::Waiting,
                source_status,
            );
            update.resume_claim_id = Some(claim_id.to_string());
            update.terminal_error = (source_status == RunStatus::Failed).then(|| {
                run.terminal_error.clone().unwrap_or_else(|| {
                    RunTerminalError::new("cli_continuation_failed", "continuation failed")
                })
            });
            commit.related_run_updates.push(update);
            Ok(())
        }
        _ => Err(CliError::Storage(
            "incomplete HITL resume claim metadata on continuation run".to_string(),
        )),
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
