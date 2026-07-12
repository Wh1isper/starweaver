#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use async_trait::async_trait;
use starweaver_agent::{
    AgentBuilder, AgentCapability, AgentContext, AgentContextHandle, AgentRunState,
    AgentRuntimePolicy, AgentStreamEvent, AgentStreamSourceKind, BackgroundSubagentCapability,
    BackgroundSubagentMonitor, DELEGATE_BACKEND_TOOL_NAME, FunctionTool, SPAWN_DELEGATE_TOOL_NAME,
    SubagentConfig, SubagentDelegationMode, SubagentExecutionHook, SubagentExecutionMetadata,
    SubagentExecutionOutcome, SubagentParentTools, SubagentRegistry, SubagentToolInheritancePolicy,
    TestModel, ToolContext, ToolError, ToolRegistry, ToolResult, WAIT_SUBAGENT_TOOL_NAME,
};
use starweaver_core::{ConversationId, RunId};
use starweaver_model::{
    ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart, ToolCallPart,
};
use starweaver_tools::{
    TOOL_METADATA_DEPENDENCIES_KEY, ToolDependencyProfile, tool_dependency_requirements,
};
use starweaver_usage::Usage;

fn delegation_tool_context(parent: &AgentContext, handle: AgentContextHandle) -> ToolContext {
    let mut dependencies = parent.dependencies.clone();
    dependencies.insert(parent.clone());
    dependencies.insert(handle);
    ToolContext::new(RunId::default(), ConversationId::default(), 0).with_dependencies(dependencies)
}

fn visible_tool_names(tools: &ToolRegistry) -> Vec<String> {
    tools
        .definitions_for_context(&AgentContext::default())
        .into_iter()
        .map(|definition| definition.name)
        .collect()
}

#[test]
fn delegation_tools_declare_dependency_profiles_explicitly() {
    let registry = Arc::new(SubagentRegistry::new());
    let monitor = Arc::new(BackgroundSubagentMonitor::new());
    let legacy_tools = [
        registry.delegate_tool(),
        registry.delegate_tool_named("custom_delegate"),
        registry.hidden_delegate_backend_tool(),
        registry.async_delegate_tool(Arc::clone(&monitor)),
        registry.spawn_delegate_tool(Arc::clone(&monitor)),
        registry.wait_subagent_tool(monitor),
    ];
    for tool in legacy_tools {
        let metadata = tool.metadata();
        assert!(
            metadata.contains_key(TOOL_METADATA_DEPENDENCIES_KEY),
            "{} must declare dependency metadata",
            tool.name()
        );
        assert_eq!(
            tool_dependency_requirements(&metadata).profile,
            ToolDependencyProfile::Legacy,
            "{} must explicitly retain Legacy compatibility",
            tool.name()
        );
    }

    let info = registry.subagent_info_tool();
    let metadata = info.metadata();
    assert!(metadata.contains_key(TOOL_METADATA_DEPENDENCIES_KEY));
    assert_eq!(
        tool_dependency_requirements(&metadata).profile,
        ToolDependencyProfile::Filtered
    );
}

#[tokio::test]
async fn subagent_registry_exports_typed_delegate_tool() {
    let child = Arc::new(
        AgentBuilder::new(Arc::new(TestModel::with_responses(vec![ModelResponse {
            usage: Usage {
                requests: 1,
                ..Usage::default()
            },
            ..ModelResponse::text("child output")
        }])))
        .build(),
    );
    let registry =
        Arc::new(SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child)));
    let delegate = registry.delegate_tool();
    let parent = AgentContext::default();
    let context_handle = AgentContextHandle::new(parent.clone());
    let mut dependencies = parent.dependencies.clone();
    dependencies.insert(parent.clone());
    dependencies.insert(context_handle.clone());
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);

    let result = delegate
        .call(
            context,
            serde_json::json!({
                "name": "child",
                "prompt": "help",
                "metadata": {"source": "tool-test"}
            }),
        )
        .await
        .unwrap();

    assert_eq!(delegate.name(), "delegate");
    let schema = delegate.parameters_schema();
    assert!(schema["properties"].get("subagent_name").is_some());
    assert!(schema["properties"].get("prompt").is_some());
    assert!(schema["properties"].get("agent_id").is_some());
    assert!(schema["properties"].get("metadata").is_none());
    assert_eq!(result.content["name"], "child");
    assert_eq!(result.content["output"], "child output");
    assert!(result.content["usage"]["requests"].as_u64().unwrap() >= 1);
    assert_eq!(result.metadata["context_mutated"], true);
    let snapshot = context_handle.snapshot();
    let event_kinds = snapshot
        .events
        .events()
        .iter()
        .map(|event| event.kind.as_str())
        .collect::<Vec<_>>();
    assert!(event_kinds.contains(&"subagent_started"));
    assert!(event_kinds.contains(&"subagent_completed"));
    assert!(event_kinds.contains(&"usage_snapshot"));
}

#[tokio::test]
async fn subagent_info_tool_lists_known_subagents_with_empty_args_schema() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child"))).build());
    let registry = Arc::new(
        SubagentRegistry::new()
            .with_subagent(
                SubagentConfig::new("child", child.clone())
                    .with_description("Answers child tasks")
                    .with_tool_inheritance(SubagentToolInheritancePolicy::new(
                        vec!["view".to_string()],
                        vec![],
                    )),
            )
            .with_subagent(SubagentConfig::new("blocked", child).with_tool_inheritance(
                SubagentToolInheritancePolicy::new(vec!["missing".to_string()], vec![]),
            )),
    );
    let subagent_info = registry.subagent_info_tool();
    let parent_tools =
        SubagentParentTools(ToolRegistry::new().with_tool(Arc::new(FunctionTool::new(
            "view",
            Some("View file".to_string()),
            serde_json::json!({"type": "object"}),
            |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
        ))));
    let mut context = ToolContext::new(RunId::default(), ConversationId::default(), 0);
    context.dependencies.insert(parent_tools);

    assert_eq!(subagent_info.name(), "subagent_info");
    let schema = subagent_info.parameters_schema();
    assert_eq!(schema["type"], "object");
    assert!(schema["properties"].as_object().unwrap().is_empty());

    let result = subagent_info
        .call(context, serde_json::json!({}))
        .await
        .unwrap();

    assert_eq!(result.content["subagents"][0]["name"], "child");
    assert_eq!(
        result.content["subagents"][0]["description"],
        "Answers child tasks"
    );
    assert_eq!(result.content["subagents"][0]["available"], true);
    assert_eq!(
        result.content["subagents"][0]["inherited_tools"],
        serde_json::json!(["view"])
    );
    assert_eq!(result.content["subagents"][1]["name"], "blocked");
    assert_eq!(result.content["subagents"][1]["available"], false);
    assert_eq!(
        result.content["subagents"][1]["diagnostics"][0]["error_kind"],
        "missing_required_tool"
    );
    assert_eq!(
        result.content["subagents"][1]["diagnostics"][0]["tool_name"],
        "missing"
    );
}

#[test]
fn subagent_tool_inheritance_never_exposes_delegation_tools_to_children() {
    let ordinary = Arc::new(FunctionTool::new(
        "ordinary",
        Some("Ordinary inherited tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    ));
    let delegate = Arc::new(FunctionTool::new(
        "delegate",
        Some("Delegation tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    ));
    let wait_subagent = Arc::new(FunctionTool::new(
        WAIT_SUBAGENT_TOOL_NAME,
        Some("Wait for subagent".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    ));
    let parent = ToolRegistry::new()
        .with_tool(ordinary)
        .with_tool(delegate)
        .with_tool(wait_subagent);

    let inherited = SubagentToolInheritancePolicy::default()
        .with_inherit_all_when_empty(true)
        .with_nested_delegation(true)
        .resolve(&parent)
        .unwrap();

    assert_eq!(inherited.names(), vec!["ordinary".to_string()]);
}

#[tokio::test]
async fn subagent_delegate_tool_reports_missing_agent_context() {
    let registry = Arc::new(SubagentRegistry::new());
    let delegate = registry.delegate_tool_named("ask_subagent");

    let error = delegate
        .call(
            ToolContext::new(RunId::default(), ConversationId::default(), 0),
            serde_json::json!({"name": "missing", "prompt": "hello"}),
        )
        .await
        .unwrap_err();

    assert_eq!(delegate.name(), "ask_subagent");
    assert!(matches!(
        error,
        starweaver_agent::ToolError::UserError { .. }
    ));
    assert!(
        error
            .to_string()
            .contains("missing AgentContextHandle dependency")
    );
}

#[test]
fn subagent_registry_reports_names_and_availability() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child"))).build());
    let registry = SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child));

    assert_eq!(registry.names(), vec!["child"]);
    assert!(registry.is_available("child"));
    assert!(!registry.is_available("missing"));
    assert!(!registry.is_empty());
}

#[test]
fn async_delegation_mode_makes_delegate_async_and_hides_backend() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child"))).build());
    let agent = AgentBuilder::new(Arc::new(TestModel::with_text("parent")))
        .subagent(SubagentConfig::new("child", child).with_description("Child helper"))
        .subagent_delegation_mode(SubagentDelegationMode::Async)
        .build();
    let tools = agent.tools();

    assert_eq!(
        tools.names(),
        vec![
            DELEGATE_BACKEND_TOOL_NAME.to_string(),
            "delegate".to_string(),
            "subagent_info".to_string(),
            WAIT_SUBAGENT_TOOL_NAME.to_string()
        ]
    );
    assert_eq!(
        visible_tool_names(&tools),
        vec!["delegate".to_string(), "subagent_info".to_string()]
    );
    let instructions = tools.get_instructions().join("\n");
    assert!(instructions.contains("delegate is asynchronous"));
    assert!(instructions.contains("do not manually poll or loop"));
    assert!(instructions.contains("wait_subagent once with a bounded timeout"));
    assert!(instructions.contains("let the Starweaver host notify you"));
    assert!(instructions.contains("Child helper"));
    assert!(!instructions.contains("Delegate calls are blocking"));
}

#[test]
fn dual_delegation_mode_keeps_blocking_delegate_and_adds_spawn_delegate() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child"))).build());
    let agent = AgentBuilder::new(Arc::new(TestModel::with_text("parent")))
        .subagent(SubagentConfig::new("child", child))
        .subagent_delegation_mode(SubagentDelegationMode::BlockingAndAsync)
        .build();
    let tools = agent.tools();

    assert_eq!(
        tools.names(),
        vec![
            "delegate".to_string(),
            SPAWN_DELEGATE_TOOL_NAME.to_string(),
            "subagent_info".to_string(),
            WAIT_SUBAGENT_TOOL_NAME.to_string()
        ]
    );
    assert_eq!(
        visible_tool_names(&tools),
        vec![
            "delegate".to_string(),
            SPAWN_DELEGATE_TOOL_NAME.to_string(),
            "subagent_info".to_string()
        ]
    );
    let instructions = tools.get_instructions().join("\n");
    assert!(instructions.contains("Delegate calls are blocking"));
    assert!(instructions.contains("Use this to run a subagent asynchronously"));
    assert!(instructions.contains("do not manually poll or loop"));
    assert!(instructions.contains("automatically notify you"));
}

#[tokio::test]
async fn hidden_delegate_backend_is_executable_but_not_model_visible() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child output"))).build());
    let registry =
        Arc::new(SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child)));
    let monitor = Arc::new(BackgroundSubagentMonitor::new());
    let backend = registry.hidden_delegate_backend_tool();
    let tools = ToolRegistry::new()
        .with_tool(backend.clone())
        .with_tool(registry.async_delegate_tool(monitor));

    assert_eq!(visible_tool_names(&tools), vec!["delegate".to_string()]);

    let parent = AgentContext::default();
    let context_handle = AgentContextHandle::new(parent.clone());
    let result = backend
        .call(
            delegation_tool_context(&parent, context_handle.clone()),
            serde_json::json!({
                "name": "child",
                "prompt": "help"
            }),
        )
        .await
        .unwrap();

    assert_eq!(backend.name(), DELEGATE_BACKEND_TOOL_NAME);
    assert_eq!(result.content["name"], "child");
    assert_eq!(result.content["output"], "child output");
    assert_eq!(result.metadata["context_mutated"], true);
    assert!(
        context_handle
            .snapshot()
            .events
            .events()
            .iter()
            .any(|event| event.kind == "subagent_completed")
    );
}

#[tokio::test]
async fn async_delegate_delivers_result_to_subscribed_parent_bus() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child output"))).build());
    let registry =
        Arc::new(SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child)));
    let monitor = Arc::new(BackgroundSubagentMonitor::new());
    let delegate = registry.async_delegate_tool(monitor.clone());
    let parent = AgentContext::default();
    let context_handle = AgentContextHandle::new(parent.clone());

    let result = delegate
        .call(
            delegation_tool_context(&parent, context_handle.clone()),
            serde_json::json!({
                "name": "child",
                "prompt": "help",
                "agent_id": "child-bg-test"
            }),
        )
        .await
        .unwrap();

    assert_eq!(result.content["status"], "spawned");
    assert_eq!(result.content["agent_id"], "child-bg-test");
    let message = result.content["message"].as_str().unwrap();
    assert!(message.contains("Do not manually poll or loop"));
    assert!(message.contains("finish your current response now"));
    assert!(message.contains("automatically notify you"));

    let mut delivered = None;
    for _ in 0..50 {
        let snapshot = context_handle.snapshot();
        let pending = snapshot.messages.peek(snapshot.agent_id.as_str());
        if !monitor.has_active_tasks() && !pending.is_empty() {
            delivered = Some(pending);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let Some(delivered) = delivered else {
        panic!("background delegate message");
    };
    assert!(!monitor.has_pending_messages());
    assert_eq!(delivered[0].source, "child-bg-test");
    assert_eq!(delivered[0].target.as_deref(), Some("main"));
    assert_eq!(delivered[0].content_text(), "child output");
}

#[tokio::test]
async fn wait_subagent_returns_cached_result_and_marks_bus_message_consumed() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child output"))).build());
    let registry =
        Arc::new(SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child)));
    let monitor = Arc::new(BackgroundSubagentMonitor::new());
    let delegate = registry.async_delegate_tool(monitor.clone());
    let wait = registry.wait_subagent_tool(monitor.clone());
    let parent = AgentContext::default();
    let context_handle = AgentContextHandle::new(parent.clone());

    delegate
        .call(
            delegation_tool_context(&parent, context_handle.clone()),
            serde_json::json!({
                "name": "child",
                "prompt": "help",
                "agent_id": "child-bg-wait"
            }),
        )
        .await
        .unwrap();

    for _ in 0..50 {
        if !monitor.has_active_tasks() && !monitor.task_results().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let result = wait
        .call(
            delegation_tool_context(&context_handle.snapshot(), context_handle.clone()),
            serde_json::json!({"agent_id": "child-bg-wait", "timeout_seconds": 0}),
        )
        .await
        .unwrap();

    assert_eq!(result.content["status"], "completed");
    assert_eq!(result.content["agent_id"], "child-bg-wait");
    assert_eq!(result.content["subagent_name"], "child");
    assert_eq!(result.content["result"], "child output");
    assert_eq!(result.content["timed_out"], false);
    assert!(context_handle.snapshot().messages.peek("main").is_empty());
}

#[tokio::test]
async fn wait_subagent_unknown_agent_reports_known_ids() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child output"))).build());
    let registry =
        Arc::new(SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child)));
    let monitor = Arc::new(BackgroundSubagentMonitor::new());
    let delegate = registry.async_delegate_tool(monitor.clone());
    let wait = registry.wait_subagent_tool(monitor.clone());
    let parent = AgentContext::default();
    let context_handle = AgentContextHandle::new(parent.clone());

    delegate
        .call(
            delegation_tool_context(&parent, context_handle.clone()),
            serde_json::json!({
                "name": "child",
                "prompt": "help",
                "agent_id": "child-bg-known"
            }),
        )
        .await
        .unwrap();

    for _ in 0..50 {
        if !monitor.has_active_tasks() && !monitor.task_results().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let result = wait
        .call(
            delegation_tool_context(&context_handle.snapshot(), context_handle),
            serde_json::json!({"agent_id": "missing-bg-id", "timeout_seconds": 0}),
        )
        .await
        .unwrap();

    assert_eq!(result.content["status"], "not_found");
    assert_eq!(result.content["agent_id"], "missing-bg-id");
    assert_eq!(
        result.content["known_agent_ids"],
        serde_json::json!(["child-bg-known"])
    );
}

#[tokio::test]
async fn async_delegate_and_wait_guard_errors_are_user_facing() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child output"))).build());
    let registry =
        Arc::new(SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child)));
    let monitor = Arc::new(BackgroundSubagentMonitor::new());
    let delegate = registry.async_delegate_tool(monitor.clone());
    let wait = registry.wait_subagent_tool(monitor);
    let empty_context = ToolContext::new(RunId::default(), ConversationId::default(), 0);

    let missing_delegate_context = delegate
        .call(
            empty_context.clone(),
            serde_json::json!({"name": "child", "prompt": "help"}),
        )
        .await
        .unwrap_err();
    match missing_delegate_context {
        ToolError::UserError { tool, message } => {
            assert_eq!(tool, "delegate");
            assert_eq!(message, "missing AgentContextHandle dependency");
        }
        other => panic!("expected user error, got {other:?}"),
    }

    let missing_wait_context = wait
        .call(empty_context, serde_json::json!({"timeout_seconds": 0}))
        .await
        .unwrap_err();
    match missing_wait_context {
        ToolError::UserError { tool, message } => {
            assert_eq!(tool, WAIT_SUBAGENT_TOOL_NAME);
            assert_eq!(message, "missing AgentContextHandle dependency");
        }
        other => panic!("expected user error, got {other:?}"),
    }

    let nested_parent = AgentContext {
        parent_run_id: Some(RunId::from_string("parent-run")),
        ..AgentContext::default()
    };
    let nested_handle = AgentContextHandle::new(nested_parent.clone());
    let nested_context = delegation_tool_context(&nested_parent, nested_handle.clone());

    let nested_delegate = delegate
        .call(
            nested_context.clone(),
            serde_json::json!({"name": "child", "prompt": "help"}),
        )
        .await
        .unwrap_err();
    match nested_delegate {
        ToolError::UserError { tool, message } => {
            assert_eq!(tool, "delegate");
            assert_eq!(
                message,
                "background subagent delegation is only available to the main agent"
            );
        }
        other => panic!("expected user error, got {other:?}"),
    }

    let nested_wait = wait
        .call(nested_context, serde_json::json!({"timeout_seconds": 0}))
        .await
        .unwrap_err();
    match nested_wait {
        ToolError::UserError { tool, message } => {
            assert_eq!(tool, WAIT_SUBAGENT_TOOL_NAME);
            assert_eq!(message, "wait_subagent is only available to the main agent");
        }
        other => panic!("expected user error, got {other:?}"),
    }
}

#[tokio::test]
async fn wait_subagent_without_agent_id_reports_empty_and_completed() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child output"))).build());
    let registry =
        Arc::new(SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child)));
    let monitor = Arc::new(BackgroundSubagentMonitor::new());
    let delegate = registry.async_delegate_tool(monitor.clone());
    let wait = registry.wait_subagent_tool(monitor.clone());
    let parent = AgentContext::default();
    let context_handle = AgentContextHandle::new(parent.clone());

    let empty = wait
        .call(
            delegation_tool_context(&parent, context_handle.clone()),
            serde_json::json!({"timeout_seconds": 0}),
        )
        .await
        .unwrap();
    assert_eq!(empty.content["status"], "empty");
    assert_eq!(empty.content["timed_out"], false);
    assert_eq!(empty.content["results"], serde_json::json!([]));

    delegate
        .call(
            delegation_tool_context(&parent, context_handle.clone()),
            serde_json::json!({
                "name": "child",
                "prompt": "help",
                "agent_id": "child-bg-all"
            }),
        )
        .await
        .unwrap();

    for _ in 0..50 {
        if !monitor.has_active_tasks() && !monitor.task_results().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let completed = wait
        .call(
            delegation_tool_context(&context_handle.snapshot(), context_handle.clone()),
            serde_json::json!({"timeout_seconds": 0}),
        )
        .await
        .unwrap();
    assert_eq!(completed.content["status"], "completed");
    assert_eq!(completed.content["timed_out"], false);
    assert_eq!(completed.content["results"][0]["status"], "completed");
    assert_eq!(completed.content["results"][0]["agent_id"], "child-bg-all");
    assert_eq!(completed.content["results"][0]["result"], "child output");
}

#[tokio::test]
async fn wait_subagent_without_agent_id_reports_running_timeout() {
    let slow_child_model = TestModel::with_responses(vec![
        starweaver_model::tool_call_response("call_slow", "slow", serde_json::json!({})),
        ModelResponse::text("slow child output"),
    ]);
    let slow_tool = Arc::new(FunctionTool::new(
        "slow",
        Some("Slow background child tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            Ok(ToolResult::new(serde_json::json!({"ok": true})))
        },
    ));
    let slow_child = Arc::new(
        AgentBuilder::new(Arc::new(slow_child_model))
            .tool(slow_tool)
            .policy(AgentRuntimePolicy {
                max_steps: 4,
                ..AgentRuntimePolicy::default()
            })
            .build(),
    );
    let slow_registry = Arc::new(
        SubagentRegistry::new().with_subagent(SubagentConfig::new("slow_child", slow_child)),
    );
    let slow_monitor = Arc::new(BackgroundSubagentMonitor::new());
    let slow_delegate = slow_registry.async_delegate_tool(slow_monitor.clone());
    let slow_wait = slow_registry.wait_subagent_tool(slow_monitor.clone());
    let slow_parent = AgentContext::default();
    let slow_handle = AgentContextHandle::new(slow_parent.clone());

    slow_delegate
        .call(
            delegation_tool_context(&slow_parent, slow_handle.clone()),
            serde_json::json!({
                "name": "slow_child",
                "prompt": "help",
                "agent_id": "child-bg-running"
            }),
        )
        .await
        .unwrap();

    let running = slow_wait
        .call(
            delegation_tool_context(&slow_parent, slow_handle),
            serde_json::json!({"timeout_seconds": 0}),
        )
        .await
        .unwrap();
    assert_eq!(running.content["status"], "running");
    assert_eq!(running.content["timed_out"], true);
    assert_eq!(running.content["results"][0]["status"], "running");
    assert_eq!(
        running.content["results"][0]["agent_id"],
        "child-bg-running"
    );

    for _ in 0..50 {
        if !slow_monitor.has_active_tasks() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(!slow_monitor.has_active_tasks());
}

#[tokio::test]
async fn async_delegate_reports_generated_agent_id_and_resume_status() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child output"))).build());
    let registry =
        Arc::new(SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child)));
    let monitor = Arc::new(BackgroundSubagentMonitor::new());
    let delegate = registry.async_delegate_tool(monitor.clone());
    let mut parent = AgentContext::default();
    parent
        .subagent_history
        .insert("child-bg-resume".to_string(), Vec::new());
    let context_handle = AgentContextHandle::new(parent.clone());

    let generated = delegate
        .call(
            delegation_tool_context(&parent, context_handle.clone()),
            serde_json::json!({"name": "child", "prompt": "help"}),
        )
        .await
        .unwrap();
    assert_eq!(generated.content["status"], "spawned");
    assert!(
        generated.content["agent_id"]
            .as_str()
            .unwrap()
            .starts_with("child-bg-")
    );

    let resumed = delegate
        .call(
            delegation_tool_context(&parent, context_handle),
            serde_json::json!({
                "name": "child",
                "prompt": "help again",
                "agent_id": "child-bg-resume"
            }),
        )
        .await
        .unwrap();
    assert_eq!(resumed.content["status"], "resumed");
    assert_eq!(resumed.content["agent_id"], "child-bg-resume");

    for _ in 0..50 {
        if !monitor.has_active_tasks() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(!monitor.has_active_tasks());
}

#[tokio::test]
async fn async_delegate_redelivers_pending_result_when_parent_is_not_subscribed() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child output"))).build());
    let registry =
        Arc::new(SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child)));
    let monitor = Arc::new(BackgroundSubagentMonitor::new());
    let delegate = registry.async_delegate_tool(monitor.clone());
    let mut parent = AgentContext::default();
    parent.messages.unsubscribe(parent.agent_id.as_str());
    let context_handle = AgentContextHandle::new(parent.clone());

    delegate
        .call(
            delegation_tool_context(&parent, context_handle),
            serde_json::json!({
                "name": "child",
                "prompt": "help",
                "agent_id": "child-bg-pending"
            }),
        )
        .await
        .unwrap();

    for _ in 0..50 {
        if !monitor.has_active_tasks() && monitor.has_pending_messages() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(!monitor.has_active_tasks());
    assert!(monitor.has_pending_messages());

    let capability = BackgroundSubagentCapability::new(monitor.clone());
    assert_eq!(
        capability.spec().id.as_str(),
        "starweaver.subagent.background"
    );
    let mut state = AgentRunState::new(RunId::default(), ConversationId::default());
    let mut resumed = AgentContext::default();
    capability
        .on_run_start_with_context(&mut state, &mut resumed)
        .await
        .unwrap();
    let messages = capability
        .prepare_model_messages_with_context(&mut state, &mut resumed, Vec::new())
        .await
        .unwrap();
    assert!(messages.is_empty());
    let mut tool_context = ToolContext::new(RunId::default(), ConversationId::default(), 0);
    capability
        .before_tool_execution_with_context(
            &mut state,
            &mut resumed,
            &mut tool_context,
            &ToolCallPart {
                id: "call-background".to_string(),
                name: "delegate".to_string(),
                arguments: serde_json::json!({}).into(),
            },
        )
        .await
        .unwrap();
    assert!(
        tool_context
            .dependency::<BackgroundSubagentMonitor>()
            .is_some()
    );

    assert!(!monitor.has_pending_messages());
    let pending = resumed.messages.peek(resumed.agent_id.as_str());
    assert_eq!(pending[0].source, "child-bg-pending");
    assert_eq!(pending[0].target.as_deref(), Some("main"));
    assert_eq!(pending[0].content_text(), "child output");
}

#[tokio::test]
async fn runtime_delegate_tool_merges_child_context_into_parent_context() {
    let child = Arc::new(
        AgentBuilder::new(Arc::new(TestModel::with_responses(vec![ModelResponse {
            usage: Usage {
                requests: 1,
                input_tokens: 2,
                cache_write_tokens: 0,
                cache_read_tokens: 0,
                output_tokens: 3,
                total_tokens: 5,
                tool_calls: 0,
            },
            ..ModelResponse::text("child output")
        }])))
        .build(),
    );
    let registry =
        Arc::new(SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child)));
    let model = Arc::new(starweaver_agent::FunctionModel::new(
        |messages, _settings, _info| {
            let has_tool_return = messages.iter().any(|message| {
                matches!(
                    message,
                    ModelMessage::Request(request)
                        if request
                            .parts
                            .iter()
                            .any(|part| matches!(part, ModelRequestPart::ToolReturn(_)))
                )
            });
            if has_tool_return {
                Ok(ModelResponse::text("parent done"))
            } else {
                Ok(ModelResponse {
                    parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                        id: "delegate-call".to_string(),
                        name: "delegate".to_string(),
                        arguments: serde_json::json!({"name": "child", "prompt": "help"}).into(),
                    })],
                    ..ModelResponse::text("")
                })
            }
        },
    ));
    let parent = AgentBuilder::new(model)
        .policy(AgentRuntimePolicy {
            max_steps: 4,
            ..AgentRuntimePolicy::default()
        })
        .tool(registry.delegate_tool())
        .build();
    let mut context = AgentContext::default();

    let result = parent
        .run_with_context("delegate", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "parent done");
    assert_eq!(context.usage.requests, 1);
    assert_eq!(result.state.usage.requests, 1);
    assert_eq!(context.usage.tool_calls, 1);
    assert_eq!(result.state.usage.tool_calls, 1);
    assert_eq!(context.events.events()[0].kind, "run_start");
    assert_eq!(context.events.events()[1].kind, "subagent_started");
    let event_kinds = context
        .events
        .events()
        .iter()
        .map(|event| event.kind.as_str())
        .collect::<Vec<_>>();
    assert!(event_kinds.contains(&"subagent_stream_record"));
    assert!(event_kinds.contains(&"subagent_completed"));
}

#[tokio::test]
async fn subagent_execution_hook_wraps_delegated_child_run() {
    #[derive(Default)]
    struct CaptureHook {
        calls: std::sync::Mutex<Vec<serde_json::Value>>,
    }

    #[async_trait]
    impl SubagentExecutionHook for CaptureHook {
        async fn before_subagent_run(
            &self,
            metadata: SubagentExecutionMetadata,
            child_context: &mut AgentContext,
        ) -> Result<(), starweaver_agent::AgentError> {
            child_context
                .metadata
                .insert("hook_before".to_string(), serde_json::json!(true));
            self.calls.lock().unwrap().push(serde_json::json!({
                "phase": "before",
                "name": metadata.name,
                "task_id": metadata.task_id.as_str(),
                "parent_run_id": metadata.parent_run_id.as_ref().map(RunId::as_str),
                "child_agent_id": metadata.child_agent_id.as_str(),
                "child_parent_run_id": child_context.parent_run_id.as_ref().map(RunId::as_str),
                "child_parent_task_id": child_context.parent_task_id.as_ref().map(starweaver_core::TaskId::as_str),
            }));
            Ok(())
        }

        async fn after_subagent_run(
            &self,
            metadata: SubagentExecutionMetadata,
            child_context: &AgentContext,
            outcome: SubagentExecutionOutcome,
        ) -> Result<(), starweaver_agent::AgentError> {
            let SubagentExecutionOutcome::Completed {
                output,
                run_id,
                usage,
            } = outcome
            else {
                panic!("expected completed subagent outcome");
            };
            self.calls.lock().unwrap().push(serde_json::json!({
                "phase": "after",
                "name": metadata.name,
                "task_id": metadata.task_id.as_str(),
                "hook_before": child_context.metadata["hook_before"],
                "output": output,
                "run_id": run_id.as_ref().map(starweaver_core::RunId::as_str),
                "requests": usage.requests,
            }));
            Ok(())
        }
    }

    let hook = Arc::new(CaptureHook::default());
    let child = Arc::new(
        AgentBuilder::new(Arc::new(TestModel::with_responses(vec![ModelResponse {
            usage: Usage {
                requests: 1,
                ..Usage::default()
            },
            ..ModelResponse::text("child wrapped")
        }])))
        .build(),
    );
    let registry = SubagentRegistry::new()
        .with_subagent(SubagentConfig::new("child", child).with_execution_hook(hook.clone()));
    let mut context = AgentContext {
        run_id: Some(RunId::from_string("run-parent")),
        ..AgentContext::default()
    };

    let result = registry
        .delegate("child", "wrap this task", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "child wrapped");
    let calls = hook.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["phase"], "before");
    assert_eq!(calls[0]["name"], "child");
    assert_eq!(calls[0]["parent_run_id"], "run-parent");
    assert_eq!(calls[0]["child_parent_run_id"], "run-parent");
    assert!(
        calls[0]["child_parent_task_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("task_"))
    );
    assert_eq!(calls[1]["phase"], "after");
    assert_eq!(calls[1]["hook_before"], true);
    assert_eq!(calls[1]["output"], "child wrapped");
    assert_eq!(calls[1]["requests"], 1);
    assert!(calls[1]["run_id"].as_str().is_some());
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn runtime_delegate_tool_merges_child_stream_records_with_source() {
    let child = Arc::new(
        AgentBuilder::new(Arc::new(TestModel::with_responses(vec![ModelResponse {
            usage: Usage {
                requests: 1,
                input_tokens: 2,
                output_tokens: 3,
                total_tokens: 5,
                ..Usage::default()
            },
            ..ModelResponse::text("child output")
        }])))
        .build(),
    );
    let registry =
        Arc::new(SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child)));
    let model = Arc::new(starweaver_agent::FunctionModel::new(
        |messages, _settings, _info| {
            let has_tool_return = messages.iter().any(|message| {
                matches!(
                    message,
                    ModelMessage::Request(request)
                        if request
                            .parts
                            .iter()
                            .any(|part| matches!(part, ModelRequestPart::ToolReturn(_)))
                )
            });
            if has_tool_return {
                Ok(ModelResponse::text("parent done"))
            } else {
                Ok(ModelResponse {
                    parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                        id: "delegate-call".to_string(),
                        name: "delegate".to_string(),
                        arguments: serde_json::json!({"name": "child", "prompt": "help"}).into(),
                    })],
                    ..ModelResponse::text("")
                })
            }
        },
    ));
    let parent = AgentBuilder::new(model)
        .policy(AgentRuntimePolicy {
            max_steps: 4,
            ..AgentRuntimePolicy::default()
        })
        .tool(registry.delegate_tool())
        .build();
    let mut context = AgentContext::default();
    let mut stream_records = Vec::new();

    let result = parent
        .run_with_context_and_stream_events("delegate", &mut context, &mut stream_records)
        .await
        .unwrap();

    assert_eq!(result.output, "parent done");
    let Some(first_child_index) = stream_records
        .iter()
        .position(|record| record.source.is_some())
    else {
        panic!("child stream record source");
    };
    let Some(first_tool_return_index) = stream_records
        .iter()
        .position(|record| matches!(record.event, AgentStreamEvent::ToolReturn { .. }))
    else {
        panic!("parent tool return");
    };
    assert!(first_child_index < first_tool_return_index);

    let child_records = stream_records
        .iter()
        .filter(|record| {
            record
                .source
                .as_ref()
                .is_some_and(|source| source.agent_name == "child")
        })
        .collect::<Vec<_>>();
    assert!(!child_records.is_empty());
    let source = child_records[0].source.as_ref().unwrap();
    assert_eq!(&source.kind, &AgentStreamSourceKind::Subagent);
    assert!(source.agent_id.as_str().starts_with("child-task_"));
    assert_eq!(source.source_sequence, 0);
    assert!(
        source
            .task_id
            .as_ref()
            .unwrap()
            .as_str()
            .starts_with("task_")
    );
    assert!(source.run_id.is_some());
    assert_eq!(
        source.parent_run_id.as_ref().map(RunId::as_str),
        Some(result.state.run_id.as_str())
    );
    assert!(
        child_records
            .iter()
            .any(|record| matches!(record.event, AgentStreamEvent::RunStart { .. }))
    );
    assert!(
        child_records
            .iter()
            .any(|record| matches!(record.event, AgentStreamEvent::RunComplete { .. }))
    );
}
