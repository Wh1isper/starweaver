use async_trait::async_trait;
use serde_json::{json, Value};

use crate::{
    adapter::{allow_real_model_requests, ModelRequestContext, ModelRequestParameters},
    message::{ModelMessage, ModelResponse},
    profile::{ModelProfile, ProtocolFamily},
    request::prepare_model_request,
    settings::{ModelSettings, ResponseStreamTransport},
    transport::{
        build_http_request, send_with_retries, should_fallback_websocket_to_http, HttpRequest,
        ModelEventStream,
    },
    ModelAdapter, ModelError, ModelResponseEventStream, ModelResponseStreamEvent, StreamDiagnostic,
};

use super::ProtocolModelClient;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResolvedResponseStreamTransport {
    HttpOnly,
    WebSocketOnly,
    WebSocketThenHttpOnRetryable,
}

#[async_trait]
impl ModelAdapter for ProtocolModelClient {
    fn model_name(&self) -> &str {
        &self.model_name
    }

    fn provider_name(&self) -> Option<&str> {
        Some(&self.provider_name)
    }

    fn profile(&self) -> &ModelProfile {
        &self.profile
    }

    fn default_settings(&self) -> Option<&ModelSettings> {
        self.default_settings.as_ref()
    }

    async fn request(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        let prepared = prepare_model_request(
            messages,
            self.default_settings.as_ref(),
            settings,
            params,
            &self.profile,
        );
        let wire_body = self.build_wire_body(
            &prepared.normalized_messages,
            prepared.settings.as_ref(),
            &prepared.params,
        )?;
        let options = self.request_options(&context, prepared.settings.as_ref(), &prepared.params);
        let mut request = build_http_request(&self.http_config, &options, wire_body);
        request.cancellation_token = context.cancellation_token();
        self.finalize_http_request(&mut request);
        if let Some(audit) = self.request_audit.as_ref() {
            audit.record(&self.provider_name, &self.model_name, false, &request);
        }
        if !allow_real_model_requests() {
            return Err(ModelError::RealModelRequestBlocked { url: request.url });
        }
        let response = send_with_retries(
            self.http_client.as_ref(),
            self.sleeper.as_ref(),
            request,
            &self.http_config.retry_policy,
        )
        .await?;
        self.parse_wire_response(&response.body)
    }

    async fn request_stream(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        let mut stream = self
            .request_stream_incremental(messages, settings, params, context)
            .await?;
        let mut events = Vec::new();
        while let Some(event) = stream.recv().await {
            events.push(event?);
        }
        Ok(events)
    }

    async fn request_stream_incremental(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponseEventStream, ModelError> {
        let cancellation_token = context.cancellation_token();
        if self.profile.protocol != ProtocolFamily::OpenAiResponses {
            let response = self.request(messages, settings, params, context).await?;
            let (sender, receiver) = tokio::sync::mpsc::channel(1);
            let _ = sender
                .send(Ok(ModelResponseStreamEvent::FinalResult(Box::new(
                    response,
                ))))
                .await;
            return Ok(ModelResponseEventStream::new_with_cancellation(
                receiver,
                cancellation_token,
            ));
        }
        let prepared = prepare_model_request(
            messages,
            self.default_settings.as_ref(),
            settings,
            params,
            &self.profile,
        );
        let wire_body = self.build_wire_body(
            &prepared.normalized_messages,
            prepared.settings.as_ref(),
            &prepared.params,
        )?;
        let options = self.request_options(&context, prepared.settings.as_ref(), &prepared.params);
        let transport = self.resolve_response_stream_transport(prepared.settings.as_ref());
        match transport {
            ResolvedResponseStreamTransport::HttpOnly => {
                let http_request = self.build_response_stream_request(
                    wire_body,
                    &options,
                    cancellation_token,
                    ResponseStreamRequestKind::HttpSse,
                );
                Self::ensure_real_model_request_allowed(&http_request)?;
                self.openai_response_stream_from_request(
                    http_request,
                    ResponseStreamRequestKind::HttpSse,
                )
                .await
            }
            ResolvedResponseStreamTransport::WebSocketOnly => {
                let websocket_request = self.build_response_stream_request(
                    wire_body,
                    &options,
                    cancellation_token,
                    ResponseStreamRequestKind::WebSocket,
                );
                Self::ensure_real_model_request_allowed(&websocket_request)?;
                self.openai_response_stream_from_request(
                    websocket_request,
                    ResponseStreamRequestKind::WebSocket,
                )
                .await
            }
            ResolvedResponseStreamTransport::WebSocketThenHttpOnRetryable => {
                let websocket_request = self.build_response_stream_request(
                    wire_body.clone(),
                    &options,
                    cancellation_token.clone(),
                    ResponseStreamRequestKind::WebSocket,
                );
                Self::ensure_real_model_request_allowed(&websocket_request)?;
                let http_request = self.build_response_stream_request(
                    wire_body,
                    &options,
                    cancellation_token.clone(),
                    ResponseStreamRequestKind::HttpSse,
                );
                Ok(self.openai_response_stream_with_fallback(
                    websocket_request,
                    http_request,
                    cancellation_token,
                ))
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ResponseStreamRequestKind {
    HttpSse,
    WebSocket,
}

impl ProtocolModelClient {
    fn resolve_response_stream_transport(
        &self,
        settings: Option<&ModelSettings>,
    ) -> ResolvedResponseStreamTransport {
        let configured_transport = settings
            .and_then(|settings| settings.provider_settings.openai_responses.as_ref())
            .and_then(|settings| settings.stream_transport);
        match configured_transport {
            Some(ResponseStreamTransport::WebSocket) => {
                ResolvedResponseStreamTransport::WebSocketOnly
            }
            Some(ResponseStreamTransport::Auto) => {
                ResolvedResponseStreamTransport::WebSocketThenHttpOnRetryable
            }
            None if self.is_codex_oauth_provider() => {
                ResolvedResponseStreamTransport::WebSocketThenHttpOnRetryable
            }
            Some(ResponseStreamTransport::Http) | None => ResolvedResponseStreamTransport::HttpOnly,
        }
    }

    fn is_codex_oauth_provider(&self) -> bool {
        self.provider_name == "codex"
            || self
                .http_config
                .metadata
                .get("oauth_provider")
                .and_then(Value::as_str)
                .is_some_and(|provider| provider == "codex")
    }

    fn build_response_stream_request(
        &self,
        wire_body: Value,
        options: &crate::transport::HttpRequestOptions,
        cancellation_token: starweaver_core::CancellationToken,
        kind: ResponseStreamRequestKind,
    ) -> HttpRequest {
        let body = match kind {
            ResponseStreamRequestKind::HttpSse => response_http_sse_body(wire_body),
            ResponseStreamRequestKind::WebSocket => response_websocket_body(wire_body),
        };
        let mut request = build_http_request(&self.http_config, options, body);
        request.cancellation_token = cancellation_token;
        match kind {
            ResponseStreamRequestKind::HttpSse => {
                request.metadata.insert(
                    "starweaver.response_stream_transport".to_string(),
                    json!("http"),
                );
            }
            ResponseStreamRequestKind::WebSocket => {
                request.metadata.insert(
                    "starweaver.response_stream_transport".to_string(),
                    json!("websocket"),
                );
            }
        }
        self.finalize_http_request(&mut request);
        request
    }

    fn ensure_real_model_request_allowed(request: &HttpRequest) -> Result<(), ModelError> {
        if allow_real_model_requests() {
            Ok(())
        } else {
            Err(ModelError::RealModelRequestBlocked {
                url: request.url.clone(),
            })
        }
    }

    fn record_stream_request_audit(&self, request: &HttpRequest) {
        if let Some(audit) = self.request_audit.as_ref() {
            audit.record(&self.provider_name, &self.model_name, true, request);
        }
    }

    async fn openai_response_stream_from_request(
        &self,
        request: HttpRequest,
        kind: ResponseStreamRequestKind,
    ) -> Result<ModelResponseEventStream, ModelError> {
        let cancellation_token = request.cancellation_token.clone();
        let diagnostic = matches!(kind, ResponseStreamRequestKind::WebSocket).then(|| {
            transport_selected_diagnostic(
                &self.provider_name,
                &self.model_name,
                transport_name(kind),
            )
        });
        self.record_stream_request_audit(&request);
        let events = match kind {
            ResponseStreamRequestKind::HttpSse => {
                self.http_client
                    .send_event_stream_incremental(request)
                    .await?
            }
            ResponseStreamRequestKind::WebSocket => {
                self.http_client
                    .send_websocket_event_stream_incremental(request)
                    .await?
            }
        };
        Ok(canonical_openai_response_stream(
            events,
            cancellation_token,
            diagnostic,
        ))
    }

    fn openai_response_stream_with_fallback(
        &self,
        websocket_request: HttpRequest,
        http_request: HttpRequest,
        cancellation_token: starweaver_core::CancellationToken,
    ) -> ModelResponseEventStream {
        let http_client = self.http_client.clone();
        let provider_name = self.provider_name.clone();
        let model_name = self.model_name.clone();
        let request_audit = self.request_audit.clone();
        let (sender, receiver) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            let mut emitted_any_event = false;
            let _ = sender
                .send(Ok(ModelResponseStreamEvent::Diagnostic(
                    transport_selected_diagnostic(&provider_name, &model_name, "websocket"),
                )))
                .await;
            if let Some(audit) = request_audit.as_ref() {
                audit.record(&provider_name, &model_name, true, &websocket_request);
            }
            let websocket_result = match http_client
                .send_websocket_event_stream_incremental(websocket_request)
                .await
            {
                Ok(events) => {
                    forward_openai_response_events(events, &sender, &mut emitted_any_event).await
                }
                Err(error) => Err(error),
            };
            match websocket_result {
                Ok(()) => {}
                Err(error) if !emitted_any_event && should_fallback_websocket_to_http(&error) => {
                    let _ = sender
                        .send(Ok(ModelResponseStreamEvent::Diagnostic(
                            transport_fallback_diagnostic(&provider_name, &model_name, &error),
                        )))
                        .await;
                    if let Some(audit) = request_audit.as_ref() {
                        audit.record(&provider_name, &model_name, true, &http_request);
                    }
                    match http_client
                        .send_event_stream_incremental(http_request)
                        .await
                    {
                        Ok(events) => {
                            if let Err(error) = forward_openai_response_events(
                                events,
                                &sender,
                                &mut emitted_any_event,
                            )
                            .await
                            {
                                let _ = sender.send(Err(error)).await;
                            }
                        }
                        Err(error) => {
                            let _ = sender.send(Err(error)).await;
                        }
                    }
                }
                Err(error) => {
                    let _ = sender.send(Err(error)).await;
                }
            }
        });
        ModelResponseEventStream::new_with_cancellation(receiver, cancellation_token)
    }
}

fn response_http_sse_body(mut body: Value) -> Value {
    if let Some(object) = body.as_object_mut() {
        object.insert("stream".to_string(), Value::Bool(true));
    }
    body
}

fn response_websocket_body(body: Value) -> Value {
    let mut envelope = serde_json::Map::new();
    if let Value::Object(mut object) = body {
        object.remove("background");
        envelope.extend(object);
    }
    envelope.insert(
        "type".to_string(),
        Value::String("response.create".to_string()),
    );
    envelope.insert("stream".to_string(), Value::Bool(true));
    Value::Object(envelope)
}

const fn transport_name(kind: ResponseStreamRequestKind) -> &'static str {
    match kind {
        ResponseStreamRequestKind::HttpSse => "http",
        ResponseStreamRequestKind::WebSocket => "websocket",
    }
}

fn transport_selected_diagnostic(
    provider_name: &str,
    model_name: &str,
    transport: &str,
) -> StreamDiagnostic {
    StreamDiagnostic::new(
        "model_transport_selected",
        json!({
            "provider": provider_name,
            "model": model_name,
            "transport": transport,
            "message": format!("model transport: {transport}"),
        }),
    )
}

fn transport_fallback_diagnostic(
    provider_name: &str,
    model_name: &str,
    error: &ModelError,
) -> StreamDiagnostic {
    StreamDiagnostic::new(
        "model_transport_fallback",
        json!({
            "provider": provider_name,
            "model": model_name,
            "from": "websocket",
            "to": "http",
            "reason": transport_fallback_reason(error),
            "message": format!(
                "model transport: websocket -> http fallback ({})",
                transport_fallback_reason(error)
            ),
        }),
    )
}

fn transport_fallback_reason(error: &ModelError) -> &'static str {
    match error {
        ModelError::ProviderStatus { body, .. } if websocket_connection_limit_reached(body) => {
            "websocket_connection_limit_reached"
        }
        ModelError::ProviderStatus { .. } => "provider_status",
        ModelError::Transport(_) => "websocket_transport_error",
        ModelError::Cancelled { .. } => "cancelled",
        ModelError::MessageMapping(_)
        | ModelError::ResponseParsing(_)
        | ModelError::RealModelRequestBlocked { .. }
        | ModelError::RetryExhausted { .. }
        | ModelError::UnsupportedResponse(_) => "model_error",
    }
}

fn websocket_connection_limit_reached(body: &Value) -> bool {
    body.get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .is_some_and(|code| code == "websocket_connection_limit_reached")
        || body
            .get("code")
            .and_then(Value::as_str)
            .is_some_and(|code| code == "websocket_connection_limit_reached")
}

fn canonical_openai_response_stream(
    events: ModelEventStream,
    cancellation_token: starweaver_core::CancellationToken,
    diagnostic: Option<StreamDiagnostic>,
) -> ModelResponseEventStream {
    let (sender, receiver) = tokio::sync::mpsc::channel(32);
    tokio::spawn(async move {
        if let Some(diagnostic) = diagnostic {
            if sender
                .send(Ok(ModelResponseStreamEvent::Diagnostic(diagnostic)))
                .await
                .is_err()
            {
                return;
            }
        }
        let mut emitted_any_event = false;
        if let Err(error) =
            forward_openai_response_events(events, &sender, &mut emitted_any_event).await
        {
            let _ = sender.send(Err(error)).await;
        }
    });
    ModelResponseEventStream::new_with_cancellation(receiver, cancellation_token)
}

async fn forward_openai_response_events(
    mut events: ModelEventStream,
    sender: &tokio::sync::mpsc::Sender<Result<ModelResponseStreamEvent, ModelError>>,
    emitted_any_event: &mut bool,
) -> Result<(), ModelError> {
    let mut parser = crate::providers::openai_responses::OpenAiResponsesStreamParser::default();
    while let Some(event) = events.recv().await {
        let event = event?;
        let stream_events = parser.push_event(&event)?;
        for stream_event in stream_events {
            if sender.send(Ok(stream_event)).await.is_err() {
                return Ok(());
            }
            *emitted_any_event = true;
        }
    }
    for stream_event in parser.finish()? {
        if sender.send(Ok(stream_event)).await.is_err() {
            return Ok(());
        }
        *emitted_any_event = true;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use serde_json::{json, Value};
    use starweaver_core::{ConversationId, RunId};

    use super::*;
    use crate::{
        adapter::ModelAdapter,
        message::ModelRequest,
        profile::ProtocolFamily,
        settings::{OpenAiResponsesSettings, ProviderSettings},
        transport::{HttpModelConfig, HttpResponse, ModelHttpClient},
    };

    #[test]
    fn websocket_body_wraps_response_create_and_forces_stream_true() {
        let body = response_websocket_body(json!({
            "model": "gpt-5-codex",
            "input": [{"role": "user", "content": "hello"}],
            "stream": false,
            "background": false,
            "store": false
        }));

        assert_eq!(
            body,
            json!({
                "type": "response.create",
                "model": "gpt-5-codex",
                "input": [{"role": "user", "content": "hello"}],
                "stream": true,
                "store": false
            })
        );
    }

    #[test]
    fn resolves_transport_defaults_and_explicit_settings() {
        let native = test_client("openai", None, Arc::new(FakeStreamClient::default()));
        assert_eq!(
            native.resolve_response_stream_transport(None),
            ResolvedResponseStreamTransport::HttpOnly
        );

        let codex_by_name = test_client("codex", None, Arc::new(FakeStreamClient::default()));
        assert_eq!(
            codex_by_name.resolve_response_stream_transport(None),
            ResolvedResponseStreamTransport::WebSocketThenHttpOnRetryable
        );

        let codex_by_metadata = test_client(
            "openai",
            Some(json!({"oauth_provider": "codex"})),
            Arc::new(FakeStreamClient::default()),
        );
        assert_eq!(
            codex_by_metadata.resolve_response_stream_transport(None),
            ResolvedResponseStreamTransport::WebSocketThenHttpOnRetryable
        );

        let explicit_http = settings_with_transport(ResponseStreamTransport::Http);
        assert_eq!(
            codex_by_name.resolve_response_stream_transport(Some(&explicit_http)),
            ResolvedResponseStreamTransport::HttpOnly
        );

        let explicit_websocket = settings_with_transport(ResponseStreamTransport::WebSocket);
        assert_eq!(
            native.resolve_response_stream_transport(Some(&explicit_websocket)),
            ResolvedResponseStreamTransport::WebSocketOnly
        );

        let explicit_auto = settings_with_transport(ResponseStreamTransport::Auto);
        assert_eq!(
            native.resolve_response_stream_transport(Some(&explicit_auto)),
            ResolvedResponseStreamTransport::WebSocketThenHttpOnRetryable
        );
    }

    #[tokio::test]
    async fn auto_transport_falls_back_to_http_for_pre_event_retryable_websocket_error() {
        let fake = Arc::new(FakeStreamClient::new(
            WebSocketBehavior::ImmediateConnectionLimit,
            vec![completed_text_event("from http")],
        ));
        let client = test_client("codex", None, fake.clone());

        let stream = client
            .request_stream_incremental(
                vec![ModelMessage::Request(ModelRequest::user_text("hello"))],
                None,
                ModelRequestParameters::default(),
                test_context(),
            )
            .await;
        let mut stream = result_or_panic(stream, "stream should be created");

        let mut final_text = None;
        while let Some(event) = stream.recv().await {
            if let ModelResponseStreamEvent::FinalResult(response) =
                result_or_panic(event, "event should parse")
            {
                final_text = Some(response.text_output());
            }
        }

        assert_eq!(final_text.as_deref(), Some("from http"));
        assert_eq!(
            fake.calls(),
            vec![FakeCallKind::WebSocket, FakeCallKind::Http]
        );
        let bodies = fake.bodies();
        assert_eq!(
            bodies[0].get("type").and_then(Value::as_str),
            Some("response.create")
        );
        assert_eq!(bodies[1].get("stream"), Some(&Value::Bool(true)));
    }

    #[tokio::test]
    async fn explicit_websocket_transport_does_not_fallback() {
        let fake = Arc::new(FakeStreamClient::new(
            WebSocketBehavior::ImmediateConnectionLimit,
            vec![completed_text_event("from http")],
        ));
        let client = test_client("codex", None, fake.clone());
        let settings = settings_with_transport(ResponseStreamTransport::WebSocket);

        let result = client
            .request_stream_incremental(
                vec![ModelMessage::Request(ModelRequest::user_text("hello"))],
                Some(settings),
                ModelRequestParameters::default(),
                test_context(),
            )
            .await;

        assert!(matches!(
            result,
            Err(ModelError::ProviderStatus {
                status: 400,
                retryable: true,
                ..
            })
        ));
        assert_eq!(fake.calls(), vec![FakeCallKind::WebSocket]);
    }

    #[tokio::test]
    async fn websocket_error_after_canonical_event_is_not_fallback_safe() {
        let fake = Arc::new(FakeStreamClient::new(
            WebSocketBehavior::TextThenConnectionLimit,
            vec![completed_text_event("from http")],
        ));
        let client = test_client("codex", None, fake.clone());

        let stream = client
            .request_stream_incremental(
                vec![ModelMessage::Request(ModelRequest::user_text("hello"))],
                None,
                ModelRequestParameters::default(),
                test_context(),
            )
            .await;
        let mut stream = result_or_panic(stream, "stream should be created");

        let first = next_non_diagnostic(&mut stream).await;
        assert!(matches!(first, ModelResponseStreamEvent::PartStart(_)));
        let second = next_non_diagnostic(&mut stream).await;
        assert!(matches!(
            second,
            ModelResponseStreamEvent::PartDelta(crate::PartDelta {
                delta: crate::StreamDelta::Text { .. },
                ..
            })
        ));
        let error = option_or_panic(stream.recv().await, "expected websocket error");
        assert!(matches!(
            error,
            Err(ModelError::ProviderStatus {
                status: 400,
                retryable: true,
                ..
            })
        ));
        assert_eq!(fake.calls(), vec![FakeCallKind::WebSocket]);
    }

    fn test_client(
        provider_name: &str,
        metadata: Option<Value>,
        http_client: Arc<FakeStreamClient>,
    ) -> ProtocolModelClient {
        let mut config = HttpModelConfig::new("https://api.openai.com/v1", "responses");
        if let Some(Value::Object(metadata)) = metadata {
            config.metadata = metadata;
        }
        ProtocolModelClient::new(
            provider_name,
            "gpt-5-codex",
            ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
            config,
            http_client,
        )
    }

    fn settings_with_transport(transport: ResponseStreamTransport) -> ModelSettings {
        ModelSettings {
            provider_settings: ProviderSettings {
                openai_responses: Some(OpenAiResponsesSettings {
                    stream_transport: Some(transport),
                    ..OpenAiResponsesSettings::default()
                }),
                ..ProviderSettings::default()
            },
            ..ModelSettings::default()
        }
    }

    fn test_context() -> ModelRequestContext {
        ModelRequestContext::new(RunId::new(), ConversationId::new())
    }

    fn completed_text_event(text: &str) -> Value {
        json!({
            "type": "response.completed",
            "response": {
                "id": "resp_test",
                "model": "gpt-5-codex",
                "status": "completed",
                "output": [{
                    "id": "msg_test",
                    "type": "message",
                    "content": [{"type": "output_text", "text": text}]
                }],
                "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
            }
        })
    }

    fn text_delta_event(text: &str) -> Value {
        json!({
            "type": "response.output_text.delta",
            "delta": text
        })
    }

    fn connection_limit_error() -> ModelError {
        ModelError::ProviderStatus {
            status: 400,
            body: json!({"error": {"code": "websocket_connection_limit_reached"}}),
            retryable: true,
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum FakeCallKind {
        Http,
        WebSocket,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum WebSocketBehavior {
        ImmediateConnectionLimit,
        TextThenConnectionLimit,
    }

    #[derive(Debug)]
    struct FakeStreamClient {
        websocket_behavior: WebSocketBehavior,
        http_events: Vec<Value>,
        calls: Mutex<Vec<FakeCallKind>>,
        bodies: Mutex<Vec<Value>>,
    }

    impl Default for FakeStreamClient {
        fn default() -> Self {
            Self::new(WebSocketBehavior::ImmediateConnectionLimit, Vec::new())
        }
    }

    impl FakeStreamClient {
        fn new(websocket_behavior: WebSocketBehavior, http_events: Vec<Value>) -> Self {
            Self {
                websocket_behavior,
                http_events,
                calls: Mutex::new(Vec::new()),
                bodies: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<FakeCallKind> {
            lock_or_panic(self.calls.lock(), "calls lock should not be poisoned").clone()
        }

        fn bodies(&self) -> Vec<Value> {
            lock_or_panic(self.bodies.lock(), "bodies lock should not be poisoned").clone()
        }

        fn record(&self, kind: FakeCallKind, request: &HttpRequest) {
            lock_or_panic(self.calls.lock(), "calls lock should not be poisoned").push(kind);
            lock_or_panic(self.bodies.lock(), "bodies lock should not be poisoned")
                .push(request.body.clone());
        }
    }

    #[async_trait]
    impl ModelHttpClient for FakeStreamClient {
        async fn send(&self, _request: HttpRequest) -> Result<HttpResponse, ModelError> {
            Err(ModelError::Transport(
                "send is not used by stream transport tests".to_string(),
            ))
        }

        async fn send_event_stream_incremental(
            &self,
            request: HttpRequest,
        ) -> Result<ModelEventStream, ModelError> {
            self.record(FakeCallKind::Http, &request);
            Ok(stream_from_results(
                self.http_events.iter().cloned().map(Ok).collect(),
            ))
        }

        async fn send_websocket_event_stream_incremental(
            &self,
            request: HttpRequest,
        ) -> Result<ModelEventStream, ModelError> {
            self.record(FakeCallKind::WebSocket, &request);
            match self.websocket_behavior {
                WebSocketBehavior::ImmediateConnectionLimit => Err(connection_limit_error()),
                WebSocketBehavior::TextThenConnectionLimit => Ok(stream_from_results(vec![
                    Ok(text_delta_event("partial")),
                    Err(connection_limit_error()),
                ])),
            }
        }
    }

    fn result_or_panic<T, E: std::fmt::Debug>(result: Result<T, E>, message: &str) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("{message}: {error:?}"),
        }
    }

    async fn next_non_diagnostic(
        stream: &mut ModelResponseEventStream,
    ) -> ModelResponseStreamEvent {
        while let Some(event) = stream.recv().await {
            let event = result_or_panic(event, "event should parse");
            if matches!(event, ModelResponseStreamEvent::Diagnostic(_)) {
                continue;
            }
            return event;
        }
        panic!("expected non-diagnostic stream event")
    }

    fn option_or_panic<T>(value: Option<T>, message: &str) -> T {
        value.unwrap_or_else(|| panic!("{message}"))
    }

    fn lock_or_panic<'a, T>(
        lock: std::sync::LockResult<std::sync::MutexGuard<'a, T>>,
        message: &str,
    ) -> std::sync::MutexGuard<'a, T> {
        lock.unwrap_or_else(|error| panic!("{message}: {error}"))
    }

    fn stream_from_results(events: Vec<Result<Value, ModelError>>) -> ModelEventStream {
        let (sender, receiver) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            for event in events {
                if sender.send(event).await.is_err() {
                    break;
                }
            }
        });
        ModelEventStream::new(receiver)
    }
}
