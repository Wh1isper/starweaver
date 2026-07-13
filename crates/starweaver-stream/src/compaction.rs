//! Realtime compaction buffer for display messages.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::{
    display::{DisplayMessage, DisplayMessageKind},
    error::ReplayResult,
    replay::{ReplayCursor, ReplayCursorFamily, ReplayScope, ReplaySnapshot},
};

fn text_part_key(message: &DisplayMessage) -> String {
    message
        .payload
        .get("message_id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| message.payload.get("part_index").map(Value::to_string))
        .unwrap_or_else(|| "default".to_string())
}

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
            .map(|sequence| ReplayCursor::display(self.scope.clone(), sequence));
        ReplaySnapshot {
            scope: Some(self.scope.clone()),
            revision: self.revision,
            cursor,
            display_messages: self.items.clone(),
            metadata: starweaver_core::Metadata::default(),
        }
    }

    /// Return compacted messages after a display cursor.
    ///
    /// # Errors
    ///
    /// Returns an invalid-cursor error for another family or scope.
    pub fn tail_after(&self, cursor: Option<ReplayCursor>) -> ReplayResult<Vec<DisplayMessage>> {
        if let Some(cursor) = cursor.as_ref() {
            cursor.validate(ReplayCursorFamily::Display, &self.scope)?;
        }
        let after = cursor.map_or(0, |cursor| cursor.sequence.saturating_add(1));
        Ok(self
            .items
            .iter()
            .filter(|message| message.sequence >= after)
            .cloned()
            .collect())
    }

    fn push_text_delta(&mut self, message: DisplayMessage) {
        let part_key = text_part_key(&message);
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
            let text = format!("{current}{delta}");
            let part_kind = existing
                .payload
                .get("part_kind")
                .cloned()
                .or_else(|| message.payload.get("part_kind").cloned());
            let part_index = existing
                .payload
                .get("part_index")
                .cloned()
                .or_else(|| message.payload.get("part_index").cloned());
            existing.payload = json!({"message_id": part_key, "text": text});
            if let Some(part_kind) = part_kind {
                existing.payload["part_kind"] = part_kind;
            }
            if let Some(part_index) = part_index {
                existing.payload["part_index"] = part_index;
            }
            if existing.metadata.is_empty() {
                existing.metadata.clone_from(&message.metadata);
            } else {
                for (key, value) in &message.metadata {
                    existing
                        .metadata
                        .entry(key.clone())
                        .or_insert(value.clone());
                }
            }
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
            let part_kind = message.payload.get("part_kind").cloned();
            let part_index = message.payload.get("part_index").cloned();
            let mut compacted = message;
            compacted.payload = json!({"message_id": part_key, "text": delta});
            if let Some(part_kind) = part_kind {
                compacted.payload["part_kind"] = part_kind;
            }
            if let Some(part_index) = part_index {
                compacted.payload["part_index"] = part_index;
            }
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
            let tool_name = existing
                .payload
                .get("tool_name")
                .or_else(|| existing.payload.get("name"))
                .cloned();
            existing.payload =
                json!({"tool_call_id": call_id, "arguments_delta": format!("{current}{delta}")});
            if let Some(tool_name) = tool_name {
                existing.payload["tool_name"] = tool_name.clone();
                existing.payload["name"] = tool_name;
            }
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
            let tool_name = compacted
                .payload
                .get("tool_name")
                .or_else(|| compacted.payload.get("name"))
                .cloned();
            compacted.payload = json!({"tool_call_id": call_id, "arguments_delta": delta});
            if let Some(tool_name) = tool_name {
                compacted.payload["tool_name"] = tool_name.clone();
                compacted.payload["name"] = tool_name;
            }
            self.tool_by_call.insert(call_id, self.items.len());
            self.items.push(compacted);
        }
    }
}
