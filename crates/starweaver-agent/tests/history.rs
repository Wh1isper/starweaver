#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use async_trait::async_trait;
use starweaver_agent::{
    AgentBuilder, AgentCapability, AgentContext, AgentRunState, CapabilityResult, TestModel,
};
use starweaver_model::ModelMessage;

struct PassthroughMessagesCapability;

#[async_trait]
impl AgentCapability for PassthroughMessagesCapability {
    async fn prepare_model_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut AgentContext,
        messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        Ok(messages)
    }
}

#[tokio::test]
async fn facade_uses_capability_for_model_message_preparation() {
    let result = AgentBuilder::new(Arc::new(TestModel::with_text("ok")))
        .instruction("System policy")
        .capability(Arc::new(PassthroughMessagesCapability))
        .build()
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
}
