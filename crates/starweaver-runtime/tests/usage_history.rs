#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use starweaver_core::Usage;
use starweaver_model::{
    FunctionModel, ModelMessage, ModelRequest, ModelRequestPart, ModelResponse, ModelResponsePart,
    TestModel, ToolCallPart,
};
use starweaver_runtime::{
    Agent, AgentError, CostBudget, FunctionHistoryProcessor, HistoryProcessorError,
    ReinjectSystemPromptProcessor, UsageLimitError, UsageLimits,
};
use starweaver_tools::{FunctionTool, ToolContext, ToolRegistry, ToolResult};

fn response_with_usage(text: &str, usage: Usage) -> ModelResponse {
    ModelResponse {
        usage,
        ..ModelResponse::text(text)
    }
}

#[tokio::test]
async fn usage_limits_check_next_request_before_model_call() {
    let model = Arc::new(TestModel::with_text("ok"));

    let error = Agent::new(model.clone())
        .with_usage_limits(UsageLimits::new().with_request_limit(0))
        .run("hello")
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        AgentError::UsageLimit(UsageLimitError::NextRequest {
            limit: 0,
            next_requests: 1
        })
    ));
    assert!(model.captured_messages().is_empty());
}

#[tokio::test]
async fn usage_limits_check_accumulated_tokens_after_response() {
    let model = Arc::new(TestModel::with_responses(vec![response_with_usage(
        "ok",
        Usage {
            requests: 1,
            input_tokens: 4,
            output_tokens: 2,
            total_tokens: 6,
            tool_calls: 0,
        },
    )]));

    let error = Agent::new(model)
        .with_usage_limits(UsageLimits::new().with_total_tokens_limit(5))
        .run("hello")
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        AgentError::UsageLimit(UsageLimitError::Token {
            kind: "total_tokens",
            limit: 5,
            actual: 6
        })
    ));
}

#[tokio::test]
async fn usage_limits_include_existing_context_usage() {
    let model = Arc::new(TestModel::with_responses(vec![response_with_usage(
        "ok",
        Usage {
            requests: 1,
            input_tokens: 1,
            output_tokens: 1,
            total_tokens: 2,
            tool_calls: 0,
        },
    )]));
    let mut context = starweaver_context::AgentContext {
        usage: Usage {
            requests: 1,
            input_tokens: 3,
            output_tokens: 2,
            total_tokens: 5,
            tool_calls: 0,
        },
        ..starweaver_context::AgentContext::default()
    };

    let error = Agent::new(model)
        .with_usage_limits(UsageLimits::new().with_total_tokens_limit(6))
        .run_with_context("hello", &mut context)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        AgentError::UsageLimit(UsageLimitError::Token {
            kind: "total_tokens",
            limit: 6,
            actual: 7
        })
    ));
}

#[tokio::test]
async fn history_processor_filters_messages_sent_to_model() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<ModelMessage>>::new()));
    let captured_clone = captured.clone();
    let model = FunctionModel::new(move |messages, _settings, _info| {
        captured_clone.lock().unwrap().push(messages);
        Ok(ModelResponse::text("ok"))
    });
    let processor = FunctionHistoryProcessor::new(|messages: Vec<ModelMessage>| async move {
        Ok(messages.into_iter().rev().take(1).collect::<Vec<_>>())
    });
    let prior = vec![
        ModelMessage::Request(ModelRequest::user_text("old")),
        ModelMessage::Response(ModelResponse::text("old answer")),
    ];

    let result = Agent::new(Arc::new(model))
        .with_history_processor(Arc::new(processor))
        .run_with_history("new", prior)
        .await
        .unwrap();

    assert_eq!(result.all_messages().len(), 4);
    let provider_messages = captured.lock().unwrap()[0].clone();
    assert_eq!(provider_messages.len(), 1);
    assert!(matches!(
        &provider_messages[0],
        ModelMessage::Request(request)
            if matches!(&request.parts[0], ModelRequestPart::UserPrompt { content, .. } if format!("{content:?}").contains("new"))
    ));
}

#[tokio::test]
async fn history_processor_failure_returns_capability_error() {
    let processor = FunctionHistoryProcessor::new(|_messages| async {
        Err(HistoryProcessorError::failed("cannot process history"))
    });

    let error = Agent::new(Arc::new(TestModel::with_text("ok")))
        .with_history_processor(Arc::new(processor))
        .run("hello")
        .await
        .unwrap_err();

    assert!(
        matches!(error, AgentError::Capability(message) if message == "cannot process history")
    );
}

#[test]
fn cost_budget_estimates_usage_cost_in_micros() {
    let budget = CostBudget::new()
        .with_request_micros(100)
        .with_input_micros_per_million_tokens(1_000_000)
        .with_output_micros_per_million_tokens(2_000_000)
        .with_total_cost_limit_micros(1_000);
    let usage = Usage {
        requests: 2,
        input_tokens: 10,
        output_tokens: 20,
        total_tokens: 30,
        tool_calls: 0,
    };

    assert_eq!(budget.estimate_micros(&usage), 250);
    assert_eq!(
        UsageLimits::new()
            .with_cost_budget(budget)
            .estimate_cost_micros(&usage),
        Some(250)
    );
}

#[tokio::test]
async fn usage_limits_check_accumulated_cost_after_response() {
    let model = Arc::new(TestModel::with_responses(vec![response_with_usage(
        "ok",
        Usage {
            requests: 1,
            input_tokens: 10,
            output_tokens: 20,
            total_tokens: 30,
            tool_calls: 0,
        },
    )]));

    let error = Agent::new(model)
        .with_usage_limits(
            UsageLimits::new().with_cost_budget(
                CostBudget::new()
                    .with_request_micros(100)
                    .with_input_micros_per_million_tokens(1_000_000)
                    .with_output_micros_per_million_tokens(2_000_000)
                    .with_total_cost_limit_micros(149),
            ),
        )
        .run("hello")
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        AgentError::UsageLimit(UsageLimitError::Cost {
            limit_micros: 149,
            actual_micros: 150
        })
    ));
}

#[tokio::test]
async fn usage_cost_budget_includes_existing_context_usage() {
    let model = Arc::new(TestModel::with_responses(vec![response_with_usage(
        "ok",
        Usage {
            requests: 1,
            input_tokens: 1,
            output_tokens: 1,
            total_tokens: 2,
            tool_calls: 0,
        },
    )]));
    let mut context = starweaver_context::AgentContext {
        usage: Usage {
            requests: 1,
            input_tokens: 2,
            output_tokens: 2,
            total_tokens: 4,
            tool_calls: 0,
        },
        ..starweaver_context::AgentContext::default()
    };

    let error = Agent::new(model)
        .with_usage_limits(
            UsageLimits::new().with_cost_budget(
                CostBudget::new()
                    .with_request_micros(100)
                    .with_input_micros_per_million_tokens(1_000_000)
                    .with_output_micros_per_million_tokens(1_000_000)
                    .with_total_cost_limit_micros(204),
            ),
        )
        .run_with_context("hello", &mut context)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        AgentError::UsageLimit(UsageLimitError::Cost {
            limit_micros: 204,
            actual_micros: 206
        })
    ));
}

#[tokio::test]
async fn capability_bundle_can_contribute_cost_budget() {
    let model = Arc::new(TestModel::with_responses(vec![response_with_usage(
        "ok",
        Usage {
            requests: 1,
            input_tokens: 10,
            output_tokens: 0,
            total_tokens: 10,
            tool_calls: 0,
        },
    )]));
    let bundle = starweaver_runtime::StaticCapabilityBundle::new("cost").with_usage_limits(
        UsageLimits::new().with_cost_budget(
            CostBudget::new()
                .with_input_micros_per_million_tokens(1_000_000)
                .with_total_cost_limit_micros(9),
        ),
    );

    let error = Agent::new(model)
        .with_capability_bundle(&bundle)
        .run("hello")
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        AgentError::UsageLimit(UsageLimitError::Cost {
            limit_micros: 9,
            actual_micros: 10
        })
    ));
}

fn echo_registry() -> ToolRegistry {
    let tool = FunctionTool::new(
        "echo",
        Some("Echo input".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );
    ToolRegistry::new().with_tool(Arc::new(tool))
}

fn tool_call_response(call_id: &str, tool_name: &str) -> ModelResponse {
    ModelResponse {
        parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
            id: call_id.to_string(),
            name: tool_name.to_string(),
            arguments: serde_json::json!({"value": call_id}).into(),
        })],
        ..ModelResponse::text("")
    }
}

#[tokio::test]
async fn usage_tracks_successful_function_tool_calls() {
    let model = Arc::new(TestModel::with_responses(vec![
        tool_call_response("call_1", "echo"),
        ModelResponse::text("done"),
    ]));

    let result = Agent::new(model)
        .with_tools(echo_registry())
        .with_usage_limits(UsageLimits::new().with_tool_calls_limit(1))
        .run("call echo")
        .await
        .unwrap();

    assert_eq!(result.state.usage.tool_calls, 1);
}

#[tokio::test]
async fn tool_calls_limit_is_checked_before_executing_tools() {
    let model = Arc::new(TestModel::with_responses(vec![tool_call_response(
        "call_1", "echo",
    )]));

    let error = Agent::new(model)
        .with_tools(echo_registry())
        .with_usage_limits(UsageLimits::new().with_tool_calls_limit(0))
        .run("call echo")
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        AgentError::UsageLimit(UsageLimitError::ToolCalls {
            limit: 0,
            tool_calls: 1
        })
    ));
}

#[tokio::test]
async fn tool_calls_limit_uses_existing_context_tool_calls() {
    let model = Arc::new(TestModel::with_responses(vec![tool_call_response(
        "call_1", "echo",
    )]));
    let mut context = starweaver_context::AgentContext {
        usage: Usage {
            tool_calls: 1,
            ..Usage::default()
        },
        ..starweaver_context::AgentContext::default()
    };

    let error = Agent::new(model)
        .with_tools(echo_registry())
        .with_usage_limits(UsageLimits::new().with_tool_calls_limit(1))
        .run_with_context("call echo", &mut context)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        AgentError::UsageLimit(UsageLimitError::ToolCalls {
            limit: 1,
            tool_calls: 2
        })
    ));
}

#[tokio::test]
async fn missing_tool_calls_are_not_counted_as_successful() {
    let model = Arc::new(TestModel::with_responses(vec![
        tool_call_response("call_1", "missing"),
        ModelResponse::text("done"),
    ]));

    let result = Agent::new(model)
        .with_tools(echo_registry())
        .with_usage_limits(UsageLimits::new().with_tool_calls_limit(0))
        .run("call missing")
        .await
        .unwrap();

    assert_eq!(result.state.usage.tool_calls, 0);
}

#[tokio::test]
async fn parallel_tool_calls_limit_is_checked_as_a_batch() {
    let model = Arc::new(TestModel::with_responses(vec![ModelResponse {
        parts: vec![
            ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_1".to_string(),
                name: "echo".to_string(),
                arguments: serde_json::json!({"value": 1}).into(),
            }),
            ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_2".to_string(),
                name: "echo".to_string(),
                arguments: serde_json::json!({"value": 2}).into(),
            }),
        ],
        ..ModelResponse::text("")
    }]));

    let error = Agent::new(model)
        .with_tools(echo_registry())
        .with_usage_limits(UsageLimits::new().with_tool_calls_limit(1))
        .run("call echo twice")
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        AgentError::UsageLimit(UsageLimitError::ToolCalls {
            limit: 1,
            tool_calls: 2
        })
    ));
}

#[tokio::test]
async fn reinject_system_prompt_processor_restores_filtered_instructions() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<ModelMessage>>::new()));
    let captured_clone = captured.clone();
    let model = FunctionModel::new(move |messages, _settings, _info| {
        captured_clone.lock().unwrap().push(messages);
        Ok(ModelResponse::text("ok"))
    });
    let strip_instructions =
        FunctionHistoryProcessor::new(|messages: Vec<ModelMessage>| async move {
            Ok(messages
                .into_iter()
                .map(|message| match message {
                    ModelMessage::Request(mut request) => {
                        request.parts.retain(|part| {
                            !matches!(
                                part,
                                ModelRequestPart::SystemPrompt { .. }
                                    | ModelRequestPart::Instruction { .. }
                            )
                        });
                        ModelMessage::Request(request)
                    }
                    ModelMessage::Response(response) => ModelMessage::Response(response),
                })
                .collect())
        });

    let result = Agent::new(Arc::new(model))
        .with_instruction("System policy")
        .with_history_processor(Arc::new(strip_instructions))
        .with_history_processor(Arc::new(ReinjectSystemPromptProcessor::new()))
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
    let provider_messages = captured.lock().unwrap()[0].clone();
    assert_eq!(provider_messages.len(), 2);
    assert!(matches!(
        &provider_messages[0],
        ModelMessage::Request(request)
            if matches!(&request.parts[0], ModelRequestPart::SystemPrompt { text, .. } if text == "System policy")
    ));
}

#[tokio::test]
async fn reinject_system_prompt_processor_keeps_existing_instructions_once() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<ModelMessage>>::new()));
    let captured_clone = captured.clone();
    let model = FunctionModel::new(move |messages, _settings, _info| {
        captured_clone.lock().unwrap().push(messages);
        Ok(ModelResponse::text("ok"))
    });

    Agent::new(Arc::new(model))
        .with_instruction("System policy")
        .with_history_processor(Arc::new(ReinjectSystemPromptProcessor::new()))
        .run("hello")
        .await
        .unwrap();

    let provider_messages = captured.lock().unwrap()[0].clone();
    let instruction_count = provider_messages
        .iter()
        .flat_map(|message| match message {
            ModelMessage::Request(request) => request.parts.iter().collect::<Vec<_>>(),
            ModelMessage::Response(_) => Vec::new(),
        })
        .filter(|part| matches!(part, ModelRequestPart::SystemPrompt { text, .. } if text == "System policy"))
        .count();
    assert_eq!(instruction_count, 1);
}
