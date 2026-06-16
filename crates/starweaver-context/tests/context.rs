#![allow(missing_docs, clippy::unwrap_used)]

use starweaver_context::{
    AgentContext, AgentEvent, AgentId, BusMessage, ModelConfig, PerThousandRatio, ResumableState,
    TaskStatus,
};
use starweaver_core::TraceContext;
use starweaver_model::{ContentPart, ModelMessage, ModelRequest, ModelResponse};
use starweaver_usage::Usage;

#[test]
fn context_exports_and_restores_state() {
    let mut context = AgentContext::new(AgentId::from_string("main"));
    context.push_message(ModelMessage::Request(ModelRequest::user_text("hello")));
    context
        .state
        .set("notes", serde_json::json!({"answer": 42}));
    context.enqueue_message(BusMessage::new(
        "steering",
        serde_json::json!({"text": "continue"}),
    ));
    context.user_prompts = Some(vec![ContentPart::Text {
        text: "do 1 and 2".to_string(),
    }]);
    context.previous_assistant_response_reference =
        Some("1. Add tests\n2. Update docs".to_string());
    context.steering_messages = vec!["Keep current approach".to_string()];
    context.set_trace_context(
        TraceContext::from_trace_id("trace-main")
            .with_span_id("span-main")
            .with_trace_state("state-main"),
    );

    let exported = context.export_full_state();
    let restored = AgentContext::from_state(exported);

    assert_eq!(restored.agent_id.as_str(), "main");
    assert_eq!(restored.message_history.len(), 1);
    assert_eq!(
        restored.user_prompts.as_deref(),
        Some(
            &[ContentPart::Text {
                text: "do 1 and 2".to_string(),
            }][..]
        ),
    );
    assert_eq!(
        restored.previous_assistant_response_reference.as_deref(),
        Some("1. Add tests\n2. Update docs"),
    );
    assert_eq!(
        restored.steering_messages,
        vec!["Keep current approach".to_string()],
    );
    assert_eq!(
        restored.state.get("notes"),
        Some(&serde_json::json!({"answer": 42}))
    );
    assert_eq!(restored.messages.len(), 1);
    assert_eq!(
        restored.trace_context.trace_id.as_deref(),
        Some("trace-main")
    );
    assert_eq!(restored.trace_context.span_id.as_deref(), Some("span-main"));
    assert_eq!(
        restored.trace_context.trace_state.as_deref(),
        Some("state-main")
    );
}

#[test]
fn event_bus_records_and_drains_events() {
    let mut context = AgentContext::default();
    context.publish_event(AgentEvent::new("run_start", serde_json::json!({"step": 0})));
    context.publish_event(AgentEvent::new(
        "run_complete",
        serde_json::json!({"step": 1}),
    ));

    assert_eq!(context.events.events().len(), 2);
    let drained = context.events.drain();
    assert_eq!(drained.len(), 2);
    assert!(context.events.events().is_empty());
}

#[test]
fn resumable_state_default_restores_fresh_context() {
    let context = AgentContext::from_state(ResumableState::default());

    assert_eq!(context.agent_id.as_str(), "main");
    assert!(context.message_history.is_empty());
    assert!(context.messages.is_empty());
}

#[derive(Debug, Eq, PartialEq)]
struct WeatherService {
    city: String,
}

#[test]
fn context_stores_typed_and_named_dependencies() {
    let mut context = AgentContext::default();
    context.insert_dependency(WeatherService {
        city: "Paris".to_string(),
    });
    context.insert_named_dependency("answer", 42_u32);

    assert_eq!(
        context.dependency::<WeatherService>().unwrap().city,
        "Paris"
    );
    assert_eq!(*context.named_dependency::<u32>("answer").unwrap(), 42);
    assert!(context.named_dependency::<String>("answer").is_none());
    assert_eq!(context.dependencies.keys().len(), 2);
}

#[test]
fn dependencies_are_not_serialized_in_resumable_state() {
    let mut context = AgentContext::default();
    context.insert_dependency(WeatherService {
        city: "Paris".to_string(),
    });

    let restored = AgentContext::from_state(context.export_full_state());

    assert!(restored.dependency::<WeatherService>().is_none());
}

#[test]
fn subagent_context_inherits_long_lived_state_and_resets_run_queues() {
    let mut context = AgentContext::new(AgentId::from_string("parent"));
    context.run_id = Some(starweaver_core::RunId::from_string("run-parent"));
    context.push_message(ModelMessage::Request(ModelRequest::user_text(
        "parent history",
    )));
    context.enqueue_message(BusMessage::new(
        "steering",
        serde_json::json!({"text": "parent only"}),
    ));
    context.user_prompts = Some(vec![ContentPart::Text {
        text: "parent prompt".to_string(),
    }]);
    context.previous_assistant_response_reference = Some("parent response".to_string());
    context.steering_messages = vec!["parent steering".to_string()];
    context
        .state
        .set("domain", serde_json::json!({"value": 42}));
    context.notes.set("lang", "Chinese");
    context.usage = Usage {
        requests: 2,
        input_tokens: 10,
        cache_write_tokens: 0,
        cache_read_tokens: 0,
        output_tokens: 4,
        total_tokens: 14,
        tool_calls: 1,
    };
    context.insert_dependency(WeatherService {
        city: "Paris".to_string(),
    });
    context.insert_named_dependency("answer", 42_u32);
    context.set_trace_context(
        TraceContext::from_trace_id("trace-parent")
            .with_span_id("span-parent")
            .with_parent_span_id("root-span"),
    );

    let child = context.subagent_context("researcher");

    assert_eq!(child.agent_id.as_str(), "researcher");
    assert_eq!(child.metadata["parent_agent_id"], "parent");
    assert_eq!(child.metadata["parent_run_id"], "run-parent");
    assert_eq!(child.conversation_id, context.conversation_id);
    assert_eq!(child.usage, context.usage);
    assert_eq!(child.state.get("domain"), context.state.get("domain"));
    assert_eq!(child.notes.get("lang"), Some("Chinese"));
    assert_eq!(child.dependency::<WeatherService>().unwrap().city, "Paris");
    assert_eq!(*child.named_dependency::<u32>("answer").unwrap(), 42);
    assert_eq!(child.trace_context, context.trace_context);
    assert!(child.message_history.is_empty());
    assert!(child.user_prompts.is_none());
    assert!(child.previous_assistant_response_reference.is_none());
    assert!(child.steering_messages.is_empty());
    assert_eq!(child.messages.len(), context.messages.len());
    assert!(child.events.events().is_empty());
}

#[test]
fn runtime_context_reports_latest_request_tokens_not_accumulated_usage() {
    let mut context = AgentContext {
        usage: Usage {
            requests: 2,
            input_tokens: 120,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            output_tokens: 80,
            total_tokens: 200,
            tool_calls: 0,
        },
        ..AgentContext::default()
    };
    let mut first = ModelResponse::text("first");
    first.usage = Usage {
        requests: 1,
        input_tokens: 20,
        cache_write_tokens: 0,
        cache_read_tokens: 0,
        output_tokens: 5,
        total_tokens: 25,
        tool_calls: 0,
    };
    let mut second = ModelResponse::text("second");
    second.usage = Usage {
        requests: 1,
        input_tokens: 40,
        cache_write_tokens: 0,
        cache_read_tokens: 0,
        output_tokens: 10,
        total_tokens: 50,
        tool_calls: 0,
    };
    context.push_message(ModelMessage::Response(first));
    context.push_message(ModelMessage::Response(second));

    let injected = context.inject_runtime_context(true).unwrap();

    assert_eq!(context.latest_request_total_tokens(), Some(50));
    assert!(injected.contains("<total-tokens>50</total-tokens>"));
    assert!(!injected.contains("<total-tokens>200</total-tokens>"));
}

#[test]
fn runtime_context_adds_proactive_context_pressure_reminder_from_latest_request_tokens() {
    let mut context = AgentContext {
        model_config: ModelConfig {
            context_window: Some(100),
            proactive_context_management_threshold: Some(PerThousandRatio::from_per_thousand(500)),
            compact_threshold: PerThousandRatio::from_per_thousand(900),
            ..ModelConfig::default()
        },
        usage: Usage {
            requests: 2,
            input_tokens: 90,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            output_tokens: 20,
            total_tokens: 110,
            tool_calls: 0,
        },
        ..AgentContext::default()
    };
    let mut first = ModelResponse::text("first");
    first.usage = Usage {
        requests: 1,
        input_tokens: 40,
        cache_write_tokens: 0,
        cache_read_tokens: 0,
        output_tokens: 10,
        total_tokens: 50,
        tool_calls: 0,
    };
    let mut second = ModelResponse::text("second");
    second.usage = Usage {
        requests: 1,
        input_tokens: 50,
        cache_write_tokens: 0,
        cache_read_tokens: 0,
        output_tokens: 10,
        total_tokens: 60,
        tool_calls: 0,
    };
    context.push_message(ModelMessage::Response(first));
    context.push_message(ModelMessage::Response(second));

    let injected = context.inject_runtime_context(true).unwrap();

    assert!(injected.contains("<system-reminder>"));
    assert!(injected.contains("Context usage is at 60% (60 / 100 tokens)"));
    assert!(injected.contains("Configured compact threshold is 90%"));
    assert!(!injected.contains("110 / 100 tokens"));
}

#[test]
fn runtime_context_includes_active_tasks_with_user_prompt_details() {
    let mut context = AgentContext::default();
    let mut pending = starweaver_context::Task::new("1", "Plan work", "Plan the implementation");
    pending.blocked_by.push("2".to_string());
    let mut in_progress = starweaver_context::Task::new("2", "Implement changes", "Edit code");
    in_progress.status = TaskStatus::InProgress;
    in_progress.active_form = Some("Implementing changes".to_string());
    let mut completed = starweaver_context::Task::new("3", "Done", "Completed task");
    completed.status = TaskStatus::Completed;
    context.set_tasks(vec![pending, in_progress, completed]);

    let injected = context.inject_runtime_context(true).unwrap();

    assert!(injected.contains("<active-tasks hint=\"Update status with task_update tool\">"));
    assert!(injected.contains("<task id=\"1\" status=\"pending\" blocked-by=\"2\">"));
    assert!(injected.contains("<subject>Plan work</subject>"));
    assert!(injected.contains("<active-form>Implementing changes</active-form>"));
    assert!(!injected.contains("Done"));
}

#[test]
fn runtime_context_includes_compact_active_tasks_for_tool_turns() {
    let mut context = AgentContext::default();
    context.set_tasks(vec![starweaver_context::Task::new(
        "1",
        "Plan work",
        "Plan the implementation",
    )]);

    let injected = context.inject_runtime_context(false).unwrap();

    assert!(injected.contains("<active-tasks>"));
    assert!(injected.contains("<task id=\"1\" status=\"pending\">Plan work</task>"));
    assert!(!injected.contains("hint="));
    assert!(!injected.contains("<subject>"));
}

#[test]
fn parent_absorbs_subagent_usage_and_notes_after_success() {
    let mut parent = AgentContext::default();
    parent.notes.set("lang", "Chinese");
    parent.usage = Usage {
        requests: 1,
        input_tokens: 3,
        cache_write_tokens: 0,
        cache_read_tokens: 0,
        output_tokens: 2,
        total_tokens: 5,
        tool_calls: 0,
    };
    let mut child = parent.subagent_context("debugger");
    child.notes.set("lang", "English");
    child.notes.set("debug", "enabled");
    child.usage.add_assign(&Usage {
        requests: 2,
        input_tokens: 4,
        cache_write_tokens: 0,
        cache_read_tokens: 0,
        output_tokens: 6,
        total_tokens: 10,
        tool_calls: 1,
    });
    child.push_message(ModelMessage::Request(ModelRequest::user_text(
        "child history",
    )));
    child.enqueue_message(BusMessage::new(
        "child",
        serde_json::json!({"text": "local"}),
    ));

    parent.absorb_subagent_context(&child);

    assert_eq!(parent.usage.requests, 3);
    assert_eq!(parent.usage.total_tokens, 15);
    assert_eq!(parent.usage.tool_calls, 1);
    assert_eq!(parent.notes.get("lang"), Some("English"));
    assert_eq!(parent.notes.get("debug"), Some("enabled"));
    assert!(parent.message_history.is_empty());
    assert_eq!(parent.messages.len(), 1);
    assert_eq!(parent.subagent_history["debugger"].len(), 1);
}
