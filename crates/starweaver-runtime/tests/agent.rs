#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use starweaver_core::Usage;
use starweaver_model::{
    ModelAdapter, ModelError, ModelMessage, ModelRequestContext, ModelRequestParameters,
    ModelResponse, ModelResponsePart, ModelSettings,
};
use starweaver_runtime::{
    Agent, AgentCapability, AgentError, AgentRuntimePolicy, CapabilityError, CapabilityResult,
    RetryEventKind,
};

#[derive(Clone)]
struct ScriptedModel {
    responses: Arc<Mutex<Vec<ModelResponse>>>,
    captured: Arc<Mutex<Vec<Vec<ModelMessage>>>>,
}

impl ScriptedModel {
    fn new(responses: Vec<ModelResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into_iter().rev().collect())),
            captured: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl ModelAdapter for ScriptedModel {
    fn model_name(&self) -> &'static str {
        "test-model"
    }

    fn provider_name(&self) -> Option<&str> {
        Some("test")
    }

    fn profile(&self) -> &starweaver_model::ModelProfile {
        static PROFILE: starweaver_model::ModelProfile =
            starweaver_model::ModelProfile::for_protocol(
                starweaver_model::ProtocolFamily::OpenAiChatCompletions,
            );
        &PROFILE
    }

    fn default_settings(&self) -> Option<&ModelSettings> {
        None
    }

    async fn request(
        &self,
        messages: Vec<ModelMessage>,
        _settings: Option<ModelSettings>,
        _params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        self.captured.lock().unwrap().push(messages);
        self.responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| ModelError::Transport("script exhausted".to_string()))
    }
}

#[tokio::test]
async fn bare_agent_runs_prompt_and_returns_output() {
    let model = Arc::new(ScriptedModel::new(vec![ModelResponse::text("Paris")]));
    let agent = Agent::new(model.clone()).with_instruction("Answer concisely.");

    let result = agent.run("What is the capital of France?").await.unwrap();

    assert_eq!(result.output, "Paris");
    assert_eq!(result.state.run_step, 1);
    assert_eq!(result.messages.len(), 2);
    let captured_len = model.captured.lock().unwrap().len();
    assert_eq!(captured_len, 1);
}

#[tokio::test]
async fn bare_agent_retries_output_validation() {
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse::text("bad"),
        ModelResponse::text("good"),
    ]));
    let agent = Agent::new(model.clone())
        .with_policy(AgentRuntimePolicy {
            max_steps: 4,
            output_retries: 1,
        })
        .with_capability(Arc::new(RequireGoodOutput));

    let result = agent.run("Produce good output").await.unwrap();

    assert_eq!(result.output, "good");
    assert_eq!(result.state.run_step, 2);
    let second_request_history = model.captured.lock().unwrap()[1].clone();
    let last_request = second_request_history.last().unwrap();
    assert!(format!("{last_request:?}").contains("RetryPrompt"));
}

#[tokio::test]
async fn bare_agent_enforces_output_retry_limit() {
    let model = Arc::new(ScriptedModel::new(vec![ModelResponse::text("bad")]));
    let agent = Agent::new(model)
        .with_policy(AgentRuntimePolicy {
            max_steps: 2,
            output_retries: 0,
        })
        .with_capability(Arc::new(RequireGoodOutput));

    let error = agent.run("Produce good output").await.unwrap_err();

    assert!(matches!(
        error,
        AgentError::OutputRetryLimitExceeded { retries: 0 }
    ));
}

#[tokio::test]
async fn capability_hooks_can_mutate_response_and_record_lifecycle() {
    let model = Arc::new(ScriptedModel::new(vec![ModelResponse {
        parts: vec![ModelResponsePart::Text {
            text: "raw".to_string(),
        }],
        usage: Usage {
            requests: 1,
            input_tokens: 1,
            output_tokens: 1,
            total_tokens: 2,
            tool_calls: 0,
        },
        model_name: None,
        provider: None,
        finish_reason: None,
        timestamp: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    }]));
    let hook = Arc::new(RewriteAndRecord::default());
    let agent = Agent::new(model).with_capability(hook.clone());

    let result = agent.run("Rewrite output").await.unwrap();

    assert_eq!(result.output, "rewritten");
    let latest_history_response = match result.messages.last() {
        Some(ModelMessage::Response(response)) => response,
        other => panic!("latest canonical message should be the mutated response, got {other:?}"),
    };
    assert_eq!(latest_history_response.text_output(), "rewritten");
    let Some(latest_response) = result.state.latest_response.as_ref() else {
        panic!("latest response should be recorded");
    };
    assert_eq!(latest_response.text_output(), "rewritten");
    assert_eq!(
        hook.events.lock().unwrap().as_slice(),
        ["start", "before", "after", "validate", "complete"]
    );
}

struct RequireGoodOutput;

#[async_trait]
impl AgentCapability for RequireGoodOutput {
    async fn validate_output(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        output: &str,
    ) -> CapabilityResult<()> {
        if output == "good" {
            Ok(())
        } else {
            Err(CapabilityError::ModelRetry(
                "Return exactly good".to_string(),
            ))
        }
    }
}

#[derive(Default)]
struct RewriteAndRecord {
    events: Mutex<Vec<&'static str>>,
}

#[async_trait]
impl AgentCapability for RewriteAndRecord {
    async fn on_run_start(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
    ) -> CapabilityResult<()> {
        self.events.lock().unwrap().push("start");
        Ok(())
    }

    async fn before_model_request(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        _request: &mut starweaver_model::ModelRequest,
        _settings: &mut Option<ModelSettings>,
    ) -> CapabilityResult<()> {
        self.events.lock().unwrap().push("before");
        Ok(())
    }

    async fn after_model_response(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        response: &mut ModelResponse,
    ) -> CapabilityResult<()> {
        self.events.lock().unwrap().push("after");
        response.parts = vec![ModelResponsePart::Text {
            text: "rewritten".to_string(),
        }];
        Ok(())
    }

    async fn validate_output(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        _output: &str,
    ) -> CapabilityResult<()> {
        self.events.lock().unwrap().push("validate");
        Ok(())
    }

    async fn on_run_complete(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
    ) -> CapabilityResult<()> {
        self.events.lock().unwrap().push("complete");
        Ok(())
    }
}

#[tokio::test]
async fn capability_hooks_observe_retry_and_output_validation_boundaries() {
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse::text("bad"),
        ModelResponse::text("good"),
    ]));
    let hook = Arc::new(RetryAndOutputBoundaryRecorder::default());
    let agent = Agent::new(model).with_capability(hook.clone());

    let result = agent.run("Produce accepted output").await.unwrap();

    assert_eq!(result.output, "good");
    assert_eq!(
        hook.events.lock().unwrap().as_slice(),
        [
            "before_output:bad",
            "retry:output:1:Return exactly good",
            "before_output:good",
            "after_output:good",
        ]
    );
}

#[derive(Default)]
struct RetryAndOutputBoundaryRecorder {
    events: Mutex<Vec<String>>,
}

#[async_trait]
impl AgentCapability for RetryAndOutputBoundaryRecorder {
    async fn before_output_validation(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        output: &str,
    ) -> CapabilityResult<()> {
        self.events
            .lock()
            .unwrap()
            .push(format!("before_output:{output}"));
        Ok(())
    }

    async fn validate_output(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        output: &str,
    ) -> CapabilityResult<()> {
        if output == "good" {
            Ok(())
        } else {
            Err(CapabilityError::ModelRetry(
                "Return exactly good".to_string(),
            ))
        }
    }

    async fn after_output_validation(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        output: &str,
    ) -> CapabilityResult<()> {
        self.events
            .lock()
            .unwrap()
            .push(format!("after_output:{output}"));
        Ok(())
    }

    async fn on_retry(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        kind: RetryEventKind,
        retries: usize,
        message: &str,
    ) -> CapabilityResult<()> {
        self.events.lock().unwrap().push(format!(
            "retry:{}:{retries}:{message}",
            match kind {
                RetryEventKind::Output => "output",
                RetryEventKind::Tool => "tool",
            }
        ));
        Ok(())
    }
}
