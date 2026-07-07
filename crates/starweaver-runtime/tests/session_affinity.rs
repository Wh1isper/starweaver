#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use starweaver_context::AgentContext;
use starweaver_core::SessionId;
use starweaver_model::{
    CodexSettings, FunctionModel, FunctionModelInfo, GatewaySettings, ModelProfile, ModelResponse,
    ModelSettings, OpenAiChatSettings, ProfileOverrideModel, ProtocolFamily, ProviderSettings,
};
use starweaver_runtime::Agent;

#[tokio::test]
async fn openai_chat_session_affinity_injects_prompt_cache_key() {
    let captured = Arc::new(Mutex::new(Vec::<Option<ModelSettings>>::new()));
    let model_captured = Arc::clone(&captured);
    let model = FunctionModel::new(move |_messages, settings, _info: FunctionModelInfo| {
        model_captured.lock().unwrap().push(settings);
        Ok(ModelResponse::text("ok"))
    })
    .with_model_name("gpt-4.1-mini");
    let agent = Agent::new(Arc::new(model));
    let mut context = AgentContext::default();
    context.set_session_id(SessionId::from_string("session_affinity_runtime"));

    let result = agent.run_with_context("hello", &mut context).await.unwrap();

    assert_eq!(result.output, "ok");
    let settings = captured.lock().unwrap()[0].clone().unwrap();
    assert_eq!(
        settings
            .provider_settings
            .openai_chat
            .as_ref()
            .and_then(|settings| settings.prompt_cache_key.as_deref()),
        Some("sw_session_affinity_runtime")
    );
}

#[tokio::test]
async fn explicit_model_settings_override_openai_chat_session_affinity() {
    let captured = Arc::new(Mutex::new(Vec::<Option<ModelSettings>>::new()));
    let model_captured = Arc::clone(&captured);
    let model = FunctionModel::new(move |_messages, settings, _info: FunctionModelInfo| {
        model_captured.lock().unwrap().push(settings);
        Ok(ModelResponse::text("ok"))
    })
    .with_model_name("gpt-4.1-mini");
    let agent = Agent::new(Arc::new(model)).with_model_settings(ModelSettings {
        provider_settings: ProviderSettings {
            openai_chat: Some(OpenAiChatSettings {
                prompt_cache_key: Some("manual-cache-key".to_string()),
                ..OpenAiChatSettings::default()
            }),
            ..ProviderSettings::default()
        },
        ..ModelSettings::default()
    });
    let mut context = AgentContext::default();
    context.set_session_id(SessionId::from_string("session_affinity_runtime"));

    let result = agent.run_with_context("hello", &mut context).await.unwrap();

    assert_eq!(result.output, "ok");
    let settings = captured.lock().unwrap()[0].clone().unwrap();
    assert_eq!(
        settings
            .provider_settings
            .openai_chat
            .as_ref()
            .and_then(|settings| settings.prompt_cache_key.as_deref()),
        Some("manual-cache-key")
    );
}

#[tokio::test]
async fn codex_responses_session_affinity_injects_typed_routing_ids() {
    let captured = Arc::new(Mutex::new(Vec::<(Option<ModelSettings>, String)>::new()));
    let model_captured = Arc::clone(&captured);
    let inner = Arc::new(FunctionModel::new(
        move |_messages, settings, info: FunctionModelInfo| {
            model_captured
                .lock()
                .unwrap()
                .push((settings, info.context.run_id.as_str().to_string()));
            Ok(ModelResponse::text("ok"))
        },
    ));
    let model = ProfileOverrideModel::new(
        inner,
        ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
    )
    .with_provider_name(Some("codex".to_string()))
    .with_model_name("gpt-5.5");
    let agent = Agent::new(Arc::new(model));
    let mut context = AgentContext::default();
    context.set_session_id(SessionId::from_string("session_affinity_codex"));

    let result = agent.run_with_context("hello", &mut context).await.unwrap();

    assert_eq!(result.output, "ok");
    let (settings, run_id) = captured.lock().unwrap()[0].clone();
    let codex = settings.unwrap().provider_settings.codex.unwrap();
    assert_eq!(
        codex,
        CodexSettings {
            session_id: Some("session_affinity_codex".to_string()),
            thread_id: Some(run_id),
        }
    );
}

#[tokio::test]
async fn gateway_session_affinity_is_opt_in_for_openai_responses_gemini_and_bedrock_families() {
    for protocol in [
        ProtocolFamily::OpenAiResponses,
        ProtocolFamily::GeminiGenerateContent,
        ProtocolFamily::BedrockConverse,
    ] {
        let captured = Arc::new(Mutex::new(Vec::<Option<ModelSettings>>::new()));
        let model_captured = Arc::clone(&captured);
        let inner = Arc::new(FunctionModel::new(
            move |_messages, settings, _info: FunctionModelInfo| {
                model_captured.lock().unwrap().push(settings);
                Ok(ModelResponse::text("ok"))
            },
        ));
        let model = ProfileOverrideModel::new(inner, ModelProfile::for_protocol(protocol))
            .with_provider_name(Some("gateway".to_string()))
            .with_model_name("gpt-5.5");
        let agent = Agent::new(Arc::new(model));
        let mut context = AgentContext::default();
        context.set_session_id(SessionId::from_string("session_affinity_gateway"));
        context.metadata.insert(
            "starweaver.gateway_session_affinity".to_string(),
            serde_json::json!(true),
        );

        let result = agent.run_with_context("hello", &mut context).await.unwrap();

        assert_eq!(result.output, "ok");
        let gateway = captured.lock().unwrap()[0]
            .clone()
            .unwrap()
            .provider_settings
            .gateway
            .unwrap();
        assert_eq!(
            gateway,
            GatewaySettings {
                x_session_id: Some("session_affinity_gateway".to_string()),
                ..GatewaySettings::default()
            }
        );
    }
}
