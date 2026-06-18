#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use async_trait::async_trait;
use starweaver_agent::{
    AgentBuilder, AgentContext, AgentContextHandle, AgentRuntimePolicy, AgentStreamEvent,
    AgentStreamSourceKind, FunctionTool, SubagentConfig, SubagentExecutionHook,
    SubagentExecutionMetadata, SubagentExecutionOutcome, SubagentParentTools, SubagentRegistry,
    SubagentToolInheritancePolicy, TestModel, ToolContext, ToolRegistry, ToolResult,
};
use starweaver_core::{ConversationId, RunId};
use starweaver_model::{
    ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart, ToolCallPart,
};
use starweaver_usage::Usage;

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
