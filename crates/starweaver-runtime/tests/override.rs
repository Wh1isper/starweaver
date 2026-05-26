#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_model::{FunctionModel, ModelResponse, TestModel};
use starweaver_runtime::{Agent, OutputSchema};

#[tokio::test]
async fn agent_override_replaces_model_for_scoped_test_run() {
    let production = Arc::new(TestModel::with_text("production"));
    let test = Arc::new(TestModel::with_text("test"));
    let agent = Agent::new(production.clone());

    let overridden = agent.override_config().model(test.clone()).build();

    let test_result = overridden.run("hello").await.unwrap();
    let production_result = agent.run("hello").await.unwrap();

    assert_eq!(test_result.output, "test");
    assert_eq!(production_result.output, "production");
    assert_eq!(test.captured_messages().len(), 1);
    assert_eq!(production.captured_messages().len(), 1);
}

#[tokio::test]
async fn agent_override_replaces_output_schema() {
    let model = Arc::new(TestModel::with_text(r#"{"answer":"ok"}"#));
    let agent = Agent::new(model.clone());
    let schema = OutputSchema::new(
        "answer",
        serde_json::json!({"type": "object", "required": ["answer"]}),
    );

    let result = agent
        .override_config()
        .output_schema(Some(schema))
        .build()
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.structured_output.unwrap()["answer"], "ok");
    assert_eq!(
        model.captured_params()[0].output_schema.as_ref().unwrap()["name"],
        "answer"
    );
}

#[tokio::test]
async fn function_model_can_drive_agent_behavior() {
    let model = FunctionModel::new(|messages, _settings, _info| {
        let text = starweaver_model::latest_user_text(&messages).unwrap();
        Ok(ModelResponse::text(format!("seen: {text}")))
    });
    let agent = Agent::new(Arc::new(model));

    let result = agent.run("hello").await.unwrap();

    assert_eq!(result.output, "seen: hello");
}
