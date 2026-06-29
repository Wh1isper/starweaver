//! Response assembly from streamed `OpenAI` Responses parts.

use serde_json::{json, Value};
use starweaver_usage::Usage;

use crate::{
    message::{
        Metadata, ModelResponse, ModelResponsePart, ProviderInfo, ProviderPartInfo, ToolCallPart,
    },
    providers::parse_tool_call_arguments,
};

use super::{OpenAiResponsesStreamParser, StreamedFunctionCall, StreamedOpaqueItems};

impl OpenAiResponsesStreamParser {
    pub(super) fn response_with_streamed_parts_fallback(
        &self,
        mut response: ModelResponse,
    ) -> ModelResponse {
        let has_text = !response.text_output().is_empty();
        let has_thinking = response.parts.iter().any(|part| {
            matches!(
                part,
                ModelResponsePart::Thinking { .. } | ModelResponsePart::ProviderThinking { .. }
            )
        });
        let existing_tool_keys = response
            .tool_calls()
            .into_iter()
            .map(|call| tool_call_key(&call.id, &call.name))
            .collect::<std::collections::BTreeSet<_>>();
        let mut prefix = Vec::new();
        if !has_thinking && (!self.reasoning.is_empty() || self.reasoning_signature.is_some()) {
            prefix.push(self.streamed_reasoning_part());
        }
        if !prefix.is_empty() {
            prefix.extend(response.parts);
            response.parts = prefix;
        }
        if !has_text && !self.text.is_empty() {
            response.parts.push(ModelResponsePart::Text {
                text: self.text.clone(),
            });
        }
        for part in self.streamed_tool_call_parts() {
            let Some(call) = part.tool_call() else {
                continue;
            };
            if !existing_tool_keys.contains(&tool_call_key(&call.id, &call.name)) {
                response.parts.push(part);
            }
        }
        append_missing_opaque_items(&mut response.parts, &self.opaque_items);
        response
    }

    pub(super) fn response_from_streamed_parts(&self) -> ModelResponse {
        self.response_with_streamed_parts_fallback(ModelResponse {
            parts: Vec::new(),
            usage: Usage::default(),
            model_name: None,
            provider: Some(ProviderInfo {
                name: "openai".to_string(),
                response_id: None,
                details: serde_json::Map::new(),
            }),
            finish_reason: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        })
    }

    fn streamed_reasoning_part(&self) -> ModelResponsePart {
        let mut provider = ProviderPartInfo::new("openai");
        if let Some(id) = &self.reasoning_item_id {
            provider = provider.with_id(id.clone());
        }
        if !self.reasoning_details.is_empty() {
            provider = provider.with_details(self.reasoning_details.clone());
        }
        ModelResponsePart::ProviderThinking {
            text: self.reasoning.clone(),
            signature: self.reasoning_signature.clone(),
            provider,
        }
    }

    fn streamed_tool_call_parts(&self) -> Vec<ModelResponsePart> {
        let mut calls = self.function_calls.values().collect::<Vec<_>>();
        calls.sort_by_key(|call| call.index);
        calls
            .into_iter()
            .filter(|call| !call.name.is_empty())
            .map(streamed_tool_call_part)
            .collect()
    }
}

fn append_missing_opaque_items(
    parts: &mut Vec<ModelResponsePart>,
    opaque_items: &StreamedOpaqueItems,
) {
    let existing_ids = parts
        .iter()
        .filter_map(|part| {
            part.provider_part()
                .and_then(|provider| provider.id.as_deref())
        })
        .map(str::to_string)
        .collect::<std::collections::BTreeSet<_>>();
    let mut items = opaque_items.iter().collect::<Vec<_>>();
    items.sort_by_key(|(key, _)| *key);
    for (key, item) in items {
        let id = item
            .get("id")
            .or_else(|| item.get("call_id"))
            .and_then(Value::as_str)
            .unwrap_or(key);
        if existing_ids.contains(id) {
            continue;
        }
        let item_type = item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("response_item")
            .to_string();
        parts.push(ModelResponsePart::ProviderOpaque {
            item_type,
            payload: item.clone(),
            provider: ProviderPartInfo::new("openai").with_id(id.to_string()),
        });
    }
}

fn streamed_tool_call_part(call: &StreamedFunctionCall) -> ModelResponsePart {
    let runtime_call = ToolCallPart {
        id: if call.call_id.is_empty() {
            call.item_id.clone()
        } else {
            call.call_id.clone()
        },
        name: call.name.clone(),
        arguments: parse_tool_call_arguments(&Value::String(call.arguments.clone())),
    };
    let mut details = Metadata::default();
    if let Some(namespace) = &call.namespace {
        details.insert("namespace".to_string(), json!(namespace));
    }
    if let Some(status) = &call.status {
        details.insert("status".to_string(), json!(status));
    }
    let provider = ProviderPartInfo::new("openai")
        .with_id(call.item_id.clone())
        .with_details(details);
    ModelResponsePart::ProviderToolCall {
        call: runtime_call,
        provider,
    }
}

fn tool_call_key(id: &str, name: &str) -> String {
    if id.is_empty() {
        format!("name:{name}")
    } else {
        format!("id:{id}")
    }
}
