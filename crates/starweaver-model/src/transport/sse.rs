use serde_json::Value;

use crate::ModelError;

pub(super) async fn send_sse_parser_events(
    sender: &tokio::sync::mpsc::Sender<Result<Value, ModelError>>,
    events: Vec<Result<Value, ModelError>>,
) -> bool {
    for event in events {
        if sender.send(event).await.is_err() {
            return false;
        }
    }
    true
}

#[derive(Debug)]
pub(super) enum StreamSendError {
    Closed,
    InvalidUtf8(std::str::Utf8Error),
}

pub(super) async fn push_sse_utf8_buffer(
    sender: &tokio::sync::mpsc::Sender<Result<Value, ModelError>>,
    parser: &mut SseJsonParser,
    utf8_buffer: &mut Vec<u8>,
) -> Result<(), StreamSendError> {
    match std::str::from_utf8(utf8_buffer) {
        Ok(text) => {
            if !send_sse_parser_events(sender, parser.push_str(text)).await {
                return Err(StreamSendError::Closed);
            }
            utf8_buffer.clear();
            Ok(())
        }
        Err(error) => {
            let valid_up_to = error.valid_up_to();
            if valid_up_to > 0 {
                let text = match std::str::from_utf8(&utf8_buffer[..valid_up_to]) {
                    Ok(text) => text,
                    Err(error) => return Err(StreamSendError::InvalidUtf8(error)),
                };
                if !send_sse_parser_events(sender, parser.push_str(text)).await {
                    return Err(StreamSendError::Closed);
                }
                utf8_buffer.drain(..valid_up_to);
            }
            if error.error_len().is_some() {
                return Err(StreamSendError::InvalidUtf8(error));
            }
            Ok(())
        }
    }
}

#[allow(dead_code)]
fn parse_sse_json_events(text: &str) -> Result<Vec<Value>, ModelError> {
    let mut parser = SseJsonParser::default();
    let mut events = Vec::new();
    for event in parser.push_str(text).into_iter().chain(parser.finish()) {
        events.push(event?);
    }
    Ok(events)
}

#[derive(Default)]
pub(super) struct SseJsonParser {
    buffer: String,
    data_lines: Vec<String>,
}

impl SseJsonParser {
    pub(super) fn push_str(&mut self, text: &str) -> Vec<Result<Value, ModelError>> {
        self.buffer.push_str(text);
        let mut events = Vec::new();
        while let Some(newline) = self.buffer.find('\n') {
            let mut line = self.buffer.drain(..=newline).collect::<String>();
            if line.ends_with('\n') {
                line.pop();
            }
            if line.ends_with('\r') {
                line.pop();
            }
            if let Some(event) = self.push_line(&line) {
                events.push(event);
            }
        }
        events
    }

    pub(super) fn finish(&mut self) -> Vec<Result<Value, ModelError>> {
        let mut events = Vec::new();
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            if let Some(event) = self.push_line(&line) {
                events.push(event);
            }
        }
        if !self.data_lines.is_empty() {
            events.push(parse_sse_json_event(&self.data_lines));
            self.data_lines.clear();
        }
        events
    }

    fn push_line(&mut self, line: &str) -> Option<Result<Value, ModelError>> {
        if let Some(data) = line.strip_prefix("data:") {
            self.data_lines.push(data.trim_start().to_string());
            return None;
        }
        if line.trim().is_empty() && !self.data_lines.is_empty() {
            let event = parse_sse_json_event(&self.data_lines);
            self.data_lines.clear();
            return Some(event);
        }
        None
    }
}

fn parse_sse_json_event(data_lines: &[String]) -> Result<Value, ModelError> {
    let data = data_lines.join("\n");
    if data.trim() == "[DONE]" {
        return Ok(Value::Null);
    }
    serde_json::from_str::<Value>(&data).map_err(|error| {
        ModelError::ResponseParsing(format!("invalid server-sent event JSON: {error}"))
    })
}
