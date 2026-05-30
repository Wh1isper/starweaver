#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use async_trait::async_trait;
use starweaver_model::{ModelResponse, TestModel};
use starweaver_runtime::{
    Agent, AgentRunState, AgentRuntimePolicy, OutputPolicy, OutputSchema, OutputValidationError,
    OutputValidationResult, OutputValidator, OutputValue,
};

struct RequiresOk;

#[async_trait]
impl OutputValidator for RequiresOk {
    async fn validate(
        &self,
        _state: &mut AgentRunState,
        output: &OutputValue,
    ) -> OutputValidationResult<()> {
        let value = output.parse::<serde_json::Value>()?;
        if value["answer"] == "ok" {
            Ok(())
        } else {
            Err(OutputValidationError::retry("answer must be ok"))
        }
    }
}

#[tokio::test]
async fn output_policy_applies_schema_validators_and_retry_budget() {
    let policy = OutputPolicy::structured(OutputSchema::new(
        "answer",
        serde_json::json!({"type": "object", "required": ["answer"]}),
    ))
    .with_validator(Arc::new(RequiresOk))
    .with_retries(2);

    let result = Agent::new(Arc::new(TestModel::with_responses(vec![
        ModelResponse::text(r#"{"answer":"bad"}"#),
        ModelResponse::text(r#"{"answer":"ok"}"#),
    ])))
    .with_policy(AgentRuntimePolicy {
        output_retries: 0,
        ..AgentRuntimePolicy::default()
    })
    .with_output_policy(policy)
    .run("answer")
    .await
    .unwrap();

    assert_eq!(result.structured_output.unwrap()["answer"], "ok");
}
