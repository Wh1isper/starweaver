#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{
    AgentBuilder, AgentContext, AgentContextHandle, AgentRuntimePolicy, FunctionModel,
    FunctionOutputValidator, OutputValidationError, OutputValidationResult, OutputValue,
    SubagentConfig, SubagentRegistry, TestModel, ToolContext,
};
use starweaver_core::{ConversationId, RunId, Usage};
use starweaver_model::{
    ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart, ToolCallPart,
};
use starweaver_runtime::AgentRunState;

fn failing_validator(
    _state: &mut AgentRunState,
    _output: &OutputValue,
) -> std::future::Ready<OutputValidationResult<()>> {
    std::future::ready(Err(OutputValidationError::failed(
        "child validation failed",
    )))
}

fn delegate_then_finish_model(name: &'static str) -> FunctionModel {
    FunctionModel::new(move |messages, _settings, _info| {
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
                    arguments: serde_json::json!({"name": name, "prompt": "help"}),
                })],
                ..ModelResponse::text("")
            })
        }
    })
}

#[tokio::test]
async fn delegate_tool_failure_updates_context_handle() {
    let registry = Arc::new(SubagentRegistry::new());
    let delegate = registry.delegate_tool();
    let parent = AgentContext::default();
    let context_handle = AgentContextHandle::new(parent.clone());
    let mut dependencies = parent.dependencies.clone();
    dependencies.insert(context_handle.clone());
    let context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);

    let error = delegate
        .call(
            context,
            serde_json::json!({"name": "missing", "prompt": "help"}),
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("missing subagent missing"));
    let snapshot = context_handle.snapshot();
    assert_eq!(snapshot.events.events().len(), 1);
    assert_eq!(snapshot.events.events()[0].kind, "subagent_failed");
}

#[tokio::test]
async fn runtime_delegate_tool_error_path_merges_failed_lifecycle_event() {
    let registry = Arc::new(SubagentRegistry::new());
    let parent = AgentBuilder::new(Arc::new(delegate_then_finish_model("missing")))
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
    assert_eq!(context.events.events()[0].kind, "run_start");
    assert_eq!(context.events.events()[1].kind, "subagent_failed");
}

#[tokio::test]
async fn failing_subagent_absorbs_usage_into_parent_context() {
    let child = Arc::new(
        AgentBuilder::new(Arc::new(TestModel::with_responses(vec![ModelResponse {
            usage: Usage {
                requests: 1,
                input_tokens: 7,
                output_tokens: 11,
                total_tokens: 18,
                tool_calls: 0,
            },
            ..ModelResponse::text("bad child output")
        }])))
        .output_validator(Arc::new(FunctionOutputValidator::new(failing_validator)))
        .build(),
    );
    let registry = SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child));
    let mut context = AgentContext {
        usage: Usage {
            requests: 2,
            input_tokens: 3,
            output_tokens: 5,
            total_tokens: 8,
            tool_calls: 1,
        },
        ..AgentContext::default()
    };

    let error = registry
        .delegate("child", "help", &mut context)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("child validation failed"));
    assert_eq!(context.usage.requests, 3);
    assert_eq!(context.usage.input_tokens, 10);
    assert_eq!(context.usage.output_tokens, 16);
    assert_eq!(context.usage.total_tokens, 26);
    assert_eq!(context.usage.tool_calls, 1);
    assert_eq!(context.events.events()[0].kind, "subagent_started");
    assert_eq!(context.events.events()[1].kind, "subagent_failed");
}
