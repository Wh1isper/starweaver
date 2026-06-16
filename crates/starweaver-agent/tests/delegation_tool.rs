#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{
    AgentBuilder, AgentContext, AgentContextHandle, AgentRuntimePolicy, SubagentConfig,
    SubagentRegistry, TestModel, ToolContext,
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
    let registry = Arc::new(SubagentRegistry::new().with_subagent(
        SubagentConfig::new("child", child).with_description("Answers child tasks"),
    ));
    let subagent_info = registry.subagent_info_tool();

    assert_eq!(subagent_info.name(), "subagent_info");
    let schema = subagent_info.parameters_schema();
    assert_eq!(schema["type"], "object");
    assert!(schema["properties"].as_object().unwrap().is_empty());

    let result = subagent_info
        .call(
            ToolContext::new(RunId::default(), ConversationId::default(), 0),
            serde_json::json!({}),
        )
        .await
        .unwrap();

    assert_eq!(result.content["subagents"][0]["name"], "child");
    assert_eq!(
        result.content["subagents"][0]["description"],
        "Answers child tasks"
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
    assert_eq!(context.events.events()[2].kind, "subagent_completed");
}
