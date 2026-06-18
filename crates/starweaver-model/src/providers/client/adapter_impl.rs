use async_trait::async_trait;
use serde_json::Value;

use crate::{
    adapter::{allow_real_model_requests, ModelRequestContext, ModelRequestParameters},
    message::{ModelMessage, ModelResponse},
    profile::{ModelProfile, ProtocolFamily},
    request::prepare_model_request,
    settings::ModelSettings,
    transport::{build_http_request, send_with_retries},
    ModelAdapter, ModelError, ModelResponseEventStream, ModelResponseStreamEvent,
};

use super::ProtocolModelClient;

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
        let mut wire_body = self.build_wire_body(
            &prepared.normalized_messages,
            prepared.settings.as_ref(),
            &prepared.params,
        )?;
        if let Some(object) = wire_body.as_object_mut() {
            object.insert("stream".to_string(), Value::Bool(true));
        }
        let options = self.request_options(&context, prepared.settings.as_ref(), &prepared.params);
        let mut request = build_http_request(&self.http_config, &options, wire_body);
        request.cancellation_token = cancellation_token.clone();
        self.finalize_http_request(&mut request);
        if let Some(audit) = self.request_audit.as_ref() {
            audit.record(&self.provider_name, &self.model_name, true, &request);
        }
        if !allow_real_model_requests() {
            return Err(ModelError::RealModelRequestBlocked { url: request.url });
        }
        let mut events = self
            .http_client
            .send_event_stream_incremental(request)
            .await?;
        let (sender, receiver) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            let mut parser =
                crate::providers::openai_responses::OpenAiResponsesStreamParser::default();
            while let Some(event) = events.recv().await {
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        let _ = sender.send(Err(error)).await;
                        return;
                    }
                };
                let events = match parser.push_event(&event) {
                    Ok(events) => events,
                    Err(error) => {
                        let _ = sender.send(Err(error)).await;
                        return;
                    }
                };
                for event in events {
                    if sender.send(Ok(event)).await.is_err() {
                        return;
                    }
                }
            }
            match parser.finish() {
                Ok(events) => {
                    for event in events {
                        if sender.send(Ok(event)).await.is_err() {
                            return;
                        }
                    }
                }
                Err(error) => {
                    let _ = sender.send(Err(error)).await;
                }
            }
        });
        Ok(ModelResponseEventStream::new_with_cancellation(
            receiver,
            cancellation_token,
        ))
    }
}
