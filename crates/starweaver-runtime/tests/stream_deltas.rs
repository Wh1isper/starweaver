#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use async_trait::async_trait;
use starweaver_model::{
    ModelAdapter, ModelError, ModelMessage, ModelProfile, ModelRequestContext,
    ModelRequestParameters, ModelResponse, ModelResponseStreamEvent, ModelSettings, PartDelta,
    PartEnd, PartStart, ProtocolFamily,
};
use starweaver_runtime::{Agent, AgentStreamEvent};

#[derive(Clone)]
struct StreamingModel;

#[async_trait]
impl ModelAdapter for StreamingModel {
    fn model_name(&self) -> &'static str {
        "streaming"
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
        Ok(ModelResponse::text("fallback"))
    }

    async fn request_stream(
        &self,
        _messages: Vec<ModelMessage>,
        _settings: Option<ModelSettings>,
        _params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        Ok(vec![
            ModelResponseStreamEvent::PartStart(PartStart {
                index: 0,
                part_kind: "text".to_string(),
            }),
            ModelResponseStreamEvent::PartDelta(PartDelta {
                index: 0,
                delta: "hel".to_string(),
            }),
            ModelResponseStreamEvent::PartDelta(PartDelta {
                index: 0,
                delta: "lo".to_string(),
            }),
            ModelResponseStreamEvent::PartEnd(PartEnd { index: 0 }),
            ModelResponseStreamEvent::FinalResult(ModelResponse::text("hello")),
        ])
    }
}

#[tokio::test]
async fn runtime_stream_includes_provider_delta_events() {
    let result = Agent::new(Arc::new(StreamingModel))
        .run_stream("hello")
        .await
        .unwrap();

    assert_eq!(result.result.output, "hello");
    let deltas = result
        .events
        .iter()
        .filter(|record| matches!(record.event, AgentStreamEvent::ModelStream { .. }))
        .count();
    assert_eq!(deltas, 5);
    assert!(result.events.iter().any(|record| matches!(
        record.event,
        AgentStreamEvent::ModelStream {
            event: ModelResponseStreamEvent::PartDelta(ref delta),
            ..
        } if delta.delta == "hel"
    )));
}
