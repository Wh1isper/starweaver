//! `OpenAI` Responses wire mapper.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use starweaver_core::Usage;

use crate::{
    adapter::{NativeToolDefinition, ToolDefinition},
    message::{
        ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart, ProviderInfo,
        ToolCallPart,
    },
    providers::{
        apply_common_settings_with_max_tokens, finish_reason_openai, insert_optional_description,
        openai_responses_content, parse_tool_call_arguments, provider_tool_parameters,
        usage_from_openai,
    },
    transport::MaxTokensParameter,
    ModelError, ModelResponseStreamEvent, ModelSettings,
};

/// `OpenAI` Responses wire mapper.
pub struct OpenAiResponsesAdapter;

impl OpenAiResponsesAdapter {
    /// Build a provider wire request.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical history cannot be mapped into response items.
    pub fn build_request(
        model: &str,
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        tools: &[ToolDefinition],
        native_tools: &[NativeToolDefinition],
    ) -> Result<Value, ModelError> {
        Self::build_request_with_options(
            model,
            messages,
            settings,
            tools,
            native_tools,
            MaxTokensParameter::Default,
        )
    }

    /// Build a provider wire request with explicit gateway/provider options.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical history cannot be mapped into response items.
    pub fn build_request_with_options(
        model: &str,
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        tools: &[ToolDefinition],
        native_tools: &[NativeToolDefinition],
        max_tokens_parameter: MaxTokensParameter,
    ) -> Result<Value, ModelError> {
        let mut input = Vec::new();
        let mut instructions = Vec::new();

        for message in messages {
            match message {
                ModelMessage::Request(request) => {
                    if let Some(request_instructions) = request.instructions.as_ref() {
                        if !request_instructions.trim().is_empty() {
                            instructions.push(request_instructions.clone());
                        }
                    }
                    for part in &request.parts {
                        match part {
                            ModelRequestPart::SystemPrompt { text, .. }
                            | ModelRequestPart::Instruction { text, .. } => {
                                instructions.push(text.clone());
                            }
                            ModelRequestPart::UserPrompt { content, .. } => input.push(json!({
                                "role": "user",
                                "content": openai_responses_content(content)
                            })),
                            ModelRequestPart::ToolReturn(tool_return) => input.push(json!({
                                "type": "function_call_output",
                                "call_id": tool_return.tool_call_id,
                                "output": tool_return.content.to_string(),
                            })),
                            ModelRequestPart::RetryPrompt { text, .. } => input.push(json!({
                                "role": "user",
                                "content": [{"type": "input_text", "text": text}]
                            })),
                        }
                    }
                }
                ModelMessage::Response(response) => {
                    let text = response.text_output();
                    if !text.is_empty() {
                        input.push(json!({
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": text}]
                        }));
                    }
                    for call in response.tool_calls() {
                        input.push(json!({
                            "type": "function_call",
                            "call_id": call.id,
                            "name": call.name,
                            "arguments": call.arguments.wire_json_string(),
                        }));
                    }
                }
            }
        }

        let mut request = serde_json::Map::new();
        request.insert("model".to_string(), json!(model));
        request.insert("input".to_string(), json!(input));
        if !instructions.is_empty() {
            request.insert("instructions".to_string(), json!(instructions.join("\n\n")));
        }
        apply_common_settings_with_max_tokens(&mut request, settings, max_tokens_parameter);
        if let Some(thinking) = settings.and_then(|settings| settings.thinking.as_ref()) {
            let mut reasoning = serde_json::Map::new();
            reasoning.insert("effort".to_string(), json!(thinking.effort));
            if let Some(summary) = &thinking.summary {
                reasoning.insert("summary".to_string(), json!(summary));
            }
            request.insert("reasoning".to_string(), Value::Object(reasoning));
            request.remove("reasoning_effort");
        }
        if let Some(tool_choice) = settings.and_then(|settings| settings.tool_choice.as_ref()) {
            request.insert(
                "tool_choice".to_string(),
                crate::providers::openai_responses_tool_choice(tool_choice),
            );
        }
        let tool_defs = response_tool_defs(tools, native_tools);
        if !tool_defs.is_empty() {
            request.insert("tools".to_string(), json!(tool_defs));
        }
        Ok(Value::Object(request))
    }

    /// Parse a provider wire response.
    ///
    /// # Errors
    ///
    /// Returns an error when required response item structure is malformed.
    pub fn parse_response(value: &Value) -> Result<ModelResponse, ModelError> {
        let mut parts = Vec::new();
        for item in value
            .get("output")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            parse_response_item(item, &mut parts);
        }

        Ok(ModelResponse {
            parts,
            usage: usage_from_openai(value),
            model_name: value
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string),
            provider: Some(ProviderInfo {
                name: "openai".to_string(),
                response_id: value.get("id").and_then(Value::as_str).map(str::to_string),
            }),
            finish_reason: value
                .get("status")
                .and_then(Value::as_str)
                .map(finish_reason_openai),
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        })
    }

    /// Parse `OpenAI` Responses server-sent JSON events into canonical stream events.
    ///
    /// # Errors
    ///
    /// Returns an error when no completed response is present in the event list.
    pub fn parse_stream_events(
        events: &[Value],
    ) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        let mut parser = OpenAiResponsesStreamParser::default();
        let mut stream = Vec::new();
        for event in events {
            stream.extend(parser.push_event(event)?);
        }
        stream.extend(parser.finish()?);
        if stream
            .iter()
            .any(|event| matches!(event, ModelResponseStreamEvent::FinalResult(_)))
        {
            Ok(stream)
        } else {
            Err(ModelError::ResponseParsing(
                "missing response.completed event".to_string(),
            ))
        }
    }
}

#[derive(Clone, Debug, Default)]
struct StreamedFunctionCall {
    index: usize,
    item_id: String,
    call_id: String,
    name: String,
    arguments: String,
    started: bool,
    ended: bool,
}

/// Incremental parser for `OpenAI` Responses server-sent JSON payloads.
#[derive(Default)]
pub struct OpenAiResponsesStreamParser {
    text_started: bool,
    text: String,
    reasoning_started: bool,
    reasoning: String,
    function_calls: BTreeMap<String, StreamedFunctionCall>,
    next_tool_index: usize,
    final_seen: bool,
}

impl OpenAiResponsesStreamParser {
    /// Push one provider event and return zero or more canonical stream events.
    ///
    /// # Errors
    ///
    /// Returns an error when a completed response payload is malformed.
    pub fn push_event(
        &mut self,
        event: &Value,
    ) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        let mut stream = Vec::new();
        match event.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                if !self.text_started {
                    self.text_started = true;
                    stream.push(ModelResponseStreamEvent::PartStart(crate::PartStart {
                        index: 0,
                        part_kind: "text".to_string(),
                    }));
                }
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    self.text.push_str(delta);
                    stream.push(ModelResponseStreamEvent::PartDelta(crate::PartDelta::text(
                        0, delta,
                    )));
                }
            }
            Some("response.output_text.done") if self.text_started => {
                self.end_text_part(&mut stream);
            }
            Some(
                "response.reasoning_summary_text.delta"
                | "response.reasoning_summary.delta"
                | "response.reasoning.delta",
            ) => {
                self.push_reasoning_delta(event, &mut stream);
            }
            Some(
                "response.reasoning_summary_text.done"
                | "response.reasoning_summary.done"
                | "response.reasoning.done",
            ) if self.reasoning_started => {
                self.end_reasoning_part(&mut stream);
            }
            Some("response.output_item.added") => {
                self.push_output_item_added(event, &mut stream);
            }
            Some("response.function_call_arguments.delta") => {
                self.push_function_call_arguments_delta(event, &mut stream);
            }
            Some("response.function_call_arguments.done") => {
                self.push_function_call_arguments_done(event, &mut stream);
            }
            Some("response.output_item.done") => {
                self.push_output_item_done(event, &mut stream);
            }
            Some("response.completed") => {
                self.end_open_parts(&mut stream);
                let response = event
                    .get("response")
                    .map(OpenAiResponsesAdapter::parse_response)
                    .transpose()?
                    .map_or_else(
                        || self.response_from_streamed_parts(),
                        |response| self.response_with_streamed_parts_fallback(response),
                    );
                stream.push(ModelResponseStreamEvent::FinalResult(Box::new(response)));
                self.final_seen = true;
            }
            _ => {}
        }
        Ok(stream)
    }

    fn push_reasoning_delta(&mut self, event: &Value, stream: &mut Vec<ModelResponseStreamEvent>) {
        if !self.reasoning_started {
            self.reasoning_started = true;
            stream.push(ModelResponseStreamEvent::PartStart(crate::PartStart {
                index: 1,
                part_kind: "thinking".to_string(),
            }));
        }
        if let Some(delta) = event
            .get("delta")
            .or_else(|| event.get("text"))
            .and_then(Value::as_str)
        {
            self.reasoning.push_str(delta);
            stream.push(ModelResponseStreamEvent::PartDelta(
                crate::PartDelta::thinking(1, delta),
            ));
        }
    }

    fn end_text_part(&mut self, stream: &mut Vec<ModelResponseStreamEvent>) {
        stream.push(ModelResponseStreamEvent::PartEnd(crate::PartEnd {
            index: 0,
            part_kind: Some("text".to_string()),
        }));
        self.text_started = false;
    }

    fn end_reasoning_part(&mut self, stream: &mut Vec<ModelResponseStreamEvent>) {
        stream.push(ModelResponseStreamEvent::PartEnd(crate::PartEnd {
            index: 1,
            part_kind: Some("thinking".to_string()),
        }));
        self.reasoning_started = false;
    }

    fn end_open_parts(&mut self, stream: &mut Vec<ModelResponseStreamEvent>) {
        if self.reasoning_started {
            self.end_reasoning_part(stream);
        }
        if self.text_started {
            self.end_text_part(stream);
        }
    }

    fn push_output_item_added(
        &mut self,
        event: &Value,
        stream: &mut Vec<ModelResponseStreamEvent>,
    ) {
        let Some(item) = event.get("item") else {
            return;
        };
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return;
        }
        let key = function_call_item_key(event, item);
        self.ensure_function_call_started(&key, item, stream);
        self.update_function_call_from_item(&key, item, stream, false);
    }

    fn push_function_call_arguments_delta(
        &mut self,
        event: &Value,
        stream: &mut Vec<ModelResponseStreamEvent>,
    ) {
        let Some(key) = event.get("item_id").and_then(Value::as_str) else {
            return;
        };
        let key = key.to_string();
        self.ensure_function_call_started(&key, &Value::Null, stream);
        if let Some(delta) = event.get("delta").and_then(Value::as_str) {
            if delta.is_empty() {
                return;
            }
            if let Some(call) = self.function_calls.get_mut(&key) {
                call.arguments.push_str(delta);
                stream.push(ModelResponseStreamEvent::PartDelta(crate::PartDelta {
                    index: call.index,
                    delta: crate::StreamDelta::ToolCallArguments {
                        arguments_delta: delta.to_string(),
                    },
                }));
            }
        }
    }

    fn push_function_call_arguments_done(
        &mut self,
        event: &Value,
        stream: &mut Vec<ModelResponseStreamEvent>,
    ) {
        let Some(key) = event.get("item_id").and_then(Value::as_str) else {
            return;
        };
        let key = key.to_string();
        self.ensure_function_call_started(&key, &Value::Null, stream);
        let Some(arguments) = event
            .get("arguments")
            .or_else(|| event.get("delta"))
            .and_then(Value::as_str)
        else {
            return;
        };
        self.update_function_call_arguments(&key, arguments, stream);
    }

    fn push_output_item_done(&mut self, event: &Value, stream: &mut Vec<ModelResponseStreamEvent>) {
        let Some(item) = event.get("item") else {
            return;
        };
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return;
        }
        let key = function_call_item_key(event, item);
        self.ensure_function_call_started(&key, item, stream);
        self.update_function_call_from_item(&key, item, stream, true);
        if let Some(call) = self.function_calls.get_mut(&key) {
            if !call.ended {
                stream.push(ModelResponseStreamEvent::PartEnd(crate::PartEnd {
                    index: call.index,
                    part_kind: Some("tool_call".to_string()),
                }));
                call.ended = true;
            }
        }
    }

    fn ensure_function_call_started(
        &mut self,
        key: &str,
        item: &Value,
        stream: &mut Vec<ModelResponseStreamEvent>,
    ) {
        if self.next_tool_index == 0 {
            self.next_tool_index = 2;
        }
        let mut start_index = None;
        let call = self
            .function_calls
            .entry(key.to_string())
            .or_insert_with(|| {
                let index = self.next_tool_index;
                self.next_tool_index = self.next_tool_index.saturating_add(1);
                StreamedFunctionCall {
                    index,
                    item_id: item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or(key)
                        .to_string(),
                    call_id: item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or(key)
                        .to_string(),
                    name: String::new(),
                    arguments: String::new(),
                    started: false,
                    ended: false,
                }
            });
        if !call.started {
            call.started = true;
            start_index = Some(call.index);
        }
        if let Some(index) = start_index {
            stream.push(ModelResponseStreamEvent::PartStart(crate::PartStart {
                index,
                part_kind: "tool_call".to_string(),
            }));
        }
    }

    fn update_function_call_from_item(
        &mut self,
        key: &str,
        item: &Value,
        stream: &mut Vec<ModelResponseStreamEvent>,
        final_item: bool,
    ) {
        let Some(call) = self.function_calls.get_mut(key) else {
            return;
        };
        if let Some(item_id) = item.get("id").and_then(Value::as_str) {
            call.item_id = item_id.to_string();
        }
        if let Some(call_id) = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
        {
            call.call_id = call_id.to_string();
        }
        if let Some(name) = item.get("name").and_then(Value::as_str) {
            if !name.is_empty() && call.name != name {
                call.name = name.to_string();
                stream.push(ModelResponseStreamEvent::PartDelta(crate::PartDelta {
                    index: call.index,
                    delta: crate::StreamDelta::ToolCallName {
                        name: name.to_string(),
                    },
                }));
            }
        }
        let arguments = item.get("arguments").and_then(Value::as_str);
        if let Some(arguments) = arguments {
            self.update_function_call_arguments(key, arguments, stream);
        } else if final_item && call.arguments.is_empty() {
            call.arguments = "{}".to_string();
        }
    }

    fn update_function_call_arguments(
        &mut self,
        key: &str,
        arguments: &str,
        stream: &mut Vec<ModelResponseStreamEvent>,
    ) {
        let Some(call) = self.function_calls.get_mut(key) else {
            return;
        };
        if arguments.is_empty() || call.arguments == arguments {
            return;
        }
        let delta = if call.arguments.is_empty() {
            Some(arguments.to_string())
        } else {
            arguments
                .strip_prefix(&call.arguments)
                .filter(|suffix| !suffix.is_empty())
                .map(ToString::to_string)
        };
        call.arguments = arguments.to_string();
        if let Some(arguments_delta) = delta {
            stream.push(ModelResponseStreamEvent::PartDelta(crate::PartDelta {
                index: call.index,
                delta: crate::StreamDelta::ToolCallArguments { arguments_delta },
            }));
        }
    }

    fn response_with_streamed_parts_fallback(&self, mut response: ModelResponse) -> ModelResponse {
        let has_text = !response.text_output().is_empty();
        let has_thinking = response
            .parts
            .iter()
            .any(|part| matches!(part, ModelResponsePart::Thinking { .. }));
        let existing_tool_keys = response
            .tool_calls()
            .into_iter()
            .map(|call| tool_call_key(&call.id, &call.name))
            .collect::<std::collections::BTreeSet<_>>();
        let mut prefix = Vec::new();
        if !has_thinking && !self.reasoning.is_empty() {
            prefix.push(ModelResponsePart::Thinking {
                text: self.reasoning.clone(),
                signature: response
                    .provider
                    .as_ref()
                    .and_then(|provider| provider.response_id.clone()),
            });
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
        for call in self.streamed_tool_calls() {
            if !existing_tool_keys.contains(&tool_call_key(&call.id, &call.name)) {
                response.parts.push(ModelResponsePart::ToolCall(call));
            }
        }
        response
    }

    fn response_from_streamed_parts(&self) -> ModelResponse {
        self.response_with_streamed_parts_fallback(ModelResponse {
            parts: Vec::new(),
            usage: Usage::default(),
            model_name: None,
            provider: Some(ProviderInfo {
                name: "openai".to_string(),
                response_id: None,
            }),
            finish_reason: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        })
    }

    fn streamed_tool_calls(&self) -> Vec<ToolCallPart> {
        let mut calls = self.function_calls.values().collect::<Vec<_>>();
        calls.sort_by_key(|call| call.index);
        calls
            .into_iter()
            .filter(|call| !call.name.is_empty())
            .map(|call| ToolCallPart {
                id: if call.call_id.is_empty() {
                    call.item_id.clone()
                } else {
                    call.call_id.clone()
                },
                name: call.name.clone(),
                arguments: parse_tool_call_arguments(&Value::String(call.arguments.clone())),
            })
            .collect()
    }

    /// Finish parsing buffered text.
    ///
    /// # Errors
    ///
    /// Returns an error when no text or completed response was received.
    pub fn finish(&mut self) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        if self.final_seen {
            return Ok(Vec::new());
        }
        if self.text.is_empty() && self.reasoning.is_empty() && self.function_calls.is_empty() {
            return Err(ModelError::ResponseParsing(
                "missing response.completed event".to_string(),
            ));
        }
        let mut stream = Vec::new();
        self.end_open_parts(&mut stream);
        stream.push(ModelResponseStreamEvent::FinalResult(Box::new(
            self.response_from_streamed_parts(),
        )));
        self.final_seen = true;
        Ok(stream)
    }
}

fn function_call_item_key(event: &Value, item: &Value) -> String {
    event
        .get("item_id")
        .or_else(|| item.get("id"))
        .or_else(|| item.get("call_id"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            event
                .get("output_index")
                .and_then(Value::as_u64)
                .map(|index| format!("output-{index}"))
        })
        .unwrap_or_else(|| "function-call".to_string())
}

fn tool_call_key(id: &str, name: &str) -> String {
    if id.is_empty() {
        format!("name:{name}")
    } else {
        format!("id:{id}")
    }
}

fn parse_response_item(item: &Value, parts: &mut Vec<ModelResponsePart>) {
    match item.get("type").and_then(Value::as_str) {
        Some("message") => push_message_content_parts(item, parts),
        Some("refusal") => push_refusal_part(item, parts),
        Some("function_call") => push_function_call_part(item, parts),
        Some("reasoning") => push_reasoning_part(item, parts),
        Some("web_search_call" | "mcp_call" | "mcp_approval_request") => {
            push_native_tool_call(item, parts);
        }
        Some("image_generation_call" | "file_search_call") => {
            push_native_tool_call(item, parts);
            push_result_file_part(item, parts);
        }
        _ => {}
    }
}

fn push_message_content_parts(item: &Value, parts: &mut Vec<ModelResponsePart>) {
    for content in item
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if matches!(
            content.get("type").and_then(Value::as_str),
            Some("output_text")
        ) {
            if let Some(text) = content.get("text").and_then(Value::as_str) {
                parts.push(ModelResponsePart::Text {
                    text: text.to_string(),
                });
            }
        }
    }
}

fn push_refusal_part(item: &Value, parts: &mut Vec<ModelResponsePart>) {
    if let Some(text) = item
        .get("refusal")
        .or_else(|| item.get("content"))
        .and_then(Value::as_str)
    {
        parts.push(ModelResponsePart::Text {
            text: text.to_string(),
        });
    }
}

fn push_function_call_part(item: &Value, parts: &mut Vec<ModelResponsePart>) {
    parts.push(ModelResponsePart::ToolCall(ToolCallPart {
        id: item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        name: item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        arguments: parse_tool_call_arguments(item.get("arguments").unwrap_or(&Value::Null)),
    }));
}

fn push_reasoning_part(item: &Value, parts: &mut Vec<ModelResponsePart>) {
    let text = item
        .get("summary")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|summary| summary.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    if !text.is_empty() {
        parts.push(ModelResponsePart::Thinking {
            text,
            signature: item.get("id").and_then(Value::as_str).map(str::to_string),
        });
    }
}

fn push_native_tool_call(item: &Value, parts: &mut Vec<ModelResponsePart>) {
    parts.push(ModelResponsePart::NativeToolCall {
        tool_type: item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        payload: item.clone(),
    });
}

fn push_result_file_part(item: &Value, parts: &mut Vec<ModelResponsePart>) {
    if let Some(url) = item.get("result").and_then(Value::as_str) {
        parts.push(ModelResponsePart::File {
            url: url.to_string(),
            media_type: item
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream")
                .to_string(),
        });
    }
}

fn response_tool_defs(
    tools: &[ToolDefinition],
    native_tools: &[NativeToolDefinition],
) -> Vec<Value> {
    let mut definitions = tools
        .iter()
        .map(|tool| {
            let mut definition = serde_json::Map::new();
            definition.insert("type".to_string(), json!("function"));
            definition.insert("name".to_string(), json!(tool.name));
            insert_optional_description(&mut definition, tool.description.as_ref());
            definition.insert(
                "parameters".to_string(),
                provider_tool_parameters(&tool.parameters),
            );
            Value::Object(definition)
        })
        .collect::<Vec<_>>();
    definitions.extend(native_tools.iter().map(native_response_tool_def));
    definitions
}

fn native_response_tool_def(tool: &NativeToolDefinition) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("type".to_string(), json!(tool.tool_type));
    for (key, value) in &tool.config {
        object.insert(key.clone(), value.clone());
    }
    Value::Object(object)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{ModelResponsePart, ModelResponseStreamEvent, StreamDelta};

    fn final_response(events: &[ModelResponseStreamEvent]) -> &ModelResponse {
        events
            .iter()
            .find_map(|event| match event {
                ModelResponseStreamEvent::FinalResult(response) => Some(response.as_ref()),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn responses_stream_function_call_deltas_become_final_tool_call() {
        let events = vec![
            json!({
                "type": "response.output_item.added",
                "output_index": 0,
                "item": {
                    "id": "fc_1",
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "shell_exec",
                    "arguments": ""
                }
            }),
            json!({
                "type": "response.function_call_arguments.delta",
                "item_id": "fc_1",
                "delta": "{\"command\":\"ls"
            }),
            json!({
                "type": "response.function_call_arguments.delta",
                "item_id": "fc_1",
                "delta": "\"}"
            }),
            json!({
                "type": "response.output_item.done",
                "output_index": 0,
                "item": {
                    "id": "fc_1",
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "shell_exec",
                    "arguments": "{\"command\":\"ls\"}"
                }
            }),
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_1",
                    "status": "completed",
                    "output": []
                }
            }),
        ];

        let stream = OpenAiResponsesAdapter::parse_stream_events(&events).unwrap();
        assert!(stream.iter().any(|event| matches!(
            event,
            ModelResponseStreamEvent::PartStart(part)
                if part.part_kind == "tool_call" && part.index == 2
        )));
        assert!(stream.iter().any(|event| matches!(
            event,
            ModelResponseStreamEvent::PartDelta(delta)
                if matches!(&delta.delta, StreamDelta::ToolCallName { name } if name == "shell_exec")
        )));
        assert!(stream.iter().any(|event| matches!(
            event,
            ModelResponseStreamEvent::PartDelta(delta)
                if matches!(&delta.delta, StreamDelta::ToolCallArguments { arguments_delta } if arguments_delta.contains("command"))
        )));

        let response = final_response(&stream);
        let tool_calls = response.tool_calls();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_1");
        assert_eq!(tool_calls[0].name, "shell_exec");
        assert_eq!(tool_calls[0].arguments.execution_value()["command"], "ls");
    }

    #[test]
    fn responses_stream_preserves_thinking_and_text_when_completed_output_is_empty() {
        let events = vec![
            json!({"type": "response.reasoning_summary_text.delta", "delta": "inspect"}),
            json!({"type": "response.output_text.delta", "delta": "done"}),
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_text",
                    "status": "completed",
                    "output": []
                }
            }),
        ];

        let stream = OpenAiResponsesAdapter::parse_stream_events(&events).unwrap();
        assert!(stream.iter().any(|event| matches!(
            event,
            ModelResponseStreamEvent::PartDelta(delta)
                if matches!(&delta.delta, StreamDelta::Thinking { text } if text == "inspect")
        )));
        assert!(stream.iter().any(|event| matches!(
            event,
            ModelResponseStreamEvent::PartDelta(delta)
                if matches!(&delta.delta, StreamDelta::Text { text } if text == "done")
        )));
        let response = final_response(&stream);
        assert_eq!(response.text_output(), "done");
        assert!(response.parts.iter().any(
            |part| matches!(part, ModelResponsePart::Thinking { text, .. } if text == "inspect")
        ));
    }
}
