use serde_json::json;

use crate::{
    adapter::{ModelRequestContext, ModelRequestParameters},
    settings::ModelSettings,
    transport::HttpRequestOptions,
};

use super::ProtocolModelClient;

impl ProtocolModelClient {
    pub(super) fn request_options(
        context: &ModelRequestContext,
        settings: Option<&ModelSettings>,
        params: &ModelRequestParameters,
    ) -> HttpRequestOptions {
        let mut options = params.http.clone();
        if let Some(settings) = settings {
            options.headers.extend(settings.extra_headers.clone());
            options.extra_body.extend(settings.extra_body.clone());
            options.timeout_ms = options.timeout_ms.or(settings.timeout_ms);
        }
        options.extra_body.extend(params.extra_body.clone());
        options.metadata.extend(params.metadata.clone());
        options.metadata.extend(context.llm_trace_metadata.clone());
        options.metadata.insert(
            "starweaver.run_id".to_string(),
            json!(context.run_id.as_str()),
        );
        options.metadata.insert(
            "starweaver.conversation_id".to_string(),
            json!(context.conversation_id.as_str()),
        );
        options
    }
}
