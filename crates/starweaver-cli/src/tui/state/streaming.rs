use crate::tui::timeline::truncate_chars;

use super::formatting::{
    format_streaming_tool_summary, format_tool_call_summary, format_tool_call_summary_from_parts,
    format_tool_return_summary,
};

use super::{
    ActiveModelSegment, ActiveModelSegmentKind, AgentStreamEvent, AgentStreamRecord,
    ContextEventCategory, HitlPanelState, InteractiveTuiState, ModelResponseStreamEvent,
    NoticeLevel, PartDelta, StreamDelta, StreamingPartKind, StreamingToolCallState, SubagentStatus,
    SubagentTimelineItem, SubagentUpdate, ToolActivityStatus, ToolTimelineItem, ToolVisibility,
    Value, approval_request_preview, compact_status_text, format_custom_context_event_lines,
    format_streaming_tool_call_line, format_tool_call_line, format_tool_return_lines,
    is_subagent_lifecycle_event_kind, is_subagent_start_event_kind, is_task_snapshot_event,
    is_task_tool_name, merge_stream_fragment, normalized_event_kind, streaming_part_kind,
    streaming_tool_arguments_match, streaming_tool_state_is_available, subagent_display_id,
    task_panel_items_from_value, tool_call_visibility_key, value_args_preview,
};

impl InteractiveTuiState {
    fn apply_subagent_lifecycle_event(&mut self, kind: &str, payload: &Value) {
        let normalized = normalized_event_kind(kind);
        let agent_id = subagent_display_id(payload);
        if is_subagent_start_event_kind(&normalized) {
            let agent_name = subagent_agent_name(payload);
            if let Some(state) = self.subagent_states.get_mut(&agent_id) {
                state.agent_name.clone_from(&agent_name);
                self.timeline.update_subagent(
                    state.item_id,
                    SubagentUpdate {
                        agent_name: Some(agent_name),
                        status: Some(SubagentStatus::Running),
                        ..SubagentUpdate::default()
                    },
                );
                self.reproject_body();
                return;
            }
            self.finish_current_model_item();
            let item_id = self.timeline.push_subagent(SubagentTimelineItem {
                agent_id: agent_id.clone(),
                agent_name: agent_name.clone(),
                status: SubagentStatus::Running,
                tool_names: Vec::new(),
                output_preview: String::new(),
                output_markdown: String::new(),
                request_count: 0,
                duration_label: None,
            });
            self.subagent_states.insert(
                agent_id,
                super::SubagentDisplayState {
                    item_id,
                    agent_name,
                    status: "running".to_string(),
                    tool_names: Vec::new(),
                    output_preview: String::new(),
                    output_markdown: String::new(),
                    request_count: 0,
                },
            );
            self.reproject_body();
            return;
        }

        let failed = normalized.contains("fail");
        let status = if failed {
            SubagentStatus::Failed
        } else {
            SubagentStatus::Done
        };
        let preview = subagent_result_preview(payload).unwrap_or_default();
        let output_markdown = subagent_result_markdown(payload).unwrap_or_default();
        let request_count = subagent_request_count(payload);
        let duration_label = subagent_duration_label(payload);
        if let Some(state) = self.subagent_states.get_mut(&agent_id) {
            state.status = if failed { "failed" } else { "done" }.to_string();
            if !preview.is_empty() {
                state.output_preview.clone_from(&preview);
            }
            if !output_markdown.is_empty() {
                state.output_markdown.clone_from(&output_markdown);
            }
            if let Some(request_count) = request_count {
                state.request_count = request_count;
            }
            self.timeline.update_subagent(
                state.item_id,
                SubagentUpdate {
                    status: Some(status),
                    output_preview: (!preview.is_empty()).then_some(preview),
                    output_markdown: (!output_markdown.is_empty()).then_some(output_markdown),
                    request_count,
                    duration_label,
                    ..SubagentUpdate::default()
                },
            );
            self.reproject_body();
            return;
        }

        let agent_name = subagent_agent_name(payload);
        self.finish_current_model_item();
        self.timeline.push_subagent(SubagentTimelineItem {
            agent_id,
            agent_name,
            status,
            tool_names: Vec::new(),
            output_preview: preview,
            output_markdown,
            request_count: request_count.unwrap_or(0),
            duration_label,
        });
        self.reproject_body();
    }

    fn update_subagent_collapsed_line(&mut self, agent_id: &str) {
        if let Some(state) = self.subagent_states.get(agent_id) {
            let status = match state.status.as_str() {
                "failed" => SubagentStatus::Failed,
                "done" => SubagentStatus::Done,
                _ => SubagentStatus::Running,
            };
            self.timeline.update_subagent(
                state.item_id,
                SubagentUpdate {
                    agent_name: Some(state.agent_name.clone()),
                    status: Some(status),
                    tool_names: state.tool_names.clone(),
                    output_preview: Some(state.output_preview.clone()),
                    output_markdown: Some(state.output_markdown.clone()),
                    request_count: Some(state.request_count),
                    duration_label: None,
                },
            );
            self.reproject_body();
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
            self.finish_current_model_item();
            let item_id = self.timeline.push_subagent(SubagentTimelineItem {
                agent_id: agent_id.clone(),
                agent_name: source.agent_name.clone(),
                status: SubagentStatus::Running,
                tool_names: Vec::new(),
                output_preview: String::new(),
                output_markdown: String::new(),
                request_count: 0,
                duration_label: None,
            });
            self.subagent_states.insert(
                agent_id.clone(),
                super::SubagentDisplayState {
                    item_id,
                    agent_name: source.agent_name.clone(),
                    status: "running".to_string(),
                    tool_names: Vec::new(),
                    output_preview: String::new(),
                    output_markdown: String::new(),
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
                if let Some(state) = self.subagent_states.get_mut(&agent_id)
                    && !state.tool_names.iter().any(|name| name == &call.name)
                {
                    state.tool_names.push(call.name.clone());
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
                    state.output_markdown.push_str(text);
                    state.output_preview = compact_status_text(&state.output_markdown, 120);
                }
            }
            AgentStreamEvent::RunComplete { output, .. } => {
                if let Some(state) = self.subagent_states.get_mut(&agent_id) {
                    state.status = "done".to_string();
                    if !output.trim().is_empty() {
                        state.output_markdown.clone_from(output);
                        state.output_preview = compact_status_text(output, 120);
                    }
                }
            }
            AgentStreamEvent::RunFailed { message, .. } => {
                if let Some(state) = self.subagent_states.get_mut(&agent_id) {
                    state.status = "failed".to_string();
                    state.output_markdown.clone_from(message);
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
        // Following is sticky: output only keeps the viewport pinned when the
        // user was already at the bottom before this event arrived.
        let should_auto_scroll = self.is_at_bottom();
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
                self.finish_current_model_item();
                self.streaming_parts.clear();
                self.streaming_tool_calls.clear();
                self.tool_call_arguments.clear();
                self.streaming_text_seen = false;
                self.streaming_reasoning_seen = false;
            }
            AgentStreamEvent::ModelStream { event, .. } => self.apply_model_stream_event(event),
            AgentStreamEvent::ModelResponse { response, .. } => {
                self.phase = "response".to_string();
                self.finish_current_model_item();
                self.apply_model_response_parts(&response.parts);
            }
            AgentStreamEvent::ToolCall { call, .. } => self.push_tool_call(call),
            AgentStreamEvent::ToolReturn { tool_return, .. } => self.apply_tool_return(tool_return),
            AgentStreamEvent::OutputRetry { retries, .. } => {
                self.phase = "retry".to_string();
                self.push_system_notice(NoticeLevel::Warning, format!("Output retry: {retries}"));
            }
            AgentStreamEvent::SteeringGuard { .. } => {
                self.phase = "steering".to_string();
                self.push_system_notice(
                    NoticeLevel::Info,
                    "Steering update pending; continuing run.".to_string(),
                );
            }
            AgentStreamEvent::Suspended { reason, .. } => {
                self.status = "WAITING".to_string();
                self.phase = "suspended".to_string();
                self.push_system_notice(NoticeLevel::Warning, format!("Suspended: {reason}"));
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
                self.finish_current_model_item();
                if !self.visible_text_seen && !output.trim().is_empty() {
                    self.push_text_lines(output);
                    self.visible_text_seen = true;
                }
                self.finish_current_model_item();
                self.reproject_body();
            }
            AgentStreamEvent::RunCancelled { reason, .. } => {
                self.status = "CANCELLED".to_string();
                self.phase = "cancelled".to_string();
                self.push_system_notice(NoticeLevel::Warning, format!("Run cancelled: {reason}"));
            }
            AgentStreamEvent::RunFailed { message, .. } => {
                self.status = "FAILED".to_string();
                self.phase = "failed".to_string();
                self.push_system_notice(NoticeLevel::Error, format!("Run failed: {message}"));
            }
            AgentStreamEvent::NodeComplete { .. } => {}
        }
        if should_auto_scroll {
            self.scroll_to_bottom();
        }
    }

    fn apply_tool_return(&mut self, tool_return: &starweaver_model::ToolReturnPart) {
        self.phase = "tools".to_string();
        self.finish_current_model_item();
        let arguments = self.tool_call_arguments.remove(&tool_return.tool_call_id);
        self.update_hitl_panel(tool_return);
        self.update_task_panel_from_tool_return(tool_return);
        let return_lines = format_tool_return_lines(tool_return, arguments.as_ref());
        let visibility = tool_visibility(tool_return);
        let concise = format_tool_return_summary(tool_return, arguments.as_ref(), visibility);
        let status = if tool_return.is_error {
            ToolActivityStatus::Failed
        } else {
            ToolActivityStatus::Completed
        };
        let item_id = self
            .tool_items_by_call_id
            .get(&tool_return.tool_call_id)
            .copied()
            .or_else(|| {
                self.tool_items_by_key
                    .get(&tool_return.tool_call_id)
                    .copied()
            })
            .filter(|item_id| {
                self.timeline.tool_status(*item_id) == Some(ToolActivityStatus::Running)
            })
            .unwrap_or_else(|| {
                self.timeline.push_tool_call(ToolTimelineItem {
                    call_id: tool_return.tool_call_id.clone(),
                    name: tool_return.name.clone(),
                    args_preview: None,
                    call_line: format!("Tool call: {}", tool_return.name),
                    status: ToolActivityStatus::Running,
                    return_lines: Vec::new(),
                    visibility,
                    concise: format_tool_call_summary_from_parts(
                        &tool_return.name,
                        arguments.as_ref(),
                        None,
                    ),
                })
            });
        self.timeline
            .finish_tool_call(item_id, status, return_lines, visibility, concise);
        self.reproject_body();
    }

    fn apply_model_transport_event(&mut self, kind: &str, payload: &Value) {
        let normalized = normalized_event_kind(kind);
        if normalized.ends_with("model_transport_fallback") {
            let reason = payload
                .get("reason")
                .and_then(Value::as_str)
                .filter(|reason| !reason.trim().is_empty());
            let status = reason.map_or_else(
                || "Transport: websocket -> http".to_string(),
                |reason| format!("Transport: websocket -> http ({reason})"),
            );
            self.model_transport_status = Some(status.clone());
            let detail = payload
                .get("detail")
                .and_then(Value::as_str)
                .filter(|detail| !detail.trim().is_empty());
            self.push_system_notice(
                NoticeLevel::Warning,
                detail.map_or_else(|| status.clone(), |detail| format!("{status}: {detail}")),
            );
        } else if normalized.ends_with("model_transport_selected")
            && let Some(transport) = payload.get("transport").and_then(Value::as_str)
        {
            self.model_transport_status = Some(format!("Transport: {transport}"));
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
                self.finish_current_model_item();
                self.timeline
                    .push_context_event(ContextEventCategory::Goal, lines);
                self.reproject_body();
            }
            if goal_completed {
                self.push_goal_total_tokens_report();
            }
        } else if let Some(lines) = format_custom_context_event_lines(kind, payload) {
            self.finish_current_model_item();
            self.timeline
                .push_context_event(context_event_category(kind), lines);
            self.reproject_body();
        } else if kind == "steering_received" {
            let text = payload.get("text").and_then(serde_json::Value::as_str);
            if let Some(text) = text.filter(|text| !text.trim().is_empty()) {
                self.push_system_notice(NoticeLevel::Info, format!("Steering received: {text}"));
            } else {
                self.push_system_notice(NoticeLevel::Info, "Steering received".to_string());
            }
        }
    }

    fn apply_model_stream_event(&mut self, event: &ModelResponseStreamEvent) {
        match event {
            ModelResponseStreamEvent::PartStart(part) => {
                let kind = streaming_part_kind(&part.part_kind);
                self.streaming_parts.insert(part.index, kind);
                self.phase = match kind {
                    StreamingPartKind::Text => "streaming".to_string(),
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
                        self.append_stream_delta(delta.index, &delta.as_text());
                        self.streaming_text_seen = true;
                        self.visible_text_seen = true;
                    }
                    StreamingPartKind::Thinking => {
                        self.phase = "thinking".to_string();
                        self.append_thinking_delta(delta.index, &delta.as_text());
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
                if self
                    .active_model_segment
                    .as_ref()
                    .is_some_and(|segment| segment.part_index == Some(part.index))
                {
                    self.finish_current_model_item();
                    self.reproject_body();
                }
            }
            ModelResponseStreamEvent::Diagnostic(_) => {}
            ModelResponseStreamEvent::FinalResult(response) => {
                self.phase = "finalizing".to_string();
                self.finish_current_model_item();
                self.apply_model_response_parts(&response.parts);
            }
        }
    }

    fn append_stream_delta(&mut self, part_index: usize, delta: &str) {
        let item_id =
            self.ensure_active_model_segment(ActiveModelSegmentKind::Text, Some(part_index), true);
        self.timeline.append_text(item_id, delta);
        self.reproject_body();
    }

    fn append_thinking_delta(&mut self, part_index: usize, delta: &str) {
        let item_id = self.ensure_active_model_segment(
            ActiveModelSegmentKind::Thinking,
            Some(part_index),
            true,
        );
        self.timeline.append_text(item_id, delta);
        self.reproject_body();
    }

    fn push_thinking_lines(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let item_id =
            self.ensure_active_model_segment(ActiveModelSegmentKind::Thinking, None, false);
        self.timeline.append_text(item_id, text);
        self.reproject_body();
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
        if text.is_empty() {
            return;
        }
        let item_id = self.ensure_active_model_segment(ActiveModelSegmentKind::Text, None, false);
        self.timeline.append_text(item_id, text);
        self.reproject_body();
    }

    fn ensure_active_model_segment(
        &mut self,
        kind: ActiveModelSegmentKind,
        part_index: Option<usize>,
        streaming: bool,
    ) -> super::TuiItemId {
        if let Some(segment) = self.active_model_segment.as_ref()
            && segment.kind == kind
            && segment.part_index == part_index
        {
            debug_assert!(
                self.timeline.is_tail_item(segment.item_id),
                "active model segment must be the timeline tail"
            );
            if self.timeline.is_tail_item(segment.item_id) {
                return segment.item_id;
            }
        }
        self.finish_current_model_item();
        let item_id = match kind {
            ActiveModelSegmentKind::Text => self.timeline.push_assistant_text("", streaming),
            ActiveModelSegmentKind::Thinking => self.timeline.push_thinking("", streaming),
        };
        self.active_model_segment = Some(ActiveModelSegment {
            item_id,
            kind,
            part_index,
        });
        self.reproject_body();
        item_id
    }

    fn apply_model_response_parts(&mut self, parts: &[starweaver_model::ModelResponsePart]) {
        let skip_text = self.streaming_text_seen;
        let skip_reasoning = self.streaming_reasoning_seen;
        let mut text_seen = false;
        let mut reasoning_seen = false;
        for part in parts {
            match part {
                starweaver_model::ModelResponsePart::Text { text }
                | starweaver_model::ModelResponsePart::ProviderText { text, .. }
                    if !skip_text =>
                {
                    self.push_text_lines(text);
                    text_seen |= !text.is_empty();
                    self.visible_text_seen |= !text.trim().is_empty();
                }
                starweaver_model::ModelResponsePart::Thinking { text, .. }
                | starweaver_model::ModelResponsePart::ProviderThinking { text, .. }
                    if !skip_reasoning =>
                {
                    self.push_thinking_lines(text);
                    reasoning_seen |= !text.is_empty();
                }
                starweaver_model::ModelResponsePart::ToolCall(call)
                | starweaver_model::ModelResponsePart::ProviderToolCall { call, .. } => {
                    self.push_tool_call(call);
                }
                _ => {}
            }
        }
        self.streaming_text_seen |= text_seen;
        self.streaming_reasoning_seen |= reasoning_seen;
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
        self.finish_current_model_item();
        if self
            .streaming_tool_calls
            .get(&index)
            .is_some_and(|state| state.item_id.is_some() && state.linked_call_key.is_none())
        {
            return;
        }
        self.streaming_tool_calls.insert(
            index,
            StreamingToolCallState {
                item_id: None,
                ..StreamingToolCallState::default()
            },
        );
        self.ensure_streaming_tool_call_line(index);
    }

    fn ensure_streaming_tool_call_line(&mut self, index: usize) {
        if self
            .streaming_tool_calls
            .get(&index)
            .and_then(|state| state.item_id)
            .is_some()
        {
            return;
        }
        let line = format_streaming_tool_call_line(self.streaming_tool_calls.get(&index));
        let name = self
            .streaming_tool_calls
            .get(&index)
            .and_then(|state| state.name.clone())
            .unwrap_or_else(|| "tool".to_string());
        let args_preview = self
            .streaming_tool_calls
            .get(&index)
            .and_then(|state| streaming_args_preview(&state.arguments));
        let concise = format_streaming_tool_summary(&name, args_preview.as_deref());
        let item_id = self.timeline.push_tool_call(ToolTimelineItem {
            call_id: format!("streaming:{index}"),
            name,
            args_preview,
            call_line: line,
            status: ToolActivityStatus::Running,
            return_lines: Vec::new(),
            visibility: ToolVisibility::Ordinary,
            concise,
        });
        self.streaming_tool_calls.entry(index).or_default().item_id = Some(item_id);
        self.reproject_body();
    }

    fn update_streaming_tool_call_line(&mut self, index: usize) {
        self.ensure_streaming_tool_call_line(index);
        let line = format_streaming_tool_call_line(self.streaming_tool_calls.get(&index));
        if let Some(item_id) = self
            .streaming_tool_calls
            .get(&index)
            .and_then(|state| state.item_id)
        {
            let state = self.streaming_tool_calls.get(&index);
            self.timeline.update_tool_call(
                item_id,
                state.and_then(|state| state.name.clone()),
                state.and_then(|state| streaming_args_preview(&state.arguments)),
                Some(line),
                None,
                state.map(|state| {
                    let name = state.name.as_deref().unwrap_or("tool");
                    let args_preview = streaming_args_preview(&state.arguments);
                    format_streaming_tool_summary(name, args_preview.as_deref())
                }),
            );
            self.reproject_body();
        }
    }

    fn push_tool_call(&mut self, call: &starweaver_model::ToolCallPart) {
        self.phase = "tools".to_string();
        self.finish_current_model_item();
        let key = tool_call_visibility_key(call);
        let args = call.arguments.replay_value();
        if !call.id.is_empty() {
            self.tool_call_arguments
                .insert(call.id.clone(), args.clone());
        }
        if !self.visible_tool_calls.insert(key.clone()) {
            return;
        }
        let line = format_tool_call_line(call);
        let item_id = if let Some(item_id) = self.matching_streamed_tool_item(call, &key) {
            self.timeline.update_tool_call(
                item_id,
                Some(call.name.clone()),
                value_args_preview(&args, 80),
                Some(line),
                Some(ToolVisibility::Ordinary),
                Some(format_tool_call_summary(call)),
            );
            item_id
        } else {
            self.timeline.push_tool_call(ToolTimelineItem {
                call_id: if call.id.is_empty() {
                    key.clone()
                } else {
                    call.id.clone()
                },
                name: call.name.clone(),
                args_preview: value_args_preview(&args, 80),
                call_line: line,
                status: ToolActivityStatus::Running,
                return_lines: Vec::new(),
                visibility: ToolVisibility::Ordinary,
                concise: format_tool_call_summary(call),
            })
        };
        if !call.id.is_empty() {
            self.tool_items_by_call_id.insert(call.id.clone(), item_id);
        }
        self.tool_items_by_key.insert(key, item_id);
        self.reproject_body();
    }

    pub(super) fn finish_current_model_item(&mut self) {
        if let Some(segment) = self.active_model_segment.take() {
            self.timeline.finish_text_item(segment.item_id);
        }
    }

    fn matching_streamed_tool_item(
        &mut self,
        call: &starweaver_model::ToolCallPart,
        key: &str,
    ) -> Option<super::TuiItemId> {
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
        state.item_id
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
            approval_id: None,
            tool_call_id: tool_return.tool_call_id.clone(),
            tool_name: tool_return.name.clone(),
            request_preview: approval.map(approval_request_preview),
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

fn streaming_args_preview(arguments: &str) -> Option<String> {
    let arguments = arguments.trim();
    if arguments.is_empty() || arguments == "{}" || arguments == "null" {
        None
    } else {
        Some(truncate_chars(arguments, 80))
    }
}

fn tool_visibility(tool_return: &starweaver_model::ToolReturnPart) -> ToolVisibility {
    if tool_return.name == "summarize" {
        return ToolVisibility::ContextHandoff;
    }
    if tool_return
        .metadata
        .get("control_flow")
        .and_then(Value::as_str)
        == Some("approval_required")
    {
        return ToolVisibility::ApprovalRequired;
    }
    if tool_return
        .metadata
        .get("control_flow")
        .and_then(Value::as_str)
        == Some("call_deferred")
        || tool_return.metadata.contains_key("deferred")
    {
        return ToolVisibility::Deferred;
    }
    if is_task_tool_name(&tool_return.name) {
        return ToolVisibility::TaskPanel;
    }
    if tool_return.is_error {
        return ToolVisibility::ErrorImportant;
    }
    ToolVisibility::Ordinary
}

fn context_event_category(kind: &str) -> ContextEventCategory {
    let normalized = normalized_event_kind(kind);
    if normalized.contains("compact") || normalized.contains("compaction") {
        ContextEventCategory::Compaction
    } else if normalized.contains("summary") || normalized.contains("handoff") {
        ContextEventCategory::Summary
    } else if normalized.contains("goal") {
        ContextEventCategory::Goal
    } else {
        ContextEventCategory::Other
    }
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
    subagent_result_markdown(payload).map(|value| compact_status_text(&value, 120))
}

fn subagent_result_markdown(payload: &Value) -> Option<String> {
    let payload = payload.get("payload").unwrap_or(payload);
    payload
        .get("metadata")
        .and_then(|metadata| {
            payload_string(
                metadata,
                &[
                    "result",
                    "result_markdown",
                    "output",
                    "result_preview",
                    "preview",
                    "error",
                    "message",
                    "reason",
                ],
            )
        })
        .or_else(|| {
            payload_string(
                payload,
                &[
                    "result",
                    "result_markdown",
                    "output",
                    "result_preview",
                    "preview",
                    "error",
                    "message",
                    "reason",
                ],
            )
        })
        .filter(|value| !value.trim().is_empty())
}

fn subagent_duration_label(payload: &Value) -> Option<String> {
    let payload = payload.get("payload").unwrap_or(payload);
    payload_f64(
        payload,
        &["duration_seconds", "duration", "elapsed_seconds"],
    )
    .map(|duration| format!("{duration:.1}s"))
}

fn payload_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        let item = value.get(*key)?;
        match item {
            Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
            Value::Null => None,
            other if !other.to_string().trim().is_empty() => Some(other.to_string()),
            _ => None,
        }
    })
}

fn payload_f64(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|item| match item {
            Value::Number(number) => number.to_string().parse::<f64>().ok(),
            Value::String(text) => text.parse::<f64>().ok(),
            _ => None,
        })
    })
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
