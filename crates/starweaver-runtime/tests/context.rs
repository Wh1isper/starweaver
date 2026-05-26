#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use async_trait::async_trait;
use starweaver_context::{AgentContext, AgentId, BusMessage};
use starweaver_model::{
    ModelAdapter, ModelError, ModelMessage, ModelProfile, ModelRequestContext,
    ModelRequestParameters, ModelResponse, ModelSettings, ProtocolFamily,
};
use starweaver_runtime::Agent;

struct ContextModel;

#[async_trait]
impl ModelAdapter for ContextModel {
    fn model_name(&self) -> &'static str {
        "context-model"
    }

    fn provider_name(&self) -> Option<&'static str> {
        Some("test")
    }

    fn profile(&self) -> &ModelProfile {
        static PROFILE: ModelProfile =
            ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions);
        &PROFILE
    }

    fn default_settings(&self) -> Option<&ModelSettings> {
        None
    }

    async fn request(
        &self,
        _messages: Vec<ModelMessage>,
        _settings: Option<ModelSettings>,
        _params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        Ok(ModelResponse::text("context output"))
    }
}

#[tokio::test]
async fn agent_run_updates_context_history_usage_and_events() {
    let mut context = AgentContext::new(AgentId::from_string("main"));
    context.enqueue_message(BusMessage::new(
        "steering",
        serde_json::json!({"text": "keep going"}),
    ));

    let result = Agent::new(Arc::new(ContextModel))
        .run_with_context("hello", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "context output");
    assert_eq!(context.message_history.len(), 2);
    assert_eq!(context.events.events().len(), 2);
    assert_eq!(context.events.events()[0].kind, "run_start");
    assert_eq!(context.events.events()[1].kind, "run_complete");
    assert_eq!(context.messages.len(), 1);
    let exported = context.export_state();
    assert_eq!(exported.message_history.len(), 2);
    assert_eq!(exported.agent_id.as_str(), "main");
}

#[tokio::test]
async fn agent_can_resume_from_exported_context_state() {
    let mut context = AgentContext::new(AgentId::from_string("main"));
    Agent::new(Arc::new(ContextModel))
        .run_with_context("first", &mut context)
        .await
        .unwrap();

    let state = context.export_state();
    let mut restored = AgentContext::from_state(state);
    let result = Agent::new(Arc::new(ContextModel))
        .run_with_context("second", &mut restored)
        .await
        .unwrap();

    assert_eq!(result.history_len, 2);
    assert_eq!(result.new_messages().len(), 2);
    assert_eq!(restored.message_history.len(), 4);
}
