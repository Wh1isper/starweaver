//! HTTP service surface for Starweaver Claw.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, Method, StatusCode, Uri},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use starweaver_session::InMemorySessionStore;
use starweaver_stream::InMemoryReplayEventLog;

use crate::{
    api::{
        dto::{ClawInfoResponse, HealthResponse},
        sse::{events_response, ready_sse_response},
    },
    controller::{
        ClawRunCreateRequest, ClawSessionCreateRequest, ClawSessionForkRequest,
        ClawSessionRunCreateRequest, ClawSessionSummary, SteerRequest, WorkspaceResolveResponse,
    },
    orchestration::{
        HeartbeatStatus, OrchestrationCatalog, ScheduleCreateRequest, ScheduleRecord,
        WorkflowDefinitionCreateRequest, WorkflowDefinitionRecord, WorkflowRunRecord,
        WorkflowTriggerRequest,
    },
    profile::{AgentProfile, ProfileResolver},
    storage::{SqliteReplayEventLog, SqliteSessionStore},
    web_assets,
    workspace::{WorkspaceProvider, WorkspaceRuntimeStatus},
    ClawController, ClawError, ClawResult, ClawRuntimeState, ClawSettings, WorkspaceBindingSpec,
};

/// Shared HTTP application state.
#[derive(Clone)]
pub struct AppState {
    settings: ClawSettings,
    controller: ClawController,
    workspace_provider: WorkspaceProvider,
    orchestration: OrchestrationCatalog,
    storage_kind: &'static str,
}

impl AppState {
    /// Build service state from settings with SQLite-backed durable storage.
    ///
    /// # Errors
    ///
    /// Returns storage initialization or profile seed errors.
    pub async fn durable(settings: ClawSettings) -> ClawResult<Self> {
        let store = Arc::new(SqliteSessionStore::open(&settings.sqlite_path)?);
        let events = Arc::new(SqliteReplayEventLog::open(&settings.sqlite_path)?);
        let runtime_state = ClawRuntimeState::new();
        let profiles = Arc::new(ProfileResolver::new(&settings));
        if settings.auto_seed_profiles {
            if let Some(path) = settings.profile_seed_file.as_ref() {
                profiles.seed_yaml_file(path).await?;
            }
        }
        let workspace_provider = WorkspaceProvider::new(settings.clone());
        let controller = ClawController::new(
            store,
            events,
            runtime_state,
            profiles,
            workspace_provider.clone(),
        )
        .with_auto_execute(true);
        Ok(Self {
            settings,
            controller,
            workspace_provider,
            orchestration: OrchestrationCatalog::default(),
            storage_kind: "sqlite",
        })
    }

    /// Build service state from settings with in-memory runtime adapters.
    #[must_use]
    pub fn in_memory(settings: ClawSettings) -> Self {
        let store = Arc::new(InMemorySessionStore::new());
        let events = Arc::new(InMemoryReplayEventLog::new());
        let runtime_state = ClawRuntimeState::new();
        let profiles = Arc::new(ProfileResolver::new(&settings));
        let workspace_provider = WorkspaceProvider::new(settings.clone());
        let controller = ClawController::new(
            store,
            events,
            runtime_state,
            profiles,
            workspace_provider.clone(),
        );
        Self {
            settings,
            controller,
            workspace_provider,
            orchestration: OrchestrationCatalog::default(),
            storage_kind: "memory",
        }
    }

    /// Return controller reference.
    #[must_use]
    pub const fn controller(&self) -> &ClawController {
        &self.controller
    }

    /// Return settings reference.
    #[must_use]
    pub const fn settings(&self) -> &ClawSettings {
        &self.settings
    }
}

/// Build the Claw HTTP router.
#[must_use]
pub fn build_router(settings: ClawSettings) -> Router {
    build_router_with_state(AppState::in_memory(settings))
}

/// Build the Claw HTTP router from explicit state.
#[must_use]
pub fn build_router_with_state(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/healthz", get(healthz))
        .route("/api/v1/claw/info", get(claw_info))
        .route("/api/v1/info", get(claw_info))
        .route("/api/v1/claw/notifications", get(notifications))
        .route("/api/v1/notifications", get(notifications))
        .route("/api/v1/profiles", get(list_profiles).post(upsert_profile))
        .route("/api/v1/profiles/seed", post(seed_profiles))
        .route("/api/v1/profiles:seed", post(seed_profiles))
        .route(
            "/api/v1/profiles/:profile_name",
            get(get_profile).put(put_profile).delete(delete_profile),
        )
        .route("/api/v1/workspace/runtime", get(workspace_runtime))
        .route("/api/v1/workspace:resolve", post(resolve_workspace))
        .route("/api/v1/sessions", get(list_sessions).post(create_session))
        .route("/api/v1/sessions:stream", post(create_session_stream))
        .route("/api/v1/sessions/:session_id", get(get_session))
        .route("/api/v1/sessions/:session_id/turns", get(session_turns))
        .route(
            "/api/v1/sessions/:session_id/workspace",
            get(session_workspace),
        )
        .route("/api/v1/sessions/:session_id/sandbox", get(session_sandbox))
        .route(
            "/api/v1/sessions/:session_id/sandbox/prepare",
            post(prepare_session_sandbox),
        )
        .route(
            "/api/v1/sessions/:session_id/sandbox/stop",
            post(stop_session_sandbox),
        )
        .route(
            "/api/v1/sessions/:session_id/runs",
            post(create_session_run),
        )
        .route(
            "/api/v1/sessions/:session_id/submit",
            post(submit_session_input),
        )
        .route(
            "/api/v1/sessions/:session_id/runs:stream",
            post(create_session_run_stream),
        )
        .route("/api/v1/sessions/:session_id/steer", post(steer_session))
        .route(
            "/api/v1/sessions/:session_id/interrupt",
            post(interrupt_session),
        )
        .route("/api/v1/sessions/:session_id/cancel", post(cancel_session))
        .route("/api/v1/sessions/:session_id/fork", post(fork_session))
        .route("/api/v1/sessions/:session_id/events", get(session_events))
        .route(
            "/api/v1/sessions/:session_id/async-tasks",
            get(list_session_async_tasks),
        )
        .route(
            "/api/v1/sessions/:session_id/async-tasks:spawn",
            post(spawn_session_async_task),
        )
        .route(
            "/api/v1/sessions/:session_id/async-tasks/:task_id_or_name",
            get(get_session_async_task).post(session_async_task_action),
        )
        .route(
            "/api/v1/sessions/:session_id/async-tasks/:task_id_or_name/cancel",
            post(cancel_session_async_task),
        )
        .route(
            "/api/v1/sessions/:session_id/async-tasks/:task_id_or_name/steer",
            post(steer_session_async_task),
        )
        .route("/api/v1/runs", post(create_run))
        .route("/api/v1/runs:stream", post(create_run_stream))
        .route("/api/v1/runs/:run_id", get(get_run))
        .route("/api/v1/runs/:run_id/trace", get(run_trace))
        .route("/api/v1/runs/:run_id/steer", post(steer_run))
        .route("/api/v1/runs/:run_id/interrupt", post(interrupt_run))
        .route("/api/v1/runs/:run_id/cancel", post(cancel_run))
        .route("/api/v1/runs/:run_id/events", get(run_events))
        .route(
            "/api/v1/workflows",
            get(list_workflows).post(create_workflow),
        )
        .route(
            "/api/v1/workflows/:workflow_id",
            get(get_workflow)
                .patch(update_workflow)
                .post(workflow_action),
        )
        .route(
            "/api/v1/workflows/:workflow_id/trigger",
            post(trigger_workflow),
        )
        .route(
            "/api/v1/workflows/:workflow_id/archive",
            post(archive_workflow),
        )
        .route("/api/v1/agent/workflows", post(create_workflow))
        .route("/api/v1/workflow-runs", get(list_workflow_runs))
        .route(
            "/api/v1/workflow-runs/:workflow_run_id",
            get(get_workflow_run),
        )
        .route(
            "/api/v1/workflow-runs/:workflow_run_id/events",
            get(list_workflow_events),
        )
        .route(
            "/api/v1/workflow-runs/:workflow_run_id/cancel",
            post(cancel_workflow_run),
        )
        .route(
            "/api/v1/workflow-runs/:workflow_run_id/nodes/:node_id/steer",
            post(steer_workflow_node),
        )
        .route(
            "/api/v1/schedules",
            get(list_schedules).post(create_schedule),
        )
        .route(
            "/api/v1/schedules/:schedule_id",
            get(get_schedule)
                .patch(update_schedule)
                .delete(delete_schedule)
                .post(schedule_action),
        )
        .route(
            "/api/v1/schedules/:schedule_id/fires",
            get(list_schedule_fires),
        )
        .route(
            "/api/v1/schedules/:schedule_id/trigger",
            post(trigger_schedule),
        )
        .route("/api/v1/schedules/:schedule_id/pause", post(pause_schedule))
        .route(
            "/api/v1/schedules/:schedule_id/resume",
            post(resume_schedule),
        )
        .route("/api/v1/heartbeat", get(heartbeat_status))
        .route("/api/v1/heartbeat/config", get(heartbeat_config))
        .route("/api/v1/heartbeat/status", get(heartbeat_status))
        .route("/api/v1/heartbeat/fires", get(heartbeat_fires))
        .route("/api/v1/heartbeat:trigger", post(trigger_heartbeat))
        .route(
            "/api/v1/bridges/conversations",
            get(list_bridge_conversations),
        )
        .route("/api/v1/bridges/events", get(list_bridge_events))
        .route(
            "/api/v1/bridges/inbound/messages",
            post(bridge_inbound_message),
        )
        .route(
            "/api/v1/bridges/inbound/actions",
            post(bridge_inbound_action),
        )
        .route("/api/v1/bridges/:adapter/events", post(bridge_ingress))
        .route("/api/v1/agency/config", get(agency_config))
        .route("/api/v1/agency/status", get(agency_status))
        .route("/api/v1/agency/fires", get(agency_fires))
        .route(
            "/api/v1/agency/source-session:submit",
            post(submit_agency_source_session),
        )
        .fallback(compat_or_frontend)
        .with_state(state)
}

/// Serve the Claw HTTP router until process shutdown.
///
/// # Errors
///
/// Returns bind or server errors from the underlying TCP listener.
pub async fn serve(settings: ClawSettings) -> ClawResult<()> {
    settings.ensure_dirs()?;
    let addr = settings.socket_addr()?;
    serve_addr(settings, addr).await
}

/// Serve the Claw HTTP router on a specific address.
///
/// # Errors
///
/// Returns bind or server errors from the underlying TCP listener.
pub async fn serve_addr(settings: ClawSettings, addr: SocketAddr) -> ClawResult<()> {
    let state = AppState::durable(settings).await?;
    state.controller.recover_queued_runs().await?;
    let router = build_router_with_state(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|error| ClawError::Failed(error.to_string()))
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

async fn healthz(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        storage: state.storage_kind,
        runtime: "single_node",
        database: "ok",
        runtime_state: "ok",
    })
}

async fn claw_info(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ClawResult<Json<ClawInfoResponse>> {
    authorize(&state, &headers)?;
    Ok(Json(ClawInfoResponse {
        name: state.settings.app_name.clone(),
        app_name: state.settings.app_name.clone(),
        version: env!("CARGO_PKG_VERSION"),
        service_version: option_env!("STARWEAVER_CLAW_SERVICE_VERSION")
            .unwrap_or(env!("CARGO_PKG_VERSION"))
            .to_string(),
        service_commit: option_env!("STARWEAVER_CLAW_SERVICE_COMMIT")
            .or(option_env!("GITHUB_SHA"))
            .map(ToOwned::to_owned),
        service_revision: option_env!("STARWEAVER_CLAW_SERVICE_VERSION")
            .unwrap_or(env!("CARGO_PKG_VERSION"))
            .to_string(),
        service_build: option_env!("STARWEAVER_CLAW_SERVICE_BUILD")
            .or(option_env!("STARWEAVER_BUILD"))
            .map(ToOwned::to_owned),
        service_image: option_env!("STARWEAVER_CLAW_SERVICE_IMAGE")
            .or(option_env!("STARWEAVER_IMAGE"))
            .map(ToOwned::to_owned),
        environment: state.settings.environment.clone(),
        public_base_url: state.settings.public_base_url.clone(),
        instance_id: "single_node".to_string(),
        auth: "bearer",
        surfaces: vec![
            "profiles",
            "sessions",
            "runs",
            "schedules",
            "workflows",
            "bridges",
            "heartbeat",
            "agency",
            "notifications",
        ],
        workspace_provider_backend: format!("{:?}", state.settings.workspace_backend)
            .to_ascii_lowercase(),
        storage_model: state.storage_kind,
        features: json!({
            "session_events": true,
            "run_events": true,
            "notifications": true,
            "notification_replay": true,
            "profiles": true,
            "schedules": state.settings.schedule_dispatch_enabled,
            "heartbeat": state.settings.heartbeat_enabled,
            "workflows": state.settings.workflow_dispatch_enabled,
            "web_console": web_assets::is_available(),
        }),
        capabilities: json!({
            "sessions": true,
            "runs": true,
            "profiles": true,
            "workspace": true,
            "events": true,
            "sse": true,
            "workflows": "contract",
            "schedules": "contract",
            "heartbeat": state.settings.heartbeat_enabled,
            "storage": state.storage_kind,
            "workspace_backend": state.settings.workspace_backend,
            "web_console": web_assets::is_available(),
        }),
    }))
}

async fn list_profiles(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ClawResult<Json<Vec<AgentProfile>>> {
    authorize(&state, &headers)?;
    Ok(Json(state.controller.list_profiles().await.profiles))
}

async fn upsert_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(profile): Json<AgentProfile>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(state.controller.upsert_profile(profile).await))
}

async fn get_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(profile_name): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(profile_detail_json(
        state.controller.get_profile(&profile_name).await?,
    )))
}

async fn put_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(profile_name): Path<String>,
    Json(payload): Json<Value>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    let profile = profile_from_payload(&profile_name, payload)?;
    Ok(Json(profile_detail_json(
        state.controller.upsert_profile(profile).await,
    )))
}

async fn delete_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(profile_name): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    state.controller.delete_profile(&profile_name).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn seed_profiles(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    let profiles = state.controller.list_profiles().await.profiles;
    let seeded_names = profiles
        .iter()
        .map(|profile| profile.name.clone())
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "profiles": profiles,
        "seeded_names": seeded_names,
        "seeded": true,
        "seed_file": state.settings.profile_seed_file.as_ref().map(|path| path.display().to_string()).unwrap_or_default(),
        "prune_missing": false,
    })))
}

async fn workspace_runtime(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ClawResult<Json<WorkspaceRuntimeStatus>> {
    authorize(&state, &headers)?;
    Ok(Json(state.workspace_provider.runtime_status()))
}

async fn resolve_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Option<WorkspaceBindingSpec>>,
) -> ClawResult<Json<WorkspaceResolveResponse>> {
    authorize(&state, &headers)?;
    Ok(Json(state.controller.resolve_workspace(request)?))
}

#[derive(Debug, Deserialize)]
struct ListSessionsQuery {
    limit: Option<usize>,
}

async fn list_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListSessionsQuery>,
) -> ClawResult<Json<Vec<ClawSessionSummary>>> {
    authorize(&state, &headers)?;
    Ok(Json(state.controller.list_sessions(query.limit).await?))
}

async fn create_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ClawSessionCreateRequest>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok((
        StatusCode::CREATED,
        Json(state.controller.create_session(request).await?),
    ))
}

async fn get_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(state.controller.get_session(&session_id).await?))
}

async fn session_turns(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(json!({
        "session_id": session_id,
        "turns": state.controller.session_turns(&session_id).await?
    })))
}

async fn session_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(state.controller.session_workspace(&session_id).await?))
}

async fn session_sandbox(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(state.controller.session_sandbox(&session_id).await?))
}

async fn fork_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(request): Json<ClawSessionForkRequest>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "session": state.controller.fork_session(&session_id, request).await?
        })),
    ))
}

async fn prepare_session_sandbox(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(json!({
        "session_id": session_id,
        "sandbox_state": {
            "backend": state.settings.workspace_backend,
            "ready_state": "ready",
            "status": "ready"
        }
    })))
}

async fn stop_session_sandbox(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(json!({
        "session_id": session_id,
        "sandbox_state": {
            "backend": state.settings.workspace_backend,
            "ready_state": "not_started",
            "status": "stopped"
        }
    })))
}

async fn create_session_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(request): Json<ClawSessionRunCreateRequest>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok((
        StatusCode::CREATED,
        Json(
            state
                .controller
                .create_session_run(&session_id, request)
                .await?,
        ),
    ))
}

async fn submit_session_input(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(request): Json<ClawSessionRunCreateRequest>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    let run = state
        .controller
        .create_session_run(&session_id, request)
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "session_id": session_id,
            "run_id": run.id,
            "delivery": "queued",
            "status": run.status,
            "run": run,
        })),
    ))
}

async fn create_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ClawRunCreateRequest>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok((
        StatusCode::CREATED,
        Json(state.controller.create_run(request).await?),
    ))
}

async fn get_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(state.controller.get_run(&run_id).await?))
}

async fn run_trace(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(state.controller.run_trace(&run_id).await?))
}

async fn steer_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
    Json(request): Json<SteerRequest>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(state.controller.steer_run(&run_id, request).await?))
}

async fn interrupt_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(state.controller.interrupt_run(&run_id).await?))
}

async fn cancel_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(state.controller.cancel_run(&run_id).await?))
}

async fn steer_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(request): Json<SteerRequest>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    let session = state.controller.get_session(&session_id).await?;
    let run_id = session
        .session
        .active_run_id
        .ok_or_else(|| ClawError::Conflict(format!("session '{session_id}' has no active run")))?;
    Ok(Json(state.controller.steer_run(&run_id, request).await?))
}

async fn interrupt_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    let session = state.controller.get_session(&session_id).await?;
    let run_id = session
        .session
        .active_run_id
        .ok_or_else(|| ClawError::Conflict(format!("session '{session_id}' has no active run")))?;
    Ok(Json(state.controller.interrupt_run(&run_id).await?))
}

async fn cancel_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    let session = state.controller.get_session(&session_id).await?;
    let run_id = session
        .session
        .active_run_id
        .ok_or_else(|| ClawError::Conflict(format!("session '{session_id}' has no active run")))?;
    Ok(Json(state.controller.cancel_run(&run_id).await?))
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    cursor: Option<usize>,
    stream: Option<bool>,
}

async fn run_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
    Query(query): Query<EventsQuery>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    let events = state.controller.run_events(&run_id, query.cursor).await?;
    Ok(events_response(events, query.stream.unwrap_or(false)))
}

async fn session_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Query(query): Query<EventsQuery>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    let events = state
        .controller
        .session_events(&session_id, query.cursor)
        .await?;
    Ok(events_response(events, query.stream.unwrap_or(false)))
}

async fn list_session_async_tasks(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(state.controller.list_async_tasks(&session_id).await?))
}

async fn get_session_async_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, task_id_or_name)): Path<(String, String)>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(
        state
            .controller
            .get_async_task(&session_id, &task_id_or_name)
            .await?,
    ))
}

async fn spawn_session_async_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, _spawn_action)): Path<(String, String)>,
    Json(payload): Json<Value>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(
            state
                .controller
                .spawn_async_task(&session_id, payload)
                .await?,
        ),
    ))
}

async fn cancel_session_async_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, task_id_or_name)): Path<(String, String)>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(
        state
            .controller
            .cancel_async_task(&session_id, &task_id_or_name)
            .await?,
    ))
}

async fn session_async_task_action(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, task_id_or_name)): Path<(String, String)>,
    Json(payload): Json<Value>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(
        state
            .controller
            .async_task_action(&session_id, &task_id_or_name, payload)
            .await?,
    ))
}

async fn steer_session_async_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, task_id_or_name)): Path<(String, String)>,
    Json(payload): Json<Value>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(
        state
            .controller
            .steer_async_task(&session_id, &task_id_or_name, payload)
            .await?,
    ))
}

async fn create_session_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<ClawSessionCreateRequest>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    request.dispatch_mode = crate::controller::DispatchMode::Stream;
    let response = state.controller.create_session(request).await?;
    let run_id = response.run.as_ref().map(|run| run.id.clone());
    Ok(Json(
        json!({ "session": response.session, "run": response.run, "stream_run_id": run_id }),
    ))
}

async fn create_session_run_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(mut request): Json<ClawSessionRunCreateRequest>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    request.dispatch_mode = crate::controller::DispatchMode::Stream;
    Ok(Json(
        state
            .controller
            .create_session_run(&session_id, request)
            .await?,
    ))
}

async fn create_run_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<ClawRunCreateRequest>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    request.dispatch_mode = crate::controller::DispatchMode::Stream;
    Ok(Json(state.controller.create_run(request).await?))
}

async fn notifications(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<EventsQuery>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    let events = state.controller.notification_events(query.cursor).await?;
    if events.events.is_empty() && query.stream.unwrap_or(false) {
        return Ok(ready_sse_response().into_response());
    }
    Ok(events_response(events, query.stream.unwrap_or(false)))
}

async fn list_workflows(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    let workflows = state
        .orchestration
        .list_workflows()
        .await
        .into_iter()
        .map(workflow_json)
        .collect::<Vec<_>>();
    Ok(Json(json!({ "workflows": workflows })))
}

async fn create_workflow(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<WorkflowDefinitionCreateRequest>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok((
        StatusCode::CREATED,
        Json(workflow_json(
            state.orchestration.create_workflow(request).await?,
        )),
    ))
}

async fn get_workflow(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(workflow_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(workflow_json(
        state.orchestration.get_workflow(&workflow_id).await?,
    )))
}

async fn update_workflow(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(workflow_id): Path<String>,
    Json(patch): Json<serde_json::Map<String, Value>>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(workflow_json(
        state
            .orchestration
            .update_workflow(&workflow_id, patch)
            .await?,
    )))
}

async fn archive_workflow(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(workflow_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    let mut patch = serde_json::Map::new();
    patch.insert("status".to_string(), json!("archived"));
    Ok(Json(workflow_json(
        state
            .orchestration
            .update_workflow(&workflow_id, patch)
            .await?,
    )))
}

async fn trigger_workflow(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(workflow_id): Path<String>,
    Json(request): Json<WorkflowTriggerRequest>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok((
        StatusCode::CREATED,
        Json(workflow_run_json(
            state
                .orchestration
                .trigger_workflow(&workflow_id, request)
                .await?,
        )),
    ))
}

async fn list_workflow_runs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    let runs = state
        .orchestration
        .list_workflow_runs()
        .await
        .into_iter()
        .map(workflow_run_json)
        .collect::<Vec<_>>();
    Ok(Json(json!({ "workflow_runs": runs })))
}

async fn get_workflow_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(workflow_run_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(workflow_run_json(
        state
            .orchestration
            .get_workflow_run(&workflow_run_id)
            .await?,
    )))
}

async fn list_workflow_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(workflow_run_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    state
        .orchestration
        .get_workflow_run(&workflow_run_id)
        .await?;
    Ok(Json(json!({
        "workflow_run_id": workflow_run_id,
        "events": []
    })))
}

async fn cancel_workflow_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(workflow_run_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(workflow_run_json(
        state
            .orchestration
            .get_workflow_run(&workflow_run_id)
            .await?,
    )))
}

async fn steer_workflow_node(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workflow_run_id, node_id)): Path<(String, String)>,
    Json(payload): Json<Value>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    let run = workflow_run_json(
        state
            .orchestration
            .get_workflow_run(&workflow_run_id)
            .await?,
    );
    Ok(Json(json!({
        "workflow_run": run,
        "node_id": node_id,
        "steering": payload,
    })))
}

async fn list_schedules(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    let schedules = state
        .orchestration
        .list_schedules()
        .await
        .into_iter()
        .map(schedule_json)
        .collect::<Vec<_>>();
    Ok(Json(json!({ "schedules": schedules })))
}

async fn get_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(schedule_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(schedule_json(
        state.orchestration.get_schedule(&schedule_id).await?,
    )))
}

async fn update_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(schedule_id): Path<String>,
    Json(patch): Json<serde_json::Map<String, Value>>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(schedule_json(
        state
            .orchestration
            .update_schedule(&schedule_id, patch)
            .await?,
    )))
}

async fn delete_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(schedule_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(schedule_json(
        state.orchestration.delete_schedule(&schedule_id).await?,
    )))
}

async fn list_schedule_fires(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(schedule_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    state.orchestration.get_schedule(&schedule_id).await?;
    Ok(Json(json!({ "fires": [] })))
}

async fn create_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ScheduleCreateRequest>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok((
        StatusCode::CREATED,
        Json(schedule_json(
            state.orchestration.create_schedule(request).await?,
        )),
    ))
}

async fn pause_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(schedule_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    let mut patch = serde_json::Map::new();
    patch.insert("enabled".to_string(), json!(false));
    Ok(Json(schedule_json(
        state
            .orchestration
            .update_schedule(&schedule_id, patch)
            .await?,
    )))
}

async fn resume_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(schedule_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    let mut patch = serde_json::Map::new();
    patch.insert("enabled".to_string(), json!(true));
    Ok(Json(schedule_json(
        state
            .orchestration
            .update_schedule(&schedule_id, patch)
            .await?,
    )))
}

async fn trigger_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(schedule_id): Path<String>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    let schedule = state.orchestration.get_schedule(&schedule_id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "id": format!("schedule_fire_{schedule_id}"),
            "schedule_id": schedule_id,
            "scheduled_at": chrono::Utc::now(),
            "fired_at": chrono::Utc::now(),
            "status": "submitted",
            "run_status": null,
            "input_preview": schedule.metadata.get("prompt").and_then(Value::as_str),
            "metadata": {},
            "created_at": chrono::Utc::now(),
            "updated_at": chrono::Utc::now(),
        })),
    ))
}

async fn heartbeat_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(HeartbeatStatus {
        enabled: state.settings.heartbeat_enabled,
        status: if state.settings.heartbeat_enabled {
            "active".to_string()
        } else {
            "disabled".to_string()
        },
        last_fire_at: None,
        last_run_id: None,
    }))
}

async fn bridge_inbound_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(event): Json<Value>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "adapter": event.get("adapter").and_then(Value::as_str).unwrap_or("default"),
            "status": "received",
            "kind": "message",
            "event": event,
        })),
    ))
}

async fn bridge_inbound_action(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(event): Json<Value>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "adapter": event.get("adapter").and_then(Value::as_str).unwrap_or("default"),
            "status": "received",
            "kind": "action",
            "event": event,
        })),
    ))
}

async fn bridge_ingress(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(adapter): Path<String>,
    Json(event): Json<Value>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "adapter": adapter,
            "status": "received",
            "event": event,
        })),
    ))
}

async fn heartbeat_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(json!({
        "enabled": state.settings.heartbeat_enabled,
        "interval_seconds": 3600,
        "profile_name": state.settings.default_profile,
        "profile_source": "default",
        "prompt": "Heartbeat check",
        "prompt_source": "heartbeat_setting",
        "on_active": "skip",
        "guidance_file": {
            "path": state.settings.workspace_dir.join("HEARTBEAT.md"),
            "exists": false
        },
        "next_fire_at": null
    })))
}

async fn heartbeat_fires(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(json!({ "fires": [] })))
}

async fn trigger_heartbeat(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "id": format!("heartbeat_fire_{}", chrono::Utc::now().timestamp_micros()),
            "scheduled_at": chrono::Utc::now(),
            "fired_at": chrono::Utc::now(),
            "status": "submitted",
            "session_id": null,
            "run_id": null,
            "run_status": null,
            "error_message": null,
            "metadata": {},
            "created_at": chrono::Utc::now(),
            "updated_at": chrono::Utc::now(),
        })),
    ))
}

async fn list_bridge_conversations(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(json!({ "conversations": [] })))
}

async fn list_bridge_events(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(json!({ "events": [] })))
}

async fn agency_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(
        state
            .controller
            .agency_config(Some(&state.settings.default_profile))
            .await,
    ))
}

async fn agency_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(state.controller.agency_status().await))
}

async fn agency_fires(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok(Json(state.controller.agency_fires().await))
}

async fn submit_agency_source_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> ClawResult<impl IntoResponse> {
    authorize(&state, &headers)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(
            state
                .controller
                .submit_agency_source_session(payload)
                .await?,
        ),
    ))
}

async fn workflow_action(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(workflow_id): Path<String>,
    body: Bytes,
) -> ClawResult<axum::response::Response> {
    authorize(&state, &headers)?;
    if let Some(id) = workflow_id.strip_suffix(":archive") {
        let mut patch = serde_json::Map::new();
        patch.insert("status".to_string(), json!("archived"));
        return Ok(Json(workflow_json(
            state.orchestration.update_workflow(id, patch).await?,
        ))
        .into_response());
    }
    if let Some(id) = workflow_id.strip_suffix(":trigger") {
        let request = parse_json_body::<WorkflowTriggerRequest>(&body)?;
        return Ok((
            StatusCode::CREATED,
            Json(workflow_run_json(
                state.orchestration.trigger_workflow(id, request).await?,
            )),
        )
            .into_response());
    }
    Err(ClawError::NotFound(format!(
        "workflow action '{workflow_id}' was not found"
    )))
}

async fn schedule_action(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(schedule_id): Path<String>,
) -> ClawResult<axum::response::Response> {
    authorize(&state, &headers)?;
    if let Some((id, action)) = schedule_id.split_once(':') {
        return legacy_schedule_action(&state, id, action).await;
    }
    Err(ClawError::NotFound(format!(
        "schedule action '{schedule_id}' was not found"
    )))
}

async fn compat_or_frontend(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> axum::response::Response {
    if uri.path().starts_with("/api/") {
        return compat_api_fallback(&state, &headers, method, uri.path(), &body).await;
    }
    if method == Method::GET || method == Method::HEAD {
        web_assets::serve(&uri)
    } else {
        StatusCode::METHOD_NOT_ALLOWED.into_response()
    }
}

async fn compat_api_fallback(
    state: &AppState,
    headers: &HeaderMap,
    method: Method,
    path: &str,
    body: &[u8],
) -> axum::response::Response {
    if let Err(error) = authorize(state, headers) {
        return error.into_response();
    }
    if method == Method::POST {
        if path == "/api/v1/agency:bootstrap" {
            match state.controller.bootstrap_agency().await {
                Ok(response) => return (StatusCode::ACCEPTED, Json(response)).into_response(),
                Err(error) => return error.into_response(),
            }
        }
        if path == "/api/v1/agency:clear" {
            match state.controller.clear_agency().await {
                Ok(response) => return Json(response).into_response(),
                Err(error) => return error.into_response(),
            }
        }
        if let Some(workflow_id) = path
            .strip_prefix("/api/v1/agent/workflows/")
            .and_then(|tail| tail.strip_suffix(":trigger"))
        {
            let request = match parse_json_body::<WorkflowTriggerRequest>(body) {
                Ok(request) => request,
                Err(error) => return error.into_response(),
            };
            match state
                .orchestration
                .trigger_workflow(workflow_id, request)
                .await
            {
                Ok(run) => {
                    return (StatusCode::CREATED, Json(workflow_run_json(run))).into_response();
                }
                Err(error) => return error.into_response(),
            }
        }
        if let Some((schedule_id, action)) = path
            .strip_prefix("/api/v1/schedules/")
            .and_then(|tail| tail.split_once(':'))
        {
            match legacy_schedule_action(state, schedule_id, action).await {
                Ok(response) => return response,
                Err(error) => return error.into_response(),
            }
        }
        if let Some((run_id, interaction_id)) = path
            .strip_prefix("/api/v1/runs/")
            .and_then(|tail| tail.split_once("/interactions/"))
            .and_then(|(run_id, interaction_id)| {
                interaction_id
                    .strip_suffix(":respond")
                    .map(|interaction_id| (run_id, interaction_id))
            })
        {
            let payload = serde_json::from_slice::<Value>(body).unwrap_or_else(|_| json!({}));
            match state
                .controller
                .respond_interaction(run_id, interaction_id, payload)
                .await
            {
                Ok(response) => return Json(response).into_response(),
                Err(error) => return error.into_response(),
            }
        }
        if let Some((session_id, task_and_action)) = path
            .strip_prefix("/api/v1/sessions/")
            .and_then(|tail| tail.split_once("/async-tasks/"))
        {
            if let Some(task_id_or_name) = task_and_action.strip_suffix(":cancel") {
                match state
                    .controller
                    .cancel_async_task(session_id, task_id_or_name)
                    .await
                {
                    Ok(response) => return Json(response).into_response(),
                    Err(error) => return error.into_response(),
                }
            }
            if let Some(task_id_or_name) = task_and_action.strip_suffix(":steer") {
                let payload = serde_json::from_slice::<Value>(body).unwrap_or_else(|_| json!({}));
                match state
                    .controller
                    .steer_async_task(session_id, task_id_or_name, payload)
                    .await
                {
                    Ok(response) => return Json(response).into_response(),
                    Err(error) => return error.into_response(),
                }
            }
        }
        if let Some((session_id, action)) = path
            .strip_prefix("/api/v1/sessions/")
            .and_then(|tail| tail.split_once("/sandbox:"))
        {
            match session_sandbox_action(state, session_id, action).await {
                Ok(response) => return response,
                Err(error) => return error.into_response(),
            }
        }
        if let Some(session_id) = path
            .strip_prefix("/api/v1/sessions/")
            .and_then(|tail| tail.strip_suffix("/memory:extract"))
        {
            match session_memory_action(state, session_id, "extract").await {
                Ok(response) => return response.into_response(),
                Err(error) => return error.into_response(),
            }
        }
        if let Some(session_id) = path
            .strip_prefix("/api/v1/sessions/")
            .and_then(|tail| tail.strip_suffix("/memory:summarize"))
        {
            match session_memory_action(state, session_id, "summary").await {
                Ok(response) => return response.into_response(),
                Err(error) => return error.into_response(),
            }
        }
    }
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "error": "api_route_not_found",
            "method": method.as_str(),
            "path": path,
        })),
    )
        .into_response()
}

async fn session_memory_action(
    state: &AppState,
    session_id: &str,
    kind: &str,
) -> ClawResult<(StatusCode, Json<Value>)> {
    Ok((
        StatusCode::ACCEPTED,
        Json(
            state
                .controller
                .session_memory_action(session_id, kind, json!({}))
                .await?,
        ),
    ))
}

async fn session_sandbox_action(
    state: &AppState,
    session_id: &str,
    action: &str,
) -> ClawResult<axum::response::Response> {
    state.controller.get_session(session_id).await?;
    match action {
        "prepare" => Ok(Json(json!({
            "session_id": session_id,
            "sandbox_state": {
                "backend": state.settings.workspace_backend,
                "ready_state": "ready",
                "status": "ready"
            }
        }))
        .into_response()),
        "stop" => Ok(Json(json!({
            "session_id": session_id,
            "sandbox_state": {
                "backend": state.settings.workspace_backend,
                "ready_state": "not_started",
                "status": "stopped"
            }
        }))
        .into_response()),
        _ => Err(ClawError::NotFound(format!(
            "session sandbox action '{session_id}:{action}' was not found"
        ))),
    }
}

async fn legacy_schedule_action(
    state: &AppState,
    schedule_id: &str,
    action: &str,
) -> ClawResult<axum::response::Response> {
    match action {
        "pause" => {
            let mut patch = serde_json::Map::new();
            patch.insert("enabled".to_string(), json!(false));
            Ok(Json(schedule_json(
                state
                    .orchestration
                    .update_schedule(schedule_id, patch)
                    .await?,
            ))
            .into_response())
        }
        "resume" => {
            let mut patch = serde_json::Map::new();
            patch.insert("enabled".to_string(), json!(true));
            Ok(Json(schedule_json(
                state
                    .orchestration
                    .update_schedule(schedule_id, patch)
                    .await?,
            ))
            .into_response())
        }
        "trigger" => {
            let schedule = state.orchestration.get_schedule(schedule_id).await?;
            Ok((
                StatusCode::ACCEPTED,
                Json(json!({
                    "id": format!("schedule_fire_{schedule_id}"),
                    "schedule_id": schedule_id,
                    "scheduled_at": chrono::Utc::now(),
                    "fired_at": chrono::Utc::now(),
                    "status": "submitted",
                    "run_status": null,
                    "input_preview": schedule.metadata.get("prompt").and_then(Value::as_str),
                    "metadata": {},
                    "created_at": chrono::Utc::now(),
                    "updated_at": chrono::Utc::now(),
                })),
            )
                .into_response())
        }
        _ => Err(ClawError::NotFound(format!(
            "schedule action '{schedule_id}:{action}' was not found"
        ))),
    }
}

fn profile_from_payload(name: &str, mut payload: Value) -> ClawResult<AgentProfile> {
    let object = payload.as_object_mut().ok_or_else(|| {
        ClawError::InvalidRequest("profile payload must be an object".to_string())
    })?;
    object.insert("name".to_string(), Value::String(name.to_string()));
    if !object.contains_key("builtin_toolsets") {
        if let Some(toolsets) = object.get("toolsets").cloned() {
            object.insert("builtin_toolsets".to_string(), toolsets);
        }
    }
    serde_json::from_value(payload).map_err(ClawError::from)
}

fn profile_detail_json(profile: AgentProfile) -> Value {
    let mut value = serde_json::to_value(&profile).unwrap_or_else(|_| json!({}));
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "toolsets".to_string(),
            object
                .get("builtin_toolsets")
                .cloned()
                .unwrap_or_else(|| json!([])),
        );
        object
            .entry("created_at".to_string())
            .or_insert(json!(profile.created_at));
    }
    value
}

fn workflow_json(workflow: WorkflowDefinitionRecord) -> Value {
    let mut value = serde_json::to_value(&workflow).unwrap_or_else(|_| json!({}));
    if let Some(object) = value.as_object_mut() {
        object.insert("owner_kind".to_string(), json!("api"));
        object.insert("owner_session_id".to_string(), Value::Null);
        object.insert("owner_run_id".to_string(), Value::Null);
        object.insert("latest_run".to_string(), Value::Null);
    }
    value
}

fn workflow_run_json(run: WorkflowRunRecord) -> Value {
    let mut value = serde_json::to_value(&run).unwrap_or_else(|_| json!({}));
    if let Some(object) = value.as_object_mut() {
        object.insert("workflow_name".to_string(), Value::Null);
        object.insert("definition".to_string(), json!(run.definition_snapshot));
        object.insert("nodes".to_string(), json!([]));
        object.insert("events".to_string(), json!([]));
    }
    value
}

fn schedule_json(schedule: ScheduleRecord) -> Value {
    let enabled = matches!(schedule.status, crate::ScheduleStatus::Active);
    let prompt = schedule
        .metadata
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let trigger_kind = match schedule.trigger_kind {
        crate::ScheduleTriggerKind::Cron => "cron",
        crate::ScheduleTriggerKind::Once => "once",
    };
    json!({
        "id": schedule.id,
        "name": schedule.name,
        "description": schedule.description,
        "enabled": enabled,
        "status": schedule.status,
        "prompt": prompt,
        "trigger": {
            "kind": trigger_kind,
            "cron": schedule.cron_expr,
            "run_at": schedule.run_at,
            "timezone": schedule.timezone,
            "next_fire_at": schedule.next_fire_at,
        },
        "cron": {
            "expr": schedule.cron_expr,
            "timezone": schedule.timezone,
            "next_fire_at": schedule.next_fire_at,
        },
        "mode": {
            "continue_current_session": matches!(schedule.execution_mode, crate::ScheduleExecutionMode::ContinueSession),
            "start_from_current_session": matches!(schedule.execution_mode, crate::ScheduleExecutionMode::ForkSession),
            "steer_when_running": false,
        },
        "execution_mode": schedule.execution_mode,
        "workflow_id": schedule.workflow_id,
        "workflow_inputs_template": null,
        "last_workflow_run_id": null,
        "owner_kind": "api",
        "owner_session_id": null,
        "owner_run_id": null,
        "profile_name": schedule.profile_name,
        "target_session_id": schedule.target_session_id,
        "source_session_id": schedule.source_session_id,
        "last_fire": null,
        "fire_count": schedule.fire_count,
        "failure_count": schedule.failure_count,
        "metadata": schedule.metadata,
        "created_at": schedule.created_at,
        "updated_at": schedule.updated_at,
    })
}

fn parse_json_body<T>(body: &[u8]) -> ClawResult<T>
where
    T: serde::de::DeserializeOwned + Default,
{
    if body.is_empty() {
        Ok(T::default())
    } else {
        Ok(serde_json::from_slice(body)?)
    }
}

fn authorize(state: &AppState, headers: &HeaderMap) -> ClawResult<()> {
    let Some(expected) = state.settings.api_token.as_deref() else {
        return Ok(());
    };
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if bearer == Some(expected) {
        Ok(())
    } else {
        Err(ClawError::Unauthorized(
            "missing or invalid bearer token".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let app = build_router(ClawSettings::default());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn embedded_frontend_serves_index_and_spa_fallback() {
        let app = build_router(ClawSettings::default());
        let root_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(root_response.status(), StatusCode::OK);

        let spa_response = app
            .oneshot(
                Request::builder()
                    .uri("/sessions/session_test")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(spa_response.status(), StatusCode::OK);
        assert_eq!(
            spa_response.headers().get(header::CONTENT_TYPE),
            Some(&header::HeaderValue::from_static("text/html")),
        );
    }

    #[tokio::test]
    async fn console_compatibility_endpoints_are_available() {
        let app = build_router(ClawSettings::default());
        let profile = serde_json::json!({
            "model": "test",
            "builtin_toolsets": ["filesystem"],
            "enabled": true
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/v1/profiles/custom")
                    .header("content-type", "application/json")
                    .body(Body::from(profile.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);

        for path in [
            "/api/v1/profiles/custom",
            "/api/v1/heartbeat/config",
            "/api/v1/heartbeat/status",
            "/api/v1/heartbeat/fires",
            "/api/v1/bridges/conversations",
            "/api/v1/bridges/events",
            "/api/v1/agency/config",
            "/api/v1/agency/status",
            "/api/v1/agency/fires",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::OK, "{path}");
        }
    }

    #[tokio::test]
    async fn latest_claw_api_routes_are_compatible() {
        let app = build_router(ClawSettings::default());
        let session_id = create_test_session(&app).await;
        let run_id = create_test_run(&app, &session_id).await;
        let workflow_id = create_test_workflow(&app).await;
        let workflow_run_id = trigger_test_workflow(&app, &workflow_id).await;
        let schedule_id = create_test_schedule(&app).await;

        for case in latest_claw_api_route_cases(
            &session_id,
            &run_id,
            &workflow_id,
            &workflow_run_id,
            &schedule_id,
        ) {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(case.method)
                        .uri(case.path.as_str())
                        .header("content-type", "application/json")
                        .body(Body::from(case.body.unwrap_or_else(|| "{}".to_string())))
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_ne!(
                response.status(),
                StatusCode::NOT_FOUND,
                "{} {} returned 404",
                case.method,
                case.path
            );
        }
    }

    struct RouteCase {
        method: &'static str,
        path: String,
        body: Option<String>,
    }

    async fn create_test_session(app: &Router) -> String {
        let body = json!({
            "profile_name": "default",
            "metadata": {"test": true},
            "input_parts": [],
            "dispatch_mode": "queue"
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::CREATED);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: Value = serde_json::from_slice(&bytes).expect("json");
        value["session"]["id"]
            .as_str()
            .expect("session id")
            .to_string()
    }

    async fn create_test_run(app: &Router, session_id: &str) -> String {
        let body = json!({
            "input_parts": [{"type":"text", "text":"run"}],
            "dispatch_mode": "queue"
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/sessions/{session_id}/runs"))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::CREATED);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: Value = serde_json::from_slice(&bytes).expect("json");
        value["id"].as_str().expect("run id").to_string()
    }

    async fn create_test_workflow(app: &Router) -> String {
        let body = json!({ "name": "compat-workflow", "definition": {"nodes": []} });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/workflows")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::CREATED);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: Value = serde_json::from_slice(&bytes).expect("json");
        value["id"].as_str().expect("workflow id").to_string()
    }

    async fn trigger_test_workflow(app: &Router, workflow_id: &str) -> String {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/workflows/{workflow_id}:trigger"))
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::CREATED);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: Value = serde_json::from_slice(&bytes).expect("json");
        value["id"].as_str().expect("workflow run id").to_string()
    }

    async fn create_test_schedule(app: &Router) -> String {
        let body = json!({
            "name": "compat-schedule",
            "trigger_kind": "cron",
            "cron_expr": "0 * * * *",
            "execution_mode": "isolate_session"
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/schedules")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::CREATED);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: Value = serde_json::from_slice(&bytes).expect("json");
        value["id"].as_str().expect("schedule id").to_string()
    }

    async fn post_json(app: &Router, path: &str, body: Value, status: StatusCode) -> Value {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        let actual_status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(
            actual_status,
            status,
            "{path}: {}",
            String::from_utf8_lossy(&bytes)
        );
        serde_json::from_slice(&bytes).expect("json")
    }

    async fn get_json(app: &Router, path: &str, status: StatusCode) -> Value {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let actual_status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(
            actual_status,
            status,
            "{path}: {}",
            String::from_utf8_lossy(&bytes)
        );
        serde_json::from_slice(&bytes).expect("json")
    }

    fn latest_claw_api_route_cases(
        session_id: &str,
        run_id: &str,
        workflow_id: &str,
        workflow_run_id: &str,
        schedule_id: &str,
    ) -> Vec<RouteCase> {
        let run_create_body = format!(
            "{{\"session_id\":\"{session_id}\",\"input_parts\":[{{\"type\":\"text\",\"text\":\"run\"}}],\"dispatch_mode\":\"queue\"}}"
        );
        let session_run_body =
            "{\"input_parts\":[{\"type\":\"text\",\"text\":\"run\"}],\"dispatch_mode\":\"queue\"}"
                .to_string();
        let session_create_body = "{\"profile_name\":\"default\",\"input_parts\":[{\"type\":\"text\",\"text\":\"session\"}],\"dispatch_mode\":\"queue\"}".to_string();
        let workflow_create_body =
            "{\"name\":\"route-compat-workflow\",\"definition\":{\"nodes\":[]}}".to_string();
        let schedule_create_body = "{\"name\":\"route-compat-schedule\",\"trigger_kind\":\"cron\",\"cron_expr\":\"0 * * * *\",\"execution_mode\":\"isolate_session\"}".to_string();
        let profile_body = "{\"model\":\"test\",\"builtin_toolsets\":[]}".to_string();
        let patch_body = "{}".to_string();
        vec![
            RouteCase { method: "GET", path: "/api/v1/agency/config".into(), body: None },
            RouteCase { method: "GET", path: "/api/v1/agency/fires".into(), body: None },
            RouteCase { method: "POST", path: "/api/v1/agency/source-session:submit".into(), body: Some(patch_body.clone()) },
            RouteCase { method: "GET", path: "/api/v1/agency/status".into(), body: None },
            RouteCase { method: "POST", path: "/api/v1/agency:bootstrap".into(), body: Some(patch_body.clone()) },
            RouteCase { method: "POST", path: "/api/v1/agency:clear".into(), body: Some(patch_body.clone()) },
            RouteCase { method: "POST", path: "/api/v1/agent/workflows".into(), body: Some(workflow_create_body.clone()) },
            RouteCase { method: "POST", path: format!("/api/v1/agent/workflows/{workflow_id}:trigger"), body: Some(patch_body.clone()) },
            RouteCase { method: "GET", path: "/api/v1/bridges/conversations".into(), body: None },
            RouteCase { method: "GET", path: "/api/v1/bridges/events".into(), body: None },
            RouteCase { method: "POST", path: "/api/v1/bridges/inbound/actions".into(), body: Some(patch_body.clone()) },
            RouteCase { method: "POST", path: "/api/v1/bridges/inbound/messages".into(), body: Some(patch_body.clone()) },
            RouteCase { method: "GET", path: "/api/v1/claw/info".into(), body: None },
            RouteCase { method: "GET", path: "/api/v1/claw/notifications".into(), body: None },
            RouteCase { method: "GET", path: "/api/v1/healthz".into(), body: None },
            RouteCase { method: "GET", path: "/api/v1/heartbeat/config".into(), body: None },
            RouteCase { method: "GET", path: "/api/v1/heartbeat/fires".into(), body: None },
            RouteCase { method: "GET", path: "/api/v1/heartbeat/status".into(), body: None },
            RouteCase { method: "POST", path: "/api/v1/heartbeat:trigger".into(), body: Some(patch_body.clone()) },
            RouteCase { method: "GET", path: "/api/v1/profiles".into(), body: None },
            RouteCase { method: "POST", path: "/api/v1/profiles/seed".into(), body: Some(patch_body.clone()) },
            RouteCase { method: "GET", path: "/api/v1/profiles/default".into(), body: None },
            RouteCase { method: "PUT", path: "/api/v1/profiles/api-compat".into(), body: Some(profile_body) },
            RouteCase { method: "DELETE", path: "/api/v1/profiles/api-compat".into(), body: None },
            RouteCase { method: "POST", path: "/api/v1/runs".into(), body: Some(run_create_body) },
            RouteCase { method: "GET", path: format!("/api/v1/runs/{run_id}"), body: None },
            RouteCase { method: "POST", path: format!("/api/v1/runs/{run_id}/cancel"), body: Some(patch_body.clone()) },
            RouteCase { method: "GET", path: format!("/api/v1/runs/{run_id}/events"), body: None },
            RouteCase { method: "POST", path: format!("/api/v1/runs/{run_id}/interactions/interaction_test:respond"), body: Some(patch_body.clone()) },
            RouteCase { method: "POST", path: format!("/api/v1/runs/{run_id}/interrupt"), body: Some(patch_body.clone()) },
            RouteCase { method: "POST", path: format!("/api/v1/runs/{run_id}/steer"), body: Some(patch_body.clone()) },
            RouteCase { method: "GET", path: format!("/api/v1/runs/{run_id}/trace"), body: None },
            RouteCase { method: "POST", path: "/api/v1/runs:stream".into(), body: Some(format!("{{\"session_id\":\"{session_id}\",\"input_parts\":[{{\"type\":\"text\",\"text\":\"stream\"}}]}}")) },
            RouteCase { method: "GET", path: "/api/v1/schedules".into(), body: None },
            RouteCase { method: "POST", path: "/api/v1/schedules".into(), body: Some(schedule_create_body) },
            RouteCase { method: "GET", path: format!("/api/v1/schedules/{schedule_id}"), body: None },
            RouteCase { method: "PATCH", path: format!("/api/v1/schedules/{schedule_id}"), body: Some("{\"metadata\":{\"patched\":true}}".into()) },
            RouteCase { method: "GET", path: format!("/api/v1/schedules/{schedule_id}/fires"), body: None },
            RouteCase { method: "POST", path: format!("/api/v1/schedules/{schedule_id}:pause"), body: Some(patch_body.clone()) },
            RouteCase { method: "POST", path: format!("/api/v1/schedules/{schedule_id}:resume"), body: Some(patch_body.clone()) },
            RouteCase { method: "POST", path: format!("/api/v1/schedules/{schedule_id}:trigger"), body: Some(patch_body.clone()) },
            RouteCase { method: "GET", path: "/api/v1/sessions".into(), body: None },
            RouteCase { method: "POST", path: "/api/v1/sessions".into(), body: Some(session_create_body) },
            RouteCase { method: "GET", path: format!("/api/v1/sessions/{session_id}"), body: None },
            RouteCase { method: "GET", path: format!("/api/v1/sessions/{session_id}/async-tasks"), body: None },
            RouteCase { method: "GET", path: format!("/api/v1/sessions/{session_id}/async-tasks/task_test"), body: None },
            RouteCase { method: "POST", path: format!("/api/v1/sessions/{session_id}/async-tasks/task_test:cancel"), body: Some(patch_body.clone()) },
            RouteCase { method: "POST", path: format!("/api/v1/sessions/{session_id}/async-tasks/task_test:steer"), body: Some(patch_body.clone()) },
            RouteCase { method: "POST", path: format!("/api/v1/sessions/{session_id}/async-tasks:spawn"), body: Some("{\"name\":\"task_test\",\"input_parts\":[]}".into()) },
            RouteCase { method: "POST", path: format!("/api/v1/sessions/{session_id}/cancel"), body: Some(patch_body.clone()) },
            RouteCase { method: "GET", path: format!("/api/v1/sessions/{session_id}/events"), body: None },
            RouteCase { method: "POST", path: format!("/api/v1/sessions/{session_id}/fork"), body: Some(patch_body.clone()) },
            RouteCase { method: "POST", path: format!("/api/v1/sessions/{session_id}/interrupt"), body: Some(patch_body.clone()) },
            RouteCase { method: "POST", path: format!("/api/v1/sessions/{session_id}/memory:extract"), body: Some(patch_body.clone()) },
            RouteCase { method: "POST", path: format!("/api/v1/sessions/{session_id}/memory:summarize"), body: Some(patch_body.clone()) },
            RouteCase { method: "POST", path: format!("/api/v1/sessions/{session_id}/runs"), body: Some(session_run_body) },
            RouteCase { method: "POST", path: format!("/api/v1/sessions/{session_id}/runs:stream"), body: Some("{\"input_parts\":[{\"type\":\"text\",\"text\":\"stream\"}]}".into()) },
            RouteCase { method: "GET", path: format!("/api/v1/sessions/{session_id}/sandbox"), body: None },
            RouteCase { method: "POST", path: format!("/api/v1/sessions/{session_id}/sandbox:prepare"), body: Some(patch_body.clone()) },
            RouteCase { method: "POST", path: format!("/api/v1/sessions/{session_id}/sandbox:stop"), body: Some(patch_body.clone()) },
            RouteCase { method: "POST", path: format!("/api/v1/sessions/{session_id}/steer"), body: Some(patch_body.clone()) },
            RouteCase { method: "POST", path: format!("/api/v1/sessions/{session_id}/submit"), body: Some("{\"input_parts\":[{\"type\":\"text\",\"text\":\"submit\"}]}".into()) },
            RouteCase { method: "GET", path: format!("/api/v1/sessions/{session_id}/turns"), body: None },
            RouteCase { method: "GET", path: format!("/api/v1/sessions/{session_id}/workspace"), body: None },
            RouteCase { method: "POST", path: "/api/v1/sessions:stream".into(), body: Some("{\"profile_name\":\"default\",\"input_parts\":[{\"type\":\"text\",\"text\":\"stream\"}]}".into()) },
            RouteCase { method: "GET", path: "/api/v1/workflow-runs".into(), body: None },
            RouteCase { method: "GET", path: format!("/api/v1/workflow-runs/{workflow_run_id}"), body: None },
            RouteCase { method: "POST", path: format!("/api/v1/workflow-runs/{workflow_run_id}/cancel"), body: Some(patch_body.clone()) },
            RouteCase { method: "GET", path: format!("/api/v1/workflow-runs/{workflow_run_id}/events"), body: None },
            RouteCase { method: "POST", path: format!("/api/v1/workflow-runs/{workflow_run_id}/nodes/node_test/steer"), body: Some(patch_body.clone()) },
            RouteCase { method: "GET", path: "/api/v1/workflows".into(), body: None },
            RouteCase { method: "POST", path: "/api/v1/workflows".into(), body: Some("{\"name\":\"compat-workflow-extra\",\"definition\":{\"nodes\":[]}}".into()) },
            RouteCase { method: "GET", path: format!("/api/v1/workflows/{workflow_id}"), body: None },
            RouteCase { method: "PATCH", path: format!("/api/v1/workflows/{workflow_id}"), body: Some("{\"tags\":[\"compat\"]}".into()) },
            RouteCase { method: "POST", path: format!("/api/v1/workflows/{workflow_id}:archive"), body: Some(patch_body.clone()) },
            RouteCase { method: "POST", path: format!("/api/v1/workflows/{workflow_id}:trigger"), body: Some(patch_body.clone()) },
            RouteCase { method: "GET", path: "/api/v1/workspace/runtime".into(), body: None },
            RouteCase { method: "POST", path: "/api/v1/workspace:resolve".into(), body: Some("null".into()) },
        ]
    }

    #[tokio::test]
    async fn claw_parity_stateful_actions_record_side_effects() {
        let app = build_router(ClawSettings::default());
        let session_id = create_test_session(&app).await;
        let run_id = create_test_run(&app, &session_id).await;

        let memory = post_json(
            &app,
            &format!("/api/v1/sessions/{session_id}/memory:extract"),
            json!({}),
            StatusCode::ACCEPTED,
        )
        .await;
        assert_eq!(memory["memory_state"]["status"], "queued");

        let spawned = post_json(
            &app,
            &format!("/api/v1/sessions/{session_id}/async-tasks:spawn"),
            json!({"name": "task_test", "input_parts": []}),
            StatusCode::ACCEPTED,
        )
        .await;
        assert_eq!(spawned["task"]["status"], "queued");
        let steered = post_json(
            &app,
            &format!("/api/v1/sessions/{session_id}/async-tasks/task_test:steer"),
            json!({"input_parts": [{"type":"text", "text":"continue"}]}),
            StatusCode::OK,
        )
        .await;
        assert_eq!(steered["task"]["status"], "running");
        let cancelled = post_json(
            &app,
            &format!("/api/v1/sessions/{session_id}/async-tasks/task_test:cancel"),
            json!({}),
            StatusCode::OK,
        )
        .await;
        assert_eq!(cancelled["task"]["status"], "cancelled");

        let interaction = post_json(
            &app,
            &format!("/api/v1/runs/{run_id}/interactions/approval_1:respond"),
            json!({"decision": "approved"}),
            StatusCode::OK,
        )
        .await;
        assert_eq!(interaction["accepted"], true);

        let bootstrap = post_json(
            &app,
            "/api/v1/agency:bootstrap",
            json!({}),
            StatusCode::ACCEPTED,
        )
        .await;
        assert_eq!(bootstrap["accepted"], true);
        let source = post_json(
            &app,
            "/api/v1/agency/source-session:submit",
            json!({"session_id": session_id}),
            StatusCode::ACCEPTED,
        )
        .await;
        assert_eq!(source["status"], "queued");
        let fires = get_json(&app, "/api/v1/agency/fires", StatusCode::OK).await;
        assert_eq!(fires["fires"].as_array().expect("fires").len(), 1);

        let notifications = get_json(&app, "/api/v1/claw/notifications", StatusCode::OK).await;
        assert!(notifications["events"].as_array().expect("events").len() >= 3);

        let clear = post_json(&app, "/api/v1/agency:clear", json!({}), StatusCode::OK).await;
        assert_eq!(clear["accepted"], true);
    }

    #[tokio::test]
    async fn workflow_and_schedule_endpoints_accept_resources() {
        let app = build_router(ClawSettings::default());
        let workflow = serde_json::json!({
            "name": "nightly-check",
            "definition": { "nodes": [] }
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/workflows")
                    .header("content-type", "application/json")
                    .body(Body::from(workflow.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::CREATED);

        let schedule = serde_json::json!({
            "name": "hourly-check",
            "trigger_kind": "cron",
            "cron_expr": "0 * * * *",
            "execution_mode": "isolate_session"
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/schedules")
                    .header("content-type", "application/json")
                    .body(Body::from(schedule.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::CREATED);
    }
}
