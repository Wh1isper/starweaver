use std::collections::BTreeMap;

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;

use crate::{allow_real_model_requests, ModelError};

use super::sse::{push_sse_utf8_buffer, send_sse_parser_events, SseJsonParser, StreamSendError};
use super::{HttpMethod, HttpRequest, HttpResponse, ModelEventStream, ModelHttpClient};
use crate::transport::is_retryable_status;

/// Reqwest-backed HTTP client.
#[derive(Clone, Debug)]
pub struct ReqwestHttpClient {
    client: reqwest::Client,
}

impl ReqwestHttpClient {
    /// Create a reqwest-backed client with rustls TLS.
    ///
    /// # Errors
    ///
    /// Returns an error when reqwest client construction fails.
    pub fn new() -> Result<Self, ModelError> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|err| ModelError::Transport(err.to_string()))?;
        Ok(Self { client })
    }

    async fn send_request(&self, request: &HttpRequest) -> Result<reqwest::Response, ModelError> {
        if !allow_real_model_requests() {
            return Err(ModelError::RealModelRequestBlocked {
                url: request.url.clone(),
            });
        }

        let mut builder = match request.method {
            HttpMethod::Post => self.client.post(&request.url),
        }
        .headers(Self::header_map(&request.headers)?)
        .json(&request.body);

        if let Some(timeout) = request.timeout {
            builder = builder.timeout(timeout);
        }

        builder
            .send()
            .await
            .map_err(|err| ModelError::Transport(err.to_string()))
    }

    fn header_map(headers: &BTreeMap<String, String>) -> Result<HeaderMap, ModelError> {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|err| {
                ModelError::Transport(format!("invalid header name {name}: {err}"))
            })?;
            let value = HeaderValue::from_str(value).map_err(|err| {
                ModelError::Transport(format!("invalid header value for {name}: {err}"))
            })?;
            map.insert(name, value);
        }
        Ok(map)
    }
}

#[async_trait]
impl ModelHttpClient for ReqwestHttpClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, ModelError> {
        let response = self.send_request(&request).await?;
        let status = response.status().as_u16();
        let headers = response_headers(&response);
        let body = response
            .json::<Value>()
            .await
            .map_err(|err| ModelError::Transport(err.to_string()))?;

        if (200..300).contains(&status) {
            Ok(HttpResponse {
                status,
                headers,
                body,
            })
        } else {
            Err(ModelError::ProviderStatus {
                status,
                body,
                retryable: is_retryable_status(status),
            })
        }
    }

    async fn send_event_stream_incremental(
        &self,
        request: HttpRequest,
    ) -> Result<ModelEventStream, ModelError> {
        let response = self.send_request(&request).await?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let text = response
                .text()
                .await
                .map_err(|err| ModelError::Transport(err.to_string()))?;
            let body = serde_json::from_str(&text).unwrap_or(Value::String(text));
            return Err(ModelError::ProviderStatus {
                status,
                body,
                retryable: is_retryable_status(status),
            });
        }
        let (sender, receiver) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            let mut parser = SseJsonParser::default();
            let mut bytes = response.bytes_stream();
            let mut utf8_buffer = Vec::new();
            while let Some(chunk) = bytes.next().await {
                match chunk {
                    Ok(bytes) => {
                        utf8_buffer.extend_from_slice(&bytes);
                        match push_sse_utf8_buffer(&sender, &mut parser, &mut utf8_buffer).await {
                            Ok(()) => {}
                            Err(StreamSendError::Closed) => return,
                            Err(StreamSendError::InvalidUtf8(error)) => {
                                let _ = sender
                                    .send(Err(ModelError::ResponseParsing(format!(
                                        "invalid server-sent event UTF-8: {error}"
                                    ))))
                                    .await;
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        let _ = sender
                            .send(Err(ModelError::Transport(error.to_string())))
                            .await;
                        return;
                    }
                }
            }
            if !utf8_buffer.is_empty() {
                match std::str::from_utf8(&utf8_buffer) {
                    Ok(text) => {
                        if !send_sse_parser_events(&sender, parser.push_str(text)).await {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = sender
                            .send(Err(ModelError::ResponseParsing(format!(
                                "invalid server-sent event UTF-8: {error}"
                            ))))
                            .await;
                        return;
                    }
                }
            }
            let _ = send_sse_parser_events(&sender, parser.finish()).await;
        });
        Ok(ModelEventStream::new(receiver))
    }
}

fn response_headers(response: &reqwest::Response) -> BTreeMap<String, String> {
    response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect()
}
