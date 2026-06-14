//! `OpenAI` Responses incremental stream parsing.

mod response_parts;
mod state;

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::{message::Metadata, ModelError, ModelResponseStreamEvent};

use state::StreamedFunctionCall;

use super::response::{parse_response, raw_reasoning_content, reasoning_summary_text};

pub(super) fn parse_stream_events(
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

/// Incremental parser for `OpenAI` Responses server-sent JSON payloads.
#[derive(Default)]
pub struct OpenAiResponsesStreamParser {
    text_started: bool,
    text: String,
    reasoning_started: bool,
    reasoning: String,
    reasoning_item_id: Option<String>,
    reasoning_signature: Option<String>,
    reasoning_details: Metadata,
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
                    .map(parse_response)
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
        if let Some(item_id) = event.get("item_id").and_then(Value::as_str) {
            self.reasoning_item_id = Some(item_id.to_string());
        }
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
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                let key = function_call_item_key(event, item);
                self.ensure_function_call_started(&key, item, stream);
                self.update_function_call_from_item(&key, item, stream, false);
            }
            Some("reasoning") => self.update_reasoning_from_item(item),
            _ => {}
        }
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
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
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
            Some("reasoning") => self.update_reasoning_from_item(item),
            _ => {}
        }
    }

    fn update_reasoning_from_item(&mut self, item: &Value) {
        if let Some(id) = item.get("id").and_then(Value::as_str) {
            self.reasoning_item_id = Some(id.to_string());
        }
        if let Some(encrypted_content) = item.get("encrypted_content").and_then(Value::as_str) {
            self.reasoning_signature = Some(encrypted_content.to_string());
            self.reasoning_details
                .insert("encrypted_content".to_string(), json!(encrypted_content));
        }
        if let Some(raw_content) = raw_reasoning_content(item) {
            self.reasoning_details
                .insert("raw_content".to_string(), json!(raw_content));
        }
        if self.reasoning.is_empty() {
            let summary = reasoning_summary_text(item);
            if !summary.is_empty() {
                self.reasoning = summary;
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
                    namespace: item
                        .get("namespace")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    status: item
                        .get("status")
                        .and_then(Value::as_str)
                        .map(str::to_string),
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
        if let Some(namespace) = item.get("namespace").and_then(Value::as_str) {
            call.namespace = Some(namespace.to_string());
        }
        if let Some(status) = item.get("status").and_then(Value::as_str) {
            call.status = Some(status.to_string());
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

    /// Finish parsing buffered text.
    ///
    /// # Errors
    ///
    /// Returns an error when the provider stream ended without `response.completed`.
    pub fn finish(&mut self) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        if self.final_seen {
            Ok(Vec::new())
        } else {
            Err(ModelError::ResponseParsing(
                "missing response.completed event".to_string(),
            ))
        }
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
