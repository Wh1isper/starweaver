use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use crate::{
    adapter::{allow_real_model_requests, ModelRequestContext, ModelRequestParameters},
    message::{ModelMessage, ModelResponse},
    profile::{ModelProfile, ProtocolFamily},
    request::prepare_model_request,
    settings::{ModelSettings, ResponseStreamTransport},
    transport::{
        build_http_request, send_with_retries, should_fallback_websocket_to_http, HttpRequest,
        ModelEventStream, ModelWebSocketEventSession,
    },
    ModelAdapter, ModelError, ModelResponseEventStream, ModelResponseStreamEvent, ModelRunSession,
    StreamDiagnostic,
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

    fn start_run_session(&self) -> Box<dyn ModelRunSession + '_> {
        Box::new(ProtocolModelClientRunSession::new(self))
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
        let mut session = ProtocolModelClientRunSession::new(self);
        session
            .request_stream_incremental(messages, settings, params, context)
            .await
    }
}

#[derive(Clone, Copy, Debug)]
enum ResponseStreamRequestKind {
    HttpSse,
    WebSocket,
}

struct ProtocolModelClientRunSession<'a> {
    client: &'a ProtocolModelClient,
    websocket_session: Box<dyn ModelWebSocketEventSession + 'a>,
    last_websocket_request: Option<Value>,
    last_response: Arc<Mutex<LastResponseSlot>>,
    fallback_to_http: Arc<AtomicBool>,
}

#[derive(Clone, Debug)]
struct LastResponse {
    response_id: String,
    replay_items: Vec<Value>,
}

#[derive(Debug, Default)]
struct LastResponseSlot {
    response: Option<LastResponse>,
    failed: bool,
}

struct SessionFallbackPlan {
    http_client: crate::transport::DynHttpClient,
    request_audit: Option<crate::transport::ProviderRequestAuditCapture>,
    provider_name: String,
    model_name: String,
    http_request: HttpRequest,
    fallback_to_http: Arc<AtomicBool>,
    last_response: Arc<Mutex<LastResponseSlot>>,
}

impl<'a> ProtocolModelClientRunSession<'a> {
    fn new(client: &'a ProtocolModelClient) -> Self {
        Self {
            client,
            websocket_session: client.http_client.websocket_event_session(),
            last_websocket_request: None,
            last_response: Arc::new(Mutex::new(LastResponseSlot::default())),
            fallback_to_http: Arc::new(AtomicBool::new(false)),
        }
    }

    fn fallback_plan(&self, http_request: HttpRequest) -> SessionFallbackPlan {
        SessionFallbackPlan {
            http_client: self.client.http_client.clone(),
            request_audit: self.client.request_audit.clone(),
            provider_name: self.client.provider_name.clone(),
            model_name: self.client.model_name.clone(),
            http_request,
            fallback_to_http: Arc::clone(&self.fallback_to_http),
            last_response: Arc::clone(&self.last_response),
        }
    }

    fn required_fallback_plan(
        &self,
        http_request: Option<HttpRequest>,
    ) -> Result<SessionFallbackPlan, ModelError> {
        http_request
            .map(|request| self.fallback_plan(request))
            .ok_or_else(|| {
                ModelError::Transport("HTTP request is required for websocket fallback".to_string())
            })
    }

    fn current_last_response(&mut self) -> Option<LastResponse> {
        let mut slot = self
            .last_response
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.failed {
            slot.failed = false;
            slot.response = None;
            self.last_websocket_request = None;
            return None;
        }
        slot.response.clone()
    }

    fn prepare_websocket_request_body(&mut self, logical_body: &Value) -> Value {
        let Some(last_response) = self.current_last_response() else {
            return logical_body.clone();
        };
        let Some(incremental_items) =
            self.websocket_incremental_items(logical_body, &last_response)
        else {
            return logical_body.clone();
        };
        if last_response.response_id.is_empty() {
            return logical_body.clone();
        }
        let mut body = logical_body.clone();
        if let Some(object) = body.as_object_mut() {
            object.insert(
                "previous_response_id".to_string(),
                json!(last_response.response_id),
            );
            object.insert("input".to_string(), Value::Array(incremental_items));
        }
        body
    }

    fn websocket_incremental_items(
        &self,
        current: &Value,
        last_response: &LastResponse,
    ) -> Option<Vec<Value>> {
        if current.get("previous_response_id").is_some() || current.get("conversation").is_some() {
            return None;
        }
        let previous = self.last_websocket_request.as_ref()?;
        if !responses_websocket_request_properties_match(previous, current) {
            return None;
        }
        let previous_input = response_input_items(previous)?;
        let current_input = response_input_items(current)?;
        let after_previous = strip_value_prefix(current_input, previous_input)?;
        let incremental = strip_value_prefix(after_previous, &last_response.replay_items)?;
        Some(incremental.to_vec())
    }

    async fn request_websocket_stream(
        &mut self,
        mut websocket_request: HttpRequest,
        http_request: Option<HttpRequest>,
        request_settings: Option<ModelSettings>,
        fallback: bool,
    ) -> Result<ModelResponseEventStream, ModelError> {
        let cancellation_token = websocket_request.cancellation_token.clone();
        let logical_body = websocket_request.body.clone();
        websocket_request.body = self.prepare_websocket_request_body(&logical_body);
        ProtocolModelClient::ensure_real_model_request_allowed(&websocket_request)?;
        let diagnostic = transport_selected_diagnostic(
            &self.client.provider_name,
            &self.client.model_name,
            "websocket",
        );
        if let Some(audit) = self.client.request_audit.as_ref() {
            audit.record(
                &self.client.provider_name,
                &self.client.model_name,
                true,
                &websocket_request,
            );
        }
        match self
            .websocket_session
            .send_websocket_event_stream_incremental(websocket_request)
            .await
        {
            Ok(events) => {
                let fallback_plan = if fallback {
                    Some(self.required_fallback_plan(http_request)?)
                } else {
                    None
                };
                self.last_websocket_request = Some(logical_body);
                Ok(canonical_openai_response_stream_for_session(
                    events,
                    cancellation_token,
                    Some(diagnostic),
                    Arc::clone(&self.last_response),
                    request_settings,
                    fallback_plan,
                ))
            }
            Err(error) if fallback => {
                mark_last_response_failed(&self.last_response);
                self.last_websocket_request = None;
                let plan = self.required_fallback_plan(http_request)?;
                Ok(openai_response_stream_from_websocket_setup_error(
                    error,
                    cancellation_token,
                    diagnostic,
                    plan,
                ))
            }
            Err(error) => {
                mark_last_response_failed(&self.last_response);
                self.last_websocket_request = None;
                Err(error)
            }
        }
    }
}

#[async_trait]
impl ModelRunSession for ProtocolModelClientRunSession<'_> {
    async fn request_stream_incremental(
        &mut self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponseEventStream, ModelError> {
        let cancellation_token = context.cancellation_token();
        if self.client.profile.protocol != ProtocolFamily::OpenAiResponses {
            let response = self
                .client
                .request(messages, settings, params, context)
                .await?;
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
            self.client.default_settings.as_ref(),
            settings,
            params,
            &self.client.profile,
        );
        let request_settings = prepared.settings.clone();
        let wire_body = self.client.build_wire_body(
            &prepared.normalized_messages,
            prepared.settings.as_ref(),
            &prepared.params,
        )?;
        let options =
            self.client
                .request_options(&context, prepared.settings.as_ref(), &prepared.params);
        let mut transport = self
            .client
            .resolve_response_stream_transport(prepared.settings.as_ref());
        if self.fallback_to_http.load(Ordering::Relaxed)
            && matches!(
                transport,
                ResolvedResponseStreamTransport::WebSocketThenHttpOnRetryable
            )
        {
            transport = ResolvedResponseStreamTransport::HttpOnly;
        }
        match transport {
            ResolvedResponseStreamTransport::HttpOnly => {
                let http_request = self.client.build_response_stream_request(
                    wire_body,
                    &options,
                    cancellation_token,
                    ResponseStreamRequestKind::HttpSse,
                );
                ProtocolModelClient::ensure_real_model_request_allowed(&http_request)?;
                self.client
                    .openai_response_stream_from_request(
                        http_request,
                        ResponseStreamRequestKind::HttpSse,
                    )
                    .await
            }
            ResolvedResponseStreamTransport::WebSocketOnly => {
                let websocket_request = self.client.build_response_stream_request(
                    wire_body,
                    &options,
                    cancellation_token,
                    ResponseStreamRequestKind::WebSocket,
                );
                self.request_websocket_stream(
                    websocket_request,
                    None,
                    request_settings,
                    /*fallback*/ false,
                )
                .await
            }
            ResolvedResponseStreamTransport::WebSocketThenHttpOnRetryable => {
                let websocket_request = self.client.build_response_stream_request(
                    wire_body.clone(),
                    &options,
                    cancellation_token.clone(),
                    ResponseStreamRequestKind::WebSocket,
                );
                let http_request = self.client.build_response_stream_request(
                    wire_body,
                    &options,
                    cancellation_token,
                    ResponseStreamRequestKind::HttpSse,
                );
                self.request_websocket_stream(
                    websocket_request,
                    Some(http_request),
                    request_settings,
                    /*fallback*/ true,
                )
                .await
            }
        }
    }

    async fn close(&mut self) {
        self.websocket_session.reset().await;
        self.last_websocket_request = None;
        mark_last_response_failed(&self.last_response);
    }
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
            "detail": error.to_string(),
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
    let drop_abort_token = events.drop_abort_token();
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
    ModelResponseEventStream::new_with_cancellation_and_drop_abort(
        receiver,
        cancellation_token,
        drop_abort_token,
    )
}

fn canonical_openai_response_stream_for_session(
    events: ModelEventStream,
    cancellation_token: starweaver_core::CancellationToken,
    diagnostic: Option<StreamDiagnostic>,
    last_response: Arc<Mutex<LastResponseSlot>>,
    request_settings: Option<ModelSettings>,
    fallback_plan: Option<SessionFallbackPlan>,
) -> ModelResponseEventStream {
    let drop_abort_token = events.drop_abort_token();
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
        let result = forward_openai_response_events_tracked(
            events,
            &sender,
            &mut emitted_any_event,
            &last_response,
            request_settings.as_ref(),
        )
        .await;
        match result {
            Ok(()) => {}
            Err(error) if !emitted_any_event => {
                if let Some(plan) = fallback_plan {
                    if should_fallback_websocket_to_http(&error) {
                        forward_http_fallback(plan, &sender, &error, &mut emitted_any_event).await;
                    } else {
                        mark_last_response_failed(&last_response);
                        let _ = sender.send(Err(error)).await;
                    }
                } else {
                    mark_last_response_failed(&last_response);
                    let _ = sender.send(Err(error)).await;
                }
            }
            Err(error) => {
                mark_last_response_failed(&last_response);
                let _ = sender.send(Err(error)).await;
            }
        }
    });
    ModelResponseEventStream::new_with_cancellation_and_drop_abort(
        receiver,
        cancellation_token,
        drop_abort_token,
    )
}

fn openai_response_stream_from_websocket_setup_error(
    error: ModelError,
    cancellation_token: starweaver_core::CancellationToken,
    diagnostic: StreamDiagnostic,
    plan: SessionFallbackPlan,
) -> ModelResponseEventStream {
    let (sender, receiver) = tokio::sync::mpsc::channel(32);
    tokio::spawn(async move {
        if sender
            .send(Ok(ModelResponseStreamEvent::Diagnostic(diagnostic)))
            .await
            .is_err()
        {
            return;
        }
        if should_fallback_websocket_to_http(&error) {
            let mut emitted_any_event = false;
            forward_http_fallback(plan, &sender, &error, &mut emitted_any_event).await;
        } else {
            mark_last_response_failed(&plan.last_response);
            let _ = sender.send(Err(error)).await;
        }
    });
    ModelResponseEventStream::new_with_cancellation(receiver, cancellation_token)
}

async fn forward_http_fallback(
    plan: SessionFallbackPlan,
    sender: &tokio::sync::mpsc::Sender<Result<ModelResponseStreamEvent, ModelError>>,
    websocket_error: &ModelError,
    emitted_any_event: &mut bool,
) {
    plan.fallback_to_http.store(true, Ordering::Relaxed);
    mark_last_response_failed(&plan.last_response);
    let _ = sender
        .send(Ok(ModelResponseStreamEvent::Diagnostic(
            transport_fallback_diagnostic(&plan.provider_name, &plan.model_name, websocket_error),
        )))
        .await;
    if let Some(audit) = plan.request_audit.as_ref() {
        audit.record(
            &plan.provider_name,
            &plan.model_name,
            true,
            &plan.http_request,
        );
    }
    match plan
        .http_client
        .send_event_stream_incremental(plan.http_request)
        .await
    {
        Ok(events) => {
            if let Err(error) =
                forward_openai_response_events(events, sender, emitted_any_event).await
            {
                let _ = sender.send(Err(error)).await;
            }
        }
        Err(error) => {
            let _ = sender.send(Err(error)).await;
        }
    }
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

async fn forward_openai_response_events_tracked(
    mut events: ModelEventStream,
    sender: &tokio::sync::mpsc::Sender<Result<ModelResponseStreamEvent, ModelError>>,
    emitted_any_event: &mut bool,
    last_response: &Arc<Mutex<LastResponseSlot>>,
    request_settings: Option<&ModelSettings>,
) -> Result<(), ModelError> {
    let mut parser = crate::providers::openai_responses::OpenAiResponsesStreamParser::default();
    while let Some(event) = events.recv().await {
        let event = event?;
        let stream_events = parser.push_event(&event)?;
        for stream_event in stream_events {
            if let ModelResponseStreamEvent::FinalResult(response) = &stream_event {
                record_last_response(last_response, response, request_settings);
            }
            if sender.send(Ok(stream_event)).await.is_err() {
                return Ok(());
            }
            *emitted_any_event = true;
        }
    }
    for stream_event in parser.finish()? {
        if let ModelResponseStreamEvent::FinalResult(response) = &stream_event {
            record_last_response(last_response, response, request_settings);
        }
        if sender.send(Ok(stream_event)).await.is_err() {
            return Ok(());
        }
        *emitted_any_event = true;
    }
    Ok(())
}

fn record_last_response(
    slot: &Arc<Mutex<LastResponseSlot>>,
    response: &crate::message::ModelResponse,
    request_settings: Option<&ModelSettings>,
) {
    let response_id = response
        .provider
        .as_ref()
        .and_then(|provider| provider.response_id.clone())
        .unwrap_or_default();
    let replay_items =
        crate::providers::openai_responses::OpenAiResponsesAdapter::response_replay_items(
            response,
            request_settings,
        );
    let mut slot = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    slot.response = Some(LastResponse {
        response_id,
        replay_items,
    });
    slot.failed = false;
}

fn mark_last_response_failed(slot: &Arc<Mutex<LastResponseSlot>>) {
    let mut slot = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    slot.response = None;
    slot.failed = true;
}

fn response_input_items(value: &Value) -> Option<&[Value]> {
    value
        .get("input")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
}

fn strip_value_prefix<'a>(items: &'a [Value], prefix: &[Value]) -> Option<&'a [Value]> {
    if items.len() < prefix.len() || &items[..prefix.len()] != prefix {
        return None;
    }
    Some(&items[prefix.len()..])
}

fn responses_websocket_request_properties_match(previous: &Value, current: &Value) -> bool {
    let Some(previous) = previous.as_object() else {
        return false;
    };
    let Some(current) = current.as_object() else {
        return false;
    };
    let mut previous = previous.clone();
    let mut current = current.clone();
    previous.remove("input");
    current.remove("input");
    previous == current
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use async_trait::async_trait;
    use serde_json::{json, Value};
    use starweaver_core::{ConversationId, RunId};

    use super::*;
    use crate::{
        adapter::ModelAdapter,
        message::{ModelRequest, ModelResponse},
        profile::ProtocolFamily,
        settings::{OpenAiResponsesSettings, ProviderReplaySettings, ProviderSettings},
        transport::{HttpModelConfig, HttpResponse, ModelHttpClient, ModelWebSocketEventSession},
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

    #[tokio::test]
    async fn run_session_reuses_websocket_session_and_sends_incremental_create() {
        let fake = Arc::new(SessionFakeClient::new(
            vec![
                Ok(vec![Ok(completed_text_event_with_ids(
                    "resp_1",
                    "msg_1",
                    "assistant output",
                ))]),
                Ok(vec![Ok(completed_text_event_with_ids(
                    "resp_2", "msg_2", "done",
                ))]),
            ],
            Vec::new(),
        ));
        let client = test_client_with_session_fake("codex", fake.clone());
        let mut session = client.start_run_session();
        let settings = settings_with_transport(ResponseStreamTransport::WebSocket);

        let first_response = final_response_from_stream(
            session
                .request_stream_incremental(
                    vec![ModelMessage::Request(ModelRequest::user_text("hello"))],
                    Some(settings.clone()),
                    ModelRequestParameters::default(),
                    test_context(),
                )
                .await,
        )
        .await;
        let second_response = final_response_from_stream(
            session
                .request_stream_incremental(
                    vec![
                        ModelMessage::Request(ModelRequest::user_text("hello")),
                        ModelMessage::Response(first_response),
                        ModelMessage::Request(ModelRequest::user_text("second")),
                    ],
                    Some(settings),
                    ModelRequestParameters::default(),
                    test_context(),
                )
                .await,
        )
        .await;

        assert_eq!(second_response.text_output(), "done");
        assert_eq!(fake.websocket_sessions(), 1);
        assert_eq!(
            fake.calls(),
            vec![FakeCallKind::WebSocket, FakeCallKind::WebSocket]
        );
        let bodies = fake.bodies();
        assert_eq!(bodies.len(), 2);
        assert_eq!(
            bodies[1]
                .get("previous_response_id")
                .and_then(Value::as_str),
            Some("resp_1")
        );
        let second_input = option_or_panic(
            bodies[1].get("input").and_then(Value::as_array),
            "second request input should be an array",
        );
        assert_eq!(second_input.len(), 1);
        assert_eq!(
            second_input[0].get("role").and_then(Value::as_str),
            Some("user")
        );
    }

    #[tokio::test]
    async fn run_session_reuses_websocket_but_full_creates_on_non_prefix_input() {
        let fake = Arc::new(SessionFakeClient::new(
            vec![
                Ok(vec![Ok(completed_text_event_with_ids(
                    "resp_1",
                    "msg_1",
                    "assistant output",
                ))]),
                Ok(vec![Ok(completed_text_event_with_ids(
                    "resp_2", "msg_2", "done",
                ))]),
            ],
            Vec::new(),
        ));
        let client = test_client_with_session_fake("codex", fake.clone());
        let mut session = client.start_run_session();
        let settings = settings_with_transport(ResponseStreamTransport::WebSocket);

        let _ = final_response_from_stream(
            session
                .request_stream_incremental(
                    vec![ModelMessage::Request(ModelRequest::user_text("hello"))],
                    Some(settings.clone()),
                    ModelRequestParameters::default(),
                    test_context(),
                )
                .await,
        )
        .await;
        let _ = final_response_from_stream(
            session
                .request_stream_incremental(
                    vec![ModelMessage::Request(ModelRequest::user_text("different"))],
                    Some(settings),
                    ModelRequestParameters::default(),
                    test_context(),
                )
                .await,
        )
        .await;

        assert_eq!(fake.websocket_sessions(), 1);
        let bodies = fake.bodies();
        assert_eq!(bodies.len(), 2);
        assert!(bodies[1].get("previous_response_id").is_none());
        assert_eq!(
            bodies[1]
                .get("input")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            bodies[1]["input"][0]
                .get("content")
                .and_then(Value::as_array)
                .and_then(|content| content.first())
                .and_then(|content| content.get("text"))
                .and_then(Value::as_str),
            Some("different")
        );
    }

    #[tokio::test]
    async fn run_session_auto_fallback_uses_http_for_remaining_requests() {
        let fake = Arc::new(SessionFakeClient::new(
            vec![Err(connection_limit_error())],
            vec![
                vec![Ok(completed_text_event_with_ids(
                    "resp_http_1",
                    "msg_http_1",
                    "from http one",
                ))],
                vec![Ok(completed_text_event_with_ids(
                    "resp_http_2",
                    "msg_http_2",
                    "from http two",
                ))],
            ],
        ));
        let client = test_client_with_session_fake("codex", fake.clone());
        let mut session = client.start_run_session();

        let first_response = final_response_from_stream(
            session
                .request_stream_incremental(
                    vec![ModelMessage::Request(ModelRequest::user_text("hello"))],
                    None,
                    ModelRequestParameters::default(),
                    test_context(),
                )
                .await,
        )
        .await;
        let second_response = final_response_from_stream(
            session
                .request_stream_incremental(
                    vec![
                        ModelMessage::Request(ModelRequest::user_text("hello")),
                        ModelMessage::Response(first_response),
                        ModelMessage::Request(ModelRequest::user_text("second")),
                    ],
                    None,
                    ModelRequestParameters::default(),
                    test_context(),
                )
                .await,
        )
        .await;

        assert_eq!(second_response.text_output(), "from http two");
        assert_eq!(
            fake.calls(),
            vec![
                FakeCallKind::WebSocket,
                FakeCallKind::Http,
                FakeCallKind::Http
            ]
        );
    }

    #[tokio::test]
    async fn run_session_does_not_delta_when_request_uses_conversation_state() {
        let fake = Arc::new(SessionFakeClient::new(
            vec![
                Ok(vec![Ok(completed_text_event_with_ids(
                    "resp_1",
                    "msg_1",
                    "assistant output",
                ))]),
                Ok(vec![Ok(completed_text_event_with_ids(
                    "resp_2", "msg_2", "done",
                ))]),
            ],
            Vec::new(),
        ));
        let client = test_client_with_session_fake("codex", fake.clone());
        let mut session = client.start_run_session();
        let websocket_settings = settings_with_transport(ResponseStreamTransport::WebSocket);

        let first_response = final_response_from_stream(
            session
                .request_stream_incremental(
                    vec![ModelMessage::Request(ModelRequest::user_text("hello"))],
                    Some(websocket_settings),
                    ModelRequestParameters::default(),
                    test_context(),
                )
                .await,
        )
        .await;
        let mut conversation_settings = settings_with_transport(ResponseStreamTransport::WebSocket);
        conversation_settings.provider_replay = Some(ProviderReplaySettings {
            conversation_id: Some("conv_manual".to_string()),
            ..ProviderReplaySettings::default()
        });
        let _ = final_response_from_stream(
            session
                .request_stream_incremental(
                    vec![
                        ModelMessage::Request(ModelRequest::user_text("hello")),
                        ModelMessage::Response(first_response),
                        ModelMessage::Request(ModelRequest::user_text("second")),
                    ],
                    Some(conversation_settings),
                    ModelRequestParameters::default(),
                    test_context(),
                )
                .await,
        )
        .await;

        let bodies = fake.bodies();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[1].get("conversation"), Some(&json!("conv_manual")));
        assert!(bodies[1].get("previous_response_id").is_none());
        assert_eq!(
            bodies[1]
                .get("input")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(3)
        );
    }

    #[tokio::test]
    async fn run_session_drops_incremental_state_after_stream_error() {
        let fake = Arc::new(SessionFakeClient::new(
            vec![
                Ok(vec![
                    Ok(text_delta_event("partial")),
                    Err(connection_limit_error()),
                ]),
                Ok(vec![Ok(completed_text_event_with_ids(
                    "resp_2", "msg_2", "done",
                ))]),
            ],
            Vec::new(),
        ));
        let client = test_client_with_session_fake("codex", fake.clone());
        let mut session = client.start_run_session();
        let settings = settings_with_transport(ResponseStreamTransport::WebSocket);

        let mut first_stream = result_or_panic(
            session
                .request_stream_incremental(
                    vec![ModelMessage::Request(ModelRequest::user_text("hello"))],
                    Some(settings.clone()),
                    ModelRequestParameters::default(),
                    test_context(),
                )
                .await,
            "first stream should be created",
        );
        let mut saw_error = false;
        while let Some(event) = first_stream.recv().await {
            if matches!(
                event,
                Err(ModelError::ProviderStatus {
                    status: 400,
                    retryable: true,
                    ..
                })
            ) {
                saw_error = true;
                break;
            }
        }
        assert!(saw_error);

        let second_response = final_response_from_stream(
            session
                .request_stream_incremental(
                    vec![ModelMessage::Request(ModelRequest::user_text("next"))],
                    Some(settings),
                    ModelRequestParameters::default(),
                    test_context(),
                )
                .await,
        )
        .await;

        assert_eq!(second_response.text_output(), "done");
        let bodies = fake.bodies();
        assert_eq!(bodies.len(), 2);
        assert!(bodies[1].get("previous_response_id").is_none());
        assert_eq!(
            bodies[1]["input"][0]
                .get("content")
                .and_then(Value::as_array)
                .and_then(|content| content.first())
                .and_then(|content| content.get("text"))
                .and_then(Value::as_str),
            Some("next")
        );
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

    fn test_client_with_session_fake(
        provider_name: &str,
        http_client: Arc<SessionFakeClient>,
    ) -> ProtocolModelClient {
        ProtocolModelClient::new(
            provider_name,
            "gpt-5-codex",
            ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
            HttpModelConfig::new("https://api.openai.com/v1", "responses"),
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
        completed_text_event_with_ids("resp_test", "msg_test", text)
    }

    fn completed_text_event_with_ids(response_id: &str, message_id: &str, text: &str) -> Value {
        json!({
            "type": "response.completed",
            "response": {
                "id": response_id,
                "model": "gpt-5-codex",
                "status": "completed",
                "output": [{
                    "id": message_id,
                    "type": "message",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{"type": "output_text", "text": text}]
                }],
                "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
            }
        })
    }

    async fn final_response_from_stream(
        stream: Result<ModelResponseEventStream, ModelError>,
    ) -> ModelResponse {
        let mut stream = result_or_panic(stream, "stream should be created");
        while let Some(event) = stream.recv().await {
            if let ModelResponseStreamEvent::FinalResult(response) =
                result_or_panic(event, "event should parse")
            {
                return *response;
            }
        }
        panic!("stream ended without final response")
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

    type SessionFakeWebSocketEvents = Vec<Result<Value, ModelError>>;
    type SessionFakeWebSocketResult = Result<SessionFakeWebSocketEvents, ModelError>;
    type SessionFakeHttpEvents = Vec<Result<Value, ModelError>>;

    #[derive(Debug)]
    struct SessionFakeClient {
        websocket_results: Mutex<VecDeque<SessionFakeWebSocketResult>>,
        http_results: Mutex<VecDeque<SessionFakeHttpEvents>>,
        calls: Mutex<Vec<FakeCallKind>>,
        bodies: Mutex<Vec<Value>>,
        websocket_sessions: Mutex<usize>,
    }

    impl SessionFakeClient {
        fn new(
            websocket_results: Vec<SessionFakeWebSocketResult>,
            http_results: Vec<SessionFakeHttpEvents>,
        ) -> Self {
            Self {
                websocket_results: Mutex::new(VecDeque::from(websocket_results)),
                http_results: Mutex::new(VecDeque::from(http_results)),
                calls: Mutex::new(Vec::new()),
                bodies: Mutex::new(Vec::new()),
                websocket_sessions: Mutex::new(0),
            }
        }

        fn calls(&self) -> Vec<FakeCallKind> {
            lock_or_panic(self.calls.lock(), "calls lock should not be poisoned").clone()
        }

        fn bodies(&self) -> Vec<Value> {
            lock_or_panic(self.bodies.lock(), "bodies lock should not be poisoned").clone()
        }

        fn websocket_sessions(&self) -> usize {
            *lock_or_panic(
                self.websocket_sessions.lock(),
                "websocket sessions lock should not be poisoned",
            )
        }

        fn record(&self, kind: FakeCallKind, request: &HttpRequest) {
            lock_or_panic(self.calls.lock(), "calls lock should not be poisoned").push(kind);
            lock_or_panic(self.bodies.lock(), "bodies lock should not be poisoned")
                .push(request.body.clone());
        }
    }

    #[async_trait]
    impl ModelHttpClient for SessionFakeClient {
        async fn send(&self, _request: HttpRequest) -> Result<HttpResponse, ModelError> {
            Err(ModelError::Transport(
                "send is not used by session transport tests".to_string(),
            ))
        }

        async fn send_event_stream_incremental(
            &self,
            request: HttpRequest,
        ) -> Result<ModelEventStream, ModelError> {
            self.record(FakeCallKind::Http, &request);
            let events = lock_or_panic(
                self.http_results.lock(),
                "http results lock should not be poisoned",
            )
            .pop_front()
            .unwrap_or_else(|| {
                vec![Err(ModelError::Transport(
                    "missing fake HTTP response".to_string(),
                ))]
            });
            Ok(stream_from_results(events))
        }

        async fn send_websocket_event_stream_incremental(
            &self,
            request: HttpRequest,
        ) -> Result<ModelEventStream, ModelError> {
            self.record(FakeCallKind::WebSocket, &request);
            let result = lock_or_panic(
                self.websocket_results.lock(),
                "websocket results lock should not be poisoned",
            )
            .pop_front()
            .unwrap_or_else(|| {
                Err(ModelError::Transport(
                    "missing fake websocket response".to_string(),
                ))
            });
            result.map(stream_from_results)
        }

        fn websocket_event_session(&self) -> Box<dyn ModelWebSocketEventSession + '_> {
            *lock_or_panic(
                self.websocket_sessions.lock(),
                "websocket sessions lock should not be poisoned",
            ) += 1;
            Box::new(SessionFakeWebSocketSession { client: self })
        }
    }

    struct SessionFakeWebSocketSession<'a> {
        client: &'a SessionFakeClient,
    }

    #[async_trait]
    impl ModelWebSocketEventSession for SessionFakeWebSocketSession<'_> {
        async fn send_websocket_event_stream_incremental(
            &mut self,
            request: HttpRequest,
        ) -> Result<ModelEventStream, ModelError> {
            self.client
                .send_websocket_event_stream_incremental(request)
                .await
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
