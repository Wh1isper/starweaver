use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Manager as _, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder, ipc::Channel,
};
use tauri_plugin_dialog::{DialogExt as _, MessageDialogButtons};

use crate::{
    app_state::{DesktopActivation, DesktopState, DesktopWindowRoute, EventViewKey},
    generated::host::{
        DesktopHostEventAcknowledgementToken, DesktopHostEventDelivery, DesktopHostInvocation,
        DesktopHostOperation, DesktopHostOperationAcknowledgementToken,
        DesktopHostOperationDelivery, DesktopHostResult, SessionId,
    },
    managed_runtime,
    platform::PlatformInfo,
    preferences::{
        DesktopPreferencesError, DesktopPreferencesSnapshot, DesktopPreferencesStore,
        DesktopPreferencesUpdate,
    },
    supervisor::{
        BackendHostEvent, HostChildState, HostSupervisorStatus, LocalHostSupervisor, RunEventTail,
        SupervisorError, SupervisorErrorCode, backend_event_from_notification,
    },
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopStatus {
    app_version: String,
    platform: crate::platform::DesktopPlatform,
    architecture: String,
    launch_generation: u64,
    single_instance: bool,
    runtime: HostSupervisorStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_issue: Option<SupervisorError>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopConversationWindow {
    label: String,
    reused: bool,
    session_id: String,
}

fn build_desktop_status(app_version: &str, state: &DesktopState) -> DesktopStatus {
    let platform = PlatformInfo::current();
    DesktopStatus {
        app_version: app_version.to_string(),
        platform: platform.platform,
        architecture: platform.architecture,
        launch_generation: state.launch_generation(),
        single_instance: true,
        runtime: state.supervisor().status(),
        runtime_issue: state.runtime_issue(),
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn get_desktop_status(app: AppHandle, state: State<'_, DesktopState>) -> DesktopStatus {
    build_desktop_status(&app.package_info().version.to_string(), state.inner())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub async fn retry_managed_runtime(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<(), SupervisorError> {
    state
        .prepare_and_start_managed_runtime(move || managed_runtime::prepare(&app))
        .await
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn get_desktop_preferences(
    preferences: State<'_, DesktopPreferencesStore>,
) -> Result<DesktopPreferencesSnapshot, DesktopPreferencesError> {
    preferences.snapshot()
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn update_desktop_preferences(
    preferences: State<'_, DesktopPreferencesStore>,
    update: DesktopPreferencesUpdate,
) -> Result<DesktopPreferencesSnapshot, DesktopPreferencesError> {
    preferences.update(update)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn reload_desktop_preferences(
    preferences: State<'_, DesktopPreferencesStore>,
) -> Result<DesktopPreferencesSnapshot, DesktopPreferencesError> {
    preferences.reload()
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn subscribe_desktop_activation(
    state: State<'_, DesktopState>,
    on_activation: Channel<DesktopActivation>,
) -> u64 {
    state.subscribe_to_activations(on_activation)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn unsubscribe_desktop_activation(state: State<'_, DesktopState>, subscription_token: u64) {
    state.unsubscribe_from_activations(subscription_token);
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn get_desktop_window_route(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<DesktopWindowRoute, SupervisorError> {
    state
        .window_route(window.label())
        .ok_or_else(|| SupervisorError::invalid_configuration("Desktop window route is invalid"))
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn open_conversation_window(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    session_id: SessionId,
) -> Result<DesktopConversationWindow, SupervisorError> {
    if window.label() != "main" {
        return Err(SupervisorError::invalid_configuration(
            "only the primary Desktop window can open another conversation window",
        ));
    }
    let _window_gate = state.lock_conversation_windows()?;
    let (label, reused) = state.reserve_conversation_window(&session_id.0)?;
    if reused {
        let existing = app.get_webview_window(&label).ok_or_else(|| {
            state.release_window(&label);
            SupervisorError::not_ready()
        })?;
        existing.show().map_err(|_| SupervisorError::transport())?;
        existing
            .unminimize()
            .map_err(|_| SupervisorError::transport())?;
        existing
            .set_focus()
            .map_err(|_| SupervisorError::transport())?;
    } else if WebviewWindowBuilder::new(&app, label.clone(), WebviewUrl::App("index.html".into()))
        .title("Starweaver Conversation")
        .inner_size(960.0, 720.0)
        .min_inner_size(680.0, 520.0)
        .build()
        .is_err()
    {
        state.release_window(&label);
        return Err(SupervisorError::transport());
    }
    Ok(DesktopConversationWindow {
        label,
        reused,
        session_id: session_id.0,
    })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesktopWorkspaceIntent {
    OpenExisting,
    CreateEmpty { name: String },
    Managed,
}

async fn confirm_native_presence(
    app: &AppHandle,
    title: &'static str,
    message: &'static str,
) -> Result<(), SupervisorError> {
    let app = app.clone();
    let accepted = tokio::task::spawn_blocking(move || {
        app.dialog()
            .message(message)
            .title(title)
            .buttons(MessageDialogButtons::OkCancel)
            .blocking_show()
    })
    .await
    .map_err(|_| SupervisorError::transport())?;
    if accepted {
        Ok(())
    } else {
        Err(SupervisorError::invalid_configuration(
            "native user confirmation was cancelled",
        ))
    }
}

fn validate_workspace_name(name: &str) -> Result<(), SupervisorError> {
    let character_count = name.chars().count();
    let normalized = name.trim();
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let windows_reserved = matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );
    if normalized != name
        || character_count == 0
        || character_count > 80
        || matches!(name, "." | "..")
        || name.ends_with('.')
        || name.contains(['/', '\\', ':'])
        || name.chars().any(char::is_control)
        || windows_reserved
    {
        return Err(SupervisorError::invalid_configuration(
            "workspace name must be a portable folder name between 1 and 80 characters",
        ));
    }
    Ok(())
}

fn create_private_workspace(parent: &Path, name: &str) -> Result<PathBuf, SupervisorError> {
    validate_workspace_name(name)?;
    let parent = fs::canonicalize(parent).map_err(|_| {
        SupervisorError::invalid_configuration("workspace parent is not an accessible local folder")
    })?;
    if !parent.is_dir() {
        return Err(SupervisorError::invalid_configuration(
            "workspace parent is not an accessible local folder",
        ));
    }
    let root = parent.join(name);
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(&root).map_err(|_| {
        SupervisorError::invalid_configuration(
            "an empty workspace could not be created with that name",
        )
    })?;
    fs::canonicalize(root).map_err(|_| {
        SupervisorError::invalid_configuration("the new workspace could not be verified")
    })
}

fn exact_workspace_path(path: &Path) -> Result<&str, SupervisorError> {
    path.to_str().ok_or_else(|| {
        SupervisorError::invalid_configuration(
            "workspace path cannot be represented exactly by the local host protocol",
        )
    })
}

async fn materialize_workspace_root(
    app: &AppHandle,
    intent: DesktopWorkspaceIntent,
) -> Result<PathBuf, SupervisorError> {
    let app = app.clone();
    tokio::task::spawn_blocking(move || match intent {
        DesktopWorkspaceIntent::OpenExisting => {
            let selected = app
                .dialog()
                .file()
                .set_title("Open Starweaver workspace")
                .blocking_pick_folder()
                .ok_or_else(|| {
                    SupervisorError::invalid_configuration("native folder selection was cancelled")
                })?;
            let root = selected.into_path().map_err(|_| {
                SupervisorError::invalid_configuration("selected workspace is not a local folder")
            })?;
            fs::canonicalize(root).map_err(|_| {
                SupervisorError::invalid_configuration(
                    "selected workspace is not an accessible local folder",
                )
            })
        }
        DesktopWorkspaceIntent::CreateEmpty { name } => {
            validate_workspace_name(&name)?;
            let selected = app
                .dialog()
                .file()
                .set_title("Choose where to create the workspace")
                .blocking_pick_folder()
                .ok_or_else(|| {
                    SupervisorError::invalid_configuration("native folder selection was cancelled")
                })?;
            let parent = selected.into_path().map_err(|_| {
                SupervisorError::invalid_configuration("workspace parent is not a local folder")
            })?;
            create_private_workspace(&parent, &name)
        }
        DesktopWorkspaceIntent::Managed => {
            let parent = app
                .path()
                .app_local_data_dir()
                .map_err(|_| {
                    SupervisorError::invalid_configuration(
                        "Desktop managed workspace storage is unavailable",
                    )
                })?
                .join("workspaces");
            fs::create_dir_all(&parent).map_err(|_| {
                SupervisorError::invalid_configuration(
                    "Desktop managed workspace storage is unavailable",
                )
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).map_err(|_| {
                    SupervisorError::invalid_configuration(
                        "Desktop managed workspace storage is not private",
                    )
                })?;
            }
            create_private_workspace(&parent, &format!("workspace-{}", uuid::Uuid::new_v4()))
        }
    })
    .await
    .map_err(|_| SupervisorError::transport())?
}

async fn native_operation_fields(
    app: &AppHandle,
    supervisor: &LocalHostSupervisor,
    invocation: &DesktopHostInvocation,
    workspace_intent: Option<DesktopWorkspaceIntent>,
) -> Result<crate::generated::host::SupervisorDynamicFields, SupervisorError> {
    use crate::generated::host::{ConfigReloadMode, DesktopHostOperation};

    let mut fields = crate::generated::host::SupervisorDynamicFields::default();
    match &invocation.operation {
        DesktopHostOperation::WorkspaceRegister(_) => {
            let intent = workspace_intent.unwrap_or(DesktopWorkspaceIntent::OpenExisting);
            let root = materialize_workspace_root(app, intent).await?;
            fields
                .insert("root", exact_workspace_path(&root)?)
                .map_err(|_| SupervisorError::transport())?;
        }
        DesktopHostOperation::ConfigUpdate(_) => {
            confirm_native_presence(
                app,
                "Apply Starweaver configuration",
                "Apply these runtime configuration changes to future runs?",
            )
            .await?;
            return supervisor.config_authorization_fields(invocation).await;
        }
        DesktopHostOperation::ConfigReload(intent) => match intent.mode {
            ConfigReloadMode::DryRun => {
                return supervisor.config_authorization_fields(invocation).await;
            }
            ConfigReloadMode::Commit => {
                intent.candidate_etag.as_ref().ok_or_else(|| {
                    SupervisorError::invalid_configuration(
                        "config reload commit requires a validated candidate etag",
                    )
                })?;
                confirm_native_presence(
                    app,
                    "Reload Starweaver configuration",
                    "Commit the validated runtime configuration reload for future runs?",
                )
                .await?;
                return supervisor.config_authorization_fields(invocation).await;
            }
        },
        DesktopHostOperation::ConfigActivate(_) => {
            confirm_native_presence(
                app,
                "Restart Starweaver runtime",
                "Activate the staged runtime configuration with a supervised restart?",
            )
            .await?;
            return supervisor.config_authorization_fields(invocation).await;
        }
        DesktopHostOperation::ConfigDiscard(_) => {
            confirm_native_presence(
                app,
                "Discard staged Starweaver configuration",
                "Discard the staged runtime configuration and keep the active configuration?",
            )
            .await?;
            return supervisor.config_authorization_fields(invocation).await;
        }
        _ => {
            if workspace_intent.is_some() {
                return Err(SupervisorError::invalid_configuration(
                    "workspace intent requires workspace registration",
                ));
            }
        }
    }
    Ok(fields)
}

fn operation_targets_session_directly(operation: &DesktopHostOperation, session_id: &str) -> bool {
    let targets_session = |candidate: &SessionId| candidate.0 == session_id;
    match operation {
        DesktopHostOperation::ApprovalDecide(intent) => targets_session(&intent.session_id),
        DesktopHostOperation::ApprovalShow(intent) => targets_session(&intent.session_id),
        DesktopHostOperation::ClarificationResolve(intent) => targets_session(&intent.session_id),
        DesktopHostOperation::ClarificationShow(intent) => targets_session(&intent.session_id),
        DesktopHostOperation::DeferredComplete(intent) => targets_session(&intent.session_id),
        DesktopHostOperation::DeferredFail(intent) => targets_session(&intent.session_id),
        DesktopHostOperation::DeferredShow(intent) => targets_session(&intent.session_id),
        DesktopHostOperation::RunInterrupt(intent) => targets_session(&intent.session_id),
        DesktopHostOperation::RunList(intent) => targets_session(&intent.session_id),
        DesktopHostOperation::RunResume(intent) => targets_session(&intent.session_id),
        DesktopHostOperation::RunStart(intent) => targets_session(&intent.session_id),
        DesktopHostOperation::RunStatus(intent) => targets_session(&intent.session_id),
        DesktopHostOperation::RunSteer(intent) => targets_session(&intent.session_id),
        DesktopHostOperation::SessionGet(intent) => targets_session(&intent.session_id),
        _ => false,
    }
}

fn require_window_operation_authority(
    state: &DesktopState,
    window_label: &str,
    operation: &DesktopHostOperation,
) -> Result<(), SupervisorError> {
    let Some(route) = state.window_route(window_label) else {
        return Err(SupervisorError::invalid_configuration(
            "Desktop window route is invalid",
        ));
    };
    let DesktopWindowRoute::Conversation { session_id } = route else {
        return Ok(());
    };
    let targets_session = |candidate: &SessionId| candidate.0 == session_id;
    let allowed = operation_targets_session_directly(operation, &session_id)
        || match operation {
            DesktopHostOperation::ApprovalList(intent) => {
                intent.session_id.as_ref().is_some_and(targets_session)
            }
            DesktopHostOperation::ClarificationList(intent) => {
                intent.session_id.as_ref().is_some_and(targets_session)
            }
            DesktopHostOperation::DeferredList(intent) => {
                intent.session_id.as_ref().is_some_and(targets_session)
            }
            DesktopHostOperation::WorkspaceList(_) => true,
            _ => false,
        };
    if allowed {
        Ok(())
    } else {
        Err(SupervisorError::invalid_configuration(
            "this operation is not available from a conversation window",
        ))
    }
}

fn restrict_workspace_result_to_route(
    delivery: &mut DesktopHostOperationDelivery,
    workspace_id: Option<&str>,
) -> Result<(), SupervisorError> {
    let DesktopHostResult::WorkspaceList(result) = &mut delivery.result else {
        return Ok(());
    };
    let workspaces = result
        .workspaces
        .as_array_mut()
        .ok_or_else(SupervisorError::transport)?;
    workspaces.retain(|workspace| {
        workspace_id.is_some_and(|expected| {
            workspace
                .get("workspaceId")
                .and_then(serde_json::Value::as_str)
                == Some(expected)
                && workspace.get("state").and_then(serde_json::Value::as_str) == Some("active")
        })
    });
    Ok(())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub async fn execute_host_operation(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    invocation: DesktopHostInvocation,
    workspace_intent: Option<DesktopWorkspaceIntent>,
) -> Result<DesktopHostOperationDelivery, SupervisorError> {
    require_window_operation_authority(state.inner(), window.label(), &invocation.operation)?;
    let route = state
        .window_route(window.label())
        .ok_or_else(|| SupervisorError::invalid_configuration("Desktop window route is invalid"))?;
    let supervisor = state.supervisor();
    let routed_workspace_id = if matches!(
        &invocation.operation,
        DesktopHostOperation::WorkspaceList(_)
    ) && let DesktopWindowRoute::Conversation { session_id } = &route
    {
        supervisor.session_workspace_id(session_id).await?
    } else {
        None
    };
    let fields = if let Some(fields) = supervisor.reusable_renderer_operation_fields(&invocation)? {
        fields
    } else {
        native_operation_fields(&app, supervisor, &invocation, workspace_intent).await?
    };
    let mut delivery = supervisor
        .execute_renderer_operation_with_fields(invocation, fields)
        .await?;
    if matches!(route, DesktopWindowRoute::Conversation { .. }) {
        restrict_workspace_result_to_route(&mut delivery, routed_workspace_id.as_deref())?;
    }
    Ok(delivery)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn list_pending_host_operations(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<DesktopHostInvocation>, SupervisorError> {
    let pending = state.supervisor().pending_renderer_operations()?;
    let route = state
        .window_route(window.label())
        .ok_or_else(|| SupervisorError::invalid_configuration("Desktop window route is invalid"))?;
    match route {
        DesktopWindowRoute::Main => Ok(pending),
        DesktopWindowRoute::Conversation { session_id } => Ok(pending
            .into_iter()
            .filter(|invocation| {
                operation_targets_session_directly(&invocation.operation, &session_id)
            })
            .collect()),
    }
}

#[tauri::command]
pub async fn acknowledge_host_operation(
    state: State<'_, DesktopState>,
    acknowledgement_token: DesktopHostOperationAcknowledgementToken,
) -> Result<(), SupervisorError> {
    state
        .supervisor()
        .acknowledge_renderer_operation(&acknowledgement_token)
        .await
}

async fn deliver_renderer_event(
    state: &DesktopState,
    key: &EventViewKey,
    event: BackendHostEvent,
    on_event: &Channel<DesktopHostEventDelivery>,
    cancelled: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<(), SupervisorError> {
    if state.event_was_acknowledged(key, &event.event_id) {
        return state
            .advance_acknowledged_duplicate(key.clone(), &event)
            .await;
    }
    let (delivery, acknowledged) = state.prepare_event_acknowledgement(key.clone(), event)?;
    let acknowledgement_token = delivery.acknowledgement_token.0.clone();
    if on_event.send(delivery).is_err() {
        state.cancel_event_acknowledgement(&acknowledgement_token);
        return Err(SupervisorError::transport());
    }
    tokio::select! {
        biased;
        changed = cancelled.changed() => {
            state.cancel_event_acknowledgement(&acknowledgement_token);
            if changed.is_ok() && *cancelled.borrow() {
                Err(SupervisorError::not_ready())
            } else {
                Err(SupervisorError::transport())
            }
        }
        acknowledged = acknowledged => {
            acknowledged.map_err(|_| SupervisorError::transport())
        }
    }
}

async fn open_renderer_event_tail(
    supervisor: &LocalHostSupervisor,
    state: &DesktopState,
    scope: &crate::generated::host::DesktopHostEventScope,
    key: &EventViewKey,
    on_event: &Channel<DesktopHostEventDelivery>,
    cancelled: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<(RunEventTail, bool), SupervisorError> {
    let mut cursor = state.acknowledged_event_cursor(key);
    let mut expected_generation = None;
    let mut made_progress = false;
    loop {
        let page = supervisor
            .replay_run_event_page(scope, cursor.clone())
            .await?;
        if page.execution_domain != key.execution_domain
            || expected_generation.is_some_and(|generation| generation != page.generation)
        {
            return Err(SupervisorError::not_ready());
        }
        expected_generation = Some(page.generation);
        for event in page.deliveries {
            deliver_renderer_event(state, key, event, on_event, cancelled).await?;
            made_progress = true;
        }
        cursor = Some(page.next_cursor);
        if !page.has_more {
            break;
        }
    }
    let tail = supervisor.open_run_event_tail(scope, cursor).await?;
    if tail.execution_domain != key.execution_domain
        || expected_generation.is_some_and(|generation| generation != tail.generation)
    {
        let _ = supervisor
            .close_event_tail(
                tail.subscription_id.clone(),
                tail.generation,
                &tail.execution_domain,
            )
            .await;
        return Err(SupervisorError::not_ready());
    }
    Ok((tail, made_progress))
}

const EVENT_TAIL_RETRY_MIN: std::time::Duration = std::time::Duration::from_millis(100);
const EVENT_TAIL_RETRY_MAX: std::time::Duration = std::time::Duration::from_secs(2);
const EVENT_TAIL_RETRY_LIMIT: usize = 20;

const fn should_retry_event_tail(code: SupervisorErrorCode, attempts: usize) -> bool {
    attempts < EVENT_TAIL_RETRY_LIMIT
        && matches!(
            code,
            SupervisorErrorCode::NotReady | SupervisorErrorCode::Transport
        )
}

fn consume_event_tail_recovery(
    attempts: &mut usize,
    retry_delay: &mut std::time::Duration,
) -> Option<std::time::Duration> {
    if *attempts >= EVENT_TAIL_RETRY_LIMIT {
        return None;
    }
    *attempts += 1;
    let delay = *retry_delay;
    *retry_delay = (*retry_delay * 2).min(EVENT_TAIL_RETRY_MAX);
    Some(delay)
}

const fn reset_event_tail_recovery(attempts: &mut usize, retry_delay: &mut std::time::Duration) {
    *attempts = 0;
    *retry_delay = EVENT_TAIL_RETRY_MIN;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EventTailCloseAction {
    Recover,
    Stop,
}

const fn event_tail_close_action(
    reason: starweaver_rpc_core::generated::SubscriptionClosedReason,
) -> EventTailCloseAction {
    use starweaver_rpc_core::generated::SubscriptionClosedReason;
    match reason {
        SubscriptionClosedReason::Overflow | SubscriptionClosedReason::SequenceExhausted => {
            EventTailCloseAction::Recover
        }
        SubscriptionClosedReason::Terminal
        | SubscriptionClosedReason::Unsubscribed
        | SubscriptionClosedReason::AuthorizationChanged => EventTailCloseAction::Stop,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EventTailGenerationAction {
    Keep,
    Reopen,
    Stop,
}

const fn event_tail_generation_action(
    state: HostChildState,
    status_generation: u64,
    tail_generation: u64,
) -> EventTailGenerationAction {
    match state {
        HostChildState::Ready if status_generation == tail_generation => {
            EventTailGenerationAction::Keep
        }
        HostChildState::Ready
        | HostChildState::Starting
        | HostChildState::Handshaking
        | HostChildState::Recovering => EventTailGenerationAction::Reopen,
        HostChildState::Unconfigured
        | HostChildState::Draining
        | HostChildState::Stopped
        | HostChildState::Incompatible
        | HostChildState::Failed => EventTailGenerationAction::Stop,
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
pub async fn subscribe_host_events(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    scope: crate::generated::host::DesktopHostEventScope,
    on_ready: Channel<String>,
    on_event: Channel<DesktopHostEventDelivery>,
    on_complete: Channel<String>,
) -> Result<String, SupervisorError> {
    if state.supervisor().status().state != HostChildState::Ready {
        return Err(SupervisorError::not_ready());
    }
    let route = state
        .window_route(window.label())
        .ok_or_else(|| SupervisorError::invalid_configuration("Desktop window route is invalid"))?;
    if let DesktopWindowRoute::Conversation { session_id } = route
        && scope.session_id.0 != session_id
    {
        return Err(SupervisorError::invalid_configuration(
            "a conversation window can subscribe only to its routed session",
        ));
    }
    let execution_domain = state.supervisor().event_origin()?;
    let key = DesktopState::event_view_key(execution_domain, window.label().to_string(), &scope);
    let (token, mut cancelled, completion) = state.replace_host_subscription(key.clone()).await?;
    // A renderer owns an in-memory transcript projection, so every fresh renderer subscription
    // rebuilds that projection from durable origin. The cursor written by this new subscription
    // remains available to its internal tail recovery across host restarts.
    if let Err(error) = state.reset_event_cursor(&key).await {
        let _ = completion.send(true);
        state.complete_host_subscription(&token);
        return Err(error);
    }
    // Publish the backend-issued cancellation handle before replay can deliver an event. The
    // renderer can therefore cancel a setup-time delivery even while this command is waiting for
    // its acknowledgement and before the command response itself has been flushed.
    if on_ready.send(token.clone()).is_err() {
        let _ = completion.send(true);
        state.complete_host_subscription(&token);
        return Err(SupervisorError::transport());
    }
    // Subscribe to process notifications before replay and live-tail admission so no event can be
    // lost in the replay-to-live handoff.
    let notifications = state.supervisor().subscribe_notifications();
    let initial_tail = match open_renderer_event_tail(
        state.supervisor(),
        state.inner(),
        &scope,
        &key,
        &on_event,
        &mut cancelled,
    )
    .await
    {
        Ok((tail, _)) => tail,
        Err(error) => {
            let cancelled_by_renderer = *cancelled.borrow();
            let _ = completion.send(true);
            state.complete_host_subscription(&token);
            if cancelled_by_renderer {
                return Ok(token);
            }
            return Err(error);
        }
    };
    let task_token = token.clone();
    tauri::async_runtime::spawn(async move {
        let mut notifications = notifications;
        let mut tail: Option<RunEventTail> = Some(initial_tail);
        let mut retry_delay = EVENT_TAIL_RETRY_MIN;
        let mut retry_attempts = 0_usize;
        let mut generation_check = tokio::time::interval(std::time::Duration::from_millis(100));
        generation_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if *cancelled.borrow() {
                break;
            }
            if tail.is_none() {
                let state = app.state::<DesktopState>();
                match open_renderer_event_tail(
                    state.supervisor(),
                    state.inner(),
                    &scope,
                    &key,
                    &on_event,
                    &mut cancelled,
                )
                .await
                {
                    Ok((opened, made_progress)) => {
                        // Keep the receiver created before replay/subscribe. Replacing it here can
                        // discard the first live notification after the subscribe response.
                        tail = Some(opened);
                        if made_progress {
                            reset_event_tail_recovery(&mut retry_attempts, &mut retry_delay);
                        }
                    }
                    Err(_) if *cancelled.borrow() => break,
                    Err(error) if should_retry_event_tail(error.code, retry_attempts) => {
                        let Some(delay) =
                            consume_event_tail_recovery(&mut retry_attempts, &mut retry_delay)
                        else {
                            break;
                        };
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    Err(_) => break,
                }
            }
            let Some(active) = tail.as_ref() else {
                continue;
            };
            let subscription_id = active.subscription_id.clone();
            let subscription_generation = active.generation;
            let active_domain = active.execution_domain.clone();
            tokio::select! {
                biased;
                changed = cancelled.changed() => {
                    if changed.is_err() || *cancelled.borrow() { break; }
                }
                _ = generation_check.tick() => {
                    let state = app.state::<DesktopState>();
                    let status = state.supervisor().status();
                    match event_tail_generation_action(
                        status.state,
                        status.generation,
                        subscription_generation,
                    ) {
                        EventTailGenerationAction::Keep => {}
                        EventTailGenerationAction::Reopen => {
                            let Some(delay) = consume_event_tail_recovery(
                                &mut retry_attempts,
                                &mut retry_delay,
                            ) else {
                                break;
                            };
                            let _ = state.supervisor().close_event_tail(
                                subscription_id,
                                subscription_generation,
                                &active_domain,
                            ).await;
                            tail = None;
                            tokio::time::sleep(delay).await;
                        }
                        EventTailGenerationAction::Stop => break,
                    }
                }
                notification = notifications.recv() => {
                    let notification = match notification {
                        Ok(notification) => notification,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let Some(delay) = consume_event_tail_recovery(
                                &mut retry_attempts,
                                &mut retry_delay,
                            ) else {
                                break;
                            };
                            let state = app.state::<DesktopState>();
                            let _ = state.supervisor().close_event_tail(
                                subscription_id,
                                subscription_generation,
                                &active_domain,
                            ).await;
                            tail = None;
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    };
                    match &notification.params {
                        starweaver_rpc_core::generated::HostNotificationParams::HostEvent(params)
                            if params.subscription_id == subscription_id => {}
                        starweaver_rpc_core::generated::HostNotificationParams::SubscriptionClosed(params)
                            if params.subscription_id == subscription_id => {
                                if event_tail_close_action(params.reason) == EventTailCloseAction::Stop {
                                    break;
                                }
                                let Some(delay) = consume_event_tail_recovery(
                                    &mut retry_attempts,
                                    &mut retry_delay,
                                ) else {
                                    break;
                                };
                                tail = None;
                                tokio::time::sleep(delay).await;
                                continue;
                            }
                        _ => continue,
                    }
                    let Ok(event) = backend_event_from_notification(notification) else {
                        let Some(delay) = consume_event_tail_recovery(
                            &mut retry_attempts,
                            &mut retry_delay,
                        ) else {
                            break;
                        };
                        let state = app.state::<DesktopState>();
                        let _ = state.supervisor().close_event_tail(
                            subscription_id,
                            subscription_generation,
                            &active_domain,
                        ).await;
                        tail = None;
                        tokio::time::sleep(delay).await;
                        continue;
                    };
                    let state = app.state::<DesktopState>();
                    if deliver_renderer_event(
                        state.inner(),
                        &key,
                        event,
                        &on_event,
                        &mut cancelled,
                    )
                    .await
                    .is_err()
                    {
                        // A failed renderer channel or acknowledgement ends renderer ownership.
                        // Release this scope so a reloaded renderer can resume from durable cursor.
                        break;
                    }
                    reset_event_tail_recovery(&mut retry_attempts, &mut retry_delay);
                }
            }
        }
        let state = app.state::<DesktopState>();
        if let Some(active) = tail {
            let _ = state
                .supervisor()
                .close_event_tail(
                    active.subscription_id,
                    active.generation,
                    &active.execution_domain,
                )
                .await;
        }
        let _ = completion.send(true);
        state.complete_host_subscription(&task_token);
        let _ = on_complete.send(task_token.clone());
    });
    Ok(token)
}

#[tauri::command]
pub async fn acknowledge_host_event(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    acknowledgement_token: DesktopHostEventAcknowledgementToken,
) -> Result<(), SupervisorError> {
    state
        .acknowledge_event(window.label(), &acknowledgement_token.0)
        .await
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub async fn unsubscribe_host_events(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    subscription_token: String,
) -> Result<(), SupervisorError> {
    let Some(mut completed) = state.begin_host_unsubscribe(window.label(), &subscription_token)?
    else {
        return Ok(());
    };
    if !*completed.borrow() {
        completed
            .changed()
            .await
            .map_err(|_| SupervisorError::transport())?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn status_is_safe_and_reports_unconfigured_runtime() {
        let state = DesktopState::default();
        let status = build_desktop_status("0.9.0", &state);

        assert_eq!(status.app_version, "0.9.0");
        assert_eq!(status.launch_generation, 1);
        assert!(status.single_instance);
        assert_eq!(status.runtime.state, HostChildState::Unconfigured);
        assert_eq!(status.runtime.generation, 0);
        assert!(!status.runtime.diagnostics_available);
    }

    #[test]
    fn conversation_window_operations_are_route_scoped() {
        let state = DesktopState::default();
        let (label, _) = state
            .reserve_conversation_window("session-one")
            .expect("conversation route");
        let matching_run: DesktopHostOperation = serde_json::from_value(serde_json::json!({
            "kind": "run.status",
            "input": {"sessionId": "session-one", "runId": "run-one"}
        }))
        .expect("matching run operation");
        let other_run: DesktopHostOperation = serde_json::from_value(serde_json::json!({
            "kind": "run.status",
            "input": {"sessionId": "session-other", "runId": "run-one"}
        }))
        .expect("other run operation");
        let matching_run_list: DesktopHostOperation = serde_json::from_value(serde_json::json!({
            "kind": "run.list",
            "input": {"pageToken": null, "sessionId": "session-one"}
        }))
        .expect("matching run list operation");
        let other_run_list: DesktopHostOperation = serde_json::from_value(serde_json::json!({
            "kind": "run.list",
            "input": {"pageToken": null, "sessionId": "session-other"}
        }))
        .expect("other run list operation");
        let config: DesktopHostOperation = serde_json::from_value(serde_json::json!({
            "kind": "config.get",
            "input": {}
        }))
        .expect("config operation");

        require_window_operation_authority(&state, &label, &matching_run)
            .expect("matching run authority");
        require_window_operation_authority(&state, &label, &matching_run_list)
            .expect("matching run list authority");
        assert!(require_window_operation_authority(&state, &label, &other_run).is_err());
        assert!(require_window_operation_authority(&state, &label, &other_run_list).is_err());
        assert!(require_window_operation_authority(&state, &label, &config).is_err());
        require_window_operation_authority(&state, "main", &config)
            .expect("main window config authority");
    }

    #[test]
    fn interaction_operations_are_direct_session_recovery_targets() {
        let matching = "session-one";
        let operations = [
            serde_json::json!({
                "kind": "approval.decide",
                "input": {
                    "approvalId": "approval-one",
                    "decision": "approved",
                    "expectedRevision": "1",
                    "sessionId": matching
                }
            }),
            serde_json::json!({
                "kind": "approval.show",
                "input": {"approvalId": "approval-one", "sessionId": matching}
            }),
            serde_json::json!({
                "kind": "clarification.resolve",
                "input": {
                    "answers": [],
                    "clarificationId": "clarification-one",
                    "expectedRevision": "1",
                    "sessionId": matching
                }
            }),
            serde_json::json!({
                "kind": "clarification.show",
                "input": {"clarificationId": "clarification-one", "sessionId": matching}
            }),
            serde_json::json!({
                "kind": "deferred.complete",
                "input": {
                    "deferredId": "deferred-one",
                    "expectedRevision": "1",
                    "resultText": "done",
                    "sessionId": matching
                }
            }),
            serde_json::json!({
                "kind": "deferred.fail",
                "input": {
                    "deferredId": "deferred-one",
                    "error": "failed",
                    "expectedRevision": "1",
                    "sessionId": matching
                }
            }),
            serde_json::json!({
                "kind": "deferred.show",
                "input": {"deferredId": "deferred-one", "sessionId": matching}
            }),
        ];

        for value in operations {
            let operation: DesktopHostOperation =
                serde_json::from_value(value.clone()).expect("matching interaction operation");
            assert!(operation_targets_session_directly(&operation, matching));
            assert!(!operation_targets_session_directly(
                &operation,
                "session-other"
            ));
        }
    }

    #[test]
    fn conversation_workspace_projection_keeps_only_the_routed_workspace() {
        let mut delivery = DesktopHostOperationDelivery {
            acknowledgement_token: None,
            result: DesktopHostResult::WorkspaceList(crate::generated::host::WorkspaceListResult {
                page: crate::generated::host::DesktopPage {
                    has_more: false,
                    next_page_token: None,
                },
                workspaces: serde_json::json!([
                    {"workspaceId": "workspace-routed", "displayLabel": "Routed", "state": "active"},
                    {"workspaceId": "workspace-other", "displayLabel": "Other", "state": "active"}
                ]),
            }),
        };

        restrict_workspace_result_to_route(&mut delivery, Some("workspace-routed"))
            .expect("workspace projection");
        let DesktopHostResult::WorkspaceList(result) = delivery.result else {
            panic!("workspace list result");
        };
        assert_eq!(
            result.workspaces,
            serde_json::json!([
                {"workspaceId": "workspace-routed", "displayLabel": "Routed", "state": "active"}
            ])
        );

        let mut revoked = DesktopHostOperationDelivery {
            acknowledgement_token: None,
            result: DesktopHostResult::WorkspaceList(crate::generated::host::WorkspaceListResult {
                page: crate::generated::host::DesktopPage {
                    has_more: false,
                    next_page_token: None,
                },
                workspaces: serde_json::json!([
                    {"workspaceId": "workspace-routed", "displayLabel": "Routed", "state": "revoked"}
                ]),
            }),
        };
        restrict_workspace_result_to_route(&mut revoked, Some("workspace-routed"))
            .expect("revoked workspace projection");
        let DesktopHostResult::WorkspaceList(result) = revoked.result else {
            panic!("workspace list result");
        };
        assert_eq!(result.workspaces, serde_json::json!([]));
    }

    #[test]
    fn workspace_names_are_portable_and_bounded() {
        for valid in ["Project", "project-alpha", "研究"] {
            validate_workspace_name(valid).expect("portable workspace name");
        }
        for invalid in [
            "",
            ".",
            "..",
            " project",
            "project ",
            "project/child",
            "project\\child",
            "project:name",
            "NUL",
            "com1.txt",
            "project.",
        ] {
            assert!(
                validate_workspace_name(invalid).is_err(),
                "{invalid:?} must be rejected"
            );
        }
        assert!(validate_workspace_name(&"x".repeat(81)).is_err());
    }

    #[test]
    fn empty_workspace_creation_is_private_and_never_reuses_an_existing_name() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let root = create_private_workspace(temp.path(), "Project").expect("new workspace");

        assert!(root.is_dir());
        assert!(create_private_workspace(temp.path(), "Project").is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(root)
                    .expect("workspace metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn event_tail_recovery_is_transient_only_and_budgeted() {
        assert!(should_retry_event_tail(SupervisorErrorCode::NotReady, 0));
        assert!(should_retry_event_tail(
            SupervisorErrorCode::Transport,
            EVENT_TAIL_RETRY_LIMIT - 1
        ));
        assert!(!should_retry_event_tail(
            SupervisorErrorCode::Transport,
            EVENT_TAIL_RETRY_LIMIT
        ));
        for code in [
            SupervisorErrorCode::InvalidConfiguration,
            SupervisorErrorCode::Remote,
            SupervisorErrorCode::Incompatible,
            SupervisorErrorCode::Internal,
        ] {
            assert!(!should_retry_event_tail(code, 0));
        }
    }

    #[test]
    fn event_tail_recovery_budget_covers_post_admission_failures() {
        let mut attempts = 0;
        let mut delay = EVENT_TAIL_RETRY_MIN;
        let mut observed = Vec::new();
        for _ in 0..EVENT_TAIL_RETRY_LIMIT {
            if let Some(value) = consume_event_tail_recovery(&mut attempts, &mut delay) {
                observed.push(value);
            }
        }
        assert_eq!(observed.len(), EVENT_TAIL_RETRY_LIMIT);
        assert_eq!(attempts, EVENT_TAIL_RETRY_LIMIT);
        assert_eq!(observed.first(), Some(&EVENT_TAIL_RETRY_MIN));
        assert_eq!(observed.last(), Some(&EVENT_TAIL_RETRY_MAX));
        assert_eq!(consume_event_tail_recovery(&mut attempts, &mut delay), None);

        reset_event_tail_recovery(&mut attempts, &mut delay);
        assert_eq!(attempts, 0);
        assert_eq!(delay, EVENT_TAIL_RETRY_MIN);
    }

    #[test]
    fn event_tail_close_recovery_is_reason_aware() {
        use starweaver_rpc_core::generated::SubscriptionClosedReason;

        for reason in [
            SubscriptionClosedReason::Overflow,
            SubscriptionClosedReason::SequenceExhausted,
        ] {
            assert_eq!(
                event_tail_close_action(reason),
                EventTailCloseAction::Recover
            );
        }
        for reason in [
            SubscriptionClosedReason::Terminal,
            SubscriptionClosedReason::Unsubscribed,
            SubscriptionClosedReason::AuthorizationChanged,
        ] {
            assert_eq!(event_tail_close_action(reason), EventTailCloseAction::Stop);
        }
    }

    #[test]
    fn event_tail_generation_monitor_stops_on_supervisor_terminal_states() {
        assert_eq!(
            event_tail_generation_action(HostChildState::Ready, 7, 7),
            EventTailGenerationAction::Keep
        );
        assert_eq!(
            event_tail_generation_action(HostChildState::Ready, 8, 7),
            EventTailGenerationAction::Reopen
        );
        for state in [
            HostChildState::Starting,
            HostChildState::Handshaking,
            HostChildState::Recovering,
        ] {
            assert_eq!(
                event_tail_generation_action(state, 8, 7),
                EventTailGenerationAction::Reopen
            );
        }
        for state in [
            HostChildState::Unconfigured,
            HostChildState::Draining,
            HostChildState::Stopped,
            HostChildState::Incompatible,
            HostChildState::Failed,
        ] {
            assert_eq!(
                event_tail_generation_action(state, 8, 7),
                EventTailGenerationAction::Stop
            );
        }
    }
}
