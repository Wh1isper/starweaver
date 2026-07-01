//! Typed agent stream event foundations.

use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use starweaver_context::AgentEvent;
use starweaver_core::{AgentId, ConversationId, Metadata, RunId, TaskId};
use starweaver_model::{ModelResponse, ModelResponseStreamEvent, ToolCallPart, ToolReturnPart};

use crate::{AgentResult, executor::AgentExecutionNode, run::RunStatus};

/// Stable category for context sideband events that are bridged into the runtime stream.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSidebandEventCategory {
    /// Run lifecycle and status events.
    Run,
    /// Model retry or model-side diagnostics.
    Model,
    /// Tool availability or direct tool lifecycle events.
    Tool,
    /// Dynamic tool-search discovery, loading, initialization, or refresh events.
    ToolSearch,
    /// Human-in-the-loop approval or deferred-tool decision events.
    Hitl,
    /// Skill scan, activation, or reload events.
    Skill,
    /// Task list or task state events.
    Task,
    /// Note state events.
    Note,
    /// File state or file-operation events.
    File,
    /// Media state or media-operation events.
    Media,
    /// Host adapter events.
    HostEvent,
    /// Subagent lifecycle events.
    Subagent,
    /// Message bus events.
    Message,
    /// Usage snapshot or usage-limit events.
    Usage,
    /// Context compaction lifecycle events.
    Compact,
    /// User steering events.
    Steering,
    /// Runtime goal-mode events.
    Goal,
    /// Background process or background shell events.
    Background,
    /// Capability-specific events that do not fit a narrower category.
    Capability,
}

/// Typed view of an application sideband event carried by [`AgentStreamEvent::Custom`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentSidebandEvent {
    /// Stable event category.
    pub category: AgentSidebandEventCategory,
    /// Event type.
    pub kind: String,
    /// Event payload.
    #[serde(default)]
    pub payload: serde_json::Value,
    /// Event metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl AgentSidebandEvent {
    /// Build a typed sideband event when the context event kind belongs to the stable taxonomy.
    #[must_use]
    pub fn from_agent_event(event: &AgentEvent) -> Option<Self> {
        Self::category_for_kind(&event.kind).map(|category| Self {
            category,
            kind: event.kind.clone(),
            payload: event.payload.clone(),
            metadata: event.metadata.clone(),
        })
    }

    /// Classify a context event kind into the stable sideband taxonomy.
    #[must_use]
    pub fn category_for_kind(kind: &str) -> Option<AgentSidebandEventCategory> {
        match kind {
            "run_start" | "run_complete" | "run_failed" | "run_waiting" | "run_cancelled" => {
                Some(AgentSidebandEventCategory::Run)
            }
            "model_error_retry"
            | "model_stream_resume"
            | "model_transport_selected"
            | "model_transport_fallback" => Some(AgentSidebandEventCategory::Model),
            "tools_unavailable"
            | "toolset_initialized"
            | "toolset_unavailable"
            | "toolset_failed"
            | "toolset_refreshed"
            | "toolset_closed" => Some(AgentSidebandEventCategory::Tool),
            "tool_search_loaded" | "tool_search_initialized" | "tool_search_refreshed" => {
                Some(AgentSidebandEventCategory::ToolSearch)
            }
            "hitl_resolved" => Some(AgentSidebandEventCategory::Hitl),
            "skills_scanned" | "skill_activated" | "skills_reloaded" => {
                Some(AgentSidebandEventCategory::Skill)
            }
            "task_snapshot" => Some(AgentSidebandEventCategory::Task),
            "usage_snapshot" => Some(AgentSidebandEventCategory::Usage),
            "compact_start" | "compact_failed" | "compact_complete" => {
                Some(AgentSidebandEventCategory::Compact)
            }
            "steering_received" | "steering_submitted" => {
                Some(AgentSidebandEventCategory::Steering)
            }
            "goal_iteration" | "goal_complete" => Some(AgentSidebandEventCategory::Goal),
            "background_shell_complete" => Some(AgentSidebandEventCategory::Background),
            "message_received" => Some(AgentSidebandEventCategory::Message),
            "subagent_started" | "subagent_completed" | "subagent_failed" => {
                Some(AgentSidebandEventCategory::Subagent)
            }
            _ if kind.starts_with("model_") => Some(AgentSidebandEventCategory::Model),
            _ if kind.starts_with("task_") => Some(AgentSidebandEventCategory::Task),
            _ if kind.starts_with("note_") => Some(AgentSidebandEventCategory::Note),
            _ if kind.starts_with("file_") => Some(AgentSidebandEventCategory::File),
            _ if kind.starts_with("media_") => Some(AgentSidebandEventCategory::Media),
            _ if kind.starts_with("host_") => Some(AgentSidebandEventCategory::HostEvent),
            _ if kind.starts_with("tool_search_") => Some(AgentSidebandEventCategory::ToolSearch),
            _ if kind.starts_with("tool_") || kind.starts_with("toolset_") => {
                Some(AgentSidebandEventCategory::Tool)
            }
            _ if kind.starts_with("approval_")
                || kind.starts_with("deferred_")
                || kind.starts_with("hitl_") =>
            {
                Some(AgentSidebandEventCategory::Hitl)
            }
            _ if kind.starts_with("skill_") || kind.starts_with("skills_") => {
                Some(AgentSidebandEventCategory::Skill)
            }
            _ if kind.starts_with("subagent_") => Some(AgentSidebandEventCategory::Subagent),
            _ if kind.starts_with("message_") => Some(AgentSidebandEventCategory::Message),
            _ if kind.starts_with("usage_") => Some(AgentSidebandEventCategory::Usage),
            _ if kind.starts_with("compact_") => Some(AgentSidebandEventCategory::Compact),
            _ if kind.starts_with("steering_") => Some(AgentSidebandEventCategory::Steering),
            _ if kind.starts_with("goal_") => Some(AgentSidebandEventCategory::Goal),
            _ if kind.starts_with("background_") => Some(AgentSidebandEventCategory::Background),
            _ if kind.starts_with("capability_") => Some(AgentSidebandEventCategory::Capability),
            _ => None,
        }
    }
}

/// Typed event emitted by the agent runtime while a run progresses.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentStreamEvent {
    /// A run started.
    RunStart {
        /// Run identifier.
        run_id: RunId,
        /// Conversation identifier.
        conversation_id: ConversationId,
    },
    /// Runtime execution entered a durable node boundary.
    NodeStart {
        /// Execution boundary being entered.
        node: AgentExecutionNode,
        /// Completed run step at this boundary.
        step: usize,
        /// Current run status at this boundary.
        status: RunStatus,
    },
    /// Runtime execution completed a durable node boundary.
    NodeComplete {
        /// Execution boundary that completed.
        node: AgentExecutionNode,
        /// Completed run step at this boundary.
        step: usize,
        /// Current run status after this boundary.
        status: RunStatus,
    },
    /// A context sideband event was published during the run.
    Custom {
        /// Application or capability event.
        event: AgentEvent,
    },
    /// A model request was prepared for a loop step.
    ModelRequest {
        /// Completed run step before sending the request.
        step: usize,
    },
    /// A model response stream event was received.
    ModelStream {
        /// Completed run step for the active model request.
        step: usize,
        /// Canonical model stream event.
        event: ModelResponseStreamEvent,
    },
    /// A model response was received.
    ModelResponse {
        /// Completed run step after receiving the response.
        step: usize,
        /// Canonical model response.
        response: ModelResponse,
    },
    /// A durable execution checkpoint was persisted or inspected.
    Checkpoint {
        /// Execution boundary.
        node: AgentExecutionNode,
        /// Completed run step at this boundary.
        step: usize,
    },
    /// Execution was suspended at a durable checkpoint.
    Suspended {
        /// Execution boundary that requested suspension.
        node: AgentExecutionNode,
        /// Human-readable suspend reason.
        reason: String,
    },
    /// A model requested a function tool call.
    ToolCall {
        /// Current run step.
        step: usize,
        /// Tool call part.
        call: ToolCallPart,
    },
    /// A function tool returned a result or structured control-flow error.
    ToolReturn {
        /// Current run step.
        step: usize,
        /// Tool return part.
        tool_return: ToolReturnPart,
    },
    /// Output validation or output function validation requested another model turn.
    OutputRetry {
        /// Retry count after this retry was scheduled.
        retries: usize,
        /// Retry prompt sent to the model.
        prompt: String,
    },
    /// Pending user steering requested another model turn before finalization.
    SteeringGuard {
        /// Current run step.
        step: usize,
        /// Control prompt sent to the model before finalization.
        prompt: String,
    },
    /// A run completed successfully.
    RunComplete {
        /// Run identifier.
        run_id: RunId,
        /// Final output text.
        output: String,
    },
    /// A run failed after preserving recoverable context state.
    RunFailed {
        /// Run identifier.
        run_id: RunId,
        /// Failure kind.
        error_kind: String,
        /// Human-readable error message.
        message: String,
    },
}

impl AgentStreamEvent {
    /// Return a typed sideband event view for known context events carried by `Custom`.
    #[must_use]
    pub fn sideband_event(&self) -> Option<AgentSidebandEvent> {
        match self {
            Self::Custom { event } => AgentSidebandEvent::from_agent_event(event),
            _ => None,
        }
    }
}

/// Origin type for a stream record emitted by a nested runtime component.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStreamSourceKind {
    /// Record emitted by a delegated subagent run.
    Subagent,
}

/// Source attribution for records merged from nested agent runs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentStreamSource {
    /// Source category.
    pub kind: AgentStreamSourceKind,
    /// Source agent identifier.
    pub agent_id: AgentId,
    /// Human-readable source agent name.
    pub agent_name: String,
    /// Delegated task identifier when the source is task scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    /// Source run identifier when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Parent run identifier when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<RunId>,
    /// Original sequence number in the source run before parent stream rebasing.
    pub source_sequence: usize,
}

impl AgentStreamSource {
    /// Build subagent source attribution.
    #[must_use]
    pub fn subagent(
        agent_id: AgentId,
        agent_name: impl Into<String>,
        task_id: TaskId,
        run_id: Option<RunId>,
        parent_run_id: Option<RunId>,
        source_sequence: usize,
    ) -> Self {
        Self {
            kind: AgentStreamSourceKind::Subagent,
            agent_id,
            agent_name: agent_name.into(),
            task_id: Some(task_id),
            run_id,
            parent_run_id,
            source_sequence,
        }
    }
}

/// In-memory sink used by tools that need to merge child stream records into the parent stream.
#[derive(Clone, Debug, Default)]
pub struct AgentStreamSink {
    records: Arc<Mutex<Vec<AgentStreamRecord>>>,
}

impl AgentStreamSink {
    /// Add one record to the sink.
    pub fn push(&self, record: AgentStreamRecord) {
        self.records
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(record);
    }

    /// Add several records to the sink.
    pub fn extend(&self, records: impl IntoIterator<Item = AgentStreamRecord>) {
        self.records
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(records);
    }

    /// Drain all currently buffered records.
    #[must_use]
    pub fn drain(&self) -> Vec<AgentStreamRecord> {
        self.records
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect()
    }

    /// Return whether no records are buffered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }
}

/// Sequenced stream event record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentStreamRecord {
    /// Monotonic event sequence number within one run.
    pub sequence: usize,
    /// Source attribution for records merged from nested runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<AgentStreamSource>,
    /// Typed event payload.
    pub event: AgentStreamEvent,
}

impl AgentStreamRecord {
    /// Create a sequenced stream record.
    #[must_use]
    pub const fn new(sequence: usize, event: AgentStreamEvent) -> Self {
        Self {
            sequence,
            source: None,
            event,
        }
    }

    /// Attach source attribution.
    #[must_use]
    pub fn with_source(mut self, source: AgentStreamSource) -> Self {
        self.source = Some(source);
        self
    }

    /// Replace the parent stream sequence after merging into another stream.
    #[must_use]
    pub const fn with_sequence(mut self, sequence: usize) -> Self {
        self.sequence = sequence;
        self
    }
}

/// Result returned by collection-based stream runs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentStreamResult {
    /// Final agent result.
    pub result: AgentResult,
    /// Events captured while the run progressed.
    pub events: Vec<AgentStreamRecord>,
}

impl AgentStreamResult {
    /// Return captured stream records.
    #[must_use]
    pub fn events(&self) -> &[AgentStreamRecord] {
        &self.events
    }

    /// Return the final result.
    #[must_use]
    pub const fn result(&self) -> &AgentResult {
        &self.result
    }
}

pub(crate) fn push_stream_event(
    events: &mut Option<&mut Vec<AgentStreamRecord>>,
    event: AgentStreamEvent,
) {
    if let Some(events) = events.as_deref_mut() {
        events.push(AgentStreamRecord::new(events.len(), event));
    }
}

pub(crate) fn push_stream_record(
    events: &mut Option<&mut Vec<AgentStreamRecord>>,
    record: AgentStreamRecord,
) {
    if let Some(events) = events.as_deref_mut() {
        events.push(record.with_sequence(events.len()));
    }
}
