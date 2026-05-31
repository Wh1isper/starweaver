#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_model::{
    latest_user_text, tool_call_response, FunctionModel, ModelAdapter, ModelError, ModelMessage,
    ModelProfile, ModelRequestContext, ModelRequestParameters, ModelResponse, ModelResponsePart,
    ModelResponseStreamEvent, ModelSettings, PartDelta, PartEnd, PartStart, ProtocolFamily,
    TestModel,
};

fn context() -> ModelRequestContext {
    ModelRequestContext::new(
        starweaver_core::RunId::from_string("run_test"),
        starweaver_core::ConversationId::from_string("conv_test"),
    )
}

#[tokio::test]
async fn test_model_returns_scripted_responses_and_captures_requests() {
    let model = TestModel::with_responses(vec![ModelResponse::text("first")]);

    let response = model
        .request(
            vec![ModelMessage::Request(
                starweaver_model::ModelRequest::user_text("hello"),
            )],
            None,
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    assert_eq!(response.text_output(), "first");
    assert_eq!(model.captured_messages().len(), 1);
    assert_eq!(
        latest_user_text(&model.captured_messages()[0]).unwrap(),
        "hello"
    );
}

#[tokio::test]
async fn test_model_streams_scripted_events_and_captures_requests() {
    let model = TestModel::with_stream_events(vec![vec![
        ModelResponseStreamEvent::PartStart(PartStart {
            index: 0,
            part_kind: "text".to_string(),
        }),
        ModelResponseStreamEvent::PartDelta(PartDelta {
            index: 0,
            delta: "stream".to_string(),
        }),
        ModelResponseStreamEvent::PartEnd(PartEnd { index: 0 }),
        ModelResponseStreamEvent::FinalResult(Box::new(ModelResponse::text("stream"))),
    ]]);

    let events = model
        .request_stream(
            vec![ModelMessage::Request(
                starweaver_model::ModelRequest::user_text("hello"),
            )],
            None,
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    assert!(matches!(events[1], ModelResponseStreamEvent::PartDelta(_)));
    assert!(matches!(
        events.last().unwrap(),
        ModelResponseStreamEvent::FinalResult(response) if response.text_output() == "stream"
    ));
    assert_eq!(model.captured_messages().len(), 1);
}

#[tokio::test]
async fn test_model_request_stream_falls_back_to_scripted_response_final_result() {
    let model = TestModel::with_responses(vec![ModelResponse::text("final")]);

    let events = model
        .request_stream(
            vec![ModelMessage::Request(
                starweaver_model::ModelRequest::user_text("hello"),
            )],
            None,
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    assert_eq!(
        events,
        vec![ModelResponseStreamEvent::FinalResult(Box::new(
            ModelResponse::text("final")
        ))]
    );
}

#[tokio::test]
async fn function_model_builds_responses_from_messages_and_params() {
    let model = FunctionModel::new(|messages, settings, info| {
        assert_eq!(latest_user_text(&messages).unwrap(), "hello");
        assert_eq!(settings.unwrap().temperature, Some(0.2));
        assert_eq!(info.params.output_schema.unwrap()["name"], "answer");
        Ok(ModelResponse::text(r#"{"answer":"ok"}"#))
    });

    let response = model
        .request(
            vec![ModelMessage::Request(
                starweaver_model::ModelRequest::user_text("hello"),
            )],
            Some(ModelSettings {
                temperature: Some(0.2),
                ..ModelSettings::default()
            }),
            ModelRequestParameters {
                output_schema: Some(serde_json::json!({"name": "answer"})),
                ..ModelRequestParameters::default()
            },
            context(),
        )
        .await
        .unwrap();

    assert_eq!(response.text_output(), r#"{"answer":"ok"}"#);
    assert_eq!(model.captured_params().len(), 1);
}

#[tokio::test]
async fn function_model_streams_events_from_messages_and_params() {
    let model = FunctionModel::streaming(|messages, settings, info| {
        assert_eq!(latest_user_text(&messages).unwrap(), "hello");
        assert_eq!(settings.unwrap().max_tokens, Some(32));
        assert_eq!(info.params.extra_body["mode"], "stream");
        Ok(vec![
            ModelResponseStreamEvent::PartStart(PartStart {
                index: 0,
                part_kind: "text".to_string(),
            }),
            ModelResponseStreamEvent::PartDelta(PartDelta {
                index: 0,
                delta: "ok".to_string(),
            }),
            ModelResponseStreamEvent::PartEnd(PartEnd { index: 0 }),
            ModelResponseStreamEvent::FinalResult(Box::new(ModelResponse::text("ok"))),
        ])
    });
    let mut params = ModelRequestParameters::default();
    params
        .extra_body
        .insert("mode".to_string(), serde_json::json!("stream"));

    let events = model
        .request_stream(
            vec![ModelMessage::Request(
                starweaver_model::ModelRequest::user_text("hello"),
            )],
            Some(ModelSettings {
                max_tokens: Some(32),
                ..ModelSettings::default()
            }),
            params,
            context(),
        )
        .await
        .unwrap();

    assert!(matches!(events[1], ModelResponseStreamEvent::PartDelta(_)));
    assert_eq!(model.captured_params().len(), 1);
}

#[tokio::test]
async fn helper_builds_tool_call_response() {
    let response = tool_call_response("call_1", "lookup", serde_json::json!({"query": "Paris"}));

    assert!(matches!(
        &response.parts[0],
        ModelResponsePart::ToolCall(call) if call.name == "lookup"
    ));
}

#[tokio::test]
async fn test_model_allows_profile_and_settings_defaults() {
    let settings = ModelSettings {
        max_tokens: Some(16),
        ..ModelSettings::default()
    };
    let model = TestModel::new()
        .with_model_name("unit")
        .with_profile(ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses))
        .with_default_settings(settings.clone());
    let adapter: Arc<dyn ModelAdapter> = Arc::new(model);

    assert_eq!(adapter.model_name(), "unit");
    assert_eq!(adapter.profile().protocol, ProtocolFamily::OpenAiResponses);
    assert_eq!(adapter.default_settings(), Some(&settings));
}

#[tokio::test]
async fn function_model_can_return_errors() {
    let model = FunctionModel::new(|_messages, _settings, _info| {
        Err(ModelError::Transport("blocked".to_string()))
    });

    let error = model
        .request(
            Vec::new(),
            None,
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, ModelError::Transport(message) if message == "blocked"));
}
