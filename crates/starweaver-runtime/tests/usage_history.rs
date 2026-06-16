#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use starweaver_context::AgentContext;
use starweaver_model::{
    FunctionModel, ModelMessage, ModelRequest, ModelRequestPart, ModelResponse, ModelResponsePart,
    TestModel, ToolCallPart, ToolReturnPart,
};
use starweaver_runtime::{
    Agent, AgentCapability, AgentError, AgentRunState, AgentStreamEvent, CapabilityError,
    CapabilityResult,
};
use starweaver_tools::{FunctionTool, ToolContext, ToolRegistry, ToolResult};
use starweaver_usage::{pricing::CostBudget, Usage, UsageLimitError, UsageLimits, UsageTokenKind};

struct KeepLatestMessageCapability;

#[async_trait]
impl AgentCapability for KeepLatestMessageCapability {
    async fn prepare_provider_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut AgentContext,
        messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        Ok(messages.into_iter().rev().take(1).collect())
    }
}

struct FailingMessagesCapability;

#[async_trait]
impl AgentCapability for FailingMessagesCapability {
    async fn prepare_model_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut AgentContext,
        _messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        Err(CapabilityError::Failed(
            "cannot process messages".to_string(),
        ))
    }
}

struct StripInstructionsCapability;

#[async_trait]
impl AgentCapability for StripInstructionsCapability {
    async fn prepare_model_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut AgentContext,
        messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
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
    }
}

struct RestoreInstructionsCapability;

#[async_trait]
impl AgentCapability for RestoreInstructionsCapability {
    async fn prepare_model_messages_with_context(
        &self,
        state: &mut AgentRunState,
        _context: &mut AgentContext,
        mut messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        let source_parts = instruction_parts(&state.message_history);
        let existing = instruction_parts(&messages);
        if !source_parts.is_empty() && !source_parts.iter().all(|part| existing.contains(part)) {
            messages.insert(
                0,
                ModelMessage::Request(ModelRequest {
                    parts: source_parts,
                    timestamp: None,
                    instructions: None,
                    run_id: None,
                    conversation_id: None,
                    metadata: serde_json::Map::new(),
                }),
            );
        }
        Ok(messages)
    }
}

fn instruction_parts(messages: &[ModelMessage]) -> Vec<ModelRequestPart> {
    messages
        .iter()
        .flat_map(|message| match message {
            ModelMessage::Request(request) => request
                .parts
                .iter()
                .filter(|part| {
                    matches!(
                        part,
                        ModelRequestPart::SystemPrompt { .. }
                            | ModelRequestPart::Instruction { .. }
                    )
                })
                .cloned()
                .collect::<Vec<_>>(),
            ModelMessage::Response(_) => Vec::new(),
        })
        .collect()
}

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
            cache_write_tokens: 0,
            cache_read_tokens: 0,
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
            kind: UsageTokenKind::TotalTokens,
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
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            output_tokens: 1,
            total_tokens: 2,
            tool_calls: 0,
        },
    )]));
    let mut context = starweaver_context::AgentContext {
        usage: Usage {
            requests: 1,
            input_tokens: 3,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
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
            kind: UsageTokenKind::TotalTokens,
            limit: 6,
            actual: 7
        })
    ));
}

#[tokio::test]
async fn usage_snapshot_includes_existing_context_usage() {
    let model = Arc::new(TestModel::with_responses(vec![response_with_usage(
        "ok",
        Usage {
            requests: 1,
            input_tokens: 1,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            output_tokens: 1,
            total_tokens: 2,
            tool_calls: 0,
        },
    )]));
    let mut context = AgentContext {
        usage: Usage {
            requests: 1,
            input_tokens: 3,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            output_tokens: 2,
            total_tokens: 5,
            tool_calls: 0,
        },
        ..AgentContext::default()
    };
    let mut events = Vec::new();

    Agent::new(model)
        .with_usage_limits(
            UsageLimits::new().with_cost_budget(
                CostBudget::new()
                    .with_request_micros(100)
                    .with_input_micros_per_million_tokens(1_000_000)
                    .with_output_micros_per_million_tokens(2_000_000),
            ),
        )
        .run_with_context_and_stream_events("hello", &mut context, &mut events)
        .await
        .unwrap();

    let snapshots = events
        .iter()
        .filter_map(|record| match &record.event {
            AgentStreamEvent::Custom { event } if event.kind == "usage_snapshot" => Some(
                serde_json::from_value::<starweaver_usage::UsageSnapshot>(event.payload.clone())
                    .unwrap(),
            ),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].latest_usage.as_ref().unwrap().total_tokens, 2);
    assert_eq!(snapshots[0].total_usage.requests, 2);
    assert_eq!(snapshots[0].total_usage.input_tokens, 4);
    assert_eq!(snapshots[0].total_usage.output_tokens, 3);
    assert_eq!(snapshots[0].total_usage.total_tokens, 7);
    assert_eq!(snapshots[0].agent_usages["main"].usage.total_tokens, 7);
    assert_eq!(
        snapshots[0]
            .estimate_pricing
            .as_ref()
            .unwrap()
            .amount_micros_usd,
        210
    );
}

#[tokio::test]
async fn run_stream_emits_cumulative_usage_snapshot_events() {
    let model = Arc::new(TestModel::with_responses(vec![
        ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_usage".to_string(),
                name: "echo".to_string(),
                arguments: serde_json::json!({"value": "again"}).into(),
            })],
            usage: Usage {
                requests: 1,
                input_tokens: 3,
                cache_write_tokens: 2,
                cache_read_tokens: 1,
                output_tokens: 4,
                total_tokens: 7,
                tool_calls: 0,
            },
            ..ModelResponse::text("")
        },
        response_with_usage(
            "done",
            Usage {
                requests: 1,
                input_tokens: 5,
                cache_write_tokens: 3,
                cache_read_tokens: 7,
                output_tokens: 6,
                total_tokens: 11,
                tool_calls: 0,
            },
        ),
    ]));
    let result = Agent::new(model)
        .with_tools(echo_registry())
        .with_usage_limits(
            UsageLimits::new().with_cost_budget(
                CostBudget::new()
                    .with_request_micros(100)
                    .with_input_micros_per_million_tokens(1_000_000)
                    .with_output_micros_per_million_tokens(2_000_000),
            ),
        )
        .run_stream("hello")
        .await
        .unwrap();

    let snapshots = result
        .events()
        .iter()
        .filter_map(|record| match &record.event {
            AgentStreamEvent::Custom { event } if event.kind == "usage_snapshot" => Some(
                serde_json::from_value::<starweaver_usage::UsageSnapshot>(event.payload.clone())
                    .unwrap(),
            ),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].latest_usage.as_ref().unwrap().total_tokens, 7);
    assert_eq!(snapshots[0].total_usage.total_tokens, 7);
    assert_eq!(snapshots[1].latest_usage.as_ref().unwrap().input_tokens, 5);
    assert_eq!(
        snapshots[1]
            .latest_usage
            .as_ref()
            .unwrap()
            .cache_read_tokens,
        7
    );
    assert_eq!(snapshots[1].latest_usage.as_ref().unwrap().total_tokens, 11);
    assert_eq!(snapshots[1].total_usage.input_tokens, 8);
    assert_eq!(snapshots[1].total_usage.cache_write_tokens, 5);
    assert_eq!(snapshots[1].total_usage.cache_read_tokens, 8);
    assert_eq!(snapshots[1].total_usage.output_tokens, 10);
    assert_eq!(snapshots[1].total_usage.total_tokens, 18);
    assert_eq!(snapshots[1].agent_usages["main"].usage.requests, 2);
    assert_eq!(snapshots[1].model_usages["test:test"].total_tokens, 18);
    assert_eq!(
        snapshots[1]
            .estimate_pricing
            .as_ref()
            .unwrap()
            .amount_micros_usd,
        228
    );
    assert_eq!(
        snapshots[1].agent_usages["main"]
            .estimate_pricing
            .as_ref()
            .unwrap()
            .amount_micros_usd,
        228
    );
    assert_eq!(
        snapshots[1].model_estimate_pricing["test:test"].amount_micros_usd,
        228
    );
}

#[tokio::test]
async fn message_prepare_capability_filters_messages_sent_to_model() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<ModelMessage>>::new()));
    let captured_clone = captured.clone();
    let model = FunctionModel::new(move |messages, _settings, _info| {
        captured_clone.lock().unwrap().push(messages);
        Ok(ModelResponse::text("ok"))
    });
    let prior = vec![
        ModelMessage::Request(ModelRequest::user_text("old")),
        ModelMessage::Response(ModelResponse::text("old answer")),
    ];

    let result = Agent::new(Arc::new(model))
        .with_capability(Arc::new(KeepLatestMessageCapability))
        .run_with_history("new", prior)
        .await
        .unwrap();

    assert_eq!(result.all_messages().len(), 4);
    let provider_messages = captured.lock().unwrap()[0].clone();
    assert_eq!(provider_messages.len(), 1);
    assert!(matches!(
        &provider_messages[0],
        ModelMessage::Request(request)
            if request.parts.iter().any(|part| matches!(part, ModelRequestPart::UserPrompt { content, .. } if format!("{content:?}").contains("new")))
    ));
}

#[tokio::test]
async fn message_prepare_capability_failure_returns_capability_error() {
    let error = Agent::new(Arc::new(TestModel::with_text("ok")))
        .with_capability(Arc::new(FailingMessagesCapability))
        .run("hello")
        .await
        .unwrap_err();

    assert!(
        matches!(error, AgentError::Capability(message) if message == "cannot process messages")
    );
}

#[tokio::test]
async fn provider_messages_sanitize_unclosed_tool_call_history() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<ModelMessage>>::new()));
    let captured_clone = captured.clone();
    let model = FunctionModel::new(move |messages, _settings, _info| {
        captured_clone.lock().unwrap().push(messages);
        Ok(ModelResponse::text("continued"))
    });
    let prior = vec![
        ModelMessage::Request(ModelRequest::user_text("run a tool")),
        ModelMessage::Response(tool_call_response("call_interrupted", "echo")),
    ];

    let result = Agent::new(Arc::new(model))
        .run_with_history("continue", prior)
        .await
        .unwrap();

    assert_eq!(result.output, "continued");
    let provider_messages = captured.lock().unwrap()[0].clone();
    assert_eq!(provider_messages.len(), 2);
    assert!(matches!(
        &provider_messages[0],
        ModelMessage::Request(request)
            if request.parts.iter().any(|part| matches!(part, ModelRequestPart::UserPrompt { content, .. } if format!("{content:?}").contains("run a tool")))
    ));
    assert!(matches!(
        &provider_messages[1],
        ModelMessage::Request(request)
            if request.parts.iter().any(|part| matches!(part, ModelRequestPart::UserPrompt { content, .. } if format!("{content:?}").contains("continue")))
    ));
    assert!(!provider_messages.iter().any(|message| matches!(
        message,
        ModelMessage::Response(response) if !response.tool_calls().is_empty()
    )));
}

#[tokio::test]
async fn provider_messages_keep_closed_tool_call_history() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<ModelMessage>>::new()));
    let captured_clone = captured.clone();
    let model = FunctionModel::new(move |messages, _settings, _info| {
        captured_clone.lock().unwrap().push(messages);
        Ok(ModelResponse::text("continued"))
    });
    let prior = vec![
        ModelMessage::Request(ModelRequest::user_text("run a tool")),
        ModelMessage::Response(tool_call_response("call_done", "echo")),
        ModelMessage::Request(ModelRequest {
            parts: vec![ModelRequestPart::ToolReturn(ToolReturnPart::new(
                "call_done",
                "echo",
                serde_json::json!({"value": "ok"}),
            ))],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        }),
        ModelMessage::Response(ModelResponse::text("tool done")),
    ];

    let result = Agent::new(Arc::new(model))
        .run_with_history("continue", prior)
        .await
        .unwrap();

    assert_eq!(result.output, "continued");
    let provider_messages = captured.lock().unwrap()[0].clone();
    assert_eq!(provider_messages.len(), 5);
    let Some(wrapped_call_id) = provider_messages.iter().find_map(|message| {
        let ModelMessage::Response(response) = message else {
            return None;
        };
        response.tool_calls().first().map(|call| call.id.clone())
    }) else {
        panic!("closed tool call should stay in provider history");
    };
    assert!(wrapped_call_id.starts_with("sw-tool-"));
    assert!(provider_messages.iter().any(|message| matches!(
        message,
        ModelMessage::Request(request)
            if request.parts.iter().any(|part| matches!(part, ModelRequestPart::ToolReturn(tool_return) if tool_return.tool_call_id == wrapped_call_id))
    )));
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
        cache_write_tokens: 0,
        cache_read_tokens: 0,
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
            cache_write_tokens: 0,
            cache_read_tokens: 0,
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
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            output_tokens: 1,
            total_tokens: 2,
            tool_calls: 0,
        },
    )]));
    let mut context = starweaver_context::AgentContext {
        usage: Usage {
            requests: 1,
            input_tokens: 2,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
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
            cache_write_tokens: 0,
            cache_read_tokens: 0,
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
async fn restore_instructions_capability_restores_filtered_instructions() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<ModelMessage>>::new()));
    let captured_clone = captured.clone();
    let model = FunctionModel::new(move |messages, _settings, _info| {
        captured_clone.lock().unwrap().push(messages);
        Ok(ModelResponse::text("ok"))
    });

    let result = Agent::new(Arc::new(model))
        .with_instruction("System policy")
        .with_capability(Arc::new(StripInstructionsCapability))
        .with_capability(Arc::new(RestoreInstructionsCapability))
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
    let provider_messages = captured.lock().unwrap()[0].clone();
    assert_eq!(provider_messages.len(), 2);
    assert!(matches!(
        &provider_messages[0],
        ModelMessage::Request(request)
            if matches!(&request.parts[0], ModelRequestPart::Instruction { text, .. } if text == "System policy")
    ));
}

#[tokio::test]
async fn restore_instructions_capability_keeps_existing_instructions_once() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<ModelMessage>>::new()));
    let captured_clone = captured.clone();
    let model = FunctionModel::new(move |messages, _settings, _info| {
        captured_clone.lock().unwrap().push(messages);
        Ok(ModelResponse::text("ok"))
    });

    Agent::new(Arc::new(model))
        .with_instruction("System policy")
        .with_capability(Arc::new(RestoreInstructionsCapability))
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
        .filter(|part| matches!(part, ModelRequestPart::Instruction { text, .. } if text == "System policy"))
        .count();
    assert_eq!(instruction_count, 1);
}
