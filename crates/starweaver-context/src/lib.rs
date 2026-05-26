//! Agent context, state, event bus, and message bus primitives for Starweaver.

use std::{
    any::{Any, TypeId},
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::{ConversationId, Metadata, RunId, Usage};
use starweaver_model::ModelMessage;

/// Runtime agent identifier.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct AgentId(String);

impl AgentId {
    /// Create an identifier from a caller-provided string.
    #[must_use]
    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Return the string representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for AgentId {
    fn default() -> Self {
        Self("main".to_string())
    }
}

/// In-memory state store for context domains.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateStore {
    domains: BTreeMap<String, Value>,
}

impl StateStore {
    /// Create an empty state store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a domain value.
    pub fn set(&mut self, key: impl Into<String>, value: Value) {
        self.domains.insert(key.into(), value);
    }

    /// Get a domain value.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.domains.get(key)
    }

    /// Remove a domain value.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.domains.remove(key)
    }

    /// Return all domains.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn domains(&self) -> &BTreeMap<String, Value> {
        &self.domains
    }
}

/// Runtime event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentEvent {
    /// Event type.
    pub kind: String,
    /// Event payload.
    #[serde(default)]
    pub payload: Value,
    /// Event metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl AgentEvent {
    /// Create an event.
    #[must_use]
    pub fn new(kind: impl Into<String>, payload: Value) -> Self {
        Self {
            kind: kind.into(),
            payload,
            metadata: Metadata::default(),
        }
    }
}

/// Append-only in-memory event bus.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EventBus {
    events: Vec<AgentEvent>,
}

impl EventBus {
    /// Create an empty event bus.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish one event.
    pub fn publish(&mut self, event: AgentEvent) {
        self.events.push(event);
    }

    /// Return all events.
    #[must_use]
    pub fn events(&self) -> &[AgentEvent] {
        &self.events
    }

    /// Drain all events.
    pub fn drain(&mut self) -> Vec<AgentEvent> {
        std::mem::take(&mut self.events)
    }
}

/// Steering or coordination message.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BusMessage {
    /// Message topic.
    pub topic: String,
    /// Message payload.
    #[serde(default)]
    pub payload: Value,
    /// Message metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl BusMessage {
    /// Create a bus message.
    #[must_use]
    pub fn new(topic: impl Into<String>, payload: Value) -> Self {
        Self {
            topic: topic.into(),
            payload,
            metadata: Metadata::default(),
        }
    }
}

/// FIFO message bus for steering active and future runs.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct MessageBus {
    messages: VecDeque<BusMessage>,
}

impl MessageBus {
    /// Create an empty message bus.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue one message.
    pub fn enqueue(&mut self, message: BusMessage) {
        self.messages.push_back(message);
    }

    /// Dequeue one message.
    pub fn dequeue(&mut self) -> Option<BusMessage> {
        self.messages.pop_front()
    }

    /// Return number of queued messages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Return whether the bus has no messages.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

/// Serializable state used to restore an agent context.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResumableState {
    /// Agent identifier.
    pub agent_id: AgentId,
    /// Current run identifier when exported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Conversation identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    /// Canonical message history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message_history: Vec<ModelMessage>,
    /// Accumulated usage.
    #[serde(default)]
    pub usage: Usage,
    /// State domains.
    #[serde(default)]
    pub state: StateStore,
    /// Pending bus messages.
    #[serde(default)]
    pub message_bus: MessageBus,
    /// Run metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Type-indexed dependency container for runtime and tool contexts.
#[derive(Clone, Default)]
pub struct DependencyStore {
    values: BTreeMap<String, Arc<dyn Any + Send + Sync>>,
    type_keys: BTreeMap<TypeId, String>,
}

impl DependencyStore {
    /// Create an empty dependency store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a dependency using its Rust type as the lookup key.
    pub fn insert<T>(&mut self, value: T)
    where
        T: Send + Sync + 'static,
    {
        self.insert_named(std::any::type_name::<T>(), value);
    }

    /// Insert a dependency with a caller-provided stable name.
    pub fn insert_named<T>(&mut self, name: impl Into<String>, value: T)
    where
        T: Send + Sync + 'static,
    {
        let name = name.into();
        self.type_keys.insert(TypeId::of::<T>(), name.clone());
        self.values.insert(name, Arc::new(value));
    }

    /// Get a dependency by Rust type.
    #[must_use]
    pub fn get<T>(&self) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.type_keys
            .get(&TypeId::of::<T>())
            .and_then(|name| self.get_named(name))
    }

    /// Get a dependency by stable name.
    #[must_use]
    pub fn get_named<T>(&self, name: &str) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.values
            .get(name)
            .cloned()
            .and_then(|value| value.downcast::<T>().ok())
    }

    /// Return all named dependency keys.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        self.values.keys().cloned().collect()
    }

    /// Return whether the store has no dependencies.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl std::fmt::Debug for DependencyStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DependencyStore")
            .field("keys", &self.keys())
            .finish()
    }
}

/// Lifecycle-wide agent context.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentContext {
    /// Agent identifier.
    pub agent_id: AgentId,
    /// Current run identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Conversation identifier.
    pub conversation_id: ConversationId,
    /// Canonical message history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message_history: Vec<ModelMessage>,
    /// Accumulated usage.
    #[serde(default)]
    pub usage: Usage,
    /// State store.
    #[serde(default)]
    pub state: StateStore,
    /// Event bus.
    #[serde(default)]
    pub events: EventBus,
    /// Message bus.
    #[serde(default)]
    pub messages: MessageBus,
    /// Context metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
    /// Typed dependencies, skipped from serialization.
    #[serde(skip)]
    pub dependencies: DependencyStore,
}

impl AgentContext {
    /// Create a fresh context.
    #[must_use]
    pub fn new(agent_id: AgentId) -> Self {
        Self {
            agent_id,
            run_id: None,
            conversation_id: ConversationId::new(),
            message_history: Vec::new(),
            usage: Usage::default(),
            state: StateStore::new(),
            events: EventBus::new(),
            messages: MessageBus::new(),
            metadata: Metadata::default(),
            dependencies: DependencyStore::new(),
        }
    }

    /// Restore a context from serialized state.
    #[must_use]
    pub fn from_state(state: ResumableState) -> Self {
        Self {
            agent_id: state.agent_id,
            run_id: state.run_id,
            conversation_id: state.conversation_id.unwrap_or_default(),
            message_history: state.message_history,
            usage: state.usage,
            state: state.state,
            events: EventBus::new(),
            messages: state.message_bus,
            metadata: state.metadata,
            dependencies: DependencyStore::new(),
        }
    }

    /// Export context state for session restoration.
    #[must_use]
    pub fn export_state(&self) -> ResumableState {
        ResumableState {
            agent_id: self.agent_id.clone(),
            run_id: self.run_id.clone(),
            conversation_id: Some(self.conversation_id.clone()),
            message_history: self.message_history.clone(),
            usage: self.usage.clone(),
            state: self.state.clone(),
            message_bus: self.messages.clone(),
            metadata: self.metadata.clone(),
        }
    }

    /// Replace context with serialized state.
    pub fn restore_state(&mut self, state: ResumableState) {
        *self = Self::from_state(state);
    }

    /// Record a model message in context history.
    pub fn push_message(&mut self, message: ModelMessage) {
        self.message_history.push(message);
    }

    /// Record usage in the context ledger.
    pub fn add_usage(&mut self, usage: &Usage) {
        self.usage.add_assign(usage);
    }

    /// Publish an event.
    pub fn publish_event(&mut self, event: AgentEvent) {
        self.events.publish(event);
    }

    /// Enqueue a message.
    pub fn enqueue_message(&mut self, message: BusMessage) {
        self.messages.enqueue(message);
    }

    /// Insert a typed dependency.
    pub fn insert_dependency<T>(&mut self, value: T)
    where
        T: Send + Sync + 'static,
    {
        self.dependencies.insert(value);
    }

    /// Insert a named typed dependency.
    pub fn insert_named_dependency<T>(&mut self, name: impl Into<String>, value: T)
    where
        T: Send + Sync + 'static,
    {
        self.dependencies.insert_named(name, value);
    }

    /// Get a typed dependency.
    #[must_use]
    pub fn dependency<T>(&self) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.dependencies.get::<T>()
    }

    /// Get a named typed dependency.
    #[must_use]
    pub fn named_dependency<T>(&self, name: &str) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.dependencies.get_named::<T>(name)
    }
}

impl Default for AgentContext {
    fn default() -> Self {
        Self::new(AgentId::default())
    }
}
