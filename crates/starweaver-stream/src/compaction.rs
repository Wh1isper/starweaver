//! Realtime compaction buffer for display messages.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::{
    display::{DisplayMessage, DisplayMessageKind},
    replay::{ReplayCursor, ReplayScope, ReplaySnapshot},
};

/// Realtime compaction buffer over display messages.
#[derive(Clone, Debug)]
pub struct RealtimeCompactionBuffer {
    scope: ReplayScope,
    revision: usize,
    max_sequence_seen: Option<usize>,
    items: Vec<DisplayMessage>,
    text_by_part: BTreeMap<String, usize>,
    tool_by_call: BTreeMap<String, usize>,
}

impl RealtimeCompactionBuffer {
    /// Create a buffer for one replay scope.
    #[must_use]
    pub const fn new(scope: ReplayScope) -> Self {
        Self {
            scope,
            revision: 0,
            max_sequence_seen: None,
            items: Vec::new(),
            text_by_part: BTreeMap::new(),
            tool_by_call: BTreeMap::new(),
        }
    }

    /// Push a live display message and compact repetitive deltas for snapshots.
    pub fn push(&mut self, message: DisplayMessage) {
        self.revision = self.revision.saturating_add(1);
        self.max_sequence_seen = Some(
            self.max_sequence_seen
                .map_or(message.sequence, |sequence| sequence.max(message.sequence)),
        );
        match message.kind {
            DisplayMessageKind::AssistantTextDelta => self.push_text_delta(message),
            DisplayMessageKind::ToolCallDelta => self.push_tool_delta(message),
            _ => self.items.push(message),
        }
    }

    /// Return a compact snapshot.
    #[must_use]
    pub fn snapshot(&self) -> ReplaySnapshot {
        let cursor = self
            .max_sequence_seen
            .map(|sequence| ReplayCursor::new(self.scope.clone(), sequence));
        ReplaySnapshot {
            scope: Some(self.scope.clone()),
            revision: self.revision,
            cursor,
            display_messages: self.items.clone(),
            metadata: starweaver_core::Metadata::default(),
        }
    }

    /// Return compacted messages after a cursor.
    #[must_use]
    pub fn tail_after(&self, cursor: Option<ReplayCursor>) -> Vec<DisplayMessage> {
        let after = cursor.map_or(0, |cursor| cursor.sequence.saturating_add(1));
        self.items
            .iter()
            .filter(|message| message.sequence >= after)
            .cloned()
            .collect()
    }

    fn push_text_delta(&mut self, message: DisplayMessage) {
        let part_key = message
            .payload
            .get("message_id")
            .or_else(|| message.payload.get("part_index"))
            .map_or_else(|| "default".to_string(), Value::to_string);
        if let Some(index) = self.text_by_part.get(&part_key).copied() {
            let delta = message
                .payload
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let existing = &mut self.items[index];
            let current = existing
                .payload
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            existing.payload = json!({"message_id": part_key, "text": format!("{current}{delta}")});
            existing.preview = existing
                .payload
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_string);
            existing.sequence = message.sequence;
            existing.timestamp = message.timestamp;
        } else {
            let delta = message
                .payload
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let mut compacted = message;
            compacted.payload = json!({"message_id": part_key, "text": delta});
            compacted.preview = Some(delta);
            self.text_by_part.insert(part_key, self.items.len());
            self.items.push(compacted);
        }
    }

    fn push_tool_delta(&mut self, message: DisplayMessage) {
        let call_id = message
            .payload
            .get("tool_call_id")
            .and_then(Value::as_str)
            .unwrap_or("tool_call")
            .to_string();
        if let Some(index) = self.tool_by_call.get(&call_id).copied() {
            let delta = message
                .payload
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let existing = &mut self.items[index];
            let current = existing
                .payload
                .get("arguments_delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            existing.payload =
                json!({"tool_call_id": call_id, "arguments_delta": format!("{current}{delta}")});
            existing.sequence = message.sequence;
            existing.timestamp = message.timestamp;
        } else {
            let delta = message
                .payload
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let mut compacted = message;
            compacted.payload = json!({"tool_call_id": call_id, "arguments_delta": delta});
            self.tool_by_call.insert(call_id, self.items.len());
            self.items.push(compacted);
        }
    }
}
