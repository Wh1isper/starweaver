#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use starweaver_model::{
    ModelMessage, ModelResponse, ModelResponsePart, TestModel, ToolCallPart, tool_call_response,
};
use starweaver_runtime::{
    Agent, AgentEndStrategy, AgentRuntimePolicy, FunctionOutputFunction, OutputFunctionContext,
    OutputFunctionDefinition, OutputValidationError, OutputValue,
};

fn final_answer_function() -> FunctionOutputFunction<
    impl Send
    + Sync
    + Fn(
        OutputFunctionContext,
        serde_json::Value,
    ) -> std::future::Ready<Result<OutputValue, OutputValidationError>>,
> {
    FunctionOutputFunction::new(
        OutputFunctionDefinition::new(
            "final_answer",
            serde_json::json!({
                "type": "object",
                "properties": {"answer": {"type": "string"}},
                "required": ["answer"]
            }),
        )
        .with_description("Return the final answer"),
        |_ctx, args: serde_json::Value| {
            std::future::ready(Ok(OutputValue::Json(serde_json::json!({
                "answer": args["answer"].as_str().unwrap_or_default()
            }))))
        },
    )
}

fn output_and_lookup_response() -> ModelResponse {
    let mut response = tool_call_response(
        "call_output",
        "final_answer",
        serde_json::json!({"answer": "Paris"}),
    );
    response
        .parts
        .push(ModelResponsePart::ToolCall(ToolCallPart {
            id: "call_lookup".to_string(),
            name: "lookup".to_string(),
            arguments: serde_json::json!({"query": "Paris"}).into(),
        }));
    response
}

#[tokio::test]
async fn output_function_call_finishes_run() {
    let model = Arc::new(TestModel::with_responses(vec![tool_call_response(
        "call_1",
        "final_answer",
        serde_json::json!({"answer": "Paris"}),
    )]));

    let result = Agent::new(model.clone())
        .with_output_function(Arc::new(final_answer_function()))
        .run("answer")
        .await
        .unwrap();

    assert_eq!(result.output, r#"{"answer":"Paris"}"#);
    assert_eq!(result.structured_output.unwrap()["answer"], "Paris");
    assert_eq!(model.captured_params()[0].tools[0].name, "final_answer");
}

#[tokio::test]
async fn early_end_strategy_skips_ordinary_tools_after_output_function() {
    let model = Arc::new(TestModel::with_responses(
        vec![output_and_lookup_response()],
    ));
    let lookup_calls = Arc::new(AtomicUsize::new(0));
    let lookup_counter = lookup_calls.clone();
    let lookup = starweaver_tools::FunctionTool::new(
        "lookup",
        Some("Lookup".to_string()),
        serde_json::json!({"type": "object"}),
        move |_ctx, args| {
            lookup_counter.fetch_add(1, Ordering::SeqCst);
            async move { Ok(starweaver_tools::ToolResult::new(args)) }
        },
    );

    let result = Agent::new(model.clone())
        .with_output_function(Arc::new(final_answer_function()))
        .with_tools(starweaver_tools::ToolRegistry::new().with_tool(Arc::new(lookup)))
        .run("answer")
        .await
        .unwrap();

    assert_eq!(result.output, r#"{"answer":"Paris"}"#);
    assert_eq!(lookup_calls.load(Ordering::SeqCst), 0);
    assert_eq!(model.captured_messages().len(), 1);
    assert!(!result
        .messages
        .iter()
        .any(|message| matches!(message, ModelMessage::Request(request) if request.parts.iter().any(|part| matches!(part, starweaver_model::ModelRequestPart::ToolReturn(_))))));
}

#[tokio::test]
async fn exhaustive_end_strategy_runs_ordinary_tools_after_output_function() {
    let model = Arc::new(TestModel::with_responses(
        vec![output_and_lookup_response()],
    ));
    let lookup_calls = Arc::new(AtomicUsize::new(0));
    let lookup_counter = lookup_calls.clone();
    let lookup = starweaver_tools::FunctionTool::new(
        "lookup",
        Some("Lookup".to_string()),
        serde_json::json!({"type": "object"}),
        move |_ctx, args| {
            lookup_counter.fetch_add(1, Ordering::SeqCst);
            async move { Ok(starweaver_tools::ToolResult::new(args)) }
        },
    );

    let result = Agent::new(model.clone())
        .with_output_function(Arc::new(final_answer_function()))
        .with_tools(starweaver_tools::ToolRegistry::new().with_tool(Arc::new(lookup)))
        .with_policy(AgentRuntimePolicy {
            end_strategy: AgentEndStrategy::Exhaustive,
            ..AgentRuntimePolicy::default()
        })
        .run("answer")
        .await
        .unwrap();

    assert_eq!(result.output, r#"{"answer":"Paris"}"#);
    assert_eq!(lookup_calls.load(Ordering::SeqCst), 1);
    assert_eq!(model.captured_messages().len(), 1);
    assert!(result.messages.iter().any(|message| {
        matches!(message, ModelMessage::Request(request)
            if request.metadata.get("starweaver.final_output_tool_returns") == Some(&serde_json::json!(true))
                && request.parts.iter().any(|part| matches!(part, starweaver_model::ModelRequestPart::ToolReturn(tool_return) if tool_return.name == "lookup")))
    }));
}

#[tokio::test]
async fn output_function_retry_sends_retry_prompt_and_accepts_next_call() {
    let retry_function = FunctionOutputFunction::new(
        OutputFunctionDefinition::new(
            "final_answer",
            serde_json::json!({"type": "object", "required": ["answer"]}),
        ),
        |_ctx, args: serde_json::Value| {
            let answer = args["answer"].as_str().unwrap_or_default().to_string();
            async move {
                if answer == "Paris" {
                    Ok(OutputValue::Json(serde_json::json!({"answer": answer})))
                } else {
                    Err(OutputValidationError::retry("answer must be Paris"))
                }
            }
        },
    );
    let model = Arc::new(TestModel::with_responses(vec![
        tool_call_response(
            "call_1",
            "final_answer",
            serde_json::json!({"answer": "London"}),
        ),
        tool_call_response(
            "call_2",
            "final_answer",
            serde_json::json!({"answer": "Paris"}),
        ),
    ]));

    let result = Agent::new(model.clone())
        .with_output_function(Arc::new(retry_function))
        .with_policy(AgentRuntimePolicy {
            max_steps: 3,
            output_retries: 1,
            ..AgentRuntimePolicy::default()
        })
        .run("answer")
        .await
        .unwrap();

    assert_eq!(result.output, r#"{"answer":"Paris"}"#);
    assert_eq!(result.structured_output.unwrap()["answer"], "Paris");
    assert_eq!(model.captured_messages().len(), 2);
    assert!(format!("{:?}", model.captured_messages()[1]).contains("answer must be Paris"));
}

#[tokio::test]
async fn output_function_retry_respects_retry_budget() {
    let retry_function = FunctionOutputFunction::new(
        OutputFunctionDefinition::new(
            "final_answer",
            serde_json::json!({"type": "object", "required": ["answer"]}),
        ),
        |_ctx, _args: serde_json::Value| async move {
            Err(OutputValidationError::retry("answer must be Paris"))
        },
    );
    let model = Arc::new(TestModel::with_responses(vec![tool_call_response(
        "call_1",
        "final_answer",
        serde_json::json!({"answer": "London"}),
    )]));

    let error = Agent::new(model.clone())
        .with_output_function(Arc::new(retry_function))
        .with_policy(AgentRuntimePolicy {
            max_steps: 3,
            output_retries: 0,
            ..AgentRuntimePolicy::default()
        })
        .run("answer")
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        starweaver_runtime::AgentError::OutputRetryLimitExceeded { retries: 0 }
    ));
    assert_eq!(model.captured_messages().len(), 1);
}

#[tokio::test]
async fn ordinary_tool_call_still_uses_tool_loop() {
    let model = Arc::new(TestModel::with_responses(vec![
        tool_call_response("call_1", "lookup", serde_json::json!({"query": "Paris"})),
        ModelResponse::text("lookup done"),
    ]));
    let lookup = starweaver_tools::FunctionTool::new(
        "lookup",
        Some("Lookup".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx, args| async move { Ok(starweaver_tools::ToolResult::new(args)) },
    );

    let result = Agent::new(model)
        .with_output_function(Arc::new(final_answer_function()))
        .with_tools(starweaver_tools::ToolRegistry::new().with_tool(Arc::new(lookup)))
        .run("lookup")
        .await
        .unwrap();

    assert_eq!(result.output, "lookup done");
}
