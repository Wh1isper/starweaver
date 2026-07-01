#![allow(missing_docs, clippy::unwrap_used)]

use starweaver_agent::{DirectModelRequest, TestModel, model_request, model_request_stream};
use starweaver_model::{
    ContentPart, ModelMessage, ModelRequest, ModelRequestPart, ModelResponseStreamEvent,
};

fn user_message(text: &str) -> ModelMessage {
    ModelMessage::Request(ModelRequest {
        parts: vec![ModelRequestPart::UserPrompt {
            content: vec![ContentPart::Text {
                text: text.to_string(),
            }],
            name: None,
            metadata: serde_json::Map::new(),
        }],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    })
}

#[tokio::test]
async fn sdk_reexports_direct_model_helpers() {
    let model = TestModel::with_text("ok");
    let response = model_request(&model, DirectModelRequest::new(vec![user_message("hi")]))
        .await
        .unwrap();

    assert_eq!(response.text_output(), "ok");
}

#[tokio::test]
async fn sdk_reexports_direct_model_stream_helpers() {
    let model = TestModel::with_text("ok");
    let events = model_request_stream(&model, DirectModelRequest::new(vec![user_message("hi")]))
        .await
        .unwrap();

    assert!(matches!(
        events.last().unwrap(),
        ModelResponseStreamEvent::FinalResult(_)
    ));
}
