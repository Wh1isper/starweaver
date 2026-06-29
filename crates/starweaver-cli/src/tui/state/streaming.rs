use std::fmt::Write as _;

use super::{
    append_delta_segments, assistant_content_line, compact_status_text,
    format_custom_context_event_lines, format_streaming_tool_call_line,
    format_subagent_finished_line, format_subagent_running_line, format_tool_call_line,
    format_tool_return_lines, is_assistant_content_line, is_subagent_lifecycle_event_kind,
    is_subagent_start_event_kind, is_task_snapshot_event, is_task_tool_name,
    is_thinking_quote_line, merge_stream_fragment, normalized_event_kind, streaming_part_kind,
    streaming_tool_arguments_match, streaming_tool_state_is_available, subagent_display_id,
    task_panel_items_from_value, tool_call_visibility_key, AgentStreamEvent, AgentStreamRecord,
    HitlPanelState, InteractiveTuiState, ModelResponseStreamEvent, PartDelta, StreamDelta,
    StreamingPartKind, StreamingToolCallState, Value,
};

impl InteractiveTuiState {
    fn apply_subagent_lifecycle_event(&mut self, kind: &str, payload: &Value) {
        let normalized = normalized_event_kind(kind);
        let agent_id = subagent_display_id(payload);
        if is_subagent_start_event_kind(&normalized) {
            if let Some(state) = self.subagent_states.get_mut(&agent_id) {
                if state.status == "running" {
                    state.agent_name = subagent_agent_name(payload);
                    self.update_subagent_collapsed_line(&agent_id);
                }
                return;
            }
            let line_index = self.body.len();
            self.body.push(format_subagent_running_line(payload));
            self.subagent_states.insert(
                agent_id,
                super::SubagentDisplayState {
                    line_index,
                    agent_name: subagent_agent_name(payload),
                    status: "running".to_string(),
                    tool_names: Vec::new(),
                    output_preview: String::new(),
                    request_count: 0,
                },
            );
            return;
        }

        let line = format_subagent_finished_line(kind, payload);
        if let Some(mut state) = self.subagent_states.remove(&agent_id) {
            state.status = if normalized.contains("fail") {
                "failed".to_string()
            } else {
                "done".to_string()
            };
            if let Some(preview) = subagent_result_preview(payload) {
                state.output_preview = preview;
            }
            if let Some(request_count) = subagent_request_count(payload) {
                state.request_count = request_count;
            }
            if let Some(slot) = self.body.get_mut(state.line_index) {
                *slot = line;
                return;
            }
        }
        self.body.push(line);
    }

    fn update_subagent_collapsed_line(&mut self, agent_id: &str) {
        if let Some(state) = self.subagent_states.get(agent_id) {
            if let Some(slot) = self.body.get_mut(state.line_index) {
                *slot = format_subagent_collapsed_line(state);
            }
        }
    }

    fn apply_subagent_source_record(&mut self, record: &AgentStreamRecord) -> bool {
        let Some(source) = record.source.as_ref() else {
            return false;
        };
        if !matches!(
            source.kind,
            starweaver_runtime::AgentStreamSourceKind::Subagent
        ) {
            return false;
        }
        let agent_id = source.agent_id.as_str().to_string();
        if !self.subagent_states.contains_key(&agent_id) {
            let line_index = self.body.len();
            self.body
                .push(format!("[{}] Running...", source.agent_name));
            self.subagent_states.insert(
                agent_id.clone(),
                super::SubagentDisplayState {
                    line_index,
                    agent_name: source.agent_name.clone(),
                    status: "running".to_string(),
                    tool_names: Vec::new(),
                    output_preview: String::new(),
                    request_count: 0,
                },
            );
        }
        match &record.event {
            AgentStreamEvent::ModelRequest { .. } => {
                if let Some(state) = self.subagent_states.get_mut(&agent_id) {
                    state.request_count = state.request_count.saturating_add(1);
                    state.status = "running".to_string();
                }
            }
            AgentStreamEvent::ToolCall { call, .. } => {
                if let Some(state) = self.subagent_states.get_mut(&agent_id) {
                    if !state.tool_names.iter().any(|name| name == &call.name) {
                        state.tool_names.push(call.name.clone());
                    }
                }
            }
            AgentStreamEvent::ModelStream {
                event:
                    ModelResponseStreamEvent::PartDelta(PartDelta {
                        delta: StreamDelta::Text { text },
                        ..
                    }),
                ..
            } => {
                if let Some(state) = self.subagent_states.get_mut(&agent_id) {
                    state.output_preview =
                        compact_status_text(&format!("{}{}", state.output_preview, text), 120);
                }
            }
            AgentStreamEvent::RunComplete { output, .. } => {
                if let Some(state) = self.subagent_states.get_mut(&agent_id) {
                    state.status = "done".to_string();
                    if !output.trim().is_empty() {
                        state.output_preview = compact_status_text(output, 120);
                    }
                }
            }
            AgentStreamEvent::RunFailed { message, .. } => {
                if let Some(state) = self.subagent_states.get_mut(&agent_id) {
                    state.status = "failed".to_string();
                    state.output_preview = compact_status_text(message, 120);
                }
            }
            _ => {}
        }
        self.update_subagent_collapsed_line(&agent_id);
        true
    }

    /// Apply a live runtime stream event to the view state.
    pub fn apply_stream_record(&mut self, record: &AgentStreamRecord) {
        let should_auto_scroll = !self.selection_mode;
        if self.apply_subagent_source_record(record) {
            if should_auto_scroll {
                self.scroll_to_bottom();
            }
            return;
        }
        match &record.event {
            AgentStreamEvent::RunStart { run_id, .. } => {
                self.current_run_id = Some(run_id.as_str().to_string());
                self.current_run_usage = None;
                self.status = "RUNNING".to_string();
                self.phase = "started".to_string();
            }
            AgentStreamEvent::NodeStart { node, .. } => {
                self.phase = format!("node:{node:?}").to_ascii_lowercase();
            }
            AgentStreamEvent::ModelRequest { .. } => {
                self.phase = "thinking".to_string();
                self.streaming_parts.clear();
                self.streaming_tool_calls.clear();
                self.tool_call_arguments.clear();
                self.streaming_text_seen = false;
                self.streaming_reasoning_seen = false;
            }
            AgentStreamEvent::ModelStream { event, .. } => self.apply_model_stream_event(event),
            AgentStreamEvent::ModelResponse { response, .. } => {
                self.phase = "response".to_string();
                self.apply_model_response_parts(&response.parts);
            }
            AgentStreamEvent::ToolCall { call, .. } => {
                self.push_tool_call(call);
            }
            AgentStreamEvent::ToolReturn { tool_return, .. } => {
                self.phase = "tools".to_string();
                let arguments = self.tool_call_arguments.remove(&tool_return.tool_call_id);
                self.update_hitl_panel(tool_return);
                self.update_task_panel_from_tool_return(tool_return);
                self.body
                    .extend(format_tool_return_lines(tool_return, arguments.as_ref()));
            }
            AgentStreamEvent::OutputRetry { retries, .. } => {
                self.phase = "retry".to_string();
                self.body.push(format!("Output retry: {retries}"));
            }
            AgentStreamEvent::SteeringGuard { .. } => {
                self.phase = "steering".to_string();
                self.body
                    .push("Steering update pending; continuing run.".to_string());
            }
            AgentStreamEvent::Suspended { reason, .. } => {
                self.status = "WAITING".to_string();
                self.phase = "suspended".to_string();
                self.body.push(format!("Suspended: {reason}"));
            }
            AgentStreamEvent::Checkpoint { node, .. } => {
                self.phase = format!("checkpoint:{node:?}").to_ascii_lowercase();
            }
            AgentStreamEvent::Custom { event } => {
                if is_model_transport_event(&event.kind) {
                    self.apply_model_transport_event(&event.kind, &event.payload);
                } else {
                    self.phase.clone_from(&event.kind);
                    self.apply_custom_stream_event(&event.kind, &event.payload, record.sequence);
                }
            }
            AgentStreamEvent::RunComplete { output, .. } => {
                self.phase = "completed".to_string();
                if !self.visible_text_seen && !output.trim().is_empty() {
                    self.push_text_lines(output);
                    self.visible_text_seen = true;
                }
            }
            AgentStreamEvent::RunFailed { message, .. } => {
                self.status = "FAILED".to_string();
                self.phase = "failed".to_string();
                self.body.push(format!("Run failed: {message}"));
            }
            AgentStreamEvent::NodeComplete { .. } => {}
        }
        if should_auto_scroll {
            self.scroll_to_bottom();
        }
    }

    fn apply_model_transport_event(&mut self, kind: &str, payload: &Value) {
        let normalized = normalized_event_kind(kind);
        if normalized.ends_with("model_transport_fallback") {
            let reason = payload
                .get("reason")
                .and_then(Value::as_str)
                .filter(|reason| !reason.trim().is_empty());
            self.model_transport_status = Some(reason.map_or_else(
                || "Transport: websocket -> http".to_string(),
                |reason| format!("Transport: websocket -> http ({reason})"),
            ));
        } else if normalized.ends_with("model_transport_selected") {
            if let Some(transport) = payload.get("transport").and_then(Value::as_str) {
                self.model_transport_status = Some(format!("Transport: {transport}"));
            }
        }
    }

    fn apply_custom_stream_event(&mut self, kind: &str, payload: &Value, sequence: usize) {
        if kind == "usage_snapshot" {
            self.apply_usage_snapshot_payload(payload, sequence);
        } else if is_subagent_lifecycle_event_kind(kind) {
            self.apply_subagent_lifecycle_event(kind, payload);
        } else if is_task_snapshot_event(kind) {
            self.apply_task_snapshot_payload(payload);
        } else if is_goal_event_kind(kind) {
            let goal_completed = is_goal_complete_event_kind(kind);
            self.apply_goal_event_payload(kind, payload);
            if let Some(lines) = format_custom_context_event_lines(kind, payload) {
                self.body.extend(lines);
            }
            if goal_completed {
                self.push_goal_total_tokens_report();
            }
        } else if let Some(lines) = format_custom_context_event_lines(kind, payload) {
            self.body.extend(lines);
        } else if kind == "steering_received" {
            let text = payload.get("text").and_then(serde_json::Value::as_str);
            if let Some(text) = text.filter(|text| !text.trim().is_empty()) {
                self.body.push(format!("Steering received: {text}"));
            } else {
                self.body.push("Steering received".to_string());
            }
        }
    }

    fn apply_model_stream_event(&mut self, event: &ModelResponseStreamEvent) {
        match event {
            ModelResponseStreamEvent::PartStart(part) => {
                let kind = streaming_part_kind(&part.part_kind);
                self.streaming_parts.insert(part.index, kind);
                self.phase = match kind {
                    StreamingPartKind::Text => {
                        self.ensure_text_stream_line();
                        "streaming".to_string()
                    }
                    StreamingPartKind::Thinking => "thinking".to_string(),
                    StreamingPartKind::ToolCall => {
                        self.begin_streaming_tool_call_line(part.index);
                        "tools".to_string()
                    }
                    StreamingPartKind::Other => format!("streaming:{}", part.part_kind),
                };
            }
            ModelResponseStreamEvent::PartDelta(delta) => {
                match self.streaming_kind_for_delta(delta) {
                    StreamingPartKind::Text => {
                        self.phase = "streaming".to_string();
                        self.append_stream_delta(&delta.as_text());
                        self.streaming_text_seen = true;
                        self.visible_text_seen = true;
                    }
                    StreamingPartKind::Thinking => {
                        self.phase = "thinking".to_string();
                        self.append_thinking_delta(&delta.as_text());
                        self.streaming_reasoning_seen = true;
                    }
                    StreamingPartKind::ToolCall => {
                        self.phase = "tools".to_string();
                        self.append_tool_call_delta(delta);
                    }
                    StreamingPartKind::Other => {
                        self.phase = "streaming".to_string();
                    }
                }
            }
            ModelResponseStreamEvent::PartEnd(part) => {
                self.streaming_parts.remove(&part.index);
            }
            ModelResponseStreamEvent::Diagnostic(_) => {}
            ModelResponseStreamEvent::FinalResult(response) => {
                self.phase = "finalizing".to_string();
                if !self.streaming_text_seen {
                    let text = response.text_output();
                    if !text.trim().is_empty() {
                        self.push_text_lines(&text);
                        self.streaming_text_seen = true;
                        self.visible_text_seen = true;
                    }
                }
                self.apply_model_response_parts(&response.parts);
            }
        }
    }

    fn append_stream_delta(&mut self, delta: &str) {
        self.ensure_text_stream_line();
        append_delta_segments(&mut self.body, delta, |line| assistant_content_line(line));
    }

    fn append_thinking_delta(&mut self, delta: &str) {
        self.ensure_thinking_blockquote();
        append_delta_segments(&mut self.body, delta, |line| {
            assistant_content_line(format!("> {line}"))
        });
    }

    fn ensure_thinking_blockquote(&mut self) {
        if !self
            .body
            .last()
            .is_some_and(|line| is_thinking_quote_line(line))
        {
            self.body.push(assistant_content_line("> "));
        }
    }

    fn push_thinking_lines(&mut self, text: &str) {
        let mut lines = text.lines().peekable();
        if lines.peek().is_none() {
            self.ensure_thinking_blockquote();
            return;
        }
        for line in lines {
            self.body.push(assistant_content_line(format!("> {line}")));
        }
    }

    fn streaming_kind_for_delta(&self, delta: &PartDelta) -> StreamingPartKind {
        match &delta.delta {
            StreamDelta::Text { .. } => StreamingPartKind::Text,
            StreamDelta::Thinking { .. } => StreamingPartKind::Thinking,
            StreamDelta::ToolCallName { .. } | StreamDelta::ToolCallArguments { .. } => {
                StreamingPartKind::ToolCall
            }
            StreamDelta::NativePayload { .. } | StreamDelta::FileMetadata { .. } => self
                .streaming_parts
                .get(&delta.index)
                .copied()
                .unwrap_or(StreamingPartKind::Other),
        }
    }

    fn push_text_lines(&mut self, text: &str) {
        self.ensure_text_stream_line();
        let mut lines = text.lines().peekable();
        if lines.peek().is_none() {
            return;
        }
        for line in lines {
            self.body.push(assistant_content_line(line));
        }
    }

    fn ensure_text_stream_line(&mut self) {
        if self.body.is_empty()
            || self.body.last().is_some_and(|line| {
                !is_assistant_content_line(line) || is_thinking_quote_line(line)
            })
        {
            self.body.push(assistant_content_line(""));
        }
    }

    fn apply_model_response_parts(&mut self, parts: &[starweaver_model::ModelResponsePart]) {
        for part in parts {
            match part {
                starweaver_model::ModelResponsePart::Text { text }
                | starweaver_model::ModelResponsePart::ProviderText { text, .. }
                    if !self.streaming_text_seen =>
                {
                    self.push_text_lines(text);
                    self.streaming_text_seen = true;
                    self.visible_text_seen = true;
                }
                starweaver_model::ModelResponsePart::Thinking { text, .. }
                | starweaver_model::ModelResponsePart::ProviderThinking { text, .. }
                    if !self.streaming_reasoning_seen =>
                {
                    self.push_thinking_lines(text);
                    self.streaming_reasoning_seen = true;
                }
                starweaver_model::ModelResponsePart::ToolCall(call)
                | starweaver_model::ModelResponsePart::ProviderToolCall { call, .. } => {
                    self.push_tool_call(call);
                }
                _ => {}
            }
        }
    }

    fn append_tool_call_delta(&mut self, delta: &PartDelta) {
        match &delta.delta {
            StreamDelta::ToolCallName { name } => {
                let state = self.streaming_tool_calls.entry(delta.index).or_default();
                state.name = Some(merge_stream_fragment(state.name.as_deref(), name));
            }
            StreamDelta::ToolCallArguments { arguments_delta } => {
                let state = self.streaming_tool_calls.entry(delta.index).or_default();
                state.arguments.push_str(arguments_delta);
            }
            _ => {}
        }
        self.update_streaming_tool_call_line(delta.index);
    }

    fn begin_streaming_tool_call_line(&mut self, index: usize) {
        if self
            .streaming_tool_calls
            .get(&index)
            .is_some_and(|state| state.line_index.is_some() && state.linked_call_key.is_none())
        {
            return;
        }
        let line_index = self.body.len();
        self.streaming_tool_calls.insert(
            index,
            StreamingToolCallState {
                line_index: Some(line_index),
                ..StreamingToolCallState::default()
            },
        );
        self.body.push(format_streaming_tool_call_line(
            self.streaming_tool_calls.get(&index),
        ));
    }

    fn ensure_streaming_tool_call_line(&mut self, index: usize) {
        if self
            .streaming_tool_calls
            .get(&index)
            .and_then(|state| state.line_index)
            .is_some()
        {
            return;
        }
        let line_index = self.body.len();
        self.body.push(format_streaming_tool_call_line(
            self.streaming_tool_calls.get(&index),
        ));
        self.streaming_tool_calls
            .entry(index)
            .or_default()
            .line_index = Some(line_index);
    }

    fn update_streaming_tool_call_line(&mut self, index: usize) {
        self.ensure_streaming_tool_call_line(index);
        let line = format_streaming_tool_call_line(self.streaming_tool_calls.get(&index));
        if let Some(line_index) = self
            .streaming_tool_calls
            .get(&index)
            .and_then(|state| state.line_index)
        {
            if let Some(existing) = self.body.get_mut(line_index) {
                *existing = line;
            }
        }
    }

    fn push_tool_call(&mut self, call: &starweaver_model::ToolCallPart) {
        self.phase = "tools".to_string();
        let key = tool_call_visibility_key(call);
        if !call.id.is_empty() {
            self.tool_call_arguments
                .insert(call.id.clone(), call.arguments.replay_value());
        }
        if !self.visible_tool_calls.insert(key.clone()) {
            return;
        }
        let line = format_tool_call_line(call);
        if let Some(line_index) = self.matching_streamed_tool_line(call, &key) {
            if let Some(existing) = self.body.get_mut(line_index) {
                *existing = line;
                return;
            }
        }
        self.body.push(line);
    }

    fn matching_streamed_tool_line(
        &mut self,
        call: &starweaver_model::ToolCallPart,
        key: &str,
    ) -> Option<usize> {
        let linked_index = self
            .streaming_tool_calls
            .iter()
            .filter(|(_, state)| state.linked_call_key.as_deref() == Some(key))
            .map(|(index, _)| *index)
            .min();
        let matching_arguments_index = linked_index.or_else(|| {
            self.streaming_tool_calls
                .iter()
                .filter(|(_, state)| streaming_tool_state_is_available(state, key))
                .filter(|(_, state)| state.name.as_deref() == Some(call.name.as_str()))
                .filter(|(_, state)| streaming_tool_arguments_match(state.arguments.trim(), call))
                .map(|(index, _)| *index)
                .min()
        });
        let fallback_index = matching_arguments_index.or_else(|| {
            self.streaming_tool_calls
                .iter()
                .filter(|(_, state)| streaming_tool_state_is_available(state, key))
                .filter(|(_, state)| state.name.as_deref() == Some(call.name.as_str()))
                .map(|(index, _)| *index)
                .min()
        })?;
        let state = self.streaming_tool_calls.get_mut(&fallback_index)?;
        state.linked_call_key = Some(key.to_string());
        state.line_index
    }

    fn update_hitl_panel(&mut self, tool_return: &starweaver_model::ToolReturnPart) {
        if tool_return
            .metadata
            .get("control_flow")
            .and_then(Value::as_str)
            != Some("approval_required")
        {
            return;
        }
        let approval = tool_return.metadata.get("approval");
        self.status = "WAITING".to_string();
        self.phase = "hitl approval".to_string();
        self.pending_hitl = Some(HitlPanelState {
            tool_call_id: tool_return.tool_call_id.clone(),
            tool_name: tool_return.name.clone(),
            command: approval
                .and_then(|value| value.get("command"))
                .and_then(Value::as_str)
                .map(str::to_string),
            risk_level: approval
                .and_then(|value| value.get("risk_level"))
                .and_then(Value::as_str)
                .map(str::to_string),
            reason: approval
                .and_then(|value| value.get("reason"))
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }

    fn update_task_panel_from_tool_return(
        &mut self,
        tool_return: &starweaver_model::ToolReturnPart,
    ) {
        if !is_task_tool_name(&tool_return.name) {
            return;
        }
        if let Some(items) = task_panel_items_from_value(&tool_return.content) {
            self.task_panel_items = items;
        }
    }

    fn apply_task_snapshot_payload(&mut self, payload: &Value) {
        if let Some(items) = task_panel_items_from_value(payload) {
            self.task_panel_items = items;
        }
    }
}

fn format_subagent_collapsed_line(state: &super::SubagentDisplayState) -> String {
    let mut line = format!("[{}] {}", state.agent_name, state.status);
    if state.request_count > 0 {
        let _ = write!(line, " | {} reqs", state.request_count);
    }
    if !state.tool_names.is_empty() {
        let _ = write!(line, " | tools: {}", state.tool_names.join(", "));
    }
    if !state.output_preview.trim().is_empty() {
        let _ = write!(line, " | \"{}\"", state.output_preview.trim());
    }
    line
}

fn subagent_agent_name(payload: &Value) -> String {
    let payload = payload.get("payload").unwrap_or(payload);
    payload
        .get("name")
        .or_else(|| payload.get("agent_name"))
        .or_else(|| payload.get("subagent_name"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("subagent")
        .to_string()
}

fn subagent_request_count(payload: &Value) -> Option<usize> {
    let payload = payload.get("payload").unwrap_or(payload);
    payload
        .get("metadata")
        .and_then(|metadata| metadata.get("request_count"))
        .or_else(|| payload.get("request_count"))
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

fn subagent_result_preview(payload: &Value) -> Option<String> {
    let payload = payload.get("payload").unwrap_or(payload);
    payload
        .get("metadata")
        .and_then(|metadata| metadata.get("result_preview"))
        .or_else(|| payload.get("result_preview"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|value| compact_status_text(value, 120))
}

fn is_model_transport_event(kind: &str) -> bool {
    let normalized = kind.to_ascii_lowercase().replace(['.', '-'], "_");
    matches!(
        normalized.as_str(),
        "model_transport_selected" | "model_transport_fallback"
    ) || normalized.ends_with("_model_transport_selected")
        || normalized.ends_with("_model_transport_fallback")
}

fn is_goal_complete_event_kind(kind: &str) -> bool {
    let normalized = kind.to_ascii_lowercase().replace(['.', '-'], "_");
    matches!(normalized.as_str(), "goal_complete" | "goal_completed")
        || normalized.ends_with("_goal_complete")
        || normalized.ends_with("_goal_completed")
}

fn is_goal_event_kind(kind: &str) -> bool {
    let normalized = kind.to_ascii_lowercase().replace(['.', '-'], "_");
    matches!(
        normalized.as_str(),
        "goal_iteration" | "goal_complete" | "goal_completed"
    ) || normalized.ends_with("_goal_iteration")
        || normalized.ends_with("_goal_complete")
        || normalized.ends_with("_goal_completed")
}
