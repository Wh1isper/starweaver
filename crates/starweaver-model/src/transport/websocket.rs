use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, future::Future, sync::Arc, time::Duration};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{
        Error as WebSocketError, Message,
        client::IntoClientRequest,
        http::{HeaderName, HeaderValue},
        protocol::{
            WebSocketConfig,
            frame::{CloseFrame, coding::CloseCode},
        },
    },
};

use crate::{ModelError, allow_real_model_requests, transport::is_retryable_status};

use super::{HttpRequest, ModelEventStream, ModelWebSocketEventSession};

type ModelWebSocketStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const WEBSOCKET_CONNECTION_LIMIT_REACHED_CODE: &str = "websocket_connection_limit_reached";
const MODEL_WEBSOCKET_MAX_MESSAGE_SIZE: usize = 256 << 20;
const MODEL_WEBSOCKET_MAX_FRAME_SIZE: usize = 64 << 20;
const MODEL_WEBSOCKET_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Eq, PartialEq)]
struct WebSocketConnectionKey {
    url: String,
    headers: BTreeMap<String, String>,
}

#[derive(Default)]
struct ReusableWebSocketInner {
    stream: Option<ModelWebSocketStream>,
    connection_key: Option<WebSocketConnectionKey>,
}

/// WebSocket event session that serializes multiple `response.create` requests on one connection.
#[derive(Default)]
pub(super) struct ReusableWebSocketEventSession {
    inner: Arc<tokio::sync::Mutex<ReusableWebSocketInner>>,
}

impl Drop for ReusableWebSocketEventSession {
    fn drop(&mut self) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let inner = Arc::clone(&self.inner);
        handle.spawn(async move {
            let mut inner = inner.lock().await;
            if let Some(mut stream) = inner.stream.take() {
                graceful_close_websocket(&mut stream, WebSocketCloseKind::SessionClosed).await;
            }
            inner.connection_key = None;
        });
    }
}

/// Send a JSON request over WebSocket and return JSON text-frame events.
pub async fn send_websocket_event_stream_incremental(
    request: HttpRequest,
) -> Result<ModelEventStream, ModelError> {
    if !allow_real_model_requests() {
        return Err(ModelError::RealModelRequestBlocked {
            url: request.url.clone(),
        });
    }
    ensure_websocket_not_cancelled(&request.cancellation_token)?;
    let cancellation_token = request.cancellation_token.clone();
    let drop_abort_token = starweaver_core::CancellationToken::default();
    let mut stream = Box::pin(connect_websocket_stream(&request)).await?;
    if let Err(error) = send_websocket_request_body(&mut stream, &request).await {
        graceful_close_websocket(
            &mut stream,
            WebSocketCloseKind::close_kind_for_error(&error),
        )
        .await;
        return Err(error);
    }

    let (sender, receiver) = tokio::sync::mpsc::channel(32);
    spawn_websocket_event_worker(
        stream,
        sender,
        cancellation_token.clone(),
        drop_abort_token.clone(),
        request.timeout,
    );
    Ok(ModelEventStream::new_with_cancellation_and_drop_abort(
        receiver,
        cancellation_token,
        Some(drop_abort_token),
    ))
}

#[async_trait::async_trait]
impl ModelWebSocketEventSession for ReusableWebSocketEventSession {
    #[allow(
        clippy::significant_drop_tightening,
        reason = "the owned guard is intentionally moved into the worker to serialize websocket use until response completion"
    )]
    async fn send_websocket_event_stream_incremental(
        &mut self,
        request: HttpRequest,
    ) -> Result<ModelEventStream, ModelError> {
        if !allow_real_model_requests() {
            return Err(ModelError::RealModelRequestBlocked {
                url: request.url.clone(),
            });
        }
        ensure_websocket_not_cancelled(&request.cancellation_token)?;
        let cancellation_token = request.cancellation_token.clone();
        let drop_abort_token = starweaver_core::CancellationToken::default();
        let mut inner = Arc::clone(&self.inner).lock_owned().await;
        Box::pin(ensure_reusable_websocket_connection(&mut inner, &request)).await?;
        let Some(stream) = inner.stream.as_mut() else {
            return Err(ModelError::Transport(
                "websocket connection is unavailable".to_string(),
            ));
        };
        if let Err(error) = send_websocket_request_body(stream, &request).await {
            graceful_close_websocket(stream, WebSocketCloseKind::close_kind_for_error(&error))
                .await;
            inner.stream = None;
            inner.connection_key = None;
            return Err(error);
        }

        let (sender, receiver) = tokio::sync::mpsc::channel(32);
        spawn_reusable_websocket_event_worker(
            inner,
            sender,
            cancellation_token.clone(),
            drop_abort_token.clone(),
            request.timeout,
        );
        Ok(ModelEventStream::new_with_cancellation_and_drop_abort(
            receiver,
            cancellation_token,
            Some(drop_abort_token),
        ))
    }

    async fn reset(&mut self) {
        let mut inner = self.inner.lock().await;
        if let Some(mut stream) = inner.stream.take() {
            graceful_close_websocket(&mut stream, WebSocketCloseKind::SessionClosed).await;
        }
        inner.connection_key = None;
    }
}

async fn ensure_reusable_websocket_connection(
    inner: &mut ReusableWebSocketInner,
    request: &HttpRequest,
) -> Result<(), ModelError> {
    let key = WebSocketConnectionKey {
        url: request.url.clone(),
        headers: request.headers.clone(),
    };
    if inner.stream.is_some() && inner.connection_key.as_ref() == Some(&key) {
        return Ok(());
    }
    if let Some(mut stream) = inner.stream.take() {
        graceful_close_websocket(&mut stream, WebSocketCloseKind::SessionClosed).await;
    }
    let stream = Box::pin(connect_websocket_stream(request)).await?;
    inner.stream = Some(stream);
    inner.connection_key = Some(key);
    Ok(())
}

fn ensure_websocket_not_cancelled(
    cancellation_token: &starweaver_core::CancellationToken,
) -> Result<(), ModelError> {
    if cancellation_token.is_cancelled() {
        Err(ModelError::Cancelled {
            reason: "model websocket stream cancellation requested".to_string(),
        })
    } else {
        Ok(())
    }
}

async fn connect_websocket_stream(
    request: &HttpRequest,
) -> Result<ModelWebSocketStream, ModelError> {
    let websocket_url = websocket_url_from_http_url(&request.url)?;
    let mut websocket_request = websocket_url
        .as_str()
        .into_client_request()
        .map_err(|err| {
            ModelError::Transport(format!("failed to build websocket request: {err}"))
        })?;
    insert_headers(websocket_request.headers_mut(), &request.headers)?;

    let connect = connect_async_with_config(
        websocket_request,
        Some(model_websocket_config()),
        /*disable_nagle*/ false,
    );
    let (stream, _response) = tokio::select! {
        biased;
        () = request.cancellation_token.cancelled() => {
            return Err(ModelError::Cancelled {
                reason: "model websocket stream cancellation requested".to_string(),
            });
        }
        result = with_optional_timeout(request.timeout, "websocket connect timeout", connect) => {
            result?.map_err(map_websocket_connect_error)?
        },
    };
    Ok(stream)
}

fn model_websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MODEL_WEBSOCKET_MAX_MESSAGE_SIZE))
        .max_frame_size(Some(MODEL_WEBSOCKET_MAX_FRAME_SIZE))
}

async fn send_websocket_request_body(
    stream: &mut ModelWebSocketStream,
    request: &HttpRequest,
) -> Result<(), ModelError> {
    let request_text = serde_json::to_string(&request.body).map_err(|err| {
        ModelError::Transport(format!("failed to encode websocket request body: {err}"))
    })?;
    tokio::select! {
        biased;
        () = request.cancellation_token.cancelled() => {
            return Err(ModelError::Cancelled {
                reason: "model websocket stream cancellation requested".to_string(),
            });
        }
        result = with_optional_timeout(
            request.timeout,
            "websocket request send timeout",
            stream.send(Message::Text(request_text.into())),
        ) => {
            result?.map_err(map_websocket_send_error)?;
        }
    }
    Ok(())
}

fn spawn_websocket_event_worker(
    mut stream: ModelWebSocketStream,
    sender: tokio::sync::mpsc::Sender<Result<Value, ModelError>>,
    cancellation_token: starweaver_core::CancellationToken,
    drop_abort_token: starweaver_core::CancellationToken,
    timeout: Option<Duration>,
) {
    tokio::spawn(async move {
        loop {
            let message = next_websocket_message(
                &mut stream,
                &sender,
                &cancellation_token,
                &drop_abort_token,
                timeout,
            )
            .await;
            let Some(message) = message else { return };
            match handle_websocket_message(message, &mut stream, &sender, timeout, true).await {
                WebSocketMessageOutcome::Continue => {}
                WebSocketMessageOutcome::Completed => return,
                WebSocketMessageOutcome::Failed(kind) => {
                    graceful_close_websocket(&mut stream, kind).await;
                    return;
                }
            }
        }
    });
}

fn spawn_reusable_websocket_event_worker(
    mut inner: tokio::sync::OwnedMutexGuard<ReusableWebSocketInner>,
    sender: tokio::sync::mpsc::Sender<Result<Value, ModelError>>,
    cancellation_token: starweaver_core::CancellationToken,
    drop_abort_token: starweaver_core::CancellationToken,
    timeout: Option<Duration>,
) {
    tokio::spawn(async move {
        loop {
            let Some(stream) = inner.stream.as_mut() else {
                let _ = sender
                    .send(Err(ModelError::Transport(
                        "websocket connection is unavailable".to_string(),
                    )))
                    .await;
                inner.connection_key = None;
                return;
            };
            let message = next_websocket_message(
                stream,
                &sender,
                &cancellation_token,
                &drop_abort_token,
                timeout,
            )
            .await;
            let Some(message) = message else {
                inner.stream = None;
                inner.connection_key = None;
                return;
            };
            match handle_websocket_message(message, stream, &sender, timeout, false).await {
                WebSocketMessageOutcome::Continue => {}
                WebSocketMessageOutcome::Completed => return,
                WebSocketMessageOutcome::Failed(kind) => {
                    graceful_close_websocket(stream, kind).await;
                    inner.stream = None;
                    inner.connection_key = None;
                    return;
                }
            }
        }
    });
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WebSocketMessageOutcome {
    Continue,
    Completed,
    Failed(WebSocketCloseKind),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResponseTerminalEvent {
    Completed,
    Failed,
    Incomplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WebSocketCloseKind {
    NormalCompletion,
    SessionClosed,
    GoingAway,
    Cancelled,
    Timeout,
    ProtocolError,
    UnsupportedData,
    ProviderError,
    InternalError,
    RemoteClosed,
}

impl WebSocketCloseKind {
    const fn code(self) -> CloseCode {
        match self {
            Self::NormalCompletion | Self::SessionClosed | Self::RemoteClosed => CloseCode::Normal,
            Self::GoingAway | Self::Cancelled => CloseCode::Away,
            Self::ProtocolError => CloseCode::Protocol,
            Self::UnsupportedData => CloseCode::Unsupported,
            Self::Timeout | Self::ProviderError | Self::InternalError => CloseCode::Error,
        }
    }

    const fn reason(self) -> &'static str {
        match self {
            Self::NormalCompletion => "response completed",
            Self::SessionClosed => "session closed",
            Self::GoingAway => "receiver dropped",
            Self::Cancelled => "request cancelled",
            Self::Timeout => "websocket timeout",
            Self::ProtocolError => "protocol error",
            Self::UnsupportedData => "unsupported data",
            Self::ProviderError => "provider terminal error",
            Self::InternalError => "internal error",
            Self::RemoteClosed => "remote closed",
        }
    }

    const fn close_kind_for_error(error: &ModelError) -> Self {
        match error {
            ModelError::Cancelled { .. } => Self::Cancelled,
            ModelError::ResponseParsing(_) => Self::ProtocolError,
            ModelError::ProviderStatus { .. } => Self::ProviderError,
            _ => Self::InternalError,
        }
    }
}

async fn graceful_close_websocket(stream: &mut ModelWebSocketStream, kind: WebSocketCloseKind) {
    let frame = CloseFrame {
        code: kind.code(),
        reason: kind.reason().into(),
    };
    let _ = tokio::time::timeout(MODEL_WEBSOCKET_CLOSE_TIMEOUT, stream.close(Some(frame))).await;
}

fn response_terminal_event(value: &Value) -> Option<ResponseTerminalEvent> {
    match value.get("type").and_then(Value::as_str) {
        Some("response.completed") => Some(ResponseTerminalEvent::Completed),
        Some("response.failed") => Some(ResponseTerminalEvent::Failed),
        Some("response.incomplete") => Some(ResponseTerminalEvent::Incomplete),
        _ => None,
    }
}

async fn next_websocket_message(
    stream: &mut ModelWebSocketStream,
    sender: &tokio::sync::mpsc::Sender<Result<Value, ModelError>>,
    cancellation_token: &starweaver_core::CancellationToken,
    drop_abort_token: &starweaver_core::CancellationToken,
    timeout: Option<Duration>,
) -> Option<Result<Message, WebSocketError>> {
    tokio::select! {
        biased;
        () = cancellation_token.cancelled() => {
            let _ = sender
                .send(Err(ModelError::Cancelled {
                    reason: "model websocket stream cancellation requested".to_string(),
                }))
                .await;
            graceful_close_websocket(stream, WebSocketCloseKind::Cancelled).await;
            None
        }
        () = drop_abort_token.cancelled() => {
            graceful_close_websocket(stream, WebSocketCloseKind::GoingAway).await;
            None
        }
        message = with_optional_timeout(timeout, "websocket receive timeout", stream.next()) => {
            match message {
                Ok(Some(message)) => Some(message),
                Ok(None) => {
                    let _ = sender
                        .send(Err(ModelError::Transport(
                            "websocket closed before response.completed".to_string(),
                        )))
                        .await;
                    graceful_close_websocket(stream, WebSocketCloseKind::RemoteClosed).await;
                    None
                }
                Err(error) => {
                    let _ = sender.send(Err(error)).await;
                    graceful_close_websocket(stream, WebSocketCloseKind::Timeout).await;
                    None
                }
            }
        }
    }
}

async fn handle_websocket_message(
    message: Result<Message, WebSocketError>,
    stream: &mut ModelWebSocketStream,
    sender: &tokio::sync::mpsc::Sender<Result<Value, ModelError>>,
    timeout: Option<Duration>,
    close_on_completed: bool,
) -> WebSocketMessageOutcome {
    match message {
        Ok(Message::Text(text)) => {
            handle_websocket_text_event(&text, stream, sender, close_on_completed).await
        }
        Ok(Message::Binary(_)) => {
            let _ = sender
                .send(Err(ModelError::ResponseParsing(
                    "unexpected binary websocket event".to_string(),
                )))
                .await;
            WebSocketMessageOutcome::Failed(WebSocketCloseKind::UnsupportedData)
        }
        Ok(Message::Close(_)) => {
            let _ = sender
                .send(Err(ModelError::Transport(
                    "websocket closed by server before response.completed".to_string(),
                )))
                .await;
            WebSocketMessageOutcome::Failed(WebSocketCloseKind::RemoteClosed)
        }
        Ok(Message::Ping(payload)) => {
            match with_optional_timeout(
                timeout,
                "websocket pong send timeout",
                stream.send(Message::Pong(payload)),
            )
            .await
            {
                Ok(Ok(())) => WebSocketMessageOutcome::Continue,
                Ok(Err(error)) => {
                    let _ = sender.send(Err(map_websocket_error(error))).await;
                    WebSocketMessageOutcome::Failed(WebSocketCloseKind::InternalError)
                }
                Err(error) => {
                    let _ = sender.send(Err(error)).await;
                    WebSocketMessageOutcome::Failed(WebSocketCloseKind::Timeout)
                }
            }
        }
        Ok(Message::Pong(_) | Message::Frame(_)) => WebSocketMessageOutcome::Continue,
        Err(error) => {
            let close_kind = match error {
                WebSocketError::ConnectionClosed | WebSocketError::AlreadyClosed => {
                    WebSocketCloseKind::RemoteClosed
                }
                _ => WebSocketCloseKind::InternalError,
            };
            let _ = sender.send(Err(map_websocket_error(error))).await;
            WebSocketMessageOutcome::Failed(close_kind)
        }
    }
}

async fn handle_websocket_text_event(
    text: &str,
    stream: &mut ModelWebSocketStream,
    sender: &tokio::sync::mpsc::Sender<Result<Value, ModelError>>,
    close_on_completed: bool,
) -> WebSocketMessageOutcome {
    if let Some(error) = websocket_error_event(text) {
        let _ = sender.send(Err(error)).await;
        return WebSocketMessageOutcome::Failed(WebSocketCloseKind::ProviderError);
    }
    match serde_json::from_str::<Value>(text) {
        Ok(value) => {
            let terminal_event = response_terminal_event(&value);
            if sender.send(Ok(value)).await.is_err() {
                return WebSocketMessageOutcome::Failed(WebSocketCloseKind::GoingAway);
            }
            match terminal_event {
                Some(ResponseTerminalEvent::Completed) => {
                    if close_on_completed {
                        graceful_close_websocket(stream, WebSocketCloseKind::NormalCompletion)
                            .await;
                    }
                    WebSocketMessageOutcome::Completed
                }
                Some(ResponseTerminalEvent::Failed | ResponseTerminalEvent::Incomplete) => {
                    WebSocketMessageOutcome::Failed(WebSocketCloseKind::ProviderError)
                }
                None => WebSocketMessageOutcome::Continue,
            }
        }
        Err(error) => {
            let _ = sender
                .send(Err(ModelError::ResponseParsing(format!(
                    "invalid websocket JSON event: {error}"
                ))))
                .await;
            WebSocketMessageOutcome::Failed(WebSocketCloseKind::ProtocolError)
        }
    }
}

async fn with_optional_timeout<T>(
    timeout: Option<Duration>,
    timeout_message: &'static str,
    future: impl Future<Output = T>,
) -> Result<T, ModelError> {
    match timeout {
        Some(timeout) => tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| ModelError::Transport(timeout_message.to_string())),
        None => Ok(future.await),
    }
}

fn websocket_url_from_http_url(url: &str) -> Result<reqwest::Url, ModelError> {
    let mut url = reqwest::Url::parse(url)
        .map_err(|err| ModelError::Transport(format!("invalid websocket URL base: {err}")))?;
    let scheme = match url.scheme() {
        "https" => "wss",
        "http" => "ws",
        "ws" | "wss" => return Ok(url),
        scheme => {
            return Err(ModelError::Transport(format!(
                "unsupported websocket URL scheme: {scheme}"
            )));
        }
    };
    url.set_scheme(scheme).map_err(|()| {
        ModelError::Transport(format!("failed to convert URL to websocket scheme: {url}"))
    })?;
    Ok(url)
}

fn insert_headers(
    target: &mut tokio_tungstenite::tungstenite::http::HeaderMap,
    headers: &BTreeMap<String, String>,
) -> Result<(), ModelError> {
    for (name, value) in headers {
        let header_name = name.parse::<HeaderName>().map_err(|err| {
            ModelError::Transport(format!("invalid websocket header name {name}: {err}"))
        })?;
        let header_value = HeaderValue::from_str(value).map_err(|err| {
            ModelError::Transport(format!("invalid websocket header value for {name}: {err}"))
        })?;
        target.insert(header_name, header_value);
    }
    Ok(())
}

fn map_websocket_connect_error(error: WebSocketError) -> ModelError {
    match map_websocket_error(error) {
        ModelError::Transport(message) => {
            ModelError::Transport(format!("failed to connect websocket: {message}"))
        }
        error => error,
    }
}

fn map_websocket_send_error(error: WebSocketError) -> ModelError {
    match map_websocket_error(error) {
        ModelError::Transport(message) => {
            ModelError::Transport(format!("failed to send websocket request: {message}"))
        }
        error => error,
    }
}

fn map_websocket_error(error: WebSocketError) -> ModelError {
    match error {
        WebSocketError::Http(response) => {
            let status = response.status().as_u16();
            let body = response
                .body()
                .as_ref()
                .and_then(|bytes| String::from_utf8(bytes.clone()).ok())
                .and_then(|text| {
                    serde_json::from_str::<Value>(&text)
                        .ok()
                        .or(Some(Value::String(text)))
                })
                .unwrap_or(Value::Null);
            ModelError::ProviderStatus {
                status,
                body,
                retryable: is_retryable_status(status),
            }
        }
        WebSocketError::ConnectionClosed | WebSocketError::AlreadyClosed => {
            ModelError::Transport("websocket closed".to_string())
        }
        other => ModelError::Transport(other.to_string()),
    }
}

#[derive(Debug, Deserialize)]
struct WrappedWebsocketError {
    #[serde(default)]
    code: Option<String>,
    #[serde(rename = "type", default)]
    error_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WrappedWebsocketErrorEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(alias = "status_code")]
    status: Option<u16>,
    #[serde(default)]
    error: Option<WrappedWebsocketError>,
}

fn websocket_error_event(payload: &str) -> Option<ModelError> {
    let event: WrappedWebsocketErrorEvent = serde_json::from_str(payload).ok()?;
    if event.kind != "error" {
        return None;
    }
    let body = serde_json::from_str::<Value>(payload)
        .unwrap_or_else(|_| Value::String(payload.to_string()));
    if event.error.as_ref().is_some_and(|error| {
        error.code.as_deref() == Some(WEBSOCKET_CONNECTION_LIMIT_REACHED_CODE)
            || error.error_type.as_deref() == Some(WEBSOCKET_CONNECTION_LIMIT_REACHED_CODE)
    }) {
        return Some(ModelError::ProviderStatus {
            status: event.status.unwrap_or(400),
            body,
            retryable: true,
        });
    }
    let status = event.status?;
    if (200..300).contains(&status) {
        return None;
    }
    Some(ModelError::ProviderStatus {
        status,
        body,
        retryable: is_retryable_status(status),
    })
}

/// Return whether an error is safe for automatic WebSocket-to-HTTP fallback.
#[must_use]
pub fn should_fallback_websocket_to_http(error: &ModelError) -> bool {
    match error {
        ModelError::ProviderStatus {
            status,
            body,
            retryable,
        } => {
            *status == 426
                || (*retryable
                    && (*status == 400 || *status == 429 || *status >= 500)
                    && is_websocket_connection_limit_error(body))
        }
        ModelError::Transport(message) => {
            message.contains("websocket closed")
                || message.contains("failed to connect websocket")
                || message.contains("failed to send websocket request")
                || message.contains("Connection reset")
                || message.contains("connection reset")
        }
        ModelError::RetryExhausted { source, .. } => should_fallback_websocket_to_http(source),
        _ => false,
    }
}

fn is_websocket_connection_limit_error(body: &Value) -> bool {
    response_error_code(body).is_some_and(|code| code == WEBSOCKET_CONNECTION_LIMIT_REACHED_CODE)
        || body == &json!({"code": WEBSOCKET_CONNECTION_LIMIT_REACHED_CODE})
}

fn response_error_code(body: &Value) -> Option<&str> {
    body.get("error")
        .and_then(|error| error.get("code").or_else(|| error.get("type")))
        .or_else(|| {
            body.get("response")
                .and_then(|response| response.get("error"))
                .and_then(|error| error.get("code").or_else(|| error.get("type")))
        })
        .or_else(|| body.get("code"))
        .or_else(|| body.get("type"))
        .and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::{accept_async, tungstenite::Message};

    use super::*;
    use crate::{allow_real_model_requests_guard, transport::HttpMethod};

    fn result_or_panic<T, E: std::fmt::Debug>(result: Result<T, E>, message: &str) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("{message}: {error:?}"),
        }
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

    #[test]
    fn wrapped_connection_limit_error_is_retryable_and_fallback_safe() {
        let payload = json!({
            "type": "error",
            "status": 400,
            "error": {
                "type": "invalid_request_error",
                "code": "websocket_connection_limit_reached",
                "message": "Responses websocket connection limit reached"
            }
        })
        .to_string();

        let error =
            websocket_error_event(&payload).unwrap_or_else(|| panic!("error event should map"));

        assert!(matches!(
            error,
            ModelError::ProviderStatus {
                status: 400,
                retryable: true,
                ..
            }
        ));
        assert!(should_fallback_websocket_to_http(&error));
    }

    #[test]
    fn nested_response_connection_limit_error_is_fallback_safe() {
        let error = ModelError::ProviderStatus {
            status: 400,
            body: json!({
                "type": "response.failed",
                "response": {
                    "error": {
                        "code": "websocket_connection_limit_reached"
                    }
                }
            }),
            retryable: true,
        };

        assert!(should_fallback_websocket_to_http(&error));
    }

    #[test]
    fn ordinary_rate_limit_error_is_not_websocket_fallback_safe() {
        let payload = json!({
            "type": "error",
            "status": 429,
            "error": {
                "type": "usage_limit_reached",
                "message": "usage limit reached"
            }
        })
        .to_string();

        let error =
            websocket_error_event(&payload).unwrap_or_else(|| panic!("error event should map"));

        assert!(matches!(
            error,
            ModelError::ProviderStatus {
                status: 429,
                retryable: true,
                ..
            }
        ));
        assert!(!should_fallback_websocket_to_http(&error));
    }

    #[test]
    fn transport_setup_errors_are_fallback_safe() {
        assert!(should_fallback_websocket_to_http(&ModelError::Transport(
            "failed to connect websocket: network unreachable".to_string()
        )));
        assert!(should_fallback_websocket_to_http(&ModelError::Transport(
            "failed to send websocket request: websocket closed".to_string()
        )));
        assert!(!should_fallback_websocket_to_http(&ModelError::Transport(
            "invalid websocket JSON event".to_string()
        )));
    }

    #[test]
    fn websocket_config_lifts_default_response_event_limits() {
        let config = model_websocket_config();

        assert_eq!(
            config.max_message_size,
            Some(MODEL_WEBSOCKET_MAX_MESSAGE_SIZE)
        );
        assert_eq!(config.max_frame_size, Some(MODEL_WEBSOCKET_MAX_FRAME_SIZE));
        assert!(
            MODEL_WEBSOCKET_MAX_MESSAGE_SIZE
                > option_or_panic(
                    WebSocketConfig::default().max_message_size,
                    "default message size should be bounded",
                )
        );
        assert!(
            MODEL_WEBSOCKET_MAX_FRAME_SIZE
                > option_or_panic(
                    WebSocketConfig::default().max_frame_size,
                    "default frame size should be bounded",
                )
        );
    }

    #[tokio::test]
    async fn reusable_session_uses_one_connection_for_sequential_requests() {
        let _guard = allow_real_model_requests_guard();
        let listener = result_or_panic(
            TcpListener::bind("127.0.0.1:0").await,
            "bind websocket test listener",
        );
        let address = result_or_panic(listener.local_addr(), "listener local addr");
        let handshakes = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let server_handshakes = Arc::clone(&handshakes);
        let server_requests = Arc::clone(&requests);
        let server = tokio::spawn(async move {
            let (stream, _) = result_or_panic(listener.accept().await, "accept websocket client");
            server_handshakes.fetch_add(1, Ordering::SeqCst);
            let mut websocket =
                result_or_panic(accept_async(stream).await, "accept websocket upgrade");
            for index in 1..=2 {
                let message = result_or_panic(
                    option_or_panic(websocket.next().await, "websocket message"),
                    "valid websocket message",
                );
                let Message::Text(text) = message else {
                    panic!("expected text websocket request");
                };
                let body: Value =
                    result_or_panic(serde_json::from_str(&text), "valid request JSON");
                lock_or_panic(
                    server_requests.lock(),
                    "requests lock should not be poisoned",
                )
                .push(body);
                result_or_panic(
                    websocket
                    .send(Message::Text(
                        json!({
                            "type": "response.completed",
                            "response": {
                                "id": format!("resp_{index}"),
                                "status": "completed",
                                "output": [],
                                "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
                            }
                        })
                        .to_string()
                        .into(),
                    ))
                        .await,
                    "send completed event",
                );
            }
        });

        let mut session = ReusableWebSocketEventSession::default();
        let mut first_stream = result_or_panic(
            session
                .send_websocket_event_stream_incremental(test_ws_request(
                    address,
                    json!({"type": "response.create", "input": [{"role": "user", "content": "one"}]}),
                ))
                .await,
            "first websocket stream",
        );
        drain_events(&mut first_stream).await;
        let mut second_stream = result_or_panic(
            session
                .send_websocket_event_stream_incremental(test_ws_request(
                    address,
                    json!({"type": "response.create", "input": [{"role": "user", "content": "two"}]}),
                ))
                .await,
            "second websocket stream",
        );
        drain_events(&mut second_stream).await;
        result_or_panic(server.await, "websocket server task");

        assert_eq!(handshakes.load(Ordering::SeqCst), 1);
        let requests = lock_or_panic(requests.lock(), "requests lock should not be poisoned");
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0]["input"][0]
                .get("content")
                .and_then(Value::as_str),
            Some("one")
        );
        assert_eq!(
            requests[1]["input"][0]
                .get("content")
                .and_then(Value::as_str),
            Some("two")
        );
        drop(requests);
    }

    #[tokio::test]
    async fn reusable_session_waits_for_response_completion_before_next_request() {
        let _guard = allow_real_model_requests_guard();
        let listener = result_or_panic(
            TcpListener::bind("127.0.0.1:0").await,
            "bind websocket test listener",
        );
        let address = result_or_panic(listener.local_addr(), "listener local addr");
        let (release_first_response, wait_for_release) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = result_or_panic(listener.accept().await, "accept websocket client");
            let mut websocket =
                result_or_panic(accept_async(stream).await, "accept websocket upgrade");
            expect_request_with_content(&mut websocket, "one").await;
            result_or_panic(wait_for_release.await, "release signal should be sent");
            send_completed_event(&mut websocket, "resp_1").await;
            expect_request_with_content(&mut websocket, "two").await;
            send_completed_event(&mut websocket, "resp_2").await;
        });

        let mut session = ReusableWebSocketEventSession::default();
        let mut first_stream = result_or_panic(
            session
                .send_websocket_event_stream_incremental(test_ws_request(
                    address,
                    json!({"type": "response.create", "input": [{"role": "user", "content": "one"}]}),
                ))
                .await,
            "first websocket stream",
        );

        let second_request = session.send_websocket_event_stream_incremental(test_ws_request(
            address,
            json!({"type": "response.create", "input": [{"role": "user", "content": "two"}]}),
        ));
        tokio::pin!(second_request);
        let second_before_completion =
            tokio::time::timeout(Duration::from_millis(50), &mut second_request).await;
        assert!(second_before_completion.is_err());

        result_or_panic(
            release_first_response.send(()),
            "release receiver should still be alive",
        );
        drain_events(&mut first_stream).await;
        let mut second_stream = result_or_panic(second_request.await, "second websocket stream");
        drain_events(&mut second_stream).await;
        result_or_panic(server.await, "websocket server task");
    }

    fn test_ws_request(address: std::net::SocketAddr, body: Value) -> HttpRequest {
        HttpRequest {
            method: HttpMethod::Post,
            url: format!("http://{address}/v1/responses"),
            headers: BTreeMap::new(),
            body,
            timeout: Some(Duration::from_secs(5)),
            metadata: serde_json::Map::new(),
            cancellation_token: starweaver_core::CancellationToken::default(),
        }
    }

    async fn expect_request_with_content(
        websocket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        expected: &str,
    ) {
        let message = result_or_panic(
            option_or_panic(websocket.next().await, "websocket message"),
            "valid websocket message",
        );
        let Message::Text(text) = message else {
            panic!("expected text websocket request");
        };
        let body: Value = result_or_panic(serde_json::from_str(&text), "valid request JSON");
        assert_eq!(
            body["input"][0].get("content").and_then(Value::as_str),
            Some(expected)
        );
    }

    async fn send_completed_event(
        websocket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        response_id: &str,
    ) {
        result_or_panic(
            websocket
                .send(Message::Text(
                    json!({
                        "type": "response.completed",
                        "response": {
                            "id": response_id,
                            "status": "completed",
                            "output": [],
                            "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await,
            "send completed event",
        );
    }

    #[tokio::test]
    async fn per_request_websocket_sends_close_after_response_completion() {
        let _guard = allow_real_model_requests_guard();
        let listener = result_or_panic(
            TcpListener::bind("127.0.0.1:0").await,
            "bind websocket test listener",
        );
        let address = result_or_panic(listener.local_addr(), "listener local addr");
        let server = tokio::spawn(async move {
            let (stream, _) = result_or_panic(listener.accept().await, "accept websocket client");
            let mut websocket =
                result_or_panic(accept_async(stream).await, "accept websocket upgrade");
            expect_request_with_content(&mut websocket, "one").await;
            send_completed_event(&mut websocket, "resp_1").await;
            let message = result_or_panic(
                option_or_panic(websocket.next().await, "client close frame"),
                "valid websocket close frame",
            );
            let Message::Close(frame) = message else {
                panic!("expected client close frame");
            };
            let frame = option_or_panic(frame, "close frame payload");
            assert_eq!(frame.code, CloseCode::Normal);
            assert_eq!(frame.reason.to_string(), "response completed");
        });

        let mut stream = result_or_panic(
            send_websocket_event_stream_incremental(test_ws_request(
                address,
                json!({"type": "response.create", "input": [{"role": "user", "content": "one"}]}),
            ))
            .await,
            "websocket stream",
        );
        drain_events(&mut stream).await;
        result_or_panic(server.await, "websocket server task");
    }

    #[tokio::test]
    async fn reusable_session_reset_sends_close_frame() {
        let _guard = allow_real_model_requests_guard();
        let listener = result_or_panic(
            TcpListener::bind("127.0.0.1:0").await,
            "bind websocket test listener",
        );
        let address = result_or_panic(listener.local_addr(), "listener local addr");
        let server = tokio::spawn(async move {
            let (stream, _) = result_or_panic(listener.accept().await, "accept websocket client");
            let mut websocket =
                result_or_panic(accept_async(stream).await, "accept websocket upgrade");
            expect_request_with_content(&mut websocket, "one").await;
            send_completed_event(&mut websocket, "resp_1").await;
            let message = result_or_panic(
                option_or_panic(websocket.next().await, "client close frame"),
                "valid websocket close frame",
            );
            let Message::Close(frame) = message else {
                panic!("expected client close frame");
            };
            let frame = option_or_panic(frame, "close frame payload");
            assert_eq!(frame.code, CloseCode::Normal);
            assert_eq!(frame.reason.to_string(), "session closed");
        });

        let mut session = ReusableWebSocketEventSession::default();
        let mut stream = result_or_panic(
            session
                .send_websocket_event_stream_incremental(test_ws_request(
                    address,
                    json!({"type": "response.create", "input": [{"role": "user", "content": "one"}]}),
                ))
                .await,
            "websocket stream",
        );
        drain_events(&mut stream).await;
        session.reset().await;
        result_or_panic(server.await, "websocket server task");
    }

    #[tokio::test]
    async fn dropping_event_stream_sends_going_away_close_frame() {
        let _guard = allow_real_model_requests_guard();
        let listener = result_or_panic(
            TcpListener::bind("127.0.0.1:0").await,
            "bind websocket test listener",
        );
        let address = result_or_panic(listener.local_addr(), "listener local addr");
        let server = tokio::spawn(async move {
            let (stream, _) = result_or_panic(listener.accept().await, "accept websocket client");
            let mut websocket =
                result_or_panic(accept_async(stream).await, "accept websocket upgrade");
            expect_request_with_content(&mut websocket, "one").await;
            let message = result_or_panic(
                option_or_panic(websocket.next().await, "client close frame"),
                "valid websocket close frame",
            );
            let Message::Close(frame) = message else {
                panic!("expected client close frame");
            };
            let frame = option_or_panic(frame, "close frame payload");
            assert_eq!(frame.code, CloseCode::Away);
            assert_eq!(frame.reason.to_string(), "receiver dropped");
        });

        let stream = result_or_panic(
            send_websocket_event_stream_incremental(test_ws_request(
                address,
                json!({"type": "response.create", "input": [{"role": "user", "content": "one"}]}),
            ))
            .await,
            "websocket stream",
        );
        drop(stream);
        result_or_panic(server.await, "websocket server task");
    }

    async fn drain_events(events: &mut ModelEventStream) {
        while let Some(event) = events.recv().await {
            let _ = result_or_panic(event, "websocket event should be valid");
        }
    }
}
