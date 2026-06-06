#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Map, Value};
use starweaver_core::{ConversationId, RunId};
use starweaver_model::{
    ContentPart, HttpModelConfig, HttpRequest, HttpResponse, ModelAdapter, ModelError,
    ModelHttpClient, ModelMessage, ModelProfile, ModelRequest, ModelRequestContext,
    ModelRequestParameters, ModelRequestPart, ProtocolFamily, ProtocolModelClient,
};

#[derive(Clone)]
struct CaptureHttpClient {
    requests: Arc<Mutex<Vec<HttpRequest>>>,
    response: HttpResponse,
}

impl CaptureHttpClient {
    fn new(response: HttpResponse) -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            response,
        }
    }

    fn last_body(&self) -> Value {
        self.requests
            .lock()
            .unwrap()
            .last()
            .map(|request| request.body.clone())
            .unwrap()
    }
}

#[async_trait]
impl ModelHttpClient for CaptureHttpClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, ModelError> {
        self.requests.lock().unwrap().push(request);
        Ok(self.response.clone())
    }
}

#[tokio::test]
async fn openai_chat_maps_binary_image_as_data_url() {
    let http = CaptureHttpClient::new(HttpResponse::ok(json!({
        "choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}]
    })));
    let client = protocol_client(ProtocolFamily::OpenAiChatCompletions, http.clone());

    client
        .request(
            history_with(ContentPart::Binary {
                data: vec![1, 2, 3],
                media_type: "image/png".to_string(),
            }),
            None,
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    let body = http.last_body();
    let url = body["messages"][0]["content"][0]["image_url"]["url"]
        .as_str()
        .unwrap();
    assert_eq!(url, "data:image/png;base64,AQID");
}

#[tokio::test]
async fn openai_responses_maps_resource_ref_to_image_url() {
    let http = CaptureHttpClient::new(HttpResponse::ok(json!({
        "output": [{"content": [{"type": "output_text", "text": "ok"}]}]
    })));
    let client = protocol_client(ProtocolFamily::OpenAiResponses, http.clone());

    client
        .request(
            history_with(ContentPart::ResourceRef {
                uri: "https://cdn.example.test/image.png".to_string(),
                media_type: "image/png".to_string(),
                resource_type: "image".to_string(),
                metadata: Map::new(),
            }),
            None,
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    let body = http.last_body();
    assert_eq!(
        body["input"][0]["content"][0],
        json!({"type": "input_image", "image_url": "https://cdn.example.test/image.png"})
    );
}

#[tokio::test]
async fn gemini_maps_binary_as_inline_data() {
    let http = CaptureHttpClient::new(HttpResponse::ok(json!({
        "candidates": [{"content": {"parts": [{"text": "ok"}]}, "finishReason": "STOP"}]
    })));
    let client = protocol_client(ProtocolFamily::GeminiGenerateContent, http.clone());

    client
        .request(
            history_with(ContentPart::Binary {
                data: vec![1, 2, 3],
                media_type: "image/png".to_string(),
            }),
            None,
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    let body = http.last_body();
    assert_eq!(
        body["contents"][0]["parts"][0]["inlineData"],
        json!({"data": "AQID", "mimeType": "image/png"})
    );
}

#[tokio::test]
async fn bedrock_maps_binary_as_base64_bytes() {
    let http = CaptureHttpClient::new(HttpResponse::ok(json!({
        "output": {"message": {"content": [{"text": "ok"}]}},
        "stopReason": "end_turn"
    })));
    let client = protocol_client(ProtocolFamily::BedrockConverse, http.clone());

    client
        .request(
            history_with(ContentPart::Binary {
                data: vec![1, 2, 3],
                media_type: "image/png".to_string(),
            }),
            None,
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    let body = http.last_body();
    assert_eq!(
        body["messages"][0]["content"][0]["image"],
        json!({"format": "png", "source": {"bytes": "AQID"}})
    );
}

fn protocol_client(protocol: ProtocolFamily, http: CaptureHttpClient) -> ProtocolModelClient {
    ProtocolModelClient::new(
        "test",
        "test-model",
        ModelProfile::for_protocol(protocol),
        HttpModelConfig::new("https://example.test", "/v1/test"),
        Arc::new(http),
    )
}

fn history_with(content: ContentPart) -> Vec<ModelMessage> {
    vec![ModelMessage::Request(ModelRequest {
        parts: vec![ModelRequestPart::UserPrompt {
            content: vec![content],
            name: None,
            metadata: Map::new(),
        }],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Map::new(),
    })]
}

fn context() -> ModelRequestContext {
    ModelRequestContext::new(
        RunId::from_string("run_multimodal"),
        ConversationId::from_string("conv_multimodal"),
    )
}
