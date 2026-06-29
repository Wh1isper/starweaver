use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::BTreeMap, future::Future, time::Duration};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        http::{HeaderName, HeaderValue},
        Error as WebSocketError, Message,
    },
};

use crate::{allow_real_model_requests, transport::is_retryable_status, ModelError};

use super::{HttpRequest, ModelEventStream};

type ModelWebSocketStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const WEBSOCKET_CONNECTION_LIMIT_REACHED_CODE: &str = "websocket_connection_limit_reached";

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
    let mut stream = Box::pin(connect_websocket_stream(&request)).await?;
    send_websocket_request_body(&mut stream, &request).await?;

    let (sender, receiver) = tokio::sync::mpsc::channel(32);
    spawn_websocket_event_worker(stream, sender, cancellation_token.clone(), request.timeout);
    Ok(ModelEventStream::new_with_cancellation(
        receiver,
        cancellation_token,
    ))
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

    let connect = connect_async(websocket_request);
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
    timeout: Option<Duration>,
) {
    tokio::spawn(async move {
        loop {
            let message =
                next_websocket_message(&mut stream, &sender, &cancellation_token, timeout).await;
            let Some(message) = message else { return };
            if handle_websocket_message(message, &mut stream, &sender, timeout).await {
                return;
            }
        }
    });
}

async fn next_websocket_message(
    stream: &mut ModelWebSocketStream,
    sender: &tokio::sync::mpsc::Sender<Result<Value, ModelError>>,
    cancellation_token: &starweaver_core::CancellationToken,
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
            let _ = with_optional_timeout(timeout, "websocket close timeout", stream.close(None)).await;
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
                    None
                }
                Err(error) => {
                    let _ = sender.send(Err(error)).await;
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
) -> bool {
    match message {
        Ok(Message::Text(text)) => {
            handle_websocket_text_event(&text, stream, sender, timeout).await
        }
        Ok(Message::Binary(_)) => {
            let _ = sender
                .send(Err(ModelError::ResponseParsing(
                    "unexpected binary websocket event".to_string(),
                )))
                .await;
            true
        }
        Ok(Message::Close(_)) => {
            let _ = sender
                .send(Err(ModelError::Transport(
                    "websocket closed by server before response.completed".to_string(),
                )))
                .await;
            true
        }
        Ok(Message::Ping(payload)) => {
            match with_optional_timeout(
                timeout,
                "websocket pong send timeout",
                stream.send(Message::Pong(payload)),
            )
            .await
            {
                Ok(Ok(())) => false,
                Ok(Err(error)) => {
                    let _ = sender.send(Err(map_websocket_error(error))).await;
                    true
                }
                Err(error) => {
                    let _ = sender.send(Err(error)).await;
                    true
                }
            }
        }
        Ok(Message::Pong(_) | Message::Frame(_)) => false,
        Err(error) => {
            let _ = sender.send(Err(map_websocket_error(error))).await;
            true
        }
    }
}

async fn handle_websocket_text_event(
    text: &str,
    stream: &mut ModelWebSocketStream,
    sender: &tokio::sync::mpsc::Sender<Result<Value, ModelError>>,
    timeout: Option<Duration>,
) -> bool {
    if let Some(error) = websocket_error_event(text) {
        let _ = sender.send(Err(error)).await;
        return true;
    }
    match serde_json::from_str::<Value>(text) {
        Ok(value) => {
            let completed = value.get("type").and_then(Value::as_str) == Some("response.completed");
            if sender.send(Ok(value)).await.is_err() {
                return true;
            }
            if completed {
                let _ =
                    with_optional_timeout(timeout, "websocket close timeout", stream.close(None))
                        .await;
            }
            completed
        }
        Err(error) => {
            let _ = sender
                .send(Err(ModelError::ResponseParsing(format!(
                    "invalid websocket JSON event: {error}"
                ))))
                .await;
            true
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
            )))
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
            *retryable
                && (*status == 400 || *status == 429 || *status >= 500)
                && is_websocket_connection_limit_error(body)
        }
        ModelError::Transport(message) => {
            message.contains("websocket closed")
                || message.contains("failed to connect websocket")
                || message.contains("failed to send websocket request")
                || message.contains("Connection reset")
                || message.contains("connection reset")
        }
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
    use super::*;

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
}
