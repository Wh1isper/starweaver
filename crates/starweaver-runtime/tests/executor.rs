//! Durable executor tests.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use starweaver_context::AgentContext;
use starweaver_model::{
    FunctionModel, ModelMessage, ModelResponse, ModelResponsePart, ModelSettings, TestModel,
};
use starweaver_runtime::{
    Agent, AgentCapability, AgentCheckpoint, AgentError, AgentExecutionDecision,
    AgentExecutionNode, AgentExecutor, AgentExecutorError, AgentStreamEvent, CapabilityError,
    CapabilityResult,
};
use starweaver_usage::Usage;

#[derive(Default)]
struct RecordingExecutor {
    nodes: Mutex<Vec<AgentExecutionNode>>,
    checkpoints: Mutex<Vec<AgentCheckpoint>>,
    suspend_at: Option<AgentExecutionNode>,
}

#[async_trait]
impl AgentExecutor for RecordingExecutor {
    async fn checkpoint(
        &self,
        checkpoint: AgentCheckpoint,
    ) -> Result<AgentExecutionDecision, AgentExecutorError> {
        if let Ok(mut nodes) = self.nodes.lock() {
            nodes.push(checkpoint.node);
        }
        if let Ok(mut checkpoints) = self.checkpoints.lock() {
            checkpoints.push(checkpoint.clone());
        }
        if Some(checkpoint.node) == self.suspend_at {
            return Ok(AgentExecutionDecision::Suspend {
                reason: "test suspension".to_string(),
            });
        }
        Ok(AgentExecutionDecision::Continue)
    }
}

#[tokio::test]
async fn executor_records_fine_grained_checkpoints() {
    let executor = Arc::new(RecordingExecutor::default());
    let result = Agent::new(Arc::new(TestModel::with_responses(vec![
        ModelResponse::text("done"),
    ])))
    .with_executor(executor.clone())
    .run("hello")
    .await;

    assert!(result.is_ok());
    let nodes = executor
        .nodes
        .lock()
        .map_or_else(|_| Vec::new(), |nodes| nodes.clone());
    assert_eq!(
        nodes,
        vec![
            AgentExecutionNode::RunStart,
            AgentExecutionNode::PrepareModelRequest,
            AgentExecutionNode::BeforeModelRequest,
            AgentExecutionNode::ModelResponse,
            AgentExecutionNode::ValidateOutput,
            AgentExecutionNode::RunComplete,
        ]
    );
}

struct SkipModelAtRequest;

#[async_trait]
impl AgentCapability for SkipModelAtRequest {
    async fn before_model_request(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        _request: &mut starweaver_model::ModelRequest,
        _settings: &mut Option<ModelSettings>,
    ) -> CapabilityResult<()> {
        Err(CapabilityError::SkipModelRequest(Box::new(
            ModelResponse::text("synthetic"),
        )))
    }
}

#[tokio::test]
async fn skipped_model_transition_preserves_request_checkpoint_and_stream_order() {
    let model_calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&model_calls);
    let model = FunctionModel::new(move |_messages, _settings, _info| {
        observed_calls.fetch_add(1, Ordering::SeqCst);
        Ok(ModelResponse::text("provider"))
    });
    let executor = Arc::new(RecordingExecutor::default());
    let mut context = AgentContext::default();
    let mut events = Vec::new();

    let result = Agent::new(Arc::new(model))
        .with_executor(executor.clone())
        .with_capability(Arc::new(SkipModelAtRequest))
        .run_with_context_and_stream_events("hello", &mut context, &mut events)
        .await
        .unwrap();

    assert_eq!(result.output, "synthetic");
    assert_eq!(model_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        executor.nodes.lock().unwrap().as_slice(),
        [
            AgentExecutionNode::RunStart,
            AgentExecutionNode::PrepareModelRequest,
            AgentExecutionNode::BeforeModelRequest,
            AgentExecutionNode::ModelResponse,
            AgentExecutionNode::ValidateOutput,
            AgentExecutionNode::RunComplete,
        ]
    );

    let event_index = |predicate: fn(&AgentStreamEvent) -> bool| {
        events
            .iter()
            .position(|record| predicate(&record.event))
            .expect("stream phase event")
    };
    let prepare_start = event_index(|event| {
        matches!(
            event,
            AgentStreamEvent::NodeStart {
                node: AgentExecutionNode::PrepareModelRequest,
                ..
            }
        )
    });
    let prepare_complete = event_index(|event| {
        matches!(
            event,
            AgentStreamEvent::NodeComplete {
                node: AgentExecutionNode::PrepareModelRequest,
                ..
            }
        )
    });
    let request = event_index(|event| matches!(event, AgentStreamEvent::ModelRequest { .. }));
    let before_start = event_index(|event| {
        matches!(
            event,
            AgentStreamEvent::NodeStart {
                node: AgentExecutionNode::BeforeModelRequest,
                ..
            }
        )
    });
    let before_complete = event_index(|event| {
        matches!(
            event,
            AgentStreamEvent::NodeComplete {
                node: AgentExecutionNode::BeforeModelRequest,
                ..
            }
        )
    });
    let response = event_index(|event| matches!(event, AgentStreamEvent::ModelResponse { .. }));
    let terminal = event_index(|event| matches!(event, AgentStreamEvent::RunComplete { .. }));
    assert!(
        prepare_start < prepare_complete
            && prepare_complete < request
            && request < before_start
            && before_start < before_complete
            && before_complete < response
            && response < terminal
    );
}

struct MutateSyntheticResponseAfterCheckpoint;

#[async_trait]
impl AgentCapability for MutateSyntheticResponseAfterCheckpoint {
    async fn before_model_request(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        _request: &mut starweaver_model::ModelRequest,
        _settings: &mut Option<ModelSettings>,
    ) -> CapabilityResult<()> {
        Err(CapabilityError::SkipModelRequest(Box::new(ModelResponse {
            usage: Usage {
                requests: 1,
                input_tokens: 2,
                output_tokens: 3,
                total_tokens: 5,
                ..Usage::default()
            },
            ..ModelResponse::text("pre-hook")
        })))
    }

    async fn after_model_response(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        response: &mut ModelResponse,
    ) -> CapabilityResult<()> {
        response.parts = vec![ModelResponsePart::Text {
            text: "post-hook".to_string(),
        }];
        response.usage = Usage {
            requests: 99,
            input_tokens: 99,
            output_tokens: 99,
            total_tokens: 297,
            ..Usage::default()
        };
        Ok(())
    }
}

#[tokio::test]
async fn classify_response_preserves_pre_hook_evidence_and_commits_post_hook_history() {
    let model_calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&model_calls);
    let model = FunctionModel::new(move |_messages, _settings, _info| {
        observed_calls.fetch_add(1, Ordering::SeqCst);
        Ok(ModelResponse::text("provider"))
    });
    let executor = Arc::new(RecordingExecutor::default());
    let mut context = AgentContext::default();
    let mut events = Vec::new();

    let result = Agent::new(Arc::new(model))
        .with_executor(executor.clone())
        .with_capability(Arc::new(MutateSyntheticResponseAfterCheckpoint))
        .run_with_context_and_stream_events("hello", &mut context, &mut events)
        .await
        .unwrap();

    assert_eq!(model_calls.load(Ordering::SeqCst), 0);
    assert_eq!(result.output, "post-hook");
    assert_eq!(result.state.usage.total_tokens, 5);
    assert!(matches!(
        result.messages.last(),
        Some(ModelMessage::Response(response)) if response.text_output() == "post-hook"
            && response.usage.total_tokens == 297
    ));

    let raw_response = events.iter().find_map(|record| match &record.event {
        AgentStreamEvent::ModelResponse { response, .. } => Some(response),
        _ => None,
    });
    assert!(matches!(
        raw_response,
        Some(response) if response.text_output() == "pre-hook"
            && response.usage.total_tokens == 5
    ));

    let checkpoints = executor.checkpoints.lock().unwrap().clone();
    let model_response_checkpoint = checkpoints
        .iter()
        .find(|checkpoint| checkpoint.node == AgentExecutionNode::ModelResponse)
        .expect("model response checkpoint");
    assert_eq!(model_response_checkpoint.state.usage.total_tokens, 5);
    assert!(matches!(
        model_response_checkpoint.state.latest_response.as_ref(),
        Some(response) if response.text_output() == "pre-hook"
            && response.usage.total_tokens == 5
    ));

    let validate_output_checkpoint = checkpoints
        .iter()
        .find(|checkpoint| checkpoint.node == AgentExecutionNode::ValidateOutput)
        .expect("validate output checkpoint");
    assert_eq!(validate_output_checkpoint.state.usage.total_tokens, 5);
    assert!(matches!(
        validate_output_checkpoint.state.latest_response.as_ref(),
        Some(response) if response.text_output() == "post-hook"
            && response.usage.total_tokens == 297
    ));
}

#[tokio::test]
async fn executor_checkpoints_record_latest_stream_cursor() {
    let executor = Arc::new(RecordingExecutor::default());
    let mut context = AgentContext::default();
    let mut events = Vec::new();

    Agent::new(Arc::new(TestModel::with_responses(vec![
        ModelResponse::text("done"),
    ])))
    .with_executor(executor.clone())
    .run_with_context_and_stream_events("hello", &mut context, &mut events)
    .await
    .unwrap();

    let checkpoints = executor
        .checkpoints
        .lock()
        .map_or_else(|_| Vec::new(), |checkpoints| checkpoints.clone());
    assert!(!checkpoints.is_empty());
    for checkpoint in checkpoints {
        let cursor = checkpoint.resume.cursor.stream_cursor.unwrap();
        let event = events.get(cursor).unwrap();
        assert_eq!(event.sequence, cursor);
        assert!(matches!(
            &event.event,
            AgentStreamEvent::NodeStart { node, step, .. }
                if *node == checkpoint.node && *step == checkpoint.run_step
        ));
    }
}

#[tokio::test]
async fn executor_can_suspend_at_checkpoint() {
    let executor = Arc::new(RecordingExecutor {
        suspend_at: Some(AgentExecutionNode::BeforeModelRequest),
        ..RecordingExecutor::default()
    });

    let error = Agent::new(Arc::new(TestModel::with_text("done")))
        .with_executor(executor)
        .run("hello")
        .await;

    assert!(matches!(
        error,
        Err(AgentError::ExecutionSuspended {
            node: AgentExecutionNode::BeforeModelRequest,
            ..
        })
    ));
}

struct FailingExecutor;

#[async_trait]
impl AgentExecutor for FailingExecutor {
    async fn checkpoint(
        &self,
        checkpoint: AgentCheckpoint,
    ) -> Result<AgentExecutionDecision, AgentExecutorError> {
        if checkpoint.node == AgentExecutionNode::BeforeModelRequest {
            return Err(AgentExecutorError::Failed(
                "checkpoint backend unavailable".to_string(),
            ));
        }
        Ok(AgentExecutionDecision::Continue)
    }
}

#[tokio::test]
async fn executor_failure_uses_fallback_terminal_cleanup() {
    let mut context = AgentContext::default();
    let mut events = Vec::new();

    let result = Agent::new(Arc::new(TestModel::with_text("never reached")))
        .with_executor(Arc::new(FailingExecutor))
        .run_with_context_and_stream_events("hello", &mut context, &mut events)
        .await;

    assert!(matches!(result, Err(AgentError::Executor(_))));
    assert!(!context.runtime.lifecycle.entered);
    assert!(context.ended_at.is_some());
    assert!(context.events.events().iter().any(|event| {
        event.kind == "run_failed" && event.payload["error_kind"] == "executor_error"
    }));
    assert_eq!(
        events
            .iter()
            .filter(|record| matches!(record.event, AgentStreamEvent::RunFailed { .. }))
            .count(),
        1
    );
    assert!(matches!(
        events.last().map(|record| &record.event),
        Some(AgentStreamEvent::RunFailed { error_kind, .. }) if error_kind == "executor_error"
    ));
}

#[test]
fn context_owned_checkpoint_contracts_keep_runtime_nominal_identity() {
    let context_state = starweaver_context::AgentRunState::new(
        starweaver_core::RunId::from_string("run-owner"),
        starweaver_core::ConversationId::from_string("conv-owner"),
    );
    let runtime_state: starweaver_runtime::AgentRunState = context_state;
    let module_state: starweaver_runtime::run::AgentRunState = runtime_state;
    let context_checkpoint =
        starweaver_context::AgentCheckpoint::new(AgentExecutionNode::RunStart, &module_state);
    let runtime_checkpoint: starweaver_runtime::AgentCheckpoint = context_checkpoint;
    let module_checkpoint: starweaver_runtime::executor::AgentCheckpoint = runtime_checkpoint;
    let context_executor: &dyn starweaver_context::AgentExecutor =
        &starweaver_runtime::DirectAgentExecutor;
    let runtime_executor: &dyn starweaver_runtime::AgentExecutor = context_executor;

    assert_eq!(module_checkpoint.state.run_id.as_str(), "run-owner");
    let _ = runtime_executor;
}

#[tokio::test]
async fn executor_checkpoint_has_stable_identifier_and_serializes() {
    let state = starweaver_runtime::AgentRunState::new(
        starweaver_core::RunId::from_string("run-checkpoint"),
        starweaver_core::ConversationId::from_string("conv-checkpoint"),
    );

    let checkpoint = AgentCheckpoint::new(AgentExecutionNode::RunStart, &state);
    let encoded = serde_json::to_value(&checkpoint).unwrap();

    assert!(checkpoint.checkpoint_id.as_str().starts_with("ckpt_"));
    assert_eq!(encoded["checkpoint_id"], checkpoint.checkpoint_id.as_str());
    assert_eq!(encoded["node"], "run_start");
    assert_eq!(encoded["resume"]["node"], "run_start");
    assert_eq!(encoded["resume"]["cursor"]["message_cursor"], 0);
}

#[tokio::test]
async fn capability_hooks_observe_executor_checkpoints() {
    let observed = Arc::new(Mutex::new(Vec::<String>::new()));
    let hook = Arc::new(CheckpointRecorder {
        nodes: observed.clone(),
    });

    let result = Agent::new(Arc::new(TestModel::with_responses(vec![
        ModelResponse::text("done"),
    ])))
    .with_capability(hook)
    .run("hello")
    .await;

    assert!(result.is_ok());
    assert_eq!(
        observed.lock().unwrap().as_slice(),
        [
            "run_start",
            "prepare_model_request",
            "before_model_request",
            "model_response",
            "validate_output",
            "run_complete",
        ]
    );
}

struct CheckpointRecorder {
    nodes: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl AgentCapability for CheckpointRecorder {
    async fn on_checkpoint(
        &self,
        _state: &starweaver_runtime::AgentRunState,
        checkpoint: &AgentCheckpoint,
    ) -> CapabilityResult<()> {
        self.nodes.lock().unwrap().push(
            serde_json::to_value(checkpoint.node)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string(),
        );
        assert!(checkpoint.checkpoint_id.as_str().starts_with("ckpt_"));
        Ok(())
    }
}
