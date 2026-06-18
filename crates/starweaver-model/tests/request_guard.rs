#![allow(missing_docs, clippy::unwrap_used)]

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use serde_json::{json, Map};
use starweaver_core::{CancellationToken, ConversationId, RunId};
use starweaver_model::{
    block_real_model_requests,
    profile::{ModelProfile, ProtocolFamily},
    transport::{HttpModelConfig, HttpRequest, HttpResponse},
    ModelAdapter, ModelError, ModelHttpClient, ModelRequestContext, ModelRequestParameters,
    ProtocolModelClient, ReqwestHttpClient,
};

#[derive(Clone, Default)]
struct PanicHttpClient;

#[async_trait]
impl ModelHttpClient for PanicHttpClient {
    async fn send(&self, _request: HttpRequest) -> Result<HttpResponse, ModelError> {
        panic!("protocol guard should block before injected transport is called");
    }
}

fn context() -> ModelRequestContext {
    ModelRequestContext::new(
        RunId::from_string("run_guard"),
        ConversationId::from_string("conv_guard"),
    )
}

#[tokio::test]
async fn global_guard_blocks_production_model_requests_and_restores_setting() {
    assert!(starweaver_model::allow_real_model_requests());

    let guard = block_real_model_requests();
    assert!(!starweaver_model::allow_real_model_requests());

    let reqwest_client = ReqwestHttpClient::new().unwrap();
    let request = starweaver_model::HttpRequest {
        method: starweaver_model::transport::HttpMethod::Post,
        url: "https://api.openai.com/v1/responses".to_string(),
        headers: BTreeMap::default(),
        body: json!({"model": "gpt-4.1-mini", "input": "hello"}),
        timeout: None,
        metadata: Map::default(),
        cancellation_token: CancellationToken::default(),
    };
    let error = reqwest_client.send(request).await.unwrap_err();
    assert!(matches!(
        error,
        ModelError::RealModelRequestBlocked { url } if url == "https://api.openai.com/v1/responses"
    ));

    let protocol_client = ProtocolModelClient::new(
        "openai",
        "gpt-4.1-mini",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
        HttpModelConfig::new("https://api.openai.com/v1", "responses"),
        Arc::new(PanicHttpClient),
    );
    let error = protocol_client
        .request(
            Vec::new(),
            None,
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ModelError::RealModelRequestBlocked { url } if url == "https://api.openai.com/v1/responses"
    ));

    {
        let _allow = starweaver_model::allow_real_model_requests_guard();
        assert!(starweaver_model::allow_real_model_requests());
    }
    assert!(!starweaver_model::allow_real_model_requests());

    drop(guard);
    assert!(starweaver_model::allow_real_model_requests());
}
