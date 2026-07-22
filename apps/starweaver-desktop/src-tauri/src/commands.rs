use serde::Serialize;
use tauri::{AppHandle, Manager as _, State, ipc::Channel};

use crate::{
    app_state::{DesktopActivation, DesktopState, EventViewKey},
    generated::host::{
        DesktopHostEventAcknowledgementToken, DesktopHostEventDelivery, DesktopHostInvocation,
        DesktopHostOperationAcknowledgementToken, DesktopHostOperationDelivery,
    },
    platform::PlatformInfo,
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
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn get_desktop_status(app: AppHandle, state: State<'_, DesktopState>) -> DesktopStatus {
    build_desktop_status(&app.package_info().version.to_string(), state.inner())
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
pub async fn execute_host_operation(
    state: State<'_, DesktopState>,
    invocation: DesktopHostInvocation,
) -> Result<DesktopHostOperationDelivery, SupervisorError> {
    state
        .supervisor()
        .execute_renderer_operation(invocation)
        .await
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn list_pending_host_operations(
    state: State<'_, DesktopState>,
) -> Result<Vec<DesktopHostInvocation>, SupervisorError> {
    state.supervisor().pending_renderer_operations()
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
    state: State<'_, DesktopState>,
    scope: crate::generated::host::DesktopHostEventScope,
    on_ready: Channel<String>,
    on_event: Channel<DesktopHostEventDelivery>,
) -> Result<String, SupervisorError> {
    if state.supervisor().status().state != HostChildState::Ready {
        return Err(SupervisorError::not_ready());
    }
    let execution_domain = state.supervisor().event_origin()?;
    let key = DesktopState::event_view_key(execution_domain, &scope);
    let (token, mut cancelled, completion) = state.replace_host_subscription(key.clone()).await?;
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
    });
    Ok(token)
}

#[tauri::command]
pub async fn acknowledge_host_event(
    state: State<'_, DesktopState>,
    acknowledgement_token: DesktopHostEventAcknowledgementToken,
) -> Result<(), SupervisorError> {
    state.acknowledge_event(&acknowledgement_token.0).await
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub async fn unsubscribe_host_events(
    state: State<'_, DesktopState>,
    subscription_token: String,
) -> Result<(), SupervisorError> {
    let Some(mut completed) = state.begin_host_unsubscribe(&subscription_token) else {
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
