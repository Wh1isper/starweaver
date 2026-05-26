//! Durable executor tests.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use starweaver_model::{ModelResponse, TestModel};
use starweaver_runtime::{
    Agent, AgentCheckpoint, AgentError, AgentExecutionDecision, AgentExecutionNode, AgentExecutor,
    AgentExecutorError,
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
