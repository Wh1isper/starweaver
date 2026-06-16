//! Core abstractions for the Starweaver agent SDK.

use serde_json::{Map, Value};

/// Serializable metadata object shared by Starweaver crates.
pub type Metadata = Map<String, Value>;

mod ids;
mod subagent;
mod trace;
mod xml;

pub use ids::{AgentId, CheckpointId, ConversationId, RunId, SessionId, TaskId};
pub use subagent::{SubagentLifecycleEvent, SubagentLifecycleKind, SubagentSpec};
pub use trace::TraceContext;
pub use xml::{escape_xml_attribute, escape_xml_text, XmlWriter};

/// Workspace-wide SDK identity.
pub const SDK_NAME: &str = "starweaver-agent-sdk";

/// Returns the SDK name used across commands and diagnostics.
#[must_use]
pub const fn sdk_name() -> &'static str {
    SDK_NAME
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn exposes_sdk_name() {
        assert_eq!(sdk_name(), "starweaver-agent-sdk");
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
        assert!(TaskId::new().as_str().starts_with("task_"));
        assert_eq!(TaskId::from_string("task-fixed").as_str(), "task-fixed");
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
