//! Session, run, and event controllers for Claw.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use starweaver_core::{ConversationId, Metadata, RunId, SessionId};
use starweaver_session::{
    InputPart, RunRecord, RunStatus, SessionFilter, SessionRecord, SessionStore,
};
use starweaver_stream::{
    ReplayCursor, ReplayEvent, ReplayEventKind, ReplayEventLog, ReplayScope, StreamTerminalMarker,
};
use uuid::Uuid;

use crate::{
    execution::{ExecutionSupervisor, NoopRunExecutor},
    profile::{AgentProfile, ProfileResolver},
    runtime_state::ClawRuntimeState,
    workspace::{ResolvedWorkspaceBinding, WorkspaceBindingSpec, WorkspaceProvider},
    ClawError, ClawResult,
};

/// Run dispatch mode.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchMode {
    /// Queue durable work for a background coordinator.
    Queue,
    /// Submit asynchronously and return immediately.
    #[default]
    Async,
    /// Stream foreground events to the caller.
    Stream,
}

impl DispatchMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queue => "queue",
            Self::Async => "async",
            Self::Stream => "stream",
        }
    }
}

/// Trigger source for a run.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClawTriggerType {
    /// API client.
    #[default]
    Api,
    /// External bridge.
    Bridge,
    /// Schedule dispatcher.
    Schedule,
    /// Heartbeat dispatcher.
    Heartbeat,
    /// Memory lifecycle.
    Memory,
    /// Agency lifecycle.
    Agency,
    /// Async task lifecycle.
    AsyncTask,
    /// Workflow executor.
    Workflow,
}

impl ClawTriggerType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Bridge => "bridge",
            Self::Schedule => "schedule",
            Self::Heartbeat => "heartbeat",
            Self::Memory => "memory",
            Self::Agency => "agency",
            Self::AsyncTask => "async_task",
            Self::Workflow => "workflow",
        }
    }
}

/// API input part with Claw-compatible `type` tags.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClawInputPart {
    /// Text prompt input.
    Text {
        /// Text content.
        text: String,
        /// Optional metadata.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Metadata>,
    },
    /// URL input.
    Url {
        /// URL.
        url: String,
        /// Optional kind.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        /// Optional filename.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        /// Optional storage hint.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        storage: Option<String>,
        /// Optional metadata.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Metadata>,
    },
    /// File input.
    File {
        /// File path.
        path: String,
        /// Optional kind.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        /// Optional metadata.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Metadata>,
    },
    /// Binary input as an inline or stored resource reference.
    Binary {
        /// Binary data or URI.
        data: String,
        /// MIME type.
        mime_type: String,
        /// Optional kind.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        /// Optional filename.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        /// Optional storage hint.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        storage: Option<String>,
        /// Optional metadata.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Metadata>,
    },
    /// Mode hint.
    Mode {
        /// Mode name.
        mode: String,
        /// Optional params.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<Value>,
        /// Optional metadata.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Metadata>,
    },
    /// Product command.
    Command {
        /// Command name.
        name: String,
        /// Optional params.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<Value>,
        /// Optional metadata.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Metadata>,
    },
}

impl ClawInputPart {
    fn preview(&self) -> String {
        match self {
            Self::Text { text, .. } => text.clone(),
            Self::Url { url, .. } => url.clone(),
            Self::File { path, .. } => path.clone(),
            Self::Binary { filename, .. } => {
                filename.clone().unwrap_or_else(|| "<binary>".to_string())
            }
            Self::Mode { mode, .. } => format!("mode:{mode}"),
            Self::Command { name, .. } => format!("/{name}"),
        }
    }

    fn into_session_input(self) -> InputPart {
        match self {
            Self::Text { text, metadata } => InputPart::Text {
                text,
                metadata: metadata.unwrap_or_default(),
            },
            Self::Url { url, metadata, .. } => InputPart::Url {
                url,
                metadata: metadata.unwrap_or_default(),
            },
            Self::File {
                path,
                kind,
                metadata,
            } => InputPart::File {
                file: starweaver_session::FileRef {
                    uri: path,
                    media_type: kind,
                    name: None,
                },
                metadata: metadata.unwrap_or_default(),
            },
            Self::Binary {
                data,
                mime_type,
                metadata,
                ..
            } => InputPart::Binary {
                binary: starweaver_session::BinaryRef {
                    uri: data,
                    media_type: Some(mime_type),
                    bytes: None,
                },
                metadata: metadata.unwrap_or_default(),
            },
            Self::Mode {
                mode,
                params,
                metadata,
            } => InputPart::Mode {
                mode,
                config: params.unwrap_or(Value::Null),
                metadata: metadata.unwrap_or_default(),
            },
            Self::Command {
                name,
                params,
                metadata,
            } => InputPart::Command {
                command: name,
                args: Vec::new(),
                payload: params.unwrap_or(Value::Null),
                metadata: metadata.unwrap_or_default(),
            },
        }
    }
}

/// Create session request.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ClawSessionCreateRequest {
    /// Profile name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    /// Metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
    /// Workspace binding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceBindingSpec>,
    /// First run input parts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_parts: Vec<ClawInputPart>,
    /// Dispatch mode.
    #[serde(default)]
    pub dispatch_mode: DispatchMode,
    /// Trigger type.
    #[serde(default)]
    pub trigger_type: ClawTriggerType,
}

/// Fork session request.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ClawSessionForkRequest {
    /// Restore source run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_from_run_id: Option<String>,
    /// Profile override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    /// Metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
    /// Workspace binding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceBindingSpec>,
}

/// Create a session run request.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ClawSessionRunCreateRequest {
    /// Restore source run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_from_run_id: Option<String>,
    /// Reset session state before this run.
    #[serde(default)]
    pub reset_state: bool,
    /// Input parts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_parts: Vec<ClawInputPart>,
    /// Metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
    /// Workspace override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceBindingSpec>,
    /// Dispatch mode.
    #[serde(default)]
    pub dispatch_mode: DispatchMode,
    /// Trigger type.
    #[serde(default)]
    pub trigger_type: ClawTriggerType,
}

/// Direct create run request.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ClawRunCreateRequest {
    /// Session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Restore source run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_from_run_id: Option<String>,
    /// Reset session state before this run.
    #[serde(default)]
    pub reset_state: bool,
    /// Profile name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    /// Input parts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_parts: Vec<ClawInputPart>,
    /// Trigger type.
    #[serde(default)]
    pub trigger_type: ClawTriggerType,
    /// Metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
    /// Workspace override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceBindingSpec>,
    /// Dispatch mode.
    #[serde(default)]
    pub dispatch_mode: DispatchMode,
}

/// Control request for steering.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SteerRequest {
    /// Input parts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_parts: Vec<ClawInputPart>,
}

/// Session summary.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ClawSessionSummary {
    /// Session id.
    pub id: String,
    /// Profile name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    /// Head run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_run_id: Option<String>,
    /// Head success run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_success_run_id: Option<String>,
    /// Active run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_run_id: Option<String>,
    /// Metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Update time.
    pub updated_at: DateTime<Utc>,
}

/// Run detail.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ClawRunDetail {
    /// Run id.
    pub id: String,
    /// Session id.
    pub session_id: String,
    /// Sequence number.
    pub sequence_no: usize,
    /// Restore source run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_from_run_id: Option<String>,
    /// Run status.
    pub status: RunStatus,
    /// Trigger type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_type: Option<String>,
    /// Profile name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    /// Input preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_preview: Option<String>,
    /// Input parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_parts: Option<Vec<InputPart>>,
    /// Output text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_text: Option<String>,
    /// Created time.
    pub created_at: DateTime<Utc>,
    /// Updated time.
    pub updated_at: DateTime<Utc>,
    /// Metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Create session response.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ClawSessionCreateResponse {
    /// Session summary.
    pub session: ClawSessionSummary,
    /// Optional created run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<ClawRunDetail>,
}

/// Session get response.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ClawSessionGetResponse {
    /// Session summary.
    pub session: ClawSessionSummary,
    /// Recent runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_runs: Vec<ClawRunDetail>,
    /// Persisted session state.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub state: Value,
}

/// Event list response.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EventListResponse {
    /// Replay scope.
    pub scope: String,
    /// Events.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<ReplayEvent>,
}

/// Profile list response.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProfileListResponse {
    /// Profiles.
    pub profiles: Vec<AgentProfile>,
}

/// Workspace resolve response.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WorkspaceResolveResponse {
    /// Resolved binding.
    pub workspace: ResolvedWorkspaceBinding,
}

/// Core Claw controller.
#[derive(Clone)]
pub struct ClawController {
    store: Arc<dyn SessionStore>,
    events: Arc<dyn ReplayEventLog>,
    runtime_state: ClawRuntimeState,
    profiles: Arc<ProfileResolver>,
    workspace_provider: WorkspaceProvider,
    auto_execute: bool,
}

impl ClawController {
    /// Build a controller.
    #[must_use]
    pub fn new(
        store: Arc<dyn SessionStore>,
        events: Arc<dyn ReplayEventLog>,
        runtime_state: ClawRuntimeState,
        profiles: Arc<ProfileResolver>,
        workspace_provider: WorkspaceProvider,
    ) -> Self {
        Self {
            store,
            events,
            runtime_state,
            profiles,
            workspace_provider,
            auto_execute: false,
        }
    }

    /// Enable automatic execution for async and stream dispatch modes.
    #[must_use]
    pub const fn with_auto_execute(mut self, auto_execute: bool) -> Self {
        self.auto_execute = auto_execute;
        self
    }

    /// Create a session and optional first run.
    pub async fn create_session(
        &self,
        request: ClawSessionCreateRequest,
    ) -> ClawResult<ClawSessionCreateResponse> {
        let profile = self
            .profiles
            .resolve(request.profile_name.as_deref())
            .await?;
        let session_id = SessionId::from_string(format!("session_{}", Uuid::new_v4()));
        let mut session = SessionRecord::new(session_id.clone());
        session.profile = Some(profile.name.clone());
        session.metadata = metadata_with_workspace(request.metadata, request.workspace.as_ref())?;
        self.store.save_session(session.clone()).await?;

        let run = if request.input_parts.is_empty() {
            None
        } else {
            Some(
                self.create_run(ClawRunCreateRequest {
                    session_id: Some(session_id.as_str().to_string()),
                    restore_from_run_id: None,
                    reset_state: false,
                    profile_name: Some(profile.name),
                    input_parts: request.input_parts,
                    trigger_type: request.trigger_type,
                    metadata: Metadata::default(),
                    workspace: request.workspace,
                    dispatch_mode: request.dispatch_mode,
                })
                .await?,
            )
        };
        let session = self.store.load_session(&session_id).await?;
        Ok(ClawSessionCreateResponse {
            session: session_summary(&session),
            run,
        })
    }

    /// List sessions.
    pub async fn list_sessions(&self, limit: Option<usize>) -> ClawResult<Vec<ClawSessionSummary>> {
        let sessions = self
            .store
            .list_sessions(SessionFilter {
                limit,
                ..SessionFilter::default()
            })
            .await?;
        Ok(sessions.iter().map(session_summary).collect())
    }

    /// Get a session detail.
    pub async fn get_session(&self, session_id: &str) -> ClawResult<ClawSessionGetResponse> {
        let session_id = SessionId::from_string(session_id.to_string());
        let session = self.store.load_session(&session_id).await?;
        let runs = self.store.list_runs(&session_id).await?;
        let recent_runs = runs
            .iter()
            .rev()
            .take(20)
            .map(|run| run_detail(run, false))
            .collect::<Vec<_>>();
        let state = serde_json::to_value(&session.state)?;
        Ok(ClawSessionGetResponse {
            session: session_summary(&session),
            recent_runs,
            state,
        })
    }

    /// Create a run under an existing session.
    pub async fn create_session_run(
        &self,
        session_id: &str,
        request: ClawSessionRunCreateRequest,
    ) -> ClawResult<ClawRunDetail> {
        let lock = self.runtime_state.session_lock(session_id).await;
        let _guard = lock.lock().await;
        let session = self
            .store
            .load_session(&SessionId::from_string(session_id.to_string()))
            .await?;
        self.ensure_session_can_accept_run(&session).await?;
        self.create_run(ClawRunCreateRequest {
            session_id: Some(session_id.to_string()),
            restore_from_run_id: request.restore_from_run_id,
            reset_state: request.reset_state,
            profile_name: session.profile,
            input_parts: request.input_parts,
            trigger_type: request.trigger_type,
            metadata: request.metadata,
            workspace: request.workspace,
            dispatch_mode: request.dispatch_mode,
        })
        .await
    }

    /// Fork a session lineage.
    pub async fn fork_session(
        &self,
        session_id: &str,
        request: ClawSessionForkRequest,
    ) -> ClawResult<ClawSessionSummary> {
        let source_id = SessionId::from_string(session_id.to_string());
        let source = self.store.load_session(&source_id).await?;
        let fork_id = SessionId::from_string(format!("session_{}", Uuid::new_v4()));
        let mut fork = SessionRecord::new(fork_id.clone());
        fork.parent_session_id = Some(source_id.clone());
        fork.profile = request.profile_name.or(source.profile);
        fork.head_success_run_id = request.restore_from_run_id.map(RunId::from_string);
        fork.metadata = metadata_with_workspace(request.metadata, request.workspace.as_ref())?;
        self.store.save_session(fork.clone()).await?;
        Ok(session_summary(&fork))
    }

    /// Create a queued run directly.
    pub async fn create_run(&self, request: ClawRunCreateRequest) -> ClawResult<ClawRunDetail> {
        if request.reset_state && request.restore_from_run_id.is_some() {
            return Err(ClawError::InvalidRequest(
                "reset_state and restore_from_run_id cannot be used together".to_string(),
            ));
        }

        let (session_id, mut session) = if let Some(session_id) = request.session_id.as_ref() {
            let session_id = SessionId::from_string(session_id.clone());
            let session = self.store.load_session(&session_id).await?;
            self.ensure_session_can_accept_run(&session).await?;
            (session_id, session)
        } else {
            let session_id = SessionId::from_string(format!("session_{}", Uuid::new_v4()));
            let mut session = SessionRecord::new(session_id.clone());
            session.profile = request.profile_name.clone();
            session.metadata =
                metadata_with_workspace(Metadata::default(), request.workspace.as_ref())?;
            self.store.save_session(session.clone()).await?;
            (session_id, session)
        };

        let resolved_profile = self
            .profiles
            .resolve(
                request
                    .profile_name
                    .as_deref()
                    .or(session.profile.as_deref()),
            )
            .await?;
        session.profile = Some(resolved_profile.name.clone());
        self.store.save_session(session.clone()).await?;

        let restore_from_run_id = if request.reset_state {
            None
        } else {
            request.restore_from_run_id.or_else(|| {
                session
                    .head_success_run_id
                    .as_ref()
                    .map(|id| id.as_str().to_string())
            })
        };
        if let Some(restore_from_run_id) = restore_from_run_id.as_ref() {
            self.store
                .load_run(
                    &session_id,
                    &RunId::from_string(restore_from_run_id.clone()),
                )
                .await?;
        }

        let sequence_no = self.store.list_runs(&session_id).await?.len() + 1;
        let run_id = RunId::new();
        let mut run = RunRecord::new(session_id.clone(), run_id.clone(), ConversationId::new());
        run.sequence_no = sequence_no;
        run.restore_from_run_id = restore_from_run_id.map(RunId::from_string);
        run.trigger_type = Some(request.trigger_type.as_str().to_string());
        run.profile = Some(resolved_profile.name);
        run.input = request
            .input_parts
            .clone()
            .into_iter()
            .map(ClawInputPart::into_session_input)
            .collect();
        run.metadata = metadata_with_workspace(request.metadata, request.workspace.as_ref())?;
        run.output_preview = Some(input_preview(&request.input_parts));
        self.workspace_provider.resolve(request.workspace)?;
        self.store.append_run(run.clone()).await?;
        self.runtime_state
            .register_run(
                session_id.as_str().to_string(),
                run_id.as_str().to_string(),
                request.dispatch_mode.as_str().to_string(),
            )
            .await;
        self.append_event(
            ReplayScope::run(run_id.as_str()),
            1,
            ReplayEventKind::Raw(json!({
                "type": "run.queued",
                "run_id": run_id.as_str(),
                "session_id": session_id.as_str(),
                "status": "queued",
                "sequence_no": sequence_no,
                "dispatch_mode": request.dispatch_mode.as_str(),
            })),
        )
        .await?;
        self.append_event(
            ReplayScope::session(session_id.as_str()),
            sequence_no,
            ReplayEventKind::Raw(json!({
                "type": "session.run_queued",
                "run_id": run_id.as_str(),
                "session_id": session_id.as_str(),
                "sequence_no": sequence_no,
            })),
        )
        .await?;

        if self.auto_execute && request.dispatch_mode != DispatchMode::Queue {
            let supervisor = self.execution_supervisor();
            let run_to_execute = run.clone();
            tokio::spawn(async move {
                let _result = supervisor.execute_run(run_to_execute).await;
            });
        }

        Ok(run_detail(&run, true))
    }

    /// Get run detail.
    pub async fn get_run(&self, run_id: &str) -> ClawResult<ClawRunDetail> {
        let run = self.find_run(run_id).await?;
        Ok(run_detail(&run, true))
    }

    /// Get compact run trace.
    pub async fn run_trace(&self, run_id: &str) -> ClawResult<starweaver_session::CompactRunTrace> {
        let run = self.find_run(run_id).await?;
        Ok(self
            .store
            .compact_run_trace(&run.session_id, &run.run_id)
            .await?)
    }

    /// List completed conversational turns for a session.
    pub async fn session_turns(&self, session_id: &str) -> ClawResult<Vec<ClawRunDetail>> {
        let session_id = SessionId::from_string(session_id.to_string());
        let runs = self.store.list_runs(&session_id).await?;
        Ok(runs
            .iter()
            .filter(|run| run.status == RunStatus::Completed)
            .map(|run| run_detail(run, true))
            .collect())
    }

    /// Return session workspace state projection.
    pub async fn session_workspace(&self, session_id: &str) -> ClawResult<serde_json::Value> {
        let session = self
            .store
            .load_session(&SessionId::from_string(session_id.to_string()))
            .await?;
        Ok(json!({
            "session_id": session_id,
            "workspace": session.metadata.get("workspace").cloned(),
            "environment_state": session.environment_state,
        }))
    }

    /// Return session sandbox state projection.
    pub async fn session_sandbox(&self, session_id: &str) -> ClawResult<serde_json::Value> {
        let workspace = self.session_workspace(session_id).await?;
        Ok(json!({
            "session_id": session_id,
            "sandbox_state": {
                "backend": self.workspace_provider.runtime_status().backend,
                "ready_state": "not_started",
                "workspace": workspace.get("workspace").cloned().unwrap_or(serde_json::Value::Null),
            }
        }))
    }

    /// Steer an active run.
    pub async fn steer_run(
        &self,
        run_id: &str,
        request: SteerRequest,
    ) -> ClawResult<ClawRunDetail> {
        let run = self.find_run(run_id).await?;
        if !matches!(
            run.status,
            RunStatus::Queued | RunStatus::Running | RunStatus::Waiting
        ) {
            return Err(ClawError::Conflict(format!(
                "run '{run_id}' cannot receive steering input in status {:?}",
                run.status
            )));
        }
        let payload = request
            .input_parts
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()?;
        self.runtime_state.record_steering(run_id, payload).await;
        self.append_event(
            ReplayScope::run(run_id),
            next_event_sequence(),
            ReplayEventKind::Raw(json!({
                "type": "run.steered",
                "run_id": run_id,
                "input_preview": input_preview(&request.input_parts),
            })),
        )
        .await?;
        Ok(run_detail(&run, true))
    }

    /// Interrupt an active run.
    pub async fn interrupt_run(&self, run_id: &str) -> ClawResult<ClawRunDetail> {
        self.stop_run(run_id, RunStatus::Cancelled, "interrupt")
            .await
    }

    /// Cancel an active run.
    pub async fn cancel_run(&self, run_id: &str) -> ClawResult<ClawRunDetail> {
        self.stop_run(run_id, RunStatus::Cancelled, "cancel").await
    }

    /// Replay events for a run.
    pub async fn run_events(
        &self,
        run_id: &str,
        cursor: Option<usize>,
    ) -> ClawResult<EventListResponse> {
        let scope = ReplayScope::run(run_id);
        let cursor = cursor.map(|sequence| ReplayCursor::new(scope.clone(), sequence));
        let events = self.events.replay_after(&scope, cursor, None).await?;
        Ok(EventListResponse {
            scope: scope.as_str().to_string(),
            events,
        })
    }

    /// Replay events for a session.
    pub async fn session_events(
        &self,
        session_id: &str,
        cursor: Option<usize>,
    ) -> ClawResult<EventListResponse> {
        let scope = ReplayScope::session(session_id);
        let cursor = cursor.map(|sequence| ReplayCursor::new(scope.clone(), sequence));
        let events = self.events.replay_after(&scope, cursor, None).await?;
        Ok(EventListResponse {
            scope: scope.as_str().to_string(),
            events,
        })
    }

    /// Build an execution supervisor for this controller.
    #[must_use]
    pub fn execution_supervisor(&self) -> ExecutionSupervisor {
        ExecutionSupervisor::new(
            self.store.clone(),
            self.events.clone(),
            self.runtime_state.clone(),
            Arc::new(NoopRunExecutor),
        )
    }

    /// Recover and dispatch queued runs through the execution supervisor.
    pub async fn recover_queued_runs(&self) -> ClawResult<Vec<String>> {
        self.execution_supervisor().recover_queued_runs().await
    }

    /// List profiles.
    pub async fn list_profiles(&self) -> ProfileListResponse {
        ProfileListResponse {
            profiles: self.profiles.list().await,
        }
    }

    /// Get one profile.
    pub async fn get_profile(&self, name: &str) -> ClawResult<AgentProfile> {
        self.profiles
            .get(name)
            .await
            .ok_or_else(|| ClawError::NotFound(format!("profile '{name}' was not found")))
    }

    /// Upsert one profile.
    pub async fn upsert_profile(&self, profile: AgentProfile) -> AgentProfile {
        self.profiles.upsert(profile).await
    }

    /// Delete one profile.
    pub async fn delete_profile(&self, name: &str) -> ClawResult<()> {
        if self.profiles.delete(name).await {
            Ok(())
        } else {
            Err(ClawError::NotFound(format!(
                "profile '{name}' was not found"
            )))
        }
    }

    /// Resolve workspace binding.
    pub fn resolve_workspace(
        &self,
        workspace: Option<WorkspaceBindingSpec>,
    ) -> ClawResult<WorkspaceResolveResponse> {
        Ok(WorkspaceResolveResponse {
            workspace: self.workspace_provider.resolve(workspace)?,
        })
    }

    async fn ensure_session_can_accept_run(&self, session: &SessionRecord) -> ClawResult<()> {
        if let Some(active_run_id) = session.active_run_id.as_ref() {
            let run = self
                .store
                .load_run(&session.session_id, active_run_id)
                .await?;
            if matches!(
                run.status,
                RunStatus::Queued | RunStatus::Running | RunStatus::Waiting
            ) {
                return Err(ClawError::Conflict(format!(
                    "session '{}' already has an active run '{}'",
                    session.session_id.as_str(),
                    active_run_id.as_str()
                )));
            }
        }
        Ok(())
    }

    async fn find_run(&self, run_id: &str) -> ClawResult<RunRecord> {
        let sessions = self.store.list_sessions(SessionFilter::default()).await?;
        for session in sessions {
            let run_id = RunId::from_string(run_id.to_string());
            if let Ok(run) = self.store.load_run(&session.session_id, &run_id).await {
                return Ok(run);
            }
        }
        Err(ClawError::NotFound(format!("run '{run_id}' was not found")))
    }

    async fn stop_run(
        &self,
        run_id: &str,
        status: RunStatus,
        reason: &str,
    ) -> ClawResult<ClawRunDetail> {
        let run = self.find_run(run_id).await?;
        self.runtime_state.request_stop(run_id, reason).await;
        self.store
            .update_run_status(
                &run.session_id,
                &run.run_id,
                status,
                Some(format!("Run requested to {reason}")),
            )
            .await?;
        self.runtime_state.close_run(run_id).await;
        let terminal = match status {
            RunStatus::Completed => StreamTerminalMarker::RunCompleted,
            RunStatus::Failed => StreamTerminalMarker::RunFailed {
                code: reason.to_string(),
                message: format!("run failed: {reason}"),
            },
            RunStatus::Cancelled => StreamTerminalMarker::RunCancelled {
                reason: reason.to_string(),
            },
            RunStatus::Queued | RunStatus::Running | RunStatus::Waiting => {
                StreamTerminalMarker::RunCancelled {
                    reason: reason.to_string(),
                }
            }
        };
        self.append_event(
            ReplayScope::run(run_id),
            next_event_sequence(),
            ReplayEventKind::Terminal(terminal),
        )
        .await?;
        let updated = self.find_run(run_id).await?;
        Ok(run_detail(&updated, true))
    }

    async fn append_event(
        &self,
        scope: ReplayScope,
        sequence: usize,
        kind: ReplayEventKind,
    ) -> ClawResult<()> {
        let event = ReplayEvent::new(scope.clone(), sequence, kind);
        self.events.append(scope, event).await?;
        Ok(())
    }
}

fn session_summary(session: &SessionRecord) -> ClawSessionSummary {
    ClawSessionSummary {
        id: session.session_id.as_str().to_string(),
        profile_name: session.profile.clone(),
        head_run_id: session
            .head_run_id
            .as_ref()
            .map(|id| id.as_str().to_string()),
        head_success_run_id: session
            .head_success_run_id
            .as_ref()
            .map(|id| id.as_str().to_string()),
        active_run_id: session
            .active_run_id
            .as_ref()
            .map(|id| id.as_str().to_string()),
        metadata: session.metadata.clone(),
        created_at: session.created_at,
        updated_at: session.updated_at,
    }
}

fn run_detail(run: &RunRecord, include_input_parts: bool) -> ClawRunDetail {
    ClawRunDetail {
        id: run.run_id.as_str().to_string(),
        session_id: run.session_id.as_str().to_string(),
        sequence_no: run.sequence_no,
        restore_from_run_id: run
            .restore_from_run_id
            .as_ref()
            .map(|id| id.as_str().to_string()),
        status: run.status,
        trigger_type: run.trigger_type.clone(),
        profile_name: run.profile.clone(),
        input_preview: run.output_preview.clone(),
        input_parts: include_input_parts.then(|| run.input.clone()),
        output_text: run.output_preview.clone(),
        created_at: run.created_at,
        updated_at: run.updated_at,
        metadata: run.metadata.clone(),
    }
}

fn metadata_with_workspace(
    mut metadata: Metadata,
    workspace: Option<&WorkspaceBindingSpec>,
) -> ClawResult<Metadata> {
    if let Some(workspace) = workspace {
        metadata.insert("workspace".to_string(), serde_json::to_value(workspace)?);
    }
    Ok(metadata)
}

fn input_preview(input_parts: &[ClawInputPart]) -> String {
    input_parts
        .iter()
        .map(ClawInputPart::preview)
        .collect::<Vec<_>>()
        .join("\n")
}

fn next_event_sequence() -> usize {
    Utc::now().timestamp_micros().unsigned_abs() as usize
}

#[cfg(test)]
mod tests {
    use starweaver_session::InMemorySessionStore;
    use starweaver_stream::InMemoryReplayEventLog;

    use super::*;
    use crate::{ClawSettings, WorkspaceProvider};

    #[tokio::test]
    async fn creates_session_with_queued_run() {
        let settings = ClawSettings::default();
        let profiles = Arc::new(ProfileResolver::new(&settings));
        let controller = ClawController::new(
            Arc::new(InMemorySessionStore::new()),
            Arc::new(InMemoryReplayEventLog::new()),
            ClawRuntimeState::new(),
            profiles,
            WorkspaceProvider::new(settings),
        );
        let response = controller
            .create_session(ClawSessionCreateRequest {
                input_parts: vec![ClawInputPart::Text {
                    text: "hello".to_string(),
                    metadata: None,
                }],
                ..ClawSessionCreateRequest::default()
            })
            .await
            .expect("session creates");
        assert!(response.run.is_some());
        assert_eq!(response.session.profile_name.as_deref(), Some("default"));
    }
}
