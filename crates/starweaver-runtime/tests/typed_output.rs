#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use serde::Deserialize;
use starweaver_model::TestModel;
use starweaver_runtime::{Agent, AgentError, OutputSchema, OutputValue};

#[derive(Debug, Deserialize, PartialEq)]
struct Answer {
    answer: String,
}

#[tokio::test]
async fn agent_result_parses_structured_output_into_type() {
    let agent = Agent::new(Arc::new(TestModel::with_json(&serde_json::json!({
        "answer": "Paris"
    }))))
    .with_output_schema(OutputSchema::new(
        "answer",
        serde_json::json!({
            "type": "object",
            "required": ["answer"]
        }),
    ));

    let result = agent.run("answer").await.unwrap();
    let parsed: Answer = result.structured().unwrap();

    assert_eq!(parsed.answer, "Paris");
}

#[tokio::test]
async fn agent_result_reports_missing_structured_output() {
    let result = Agent::new(Arc::new(TestModel::with_text("plain")))
        .run("answer")
        .await
        .unwrap();

    let error = result.structured::<Answer>().unwrap_err();

    assert!(
        matches!(error, AgentError::StructuredOutput(message) if message == "missing structured output")
    );
}

#[test]
fn output_value_parses_text_or_json_into_type() {
    let from_text: Answer = OutputValue::Text(r#"{"answer":"Paris"}"#.to_string())
        .parse()
        .unwrap();
    let from_json: Answer = OutputValue::Json(serde_json::json!({"answer": "Paris"}))
        .parse()
        .unwrap();

    assert_eq!(from_text, from_json);
}
