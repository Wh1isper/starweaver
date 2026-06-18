#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Map};
use starweaver_core::{ConversationId, RunId};
use starweaver_model::{
    ContentPart, FunctionModel, HookedModel, ModelAdapter, ModelError, ModelExecutionHook,
    ModelExecutionMetadata, ModelMessage, ModelRequest, ModelRequestContext,
    ModelRequestParameters, ModelRequestPart, ModelResponse, ModelSettings,
};

#[derive(Default)]
struct CaptureHook {
    calls: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl ModelExecutionHook for CaptureHook {
    async fn before_model_request(
        &self,
        metadata: ModelExecutionMetadata,
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        params: &ModelRequestParameters,
        _context: &ModelRequestContext,
    ) -> Result<(), ModelError> {
        self.calls.lock().unwrap().push(json!({
            "phase": "before",
            "model_name": metadata.model_name,
            "provider_name": metadata.provider_name,
            "run_id": metadata.run_id.as_str(),
            "conversation_id": metadata.conversation_id.as_str(),
            "agent_name": metadata.agent_name,
            "stream": metadata.stream,
            "message_count": messages.len(),
            "temperature": settings.and_then(|settings| settings.temperature),
            "metadata_route": params.metadata.get("route"),
        }));
        Ok(())
    }

    async fn after_model_response(
        &self,
        metadata: ModelExecutionMetadata,
        response: &ModelResponse,
    ) -> Result<(), ModelError> {
        self.calls.lock().unwrap().push(json!({
            "phase": "after",
            "model_name": metadata.model_name,
            "provider_name": metadata.provider_name,
            "run_id": metadata.run_id.as_str(),
            "agent_name": metadata.agent_name,
            "output": response.text_output(),
        }));
        Ok(())
    }
}

#[tokio::test]
async fn hooked_model_wraps_request_with_typed_metadata() {
    let hook = Arc::new(CaptureHook::default());
    let inner = Arc::new(
        FunctionModel::new(|_messages, _settings, _info| Ok(ModelResponse::text("wrapped")))
            .with_model_name("capture-model"),
    );
    let model = HookedModel::new(inner).with_hook(hook.clone());
    let mut settings = ModelSettings {
        temperature: Some(0.3),
        ..ModelSettings::default()
    };
    settings
        .extra_body
        .insert("mode".to_string(), json!("test"));
    let mut params = ModelRequestParameters::default();
    params
        .metadata
        .insert("route".to_string(), json!("wrapper"));

    let response = model
        .request(
            vec![ModelMessage::Request(ModelRequest {
                parts: vec![ModelRequestPart::UserPrompt {
                    content: vec![ContentPart::text("hello")],
                    name: None,
                    metadata: Map::new(),
                }],
                timestamp: None,
                instructions: None,
                run_id: None,
                conversation_id: None,
                metadata: Map::new(),
            })],
            Some(settings),
            params,
            ModelRequestContext::new(
                RunId::from_string("run_wrapper"),
                ConversationId::from_string("conversation_wrapper"),
            )
            .with_llm_trace_metadata(Map::from_iter([(
                "agent_name".to_string(),
                json!("research-agent"),
            )])),
        )
        .await
        .unwrap();

    assert_eq!(response.text_output(), "wrapped");
    let calls = hook.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["phase"], "before");
    assert_eq!(calls[0]["model_name"], "capture-model");
    assert_eq!(calls[0]["provider_name"], "test");
    assert_eq!(calls[0]["run_id"], "run_wrapper");
    assert_eq!(calls[0]["conversation_id"], "conversation_wrapper");
    assert_eq!(calls[0]["agent_name"], "research-agent");
    assert_eq!(calls[0]["stream"], false);
    assert_eq!(calls[0]["message_count"], 1);
    assert_eq!(calls[0]["temperature"], 0.3);
    assert_eq!(calls[0]["metadata_route"], "wrapper");
    assert_eq!(calls[1]["phase"], "after");
    assert_eq!(calls[1]["output"], "wrapped");
}
