#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use starweaver_context::{AgentContext, AgentContextHandle, AgentEvent};
use starweaver_core::{AgentId, RunId, TaskId, TraceContext};
use starweaver_model::{
    ModelAdapter, ModelError, ModelMessage, ModelProfile, ModelRequestContext,
    ModelRequestParameters, ModelResponse, ModelResponsePart, ModelSettings, ProtocolFamily,
    ToolCallPart,
};
use starweaver_runtime::{
    Agent, AgentCapability, AgentCheckpoint, AgentError, AgentExecutionDecision,
    AgentExecutionNode, AgentExecutor, AgentExecutorError, AgentSidebandEventCategory,
    AgentStreamEvent, AgentStreamRecord, AgentStreamSource, CapabilityError, CapabilityResult,
    OutputSchema, StaticCapabilityBundle,
};
use starweaver_tools::{FunctionTool, ToolContext, ToolRegistry, ToolResult};

#[derive(Clone)]
struct ScriptedModel {
    responses: Arc<Mutex<Vec<ModelResponse>>>,
}

impl ScriptedModel {
    fn new(responses: Vec<ModelResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into_iter().rev().collect())),
        }
    }
}

#[async_trait]
impl ModelAdapter for ScriptedModel {
    fn model_name(&self) -> &'static str {
        "scripted"
    }

    fn provider_name(&self) -> Option<&'static str> {
        Some("test")
    }

    fn profile(&self) -> &ModelProfile {
        static PROFILE: LazyLock<ModelProfile> =
            LazyLock::new(|| ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions));
        &PROFILE
    }

    fn default_settings(&self) -> Option<&ModelSettings> {
        None
    }

    async fn request(
        &self,
        _messages: Vec<ModelMessage>,
        _settings: Option<ModelSettings>,
        _params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        self.responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| ModelError::Transport("script exhausted".to_string()))
    }
}

fn lookup_registry() -> ToolRegistry {
    let tool = FunctionTool::new(
        "lookup",
        Some("Lookup a value".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );
    ToolRegistry::new().with_tool(Arc::new(tool))
}

#[test]
fn stream_protocol_owner_types_preserve_runtime_and_context_compatibility_paths() {
    let stream_record = starweaver_stream::AgentStreamRecord::new(
        0,
        starweaver_stream::AgentStreamEvent::ModelRequest { step: 0 },
    );
    let runtime_record: starweaver_runtime::AgentStreamRecord = stream_record;
    let module_record: starweaver_runtime::stream::AgentStreamRecord = runtime_record;
    assert_eq!(module_record.sequence, 0);

    let core_node: starweaver_core::AgentExecutionNode =
        starweaver_runtime::AgentExecutionNode::RunStart;
    let module_node: starweaver_runtime::executor::AgentExecutionNode = core_node;
    assert_eq!(module_node, starweaver_core::AgentExecutionNode::RunStart);

    let context_event = starweaver_context::AgentEvent::new("compat", serde_json::json!({}));
    let core_event: starweaver_core::AgentEvent = context_event;
    assert_eq!(core_event.kind, "compat");
}

#[test]
fn stream_record_source_defaults_to_none_and_skips_serialization() {
    let record = AgentStreamRecord::new(7, AgentStreamEvent::ModelRequest { step: 1 });

    assert!(record.source.is_none());
    let value = serde_json::to_value(&record).unwrap();
    assert!(value.get("source").is_none());
    let restored: AgentStreamRecord = serde_json::from_value(value).unwrap();
    assert!(restored.source.is_none());
    assert_eq!(restored.sequence, 7);
}

#[test]
fn stream_record_raw_json_projection_matches_serialized_record() {
    let record = AgentStreamRecord::new(7, AgentStreamEvent::ModelRequest { step: 1 });

    let raw = record.to_raw_json().unwrap();

    assert_eq!(raw, serde_json::to_value(&record).unwrap());
    assert_eq!(raw["sequence"], 7);
    assert_eq!(raw["event"]["kind"], "model_request");
}

#[test]
fn stream_record_source_attribution_round_trips() {
    let source = AgentStreamSource::subagent(
        AgentId::from_string("child-agent"),
        "child",
        TaskId::from_string("task-1"),
        Some(RunId::from_string("run-child")),
        Some(RunId::from_string("run-parent")),
        3,
    );
    let record =
        AgentStreamRecord::new(10, AgentStreamEvent::ModelRequest { step: 1 }).with_source(source);

    let value = serde_json::to_value(&record).unwrap();

    assert_eq!(value["source"]["kind"], "subagent");
    assert_eq!(value["source"]["agent_id"], "child-agent");
    assert_eq!(value["source"]["agent_name"], "child");
    assert_eq!(value["source"]["task_id"], "task-1");
    assert_eq!(value["source"]["run_id"], "run-child");
    assert_eq!(value["source"]["parent_run_id"], "run-parent");
    assert_eq!(value["source"]["source_sequence"], 3);
    let restored: AgentStreamRecord = serde_json::from_value(value).unwrap();
    assert_eq!(
        restored.source.unwrap().parent_run_id.unwrap().as_str(),
        "run-parent"
    );
}

#[tokio::test]
async fn run_stream_collects_text_run_events() {
    let stream = Agent::new(Arc::new(ScriptedModel::new(vec![ModelResponse::text(
        "hello",
    )])))
    .run_stream("hi")
    .await
    .unwrap();

    assert_eq!(stream.result.output, "hello");
    assert_eq!(stream.events[0].sequence, 0);
    assert!(matches!(
        stream.events[0].event,
        AgentStreamEvent::RunStart { .. }
    ));
    assert!(stream.events.iter().any(|record| matches!(
        record.event,
        AgentStreamEvent::NodeStart {
            node: AgentExecutionNode::RunStart,
            step: 0,
            ..
        }
    )));
    assert!(stream.events.iter().any(|record| matches!(
        record.event,
        AgentStreamEvent::Checkpoint {
            node: AgentExecutionNode::RunStart,
            step: 0
        }
    )));
    assert!(stream.events.iter().any(|record| matches!(
        record.event,
        AgentStreamEvent::NodeComplete {
            node: AgentExecutionNode::RunStart,
            step: 0,
            ..
        }
    )));
    assert!(
        stream
            .events
            .iter()
            .any(|record| matches!(record.event, AgentStreamEvent::ModelRequest { step: 0 }))
    );
    assert!(stream.events.iter().any(|record| matches!(
        record.event,
        AgentStreamEvent::ModelResponse { step: 1, .. }
    )));
    assert!(
        matches!(stream.events.last().unwrap().event, AgentStreamEvent::RunComplete { ref output, .. } if output == "hello")
    );
    assert!(
        stream
            .events
            .windows(2)
            .all(|window| window[0].sequence + 1 == window[1].sequence)
    );
    let raw = stream.raw_json_records().unwrap();
    assert_eq!(raw.len(), stream.events.len());
    assert_eq!(raw[0]["sequence"], 0);
    assert_eq!(raw[0]["event"]["kind"], "run_start");
}

#[tokio::test]
async fn stream_events_include_tool_calls_and_returns() {
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_1".to_string(),
                name: "lookup".to_string(),
                arguments: serde_json::json!({"query": "Paris"}).into(),
            })],
            ..ModelResponse::text("")
        },
        ModelResponse::text("done"),
    ]));

    let stream = Agent::new(model)
        .with_tools(lookup_registry())
        .run_stream("lookup")
        .await
        .unwrap();

    assert!(stream
        .events
        .iter()
        .any(|record| matches!(record.event, AgentStreamEvent::ToolCall { ref call, .. } if call.name == "lookup")));
    assert!(stream
        .events
        .iter()
        .any(|record| matches!(record.event, AgentStreamEvent::ToolReturn { ref tool_return, .. } if tool_return.name == "lookup" && !tool_return.is_error)));
    assert!(matches!(
        stream.events.last().unwrap().event,
        AgentStreamEvent::RunComplete { .. }
    ));
}

#[tokio::test]
async fn stream_events_include_output_retries() {
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse::text("not json"),
        ModelResponse::text(r#"{"answer":"ok"}"#),
    ]));

    let stream = Agent::new(model)
        .with_output_schema(OutputSchema::new(
            "answer",
            serde_json::json!({"type": "object", "required": ["answer"]}),
        ))
        .run_stream("answer as json")
        .await
        .unwrap();

    assert_eq!(stream.result.structured_output.unwrap()["answer"], "ok");
    assert!(stream.events.iter().any(|record| matches!(
        record.event,
        AgentStreamEvent::OutputRetry { retries: 1, .. }
    )));
}

#[tokio::test]
async fn run_with_context_can_collect_stream_events() {
    let original_trace = TraceContext::from_trace_id("trace-before-success");
    let mut context = AgentContext {
        trace_context: original_trace.clone(),
        ..AgentContext::default()
    };
    let mut events = Vec::new();

    let result = Agent::new(Arc::new(ScriptedModel::new(vec![ModelResponse::text(
        "context",
    )])))
    .run_with_context_and_stream_events("hi", &mut context, &mut events)
    .await
    .unwrap();

    assert_eq!(result.output, "context");
    assert_eq!(context.events.events().len(), 2);
    assert!(events.iter().any(|record| matches!(
        record.event,
        AgentStreamEvent::NodeStart {
            node: AgentExecutionNode::RunComplete,
            ..
        }
    )));
    assert!(events.iter().any(|record| matches!(
        record.event,
        AgentStreamEvent::Checkpoint {
            node: AgentExecutionNode::RunComplete,
            ..
        }
    )));
    assert!(matches!(
        events.last().unwrap().event,
        AgentStreamEvent::RunComplete { .. }
    ));
    assert_eq!(
        events
            .iter()
            .filter(|record| matches!(record.event, AgentStreamEvent::RunComplete { .. }))
            .count(),
        1
    );
    assert!(!context.runtime.lifecycle.entered);
    assert!(context.ended_at.is_some());
    assert_eq!(context.trace_context, original_trace);
}

struct SuspendAtBeforeModelRequest;

#[async_trait]
impl AgentExecutor for SuspendAtBeforeModelRequest {
    async fn checkpoint(
        &self,
        checkpoint: AgentCheckpoint,
    ) -> Result<AgentExecutionDecision, AgentExecutorError> {
        if checkpoint.node == AgentExecutionNode::BeforeModelRequest {
            return Ok(AgentExecutionDecision::Suspend {
                reason: "waiting for approval".to_string(),
            });
        }
        Ok(AgentExecutionDecision::Continue)
    }
}

#[tokio::test]
async fn stream_events_include_checkpoints_and_suspension() {
    let original_trace = TraceContext::from_trace_id("trace-before-suspension");
    let mut context = AgentContext {
        trace_context: original_trace.clone(),
        ..AgentContext::default()
    };
    let mut events = Vec::new();
    let error = Agent::new(Arc::new(ScriptedModel::new(vec![ModelResponse::text(
        "never reached",
    )])))
    .with_executor(Arc::new(SuspendAtBeforeModelRequest))
    .run_with_context_and_stream_events("hi", &mut context, &mut events)
    .await;

    assert!(matches!(
        error,
        Err(AgentError::ExecutionSuspended {
            node: AgentExecutionNode::BeforeModelRequest,
            ..
        })
    ));
    assert!(events.iter().any(|record| matches!(
        record.event,
        AgentStreamEvent::Checkpoint {
            node: AgentExecutionNode::RunStart,
            step: 0,
        }
    )));
    assert!(events.iter().any(|record| matches!(
        record.event,
        AgentStreamEvent::Checkpoint {
            node: AgentExecutionNode::BeforeModelRequest,
            step: 0,
        }
    )));
    assert!(events.iter().any(|record| matches!(
        record.event,
        AgentStreamEvent::Suspended {
            node: AgentExecutionNode::BeforeModelRequest,
            ref reason,
        } if reason == "waiting for approval"
    )));
    assert!(events.iter().any(|record| matches!(
        record.event,
        AgentStreamEvent::NodeComplete {
            node: AgentExecutionNode::BeforeModelRequest,
            ..
        }
    )));
    assert_eq!(
        events
            .iter()
            .filter(|record| matches!(record.event, AgentStreamEvent::Suspended { .. }))
            .count(),
        1
    );
    assert!(!context.runtime.lifecycle.entered);
    assert!(context.ended_at.is_some());
    assert_eq!(context.trace_context, original_trace);
}

#[tokio::test]
async fn stream_events_expose_context_sideband_events() {
    let tool = FunctionTool::new(
        "announce",
        Some("Publish a sideband event".to_string()),
        serde_json::json!({"type": "object"}),
        |ctx: ToolContext, args: serde_json::Value| async move {
            if let Some(handle) = ctx.dependency::<AgentContextHandle>() {
                let mut context = handle.snapshot();
                context.publish_event(AgentEvent::new("tool_progress", args.clone()));
                handle.replace(context);
            }
            Ok(ToolResult::new(args))
        },
    );
    let registry = ToolRegistry::new().with_tool(Arc::new(tool));
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_1".to_string(),
                name: "announce".to_string(),
                arguments: serde_json::json!({"status": "working"}).into(),
            })],
            ..ModelResponse::text("")
        },
        ModelResponse::text("done"),
    ]));

    let stream = Agent::new(model)
        .with_tools(registry)
        .run_stream("announce progress")
        .await
        .unwrap();

    assert!(stream.events.iter().any(|record| matches!(
        &record.event,
        AgentStreamEvent::Custom { event }
            if event.kind == "tool_progress" && event.payload["status"] == "working"
    )));
    let progress = stream
        .events
        .iter()
        .filter_map(|record| record.event.sideband_event())
        .find(|event| event.kind == "tool_progress")
        .unwrap();
    assert_eq!(progress.category, AgentSidebandEventCategory::Tool);
    assert_eq!(progress.kind, "tool_progress");
    assert_eq!(progress.payload["status"], "working");
}

#[test]
fn sideband_event_classifies_stable_context_event_taxonomy() {
    let event = AgentStreamEvent::Custom {
        event: AgentEvent::new(
            "tool_search_initialized",
            serde_json::json!({"namespace_count": 2}),
        ),
    };
    let sideband = event.sideband_event().unwrap();
    assert_eq!(sideband.category, AgentSidebandEventCategory::ToolSearch);
    assert_eq!(sideband.kind, "tool_search_initialized");
    assert_eq!(sideband.payload["namespace_count"], 2);

    let event = AgentStreamEvent::Custom {
        event: AgentEvent::new("toolset_initialized", serde_json::json!({"tool_count": 2})),
    };
    let sideband = event.sideband_event().unwrap();
    assert_eq!(sideband.category, AgentSidebandEventCategory::Tool);
    assert_eq!(sideband.kind, "toolset_initialized");

    let event = AgentStreamEvent::Custom {
        event: AgentEvent::new("skill_activated", serde_json::json!({"name": "rust"})),
    };
    let sideband = event.sideband_event().unwrap();
    assert_eq!(sideband.category, AgentSidebandEventCategory::Skill);

    let event = AgentStreamEvent::Custom {
        event: AgentEvent::new("skills_reloaded", serde_json::json!({"changes": []})),
    };
    let sideband = event.sideband_event().unwrap();
    assert_eq!(sideband.category, AgentSidebandEventCategory::Skill);

    let event = AgentStreamEvent::Custom {
        event: AgentEvent::new("hitl_resolved", serde_json::json!({"approved": 1})),
    };
    let sideband = event.sideband_event().unwrap();
    assert_eq!(sideband.category, AgentSidebandEventCategory::Hitl);

    let event = AgentStreamEvent::Custom {
        event: AgentEvent::new("external_signal", serde_json::json!({})),
    };
    assert!(event.sideband_event().is_none());
}

#[tokio::test]
async fn capability_bundle_stream_observer_sees_recorded_events() {
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let observer = Arc::new(StreamObserverRecorder {
        events: events.clone(),
    });
    let bundle = StaticCapabilityBundle::new("stream-observer").with_stream_observer(observer);

    let result = Agent::new(Arc::new(ScriptedModel::new(vec![ModelResponse::text(
        "observed",
    )])))
    .with_capability_bundle(&bundle)
    .run_stream("hi")
    .await
    .unwrap();

    assert_eq!(result.result.output, "observed");
    let recorded_kinds = events.lock().unwrap().clone();
    assert!(recorded_kinds.iter().any(|kind| kind == "run_start"));
    assert!(recorded_kinds.iter().any(|kind| kind == "node_start"));
    assert!(recorded_kinds.iter().any(|kind| kind == "checkpoint"));
    assert!(recorded_kinds.iter().any(|kind| kind == "node_complete"));
    assert!(recorded_kinds.iter().any(|kind| kind == "model_request"));
    assert!(recorded_kinds.iter().any(|kind| kind == "model_response"));
    assert!(recorded_kinds.iter().any(|kind| kind == "run_complete"));
}

struct FailOnRunCompleteObserver;

#[async_trait]
impl AgentCapability for FailOnRunCompleteObserver {
    async fn on_stream_event(
        &self,
        _state: &starweaver_runtime::AgentRunState,
        event: &AgentStreamRecord,
    ) -> CapabilityResult<()> {
        if matches!(event.event, AgentStreamEvent::RunComplete { .. }) {
            return Err(CapabilityError::Failed(
                "terminal observer failed".to_string(),
            ));
        }
        Ok(())
    }
}

#[tokio::test]
async fn failing_run_complete_observer_does_not_reverse_committed_success() {
    let observed = Arc::new(Mutex::new(Vec::<String>::new()));
    let bundle = StaticCapabilityBundle::new("failing-terminal-observer")
        .with_stream_observer(Arc::new(StreamObserverRecorder {
            events: observed.clone(),
        }))
        .with_stream_observer(Arc::new(FailOnRunCompleteObserver));
    let mut context = AgentContext::default();
    let mut events = Vec::new();

    let result = Agent::new(Arc::new(ScriptedModel::new(vec![ModelResponse::text(
        "committed",
    )])))
    .with_capability_bundle(&bundle)
    .run_with_context_and_stream_events("hi", &mut context, &mut events)
    .await
    .unwrap();

    assert_eq!(result.output, "committed");
    assert_eq!(
        events
            .iter()
            .filter(|record| matches!(record.event, AgentStreamEvent::RunComplete { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|record| matches!(record.event, AgentStreamEvent::RunFailed { .. }))
            .count(),
        0
    );
    assert!(
        context
            .events
            .events()
            .iter()
            .any(|event| event.kind == "run_complete")
    );
    assert!(
        !context
            .events
            .events()
            .iter()
            .any(|event| event.kind == "run_failed")
    );
    assert!(
        context
            .events
            .events()
            .iter()
            .any(|event| event.kind == "terminal_stream_observer_failed")
    );
    assert!(
        observed
            .lock()
            .unwrap()
            .iter()
            .any(|kind| kind == "run_complete")
    );
    assert!(!context.runtime.lifecycle.entered);
    assert!(context.runtime.run_toolsets_closed);
    assert!(context.ended_at.is_some());
}

struct StreamObserverRecorder {
    events: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl AgentCapability for StreamObserverRecorder {
    async fn on_stream_event(
        &self,
        _state: &starweaver_runtime::AgentRunState,
        event: &AgentStreamRecord,
    ) -> CapabilityResult<()> {
        self.events.lock().unwrap().push(
            match &event.event {
                AgentStreamEvent::RunStart { .. } => "run_start",
                AgentStreamEvent::NodeStart { .. } => "node_start",
                AgentStreamEvent::NodeComplete { .. } => "node_complete",
                AgentStreamEvent::Custom { .. } => "custom",
                AgentStreamEvent::ModelRequest { .. } => "model_request",
                AgentStreamEvent::ModelStream { .. } => "model_stream",
                AgentStreamEvent::ModelResponse { .. } => "model_response",
                AgentStreamEvent::Checkpoint { .. } => "checkpoint",
                AgentStreamEvent::Suspended { .. } => "suspended",
                AgentStreamEvent::ToolCall { .. } => "tool_call",
                AgentStreamEvent::ToolReturn { .. } => "tool_return",
                AgentStreamEvent::OutputRetry { .. } => "output_retry",
                AgentStreamEvent::SteeringGuard { .. } => "steering_guard",
                AgentStreamEvent::RunComplete { .. } => "run_complete",
                AgentStreamEvent::RunFailed { .. } => "run_failed",
                AgentStreamEvent::RunCancelled { .. } => "run_cancelled",
            }
            .to_string(),
        );
        Ok(())
    }
}
