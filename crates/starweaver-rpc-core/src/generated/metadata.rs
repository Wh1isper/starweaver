//! Generated method, notification, event-class, and event-profile metadata.

use super::types::EventProfile;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transport {
    Stdio,
    Http,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Idempotency {
    None,
    Idempotent,
    Effectful,
    Connection,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MethodMetadata {
    pub method: Method,
    pub name: &'static str,
    pub features: &'static [&'static str],
    pub transports: &'static [Transport],
    pub scopes: &'static [&'static str],
    pub idempotency: Idempotency,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NotificationMetadata {
    pub notification: Notification,
    pub name: &'static str,
    pub features: &'static [&'static str],
    pub transports: &'static [Transport],
    pub scopes: &'static [&'static str],
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventClassMetadata {
    pub event_class: EventClass,
    pub name: &'static str,
    pub schema_type: &'static str,
    pub feature: Option<&'static str>,
    pub scopes: &'static [&'static str],
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventProfileMetadata {
    pub profile: EventProfile,
    pub name: &'static str,
    pub event_classes: &'static [EventClass],
}
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EventClass {
    ApprovalChanged,
    ClarificationChanged,
    DeferredChanged,
    Diagnostic,
    EnvironmentChanged,
    OutputAvailable,
    RunChanged,
    SessionChanged,
}
pub const EVENT_CLASSES: &[EventClassMetadata] = &[
    EventClassMetadata {
        event_class: EventClass::ApprovalChanged,
        name: "approval_changed",
        schema_type: "ApprovalChangedEvent",
        feature: Some("hitl"),
        scopes: &["approval"],
    },
    EventClassMetadata {
        event_class: EventClass::ClarificationChanged,
        name: "clarification_changed",
        schema_type: "ClarificationChangedEvent",
        feature: Some("clarifications"),
        scopes: &["approval"],
    },
    EventClassMetadata {
        event_class: EventClass::DeferredChanged,
        name: "deferred_changed",
        schema_type: "DeferredChangedEvent",
        feature: Some("hitl"),
        scopes: &["approval"],
    },
    EventClassMetadata {
        event_class: EventClass::Diagnostic,
        name: "diagnostic",
        schema_type: "DiagnosticEvent",
        feature: Some("diagnostics.safe"),
        scopes: &["admin"],
    },
    EventClassMetadata {
        event_class: EventClass::EnvironmentChanged,
        name: "environment_changed",
        schema_type: "EnvironmentChangedEvent",
        feature: Some("environment.attachments"),
        scopes: &["run"],
    },
    EventClassMetadata {
        event_class: EventClass::OutputAvailable,
        name: "output_available",
        schema_type: "OutputAvailableEvent",
        feature: Some("runs"),
        scopes: &["run"],
    },
    EventClassMetadata {
        event_class: EventClass::RunChanged,
        name: "run_changed",
        schema_type: "RunChangedEvent",
        feature: Some("runs"),
        scopes: &["run"],
    },
    EventClassMetadata {
        event_class: EventClass::SessionChanged,
        name: "session_changed",
        schema_type: "SessionChangedEvent",
        feature: Some("sessions"),
        scopes: &["read"],
    },
];
impl EventClass {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "approval_changed" => Some(Self::ApprovalChanged),
            "clarification_changed" => Some(Self::ClarificationChanged),
            "deferred_changed" => Some(Self::DeferredChanged),
            "diagnostic" => Some(Self::Diagnostic),
            "environment_changed" => Some(Self::EnvironmentChanged),
            "output_available" => Some(Self::OutputAvailable),
            "run_changed" => Some(Self::RunChanged),
            "session_changed" => Some(Self::SessionChanged),
            _ => None,
        }
    }
    #[must_use]
    pub fn metadata(self) -> &'static EventClassMetadata {
        EVENT_CLASSES
            .iter()
            .find(|entry| entry.event_class == self)
            .expect("generated event-class metadata is exhaustive")
    }
    #[must_use]
    pub fn is_admitted(self, features: &[&str], scopes: &[&str]) -> bool {
        let metadata = self.metadata();
        let feature_admitted = metadata
            .feature
            .is_none_or(|feature| features.contains(&feature));
        feature_admitted && metadata.scopes.iter().all(|scope| scopes.contains(scope))
    }
}
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Method {
    ApprovalDecide,
    ApprovalList,
    ApprovalShow,
    CatalogList,
    ClarificationResolve,
    DeferredComplete,
    DeferredFail,
    DeferredList,
    DeferredShow,
    DiagnosticsGet,
    EnvironmentAttach,
    EnvironmentDetach,
    EnvironmentHealth,
    EnvironmentList,
    EnvironmentMount,
    EnvironmentMountsList,
    EnvironmentUnmount,
    EventsReplay,
    EventsSubscribe,
    EventsUnsubscribe,
    Initialize,
    ModelSelect,
    ModelSelectionGet,
    ProfileGet,
    RunInterrupt,
    RunResume,
    RunStart,
    RunStatus,
    RunSteer,
    SessionCreate,
    SessionDelete,
    SessionFork,
    SessionGet,
    SessionList,
    SessionSearch,
    Shutdown,
}
pub const METHODS: &[MethodMetadata] = &[
    MethodMetadata {
        method: Method::ApprovalDecide,
        name: "approval.decide",
        features: &["hitl"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["approval"],
        idempotency: Idempotency::Idempotent,
    },
    MethodMetadata {
        method: Method::ApprovalList,
        name: "approval.list",
        features: &["hitl"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["read"],
        idempotency: Idempotency::None,
    },
    MethodMetadata {
        method: Method::ApprovalShow,
        name: "approval.show",
        features: &["hitl"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["read"],
        idempotency: Idempotency::None,
    },
    MethodMetadata {
        method: Method::CatalogList,
        name: "catalog.list",
        features: &["profiles"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["read"],
        idempotency: Idempotency::None,
    },
    MethodMetadata {
        method: Method::ClarificationResolve,
        name: "clarification.resolve",
        features: &["clarifications", "hitl"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["approval"],
        idempotency: Idempotency::Idempotent,
    },
    MethodMetadata {
        method: Method::DeferredComplete,
        name: "deferred.complete",
        features: &["hitl"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["approval"],
        idempotency: Idempotency::Idempotent,
    },
    MethodMetadata {
        method: Method::DeferredFail,
        name: "deferred.fail",
        features: &["hitl"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["approval"],
        idempotency: Idempotency::Idempotent,
    },
    MethodMetadata {
        method: Method::DeferredList,
        name: "deferred.list",
        features: &["hitl"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["read"],
        idempotency: Idempotency::None,
    },
    MethodMetadata {
        method: Method::DeferredShow,
        name: "deferred.show",
        features: &["hitl"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["read"],
        idempotency: Idempotency::None,
    },
    MethodMetadata {
        method: Method::DiagnosticsGet,
        name: "diagnostics.get",
        features: &["diagnostics.safe"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["read"],
        idempotency: Idempotency::None,
    },
    MethodMetadata {
        method: Method::EnvironmentAttach,
        name: "environment.attach",
        features: &["environment.attachments"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["run"],
        idempotency: Idempotency::Idempotent,
    },
    MethodMetadata {
        method: Method::EnvironmentDetach,
        name: "environment.detach",
        features: &["environment.attachments"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["run"],
        idempotency: Idempotency::Idempotent,
    },
    MethodMetadata {
        method: Method::EnvironmentHealth,
        name: "environment.health",
        features: &["environment.attachments"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["run"],
        idempotency: Idempotency::None,
    },
    MethodMetadata {
        method: Method::EnvironmentList,
        name: "environment.list",
        features: &["environment.attachments"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["read"],
        idempotency: Idempotency::None,
    },
    MethodMetadata {
        method: Method::EnvironmentMount,
        name: "environment.mount",
        features: &["environment.mounts"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["run"],
        idempotency: Idempotency::Idempotent,
    },
    MethodMetadata {
        method: Method::EnvironmentMountsList,
        name: "environment.mounts.list",
        features: &["environment.mounts"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["read"],
        idempotency: Idempotency::None,
    },
    MethodMetadata {
        method: Method::EnvironmentUnmount,
        name: "environment.unmount",
        features: &["environment.mounts"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["run"],
        idempotency: Idempotency::Idempotent,
    },
    MethodMetadata {
        method: Method::EventsReplay,
        name: "events.replay",
        features: &["events.replay"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["read"],
        idempotency: Idempotency::None,
    },
    MethodMetadata {
        method: Method::EventsSubscribe,
        name: "events.subscribe",
        features: &["events.subscribe"],
        transports: &[Transport::Stdio],
        scopes: &["read"],
        idempotency: Idempotency::Connection,
    },
    MethodMetadata {
        method: Method::EventsUnsubscribe,
        name: "events.unsubscribe",
        features: &["events.subscribe"],
        transports: &[Transport::Stdio],
        scopes: &["read"],
        idempotency: Idempotency::Connection,
    },
    MethodMetadata {
        method: Method::Initialize,
        name: "initialize",
        features: &[],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["public"],
        idempotency: Idempotency::None,
    },
    MethodMetadata {
        method: Method::ModelSelect,
        name: "model.select",
        features: &["profiles"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["admin"],
        idempotency: Idempotency::Idempotent,
    },
    MethodMetadata {
        method: Method::ModelSelectionGet,
        name: "model.selection.get",
        features: &["profiles"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["read"],
        idempotency: Idempotency::None,
    },
    MethodMetadata {
        method: Method::ProfileGet,
        name: "profile.get",
        features: &["profiles"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["read"],
        idempotency: Idempotency::None,
    },
    MethodMetadata {
        method: Method::RunInterrupt,
        name: "run.interrupt",
        features: &["runs"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["run"],
        idempotency: Idempotency::Idempotent,
    },
    MethodMetadata {
        method: Method::RunResume,
        name: "run.resume",
        features: &["hitl", "runs"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["run"],
        idempotency: Idempotency::Idempotent,
    },
    MethodMetadata {
        method: Method::RunStart,
        name: "run.start",
        features: &["runs"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["run"],
        idempotency: Idempotency::Idempotent,
    },
    MethodMetadata {
        method: Method::RunStatus,
        name: "run.status",
        features: &["runs"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["read"],
        idempotency: Idempotency::None,
    },
    MethodMetadata {
        method: Method::RunSteer,
        name: "run.steer",
        features: &["runs", "steering"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["run"],
        idempotency: Idempotency::Idempotent,
    },
    MethodMetadata {
        method: Method::SessionCreate,
        name: "session.create",
        features: &["sessions"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["run"],
        idempotency: Idempotency::Idempotent,
    },
    MethodMetadata {
        method: Method::SessionDelete,
        name: "session.delete",
        features: &["sessions"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["admin"],
        idempotency: Idempotency::Idempotent,
    },
    MethodMetadata {
        method: Method::SessionFork,
        name: "session.fork",
        features: &["session.fork", "sessions"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["run"],
        idempotency: Idempotency::Idempotent,
    },
    MethodMetadata {
        method: Method::SessionGet,
        name: "session.get",
        features: &["sessions"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["read"],
        idempotency: Idempotency::None,
    },
    MethodMetadata {
        method: Method::SessionList,
        name: "session.list",
        features: &["sessions"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["read"],
        idempotency: Idempotency::None,
    },
    MethodMetadata {
        method: Method::SessionSearch,
        name: "session.search",
        features: &["session.search", "sessions"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["read"],
        idempotency: Idempotency::None,
    },
    MethodMetadata {
        method: Method::Shutdown,
        name: "shutdown",
        features: &["host.shutdown"],
        transports: &[Transport::Stdio, Transport::Http],
        scopes: &["shutdown"],
        idempotency: Idempotency::Effectful,
    },
];
impl Method {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "approval.decide" => Some(Self::ApprovalDecide),
            "approval.list" => Some(Self::ApprovalList),
            "approval.show" => Some(Self::ApprovalShow),
            "catalog.list" => Some(Self::CatalogList),
            "clarification.resolve" => Some(Self::ClarificationResolve),
            "deferred.complete" => Some(Self::DeferredComplete),
            "deferred.fail" => Some(Self::DeferredFail),
            "deferred.list" => Some(Self::DeferredList),
            "deferred.show" => Some(Self::DeferredShow),
            "diagnostics.get" => Some(Self::DiagnosticsGet),
            "environment.attach" => Some(Self::EnvironmentAttach),
            "environment.detach" => Some(Self::EnvironmentDetach),
            "environment.health" => Some(Self::EnvironmentHealth),
            "environment.list" => Some(Self::EnvironmentList),
            "environment.mount" => Some(Self::EnvironmentMount),
            "environment.mounts.list" => Some(Self::EnvironmentMountsList),
            "environment.unmount" => Some(Self::EnvironmentUnmount),
            "events.replay" => Some(Self::EventsReplay),
            "events.subscribe" => Some(Self::EventsSubscribe),
            "events.unsubscribe" => Some(Self::EventsUnsubscribe),
            "initialize" => Some(Self::Initialize),
            "model.select" => Some(Self::ModelSelect),
            "model.selection.get" => Some(Self::ModelSelectionGet),
            "profile.get" => Some(Self::ProfileGet),
            "run.interrupt" => Some(Self::RunInterrupt),
            "run.resume" => Some(Self::RunResume),
            "run.start" => Some(Self::RunStart),
            "run.status" => Some(Self::RunStatus),
            "run.steer" => Some(Self::RunSteer),
            "session.create" => Some(Self::SessionCreate),
            "session.delete" => Some(Self::SessionDelete),
            "session.fork" => Some(Self::SessionFork),
            "session.get" => Some(Self::SessionGet),
            "session.list" => Some(Self::SessionList),
            "session.search" => Some(Self::SessionSearch),
            "shutdown" => Some(Self::Shutdown),
            _ => None,
        }
    }
    #[must_use]
    pub fn metadata(self) -> &'static MethodMetadata {
        METHODS
            .iter()
            .find(|entry| entry.method == self)
            .expect("generated metadata is exhaustive")
    }
    #[must_use]
    pub fn is_admitted(self, features: &[&str], scopes: &[&str], transport: Transport) -> bool {
        let metadata = self.metadata();
        metadata.transports.contains(&transport)
            && metadata
                .features
                .iter()
                .all(|feature| features.contains(feature))
            && metadata
                .scopes
                .iter()
                .all(|scope| *scope == "public" || scopes.contains(scope))
    }
}
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Notification {
    HostEvent,
    SubscriptionClosed,
}
pub const NOTIFICATIONS: &[NotificationMetadata] = &[
    NotificationMetadata {
        notification: Notification::HostEvent,
        name: "host.event",
        features: &["events.subscribe"],
        transports: &[Transport::Stdio],
        scopes: &["read"],
    },
    NotificationMetadata {
        notification: Notification::SubscriptionClosed,
        name: "subscription.closed",
        features: &["events.subscribe"],
        transports: &[Transport::Stdio],
        scopes: &["read"],
    },
];
impl Notification {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "host.event" => Some(Self::HostEvent),
            "subscription.closed" => Some(Self::SubscriptionClosed),
            _ => None,
        }
    }
    #[must_use]
    pub fn metadata(self) -> &'static NotificationMetadata {
        NOTIFICATIONS
            .iter()
            .find(|entry| entry.notification == self)
            .expect("generated notification metadata is exhaustive")
    }
}
pub const EVENT_PROFILES: &[EventProfileMetadata] = &[
    EventProfileMetadata {
        profile: EventProfile::ConversationV1,
        name: "conversation.v1",
        event_classes: &[
            EventClass::RunChanged,
            EventClass::OutputAvailable,
            EventClass::ApprovalChanged,
            EventClass::DeferredChanged,
            EventClass::ClarificationChanged,
        ],
    },
    EventProfileMetadata {
        profile: EventProfile::DesktopConversationV1,
        name: "desktop.conversation.v1",
        event_classes: &[
            EventClass::RunChanged,
            EventClass::OutputAvailable,
            EventClass::ApprovalChanged,
            EventClass::DeferredChanged,
            EventClass::ClarificationChanged,
        ],
    },
    EventProfileMetadata {
        profile: EventProfile::OperationsV1,
        name: "operations.v1",
        event_classes: &[
            EventClass::SessionChanged,
            EventClass::RunChanged,
            EventClass::EnvironmentChanged,
            EventClass::Diagnostic,
        ],
    },
];
impl EventProfile {
    #[must_use]
    pub fn metadata(self) -> &'static EventProfileMetadata {
        EVENT_PROFILES
            .iter()
            .find(|entry| entry.profile == self)
            .expect("generated event-profile metadata is exhaustive")
    }
    #[must_use]
    pub fn allows_event_class(self, event_class: EventClass) -> bool {
        self.metadata().event_classes.contains(&event_class)
    }
    #[must_use]
    pub fn is_admitted(self, features: &[&str], scopes: &[&str]) -> bool {
        self.metadata()
            .event_classes
            .iter()
            .all(|event_class| event_class.is_admitted(features, scopes))
    }
}
