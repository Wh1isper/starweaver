//! CLI-owned query-only session-management adapter.

use async_trait::async_trait;
use serde_json::Value;
use starweaver_agent::AgentSessionQuery;
use starweaver_core::{RunId, SessionId};
use starweaver_session::{
    AgentDisplayPage, AgentReplayQuery, AgentRunListQuery, AgentRunPage, AgentRunView,
    AgentSessionInclude, AgentSessionListQuery, AgentSessionPage, AgentSessionQueryError,
    AgentSessionQueryErrorCode, AgentSessionScope, AgentSessionView, ManagedRunTarget,
    ManagedSessionTarget, RunRecord, SessionFilter, SessionRecord, SessionStore,
};
use starweaver_storage::SqliteStorage;
use starweaver_stream::{DisplayVisibility, ReplayCursor, ReplayScope};

/// Read-only CLI adapter over the selected local shared store.
#[derive(Clone)]
pub struct CliAgentSessionQuery {
    storage: SqliteStorage,
}

impl CliAgentSessionQuery {
    /// Open the adapter over the selected CLI database.
    #[must_use]
    pub const fn new(storage: SqliteStorage) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl AgentSessionQuery for CliAgentSessionQuery {
    async fn list_sessions(
        &self,
        scope: &AgentSessionScope,
        query: AgentSessionListQuery,
    ) -> Result<AgentSessionPage, AgentSessionQueryError> {
        ensure_read_scope(scope)?;
        let limit = query.limit.max(1).min(scope.max_page_size.max(1)) as usize;
        let sessions = self
            .storage
            .session_store()
            .list_sessions(SessionFilter {
                status: query.status,
                profile: query.profile,
                workspace: query.workspace,
                limit: None,
            })
            .await
            .map_err(query_store_error)?;
        let start = decode_page_token(query.page_token.as_deref(), &sessions)?;
        let visible = sessions
            .into_iter()
            .skip(start)
            .filter(|session| authorized(scope, session))
            .take(limit.saturating_add(1))
            .collect::<Vec<_>>();
        let has_more = visible.len() > limit;
        let selected = visible.into_iter().take(limit).collect::<Vec<_>>();
        let mut views = Vec::with_capacity(selected.len());
        for session in &selected {
            views.push(session_view(scope, session, Vec::new()));
        }
        let next_page_token = has_more
            .then(|| {
                selected
                    .last()
                    .map(|session| session.session_id.as_str().to_string())
            })
            .flatten();
        Ok(AgentSessionPage {
            sessions: views,
            next_page_token,
        })
    }

    async fn get_session(
        &self,
        scope: &AgentSessionScope,
        session_id: &SessionId,
        include: AgentSessionInclude,
    ) -> Result<AgentSessionView, AgentSessionQueryError> {
        ensure_read_scope(scope)?;
        let store = self.storage.session_store();
        let session = store
            .load_session(session_id)
            .await
            .map_err(query_store_error)?;
        ensure_authorized(scope, &session)?;
        let recent_runs = if include.recent_runs {
            let mut runs = store
                .list_runs(session_id)
                .await
                .map_err(query_store_error)?;
            let keep = scope.max_page_size.min(10) as usize;
            if runs.len() > keep {
                runs = runs.split_off(runs.len() - keep);
            }
            runs.iter().map(|run| run_view(scope, run, false)).collect()
        } else {
            Vec::new()
        };
        Ok(session_view(scope, &session, recent_runs))
    }

    async fn list_runs(
        &self,
        scope: &AgentSessionScope,
        session_id: &SessionId,
        query: AgentRunListQuery,
    ) -> Result<AgentRunPage, AgentSessionQueryError> {
        ensure_read_scope(scope)?;
        let store = self.storage.session_store();
        let session = store
            .load_session(session_id)
            .await
            .map_err(query_store_error)?;
        ensure_authorized(scope, &session)?;
        let runs = store
            .list_runs(session_id)
            .await
            .map_err(query_store_error)?;
        let start = query
            .page_token
            .as_deref()
            .map(|token| {
                token.parse::<usize>().map_err(|_| AgentSessionQueryError {
                    code: AgentSessionQueryErrorCode::InvalidCursor,
                    message: "invalid run page token".to_string(),
                })
            })
            .transpose()?
            .unwrap_or(0);
        let limit = query.limit.max(1).min(scope.max_page_size.max(1)) as usize;
        let selected = runs
            .iter()
            .filter(|run| run.sequence_no > start)
            .take(limit.saturating_add(1))
            .collect::<Vec<_>>();
        let has_more = selected.len() > limit;
        let selected = selected.into_iter().take(limit).collect::<Vec<_>>();
        let next_page_token = has_more
            .then(|| selected.last().map(|run| run.sequence_no.to_string()))
            .flatten();
        Ok(AgentRunPage {
            runs: selected
                .into_iter()
                .map(|run| run_view(scope, run, false))
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
        ensure_read_scope(scope)?;
        let store = self.storage.session_store();
        let session = store
            .load_session(session_id)
            .await
            .map_err(query_store_error)?;
        ensure_authorized(scope, &session)?;
        let run = store
            .load_run(session_id, run_id)
            .await
            .map_err(query_store_error)?;
        Ok(run_view(scope, &run, false))
    }

    async fn replay_run(
        &self,
        scope: &AgentSessionScope,
        target: ManagedRunTarget,
        query: AgentReplayQuery,
    ) -> Result<AgentDisplayPage, AgentSessionQueryError> {
        ensure_read_scope(scope)?;
        if target.namespace_id != scope.namespace_id {
            return Err(hidden_target());
        }
        let store = self.storage.session_store();
        let session = store
            .load_session(&target.session_id)
            .await
            .map_err(query_store_error)?;
        ensure_authorized(scope, &session)?;
        store
            .load_run(&target.session_id, &target.run_id)
            .await
            .map_err(query_store_error)?;
        if query.after.as_ref().is_some_and(|cursor| {
            cursor.scope != ReplayScope::run(target.run_id.as_str())
                || cursor.family != starweaver_stream::ReplayCursorFamily::Display
        }) {
            return Err(AgentSessionQueryError {
                code: AgentSessionQueryErrorCode::InvalidCursor,
                message: "display replay cursor belongs to another run or family".to_string(),
            });
        }
        let after = query.after.as_ref().map(|cursor| cursor.sequence);
        let storage = self.storage.clone();
        let session_id = target.session_id.clone();
        let run_id = target.run_id.clone();
        let messages = tokio::task::spawn_blocking(move || {
            storage.load_display_messages(&session_id, Some(&run_id), after)
        })
        .await
        .map_err(|_| unavailable("display replay worker failed"))?
        .map_err(query_store_error)?;
        let limit = query.limit.max(1).min(scope.max_page_size.max(1)) as usize;
        let mut messages = messages
            .into_iter()
            .filter(|message| message.visibility == DisplayVisibility::Public)
            .take(limit)
            .map(|mut message| {
                message.payload = Value::Null;
                message.metadata.clear();
                message.preview = message.preview.map(|value| truncate(&value, 2_000));
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

fn session_view(
    _scope: &AgentSessionScope,
    session: &SessionRecord,
    recent_runs: Vec<AgentRunView>,
) -> AgentSessionView {
    AgentSessionView {
        target: ManagedSessionTarget::new(session.namespace_id.clone(), session.session_id.clone()),
        title: session.title.as_deref().map(|value| truncate(value, 256)),
        status: session.status,
        profile: session.profile.as_deref().map(|value| truncate(value, 128)),
        workspace: session.workspace.as_deref().map(safe_workspace),
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

fn run_view(scope: &AgentSessionScope, run: &RunRecord, controllable: bool) -> AgentRunView {
    let input_preview = run
        .input
        .iter()
        .filter_map(|part| match part {
            starweaver_session::InputPart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    AgentRunView {
        target: ManagedRunTarget::new(
            scope.namespace_id.clone(),
            run.session_id.clone(),
            run.run_id.clone(),
        ),
        status: run.status,
        sequence_no: run.sequence_no,
        input_preview: (!input_preview.is_empty()).then(|| truncate(&input_preview, 1_000)),
        output_preview: run
            .output_preview
            .as_deref()
            .map(|value| truncate(value, 2_000)),
        error_category: (run.status == starweaver_session::RunStatus::Failed)
            .then(|| "run_failed".to_string()),
        controllable,
        created_at: run.created_at,
        updated_at: run.updated_at,
    }
}

fn ensure_read_scope(scope: &AgentSessionScope) -> Result<(), AgentSessionQueryError> {
    if scope
        .deadline
        .is_some_and(|deadline| deadline <= chrono::Utc::now())
    {
        return Err(AgentSessionQueryError {
            code: AgentSessionQueryErrorCode::PermissionDenied,
            message: "session capability deadline expired".to_string(),
        });
    }
    if !scope.allows(starweaver_session::AgentSessionOperation::Read) {
        return Err(AgentSessionQueryError {
            code: AgentSessionQueryErrorCode::PermissionDenied,
            message: "session.read is not granted".to_string(),
        });
    }
    Ok(())
}

fn authorized(scope: &AgentSessionScope, session: &SessionRecord) -> bool {
    session.namespace_id == scope.namespace_id
        && session.status != starweaver_session::SessionStatus::Deleted
        && scope.allows_session(&session.session_id)
        && (scope.allow_self_query || scope.source_session_id.as_ref() != Some(&session.session_id))
        && scope.owner_id.as_ref().is_none_or(|owner| {
            session
                .owner_id
                .as_ref()
                .is_some_and(|current| current == owner)
        })
}

fn ensure_authorized(
    scope: &AgentSessionScope,
    session: &SessionRecord,
) -> Result<(), AgentSessionQueryError> {
    if authorized(scope, session) {
        Ok(())
    } else {
        Err(hidden_target())
    }
}

fn decode_page_token(
    token: Option<&str>,
    sessions: &[SessionRecord],
) -> Result<usize, AgentSessionQueryError> {
    let Some(token) = token else {
        return Ok(0);
    };
    sessions
        .iter()
        .position(|session| session.session_id.as_str() == token)
        .map(|index| index.saturating_add(1))
        .ok_or_else(|| AgentSessionQueryError {
            code: AgentSessionQueryErrorCode::InvalidCursor,
            message: "invalid or stale session page token".to_string(),
        })
}

fn safe_workspace(value: &str) -> String {
    std::path::Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| "workspace".to_string(), |name| truncate(name, 128))
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn hidden_target() -> AgentSessionQueryError {
    AgentSessionQueryError {
        code: AgentSessionQueryErrorCode::NotFound,
        message: "session or run was not found".to_string(),
    }
}

fn unavailable(message: &str) -> AgentSessionQueryError {
    AgentSessionQueryError {
        code: AgentSessionQueryErrorCode::Unavailable,
        message: message.to_string(),
    }
}

#[allow(clippy::needless_pass_by_value)]
fn query_store_error(error: starweaver_session::SessionStoreError) -> AgentSessionQueryError {
    match error {
        starweaver_session::SessionStoreError::NotFound(_) => hidden_target(),
        _ => unavailable("canonical session storage is unavailable"),
    }
}
