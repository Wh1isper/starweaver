//! Core abstractions for the Starweaver agent SDK.

use serde_json::{Map, Value};

/// Serializable metadata object shared by Starweaver crates.
pub type Metadata = Map<String, Value>;

mod attachments;
mod cancellation;
mod events;
mod ids;
mod lifecycle;
mod protocol;
mod subagent;
mod trace;
mod xml;

pub use attachments::RunAttachments;
pub use cancellation::CancellationToken;
pub use events::{AgentEvent, DEFERRED_TOOL_REQUESTED_EVENT_KIND, TASK_SNAPSHOT_EVENT_KIND};
pub use ids::{AgentId, CheckpointId, ConversationId, RunId, SessionId, SubagentAttemptId, TaskId};
pub use lifecycle::{AgentExecutionNode, RunLifecycle};
pub use protocol::{
    ProtocolError, ProtocolIdentity, VersionedEnvelope, VersionedRecord, VersionedRecordError,
    from_versioned_json, from_versioned_value, to_versioned_json, to_versioned_value,
};
pub use subagent::{SubagentLifecycleEvent, SubagentLifecycleKind, SubagentSpec};
pub use trace::TraceContext;
pub use xml::{XmlWriter, escape_xml_attribute, escape_xml_text};

/// Workspace-wide SDK identity.
pub const SDK_NAME: &str = "starweaver-agent-sdk";

/// Returns the SDK name used across commands and diagnostics.
#[must_use]
pub const fn sdk_name() -> &'static str {
    SDK_NAME
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use serde_json::Value;

    #[test]
    fn exposes_sdk_name() {
        assert_eq!(sdk_name(), "starweaver-agent-sdk");
    }

    #[test]
    fn exposes_stable_task_snapshot_event_kind() {
        assert_eq!(TASK_SNAPSHOT_EVENT_KIND, "task_snapshot");
    }

    #[test]
    fn validates_protocol_identity() {
        let identity =
            ProtocolIdentity::new("starweaver.test", 1, "2026-07-11").with_features(["one", "two"]);
        assert_eq!(identity.features, ["one", "two"]);
        identity.validate("starweaver.test", 1).unwrap();
        assert!(matches!(
            identity.validate("starweaver.other", 1),
            Err(ProtocolError::UnexpectedProtocol { .. })
        ));
        assert!(matches!(
            identity.validate("starweaver.test", 2),
            Err(ProtocolError::UnsupportedMajor { .. })
        ));
    }

    #[test]
    fn versioned_records_read_legacy_and_reject_unknown_versions() {
        #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
        struct Fixture {
            value: String,
        }

        impl VersionedRecord for Fixture {
            const SCHEMA: &'static str = "starweaver.test.fixture";
            const ALLOW_BARE_V0: bool = true;
        }

        let expected = Fixture {
            value: "hello".to_string(),
        };
        let encoded = to_versioned_json(&expected).unwrap();
        assert_eq!(from_versioned_json::<Fixture>(&encoded).unwrap(), expected);
        assert_eq!(
            from_versioned_json::<Fixture>(r#"{"value":"hello"}"#).unwrap(),
            expected
        );
        let unknown =
            r#"{"schema":"starweaver.test.fixture","version":2,"payload":{"value":"hello"}}"#;
        assert!(matches!(
            from_versioned_json::<Fixture>(unknown),
            Err(VersionedRecordError::UnsupportedVersion { actual: 2, .. })
        ));
    }

    #[test]
    fn creates_prefixed_ids() {
        assert_eq!(AgentId::default().as_str(), "main");
        assert_eq!(AgentId::from_string("agent-1").as_str(), "agent-1");
        assert_eq!(
            SessionId::from_string("session-fixed").as_str(),
            "session-fixed"
        );
        assert!(RunId::new().as_str().starts_with("run_"));
        assert_eq!(RunId::from_string("run-fixed").as_str(), "run-fixed");
        assert!(ConversationId::new().as_str().starts_with("conv_"));
        assert_eq!(
            ConversationId::from_string("conv-fixed").as_str(),
            "conv-fixed"
        );
        assert!(CheckpointId::new().as_str().starts_with("ckpt_"));
        assert_eq!(
            CheckpointId::from_string("ckpt-fixed").as_str(),
            "ckpt-fixed"
        );
        assert!(SubagentAttemptId::new().as_str().starts_with("subattempt_"));
        assert_eq!(
            SubagentAttemptId::from_string("subattempt-fixed").as_str(),
            "subattempt-fixed"
        );
        assert!(TaskId::new().as_str().starts_with("task_"));
        assert_eq!(TaskId::from_string("task-fixed").as_str(), "task-fixed");
    }

    #[test]
    fn run_attachments_wrap_metadata() {
        let mut attachments = RunAttachments::new();
        assert!(attachments.is_empty());
        attachments.insert("tenant", Value::String("alpha".to_string()));
        assert_eq!(
            attachments.get("tenant"),
            Some(&Value::String("alpha".to_string()))
        );
        assert_eq!(attachments.len(), 1);
        let metadata: Metadata = attachments.clone().into();
        assert_eq!(metadata["tenant"], "alpha");
        assert_eq!(RunAttachments::from(metadata).values["tenant"], "alpha");
    }

    #[test]
    fn parses_and_builds_trace_context() {
        let context = TraceContext::from_trace_parent(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )
        .with_span_id("span-1")
        .with_trace_state("vendor=state");

        assert_eq!(
            context.trace_id.as_deref(),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
        assert_eq!(context.parent_span_id.as_deref(), Some("00f067aa0ba902b7"));
        assert_eq!(context.span_id.as_deref(), Some("span-1"));
        assert_eq!(context.trace_state.as_deref(), Some("vendor=state"));
        assert_eq!(context.metadata["trace_flags"], "01");
        assert!(!context.is_empty());

        let mut metadata = Metadata::default();
        metadata.insert("tenant".to_string(), Value::String("acme".to_string()));
        let fallback = TraceContext::from_trace_parent("trace-id")
            .with_parent_span_id("parent")
            .with_metadata(metadata.clone());
        assert_eq!(fallback.trace_id.as_deref(), Some("trace-id"));
        assert_eq!(fallback.parent_span_id.as_deref(), Some("parent"));
        assert_eq!(fallback.metadata, metadata);
        assert!(TraceContext::new().is_empty());
    }

    #[test]
    fn trace_parent_parser_ignores_invalid_w3c_shapes() {
        for invalid in [
            "00-short-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-xyz067aa0ba902b7-01",
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
            "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-zz",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra",
        ] {
            let context = TraceContext::from_trace_parent(invalid);
            assert_eq!(context.trace_id.as_deref(), Some(invalid));
            assert_eq!(context.parent_span_id, None);
            assert!(context.metadata.is_empty());
        }
    }

    #[test]
    fn builds_subagent_specs_lifecycle_events() {
        let spec = SubagentSpec::new("research", "Research helper", "Find facts")
            .with_tools(vec!["search".to_string()])
            .with_optional_tools(vec!["browser".to_string()]);
        assert_eq!(spec.name, "research");
        assert_eq!(spec.description, "Research helper");
        assert_eq!(spec.system_prompt, "Find facts");
        assert_eq!(spec.tools, ["search"]);
        assert_eq!(spec.optional_tools, ["browser"]);

        let event = SubagentLifecycleEvent::new(
            SubagentLifecycleKind::Completed,
            "research",
            TaskId::from_string("task-1"),
        )
        .with_run_id(RunId::from_string("run-1"))
        .with_metadata(serde_json::json!({"ok": true}));
        assert_eq!(event.kind, SubagentLifecycleKind::Completed);
        assert_eq!(event.name, "research");
        assert_eq!(event.task_id.as_str(), "task-1");
        assert_eq!(event.run_id.as_ref().map(RunId::as_str), Some("run-1"));
        assert_eq!(event.metadata["ok"], true);
    }
}
