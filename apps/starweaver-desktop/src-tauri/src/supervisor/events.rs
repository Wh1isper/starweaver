use starweaver_rpc_core::generated as host;

use crate::generated::host as bridge;

use super::{HostChildState, LocalHostSupervisor, SupervisorError, SupervisorErrorCode};

pub struct RunEventTail {
    pub subscription_id: host::SubscriptionId,
    pub generation: u64,
    pub execution_domain: String,
}

pub struct BackendHostEvent {
    pub event: bridge::SafeHostEvent,
    pub cursor: String,
    pub event_id: String,
}

pub struct BackendHostEventPage {
    pub deliveries: Vec<BackendHostEvent>,
    pub next_cursor: String,
    pub has_more: bool,
    pub generation: u64,
    pub execution_domain: String,
}

impl LocalHostSupervisor {
    pub(crate) fn event_origin(&self) -> Result<String, SupervisorError> {
        self.shared.ready_domain().map(|(_, domain)| domain)
    }

    pub(crate) async fn session_workspace_id(
        &self,
        session_id: &str,
    ) -> Result<Option<String>, SupervisorError> {
        let (generation, execution_domain) = self.shared.ready_domain()?;
        let request = host::HostRequest {
            id: host::RequestId::new(self.next_request_id(generation))
                .map_err(|_| SupervisorError::transport())?,
            call: host::HostCall::SessionGet(host::SessionGetParams {
                run_limit: 1,
                session_id: host::SessionId::new(session_id)
                    .map_err(|_| SupervisorError::invalid_configuration("session ID is invalid"))?,
            }),
        };
        let result = self
            .execute_fenced(request, generation, &execution_domain)
            .await?;
        match result {
            host::HostResult::SessionGet(result) => Ok(result
                .session
                .workspace_id
                .map(|workspace_id| workspace_id.as_str().to_string())),
            _ => Err(SupervisorError::transport()),
        }
    }

    pub(crate) async fn replay_run_event_page(
        &self,
        scope: &bridge::DesktopHostEventScope,
        cursor: Option<String>,
    ) -> Result<BackendHostEventPage, SupervisorError> {
        let (generation, execution_domain) = self.shared.ready_domain()?;
        let view = desktop_event_view(scope)?;
        let wire_cursor = cursor
            .as_deref()
            .map(host::HostEventCursor::new)
            .transpose()
            .map_err(|_| SupervisorError::invalid_configuration("event cursor is invalid"))?;
        let request = host::HostRequest {
            id: host::RequestId::new(self.next_request_id(generation)).map_err(|_| {
                SupervisorError::new(SupervisorErrorCode::Internal, "request identity failed")
            })?,
            call: host::HostCall::EventsReplay(host::EventsReplayParams {
                cursor: wire_cursor,
                limit: 500,
                view,
            }),
        };
        let host::HostResult::EventsReplay(result) = self
            .send_actor_fenced(request, generation, &execution_domain)
            .await?
        else {
            return Err(SupervisorError::transport());
        };
        if result.has_more && cursor.as_deref() == Some(result.next_cursor.as_str()) {
            return Err(SupervisorError::transport());
        }
        let mut deliveries = Vec::with_capacity(result.deliveries.len());
        for (index, delivery) in result.deliveries.into_iter().enumerate() {
            let sequence = u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1);
            deliveries.push(backend_event_from_delivery(delivery, sequence)?);
        }
        Ok(BackendHostEventPage {
            deliveries,
            next_cursor: result.next_cursor.as_str().to_string(),
            has_more: result.has_more,
            generation,
            execution_domain,
        })
    }

    pub(crate) async fn open_run_event_tail(
        &self,
        scope: &bridge::DesktopHostEventScope,
        cursor: Option<String>,
    ) -> Result<RunEventTail, SupervisorError> {
        let (generation, execution_domain) = self.shared.ready_domain()?;
        let wire_cursor = cursor
            .as_deref()
            .map(host::HostEventCursor::new)
            .transpose()
            .map_err(|_| SupervisorError::invalid_configuration("event cursor is invalid"))?;
        let request = host::HostRequest {
            id: host::RequestId::new(self.next_request_id(generation)).map_err(|_| {
                SupervisorError::new(SupervisorErrorCode::Internal, "request identity failed")
            })?,
            call: host::HostCall::EventsSubscribe(host::EventsSubscribeParams {
                cursor: wire_cursor,
                view: desktop_event_view(scope)?,
            }),
        };
        let host::HostResult::EventsSubscribe(result) = self
            .send_actor_fenced(request, generation, &execution_domain)
            .await?
        else {
            return Err(SupervisorError::transport());
        };
        Ok(RunEventTail {
            subscription_id: result.subscription_id,
            generation,
            execution_domain,
        })
    }

    pub(crate) async fn close_event_tail(
        &self,
        subscription_id: host::SubscriptionId,
        generation: u64,
        execution_domain: &str,
    ) -> Result<(), SupervisorError> {
        if self.status().state != HostChildState::Ready {
            return Ok(());
        }
        let request = host::HostRequest {
            id: host::RequestId::new(self.next_request_id(generation)).map_err(|_| {
                SupervisorError::new(SupervisorErrorCode::Internal, "request identity failed")
            })?,
            call: host::HostCall::EventsUnsubscribe(host::EventsUnsubscribeParams {
                subscription_id,
            }),
        };
        let host::HostResult::EventsUnsubscribe(_) = self
            .send_actor_fenced(request, generation, execution_domain)
            .await?
        else {
            return Err(SupervisorError::transport());
        };
        Ok(())
    }
}

fn desktop_event_view(
    scope: &bridge::DesktopHostEventScope,
) -> Result<host::EventViewRequest, SupervisorError> {
    let session_id = host::SessionId::new(scope.session_id.0.clone()).map_err(|_| {
        SupervisorError::new(
            SupervisorErrorCode::InvalidConfiguration,
            "invalid event scope",
        )
    })?;
    let run_id = host::RunId::new(scope.run_id.0.clone()).map_err(|_| {
        SupervisorError::new(
            SupervisorErrorCode::InvalidConfiguration,
            "invalid event scope",
        )
    })?;
    Ok(host::EventViewRequest {
        optional_features: vec![
            "clarifications".to_string(),
            "hitl".to_string(),
            "runs".to_string(),
        ],
        profile: host::EventProfile::DesktopConversationV1,
        scope: host::ResourceScope::RunResourceScope(host::RunResourceScope {
            kind: host::RunResourceScopeKind::Value,
            run_id,
            session_id,
        }),
    })
}

fn backend_event_from_delivery(
    delivery: host::EventDelivery,
    sequence: u64,
) -> Result<BackendHostEvent, SupervisorError> {
    let cursor = delivery.cursor.as_str().to_string();
    let event_id = delivery.record.event_id.as_str().to_string();
    let notification = host::HostNotification {
        params: host::HostNotificationParams::HostEvent(Box::new(
            host::HostEventNotificationParams {
                delivery,
                delivery_sequence: host::DecimalU64::new(sequence),
                subscription_id: host::SubscriptionId::new("desktop-projection")
                    .map_err(|_| SupervisorError::transport())?,
            },
        )),
    };
    let event = bridge::project_host_notification(notification).map_err(|_| {
        SupervisorError::new(
            SupervisorErrorCode::Internal,
            "host event projection failed",
        )
    })?;
    Ok(BackendHostEvent {
        event,
        cursor,
        event_id,
    })
}

pub fn backend_event_from_notification(
    notification: host::HostNotification,
) -> Result<BackendHostEvent, SupervisorError> {
    let host::HostNotificationParams::HostEvent(params) = notification.params else {
        return Err(SupervisorError::transport());
    };
    backend_event_from_delivery(params.delivery, params.delivery_sequence.get())
}
