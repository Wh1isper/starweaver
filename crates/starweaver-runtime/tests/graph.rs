#![allow(missing_docs, clippy::unwrap_used)]

use starweaver_core::{ConversationId, RunId};
use starweaver_model::{message::ToolCallPart, ModelResponse, ModelResponsePart, ToolReturnPart};
use starweaver_runtime::{next_node, AgentNode, AgentRunState, RunStatus};

fn state() -> AgentRunState {
    AgentRunState::new(
        RunId::from_string("run_test"),
        ConversationId::from_string("conv_test"),
    )
}

#[test]
fn transitions_final_text_path() {
    let mut state = state();
    assert_eq!(
        next_node(AgentNode::StartRun, &state, 8).unwrap().next,
        AgentNode::PrepareRequest
    );
    assert_eq!(
        next_node(AgentNode::PrepareRequest, &state, 8)
            .unwrap()
            .next,
        AgentNode::DrainMessages
    );
    assert_eq!(
        next_node(AgentNode::DrainMessages, &state, 8).unwrap().next,
        AgentNode::ModelRequest
    );

    state.apply_model_response(ModelResponse::text("done"));
    assert_eq!(
        next_node(AgentNode::ModelRequest, &state, 8).unwrap().next,
        AgentNode::HandleResponse
    );

    state.output = Some("done".to_string());
    assert_eq!(
        next_node(AgentNode::HandleResponse, &state, 8)
            .unwrap()
            .next,
        AgentNode::FinalizeRun
    );
    assert_eq!(
        next_node(AgentNode::FinalizeRun, &state, 8).unwrap().next,
        AgentNode::DrainIdleMessages
    );
    assert_eq!(
        next_node(AgentNode::DrainIdleMessages, &state, 8)
            .unwrap()
            .next,
        AgentNode::Complete
    );
}

#[test]
fn transitions_tool_call_path() {
    let mut state = state();
    state.apply_model_response(ModelResponse {
        parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
            id: "call_1".to_string(),
            name: "lookup".to_string(),
            arguments: serde_json::json!({"query": "x"}),
        })],
        ..ModelResponse::text("")
    });
    state.pending_tool_calls = state.latest_response.as_ref().unwrap().tool_calls();

    assert_eq!(
        next_node(AgentNode::HandleResponse, &state, 8)
            .unwrap()
            .next,
        AgentNode::ExecuteTools
    );

    state.pending_tool_returns.push(ToolReturnPart {
        tool_call_id: "call_1".to_string(),
        name: "lookup".to_string(),
        content: serde_json::json!({"value": "x"}),
        is_error: false,
        metadata: serde_json::Map::new(),
    });
    assert_eq!(
        next_node(AgentNode::ExecuteTools, &state, 8).unwrap().next,
        AgentNode::PrepareRequest
    );
}

#[test]
fn transitions_retry_path() {
    let mut state = state();
    state.pending_tool_returns.push(ToolReturnPart {
        tool_call_id: "retry".to_string(),
        name: "output_validation".to_string(),
        content: serde_json::json!({"error": "invalid"}),
        is_error: true,
        metadata: serde_json::Map::new(),
    });

    assert_eq!(
        next_node(AgentNode::HandleResponse, &state, 8)
            .unwrap()
            .next,
        AgentNode::PrepareRequest
    );
}

#[test]
fn transitions_idle_message_redirect_path() {
    let mut state = state();
    state.output = Some("done".to_string());
    state.idle_messages.push("continue".to_string());

    assert_eq!(
        next_node(AgentNode::DrainIdleMessages, &state, 8)
            .unwrap()
            .next,
        AgentNode::PrepareRequest
    );
}

#[test]
fn transitions_max_step_path_to_finalize() {
    let mut state = state();
    state.run_step = 8;
    state.status = RunStatus::Running;

    assert_eq!(
        next_node(AgentNode::PrepareRequest, &state, 8)
            .unwrap()
            .next,
        AgentNode::FinalizeRun
    );
}
