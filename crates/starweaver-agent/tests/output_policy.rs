#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{
    AgentBuilder, FunctionModel, OutputPolicy, OutputSchema, TestModel, ToolKind,
    tool_metadata_kind,
};
use starweaver_model::{ModelResponse, OutputMode, tool_call_response};

#[tokio::test]
async fn builder_accepts_complete_output_policy() {
    let policy = OutputPolicy::structured(OutputSchema::new(
        "answer",
        serde_json::json!({"type": "object", "required": ["answer"]}),
    ))
    .with_retries(2);

    let result = AgentBuilder::new(Arc::new(TestModel::with_responses(vec![
        ModelResponse::text("not-json"),
        ModelResponse::text(r#"{"answer":"ok"}"#),
    ])))
    .output_policy(policy)
    .build()
    .run("answer")
    .await
    .unwrap();

    assert_eq!(result.structured_output.unwrap()["answer"], "ok");
}

fn answer_schema() -> OutputSchema {
    OutputSchema::new(
        "answer",
        serde_json::json!({
            "type": "object",
            "properties": {"answer": {"type": "string"}},
            "required": ["answer"],
            "additionalProperties": false
        }),
    )
}

#[tokio::test]
async fn builder_output_policy_helpers_forward_request_modes() {
    let prompted_model = Arc::new(FunctionModel::new(|_, _, _| {
        Ok(ModelResponse::text(r#"{"answer":"ok"}"#))
    }));
    AgentBuilder::new(prompted_model.clone())
        .output_policy(OutputPolicy::prompted(answer_schema()))
        .build()
        .run("answer")
        .await
        .unwrap();
    let prompted_params = prompted_model.captured_params()[0].clone();
    assert_eq!(prompted_params.output_mode, Some(OutputMode::Prompted));

    let tool_or_text_model = Arc::new(FunctionModel::new(|_, _, _| {
        Ok(ModelResponse::text(r#"{"answer":"ok"}"#))
    }));
    AgentBuilder::new(tool_or_text_model.clone())
        .output_policy(OutputPolicy::tool_or_text(answer_schema()))
        .build()
        .run("answer")
        .await
        .unwrap();
    let tool_or_text_params = tool_or_text_model.captured_params()[0].clone();
    assert_eq!(
        tool_or_text_params.output_mode,
        Some(OutputMode::ToolOrText)
    );
    assert_eq!(tool_or_text_params.allow_text_output, Some(true));

    let image_model = Arc::new(FunctionModel::new(|_, _, _| Ok(ModelResponse::text("ok"))));
    AgentBuilder::new(image_model.clone())
        .output_policy(OutputPolicy::image())
        .build()
        .run("image")
        .await
        .unwrap();
    let image_params = image_model.captured_params()[0].clone();
    assert_eq!(image_params.output_mode, Some(OutputMode::Image));
    assert_eq!(image_params.allow_image_output, Some(true));
    assert_eq!(image_params.allow_text_output, Some(false));
}

#[tokio::test]
async fn builder_tool_output_policy_installs_schema_output_function() {
    let model = Arc::new(TestModel::with_responses(vec![tool_call_response(
        "call_1",
        "answer",
        serde_json::json!({"answer":"Paris"}),
    )]));

    let result = AgentBuilder::new(model.clone())
        .output_policy(OutputPolicy::tool(answer_schema()))
        .build()
        .run("answer")
        .await
        .unwrap();

    assert_eq!(result.output, r#"{"answer":"Paris"}"#);
    assert_eq!(result.structured_output.unwrap()["answer"], "Paris");
    let params = model.captured_params()[0].clone();
    assert_eq!(params.output_mode, Some(OutputMode::Tool));
    assert!(params.tools.iter().any(|tool| {
        tool.name == "answer" && tool_metadata_kind(&tool.metadata) == Some(ToolKind::Output)
    }));
}
