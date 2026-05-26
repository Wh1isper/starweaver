#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_model::{FunctionModel, ModelMessage, ModelRequestPart, ModelResponse, TestModel};
use starweaver_runtime::{
    Agent, AgentError, AgentRunState, DynamicInstructionError, FunctionDynamicInstruction,
    StaticCapabilityBundle,
};

#[tokio::test]
async fn dynamic_instruction_is_injected_on_first_request() {
    let model = FunctionModel::new(|messages, _settings, _info| {
        let has_dynamic = messages.iter().any(|message| matches!(message, ModelMessage::Request(request) if request.parts.iter().any(|part| matches!(part, ModelRequestPart::Instruction { text, .. } if text == "Dynamic run step 0"))));
        assert!(has_dynamic);
        Ok(ModelResponse::text("ok"))
    });
    let instruction = FunctionDynamicInstruction::new(|state: AgentRunState| async move {
        Ok(format!("Dynamic run step {}", state.run_step))
    });

    let result = Agent::new(Arc::new(model))
        .with_dynamic_instruction(Arc::new(instruction))
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
}

#[tokio::test]
async fn dynamic_instruction_error_fails_run() {
    let instruction = FunctionDynamicInstruction::new(|_state: AgentRunState| async move {
        Err(DynamicInstructionError::failed("missing runtime data"))
    });

    let error = Agent::new(Arc::new(TestModel::with_text("ok")))
        .with_dynamic_instruction(Arc::new(instruction))
        .run("hello")
        .await
        .unwrap_err();

    assert!(
        matches!(error, AgentError::DynamicInstruction(message) if message == "missing runtime data")
    );
}

#[tokio::test]
async fn capability_bundle_can_contribute_dynamic_instruction() {
    let model = FunctionModel::new(|messages, _settings, _info| {
        let has_dynamic = messages.iter().any(|message| matches!(message, ModelMessage::Request(request) if request.parts.iter().any(|part| matches!(part, ModelRequestPart::Instruction { text, .. } if text == "Bundle dynamic instruction"))));
        assert!(has_dynamic);
        Ok(ModelResponse::text("ok"))
    });
    let instruction = FunctionDynamicInstruction::new(|_state: AgentRunState| async move {
        Ok("Bundle dynamic instruction".to_string())
    });
    let bundle =
        StaticCapabilityBundle::new("dynamic").with_dynamic_instruction(Arc::new(instruction));

    let result = Agent::new(Arc::new(model))
        .with_capability_bundle(&bundle)
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
}
