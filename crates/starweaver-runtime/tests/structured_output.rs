#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use starweaver_model::{
    ModelAdapter, ModelError, ModelMessage, ModelProfile, ModelRequestContext,
    ModelRequestParameters, ModelResponse, ModelSettings, ProtocolFamily,
};
use starweaver_runtime::{
    Agent, AgentError, AgentRuntimePolicy, FunctionOutputValidator, OutputSchema,
    OutputValidationError, OutputValue,
};

#[derive(Clone)]
struct ScriptedModel {
    responses: Arc<Mutex<Vec<ModelResponse>>>,
    captured_messages: Arc<Mutex<Vec<Vec<ModelMessage>>>>,
    captured_params: Arc<Mutex<Vec<ModelRequestParameters>>>,
}

impl ScriptedModel {
    fn new(responses: Vec<ModelResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into_iter().rev().collect())),
            captured_messages: Arc::new(Mutex::new(Vec::new())),
            captured_params: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl ModelAdapter for ScriptedModel {
    fn model_name(&self) -> &'static str {
        "scripted"
    }

    fn provider_name(&self) -> Option<&'static str> {
        Some("test")
    }

    fn profile(&self) -> &ModelProfile {
        static PROFILE: LazyLock<ModelProfile> =
            LazyLock::new(|| ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions));
        &PROFILE
    }

    fn default_settings(&self) -> Option<&ModelSettings> {
        None
    }

    async fn request(
        &self,
        messages: Vec<ModelMessage>,
        _settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        self.captured_messages.lock().unwrap().push(messages);
        self.captured_params.lock().unwrap().push(params);
        self.responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| ModelError::Transport("script exhausted".to_string()))
    }
}

fn answer_schema() -> OutputSchema {
    OutputSchema::new(
        "answer",
        serde_json::json!({
            "type": "object",
            "properties": {
                "answer": {"type": "string"}
            },
            "required": ["answer"]
        }),
    )
}

fn strict_nested_answer_schema() -> OutputSchema {
    OutputSchema::new(
        "answer",
        serde_json::json!({
            "type": "object",
            "properties": {
                "answer": {"type": "string", "enum": ["Paris"]},
                "details": {
                    "type": "object",
                    "properties": {
                        "confidence": {"type": "number", "minimum": 0.0, "maximum": 1.0},
                        "tags": {
                            "type": "array",
                            "items": {"type": "string", "enum": ["capital", "france"]},
                            "minItems": 1
                        }
                    },
                    "required": ["confidence", "tags"],
                    "additionalProperties": false
                }
            },
            "required": ["answer", "details"],
            "additionalProperties": false
        }),
    )
}

#[tokio::test]
async fn structured_output_schema_is_passed_to_model_params() {
    let model = Arc::new(ScriptedModel::new(vec![ModelResponse::text(
        r#"{"answer":"Paris"}"#,
    )]));

    let result = Agent::new(model.clone())
        .with_output_schema(answer_schema())
        .run("return json")
        .await
        .unwrap();

    assert_eq!(result.output, r#"{"answer":"Paris"}"#);
    assert_eq!(result.structured_output.unwrap()["answer"], "Paris");
    let params = model.captured_params.lock().unwrap()[0].clone();
    let schema = params.output_schema.unwrap();
    assert_eq!(schema["name"], "answer");
    assert_eq!(schema["schema"]["type"], "object");
}

#[tokio::test]
async fn invalid_json_output_retries_and_accepts_next_response() {
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse::text("plain text"),
        ModelResponse::text(r#"{"answer":"Paris"}"#),
    ]));

    let result = Agent::new(model.clone())
        .with_output_schema(answer_schema())
        .with_policy(AgentRuntimePolicy {
            output_retries: 1,
            ..AgentRuntimePolicy::default()
        })
        .run("return json")
        .await
        .unwrap();

    assert_eq!(result.structured_output.unwrap()["answer"], "Paris");
    assert_eq!(model.captured_messages.lock().unwrap().len(), 2);
    let second_request = model.captured_messages.lock().unwrap()[1]
        .last()
        .cloned()
        .unwrap();
    assert!(format!("{second_request:?}").contains("RetryPrompt"));
    assert!(format!("{second_request:?}").contains("expected value"));
}

#[tokio::test]
async fn full_json_schema_validation_retries_nested_schema_failures() {
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse::text(
            r#"{"answer":"Berlin","details":{"confidence":1.2,"tags":["city"],"extra":true}}"#,
        ),
        ModelResponse::text(
            r#"{"answer":"Paris","details":{"confidence":0.9,"tags":["capital","france"]}}"#,
        ),
    ]));

    let result = Agent::new(model.clone())
        .with_output_schema(strict_nested_answer_schema())
        .with_policy(AgentRuntimePolicy {
            output_retries: 1,
            ..AgentRuntimePolicy::default()
        })
        .run("return strict json")
        .await
        .unwrap();

    let structured = result.structured_output.unwrap();
    assert_eq!(structured["answer"], "Paris");
    assert_eq!(structured["details"]["tags"][0], "capital");
    assert_eq!(model.captured_messages.lock().unwrap().len(), 2);
    let retry_request = model.captured_messages.lock().unwrap()[1]
        .last()
        .cloned()
        .unwrap();
    assert!(format!("{retry_request:?}").contains("RetryPrompt"));
}

#[tokio::test]
async fn invalid_structured_output_reports_retry_limit() {
    let model = Arc::new(ScriptedModel::new(vec![ModelResponse::text("plain text")]));

    let error = Agent::new(model)
        .with_output_schema(answer_schema())
        .with_policy(AgentRuntimePolicy {
            output_retries: 0,
            ..AgentRuntimePolicy::default()
        })
        .run("return json")
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        AgentError::OutputRetryLimitExceeded { retries: 0 }
    ));
}

#[tokio::test]
async fn output_validator_can_request_retry() {
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse::text(r#"{"answer":"London"}"#),
        ModelResponse::text(r#"{"answer":"Paris"}"#),
    ]));
    let validator = FunctionOutputValidator::new(
        |_state: &mut starweaver_runtime::AgentRunState, output: &OutputValue| {
            let answer = output
                .as_json()
                .and_then(|value| value.get("answer"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            async move {
                if answer == "Paris" {
                    Ok(())
                } else {
                    Err(OutputValidationError::retry("answer must be Paris"))
                }
            }
        },
    );

    let result = Agent::new(model.clone())
        .with_output_schema(answer_schema())
        .with_output_validator(Arc::new(validator))
        .with_policy(AgentRuntimePolicy {
            output_retries: 1,
            ..AgentRuntimePolicy::default()
        })
        .run("return json")
        .await
        .unwrap();

    assert_eq!(result.structured_output.unwrap()["answer"], "Paris");
    assert_eq!(model.captured_messages.lock().unwrap().len(), 2);
}
