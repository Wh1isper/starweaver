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
    ModelResponsePart,
};

fn assert_json_eq(actual: &serde_json::Value, expected: &serde_json::Value) {
    support::replay::assert_json_eq(actual, expected);
}

fn assert_openai_chat_fixture(name: &str) -> starweaver_model::ModelResponse {
    let fixture = support::replay::load_replay_fixture("openai_chat", name);
    let request = OpenAiChatAdapter::build_request(
        &fixture.request.model,
        &fixture.request.history,
        fixture.request.settings.as_ref(),
        &fixture.request.tools,
    )
    .unwrap();
    assert_json_eq(&request, &fixture.request.expected_provider_request);
    assert_eq!(
        fixture.request.request_parameters.tools,
        fixture.request.tools
    );

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
    assert_eq!(response, fixture.expected_response);
    response
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
    let request = GeminiGenerateContentAdapter::build_request(
        &fixture.request.history,
        fixture.request.settings.as_ref(),
        &fixture.request.tools,
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
    assert!(matches!(response.parts[1], ModelResponsePart::ToolCall(_)));
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
