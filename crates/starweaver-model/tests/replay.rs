#![allow(missing_docs, clippy::unwrap_used)]

mod support;

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use starweaver_model::{
    ModelError, ModelProfile, ModelRequestParameters, ModelResponsePart, OutputMode,
    ProtocolFamily,
    adapter::{NativeToolDefinition, ToolDefinition},
    message::{ModelMessage, ModelRequest, ModelRequestPart, ModelResponse},
    prepare_model_request,
    providers::{
        anthropic::AnthropicMessagesAdapter, bedrock::BedrockConverseAdapter,
        gemini::GeminiGenerateContentAdapter, openai_chat::OpenAiChatAdapter,
        openai_responses::OpenAiResponsesAdapter,
    },
};

fn assert_json_eq(actual: &serde_json::Value, expected: &serde_json::Value) {
    support::replay::assert_json_eq(actual, expected);
}

fn build_provider_request(
    provider: &str,
    model: &str,
    history: &[ModelMessage],
    settings: Option<&starweaver_model::ModelSettings>,
    tools: &[ToolDefinition],
    native_tools: &[NativeToolDefinition],
) -> serde_json::Value {
    match provider {
        "openai_chat" => OpenAiChatAdapter::build_request(model, history, settings, tools).unwrap(),
        "openai_responses" => {
            OpenAiResponsesAdapter::build_request(model, history, settings, tools, native_tools)
                .unwrap()
        }
        "anthropic" => {
            AnthropicMessagesAdapter::build_request(model, history, settings, tools).unwrap()
        }
        "gemini" => GeminiGenerateContentAdapter::build_request_with_native_tools(
            history,
            settings,
            tools,
            native_tools,
        )
        .unwrap(),
        "bedrock" => {
            BedrockConverseAdapter::build_request(model, history, settings, tools).unwrap()
        }
        other => panic!("unknown provider fixture namespace: {other}"),
    }
}

fn json_round_trip<T>(value: &T) -> T
where
    T: Serialize + DeserializeOwned,
{
    serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap()
}

fn fixture_names(provider: &str) -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(provider);
    let mut names = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(|extension| extension.to_str()) == Some("json"))
                .then(|| path.file_stem()?.to_str().map(str::to_string))
                .flatten()
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn representative_compacted_history() -> Vec<ModelMessage> {
    let mut summary = ModelResponse::text(
        "## Condensed conversation summary\n\nThe user asked for provider replay coverage.",
    );
    summary
        .metadata
        .insert("keep".to_string(), json!("compact"));
    vec![
        ModelMessage::Request(ModelRequest {
            parts: vec![
                ModelRequestPart::SystemPrompt {
                    text: "You are concise.".to_string(),
                    metadata: serde_json::Map::new(),
                },
                ModelRequestPart::UserPrompt {
                    content: vec![starweaver_model::ContentPart::Text {
                        text: "Compact the conversation history.".to_string(),
                    }],
                    name: None,
                    metadata: serde_json::Map::new(),
                },
            ],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        }),
        ModelMessage::Response(summary),
        ModelMessage::Request(ModelRequest::user_text(
            "Continue from the restored context.",
        )),
    ]
}

fn prompted_retry_history() -> Vec<ModelMessage> {
    vec![
        ModelMessage::Request(ModelRequest::user_text("Return an answer.")),
        ModelMessage::Request(ModelRequest {
            parts: vec![ModelRequestPart::RetryPrompt {
                text: "output schema validation failed: /answer: expected string".to_string(),
                tool_call_id: None,
                metadata: serde_json::Map::new(),
            }],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        }),
    ]
}

fn json_contains_text(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(text) => text.contains(needle),
        Value::Array(items) => items.iter().any(|item| json_contains_text(item, needle)),
        Value::Object(map) => map.values().any(|item| json_contains_text(item, needle)),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn assert_openai_chat_request(fixture: &support::replay::RequestFixture) -> serde_json::Value {
    let request = build_provider_request(
        "openai_chat",
        &fixture.model,
        &fixture.history,
        fixture.settings.as_ref(),
        &fixture.tools,
        &fixture.native_tools,
    );
    assert_json_eq(&request, &fixture.expected_provider_request);
    if !fixture.request_parameters.tools.is_empty() {
        assert_eq!(fixture.request_parameters.tools, fixture.tools);
    }
    request
}

fn assert_openai_chat_fixture(name: &str) -> starweaver_model::ModelResponse {
    let fixture = support::replay::load_replay_fixture("openai_chat", name);
    assert_openai_chat_request(&fixture.request);

    let response = OpenAiChatAdapter::parse_response(&fixture.provider_response).unwrap();
    assert_eq!(response, fixture.expected_response);
    response
}

fn assert_openai_responses_fixture(name: &str) -> starweaver_model::ModelResponse {
    let fixture = support::replay::load_replay_fixture("openai_responses", name);
    let request = build_provider_request(
        "openai_responses",
        &fixture.request.model,
        &fixture.request.history,
        fixture.request.settings.as_ref(),
        &fixture.request.tools,
        &fixture.request.native_tools,
    );
    assert_json_eq(&request, &fixture.request.expected_provider_request);

    let response = OpenAiResponsesAdapter::parse_response(&fixture.provider_response).unwrap();
    assert_eq!(
        openai_response_projection(&response),
        fixture.expected_response
    );
    response
}

fn openai_response_projection(
    response: &starweaver_model::ModelResponse,
) -> starweaver_model::ModelResponse {
    let mut projected = response.clone();
    if let Some(provider) = &mut projected.provider {
        provider.details.clear();
    }
    projected.parts = response
        .parts
        .iter()
        .map(|part| match part {
            ModelResponsePart::ProviderText { text, .. } => {
                ModelResponsePart::Text { text: text.clone() }
            }
            ModelResponsePart::ProviderThinking {
                text,
                signature,
                provider,
            } => ModelResponsePart::Thinking {
                text: text.clone(),
                signature: signature.clone().or_else(|| provider.id.clone()),
            },
            ModelResponsePart::ProviderToolCall { call, .. } => {
                ModelResponsePart::ToolCall(call.clone())
            }
            ModelResponsePart::ProviderOpaque {
                item_type, payload, ..
            } => ModelResponsePart::NativeToolCall {
                tool_type: item_type.clone(),
                payload: payload.clone(),
            },
            other => other.clone(),
        })
        .collect();
    projected
}

fn assert_anthropic_fixture(name: &str) -> starweaver_model::ModelResponse {
    let fixture = support::replay::load_replay_fixture("anthropic", name);
    let request = build_provider_request(
        "anthropic",
        &fixture.request.model,
        &fixture.request.history,
        fixture.request.settings.as_ref(),
        &fixture.request.tools,
        &fixture.request.native_tools,
    );
    assert_json_eq(&request, &fixture.request.expected_provider_request);

    let response = AnthropicMessagesAdapter::parse_response(&fixture.provider_response).unwrap();
    assert_eq!(response, fixture.expected_response);
    response
}

fn assert_gemini_fixture(name: &str) -> starweaver_model::ModelResponse {
    let fixture = support::replay::load_replay_fixture("gemini", name);
    let request = build_provider_request(
        "gemini",
        &fixture.request.model,
        &fixture.request.history,
        fixture.request.settings.as_ref(),
        &fixture.request.tools,
        &fixture.request.native_tools,
    );
    assert_json_eq(&request, &fixture.request.expected_provider_request);

    let response =
        GeminiGenerateContentAdapter::parse_response(&fixture.provider_response).unwrap();
    assert_eq!(response, fixture.expected_response);
    response
}

fn assert_bedrock_fixture(name: &str) -> starweaver_model::ModelResponse {
    let fixture = support::replay::load_replay_fixture("bedrock", name);
    let request = build_provider_request(
        "bedrock",
        &fixture.request.model,
        &fixture.request.history,
        fixture.request.settings.as_ref(),
        &fixture.request.tools,
        &fixture.request.native_tools,
    );
    assert_json_eq(&request, &fixture.request.expected_provider_request);

    let response = BedrockConverseAdapter::parse_response(&fixture.provider_response).unwrap();
    assert_eq!(response, fixture.expected_response);
    response
}

#[test]
fn replays_openai_chat_request_and_response() {
    let response = assert_openai_chat_fixture("text_response");
    assert_eq!(response.text_output(), "4");
    assert_eq!(response.usage.total_tokens, 11);
    assert_eq!(response.provider.unwrap().name, "openai");
    assert_eq!(
        response.finish_reason.unwrap(),
        starweaver_model::message::FinishReason::Stop
    );
}

#[test]
fn replays_openai_chat_tool_call_response() {
    let response = assert_openai_chat_fixture("tool_call_response");
    assert_eq!(response.text_output(), "");
    assert_eq!(response.usage.total_tokens, 27);
    assert!(matches!(response.parts[0], ModelResponsePart::ToolCall(_)));
    assert_eq!(
        response.provider.unwrap().response_id.unwrap(),
        "chatcmpl_tool_1"
    );
    assert_eq!(
        response.finish_reason.unwrap(),
        starweaver_model::message::FinishReason::ToolCalls
    );
}

#[test]
fn replays_openai_chat_tool_return_history() {
    let response = assert_openai_chat_fixture("tool_return_history");
    assert_eq!(response.text_output(), "Paris is clear and 18C.");
    assert_eq!(response.usage.total_tokens, 45);
    assert_eq!(
        response.provider.unwrap().response_id.unwrap(),
        "chatcmpl_final_1"
    );
    assert_eq!(
        response.finish_reason.unwrap(),
        starweaver_model::message::FinishReason::Stop
    );
}

#[test]
fn replays_openai_chat_extended_request_and_safety_fixtures() {
    for name in [
        "tool_choice_auto",
        "tool_choice_none",
        "tool_choice_required",
        "tool_choice_named_tool",
        "parallel_tool_calls",
        "json_object_mode",
        "content_filter_response",
        "refusal_response",
        "multimodal_user_input",
    ] {
        assert_openai_chat_fixture(name);
    }
}

#[test]
fn replays_openai_chat_malformed_choices_error_fixture() {
    let fixture = support::replay::load_error_fixture("openai_chat", "malformed_choices");
    assert_openai_chat_request(&fixture.request);
    let error = OpenAiChatAdapter::parse_response(&fixture.provider_response).unwrap_err();
    match error {
        ModelError::ResponseParsing(message) => {
            assert_eq!(fixture.expected_error.kind, "response_parsing");
            assert_eq!(message, fixture.expected_error.message);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn replays_openai_responses_text_response() {
    let response = assert_openai_responses_fixture("text_response");
    assert_eq!(response.text_output(), "4");
    assert_eq!(response.usage.total_tokens, 11);
    assert_eq!(
        response.provider.unwrap().response_id.unwrap(),
        "resp_text_1"
    );
    assert_eq!(
        response.finish_reason.unwrap(),
        starweaver_model::message::FinishReason::Stop
    );
}

#[test]
fn replays_openai_responses_request_and_tool_response() {
    let response = assert_openai_responses_fixture("tool_response");
    assert_eq!(response.text_output(), "Need lookup");
    assert!(matches!(
        &response.parts[1],
        ModelResponsePart::ProviderToolCall { call, provider }
            if call.id == "call_1" && call.name == "lookup" && provider.provider_name.as_deref() == Some("openai")
    ));
    assert_eq!(response.tool_calls()[0].id, "call_1");
    assert_eq!(response.provider.unwrap().response_id.unwrap(), "resp_1");
}

#[test]
fn maps_native_tools_to_openai_responses_tools() {
    let fixture =
        support::replay::load_request_fixture("openai_responses", "native_web_search_request");
    let request = OpenAiResponsesAdapter::build_request(
        &fixture.model,
        &fixture.history,
        fixture.settings.as_ref(),
        &fixture.tools,
        &fixture.native_tools,
    )
    .unwrap();

    assert_json_eq(&request, &fixture.expected_provider_request);
    assert_eq!(request["tools"][0]["type"], "function");
    assert_eq!(request["tools"][0]["name"], "lookup");
    assert_eq!(request["tools"][1]["type"], "web_search_preview");
    assert_eq!(request["tools"][1]["search_context_size"], "low");
}

#[test]
fn maps_native_mcp_to_openai_responses_tools() {
    let fixture = support::replay::load_request_fixture("openai_responses", "native_mcp_request");
    let request = OpenAiResponsesAdapter::build_request(
        &fixture.model,
        &fixture.history,
        fixture.settings.as_ref(),
        &fixture.tools,
        &fixture.native_tools,
    )
    .unwrap();

    assert_json_eq(&request, &fixture.expected_provider_request);
    assert_eq!(request["tools"][0]["type"], "mcp");
    assert_eq!(request["tools"][0]["server_label"], "deepwiki");
    assert_eq!(request["tools"][0]["require_approval"], "never");
}

#[test]
fn replays_openai_responses_extended_items_and_tool_choice() {
    for name in [
        "structured_output_response",
        "reasoning_item",
        "summary_item",
        "native_web_search_response",
        "native_mcp_approval_response",
        "file_image_output",
        "provider_refusal",
        "multimodal_user_input",
        "tool_choice_auto",
        "tool_choice_none",
        "tool_choice_required",
        "tool_choice_named_tool",
        "status_error",
    ] {
        assert_openai_responses_fixture(name);
    }
}

#[test]
fn replays_anthropic_request_and_response() {
    let response = assert_anthropic_fixture("text_response");
    assert_eq!(response.text_output(), "4");
    assert_eq!(response.usage.total_tokens, 10);
    assert_eq!(response.provider.unwrap().response_id.unwrap(), "msg_1");
}

#[test]
fn replays_anthropic_tool_use_response() {
    let response = assert_anthropic_fixture("tool_use_response");
    assert_eq!(response.text_output(), "");
    assert_eq!(response.usage.total_tokens, 16);
    assert!(matches!(response.parts[0], ModelResponsePart::ToolCall(_)));
    assert_eq!(
        response.finish_reason.unwrap(),
        starweaver_model::message::FinishReason::ToolCalls
    );
}

#[test]
fn replays_anthropic_tool_return_history() {
    let response = assert_anthropic_fixture("tool_return_history");
    assert_eq!(response.text_output(), "Paris is clear and 18C.");
    assert_eq!(response.usage.total_tokens, 36);
    assert_eq!(
        response.finish_reason.unwrap(),
        starweaver_model::message::FinishReason::Stop
    );
}

#[test]
fn anthropic_private_thinking_replay_fixture_maps_signature_natively() {
    let fixture = support::replay::load_request_fixture("anthropic", "provider_thinking_replay");
    let request = build_provider_request(
        "anthropic",
        &fixture.model,
        &fixture.history,
        fixture.settings.as_ref(),
        &fixture.tools,
        &fixture.native_tools,
    );
    assert_json_eq(&request, &fixture.expected_provider_request);

    let content = request["messages"][1]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "thinking");
    assert_eq!(content[0]["thinking"], "Need arithmetic.");
    assert_eq!(content[0]["signature"], "sig_replay_1");
}

#[test]
fn replays_anthropic_thinking_error_cache_and_stop_fixtures() {
    for name in [
        "thinking_block",
        "thinking_request",
        "tool_result_error_cache_control",
        "max_token_stop",
        "tool_use_with_text_preamble",
        "image_input",
        "safety_style_response",
    ] {
        assert_anthropic_fixture(name);
    }
}

#[test]
fn replays_gemini_request_and_response() {
    let response = assert_gemini_fixture("text_response");
    assert_eq!(response.text_output(), "4");
    assert_eq!(response.usage.total_tokens, 6);
    assert_eq!(response.provider.unwrap().name, "gemini");
}

#[test]
fn replays_gemini_tool_use_response() {
    let response = assert_gemini_fixture("tool_use_response");
    assert_eq!(response.text_output(), "");
    assert_eq!(response.usage.total_tokens, 16);
    assert!(matches!(response.parts[0], ModelResponsePart::ToolCall(_)));
}

#[test]
fn replays_gemini_tool_return_history() {
    let response = assert_gemini_fixture("tool_return_history");
    assert_eq!(response.text_output(), "Paris is clear and 18C.");
    assert_eq!(response.usage.total_tokens, 36);
}

#[test]
fn replays_gemini_safety_tool_config_and_finish_reason_fixtures() {
    for name in [
        "safety_block",
        "max_token_stop",
        "function_call_missing_id",
        "tool_config_function_calling_mode",
        "native_code_execution_request",
        "native_google_search_request",
        "multimodal_input",
        "malformed_candidate",
    ] {
        assert_gemini_fixture(name);
    }
}

#[test]
fn replays_bedrock_request_and_response() {
    let response = assert_bedrock_fixture("text_response");
    assert_eq!(response.text_output(), "4");
    assert_eq!(response.usage.total_tokens, 9);
    assert_eq!(response.provider.unwrap().response_id.unwrap(), "aws_1");
}

#[test]
fn replays_bedrock_tool_use_response() {
    let response = assert_bedrock_fixture("tool_use_response");
    assert_eq!(response.text_output(), "");
    assert_eq!(response.usage.total_tokens, 16);
    assert!(matches!(response.parts[0], ModelResponsePart::ToolCall(_)));
    assert_eq!(
        response.finish_reason.unwrap(),
        starweaver_model::message::FinishReason::ToolCalls
    );
}

#[test]
fn replays_bedrock_tool_return_history() {
    let response = assert_bedrock_fixture("tool_return_history");
    assert_eq!(response.text_output(), "Paris is clear and 18C.");
    assert_eq!(response.usage.total_tokens, 36);
    assert_eq!(
        response.finish_reason.unwrap(),
        starweaver_model::message::FinishReason::Stop
    );
}

#[test]
fn replays_bedrock_strict_tool_stop_fields_and_filter_fixtures() {
    for name in [
        "strict_tool_call",
        "max_token_stop",
        "additional_model_response_fields",
        "provider_status_error",
        "content_block_variants",
        "tool_result_error",
        "sigv4_gateway_metadata",
    ] {
        assert_bedrock_fixture(name);
    }
}

#[test]
fn provider_requests_are_stable_after_restored_fixture_state() {
    for provider in [
        "openai_chat",
        "openai_responses",
        "anthropic",
        "gemini",
        "bedrock",
    ] {
        for name in fixture_names(provider) {
            let fixture = support::replay::load_request_fixture(provider, &name);
            let baseline = build_provider_request(
                provider,
                &fixture.model,
                &fixture.history,
                fixture.settings.as_ref(),
                &fixture.tools,
                &fixture.native_tools,
            );
            let restored_history: Vec<ModelMessage> = json_round_trip(&fixture.history);
            let restored_settings: Option<starweaver_model::ModelSettings> =
                json_round_trip(&fixture.settings);
            let restored_tools: Vec<ToolDefinition> = json_round_trip(&fixture.tools);
            let restored_native_tools: Vec<NativeToolDefinition> =
                json_round_trip(&fixture.native_tools);
            let restored = build_provider_request(
                provider,
                &fixture.model,
                &restored_history,
                restored_settings.as_ref(),
                &restored_tools,
                &restored_native_tools,
            );

            assert_json_eq(&baseline, &fixture.expected_provider_request);
            assert_json_eq(&restored, &baseline);
        }
    }
}

#[test]
fn provider_requests_preserve_representative_compacted_history_shape() {
    let history = representative_compacted_history();

    let openai_chat = build_provider_request("openai_chat", "model", &history, None, &[], &[]);
    let chat_messages = openai_chat["messages"].as_array().unwrap();
    assert_eq!(chat_messages[0]["role"], "system");
    assert_eq!(chat_messages[1]["role"], "user");
    assert_eq!(chat_messages[2]["role"], "assistant");
    assert!(
        chat_messages[2]["content"]
            .as_str()
            .unwrap()
            .contains("Condensed conversation summary")
    );
    assert_eq!(chat_messages[3]["role"], "user");

    let openai_responses =
        build_provider_request("openai_responses", "model", &history, None, &[], &[]);
    assert_eq!(openai_responses["instructions"], "You are concise.");
    let responses_input = openai_responses["input"].as_array().unwrap();
    assert_eq!(responses_input[0]["role"], "user");
    assert_eq!(responses_input[1]["role"], "assistant");
    assert!(
        responses_input[1]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Condensed conversation summary")
    );
    assert_eq!(responses_input[2]["role"], "user");

    let anthropic = build_provider_request("anthropic", "model", &history, None, &[], &[]);
    let anthropic_messages = anthropic["messages"].as_array().unwrap();
    assert_eq!(anthropic_messages[0]["role"], "user");
    assert_eq!(anthropic_messages[1]["role"], "assistant");
    assert!(
        anthropic_messages[1]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Condensed conversation summary")
    );
    assert_eq!(anthropic_messages[2]["role"], "user");

    let gemini = build_provider_request("gemini", "model", &history, None, &[], &[]);
    let gemini_contents = gemini["contents"].as_array().unwrap();
    assert_eq!(gemini_contents[0]["role"], "user");
    assert_eq!(gemini_contents[1]["role"], "model");
    assert!(
        gemini_contents[1]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Condensed conversation summary")
    );
    assert_eq!(gemini_contents[2]["role"], "user");

    let bedrock = build_provider_request("bedrock", "model", &history, None, &[], &[]);
    let bedrock_messages = bedrock["messages"].as_array().unwrap();
    assert_eq!(bedrock_messages[0]["role"], "user");
    assert_eq!(bedrock_messages[1]["role"], "assistant");
    assert!(
        bedrock_messages[1]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Condensed conversation summary")
    );
    assert_eq!(bedrock_messages[2]["role"], "user");
}

#[test]
fn provider_requests_include_prompted_output_retry_diagnostics() {
    let params = ModelRequestParameters {
        output_mode: Some(OutputMode::Prompted),
        output_schema: Some(json!({
            "name": "answer",
            "schema": {
                "type": "object",
                "properties": {
                    "answer": {"type": "string"}
                },
                "required": ["answer"]
            }
        })),
        ..ModelRequestParameters::default()
    };

    for (provider, protocol) in [
        ("openai_chat", ProtocolFamily::OpenAiChatCompletions),
        ("openai_responses", ProtocolFamily::OpenAiResponses),
        ("anthropic", ProtocolFamily::AnthropicMessages),
        ("gemini", ProtocolFamily::GeminiGenerateContent),
        ("bedrock", ProtocolFamily::BedrockConverse),
    ] {
        let profile = ModelProfile::for_protocol(protocol);
        let prepared = prepare_model_request(
            prompted_retry_history(),
            None,
            None,
            params.clone(),
            &profile,
        );
        let request = build_provider_request(
            provider,
            "model",
            &prepared.normalized_messages,
            prepared.settings.as_ref(),
            &prepared.params.tools,
            &prepared.params.native_tools,
        );

        assert_eq!(prepared.output_mode, OutputMode::Prompted);
        assert!(
            json_contains_text(
                &request,
                "Always respond with a JSON object that matches this schema"
            ),
            "{provider} request missing prompted output schema instruction: {request:#}"
        );
        assert!(
            json_contains_text(&request, "\"answer\""),
            "{provider} request missing output schema content: {request:#}"
        );
        assert!(
            json_contains_text(&request, "output schema validation failed"),
            "{provider} request missing retry diagnostic: {request:#}"
        );
    }
}

#[test]
fn canonical_messages_round_trip() {
    let fixture = support::replay::load_replay_fixture("openai_chat", "tool_return_history");
    let encoded = serde_json::to_string_pretty(&fixture.request.history).unwrap();
    let decoded: Vec<ModelMessage> = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, fixture.request.history);
}

#[test]
fn fixture_supports_request_parameters_round_trip() {
    let fixture = support::replay::load_replay_fixture("openai_chat", "text_response");
    let encoded = serde_json::to_value(&fixture.request.request_parameters).unwrap();
    assert_eq!(encoded["tools"][0]["name"], json!("lookup"));
    assert_eq!(
        fixture.request.request_parameters.tools,
        fixture.request.tools
    );
}
