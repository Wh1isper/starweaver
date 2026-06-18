//! Subscriber/cursor message bus records for inter-agent communication.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::Metadata;
use uuid::Uuid;

/// Default maximum number of messages retained by a [`MessageBus`].
pub const DEFAULT_MESSAGE_BUS_MAXLEN: usize = 500;

/// Inter-agent or user steering message.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BusMessage {
    /// Stable message id used for idempotent send and consume.
    pub id: String,
    /// Renderable text or JSON content.
    #[serde(default)]
    pub content: Value,
    /// Sender identifier such as `user`, `main`, or a subagent id.
    pub source: String,
    /// Recipient agent id. `None` means broadcast to all subscribers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Optional template string. The Rust context stores the value; rendering is caller-owned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Message creation time.
    #[serde(default = "Utc::now")]
    pub timestamp: DateTime<Utc>,
    /// Additional message metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl BusMessage {
    /// Create a topic-scoped message.
    #[must_use]
    pub fn new(topic: impl Into<String>, payload: Value) -> Self {
        let topic = topic.into();
        let mut metadata = Metadata::default();
        metadata.insert("starweaver.topic".to_string(), Value::String(topic));
        Self {
            id: Uuid::new_v4().simple().to_string(),
            content: payload,
            source: "system".to_string(),
            target: None,
            template: None,
            timestamp: Utc::now(),
            metadata,
        }
    }

    /// Create a text bus message.
    #[must_use]
    pub fn text(content: impl Into<String>, source: impl Into<String>) -> Self {
        let content = content.into();
        Self {
            id: Uuid::new_v4().simple().to_string(),
            content: Value::String(content),
            source: source.into(),
            target: None,
            template: None,
            timestamp: Utc::now(),
            metadata: Metadata::default(),
        }
    }

    /// Set an explicit message id.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Set target recipient.
    #[must_use]
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// Set source sender.
    #[must_use]
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    /// Set template metadata.
    #[must_use]
    pub fn with_template(mut self, template: impl Into<String>) -> Self {
        self.template = Some(template.into());
        self
    }

    /// Return message text for logs, steering, and display.
    #[must_use]
    pub fn content_text(&self) -> String {
        if let Some(text) = self.content.as_str() {
            return text.to_string();
        }
        if let Some(text) = self.content.get("text").and_then(Value::as_str) {
            return text.to_string();
        }
        if let Some(text) = self.content.get("message").and_then(Value::as_str) {
            return text.to_string();
        }
        self.content.to_string()
    }

    /// Render message content. Minimal template support keeps raw content unless no JSON text exists.
    #[must_use]
    pub fn render_text(&self) -> String {
        let content = self.content_text();
        if let Some(template) = &self.template {
            template.replace("{{ content }}", &content)
        } else {
            content
        }
    }
}

impl Eq for BusMessage {}

/// Subscriber/cursor message bus with idempotent send and consume.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MessageBus {
    #[serde(default)]
    messages: Vec<BusMessage>,
    #[serde(default)]
    message_ids: BTreeSet<String>,
    #[serde(default)]
    cursors: BTreeMap<String, usize>,
    #[serde(default)]
    consumed_ids: BTreeMap<String, BTreeSet<String>>,
    #[serde(default = "default_maxlen")]
    maxlen: usize,
}

impl Default for MessageBus {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageBus {
    /// Create an empty message bus.
    #[must_use]
    pub const fn new() -> Self {
        Self::with_maxlen(DEFAULT_MESSAGE_BUS_MAXLEN)
    }

    /// Create an empty message bus with a bounded retention length.
    #[must_use]
    pub const fn with_maxlen(maxlen: usize) -> Self {
        Self {
            messages: Vec::new(),
            message_ids: BTreeSet::new(),
            cursors: BTreeMap::new(),
            consumed_ids: BTreeMap::new(),
            maxlen,
        }
    }

    /// Register a subscriber. New subscribers start at the current tail.
    pub fn subscribe(&mut self, agent_id: impl Into<String>) {
        let agent_id = agent_id.into();
        if !self.cursors.contains_key(&agent_id) {
            self.cursors.insert(agent_id.clone(), self.messages.len());
            self.consumed_ids.insert(agent_id, BTreeSet::new());
        }
    }

    /// Remove a subscriber and its cursor state.
    pub fn unsubscribe(&mut self, agent_id: &str) {
        self.cursors.remove(agent_id);
        self.consumed_ids.remove(agent_id);
    }

    /// Send a message idempotently.
    pub fn send(&mut self, message: BusMessage) -> BusMessage {
        if self.message_ids.contains(&message.id) {
            return self
                .messages
                .iter()
                .find(|existing| existing.id == message.id)
                .cloned()
                .unwrap_or(message);
        }
        self.message_ids.insert(message.id.clone());
        self.messages.push(message.clone());
        self.trim();
        message
    }

    /// Consume unread messages for a subscriber.
    pub fn consume(&mut self, agent_id: impl Into<String>) -> Vec<BusMessage> {
        self.consume_matching(agent_id, |_| true)
    }

    /// Consume unread messages matching a predicate while preserving other pending messages.
    pub fn consume_matching(
        &mut self,
        agent_id: impl Into<String>,
        predicate: impl Fn(&BusMessage) -> bool,
    ) -> Vec<BusMessage> {
        let agent_id = agent_id.into();
        if !self.cursors.contains_key(&agent_id) {
            self.subscribe(agent_id.clone());
        }
        let cursor = self.cursors.get(&agent_id).copied().unwrap_or_default();
        let consumed = self.consumed_ids.entry(agent_id.clone()).or_default();
        let mut result = Vec::new();
        for message in self.messages.iter().skip(cursor) {
            if is_deliverable(message, &agent_id)
                && !consumed.contains(&message.id)
                && predicate(message)
            {
                consumed.insert(message.id.clone());
                result.push(message.clone());
            }
        }
        let next_cursor = self
            .messages
            .iter()
            .enumerate()
            .skip(cursor)
            .find(|(_index, message)| {
                is_deliverable(message, &agent_id) && !consumed.contains(&message.id)
            })
            .map_or(self.messages.len(), |(index, _message)| index);
        self.cursors.insert(agent_id, next_cursor);
        result
    }

    /// Return unread messages without advancing the cursor.
    #[must_use]
    pub fn peek(&self, agent_id: &str) -> Vec<BusMessage> {
        let Some(cursor) = self.cursors.get(agent_id).copied() else {
            return Vec::new();
        };
        let consumed = self.consumed_ids.get(agent_id);
        self.messages
            .iter()
            .skip(cursor)
            .filter(|message| {
                is_deliverable(message, agent_id)
                    && consumed.map_or(true, |ids| !ids.contains(&message.id))
            })
            .cloned()
            .collect()
    }

    /// Return whether there are pending messages for one subscriber.
    #[must_use]
    pub fn has_pending(&self, agent_id: &str) -> bool {
        !self.peek(agent_id).is_empty()
    }

    /// Return whether any queued message has the provided topic.
    #[must_use]
    pub fn has_topic(&self, topic: &str) -> bool {
        self.messages.iter().any(|message| {
            message
                .metadata
                .get("starweaver.topic")
                .and_then(Value::as_str)
                == Some(topic)
        })
    }

    /// Clear all messages and subscriber state.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.message_ids.clear();
        self.cursors.clear();
        self.consumed_ids.clear();
    }

    /// Return number of retained messages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Return number of subscribers.
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.cursors.len()
    }

    /// Return whether the bus has no messages.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Return retained messages.
    #[must_use]
    pub fn messages(&self) -> &[BusMessage] {
        &self.messages
    }

    fn trim(&mut self) {
        if self.maxlen == 0 {
            self.clear();
            return;
        }
        if self.messages.len() <= self.maxlen {
            return;
        }
        let trim_count = self.messages.len() - self.maxlen;
        let trimmed_ids = self
            .messages
            .iter()
            .take(trim_count)
            .map(|message| message.id.clone())
            .collect::<BTreeSet<_>>();
        self.messages.drain(0..trim_count);
        for id in &trimmed_ids {
            self.message_ids.remove(id);
        }
        for cursor in self.cursors.values_mut() {
            *cursor = cursor.saturating_sub(trim_count);
        }
        for consumed in self.consumed_ids.values_mut() {
            for id in &trimmed_ids {
                consumed.remove(id);
            }
        }
    }
}

const fn default_maxlen() -> usize {
    DEFAULT_MESSAGE_BUS_MAXLEN
}

fn is_deliverable(message: &BusMessage, agent_id: &str) -> bool {
    message.target.is_none() || message.target.as_deref() == Some(agent_id)
}
