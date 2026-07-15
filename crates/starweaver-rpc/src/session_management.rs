//! RPC-owned in-process agent session query/control adapter.
//!
//! This module calls typed storage/coordinator operations directly. It never serializes a
//! JSON-RPC request or calls `RpcService::handle_text`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use starweaver_agent::{AgentSessionControl, AgentSessionQuery};
use starweaver_core::{RunId, SessionId};
use starweaver_runtime::AgentInput;
use starweaver_session::{
    AgentDisplayPage, AgentReplayQuery, AgentRunListQuery, AgentRunPage, AgentRunView,
    AgentSessionControlError, AgentSessionControlErrorCode, AgentSessionInclude,
    AgentSessionListQuery, AgentSessionOperation, AgentSessionPage, AgentSessionQueryError,
    AgentSessionQueryErrorCode, AgentSessionScope, AgentSessionView, CreateManagedSession,
    DeleteManagedSession, InterruptManagedRun, ManagedRunTarget, ManagedSessionTarget,
    RunControlReceipt, RunRecord, RunStartReceipt, SessionMutationReceipt, SessionRecord,
    SessionStore, StartManagedRun, SteerManagedRun, UpdateManagedSession,
};
use starweaver_storage::SqliteStorage;
use starweaver_stream::{DisplayVisibility, ReplayCursor, ReplayScope};

use crate::{RpcAgentCatalog, RpcHostError, RpcRunRequest, RpcRuntimeCoordinator};

/// In-process adapter over RPC-owned storage and runtime coordination.
#[derive(Clone)]
pub struct RpcAgentSessionAdapter {
    storage: SqliteStorage,
    coordinator: RpcRuntimeCoordinator,
    catalog: RpcAgentCatalog,
    workspace_root: PathBuf,
}

impl RpcAgentSessionAdapter {
    pub const fn new(
        storage: SqliteStorage,
        coordinator: RpcRuntimeCoordinator,
        catalog: RpcAgentCatalog,
        workspace_root: PathBuf,
    ) -> Self {
        Self {
            storage,
            coordinator,
            catalog,
            workspace_root,
        }
    }
}

#[async_trait]
impl AgentSessionQuery for RpcAgentSessionAdapter {
    async fn list_sessions(
        &self,
        scope: &AgentSessionScope,
        query: AgentSessionListQuery,
    ) -> Result<AgentSessionPage, AgentSessionQueryError> {
        require_query(scope)?;
        let sessions = self
            .storage
            .session_store()
            .list_sessions(starweaver_session::SessionFilter {
                status: query.status,
                profile: query.profile,
                workspace: query.workspace,
                limit: None,
            })
            .await
            .map_err(map_query_store)?;
        let start = query.page_token.as_deref().map_or(Ok(0), |token| {
            sessions
                .iter()
                .position(|session| session.session_id.as_str() == token)
                .map(|index| index + 1)
                .ok_or_else(invalid_cursor)
        })?;
        let limit =
            usize::try_from(query.limit.max(1).min(scope.max_page_size.max(1))).unwrap_or(50);
        let selected = sessions
            .into_iter()
            .skip(start)
            .filter(|session| authorized(scope, session))
            .take(limit + 1)
            .collect::<Vec<_>>();
        let more = selected.len() > limit;
        let selected = selected.into_iter().take(limit).collect::<Vec<_>>();
        let next_page_token = more
            .then(|| {
                selected
                    .last()
                    .map(|session| session.session_id.as_str().to_string())
            })
            .flatten();
        Ok(AgentSessionPage {
            sessions: selected
                .iter()
                .map(|session| {
                    let mut view = session_view(scope, session, Vec::new());
                    view.controllable = self.session_controllable(scope, session);
                    view
                })
                .collect(),
            next_page_token,
        })
    }

    async fn get_session(
        &self,
        scope: &AgentSessionScope,
        session_id: &SessionId,
        include: AgentSessionInclude,
    ) -> Result<AgentSessionView, AgentSessionQueryError> {
        require_query(scope)?;
        let store = self.storage.session_store();
        let session = store
            .load_session(session_id)
            .await
            .map_err(map_query_store)?;
        require_authorized(scope, &session)?;
        let recent_runs = if include.recent_runs {
            let mut runs = store.list_runs(session_id).await.map_err(map_query_store)?;
            let keep = usize::try_from(scope.max_page_size.min(10)).unwrap_or(10);
            if runs.len() > keep {
                runs = runs.split_off(runs.len() - keep);
            }
            runs.iter().map(|run| self.run_view(scope, run)).collect()
        } else {
            Vec::new()
        };
        let mut view = session_view(scope, &session, recent_runs);
        view.controllable = self.session_controllable(scope, &session);
        Ok(view)
    }

    async fn list_runs(
        &self,
        scope: &AgentSessionScope,
        session_id: &SessionId,
        query: AgentRunListQuery,
    ) -> Result<AgentRunPage, AgentSessionQueryError> {
        require_query(scope)?;
        let store = self.storage.session_store();
        let session = store
            .load_session(session_id)
            .await
            .map_err(map_query_store)?;
        require_authorized(scope, &session)?;
        let runs = store.list_runs(session_id).await.map_err(map_query_store)?;
        let after = query
            .page_token
            .as_deref()
            .map(|token| token.parse::<usize>().map_err(|_| invalid_cursor()))
            .transpose()?
            .unwrap_or(0);
        let limit =
            usize::try_from(query.limit.max(1).min(scope.max_page_size.max(1))).unwrap_or(50);
        let selected = runs
            .iter()
            .filter(|run| run.sequence_no > after)
            .take(limit + 1)
            .collect::<Vec<_>>();
        let more = selected.len() > limit;
        let selected = selected.into_iter().take(limit).collect::<Vec<_>>();
        let next_page_token = more
            .then(|| selected.last().map(|run| run.sequence_no.to_string()))
            .flatten();
        Ok(AgentRunPage {
            runs: selected
                .into_iter()
                .map(|run| self.run_view(scope, run))
                .collect(),
            next_page_token,
        })
    }

    async fn get_run(
        &self,
        scope: &AgentSessionScope,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> Result<AgentRunView, AgentSessionQueryError> {
        require_query(scope)?;
        let store = self.storage.session_store();
        let session = store
            .load_session(session_id)
            .await
            .map_err(map_query_store)?;
        require_authorized(scope, &session)?;
        let run = store
            .load_run(session_id, run_id)
            .await
            .map_err(map_query_store)?;
        Ok(self.run_view(scope, &run))
    }

    async fn replay_run(
        &self,
        scope: &AgentSessionScope,
        target: ManagedRunTarget,
        query: AgentReplayQuery,
    ) -> Result<AgentDisplayPage, AgentSessionQueryError> {
        require_query(scope)?;
        if target.namespace_id != scope.namespace_id {
            return Err(hidden_query());
        }
        let store = self.storage.session_store();
        let session = store
            .load_session(&target.session_id)
            .await
            .map_err(map_query_store)?;
        require_authorized(scope, &session)?;
        store
            .load_run(&target.session_id, &target.run_id)
            .await
            .map_err(map_query_store)?;
        if query.after.as_ref().is_some_and(|cursor| {
            cursor.scope != ReplayScope::run(target.run_id.as_str())
                || cursor.family != starweaver_stream::ReplayCursorFamily::Display
        }) {
            return Err(invalid_cursor());
        }
        let after = query.after.as_ref().map(|cursor| cursor.sequence);
        let storage = self.storage.clone();
        let session_id = target.session_id.clone();
        let run_id = target.run_id.clone();
        let messages = tokio::task::spawn_blocking(move || {
            storage.load_display_messages(&session_id, Some(&run_id), after)
        })
        .await
        .map_err(|_| unavailable_query("RPC display replay worker failed"))?
        .map_err(map_query_store)?;
        let limit =
            usize::try_from(query.limit.max(1).min(scope.max_page_size.max(1))).unwrap_or(50);
        let mut messages = messages
            .into_iter()
            .filter(|message| message.visibility == DisplayVisibility::Public)
            .take(limit)
            .map(|mut message| {
                message.payload = Value::Null;
                message.metadata.clear();
                message.preview = message.preview.map(|text| truncate(&text, 2_000));
                message
            })
            .collect::<Vec<_>>();
        let next_cursor = messages.last().map(|message| {
            ReplayCursor::display(ReplayScope::run(target.run_id.as_str()), message.sequence)
        });
        messages.shrink_to_fit();
        Ok(AgentDisplayPage {
            messages,
            next_cursor,
            trust: "untrusted_historical_evidence".to_string(),
        })
    }
}

impl RpcAgentSessionAdapter {
    fn run_view(&self, scope: &AgentSessionScope, run: &RunRecord) -> AgentRunView {
        let target = ManagedRunTarget::new(
            scope.namespace_id.clone(),
            run.session_id.clone(),
            run.run_id.clone(),
        );
        let input = run
            .input
            .iter()
            .filter_map(|part| match part {
                starweaver_session::InputPart::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        AgentRunView {
            controllable: scope.allows(AgentSessionOperation::Control)
                && self.coordinator.is_controllable(&target),
            target,
            status: run.status,
            sequence_no: run.sequence_no,
            input_preview: (!input.is_empty()).then(|| truncate(&input, 1_000)),
            output_preview: run
                .output_preview
                .as_deref()
                .map(|text| truncate(text, 2_000)),
            error_category: (run.status == starweaver_session::RunStatus::Failed)
                .then(|| "run_failed".to_string()),
            created_at: run.created_at,
            updated_at: run.updated_at,
        }
    }

    fn session_controllable(&self, scope: &AgentSessionScope, session: &SessionRecord) -> bool {
        scope.allows(AgentSessionOperation::Control)
            && session.active_run_id.as_ref().is_some_and(|run_id| {
                self.coordinator.is_controllable(&ManagedRunTarget::new(
                    scope.namespace_id.clone(),
                    session.session_id.clone(),
                    run_id.clone(),
                ))
            })
    }

    async fn require_owned_target(
        &self,
        scope: &AgentSessionScope,
        session_id: &SessionId,
    ) -> Result<(), AgentSessionControlError> {
        let session = self
            .storage
            .session_store()
            .load_session(session_id)
            .await
            .map_err(map_control_store)?;
        require_authorized_control(scope, &session)
    }
}

#[async_trait]
impl AgentSessionControl for RpcAgentSessionAdapter {
    async fn create_session(
        &self,
        scope: &AgentSessionScope,
        command: CreateManagedSession,
    ) -> Result<SessionMutationReceipt, AgentSessionControlError> {
        require_control(scope, AgentSessionOperation::Create)?;
        validate_key(&command.idempotency_key)?;
        validate_metadata(&command.metadata)?;
        if let Some(profile) = command.profile.as_deref() {
            self.catalog.profile(profile).map_err(map_profile_error)?;
        }
        let workspace = command
            .workspace
            .as_deref()
            .map(|workspace| validate_workspace(&self.workspace_root, workspace))
            .transpose()?;
        let fingerprint = command_fingerprint("create_session", &command)
            .map_err(|_| invalid_control("managed session command cannot be fingerprinted"))?;
        let idempotency_key = command.idempotency_key.clone();
        let mut pending = SessionRecord::new(SessionId::new());
        let pending_session_id = pending.session_id.clone();
        pending.namespace_id.clone_from(&scope.namespace_id);
        pending.owner_id.clone_from(&scope.owner_id);
        pending.title = command.title.map(|title| truncate(&title, 256));
        pending.profile = command.profile;
        pending.workspace = workspace;
        let session = self
            .storage
            .session_store()
            .create_session_idempotent(pending, &idempotency_key, &fingerprint)
            .await
            .map_err(map_control_store)?;
        let idempotent_replay = session.session_id != pending_session_id;
        Ok(SessionMutationReceipt {
            receipt_id: format!("session_{idempotency_key}"),
            session: session_view(scope, &session, Vec::new()),
            idempotent_replay,
        })
    }

    async fn update_session(
        &self,
        scope: &AgentSessionScope,
        command: UpdateManagedSession,
    ) -> Result<SessionMutationReceipt, AgentSessionControlError> {
        require_control(scope, AgentSessionOperation::Update)?;
        require_control_target(scope, &command.session_id, false)?;
        validate_key(&command.idempotency_key)?;
        validate_metadata(&command.patch.metadata)?;
        if let Some(Some(profile)) = command.patch.profile.as_ref() {
            self.catalog.profile(profile).map_err(map_profile_error)?;
        }
        let store = self.storage.session_store();
        let current = store
            .load_session(&command.session_id)
            .await
            .map_err(map_control_store)?;
        require_authorized_control(scope, &current)?;
        let idempotent_replay = current.revision != command.expected_revision;
        let fingerprint = command_fingerprint("update_session", &command)
            .map_err(|_| invalid_control("managed session command cannot be fingerprinted"))?;
        let idempotency_key = command.idempotency_key.clone();
        let session_id = command.session_id.clone();
        let session = match store.update_managed_session(command, &fingerprint).await {
            Ok(session) => session,
            Err(error) => {
                return Err(map_control_store_for_session(&store, &session_id, error).await);
            }
        };
        Ok(SessionMutationReceipt {
            receipt_id: format!("session_{idempotency_key}"),
            session: session_view(scope, &session, Vec::new()),
            idempotent_replay,
        })
    }

    async fn delete_session(
        &self,
        scope: &AgentSessionScope,
        command: DeleteManagedSession,
    ) -> Result<SessionMutationReceipt, AgentSessionControlError> {
        require_control(scope, AgentSessionOperation::Delete)?;
        require_control_target(scope, &command.session_id, true)?;
        if command
            .approval_receipt_id
            .as_deref()
            .is_none_or(|receipt| receipt.trim().is_empty() || receipt.len() > 256)
        {
            return Err(control_error(
                AgentSessionControlErrorCode::ApprovalRequired,
                "session deletion requires bounded host approval evidence",
            ));
        }
        validate_key(&command.idempotency_key)?;
        let store = self.storage.session_store();
        let session = store
            .load_session(&command.session_id)
            .await
            .map_err(map_control_store)?;
        require_authorized_control(scope, &session)?;
        let idempotent_replay = session.revision != command.expected_revision;
        let fence_id = format!("delete_{}", command.idempotency_key);
        let fingerprint = command_fingerprint("delete_session", &command)
            .map_err(|_| invalid_control("managed session command cannot be fingerprinted"))?;
        if let Err(error) = store
            .acquire_session_deletion_fence(
                &command.session_id,
                command.expected_revision,
                &fence_id,
                scope.owner_id.as_deref().unwrap_or("rpc-agent"),
                &command.idempotency_key,
                &fingerprint,
            )
            .await
        {
            return Err(map_control_store_for_session(&store, &command.session_id, error).await);
        }
        self.coordinator
            .cancel_session_subagents(&command.session_id, std::time::Duration::from_secs(10))
            .await
            .map_err(map_control_host)?;
        for run in store
            .list_runs(&command.session_id)
            .await
            .map_err(map_control_store)?
        {
            if run.status.is_active()
                && self.coordinator.is_controllable(&ManagedRunTarget::new(
                    scope.namespace_id.clone(),
                    command.session_id.clone(),
                    run.run_id.clone(),
                ))
            {
                let _ = self
                    .coordinator
                    .cancel(
                        &command.session_id,
                        &run.run_id,
                        Some("session deletion fence".to_string()),
                    )
                    .await
                    .map_err(map_control_host)?;
                let _ = self
                    .coordinator
                    .await_terminal(
                        &command.session_id,
                        &run.run_id,
                        Some(std::time::Duration::from_secs(10)),
                    )
                    .await
                    .map_err(map_control_host)?;
            }
        }
        let session = store
            .tombstone_session(&command.session_id, &fence_id)
            .await
            .map_err(map_control_store)?;
        Ok(SessionMutationReceipt {
            receipt_id: fence_id,
            session: session_view(scope, &session, Vec::new()),
            idempotent_replay,
        })
    }

    async fn start_run(
        &self,
        scope: &AgentSessionScope,
        command: StartManagedRun,
    ) -> Result<RunStartReceipt, AgentSessionControlError> {
        require_control(scope, AgentSessionOperation::Create)?;
        require_control_target(scope, &command.session_id, false)?;
        validate_key(&command.idempotency_key)?;
        if !command.environment_refs.is_empty() {
            return Err(control_error(
                AgentSessionControlErrorCode::InvalidCommand,
                "managed environment references require a configured typed RPC attachment adapter",
            ));
        }
        let store = self.storage.session_store();
        let session = store
            .load_session(&command.session_id)
            .await
            .map_err(map_control_store)?;
        require_authorized_control(scope, &session)?;
        let profile = command
            .profile
            .clone()
            .unwrap_or_else(|| self.catalog.default_profile().to_string());
        self.catalog.profile(&profile).map_err(map_profile_error)?;
        let fingerprint = command_fingerprint("start_session_run", &command)
            .map_err(|_| invalid_control("managed run command cannot be fingerprinted"))?;
        let content = command
            .input
            .iter()
            .cloned()
            .map(starweaver_model::ContentPart::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| invalid_control("managed run input contains an unsupported part"))?;
        let started = self
            .coordinator
            .start(RpcRunRequest {
                durable_input: command.input,
                input: AgentInput::parts(content),
                session_id: Some(command.session_id),
                restore_from_run_id: None,
                profile,
                environment_attachments: Vec::new(),
                idempotency_key: command.idempotency_key,
                command_fingerprint: fingerprint,
                install_session_management: false,
            })
            .await
            .map_err(map_control_host)?;
        let target = ManagedRunTarget::new(
            scope.namespace_id.clone(),
            started.session_id,
            started.run_id,
        );
        Ok(RunStartReceipt {
            receipt_id: started.admission_id,
            target,
            status: started.status,
            fencing_generation: started.fencing_generation,
            idempotent_replay: started.idempotent_replay,
        })
    }

    async fn steer_run(
        &self,
        scope: &AgentSessionScope,
        command: SteerManagedRun,
    ) -> Result<RunControlReceipt, AgentSessionControlError> {
        require_control(scope, AgentSessionOperation::Control)?;
        require_run_target(scope, &command.target)?;
        self.require_owned_target(scope, &command.target.session_id)
            .await?;
        if let Some(key) = command.idempotency_key.as_deref() {
            validate_key(key)?;
        }
        let result = self
            .coordinator
            .steer_idempotent(
                &command.target.session_id,
                &command.target.run_id,
                command.steering_id.clone(),
                command.text,
                command.idempotency_key,
            )
            .await
            .map_err(map_control_host)?;
        Ok(control_receipt_from_value(
            command.target,
            command.steering_id,
            &result,
        ))
    }

    async fn interrupt_run(
        &self,
        scope: &AgentSessionScope,
        command: InterruptManagedRun,
    ) -> Result<RunControlReceipt, AgentSessionControlError> {
        require_control(scope, AgentSessionOperation::Control)?;
        require_run_target(scope, &command.target)?;
        self.require_owned_target(scope, &command.target.session_id)
            .await?;
        if let Some(key) = command.idempotency_key.as_deref() {
            validate_key(key)?;
        }
        let result = self
            .coordinator
            .cancel_idempotent(
                &command.target.session_id,
                &command.target.run_id,
                command.operation_id.clone(),
                command.reason_category,
                command.idempotency_key,
            )
            .await
            .map_err(map_control_host)?;
        Ok(control_receipt_from_value(
            command.target,
            command.operation_id,
            &result,
        ))
    }
}

fn session_view(
    _scope: &AgentSessionScope,
    session: &SessionRecord,
    recent_runs: Vec<AgentRunView>,
) -> AgentSessionView {
    AgentSessionView {
        target: ManagedSessionTarget::new(session.namespace_id.clone(), session.session_id.clone()),
        title: session.title.as_deref().map(|text| truncate(text, 256)),
        status: session.status,
        profile: session.profile.as_deref().map(|text| truncate(text, 128)),
        workspace: session
            .workspace
            .as_deref()
            .and_then(|path| std::path::Path::new(path).file_name()?.to_str())
            .map(|text| truncate(text, 128)),
        revision: session.revision,
        head_run_id: session.head_run_id.clone(),
        active_run_id: session.active_run_id.clone(),
        resumable: session.head_success_run_id.is_some(),
        controllable: false,
        recent_runs,
        created_at: session.created_at,
        updated_at: session.updated_at,
    }
}

fn control_receipt_from_value(
    target: ManagedRunTarget,
    operation_id: String,
    value: &Value,
) -> RunControlReceipt {
    let receipt_id = value
        .get("receiptId")
        .and_then(Value::as_str)
        .unwrap_or("control-accepted")
        .to_string();
    RunControlReceipt {
        receipt_id,
        target,
        operation_id,
        fencing_generation: value
            .get("fencingGeneration")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        accepted: true,
        idempotent_replay: value
            .get("idempotent")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        created_at: chrono::Utc::now(),
    }
}

fn resource_authorized(scope: &AgentSessionScope, session: &SessionRecord) -> bool {
    session.namespace_id == scope.namespace_id
        && scope.allows_session(&session.session_id)
        && scope.owner_id.as_ref().is_none_or(|owner| {
            session
                .owner_id
                .as_ref()
                .is_some_and(|current| current == owner)
        })
}

fn authorized(scope: &AgentSessionScope, session: &SessionRecord) -> bool {
    resource_authorized(scope, session)
        && session.status != starweaver_session::SessionStatus::Deleted
        && (scope.allow_self_query || scope.source_session_id.as_ref() != Some(&session.session_id))
}

fn require_authorized(
    scope: &AgentSessionScope,
    session: &SessionRecord,
) -> Result<(), AgentSessionQueryError> {
    if authorized(scope, session) {
        Ok(())
    } else {
        Err(hidden_query())
    }
}

fn require_authorized_control(
    scope: &AgentSessionScope,
    session: &SessionRecord,
) -> Result<(), AgentSessionControlError> {
    if resource_authorized(scope, session) {
        Ok(())
    } else {
        Err(control_error(
            AgentSessionControlErrorCode::NotFound,
            "session or run was not found",
        ))
    }
}

fn require_query(scope: &AgentSessionScope) -> Result<(), AgentSessionQueryError> {
    if scope
        .deadline
        .is_some_and(|deadline| deadline <= chrono::Utc::now())
    {
        return Err(AgentSessionQueryError {
            code: AgentSessionQueryErrorCode::PermissionDenied,
            message: "session capability deadline expired".to_string(),
        });
    }
    if scope.allows(AgentSessionOperation::Read) {
        Ok(())
    } else {
        Err(AgentSessionQueryError {
            code: AgentSessionQueryErrorCode::PermissionDenied,
            message: "session.read is not granted".to_string(),
        })
    }
}

fn require_control(
    scope: &AgentSessionScope,
    operation: AgentSessionOperation,
) -> Result<(), AgentSessionControlError> {
    if scope
        .deadline
        .is_some_and(|deadline| deadline <= chrono::Utc::now())
    {
        return Err(control_error(
            AgentSessionControlErrorCode::PermissionDenied,
            "session capability deadline expired",
        ));
    }
    if scope.allows(operation) {
        Ok(())
    } else {
        Err(control_error(
            AgentSessionControlErrorCode::PermissionDenied,
            "session operation is not granted",
        ))
    }
}

fn require_control_target(
    scope: &AgentSessionScope,
    session_id: &SessionId,
    always_deny_self: bool,
) -> Result<(), AgentSessionControlError> {
    if !scope.allows_session(session_id)
        || ((always_deny_self || !scope.allow_self_control)
            && scope.source_session_id.as_ref() == Some(session_id))
    {
        return Err(control_error(
            AgentSessionControlErrorCode::PermissionDenied,
            "session target is not authorized",
        ));
    }
    Ok(())
}

fn require_run_target(
    scope: &AgentSessionScope,
    target: &ManagedRunTarget,
) -> Result<(), AgentSessionControlError> {
    if target.namespace_id != scope.namespace_id
        || !scope.allows_session(&target.session_id)
        || (!scope.allow_self_control && scope.is_self_run(target))
    {
        return Err(control_error(
            AgentSessionControlErrorCode::PermissionDenied,
            "run target is not authorized",
        ));
    }
    Ok(())
}

fn validate_key(key: &str) -> Result<(), AgentSessionControlError> {
    if key.trim().is_empty() || key.len() > 128 {
        return Err(control_error(
            AgentSessionControlErrorCode::InvalidCommand,
            "idempotency key must contain 1..128 bytes",
        ));
    }
    Ok(())
}

fn validate_metadata(
    metadata: &std::collections::BTreeMap<String, Value>,
) -> Result<(), AgentSessionControlError> {
    if metadata.is_empty() {
        Ok(())
    } else {
        Err(invalid_control(
            "RPC agent session metadata has no configured allowlisted keys",
        ))
    }
}

fn validate_workspace(root: &Path, workspace: &str) -> Result<String, AgentSessionControlError> {
    let root = root
        .canonicalize()
        .map_err(|_| invalid_control("configured RPC workspace root is unavailable"))?;
    let requested = Path::new(workspace);
    let requested = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    let requested = requested
        .canonicalize()
        .map_err(|_| invalid_control("requested workspace does not exist"))?;
    if !requested.starts_with(&root) {
        return Err(invalid_control(
            "requested workspace is outside the configured RPC workspace root",
        ));
    }
    Ok(requested.to_string_lossy().into_owned())
}

pub fn command_fingerprint(
    operation: &str,
    command: &impl Serialize,
) -> Result<String, serde_json::Error> {
    let payload = serde_json::to_vec(&(operation, command))?;
    Ok(format!("{operation}:{:x}", Sha256::digest(payload)))
}

fn invalid_control(message: &str) -> AgentSessionControlError {
    control_error(AgentSessionControlErrorCode::InvalidCommand, message)
}

fn map_profile_error(_error: RpcHostError) -> AgentSessionControlError {
    invalid_control("requested RPC profile is not configured")
}

async fn map_control_store_for_session(
    store: &starweaver_storage::SqliteSessionStore,
    session_id: &SessionId,
    error: starweaver_session::SessionStoreError,
) -> AgentSessionControlError {
    if matches!(error, starweaver_session::SessionStoreError::Conflict(_)) {
        let current_revision = store
            .load_session(session_id)
            .await
            .ok()
            .map(|session| session.revision);
        let mut mapped = map_control_store(error);
        mapped.current_revision = current_revision;
        mapped
    } else {
        map_control_store(error)
    }
}

#[allow(clippy::needless_pass_by_value)]
fn map_query_store(error: starweaver_session::SessionStoreError) -> AgentSessionQueryError {
    if matches!(error, starweaver_session::SessionStoreError::NotFound(_)) {
        hidden_query()
    } else {
        AgentSessionQueryError {
            code: AgentSessionQueryErrorCode::Unavailable,
            message: "canonical session storage is unavailable".to_string(),
        }
    }
}

fn unavailable_query(message: &str) -> AgentSessionQueryError {
    AgentSessionQueryError {
        code: AgentSessionQueryErrorCode::Unavailable,
        message: message.to_string(),
    }
}

#[allow(clippy::needless_pass_by_value)]
fn map_control_store(error: starweaver_session::SessionStoreError) -> AgentSessionControlError {
    use starweaver_session::SessionStoreError;
    let code = match error {
        SessionStoreError::NotFound(_) => AgentSessionControlErrorCode::NotFound,
        SessionStoreError::AlreadyExists(_)
        | SessionStoreError::Conflict(_)
        | SessionStoreError::QuotaExceeded(_) => AgentSessionControlErrorCode::Conflict,
        SessionStoreError::IdempotencyConflict(_) => {
            AgentSessionControlErrorCode::IdempotencyConflict
        }
        SessionStoreError::RunConflict(_) => AgentSessionControlErrorCode::RunConflict,
        SessionStoreError::Failed(_) => AgentSessionControlErrorCode::Unavailable,
    };
    control_error(code, "session operation was not accepted")
}

fn map_control_host(error: RpcHostError) -> AgentSessionControlError {
    match error {
        RpcHostError::NotFound(_) => control_error(
            AgentSessionControlErrorCode::NotActive,
            "run is not active in the current fenced host",
        ),
        RpcHostError::Invalid(_) => control_error(
            AgentSessionControlErrorCode::InvalidCommand,
            "managed run command is invalid",
        ),
        RpcHostError::Storage(message) if message.contains("idempotency") => control_error(
            AgentSessionControlErrorCode::IdempotencyConflict,
            "managed run idempotency key is bound to another command",
        ),
        RpcHostError::Storage(message) if message.contains("active run") => control_error(
            AgentSessionControlErrorCode::RunConflict,
            "session already has an active run",
        ),
        _ => control_error(
            AgentSessionControlErrorCode::Unavailable,
            "RPC coordinator is unavailable",
        ),
    }
}

fn hidden_query() -> AgentSessionQueryError {
    AgentSessionQueryError {
        code: AgentSessionQueryErrorCode::NotFound,
        message: "session or run was not found".to_string(),
    }
}

fn invalid_cursor() -> AgentSessionQueryError {
    AgentSessionQueryError {
        code: AgentSessionQueryErrorCode::InvalidCursor,
        message: "invalid or stale page/replay cursor".to_string(),
    }
}

fn control_error(code: AgentSessionControlErrorCode, message: &str) -> AgentSessionControlError {
    AgentSessionControlError {
        code,
        message: message.to_string(),
        current_revision: None,
    }
}

fn truncate(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::{RpcConfig, environment_manager::EnvironmentAttachmentManager};

    fn scope(owner: &str) -> AgentSessionScope {
        AgentSessionScope {
            namespace_id: starweaver_session::LOCAL_SESSION_NAMESPACE.to_string(),
            owner_id: Some(owner.to_string()),
            source_product: "rpc-test".to_string(),
            source_session_id: None,
            source_run_id: None,
            operations: BTreeSet::from([
                AgentSessionOperation::Read,
                AgentSessionOperation::Create,
                AgentSessionOperation::Update,
                AgentSessionOperation::Control,
                AgentSessionOperation::Delete,
            ]),
            allowed_session_ids: BTreeSet::new(),
            allow_self_query: true,
            allow_self_control: false,
            policy_fingerprint: "rpc-test-policy".to_string(),
            deadline: None,
            max_page_size: 20,
        }
    }

    fn adapter(root: &Path) -> RpcAgentSessionAdapter {
        let config = RpcConfig::for_tests(root);
        std::fs::create_dir_all(&config.workspace_root).unwrap();
        let storage = SqliteStorage::open(&config.database_path).unwrap();
        let catalog = RpcAgentCatalog::new(config.clone()).unwrap();
        let coordinator = RpcRuntimeCoordinator::new(
            config.clone(),
            catalog.clone(),
            storage.clone(),
            EnvironmentAttachmentManager::new(),
        );
        RpcAgentSessionAdapter::new(storage, coordinator, catalog, config.workspace_root)
    }

    #[tokio::test]
    async fn mutations_enforce_owner_revision_profile_metadata_and_idempotency() {
        let temp = tempfile::tempdir().unwrap();
        let adapter = adapter(temp.path());
        let owner_scope = scope("owner-a");
        let create = CreateManagedSession {
            title: Some("managed".to_string()),
            profile: Some("default".to_string()),
            workspace: None,
            metadata: BTreeMap::new(),
            idempotency_key: "create-one".to_string(),
        };
        let first = adapter
            .create_session(&owner_scope, create.clone())
            .await
            .unwrap();
        assert!(!first.idempotent_replay);
        assert_eq!(first.session.revision, 1);
        let replay = adapter
            .create_session(&owner_scope, create.clone())
            .await
            .unwrap();
        assert_eq!(replay.session.target, first.session.target);
        assert!(replay.idempotent_replay);

        let idempotency_conflict = adapter
            .create_session(
                &owner_scope,
                CreateManagedSession {
                    title: Some("different".to_string()),
                    ..create
                },
            )
            .await
            .unwrap_err();
        assert_eq!(
            idempotency_conflict.code,
            AgentSessionControlErrorCode::IdempotencyConflict
        );

        let metadata_error = adapter
            .create_session(
                &owner_scope,
                CreateManagedSession {
                    title: None,
                    profile: None,
                    workspace: None,
                    metadata: BTreeMap::from([("secret".to_string(), Value::Bool(true))]),
                    idempotency_key: "metadata".to_string(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(
            metadata_error.code,
            AgentSessionControlErrorCode::InvalidCommand
        );

        let session_id = first.session.target.session_id.clone();
        let update = UpdateManagedSession {
            session_id: session_id.clone(),
            expected_revision: first.session.revision,
            patch: starweaver_session::ManagedSessionPatch {
                title: Some(Some("updated".to_string())),
                profile: None,
                archived: None,
                metadata: BTreeMap::new(),
            },
            idempotency_key: "update-one".to_string(),
        };
        let updated = adapter
            .update_session(&owner_scope, update.clone())
            .await
            .unwrap();
        assert_eq!(updated.session.revision, 2);
        assert!(!updated.idempotent_replay);
        let replay = adapter
            .update_session(&owner_scope, update.clone())
            .await
            .unwrap();
        assert!(replay.idempotent_replay);

        let conflict = adapter
            .update_session(
                &owner_scope,
                UpdateManagedSession {
                    idempotency_key: "other-update".to_string(),
                    ..update.clone()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(conflict.code, AgentSessionControlErrorCode::Conflict);
        assert_eq!(conflict.current_revision, Some(2));

        let unauthorized = adapter
            .update_session(
                &scope("owner-b"),
                UpdateManagedSession {
                    expected_revision: 2,
                    idempotency_key: "owner-b-update".to_string(),
                    ..update
                },
            )
            .await
            .unwrap_err();
        assert_eq!(unauthorized.code, AgentSessionControlErrorCode::NotFound);
    }

    #[tokio::test]
    async fn query_scope_hides_disallowed_self_and_foreign_owner_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let adapter = adapter(temp.path());
        let owner_scope = scope("owner-a");
        let created = adapter
            .create_session(
                &owner_scope,
                CreateManagedSession {
                    title: Some("owned".to_string()),
                    profile: Some("default".to_string()),
                    workspace: None,
                    metadata: BTreeMap::new(),
                    idempotency_key: "query-owned".to_string(),
                },
            )
            .await
            .unwrap();

        let mut no_self = owner_scope.clone();
        no_self.source_session_id = Some(created.session.target.session_id.clone());
        no_self.allow_self_query = false;
        let page = adapter
            .list_sessions(&no_self, AgentSessionListQuery::default())
            .await
            .unwrap();
        assert!(page.sessions.is_empty());

        let page = adapter
            .list_sessions(&scope("owner-b"), AgentSessionListQuery::default())
            .await
            .unwrap();
        assert!(page.sessions.is_empty());
    }
}
