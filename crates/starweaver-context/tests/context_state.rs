#![allow(missing_docs, clippy::unwrap_used)]

use std::collections::BTreeSet;

use starweaver_context::{
    AgentContext, AgentContextHandle, AgentId, BusMessage, HostCapabilities, MessageBus,
    ModelConfig, PerThousandRatio, ResumableState, TaskManager, TaskStatus, ToolIdWrapper,
    ToolRuntimeSnapshot,
};
use starweaver_core::{Metadata, RunId};
use starweaver_model::{
    ModelMessage, ModelRequest, ModelRequestPart, ModelResponse, ModelResponsePart, ToolArguments,
    ToolCallPart, ToolReturnPart,
};
use starweaver_usage::{PricingEstimate, Usage};

#[test]
fn message_bus_supports_subscribers_targets_idempotency_and_matching() {
    let mut bus = MessageBus::with_maxlen(10);
    assert!(!bus.is_subscribed("main"));
    bus.subscribe("main");
    bus.subscribe("debugger");
    assert!(bus.is_subscribed("main"));
    assert!(bus.is_subscribed("debugger"));

    let broadcast = BusMessage::text("broadcast", "system").with_id("broadcast-1");
    assert_eq!(bus.send(broadcast.clone()).id, "broadcast-1");
    assert_eq!(bus.send(broadcast).id, "broadcast-1");
    bus.send(
        BusMessage::text("debug only", "main")
            .with_id("target-1")
            .with_target("debugger"),
    );
    bus.send(BusMessage::text("steer", "user").with_id("steer-1"));

    let main_steering = bus.consume_matching("main", |message| message.source == "user");
    assert_eq!(main_steering.len(), 1);
    assert_eq!(main_steering[0].id, "steer-1");
    assert!(bus.has_pending("main"));

    let main_rest = bus.consume("main");
    assert_eq!(
        main_rest
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        vec!["broadcast-1"]
    );
    assert!(bus.consume("main").is_empty());

    let debugger_messages = bus.consume("debugger");
    assert_eq!(
        debugger_messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        vec!["broadcast-1", "target-1", "steer-1"]
    );
    bus.unsubscribe("debugger");
    assert!(!bus.is_subscribed("debugger"));
}

#[test]
fn message_bus_mark_consumed_skips_selected_unread_messages() {
    let mut bus = MessageBus::with_maxlen(10);
    bus.subscribe("main");
    bus.subscribe("other");
    bus.send(
        BusMessage::text("skip", "user")
            .with_id("skip-me")
            .with_target("main"),
    );
    bus.send(
        BusMessage::text("keep", "user")
            .with_id("keep-me")
            .with_target("main"),
    );
    bus.send(
        BusMessage::text("other", "user")
            .with_id("other-only")
            .with_target("other"),
    );

    let marked = bus.mark_consumed(
        "main",
        &[
            "skip-me".to_string(),
            "other-only".to_string(),
            "missing".to_string(),
        ]
        .into_iter()
        .collect(),
    );

    assert_eq!(marked, 1);
    assert_eq!(
        bus.consume("main")
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        vec!["keep-me"]
    );
    assert_eq!(
        bus.consume("other")
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        vec!["other-only"]
    );
}

#[test]
fn task_manager_completion_unblocks_dependent_tasks() {
    let mut manager = TaskManager::new();
    let first = manager.create("Prepare", "Prepare work", None, Metadata::default());
    let second = manager.create("Implement", "Implement work", None, Metadata::default());

    manager.update(
        &first.id,
        None,
        None,
        None,
        None,
        None,
        Some(std::slice::from_ref(&second.id)),
        None,
        None,
    );
    assert_eq!(
        manager.get(&second.id).unwrap().blocked_by,
        vec![first.id.clone()]
    );

    manager.update(
        &first.id,
        Some(TaskStatus::Completed),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    assert!(manager.get(&second.id).unwrap().blocked_by.is_empty());
}

#[test]
fn curated_export_keeps_portable_fields_and_omits_runtime_extensions() {
    let mut context = AgentContext::new(AgentId::from_string("main"));
    context.run_id = Some(RunId::from_string("run-1"));
    context.push_message(ModelMessage::Request(ModelRequest::user_text("hello")));
    context.usage = Usage {
        requests: 1,
        input_tokens: 2,
        cache_write_tokens: 0,
        cache_read_tokens: 0,
        output_tokens: 3,
        total_tokens: 5,
        tool_calls: 0,
    };
    context.model_config.context_window = Some(100);
    context.model_config.proactive_context_management_threshold =
        Some(PerThousandRatio::from_per_thousand(500));
    context
        .state
        .set("domain", serde_json::json!({"value": true}));
    context.enqueue_message(BusMessage::text("queued", "system"));
    context.handoff_message = Some("handoff".to_string());
    context.tools.auto_load_files = vec!["src/lib.rs".to_string()];
    context
        .tools
        .shell_environment
        .insert("KEY".to_string(), "VALUE".to_string());
    context.notes.set("language", "Chinese");
    context
        .tools
        .tasks
        .create("Plan", "Plan work", None, Metadata::default());

    let exported = context.export_state();

    assert!(exported.run_id.is_none());
    assert!(exported.conversation_id.is_none());
    assert!(exported.message_history.is_empty());
    assert_eq!(exported.usage, Usage::default());
    assert_eq!(exported.model_config, ModelConfig::default());
    assert!(exported.state.get("domain").is_none());
    assert!(exported.message_bus.is_empty());
    assert_eq!(exported.handoff_message.as_deref(), Some("handoff"));
    assert_eq!(exported.auto_load_files, vec!["src/lib.rs".to_string()]);
    assert!(exported.shell_env.is_empty());
    assert_eq!(exported.notes.get("language"), Some(&"Chinese".to_string()));
    assert_eq!(exported.tasks.len(), 1);
    assert!(exported.agent_registry.contains_key("main"));

    let value = serde_json::to_value(&exported).unwrap();
    assert!(value.get("run_id").is_none());
    assert!(value.get("message_history").is_none());
    assert!(value.get("message_bus").is_none());
    assert!(value.get("shell_env").is_none());
    assert!(value.get("approval_required_tools").is_none());
    assert!(value.get("approval_required_mcp_servers").is_none());
    assert_eq!(value["auto_load_files"], serde_json::json!(["src/lib.rs"]));
    let restored = AgentContext::from_state(serde_json::from_value(value.clone()).unwrap());
    assert_eq!(
        restored.tools.auto_load_files,
        vec!["src/lib.rs".to_string()]
    );
    assert_eq!(value["notes"]["language"], "Chinese");
    assert_eq!(value["tasks"].as_object().unwrap().len(), 1);
    assert!(value["tasks"]["1"].get("subject").is_some());
    assert!(value["tasks"]["1"].get("tasks").is_none());
    assert!(value["notes"].get("notes").is_none());
}

#[test]
fn legacy_host_policy_fields_are_readable_but_never_reexported() {
    let state: ResumableState = serde_json::from_value(serde_json::json!({
        "agent_id": "main",
        "shell_env": {"LEGACY_SECRET": "value"},
        "approval_required_tools": ["shell"],
        "approval_required_mcp_servers": ["legacy-server"]
    }))
    .unwrap();
    assert_eq!(state.shell_env["LEGACY_SECRET"], "value");
    assert_eq!(state.approval_required_tools, vec!["shell"]);
    assert_eq!(state.approval_required_mcp_servers, vec!["legacy-server"]);

    let restored = AgentContext::from_state(state);
    assert_eq!(restored.tools.shell_environment["LEGACY_SECRET"], "value");
    let exported = serde_json::to_value(restored.export_full_state()).unwrap();
    assert!(exported.get("shell_env").is_none());
    assert!(exported.get("approval_required_tools").is_none());
    assert!(exported.get("approval_required_mcp_servers").is_none());
}

#[test]
fn tool_id_wrapper_normalizes_tool_ids_across_history_and_payloads() {
    let mut wrapper = ToolIdWrapper::default();
    let mut messages = vec![
        ModelMessage::Response(ModelResponse {
            parts: vec![
                ModelResponsePart::ToolCall(ToolCallPart {
                    id: "provider-call".to_string(),
                    name: "shell".to_string(),
                    arguments: ToolArguments::Parsed(serde_json::json!({})),
                }),
                ModelResponsePart::NativeToolCall {
                    tool_type: "web_search".to_string(),
                    payload: serde_json::json!({"call_id": "native-call"}),
                },
            ],
            usage: Usage::default(),
            model_name: None,
            provider: None,
            finish_reason: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        }),
        ModelMessage::Request(ModelRequest {
            parts: vec![
                ModelRequestPart::ToolReturn(ToolReturnPart::new(
                    "provider-call",
                    "shell",
                    serde_json::json!("ok"),
                )),
                ModelRequestPart::RetryPrompt {
                    text: "retry".to_string(),
                    tool_call_id: Some("native-call".to_string()),
                    metadata: serde_json::Map::new(),
                },
            ],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        }),
    ];

    wrapper.wrap_messages(&mut messages);

    let ModelMessage::Response(response) = &messages[0] else {
        panic!("response");
    };
    let wrapped_function_id = response.parts[0].tool_call().unwrap().id.clone();
    assert!(wrapped_function_id.starts_with("sw-tool-"));
    let ModelResponsePart::NativeToolCall { payload, .. } = &response.parts[1] else {
        panic!("native");
    };
    let wrapped_native_id = payload["call_id"].as_str().unwrap().to_string();
    assert!(wrapped_native_id.starts_with("sw-tool-"));

    let ModelMessage::Request(request) = &messages[1] else {
        panic!("request");
    };
    assert!(
        matches!(&request.parts[0], ModelRequestPart::ToolReturn(part) if part.tool_call_id == wrapped_function_id)
    );
    assert!(
        matches!(&request.parts[1], ModelRequestPart::RetryPrompt { tool_call_id: Some(id), .. } if id == &wrapped_native_id)
    );
}

#[test]
fn ephemeral_runtime_state_preserves_flat_context_json_and_stays_out_of_resume_state() {
    let fresh = AgentContext::new(AgentId::from_string("fresh"));
    assert_eq!(
        fresh.runtime.injected_context_tags,
        vec!["runtime-context", "environment-context"]
    );

    let mut legacy_value = serde_json::to_value(&fresh).unwrap();
    legacy_value
        .as_object_mut()
        .unwrap()
        .remove("injected_context_tags");
    let legacy_restored: AgentContext = serde_json::from_value(legacy_value).unwrap();
    assert!(legacy_restored.runtime.injected_context_tags.is_empty());

    let mut empty_tags = fresh;
    empty_tags.runtime.injected_context_tags.clear();
    let empty_value = serde_json::to_value(&empty_tags).unwrap();
    assert!(empty_value.get("injected_context_tags").is_none());
    let empty_restored: AgentContext = serde_json::from_value(empty_value).unwrap();
    assert!(empty_restored.runtime.injected_context_tags.is_empty());

    let mut context = AgentContext::new(AgentId::from_string("main"));
    context.runtime.force_inject_context = true;
    context.runtime.current_run_step = 7;
    context.runtime.lifecycle.entered = true;
    context
        .runtime
        .injected_context_tags
        .push("custom-context".to_string());
    context
        .runtime
        .wrapper_metadata
        .insert("wrapper".to_string(), serde_json::json!(true));

    let value = serde_json::to_value(&context).unwrap();
    assert!(value.get("runtime").is_none());
    assert_eq!(value["force_inject_context"], true);
    assert_eq!(value["lifecycle"]["entered"], true);
    assert_eq!(value["wrapper_metadata"]["wrapper"], true);
    assert!(value.get("current_run_step").is_none());

    let restored: AgentContext = serde_json::from_value(value).unwrap();
    assert!(restored.runtime.force_inject_context);
    assert!(restored.runtime.lifecycle.entered);
    assert_eq!(restored.runtime.current_run_step, 0);
    assert!(
        restored
            .runtime
            .injected_context_tags
            .contains(&"custom-context".to_string())
    );

    let resume_value = serde_json::to_value(context.export_full_state()).unwrap();
    assert!(resume_value.get("force_inject_context").is_none());
    assert!(resume_value.get("lifecycle").is_none());
    assert!(resume_value.get("wrapper_metadata").is_none());
}

#[test]
fn agent_tool_state_restores_legacy_flat_wire_shape_and_redacts_secrets() {
    const SECRET: &str = "STARWEAVER_CONTEXT_SECRET_SENTINEL_7f9c";

    let mut legacy = serde_json::to_value(AgentContext::new(AgentId::from_string("main"))).unwrap();
    legacy.as_object_mut().unwrap().extend(
        serde_json::json!({
            "shell_env": {"LEGACY_SECRET": SECRET},
            "deferred_tool_metadata": {"call-1": {"legacy": true}},
            "auto_load_files": ["AGENTS.md"],
            "task_manager": {
                "tasks": {
                    "1": {
                        "id": "1",
                        "subject": "Audit",
                        "description": "Audit boundaries",
                        "status": "pending"
                    }
                }
            },
            "tool_search_loaded_tools": ["search"],
            "tool_search_loaded_namespaces": ["host_io"]
        })
        .as_object()
        .unwrap()
        .clone(),
    );

    let restored: AgentContext = serde_json::from_value(legacy).unwrap();
    assert_eq!(restored.tools.shell_environment["LEGACY_SECRET"], SECRET);
    assert!(restored.tools.deferred_call_metadata.contains_key("call-1"));
    assert_eq!(restored.tools.auto_load_files, vec!["AGENTS.md"]);
    assert_eq!(restored.tools.loaded_tool_names, vec!["search"]);
    assert_eq!(restored.tools.loaded_tool_namespaces, vec!["host_io"]);
    assert_eq!(restored.tools.tasks.list_all().len(), 1);

    let value = serde_json::to_value(&restored).unwrap();
    assert!(value.get("tools").is_none());
    assert!(value.get("shell_env").is_none());
    assert!(value["deferred_tool_metadata"].get("call-1").is_some());
    assert_eq!(value["auto_load_files"][0], "AGENTS.md");
    assert_eq!(value["tool_search_loaded_tools"][0], "search");
    assert_eq!(value["tool_search_loaded_namespaces"][0], "host_io");
    assert_eq!(value["task_manager"]["tasks"].as_object().unwrap().len(), 1);

    let secret_surfaces = [
        serde_json::to_string(&restored).unwrap(),
        serde_json::to_string(&restored.export_state()).unwrap(),
        serde_json::to_string(&restored.export_full_state()).unwrap(),
        format!("{:?}", restored.tools),
        format!("{restored:?}"),
        format!("{:?}", AgentContextHandle::new(restored.clone())),
        format!("{:?}", restored.tool_runtime_snapshot()),
        format!("{:?}", restored.shell_environment_snapshot()),
    ];
    for surface in secret_surfaces {
        assert!(!surface.contains(SECRET), "secret leaked through {surface}");
    }
}

#[test]
fn context_run_helpers_prepare_lifecycle_and_wrapper_metadata() {
    let mut context = AgentContext::new(AgentId::from_string("main"));
    context.parent_run_id = Some(RunId::from_string("parent-run"));
    context
        .runtime
        .wrapper_metadata
        .insert("trace_id".to_string(), serde_json::json!("trace-1"));
    context.prepare_new_run();

    assert!(context.run_id.is_some());
    assert!(context.runtime.lifecycle.entered);
    assert!(context.ended_at.is_none());

    let metadata = context.get_wrapper_metadata();
    assert_eq!(metadata["agent_id"], "main");
    assert_eq!(metadata["parent_run_id"], "parent-run");
    assert_eq!(metadata["trace_id"], "trace-1");

    context.finish_run();
    assert!(!context.runtime.lifecycle.entered);
    assert!(context.ended_at.is_some());
}

#[test]
fn full_export_includes_starweaver_runtime_state() {
    let mut context = AgentContext::new(AgentId::from_string("main"));
    context.run_id = Some(RunId::from_string("run-full-state"));
    context.push_message(ModelMessage::Request(ModelRequest::user_text("hello")));
    context.enqueue_message(BusMessage::text("queued", "system"));

    let exported = context.export_full_state();

    assert_eq!(exported.run_id.as_ref().unwrap().as_str(), "run-full-state");
    assert_eq!(exported.message_history.len(), 1);
    assert_eq!(exported.message_bus.len(), 1);
}

#[test]
fn usage_snapshot_uses_parent_run_id_for_subagent_contexts() {
    let mut context = AgentContext::new(AgentId::from_string("child"));
    context.run_id = Some(RunId::from_string("child-run"));
    context.parent_run_id = Some(RunId::from_string("parent-run"));

    let snapshot = context.update_usage_snapshot_entry(
        "child",
        "debugger",
        "test-model",
        Usage {
            requests: 1,
            input_tokens: 2,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            output_tokens: 3,
            total_tokens: 5,
            tool_calls: 0,
        },
        None,
        None,
        "subagent",
        None,
    );

    assert_eq!(snapshot.run_id, "parent-run");
}

#[test]
fn external_usage_snapshot_entries_are_idempotent_and_aggregated() {
    let mut context = AgentContext::new(AgentId::from_string("main"));
    context.run_id = Some(RunId::from_string("run-extra-usage"));

    let first = context.update_external_usage_snapshot_entry(
        "embedding-cache",
        "Embedding cache",
        "cache-model",
        Usage {
            requests: 1,
            input_tokens: 5,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            output_tokens: 0,
            total_tokens: 5,
            tool_calls: 0,
        },
        Some(PricingEstimate::from_micros_usd(7)),
        Some("usage-cache-1".to_string()),
    );
    assert_eq!(first.entries.len(), 1);
    assert_eq!(first.entries[0].source, "external");
    assert_eq!(
        first.agent_usages["embedding-cache"].agent_name,
        "Embedding cache"
    );
    assert_eq!(first.model_usages["cache-model"].total_tokens, 5);

    let second = context.update_external_usage_snapshot_entry(
        "embedding-cache",
        "Embedding cache",
        "cache-model",
        Usage {
            requests: 2,
            input_tokens: 8,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            output_tokens: 0,
            total_tokens: 8,
            tool_calls: 0,
        },
        Some(PricingEstimate::from_micros_usd(11)),
        Some("usage-cache-1".to_string()),
    );

    assert_eq!(second.run_id, "run-extra-usage");
    assert_eq!(second.entries.len(), 1);
    assert_eq!(second.total_usage.requests, 2);
    assert_eq!(second.total_usage.total_tokens, 8);
    assert_eq!(
        second.estimate_pricing,
        Some(PricingEstimate::from_micros_usd(11))
    );
    assert_eq!(
        second.model_estimate_pricing["cache-model"],
        PricingEstimate::from_micros_usd(11)
    );
}

#[test]
fn tool_dependency_store_exposes_a_narrow_read_only_snapshot() {
    let mut context = AgentContext::default();
    context.model_config.max_image_bytes = 123;
    context.tool_config.fetch_stream_chunk_size = 456;
    context
        .tools
        .shell_environment
        .insert("STARWEAVER_TEST".to_string(), "before".to_string());
    context.insert_dependency(17_u32);

    let dependencies = context.tool_dependency_store();
    assert_eq!(dependencies.get::<u32>().as_deref(), Some(&17));
    assert!(dependencies.get::<AgentContext>().is_none());
    let capabilities = dependencies.get::<HostCapabilities>().unwrap();
    assert_eq!(capabilities.get::<u32>().as_deref(), Some(&17));

    let runtime = dependencies.get::<ToolRuntimeSnapshot>().unwrap();
    assert_eq!(runtime.model_config().max_image_bytes, 123);
    assert_eq!(runtime.tool_config().fetch_stream_chunk_size, 456);
    let runtime_debug = format!("{runtime:?}");
    assert!(runtime_debug.contains("STARWEAVER_TEST"));
    assert!(!runtime_debug.contains("before"));
    assert_eq!(
        runtime
            .shell_environment()
            .get("STARWEAVER_TEST")
            .map(String::as_str),
        Some("before")
    );

    context.model_config.max_image_bytes = 999;
    context
        .tools
        .shell_environment
        .insert("STARWEAVER_TEST".to_string(), "after".to_string());
    assert_eq!(runtime.model_config().max_image_bytes, 123);
    assert_eq!(
        runtime
            .shell_environment()
            .get("STARWEAVER_TEST")
            .map(String::as_str),
        Some("before")
    );
}

#[test]
fn strict_tool_dependencies_expose_only_named_host_grants() {
    let mut context = AgentContext::default();
    context.insert_named_dependency("allowed", 17_u32);
    context.insert_named_dependency("hidden", 23_u64);

    let dependencies =
        context.strict_tool_dependency_store(&BTreeSet::from(["allowed".to_string()]), false);

    assert!(dependencies.get::<u32>().is_none());
    assert!(dependencies.get::<u64>().is_none());
    let capabilities = dependencies.get::<HostCapabilities>().unwrap();
    assert_eq!(capabilities.keys(), vec!["allowed".to_string()]);
    assert_eq!(
        capabilities.get_named::<u32>("allowed").as_deref(),
        Some(&17)
    );
    assert!(capabilities.get_named::<u64>("hidden").is_none());
}

#[test]
fn task_status_rejects_unknown_values() {
    assert_eq!(TaskStatus::parse("pending"), Some(TaskStatus::Pending));
    assert_eq!(
        TaskStatus::parse("in_progress"),
        Some(TaskStatus::InProgress)
    );
    assert_eq!(TaskStatus::parse("completed"), Some(TaskStatus::Completed));
    assert_eq!(TaskStatus::parse("blocked"), None);
    assert!(serde_json::from_value::<TaskStatus>(serde_json::json!("blocked")).is_err());
}
