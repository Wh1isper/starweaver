#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{AgentBuilder, OutputPolicy, OutputSchema, TestModel};
use starweaver_model::ModelResponse;

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
