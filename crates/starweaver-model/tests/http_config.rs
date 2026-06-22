#![allow(missing_docs)]

use starweaver_model::{
    anthropic_http_config, gemini_http_config, openai_chat_http_config,
    openai_responses_http_config, HttpModelConfig,
};

#[test]
fn provider_presets_insert_api_root_for_plain_gateway_base_urls() {
    assert_eq!(
        anthropic_http_config("ak")
            .with_base_url("http://localhost:8090")
            .endpoint_url(),
        "http://localhost:8090/v1/messages"
    );
    assert_eq!(
        openai_responses_http_config("sk")
            .with_base_url("http://localhost:8090")
            .endpoint_url(),
        "http://localhost:8090/v1/responses"
    );
    assert_eq!(
        openai_chat_http_config("sk")
            .with_base_url("http://localhost:8090")
            .endpoint_url(),
        "http://localhost:8090/v1/chat/completions"
    );
    assert_eq!(
        gemini_http_config("gk", "gemini-3-pro")
            .with_base_url("http://localhost:8090")
            .endpoint_url(),
        "http://localhost:8090/v1beta/models/gemini-3-pro:generateContent?key=gk"
    );
}

#[test]
fn provider_presets_append_endpoint_directly_for_gateway_sub_paths() {
    assert_eq!(
        anthropic_http_config("ak")
            .with_base_url("http://localhost:8090/abc")
            .endpoint_url(),
        "http://localhost:8090/abc/messages"
    );
    assert_eq!(
        openai_responses_http_config("sk")
            .with_base_url("http://localhost:8090/abc")
            .endpoint_url(),
        "http://localhost:8090/abc/responses"
    );
    assert_eq!(
        openai_chat_http_config("sk")
            .with_base_url("http://localhost:8090/abc")
            .endpoint_url(),
        "http://localhost:8090/abc/chat/completions"
    );
    assert_eq!(
        gemini_http_config("gk", "gemini-3-pro")
            .with_base_url("http://localhost:8090/abc")
            .endpoint_url(),
        "http://localhost:8090/abc/models/gemini-3-pro:generateContent?key=gk"
    );
}

#[test]
fn provider_presets_do_not_duplicate_existing_api_root_paths() {
    assert_eq!(
        anthropic_http_config("ak")
            .with_base_url("https://gateway.example/v1")
            .endpoint_url(),
        "https://gateway.example/v1/messages"
    );
    assert_eq!(
        openai_responses_http_config("sk")
            .with_base_url("https://gateway.example/v1")
            .endpoint_url(),
        "https://gateway.example/v1/responses"
    );
    assert_eq!(
        openai_chat_http_config("sk")
            .with_base_url("https://gateway.example/v1")
            .endpoint_url(),
        "https://gateway.example/v1/chat/completions"
    );
    assert_eq!(
        gemini_http_config("gk", "gemini-3-pro")
            .with_base_url("https://gateway.example/v1beta")
            .endpoint_url(),
        "https://gateway.example/v1beta/models/gemini-3-pro:generateContent?key=gk"
    );
}

#[test]
fn explicit_endpoint_path_overrides_provider_api_root_policy() {
    assert_eq!(
        anthropic_http_config("ak")
            .with_base_url("http://localhost:8090")
            .with_endpoint_path("custom/messages")
            .endpoint_url(),
        "http://localhost:8090/custom/messages"
    );
    assert_eq!(
        openai_responses_http_config("sk")
            .with_base_url("http://localhost:8090")
            .with_endpoint_path("custom/responses")
            .endpoint_url(),
        "http://localhost:8090/custom/responses"
    );
}

#[test]
fn plain_http_config_keeps_legacy_base_and_endpoint_joining() {
    assert_eq!(
        HttpModelConfig::new("https://gateway.example", "responses").endpoint_url(),
        "https://gateway.example/responses"
    );
    assert_eq!(
        HttpModelConfig::new("https://gateway.example/v1/", "/responses").endpoint_url(),
        "https://gateway.example/v1/responses"
    );
}
