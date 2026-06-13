#![allow(missing_docs, clippy::unwrap_used)]

mod support;

use serde_json::json;
use starweaver_model::{
    message::ModelMessage,
    providers::{
        anthropic::AnthropicMessagesAdapter, bedrock::BedrockConverseAdapter,
        gemini::GeminiGenerateContentAdapter, openai_chat::OpenAiChatAdapter,
        openai_responses::OpenAiResponsesAdapter,
    },
    ModelError, ModelResponsePart,
};

fn assert_json_eq(actual: &serde_json::Value, expected: &serde_json::Value) {
    support::replay::assert_json_eq(actual, expected);
}

fn assert_openai_chat_request(fixture: &support::replay::RequestFixture) -> serde_json::Value {
    let request = OpenAiChatAdapter::build_request(
        &fixture.model,
        &fixture.history,
        fixture.settings.as_ref(),
        &fixture.tools,
    )
    .unwrap();
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
    let request = OpenAiResponsesAdapter::build_request(
        &fixture.request.model,
        &fixture.request.history,
        fixture.request.settings.as_ref(),
        &fixture.request.tools,
        &fixture.request.native_tools,
    )
    .unwrap();
    assert_json_eq(&request, &fixture.request.expected_provider_request);

    let response = OpenAiResponsesAdapter::parse_response(&fixture.provider_response).unwrap();
    assert_eq!(
        legacy_openai_response_projection(&response),
        fixture.expected_response
    );
    response
}

fn legacy_openai_response_projection(
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
    let request = AnthropicMessagesAdapter::build_request(
        &fixture.request.model,
        &fixture.request.history,
        fixture.request.settings.as_ref(),
        &fixture.request.tools,
    )
    .unwrap();
    assert_json_eq(&request, &fixture.request.expected_provider_request);

    let response = AnthropicMessagesAdapter::parse_response(&fixture.provider_response).unwrap();
    assert_eq!(response, fixture.expected_response);
    response
}

fn assert_gemini_fixture(name: &str) -> starweaver_model::ModelResponse {
    let fixture = support::replay::load_replay_fixture("gemini", name);
    let request = GeminiGenerateContentAdapter::build_request_with_native_tools(
        &fixture.request.history,
        fixture.request.settings.as_ref(),
        &fixture.request.tools,
        &fixture.request.native_tools,
    )
    .unwrap();
    assert_json_eq(&request, &fixture.request.expected_provider_request);

    let response =
        GeminiGenerateContentAdapter::parse_response(&fixture.provider_response).unwrap();
    assert_eq!(response, fixture.expected_response);
    response
}

fn assert_bedrock_fixture(name: &str) -> starweaver_model::ModelResponse {
    let fixture = support::replay::load_replay_fixture("bedrock", name);
    let request = BedrockConverseAdapter::build_request(
        &fixture.request.model,
        &fixture.request.history,
        fixture.request.settings.as_ref(),
        &fixture.request.tools,
    )
    .unwrap();
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
