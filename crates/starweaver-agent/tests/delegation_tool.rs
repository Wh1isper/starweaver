#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use async_trait::async_trait;
use starweaver_agent::{
    AgentBuilder, AgentCapability, AgentContext, AgentContextHandle, AgentRunState,
    AgentRuntimePolicy, AgentStreamEvent, AgentStreamSourceKind, BackgroundSubagentCapability,
    BackgroundSubagentMonitor, FunctionTool, SubagentConfig, SubagentDelegationMode,
    SubagentExecutionHook, SubagentExecutionMetadata, SubagentExecutionOutcome,
    SubagentParentTools, SubagentRegistry, SubagentToolInheritancePolicy, TestModel, ToolContext,
    ToolRegistry, ToolResult, DELEGATE_BACKEND_TOOL_NAME, SPAWN_DELEGATE_TOOL_NAME,
};
use starweaver_core::{ConversationId, RunId};
use starweaver_model::{
    ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart, ToolCallPart,
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
    assert!(error
        .to_string()
        .contains("missing AgentContextHandle dependency"));
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
            "subagent_info".to_string()
        ]
    );
    assert_eq!(
        visible_tool_names(&tools),
        vec!["delegate".to_string(), "subagent_info".to_string()]
    );
    let instructions = tools.get_instructions().join("\n");
    assert!(instructions.contains("delegate is asynchronous"));
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
            "subagent_info".to_string()
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
    assert!(context_handle
        .snapshot()
        .events
        .events()
        .iter()
        .any(|event| event.kind == "subagent_completed"));
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
    let mut state = AgentRunState::new(RunId::default(), ConversationId::default());
    let mut resumed = AgentContext::default();
    capability
        .on_run_start_with_context(&mut state, &mut resumed)
        .await
        .unwrap();

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
                "child_agent_id": metadata.child_agent_id.as_str(),
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
    let mut context = AgentContext::default();

    let result = registry
        .delegate("child", "wrap this task", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "child wrapped");
    let calls = hook.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["phase"], "before");
    assert_eq!(calls[0]["name"], "child");
    assert_eq!(calls[1]["phase"], "after");
    assert_eq!(calls[1]["hook_before"], true);
    assert_eq!(calls[1]["output"], "child wrapped");
    assert_eq!(calls[1]["requests"], 1);
    assert!(calls[1]["run_id"].as_str().is_some());
}

#[tokio::test]
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
    assert!(source
        .task_id
        .as_ref()
        .unwrap()
        .as_str()
        .starts_with("task_"));
    assert!(source.run_id.is_some());
    assert_eq!(
        source.parent_run_id.as_ref().map(RunId::as_str),
        Some(result.state.run_id.as_str())
    );
    assert!(child_records
        .iter()
        .any(|record| matches!(record.event, AgentStreamEvent::RunStart { .. })));
    assert!(child_records
        .iter()
        .any(|record| matches!(record.event, AgentStreamEvent::RunComplete { .. })));
}
