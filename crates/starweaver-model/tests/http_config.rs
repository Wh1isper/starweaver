#![allow(missing_docs)]

use starweaver_model::{
    anthropic_http_config, gemini_http_config, google_cloud_http_config,
    google_cloud_project_http_config, openai_chat_http_config, openai_responses_http_config,
    HttpModelConfig,
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
    assert_eq!(
        google_cloud_http_config("gk", "gemini-3-pro")
            .with_base_url("http://localhost:8090")
            .endpoint_url(),
        "http://localhost:8090/v1beta1/publishers/google/models/gemini-3-pro:generateContent"
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
    assert_eq!(
        google_cloud_http_config("gk", "gemini-3-pro")
            .with_base_url("http://localhost:8090/abc")
            .endpoint_url(),
        "http://localhost:8090/abc/publishers/google/models/gemini-3-pro:generateContent"
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
    assert_eq!(
        google_cloud_http_config("gk", "gemini-3-pro")
            .with_base_url("https://gateway.example/v1beta1")
            .endpoint_url(),
        "https://gateway.example/v1beta1/publishers/google/models/gemini-3-pro:generateContent"
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

#[test]
fn google_cloud_presets_match_vertex_generate_content_paths() {
    let express = google_cloud_http_config("gk", "gemini-3-pro");
    assert_eq!(
        express.endpoint_url(),
        "https://aiplatform.googleapis.com/v1beta1/publishers/google/models/gemini-3-pro:generateContent"
    );
    assert_eq!(express.headers["x-goog-api-key"], "gk");

    let project = google_cloud_project_http_config(
        "token",
        "gemini-3-pro",
        "starweaver-project",
        "us-central1",
    );
    assert_eq!(
        project.endpoint_url(),
        "https://us-central1-aiplatform.googleapis.com/v1beta1/projects/starweaver-project/locations/us-central1/publishers/google/models/gemini-3-pro:generateContent"
    );

    let multi_region =
        google_cloud_project_http_config("token", "gemini-3-pro", "starweaver-project", "eu");
    assert_eq!(
        multi_region.endpoint_url(),
        "https://aiplatform.eu.rep.googleapis.com/v1beta1/projects/starweaver-project/locations/eu/publishers/google/models/gemini-3-pro:generateContent"
    );

    let publisher_model = google_cloud_http_config("gk", "anthropic/claude-sonnet-4-5");
    assert_eq!(
        publisher_model.endpoint_url(),
        "https://aiplatform.googleapis.com/v1beta1/publishers/anthropic/models/claude-sonnet-4-5:generateContent"
    );
}
