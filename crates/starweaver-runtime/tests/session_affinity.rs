#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use starweaver_context::{AgentContext, ModelCapability};
use starweaver_core::SessionId;
use starweaver_model::{
    CodexSettings, FunctionModel, FunctionModelInfo, GatewaySettings, ModelProfile,
    ModelRequestParameters, ModelResponse, ModelSettings, OpenAiResponsesSettings,
    ProfileOverrideModel, ProtocolFamily, ProviderSettings, canonical_codex_routing_id,
};
use starweaver_runtime::Agent;

#[tokio::test]
async fn capable_openai_responses_transport_binds_cache_key_without_mutating_inputs() {
    let captured = Arc::new(Mutex::new(Vec::<(
        Option<ModelSettings>,
        ModelRequestParameters,
    )>::new()));
    let model_captured = Arc::clone(&captured);
    let mut profile = ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses);
    profile.supports_openai_responses_session_header = true;
    let model = FunctionModel::new(move |_messages, settings, info: FunctionModelInfo| {
        model_captured.lock().unwrap().push((settings, info.params));
        Ok(ModelResponse::text("ok"))
    })
    .with_model_name("gpt-5.5")
    .with_profile(profile);
    let configured_settings = ModelSettings {
        temperature: Some(0.1),
        provider_settings: ProviderSettings {
            openai_responses: Some(OpenAiResponsesSettings {
                prompt_cache_key: Some("typed-stale".to_string()),
                ..OpenAiResponsesSettings::default()
            }),
            ..ProviderSettings::default()
        },
        extra_headers: std::collections::BTreeMap::from([
            ("X-Session-ID".to_string(), "header-stale".to_string()),
            ("x-other".to_string(), "value".to_string()),
        ]),
        extra_body: serde_json::Map::from_iter([
            (
                "prompt_cache_key".to_string(),
                serde_json::json!("body-stale"),
            ),
            ("other".to_string(), serde_json::json!("value")),
        ]),
        ..ModelSettings::default()
    };
    let original_settings = configured_settings.clone();
    let mut configured_params = ModelRequestParameters::default();
    configured_params
        .http
        .headers
        .insert("x-session-id".to_string(), "request-stale".to_string());
    configured_params.http.extra_body.insert(
        "prompt_cache_key".to_string(),
        serde_json::json!("http-stale"),
    );
    configured_params.extra_body.insert(
        "prompt_cache_key".to_string(),
        serde_json::json!("params-stale"),
    );
    let original_params = configured_params.clone();
    let agent = Agent::new(Arc::new(model))
        .with_model_settings(configured_settings.clone())
        .with_request_params(configured_params.clone());
    let mut context = AgentContext::default();
    context.set_session_id(SessionId::from_string("session-affinity-runtime"));
    context
        .model_config
        .capabilities
        .insert(ModelCapability::OpenAiPromptCacheKey);

    let result = agent.run_with_context("hello", &mut context).await.unwrap();

    assert_eq!(result.output, "ok");
    let (settings, params) = captured.lock().unwrap()[0].clone();
    let settings = settings.unwrap();
    assert_eq!(
        settings
            .provider_settings
            .openai_responses
            .as_ref()
            .and_then(|settings| settings.prompt_cache_key.as_deref()),
        Some("session-affinity-runtime")
    );
    assert_eq!(
        settings
            .extra_headers
            .get("x-session-id")
            .map(String::as_str),
        Some("session-affinity-runtime")
    );
    assert_eq!(
        settings.extra_headers.get("x-other").map(String::as_str),
        Some("value")
    );
    assert!(!settings.extra_body.contains_key("prompt_cache_key"));
    assert_eq!(
        params.http.headers.get("x-session-id").map(String::as_str),
        Some("session-affinity-runtime")
    );
    assert!(!params.http.extra_body.contains_key("prompt_cache_key"));
    assert_eq!(
        params.extra_body.get("prompt_cache_key"),
        Some(&serde_json::json!("session-affinity-runtime"))
    );
    assert_eq!(
        params
            .metadata
            .get(starweaver_model::settings::SESSION_BOUND_PROMPT_CACHE_METADATA_KEY),
        Some(&serde_json::json!(true))
    );
    assert_eq!(configured_settings, original_settings);
    assert_eq!(configured_params, original_params);
}

#[tokio::test]
async fn prompt_cache_binding_requires_explicit_model_capability_and_supported_transport() {
    for (supports_header, has_capability, expected_cache_key) in [
        (true, false, None),
        (false, true, None),
        (true, true, Some("session-capable")),
    ] {
        let captured = Arc::new(Mutex::new(Vec::<Option<ModelSettings>>::new()));
        let model_captured = Arc::clone(&captured);
        let mut profile = ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses);
        profile.supports_openai_responses_session_header = supports_header;
        let model = FunctionModel::new(move |_messages, settings, _info: FunctionModelInfo| {
            model_captured.lock().unwrap().push(settings);
            Ok(ModelResponse::text("ok"))
        })
        .with_model_name("gpt-5.5")
        .with_profile(profile);
        let agent = Agent::new(Arc::new(model));
        let mut context = AgentContext::default();
        context.set_session_id(SessionId::from_string("session-capable"));
        if has_capability {
            context
                .model_config
                .capabilities
                .insert(ModelCapability::OpenAiPromptCacheKey);
        }

        agent.run_with_context("hello", &mut context).await.unwrap();

        let settings = captured.lock().unwrap()[0].clone();
        let cache_key = settings
            .as_ref()
            .and_then(|settings| settings.provider_settings.openai_responses.as_ref())
            .and_then(|settings| settings.prompt_cache_key.as_deref());
        assert_eq!(cache_key, expected_cache_key);
    }
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
    let agent = Agent::new(Arc::new(model)).with_model_settings(ModelSettings {
        provider_settings: ProviderSettings {
            codex: Some(CodexSettings {
                session_id: Some("configured-stale".to_string()),
                thread_id: Some("configured-thread-stale".to_string()),
            }),
            openai_responses: Some(OpenAiResponsesSettings {
                prompt_cache_key: Some("configured-cache-stale".to_string()),
                ..OpenAiResponsesSettings::default()
            }),
            ..ProviderSettings::default()
        },
        ..ModelSettings::default()
    });
    let mut context = AgentContext::default();
    let affinity_id = "会话".repeat(40);
    let canonical_affinity_id = canonical_codex_routing_id(&affinity_id);
    context.set_session_id(SessionId::from_string(affinity_id));
    context
        .model_config
        .capabilities
        .insert(ModelCapability::OpenAiPromptCacheKey);

    let result = agent.run_with_context("hello", &mut context).await.unwrap();

    assert_eq!(result.output, "ok");
    let (settings, run_id) = captured.lock().unwrap()[0].clone();
    let provider_settings = settings.unwrap().provider_settings;
    assert_eq!(
        provider_settings.codex,
        Some(CodexSettings {
            session_id: Some(canonical_affinity_id.clone()),
            thread_id: Some(run_id),
        })
    );
    assert_eq!(
        provider_settings
            .openai_responses
            .and_then(|settings| settings.prompt_cache_key),
        Some(canonical_affinity_id)
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
        let agent = Agent::new(Arc::new(model)).with_model_settings(ModelSettings {
            provider_settings: ProviderSettings {
                gateway: Some(GatewaySettings {
                    x_session_id: Some("configured-affinity".to_string()),
                    ..GatewaySettings::default()
                }),
                ..ProviderSettings::default()
            },
            ..ModelSettings::default()
        });
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
        let expected_session_id = if matches!(protocol, ProtocolFamily::OpenAiResponses) {
            "session_affinity_gateway"
        } else {
            "configured-affinity"
        };
        assert_eq!(
            gateway,
            GatewaySettings {
                x_session_id: Some(expected_session_id.to_string()),
                ..GatewaySettings::default()
            }
        );
    }
}
