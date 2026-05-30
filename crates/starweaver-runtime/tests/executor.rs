//! Durable executor tests.

#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use starweaver_model::{ModelResponse, TestModel};
use starweaver_runtime::{
    Agent, AgentCapability, AgentCheckpoint, AgentError, AgentExecutionDecision,
    AgentExecutionNode, AgentExecutor, AgentExecutorError, CapabilityResult,
};

#[derive(Default)]
struct RecordingExecutor {
    nodes: Mutex<Vec<AgentExecutionNode>>,
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
