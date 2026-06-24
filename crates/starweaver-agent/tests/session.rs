#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use starweaver_agent::{
    live_mcp_toolset, LiveMcpClient, LiveMcpError, LiveMcpServerSnapshot, McpToolSpec,
    McpTransport, RmcpLiveMcpClient,
};
use starweaver_agent::{
    AgentBuilder, AgentContext, AgentHitlError, AgentHitlResults, AgentHitlUserInteraction,
    AgentRunOptions, AgentRuntimeBuilder, AgentSession, AgentStreamDropPolicy, AgentStreamError,
    AgentStreamEvent, AgentStreamOptions, ApprovalRequiredToolset, DeferredToolResult,
    DeferredToolResults, DynToolset, FunctionModel, FunctionModelInfo, FunctionTool,
    InMemoryReplayEventLog, InMemorySessionStore, InMemoryStreamArchive, ModelConfig,
    ModelRequestParameters, ModelSettings, OutputPolicy, PerThousandRatio, ReplayEventKind,
    ReplayEventLog, ReplayScope, RunStatus, SessionRunStatus, SessionStore, StaticToolset,
    StreamArchive, ToolApprovalDecision, ToolContext, ToolError, ToolResult,
    ToolUserInputPreprocessResult, TraceContext, HITL_DECISION_DIAGNOSTIC_EVENT_KIND,
};
use starweaver_core::{AgentId, CancellationToken, Metadata};
use starweaver_model::{
    tool_call_response, ModelAdapter, ModelError, ModelMessage, ModelProfile, ModelRequest,
    ModelRequestContext, ModelRequestPart, ModelResponse, ModelResponseEventStream,
    ModelResponsePart, ModelResponseStreamEvent, PartDelta, PartEnd, PartStart, ProtocolFamily,
    ToolCallPart,
};
use starweaver_stream::{InMemoryReplayTransport, ReplayCursor, ReplayEnvelope, ReplayTransport};
use starweaver_usage::Usage;

fn reusable_text_model(text: &'static str) -> FunctionModel {
    FunctionModel::new(move |_messages, _settings, _info| {
        Ok(ModelResponse {
            usage: Usage {
                requests: 1,
                ..Usage::default()
            },
            ..ModelResponse::text(text)
        })
    })
}

fn high_volume_stream_model(text: &'static str, chunks: usize) -> FunctionModel {
    FunctionModel::streaming(move |_messages, _settings, _info| {
        let mut events = vec![ModelResponseStreamEvent::PartStart(PartStart {
            index: 0,
            part_kind: "text".to_string(),
        })];
        for index in 0..chunks {
            events.push(ModelResponseStreamEvent::PartDelta(PartDelta::text(
                0,
                format!("{index};"),
            )));
        }
        events.push(ModelResponseStreamEvent::PartEnd(PartEnd::with_kind(
            0, "text",
        )));
        events.push(ModelResponseStreamEvent::FinalResult(Box::new(
            ModelResponse {
                usage: Usage {
                    requests: 1,
                    ..Usage::default()
                },
                ..ModelResponse::text(text)
            },
        )));
        Ok(events)
    })
}

#[derive(Clone)]
struct BlockingStreamModel {
    profile: ModelProfile,
    observed_token: Arc<Mutex<Option<CancellationToken>>>,
}

impl BlockingStreamModel {
    fn new(observed_token: Arc<Mutex<Option<CancellationToken>>>) -> Self {
        Self {
            profile: ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
            observed_token,
        }
    }
}

#[async_trait]
impl ModelAdapter for BlockingStreamModel {
    fn model_name(&self) -> &'static str {
        "blocking-stream"
    }

    fn provider_name(&self) -> Option<&str> {
        Some("test")
    }

    fn profile(&self) -> &ModelProfile {
        &self.profile
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
        Err(ModelError::Transport(
            "blocking stream model only supports incremental streaming".to_string(),
        ))
    }

    async fn request_stream_incremental(
        &self,
        _messages: Vec<ModelMessage>,
        _settings: Option<ModelSettings>,
        _params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponseEventStream, ModelError> {
        let token = context.cancellation_token();
        *self.observed_token.lock().unwrap() = Some(token.clone());
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let worker_token = token.clone();
        tokio::spawn(async move {
            worker_token.cancelled().await;
            drop(sender);
        });
        Ok(ModelResponseEventStream::new_with_cancellation(
            receiver, token,
        ))
    }
}

#[derive(Clone)]
struct StreamResumeModel {
    profile: ModelProfile,
    calls: Arc<Mutex<usize>>,
}

impl StreamResumeModel {
    fn new(calls: Arc<Mutex<usize>>) -> Self {
        Self {
            profile: ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
            calls,
        }
    }
}

#[async_trait]
impl ModelAdapter for StreamResumeModel {
    fn model_name(&self) -> &'static str {
        "stream-resume"
    }

    fn provider_name(&self) -> Option<&str> {
        Some("test")
    }

    fn profile(&self) -> &ModelProfile {
        &self.profile
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
        Err(ModelError::Transport(
            "stream resume model only supports incremental streaming".to_string(),
        ))
    }

    async fn request_stream_incremental(
        &self,
        _messages: Vec<ModelMessage>,
        _settings: Option<ModelSettings>,
        _params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponseEventStream, ModelError> {
        let call_index = {
            let mut calls = self.calls.lock().unwrap();
            let call_index = *calls;
            *calls += 1;
            call_index
        };
        let (sender, receiver) = tokio::sync::mpsc::channel(4);
        if call_index == 0 {
            sender
                .send(Ok(ModelResponseStreamEvent::PartDelta(PartDelta::text(
                    0, "partial",
                ))))
                .await
                .unwrap();
            sender
                .send(Err(ModelError::Transport(
                    "server-sent event stream disconnected".to_string(),
                )))
                .await
                .unwrap();
        } else {
            sender
                .send(Ok(ModelResponseStreamEvent::FinalResult(Box::new(
                    ModelResponse::text("provider stream resumed"),
                ))))
                .await
                .unwrap();
        }
        Ok(ModelResponseEventStream::new(receiver))
    }
}

fn latest_tool_return<'a>(
    messages: &'a [ModelMessage],
    tool_name: &str,
) -> &'a starweaver_model::ToolReturnPart {
    let Some(ModelMessage::Request(request)) = messages.last() else {
        panic!("latest message should be a request");
    };
    assert!(
        !request.parts.iter().any(|part| {
            matches!(
                part,
                ModelRequestPart::UserPrompt { metadata, .. }
                    if !metadata.contains_key("starweaver_context_origin")
            )
        }),
        "HITL resume should continue with tool returns, not a new user prompt: {request:?}"
    );
    request
        .parts
        .iter()
        .find_map(|part| match part {
            ModelRequestPart::ToolReturn(tool_return) if tool_return.name == tool_name => {
                Some(tool_return)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing tool return for {tool_name}"))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[allow(dead_code)]
struct StreamAnswer {
    answer: String,
}

#[tokio::test]
async fn session_keeps_context_across_runs() {
    let app = AgentBuilder::new(Arc::new(reusable_text_model("ok"))).build_app();
    let mut session = app.session();

    let first = session.run("hello").await.unwrap();
    let second = session.run("again").await.unwrap();

    assert_eq!(first.output, "ok");
    assert_eq!(second.output, "ok");
    assert!(session.context().message_history.len() > first.messages.len());
    assert_eq!(session.context().usage.requests, 2);
}

#[tokio::test]
async fn session_exports_and_restores_state() {
    let app = AgentBuilder::new(Arc::new(reusable_text_model("ok"))).build_app();
    let mut session = app.session();
    session.run("hello").await.unwrap();
    session
        .context_mut()
        .state
        .set("preference", serde_json::json!({"language": "Chinese"}));

    let state = session.export_full_state();
    let mut restored = app.session_from_state(state);
    let result = restored.run("again").await.unwrap();

    assert_eq!(result.output, "ok");
    assert_eq!(restored.context().usage.requests, 2);
    assert_eq!(
        restored.context().state.get("preference").unwrap()["language"],
        "Chinese"
    );
}

#[tokio::test]
async fn session_resume_after_hitl_approval_executes_tool_and_continues() {
    let model_calls = Arc::new(Mutex::new(0usize));
    let model_calls_for_model = model_calls.clone();
    let model = FunctionModel::new(move |messages, _settings, _info| {
        let first_call = {
            let mut calls = model_calls_for_model.lock().unwrap();
            *calls += 1;
            *calls == 1
        };
        if first_call {
            return Ok(tool_call_response(
                "call_approve",
                "dangerous",
                serde_json::json!({"path": "target/file.txt"}),
            ));
        }

        let tool_return = latest_tool_return(&messages, "dangerous");
        assert!(!tool_return.is_error);
        assert_eq!(
            tool_return.content["executed"]["path"],
            serde_json::json!("target/file.txt")
        );
        assert_eq!(tool_return.metadata["approval_state"], "approved");
        Ok(ModelResponse::text("resumed"))
    });
    let executed = Arc::new(Mutex::new(0usize));
    let executed_for_tool = executed.clone();
    let tool = FunctionTool::new(
        "dangerous",
        Some("Dangerous operation".to_string()),
        serde_json::json!({"type": "object"}),
        move |_ctx: ToolContext, args: serde_json::Value| {
            let executed = executed_for_tool.clone();
            async move {
                *executed.lock().unwrap() += 1;
                Ok(ToolResult::new(serde_json::json!({"executed": args})))
            }
        },
    );
    let base: DynToolset =
        Arc::new(StaticToolset::new("dangerous-tools").with_tool(Arc::new(tool)));
    let app = AgentBuilder::new(Arc::new(model))
        .approval_required_tools(["dangerous"])
        .toolset(&base)
        .build_app();
    let mut session = app.session();

    let waiting = session.run("try it").await.unwrap();
    assert_eq!(waiting.state.status, RunStatus::Waiting);
    assert_eq!(session.last_run_state().unwrap().status, RunStatus::Waiting);

    let result = session
        .resume_after_hitl(
            AgentHitlResults::new().approval("call_approve", ToolApprovalDecision::approved()),
        )
        .await
        .unwrap();

    assert_eq!(result.output, "resumed");
    assert_eq!(*executed.lock().unwrap(), 1);
    assert!(session.context().pending_tool_returns.is_empty());
    assert_eq!(session.context().usage.tool_calls, 1);
    assert_eq!(*model_calls.lock().unwrap(), 2);
}

#[tokio::test]
async fn session_preprocesses_hitl_user_input_before_approved_execution() {
    let model_calls = Arc::new(Mutex::new(0usize));
    let model_calls_for_model = model_calls.clone();
    let model = FunctionModel::new(move |messages, _settings, _info| {
        let first_call = {
            let mut calls = model_calls_for_model.lock().unwrap();
            *calls += 1;
            *calls == 1
        };
        if first_call {
            return Ok(tool_call_response(
                "call_edit",
                "dangerous",
                serde_json::json!({"path": "target/unsafe.txt"}),
            ));
        }

        let tool_return = latest_tool_return(&messages, "dangerous");
        assert!(!tool_return.is_error);
        assert_eq!(tool_return.content["executed"]["path"], "target/safe.txt");
        assert_eq!(tool_return.metadata["approval_state"], "approved");
        assert_eq!(
            tool_return.metadata["approval_metadata"]["host"],
            "review-ui"
        );
        assert_eq!(
            tool_return.metadata["approval_metadata"]["source"],
            "user-edit"
        );
        Ok(ModelResponse::text("resumed"))
    });
    let executed = Arc::new(Mutex::new(0usize));
    let executed_for_tool = executed.clone();
    let tool = FunctionTool::new(
        "dangerous",
        Some("Dangerous operation".to_string()),
        serde_json::json!({"type": "object"}),
        move |_ctx: ToolContext, args: serde_json::Value| {
            let executed = executed_for_tool.clone();
            async move {
                *executed.lock().unwrap() += 1;
                Ok(ToolResult::new(serde_json::json!({"executed": args})))
            }
        },
    )
    .with_user_input_preprocessor(|context, user_input| async move {
        assert!(context
            .metadata
            .get("tool_call_id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|tool_call_id| tool_call_id.starts_with("sw-tool-")));
        assert_eq!(context.metadata["tool_name"], "dangerous");
        let mut metadata = Metadata::default();
        metadata.insert(
            "source".to_string(),
            user_input
                .get("source")
                .cloned()
                .unwrap_or_else(|| serde_json::json!("unknown")),
        );
        Ok(ToolUserInputPreprocessResult::new()
            .with_override_arguments(serde_json::json!({
                "path": user_input["path"].clone(),
            }))
            .with_metadata(metadata))
    });
    let base: DynToolset =
        Arc::new(StaticToolset::new("dangerous-tools").with_tool(Arc::new(tool)));
    let app = AgentBuilder::new(Arc::new(model))
        .approval_required_tools(["dangerous"])
        .toolset(&base)
        .build_app();
    let mut session = app.session();

    let waiting = session.run("try it").await.unwrap();
    assert_eq!(waiting.state.status, RunStatus::Waiting);
    let mut host_metadata = Metadata::default();
    host_metadata.insert("host".to_string(), serde_json::json!("review-ui"));
    let hitl_results = session
        .preprocess_hitl_user_interactions([AgentHitlUserInteraction::approved("call_edit")
            .with_user_input(serde_json::json!({
                "path": "target/safe.txt",
                "source": "user-edit",
            }))
            .with_metadata(host_metadata)])
        .await
        .unwrap();

    let result = session.resume_after_hitl(hitl_results).await.unwrap();

    assert_eq!(result.output, "resumed");
    assert_eq!(*executed.lock().unwrap(), 1);
    assert_eq!(*model_calls.lock().unwrap(), 2);
}

#[tokio::test]
async fn session_resume_after_hitl_denial_does_not_execute_tool() {
    let model = FunctionModel::new(move |messages, _settings, _info| {
        if messages
            .last()
            .is_some_and(|message| matches!(message, ModelMessage::Request(request) if request.parts.iter().any(|part| matches!(part, ModelRequestPart::ToolReturn(_)))))
        {
            let tool_return = latest_tool_return(&messages, "dangerous");
            assert!(tool_return.is_error);
            assert_eq!(tool_return.content["kind"], "approval_denied");
            assert_eq!(tool_return.content["message"], "too risky");
            assert_eq!(tool_return.metadata["approval_state"], "denied");
            Ok(ModelResponse::text("denied"))
        } else {
            Ok(tool_call_response(
                "call_deny",
                "dangerous",
                serde_json::json!({"path": "/etc/passwd"}),
            ))
        }
    });
    let executed = Arc::new(Mutex::new(0usize));
    let executed_for_tool = executed.clone();
    let tool = FunctionTool::new(
        "dangerous",
        Some("Dangerous operation".to_string()),
        serde_json::json!({"type": "object"}),
        move |_ctx: ToolContext, _args: serde_json::Value| {
            let executed = executed_for_tool.clone();
            async move {
                *executed.lock().unwrap() += 1;
                Ok(ToolResult::new(serde_json::json!({"executed": true})))
            }
        },
    );
    let base: DynToolset =
        Arc::new(StaticToolset::new("dangerous-tools").with_tool(Arc::new(tool)));
    let gated: DynToolset = Arc::new(ApprovalRequiredToolset::new(base, ["dangerous"]));
    let app = AgentBuilder::new(Arc::new(model))
        .toolset(&gated)
        .build_app();
    let mut session = app.session();

    let waiting = session.run("try it").await.unwrap();
    assert_eq!(waiting.state.status, RunStatus::Waiting);

    let result = session
        .resume_after_hitl(
            AgentHitlResults::new()
                .approval("call_deny", ToolApprovalDecision::denied("too risky")),
        )
        .await
        .unwrap();

    assert_eq!(result.output, "denied");
    assert_eq!(*executed.lock().unwrap(), 0);
    assert_eq!(session.context().usage.tool_calls, 0);
}

#[tokio::test]
async fn session_resume_after_deferred_complete_injects_worker_result() {
    let model = FunctionModel::new(move |messages, _settings, _info| {
        if messages
            .last()
            .is_some_and(|message| matches!(message, ModelMessage::Request(request) if request.parts.iter().any(|part| matches!(part, ModelRequestPart::ToolReturn(_)))))
        {
            let tool_return = latest_tool_return(&messages, "slow");
            assert!(!tool_return.is_error);
            assert_eq!(tool_return.content["answer"], "ready");
            assert_eq!(tool_return.metadata["deferred_status"], "completed");
            Ok(ModelResponse::text("deferred done"))
        } else {
            Ok(tool_call_response(
                "call_deferred",
                "slow",
                serde_json::json!({"job": "render"}),
            ))
        }
    });
    let tool = FunctionTool::new(
        "slow",
        Some("Slow operation".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            Err(ToolError::CallDeferred {
                tool: "slow".to_string(),
                metadata: serde_json::json!({"queue": "worker"}),
            })
        },
    );
    let app = AgentBuilder::new(Arc::new(model))
        .tool(Arc::new(tool))
        .build_app();
    let mut session = app.session();

    let waiting = session.run("start slow").await.unwrap();
    assert_eq!(waiting.state.status, RunStatus::Waiting);
    let deferred_id = format!("deferred_{}_call_deferred", waiting.state.run_id.as_str());

    let result =
        session
            .resume_after_hitl(AgentHitlResults::new().deferred_result(
                DeferredToolResult::completed(deferred_id, serde_json::json!({"answer": "ready"})),
            ))
            .await
            .unwrap();

    assert_eq!(result.output, "deferred done");
}

#[tokio::test]
async fn session_resume_after_deferred_fail_injects_error_result() {
    let model = FunctionModel::new(move |messages, _settings, _info| {
        if messages
            .last()
            .is_some_and(|message| matches!(message, ModelMessage::Request(request) if request.parts.iter().any(|part| matches!(part, ModelRequestPart::ToolReturn(_)))))
        {
            let tool_return = latest_tool_return(&messages, "slow");
            assert!(tool_return.is_error);
            assert_eq!(tool_return.content["kind"], "deferred_failed");
            assert_eq!(tool_return.content["response"]["error"], "worker failed");
            assert_eq!(tool_return.metadata["deferred_status"], "failed");
            Ok(ModelResponse::text("deferred failed"))
        } else {
            Ok(tool_call_response(
                "call_deferred_fail",
                "slow",
                serde_json::json!({"job": "render"}),
            ))
        }
    });
    let tool = FunctionTool::new(
        "slow",
        Some("Slow operation".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            Err(ToolError::CallDeferred {
                tool: "slow".to_string(),
                metadata: serde_json::json!({"queue": "worker"}),
            })
        },
    );
    let app = AgentBuilder::new(Arc::new(model))
        .tool(Arc::new(tool))
        .build_app();
    let mut session = app.session();

    let waiting = session.run("start slow").await.unwrap();
    assert_eq!(waiting.state.status, RunStatus::Waiting);
    let deferred_id = format!(
        "deferred_{}_call_deferred_fail",
        waiting.state.run_id.as_str()
    );

    let result = session
        .resume_after_hitl(
            AgentHitlResults::new().deferred_result(DeferredToolResult::failed(
                deferred_id,
                serde_json::json!({"error": "worker failed"}),
            )),
        )
        .await
        .unwrap();

    assert_eq!(result.output, "deferred failed");
}

#[tokio::test]
#[allow(clippy::too_many_lines, clippy::large_futures)]
async fn runtime_durable_store_resumes_hitl_and_replays_streams_by_id() {
    let model_calls = Arc::new(Mutex::new(0usize));
    let model_calls_for_model = model_calls.clone();
    let model = Arc::new(FunctionModel::new(move |messages, _settings, _info| {
        let first_call = {
            let mut calls = model_calls_for_model.lock().unwrap();
            *calls += 1;
            *calls == 1
        };
        if first_call {
            return Ok(ModelResponse {
                parts: vec![
                    ModelResponsePart::ToolCall(ToolCallPart {
                        id: "call_danger".to_string(),
                        name: "dangerous".to_string(),
                        arguments: serde_json::json!({"path": "target/durable.txt"}).into(),
                    }),
                    ModelResponsePart::ToolCall(ToolCallPart {
                        id: "call_slow".to_string(),
                        name: "slow".to_string(),
                        arguments: serde_json::json!({"job": "render"}).into(),
                    }),
                ],
                ..ModelResponse::text("")
            });
        }

        let dangerous = latest_tool_return(&messages, "dangerous");
        assert!(!dangerous.is_error);
        assert_eq!(
            dangerous.content["executed"]["path"],
            serde_json::json!("target/durable.txt")
        );
        assert_eq!(dangerous.metadata["approval_state"], "approved");
        let slow = latest_tool_return(&messages, "slow");
        assert!(!slow.is_error);
        assert_eq!(slow.content["answer"], "ready");
        assert_eq!(slow.metadata["deferred_status"], "completed");
        Ok(ModelResponse::text("durable resumed"))
    }));
    let dangerous = FunctionTool::new(
        "dangerous",
        Some("Dangerous operation".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move {
            Ok(ToolResult::new(serde_json::json!({"executed": args})))
        },
    );
    let slow = FunctionTool::new(
        "slow",
        Some("Slow operation".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            Err(ToolError::CallDeferred {
                tool: "slow".to_string(),
                metadata: serde_json::json!({"queue": "worker"}),
            })
        },
    );
    let base: DynToolset = Arc::new(
        StaticToolset::new("durable-tools")
            .with_tool(Arc::new(dangerous))
            .with_tool(Arc::new(slow)),
    );
    let store = Arc::new(InMemorySessionStore::new());
    let archive = Arc::new(InMemoryStreamArchive::new());
    let replay = Arc::new(InMemoryReplayEventLog::new());
    let session_id = starweaver_agent::SessionId::from_string("session-durable-hitl");
    let mut runtime = AgentRuntimeBuilder::new(model.clone())
        .durable_session_id(session_id.clone())
        .session_store(store.clone())
        .stream_archive(archive.clone())
        .replay_event_log(replay.clone())
        .approval_required_tools(["dangerous"])
        .toolset(&base)
        .build();

    let waiting = runtime.run("try durable HITL").await.unwrap();
    assert_eq!(waiting.state.status, RunStatus::Waiting);
    let waiting_run_id = waiting.state.run_id.clone();
    let approval_id = waiting.state.pending_approval_tool_returns[0]
        .tool_call_id
        .clone();
    let deferred_tool_call_id = waiting.state.deferred_tool_returns[0].tool_call_id.clone();
    let waiting_run = store.load_run(&session_id, &waiting_run_id).await.unwrap();
    assert_eq!(waiting_run.status, SessionRunStatus::Waiting);
    assert_eq!(waiting_run.input.len(), 1);
    let approvals = store
        .load_approvals(&session_id, &waiting_run_id)
        .await
        .unwrap();
    assert_eq!(approvals.len(), 1);
    assert_eq!(
        approvals[0].status,
        starweaver_agent::ApprovalStatus::Pending
    );
    let deferred = store
        .load_deferred_tools(&session_id, &waiting_run_id)
        .await
        .unwrap();
    assert_eq!(deferred.len(), 1);
    assert_eq!(
        deferred[0].status,
        starweaver_agent::ExecutionStatus::Waiting
    );
    assert!(!store
        .load_checkpoints(&session_id, &waiting_run_id)
        .await
        .unwrap()
        .is_empty());
    assert!(!store
        .replay_stream_records(&session_id, &waiting_run_id)
        .await
        .unwrap()
        .is_empty());
    assert!(!archive
        .replay_raw_after(&session_id, &waiting_run_id, None)
        .await
        .unwrap()
        .is_empty());
    let waiting_replay_events = replay
        .replay_after(&ReplayScope::run(waiting_run_id.as_str()), None, None)
        .await
        .unwrap();
    assert!(waiting_replay_events
        .iter()
        .any(|event| matches!(event.event, ReplayEventKind::DisplayMessage(_))));

    let deferred_id = format!(
        "deferred_{}_{}",
        waiting_run_id.as_str(),
        deferred_tool_call_id
    );
    let mut restored_runtime = AgentRuntimeBuilder::new(model.clone())
        .durable_session_id(session_id.clone())
        .session_store(store.clone())
        .stream_archive(archive.clone())
        .replay_event_log(replay.clone())
        .approval_required_tools(["dangerous"])
        .toolset(&base)
        .build();
    let resumed = restored_runtime
        .resume_after_hitl_by_id(
            &session_id,
            &waiting_run_id,
            AgentHitlResults::new()
                .approval(approval_id, ToolApprovalDecision::approved())
                .deferred_result(DeferredToolResult::completed(
                    deferred_id.clone(),
                    serde_json::json!({"answer": "ready"}),
                )),
        )
        .await
        .unwrap();

    assert_eq!(resumed.output, "durable resumed");
    assert_eq!(*model_calls.lock().unwrap(), 2);
    let approvals = store
        .load_approvals(&session_id, &waiting_run_id)
        .await
        .unwrap();
    assert_eq!(approvals.len(), 1);
    assert_eq!(
        approvals[0].status,
        starweaver_agent::ApprovalStatus::Approved
    );
    assert!(approvals[0].decision.is_some());
    let deferred = store
        .load_deferred_tools(&session_id, &waiting_run_id)
        .await
        .unwrap();
    assert_eq!(deferred.len(), 1);
    assert_eq!(
        deferred[0].status,
        starweaver_agent::ExecutionStatus::Completed
    );
    assert_eq!(deferred[0].response["answer"], "ready");
    let resolved_waiting_run = store.load_run(&session_id, &waiting_run_id).await.unwrap();
    assert_eq!(resolved_waiting_run.status, SessionRunStatus::Completed);
    let resumed_run = store
        .load_run(&session_id, &resumed.state.run_id)
        .await
        .unwrap();
    assert_eq!(resumed_run.status, SessionRunStatus::Completed);
    assert_eq!(
        resumed_run.restore_from_run_id,
        Some(waiting_run_id.clone())
    );
    let resumed_events = replay
        .replay_after(&ReplayScope::run(resumed.state.run_id.as_str()), None, None)
        .await
        .unwrap();
    assert!(resumed_events.iter().any(|event| {
        matches!(
            event.event,
            ReplayEventKind::Terminal {
                marker: starweaver_agent::StreamTerminalMarker::RunCompleted
            }
        )
    }));
}

#[tokio::test]
#[allow(clippy::too_many_lines, clippy::large_futures)]
async fn runtime_durable_store_resumes_live_mcp_approval_and_deferred_records() {
    struct HitlMcp {
        executed: Arc<Mutex<Vec<serde_json::Value>>>,
    }

    #[async_trait]
    impl LiveMcpClient for HitlMcp {
        async fn discover(
            &self,
            id: &str,
            _transport: &McpTransport,
        ) -> Result<LiveMcpServerSnapshot, LiveMcpError> {
            Ok(LiveMcpServerSnapshot::new(id)
                .with_tool(McpToolSpec::new(
                    "dangerous",
                    serde_json::json!({"type": "object"}),
                ))
                .with_tool(McpToolSpec::new(
                    "slow",
                    serde_json::json!({"type": "object"}),
                )))
        }

        async fn call_tool(
            &self,
            context: ToolContext,
            id: &str,
            transport: &McpTransport,
            tool_name: &str,
            arguments: serde_json::Value,
        ) -> Result<ToolResult, LiveMcpError> {
            if tool_name != "dangerous" {
                return Err(LiveMcpError::ToolCallUnsupported {
                    server_id: id.to_string(),
                    tool_name: tool_name.to_string(),
                });
            }
            self.executed.lock().unwrap().push(serde_json::json!({
                "run_step": context.run_step,
                "server_id": id,
                "transport": transport.kind(),
                "tool_name": tool_name,
                "arguments": arguments,
            }));
            Ok(ToolResult::new(serde_json::json!({
                "executed": true
            })))
        }
    }

    let model_calls = Arc::new(Mutex::new(0usize));
    let model_calls_for_model = model_calls.clone();
    let model = Arc::new(FunctionModel::new(move |messages, _settings, _info| {
        let first_call = {
            let mut calls = model_calls_for_model.lock().unwrap();
            *calls += 1;
            *calls == 1
        };
        if first_call {
            return Ok(ModelResponse {
                parts: vec![
                    ModelResponsePart::ToolCall(ToolCallPart {
                        id: "call_mcp_danger".to_string(),
                        name: "dangerous".to_string(),
                        arguments: serde_json::json!({"path": "target/mcp.txt"}).into(),
                    }),
                    ModelResponsePart::ToolCall(ToolCallPart {
                        id: "call_mcp_slow".to_string(),
                        name: "slow".to_string(),
                        arguments: serde_json::json!({"job": "render"}).into(),
                    }),
                ],
                ..ModelResponse::text("")
            });
        }

        let dangerous = latest_tool_return(&messages, "dangerous");
        assert!(!dangerous.is_error);
        assert_eq!(dangerous.content["executed"], true);
        assert_eq!(dangerous.metadata["approval_state"], "approved");
        assert_eq!(dangerous.metadata["mcp_server_id"], "live");
        assert_eq!(dangerous.metadata["mcp_transport"], "stdio");
        assert_eq!(dangerous.metadata["mcp_tool_name"], "dangerous");

        let slow = latest_tool_return(&messages, "slow");
        assert!(!slow.is_error);
        assert_eq!(slow.content["answer"], "ready");
        assert_eq!(slow.metadata["deferred_status"], "completed");
        Ok(ModelResponse::text("live mcp HITL resumed"))
    }));
    let executed = Arc::new(Mutex::new(Vec::new()));
    let live_toolset = live_mcp_toolset(
        Arc::new(HitlMcp {
            executed: executed.clone(),
        }),
        "live",
        McpTransport::stdio("fake-mcp"),
    )
    .await
    .unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let session_id = starweaver_agent::SessionId::from_string("session-live-mcp-hitl");
    let mut runtime = AgentRuntimeBuilder::new(model.clone())
        .durable_session_id(session_id.clone())
        .session_store(store.clone())
        .approval_required_tools(["dangerous"])
        .toolset(&live_toolset)
        .build();

    let waiting = runtime.run("try MCP HITL").await.unwrap();
    assert_eq!(waiting.state.status, RunStatus::Waiting);
    let waiting_run_id = waiting.state.run_id.clone();
    let approval_id = waiting.state.pending_approval_tool_returns[0]
        .tool_call_id
        .clone();
    let deferred_tool_call_id = waiting.state.deferred_tool_returns[0].tool_call_id.clone();
    let approvals = store
        .load_approvals(&session_id, &waiting_run_id)
        .await
        .unwrap();
    assert_eq!(approvals.len(), 1);
    assert_eq!(
        approvals[0].request["tool_metadata"]["mcp_server_id"],
        "live"
    );
    assert_eq!(
        approvals[0].request["tool_metadata"]["mcp_tool_name"],
        "dangerous"
    );
    let deferred = store
        .load_deferred_tools(&session_id, &waiting_run_id)
        .await
        .unwrap();
    assert_eq!(deferred.len(), 1);
    assert_eq!(deferred[0].request["kind"], "mcp_tool_call");
    assert_eq!(deferred[0].request["server_id"], "live");
    assert_eq!(deferred[0].request["tool_name"], "slow");
    assert_eq!(deferred[0].request["arguments"]["job"], "render");

    let deferred_id = format!(
        "deferred_{}_{}",
        waiting_run_id.as_str(),
        deferred_tool_call_id
    );
    let mut restored_runtime = AgentRuntimeBuilder::new(model.clone())
        .durable_session_id(session_id.clone())
        .session_store(store.clone())
        .approval_required_tools(["dangerous"])
        .toolset(&live_toolset)
        .build();
    let resumed = restored_runtime
        .resume_after_hitl_by_id(
            &session_id,
            &waiting_run_id,
            AgentHitlResults::new()
                .approval(approval_id, ToolApprovalDecision::approved())
                .deferred_result(DeferredToolResult::completed(
                    deferred_id,
                    serde_json::json!({"answer": "ready"}),
                )),
        )
        .await
        .unwrap();

    assert_eq!(resumed.output, "live mcp HITL resumed");
    assert_eq!(
        *executed.lock().unwrap(),
        vec![serde_json::json!({
            "run_step": 1,
            "server_id": "live",
            "transport": "stdio",
            "tool_name": "dangerous",
            "arguments": {"path": "target/mcp.txt"},
        })]
    );
    let approvals = store
        .load_approvals(&session_id, &waiting_run_id)
        .await
        .unwrap();
    assert_eq!(
        approvals[0].status,
        starweaver_agent::ApprovalStatus::Approved
    );
    let deferred = store
        .load_deferred_tools(&session_id, &waiting_run_id)
        .await
        .unwrap();
    assert_eq!(
        deferred[0].status,
        starweaver_agent::ExecutionStatus::Completed
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines, clippy::large_futures)]
async fn runtime_durable_store_resumes_rmcp_stdio_approval_and_deferred_records() {
    async fn rmcp_fixture_toolset(id: &str) -> DynToolset {
        live_mcp_toolset(
            Arc::new(RmcpLiveMcpClient::new()),
            id,
            McpTransport::stdio(env!("CARGO_BIN_EXE_starweaver_agent_rmcp_stdio_fixture")),
        )
        .await
        .unwrap()
    }

    let model_calls = Arc::new(Mutex::new(0usize));
    let model_calls_for_model = model_calls.clone();
    let model = Arc::new(FunctionModel::new(move |messages, _settings, _info| {
        let first_call = {
            let mut calls = model_calls_for_model.lock().unwrap();
            *calls += 1;
            *calls == 1
        };
        if first_call {
            return Ok(ModelResponse {
                parts: vec![
                    ModelResponsePart::ToolCall(ToolCallPart {
                        id: "call_rmcp_danger".to_string(),
                        name: "dangerous".to_string(),
                        arguments: serde_json::json!({"path": "target/rmcp.txt"}).into(),
                    }),
                    ModelResponsePart::ToolCall(ToolCallPart {
                        id: "call_rmcp_slow".to_string(),
                        name: "slow".to_string(),
                        arguments: serde_json::json!({"job": "render"}).into(),
                    }),
                ],
                ..ModelResponse::text("")
            });
        }

        let dangerous = latest_tool_return(&messages, "dangerous");
        assert!(!dangerous.is_error);
        assert_eq!(dangerous.content["executed"], true);
        assert_eq!(dangerous.content["path"], "target/rmcp.txt");
        assert_eq!(dangerous.metadata["approval_state"], "approved");
        assert_eq!(dangerous.metadata["mcp_server_id"], "rmcp-live");
        assert_eq!(dangerous.metadata["mcp_transport"], "stdio");
        assert_eq!(dangerous.metadata["mcp_tool_name"], "dangerous");
        assert_eq!(dangerous.metadata["rmcp_live"], true);

        let slow = latest_tool_return(&messages, "slow");
        assert!(!slow.is_error);
        assert_eq!(slow.content["answer"], "ready");
        assert_eq!(slow.metadata["deferred_status"], "completed");
        Ok(ModelResponse::text("rmcp HITL resumed"))
    }));
    let store = Arc::new(InMemorySessionStore::new());
    let session_id = starweaver_agent::SessionId::from_string("session-rmcp-hitl");
    let waiting_toolset = rmcp_fixture_toolset("rmcp-live").await;
    let mut runtime = AgentRuntimeBuilder::new(model.clone())
        .durable_session_id(session_id.clone())
        .session_store(store.clone())
        .approval_required_tools(["dangerous"])
        .toolset(&waiting_toolset)
        .build();

    let waiting = runtime.run("try rmcp HITL").await.unwrap();
    assert_eq!(waiting.state.status, RunStatus::Waiting);
    let waiting_run_id = waiting.state.run_id.clone();
    let approval_id = waiting.state.pending_approval_tool_returns[0]
        .tool_call_id
        .clone();
    let deferred_tool_call_id = waiting.state.deferred_tool_returns[0].tool_call_id.clone();
    let approvals = store
        .load_approvals(&session_id, &waiting_run_id)
        .await
        .unwrap();
    assert_eq!(approvals.len(), 1);
    assert_eq!(
        approvals[0].request["tool_metadata"]["mcp_server_id"],
        "rmcp-live"
    );
    assert_eq!(
        approvals[0].request["tool_metadata"]["mcp_tool_name"],
        "dangerous"
    );
    let deferred = store
        .load_deferred_tools(&session_id, &waiting_run_id)
        .await
        .unwrap();
    assert_eq!(deferred.len(), 1);
    assert_eq!(deferred[0].request["kind"], "mcp_tool_call");
    assert_eq!(deferred[0].request["server_id"], "rmcp-live");
    assert_eq!(deferred[0].request["tool_name"], "slow");
    assert_eq!(deferred[0].request["arguments"]["job"], "render");
    assert_eq!(deferred[0].request["task"], true);

    let deferred_id = format!(
        "deferred_{}_{}",
        waiting_run_id.as_str(),
        deferred_tool_call_id
    );
    let resumed_toolset = rmcp_fixture_toolset("rmcp-live").await;
    let mut restored_runtime = AgentRuntimeBuilder::new(model.clone())
        .durable_session_id(session_id.clone())
        .session_store(store.clone())
        .approval_required_tools(["dangerous"])
        .toolset(&resumed_toolset)
        .build();
    let resumed = restored_runtime
        .resume_after_hitl_by_id(
            &session_id,
            &waiting_run_id,
            AgentHitlResults::new()
                .approval(approval_id, ToolApprovalDecision::approved())
                .deferred_result(DeferredToolResult::completed(
                    deferred_id,
                    serde_json::json!({"answer": "ready"}),
                )),
        )
        .await
        .unwrap();

    assert_eq!(resumed.output, "rmcp HITL resumed");
    assert_eq!(*model_calls.lock().unwrap(), 2);
    let approvals = store
        .load_approvals(&session_id, &waiting_run_id)
        .await
        .unwrap();
    assert_eq!(
        approvals[0].status,
        starweaver_agent::ApprovalStatus::Approved
    );
    let deferred = store
        .load_deferred_tools(&session_id, &waiting_run_id)
        .await
        .unwrap();
    assert_eq!(
        deferred[0].status,
        starweaver_agent::ExecutionStatus::Completed
    );
}

#[tokio::test]
async fn runtime_finish_stream_persists_live_stream_records() {
    let store = Arc::new(InMemorySessionStore::new());
    let archive = Arc::new(InMemoryStreamArchive::new());
    let replay = Arc::new(InMemoryReplayEventLog::new());
    let session_id = starweaver_agent::SessionId::from_string("session-durable-live-stream");
    let mut runtime =
        AgentRuntimeBuilder::new(Arc::new(high_volume_stream_model("live durable", 8)))
            .durable_session_id(session_id.clone())
            .session_store(store.clone())
            .stream_archive(archive.clone())
            .replay_event_log(replay.clone())
            .build();

    let mut handle = runtime.stream_with_stream_options(
        "hello live",
        AgentStreamOptions::new().drop_policy(AgentStreamDropPolicy::Backpressure),
    );
    let mut observed = Vec::new();
    while let Some(record) = handle.recv().await {
        observed.push(record);
    }
    assert!(!observed.is_empty());

    let result = runtime.finish_stream("hello live", handle).await.unwrap();

    assert_eq!(result.result.output, "live durable");
    let run_id = result.result.state.run_id.clone();
    let run = store.load_run(&session_id, &run_id).await.unwrap();
    assert_eq!(run.status, SessionRunStatus::Completed);
    assert_eq!(run.input.len(), 1);
    let stored_records = store
        .replay_stream_records(&session_id, &run_id)
        .await
        .unwrap();
    assert_eq!(stored_records, result.events);
    assert_eq!(stored_records, observed);
    assert_eq!(
        archive
            .replay_raw_after(&session_id, &run_id, None)
            .await
            .unwrap(),
        stored_records
    );
    let replay_events = replay
        .replay_after(&ReplayScope::run(run_id.as_str()), None, None)
        .await
        .unwrap();
    assert!(replay_events.iter().any(|event| {
        matches!(
            event.event,
            ReplayEventKind::Terminal {
                marker: starweaver_agent::StreamTerminalMarker::RunCompleted
            }
        )
    }));
}

#[tokio::test]
async fn runtime_finish_stream_persists_interrupted_live_stream_recovery() {
    let store = Arc::new(InMemorySessionStore::new());
    let archive = Arc::new(InMemoryStreamArchive::new());
    let replay = Arc::new(InMemoryReplayEventLog::new());
    let session_id =
        starweaver_agent::SessionId::from_string("session-durable-interrupted-live-stream");
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(high_volume_stream_model(
        "interrupted durable",
        32,
    )))
    .durable_session_id(session_id.clone())
    .session_store(store.clone())
    .stream_archive(archive.clone())
    .replay_event_log(replay.clone())
    .build();

    let mut handle = runtime.stream_with_stream_options(
        "hello interrupted live",
        AgentStreamOptions::new()
            .buffer_size(1)
            .drop_policy(AgentStreamDropPolicy::Backpressure),
    );
    let first = handle.recv().await.unwrap();
    assert!(matches!(first.event, AgentStreamEvent::RunStart { .. }));

    handle.interrupt();
    while handle.recv().await.is_some() {}

    let error = runtime
        .finish_stream("hello interrupted live", handle)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        starweaver_agent::AgentDurabilityError::Stream(AgentStreamError::Interrupted)
    ));

    let run_id = runtime.export_full_state().run_id.unwrap();
    let run = store.load_run(&session_id, &run_id).await.unwrap();
    assert_eq!(run.status, SessionRunStatus::Cancelled);
    assert_eq!(run.input.len(), 1);
    assert_eq!(
        run.metadata["live_stream_error"],
        "agent stream interrupted"
    );

    let stored_records = store
        .replay_stream_records(&session_id, &run_id)
        .await
        .unwrap();
    assert!(!stored_records.is_empty());
    assert_eq!(
        archive
            .replay_raw_after(&session_id, &run_id, None)
            .await
            .unwrap(),
        stored_records
    );

    let replay_events = replay
        .replay_after(&ReplayScope::run(run_id.as_str()), None, None)
        .await
        .unwrap();
    assert!(replay_events.iter().any(|event| {
        matches!(
            &event.event,
            ReplayEventKind::Terminal {
                marker: starweaver_agent::StreamTerminalMarker::RunCancelled { reason }
            } if reason == "agent run cancelled"
        )
    }));

    let snapshot = runtime.resume_snapshot(&session_id, &run_id).await.unwrap();
    assert_eq!(snapshot.run.status, SessionRunStatus::Cancelled);
    assert_eq!(snapshot.stream_records, stored_records);
}

#[tokio::test]
async fn runtime_durable_store_persists_provider_stream_resume_replay() {
    let store = Arc::new(InMemorySessionStore::new());
    let archive = Arc::new(InMemoryStreamArchive::new());
    let replay = Arc::new(InMemoryReplayEventLog::new());
    let session_id = starweaver_agent::SessionId::from_string("session-provider-stream-resume");
    let calls = Arc::new(Mutex::new(0usize));
    let model = Arc::new(StreamResumeModel::new(calls.clone()));
    let mut runtime = AgentRuntimeBuilder::new(model)
        .durable_session_id(session_id.clone())
        .session_store(store.clone())
        .stream_archive(archive.clone())
        .replay_event_log(replay.clone())
        .build();

    let stream = runtime.run_stream("resume provider stream").await.unwrap();

    assert_eq!(stream.result.output, "provider stream resumed");
    assert_eq!(*calls.lock().unwrap(), 2);
    assert!(stream.events.iter().any(|record| matches!(
        &record.event,
        AgentStreamEvent::Custom { event } if event.kind == "model_stream_resume"
    )));
    let run_id = stream.result.state.run_id.clone();
    let run = store.load_run(&session_id, &run_id).await.unwrap();
    assert_eq!(run.status, SessionRunStatus::Completed);
    let stored_records = store
        .replay_stream_records(&session_id, &run_id)
        .await
        .unwrap();
    assert_eq!(stored_records, stream.events);
    assert_eq!(
        archive
            .replay_raw_after(&session_id, &run_id, None)
            .await
            .unwrap(),
        stored_records
    );
    let replay_events = replay
        .replay_after(&ReplayScope::run(run_id.as_str()), None, None)
        .await
        .unwrap();
    assert!(replay_events.iter().any(|event| {
        matches!(
            event.event,
            ReplayEventKind::Terminal {
                marker: starweaver_agent::StreamTerminalMarker::RunCompleted
            }
        )
    }));
    let scope = ReplayScope::run(run_id.as_str());
    let transport = InMemoryReplayTransport::sse((*replay).clone());
    let all_frames = transport.replay(scope.clone(), None).await.unwrap();
    let tail_frames = transport
        .replay(scope.clone(), Some(ReplayCursor::new(scope, 0)))
        .await
        .unwrap();
    assert_eq!(tail_frames.len(), all_frames.len().saturating_sub(1));
    assert!(matches!(
        tail_frames.last(),
        Some(ReplayEnvelope::Sse(envelope)) if envelope.event == "terminal"
    ));
    let snapshot = runtime.resume_snapshot(&session_id, &run_id).await.unwrap();
    assert_eq!(snapshot.run.status, SessionRunStatus::Completed);
    assert!(snapshot.stream_records.len() < stored_records.len());
    assert!(snapshot
        .stream_records
        .iter()
        .any(|record| { matches!(record.event, AgentStreamEvent::RunComplete { .. }) }));
}

#[tokio::test]
async fn session_can_restore_after_injecting_hitl_tool_returns() {
    let model_calls = Arc::new(Mutex::new(0usize));
    let model_calls_for_model = model_calls.clone();
    let model = FunctionModel::new(move |messages, _settings, _info| {
        let first_call = {
            let mut calls = model_calls_for_model.lock().unwrap();
            *calls += 1;
            *calls == 1
        };
        if first_call {
            return Ok(tool_call_response(
                "call_restore",
                "dangerous",
                serde_json::json!({"path": "target/restored.txt"}),
            ));
        }
        let tool_return = latest_tool_return(&messages, "dangerous");
        assert_eq!(
            tool_return.content["executed"]["path"],
            "target/restored.txt"
        );
        Ok(ModelResponse::text("restored"))
    });
    let tool = FunctionTool::new(
        "dangerous",
        Some("Dangerous operation".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move {
            Ok(ToolResult::new(serde_json::json!({"executed": args})))
        },
    );
    let base: DynToolset =
        Arc::new(StaticToolset::new("dangerous-tools").with_tool(Arc::new(tool)));
    let gated: DynToolset = Arc::new(ApprovalRequiredToolset::new(base, ["dangerous"]));
    let app = AgentBuilder::new(Arc::new(model))
        .toolset(&gated)
        .build_app();
    let mut session = app.session();

    let waiting = session.run("try it").await.unwrap();
    assert_eq!(waiting.state.status, RunStatus::Waiting);
    let resolved = session
        .inject_hitl_results(
            AgentHitlResults::new().approval("call_restore", ToolApprovalDecision::approved()),
        )
        .await
        .unwrap();
    assert_eq!(resolved.tool_returns.len(), 1);
    let state = session.export_full_state();
    assert_eq!(state.pending_tool_returns.len(), 1);

    let mut restored = app.session_from_state(state);
    let result = restored
        .resume_after_hitl(AgentHitlResults::new())
        .await
        .unwrap();

    assert_eq!(result.output, "restored");
    assert_eq!(*model_calls.lock().unwrap(), 2);
}

#[tokio::test]
async fn session_rejects_duplicate_deferred_results() {
    let model = Arc::new(FunctionModel::new(move |_messages, _settings, _info| {
        Ok(tool_call_response(
            "call_duplicate",
            "slow",
            serde_json::json!({}),
        ))
    }));
    let tool = FunctionTool::new(
        "slow",
        Some("Slow operation".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            Err(ToolError::CallDeferred {
                tool: "slow".to_string(),
                metadata: serde_json::json!({"queue": "worker"}),
            })
        },
    );
    let app = AgentBuilder::new(model).tool(Arc::new(tool)).build_app();
    let mut session = app.session();

    let waiting = session.run("start slow").await.unwrap();
    assert_eq!(waiting.state.status, RunStatus::Waiting);
    let deferred_id = format!("deferred_{}_call_duplicate", waiting.state.run_id.as_str());
    let canonical_deferred_id = format!(
        "deferred_{}_{}",
        waiting.state.run_id.as_str(),
        waiting.state.deferred_tool_returns[0].tool_call_id
    );

    let error = session
        .inject_hitl_results(
            AgentHitlResults::new().deferred_results(DeferredToolResults::new([
                DeferredToolResult::completed(
                    deferred_id.clone(),
                    serde_json::json!({"answer": "one"}),
                ),
                DeferredToolResult::completed(
                    deferred_id.clone(),
                    serde_json::json!({"answer": "two"}),
                ),
            ])),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        AgentHitlError::DuplicateDecision(id) if id == canonical_deferred_id
    ));

    let Some(diagnostic) = session
        .context()
        .events
        .events()
        .iter()
        .rev()
        .find(|event| event.kind == HITL_DECISION_DIAGNOSTIC_EVENT_KIND)
    else {
        panic!("duplicate decision should publish a HITL diagnostic event");
    };
    assert_eq!(diagnostic.payload["error_kind"], "duplicate_decision");
    assert_eq!(diagnostic.payload["decision_id"], canonical_deferred_id);
}

#[tokio::test]
async fn session_reports_unknown_approval_diagnostic() {
    let model = FunctionModel::new(move |_messages, _settings, _info| {
        Ok(tool_call_response(
            "call_unknown",
            "dangerous",
            serde_json::json!({"path": "target/file.txt"}),
        ))
    });
    let tool = FunctionTool::new(
        "dangerous",
        Some("Dangerous operation".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move {
            Ok(ToolResult::new(serde_json::json!({"executed": args})))
        },
    );
    let base: DynToolset =
        Arc::new(StaticToolset::new("dangerous-tools").with_tool(Arc::new(tool)));
    let gated: DynToolset = Arc::new(ApprovalRequiredToolset::new(base, ["dangerous"]));
    let app = AgentBuilder::new(Arc::new(model))
        .toolset(&gated)
        .build_app();
    let mut session = app.session();

    let waiting = session.run("try it").await.unwrap();
    assert_eq!(waiting.state.status, RunStatus::Waiting);

    let error = session
        .inject_hitl_results(
            AgentHitlResults::new().approval("not_pending", ToolApprovalDecision::approved()),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        AgentHitlError::UnknownApproval(id) if id == "not_pending"
    ));
    let Some(diagnostic) = session
        .context()
        .events
        .events()
        .iter()
        .rev()
        .find(|event| event.kind == HITL_DECISION_DIAGNOSTIC_EVENT_KIND)
    else {
        panic!("unknown approval should publish a HITL diagnostic event");
    };
    assert_eq!(diagnostic.payload["error_kind"], "unknown_approval");
    assert_eq!(diagnostic.payload["approval_id"], "not_pending");
}

#[test]
fn hitl_results_try_insert_approval_rejects_duplicates() {
    let mut results = AgentHitlResults::new();
    results
        .try_insert_approval("call", ToolApprovalDecision::approved())
        .unwrap();

    let error = results
        .try_insert_approval("call", ToolApprovalDecision::denied("duplicate"))
        .unwrap_err();

    assert!(matches!(
        error,
        AgentHitlError::DuplicateDecision(id) if id == "call"
    ));
}

#[tokio::test]
async fn session_accepts_caller_provided_context() {
    let app = AgentBuilder::new(Arc::new(reusable_text_model("ok"))).build_app();
    let mut context = AgentContext::new(AgentId::from_string("agent-session"));
    context
        .state
        .set("workspace", serde_json::json!({"root": "/repo"}));

    let mut session = app.session_with_context(context);
    let result = session.run("hello").await.unwrap();

    assert_eq!(result.output, "ok");
    assert_eq!(session.context().agent_id.as_str(), "agent-session");
    assert_eq!(
        session.context().state.get("workspace").unwrap()["root"],
        "/repo"
    );
}

#[tokio::test]
async fn builder_agent_identity_configures_default_session_context() {
    let app = AgentBuilder::new(Arc::new(reusable_text_model("ok")))
        .agent_identity("assistant-main", "Assistant Main")
        .build_app();

    let session = app.session();

    assert_eq!(session.context().agent_id.as_str(), "assistant-main");
    assert_eq!(
        session.context().agent_registry["assistant-main"].agent_name,
        "Assistant Main"
    );
    assert_eq!(session.context().metadata["agent_name"], "Assistant Main");
}

#[tokio::test]
async fn builder_agent_identity_configures_direct_run_context() {
    let agent = AgentBuilder::new(Arc::new(reusable_text_model("ok")))
        .agent_identity("direct-agent", "Direct Agent")
        .build();
    let context = agent.new_context();

    assert_eq!(
        context.agent_registry[context.agent_id.as_str()].agent_name,
        "Direct Agent"
    );
    assert_eq!(context.agent_id.as_str(), "direct-agent");
}

#[test]
fn runtime_builder_agent_name_configures_owned_runtime_session() {
    let runtime = AgentRuntimeBuilder::new(Arc::new(reusable_text_model("ok")))
        .agent_name("Owned Runtime")
        .build();

    assert_eq!(runtime.session().context().agent_id.as_str(), "main");
    assert_eq!(
        runtime.session().context().agent_registry["main"].agent_name,
        "Owned Runtime"
    );
}

#[tokio::test]
async fn builder_compact_config_uses_dedicated_model_settings_and_params() {
    let compact_called = Arc::new(Mutex::new(false));
    let compact_called_model = compact_called.clone();
    let compact_model = FunctionModel::streaming(
        move |messages: Vec<ModelMessage>,
              settings: Option<ModelSettings>,
              info: FunctionModelInfo| {
            *compact_called_model.lock().unwrap() = true;
            assert!(format!("{messages:?}").contains("Compact the conversation history"));
            assert_eq!(settings.unwrap().temperature, Some(0.3));
            assert_eq!(
                info.params.extra_body.get("compact_route"),
                Some(&serde_json::json!("builder"))
            );
            Ok(vec![ModelResponseStreamEvent::FinalResult(Box::new(
                ModelResponse::text(
                    "## Condensed conversation summary\n\n### Analysis\n\nBuilder compacted.",
                ),
            ))])
        },
    );
    let main_model = FunctionModel::new(|messages, _settings, _info| {
        assert!(format!("{messages:?}").contains("Builder compacted"));
        Ok(ModelResponse::text("main"))
    });
    let mut compact_params = ModelRequestParameters::default();
    compact_params
        .extra_body
        .insert("compact_route".to_string(), serde_json::json!("builder"));
    let compact_settings = ModelSettings {
        temperature: Some(0.3),
        ..ModelSettings::default()
    };
    let app = AgentBuilder::new(Arc::new(main_model))
        .compact_model(Arc::new(compact_model))
        .compact_model_settings(compact_settings)
        .compact_request_params(compact_params)
        .build_app();
    let mut session = app.session();
    session.context_mut().model_config = ModelConfig {
        context_window: Some(100),
        compact_threshold: PerThousandRatio::from_per_thousand(900),
        ..ModelConfig::default()
    };
    let mut prior_response = ModelResponse::text("large prior response");
    prior_response.usage = Usage {
        requests: 1,
        input_tokens: 90,
        output_tokens: 5,
        total_tokens: 95,
        ..Usage::default()
    };
    session.context_mut().message_history = vec![
        ModelMessage::Request(ModelRequest::user_text("old request")),
        ModelMessage::Response(prior_response),
    ];

    let result = session.run("continue").await.unwrap();

    assert_eq!(result.output, "main");
    assert!(*compact_called.lock().unwrap());
}

#[tokio::test]
async fn session_stream_uses_session_context() {
    let mut session =
        AgentSession::new(AgentBuilder::new(Arc::new(reusable_text_model("streamed"))).build());

    let stream = session.run_stream("hello").await.unwrap();

    assert_eq!(stream.result().output, "streamed");
    assert_eq!(session.context().usage.requests, 1);
    assert!(matches!(
        stream.events()[0].event,
        AgentStreamEvent::RunStart { .. }
    ));
    assert!(matches!(
        stream.events().last().unwrap().event,
        AgentStreamEvent::RunComplete { .. }
    ));
}

#[test]
fn session_try_stream_reports_missing_tokio_runtime() {
    let mut session =
        AgentSession::new(AgentBuilder::new(Arc::new(reusable_text_model("live"))).build());

    let Err(error) = session.try_stream("hello") else {
        panic!("try_stream should report a missing Tokio runtime");
    };

    assert!(matches!(error, AgentStreamError::RuntimeUnavailable(_)));
}

#[tokio::test]
async fn session_try_stream_returns_live_handle_inside_tokio_runtime() {
    let mut session =
        AgentSession::new(AgentBuilder::new(Arc::new(reusable_text_model("live"))).build());

    let mut handle = session.try_stream("hello").unwrap();
    let first = handle.recv().await.unwrap();

    assert!(matches!(first.event, AgentStreamEvent::RunStart { .. }));
    let result = handle.finish_into_session(&mut session).await.unwrap();
    assert_eq!(result.result.output, "live");
}

#[tokio::test]
async fn session_live_stream_yields_events_and_writes_context_back() {
    let mut session =
        AgentSession::new(AgentBuilder::new(Arc::new(reusable_text_model("live"))).build());

    let mut handle = session.stream("hello");
    let first = handle.recv().await.unwrap();

    assert!(matches!(first.event, AgentStreamEvent::RunStart { .. }));
    assert_eq!(session.context().usage.requests, 0);

    let result = handle.finish_into_session(&mut session).await.unwrap();

    assert_eq!(result.result.output, "live");
    assert_eq!(session.context().usage.requests, 1);
    assert!(result
        .events
        .iter()
        .any(|record| matches!(record.event, AgentStreamEvent::RunComplete { .. })));
}

#[tokio::test]
async fn session_live_stream_complete_returns_error_and_recoverable_state() {
    let mut session =
        AgentSession::new(AgentBuilder::new(Arc::new(reusable_text_model("plain text"))).build());

    let completion = session
        .stream_with_options(
            "hello",
            AgentRunOptions::new().output_policy(OutputPolicy::typed::<StreamAnswer>()),
        )
        .complete()
        .await;

    assert!(completion.is_err());
    assert!(completion.result.is_none());
    assert!(matches!(completion.error, Some(AgentStreamError::Agent(_))));
    assert!(completion.state.run_id.is_some());
    assert!(!completion.state.message_history.is_empty());
}

#[tokio::test]
async fn session_live_stream_options_count_dropped_records_when_receiver_lags() {
    let mut session = AgentSession::new(
        AgentBuilder::new(Arc::new(high_volume_stream_model("streamed", 32))).build(),
    );

    let handle = session.stream_with_stream_options(
        "hello",
        AgentStreamOptions::new()
            .buffer_size(1)
            .drop_policy(AgentStreamDropPolicy::DropNewest),
    );

    for _ in 0..50 {
        if handle.is_finished() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert_eq!(handle.options().buffer_size, 1);
    assert!(handle.dropped_events() > 0);
    let completion = handle.complete().await;

    assert!(completion.is_ok());
    assert_eq!(completion.result.unwrap().result.output, "streamed");
}

#[tokio::test]
async fn session_live_stream_backpressure_delivers_without_drops() {
    let mut session = AgentSession::new(
        AgentBuilder::new(Arc::new(high_volume_stream_model("streamed", 16))).build(),
    );

    let mut handle = session.stream_with_stream_options(
        "hello",
        AgentStreamOptions::new()
            .buffer_size(1)
            .drop_policy(AgentStreamDropPolicy::Backpressure),
    );
    let mut received = 0;
    while handle.recv().await.is_some() {
        received += 1;
    }

    assert!(received > 16);
    assert_eq!(handle.dropped_events(), 0);
    let result = handle.join().await.unwrap();
    assert_eq!(result.result.output, "streamed");
}

#[tokio::test]
async fn session_live_stream_closed_receiver_does_not_fail_run() {
    let mut session = AgentSession::new(
        AgentBuilder::new(Arc::new(high_volume_stream_model("streamed", 16))).build(),
    );

    let mut handle = session.stream_with_stream_options(
        "hello",
        AgentStreamOptions::new()
            .buffer_size(1)
            .drop_policy(AgentStreamDropPolicy::Backpressure),
    );
    let first = handle.recv().await.unwrap();
    assert!(matches!(first.event, AgentStreamEvent::RunStart { .. }));
    handle.close_receiver();

    for _ in 0..50 {
        if handle.is_finished() && handle.receiver_closed() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert!(handle.receiver_closed());
    let result = handle.join().await.unwrap();
    assert_eq!(result.result.output, "streamed");
}

#[tokio::test]
async fn session_live_stream_interrupt_returns_recoverable_state() {
    let mut session = AgentSession::new(
        AgentBuilder::new(Arc::new(high_volume_stream_model("streamed", 64))).build(),
    );

    let mut handle = session.stream_with_stream_options(
        "hello",
        AgentStreamOptions::new()
            .buffer_size(1)
            .drop_policy(AgentStreamDropPolicy::Backpressure),
    );
    let first = handle.recv().await.unwrap();
    assert!(matches!(first.event, AgentStreamEvent::RunStart { .. }));
    assert!(!handle.cancel_requested());

    handle.interrupt();
    assert!(handle.cancel_requested());
    let completion = handle.complete().await;

    assert!(completion.is_err());
    assert!(matches!(
        completion.error,
        Some(AgentStreamError::Interrupted)
    ));
    assert!(completion.state.run_id.is_some());
}

#[tokio::test]
async fn session_live_stream_interrupt_cancels_model_stream_token() {
    let observed_token = Arc::new(Mutex::new(None));
    let mut session = AgentSession::new(
        AgentBuilder::new(Arc::new(BlockingStreamModel::new(observed_token.clone()))).build(),
    );

    let mut handle = session.stream_with_stream_options(
        "hello",
        AgentStreamOptions::new()
            .buffer_size(8)
            .drop_policy(AgentStreamDropPolicy::Backpressure),
    );
    let first = handle.recv().await.unwrap();
    assert!(matches!(first.event, AgentStreamEvent::RunStart { .. }));

    for _ in 0..50 {
        while handle.try_recv().is_ok() {}
        if observed_token.lock().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let Some(token) = observed_token.lock().unwrap().clone() else {
        panic!("model request should receive a cancellation token");
    };
    assert!(!token.is_cancelled());

    handle.interrupt();
    assert!(handle.cancel_requested());
    assert!(token.is_cancelled());

    let completion = handle.complete().await;
    assert!(completion.is_err());
    assert!(matches!(
        completion.error,
        Some(AgentStreamError::Interrupted)
    ));
}

#[tokio::test]
async fn session_live_stream_interrupt_cancels_running_tool_token() {
    let model_calls = Arc::new(Mutex::new(0usize));
    let model_calls_for_model = model_calls.clone();
    let model = FunctionModel::new(move |_messages, _settings, _info| {
        let mut calls = model_calls_for_model.lock().unwrap();
        *calls += 1;
        if *calls == 1 {
            Ok(tool_call_response(
                "call_slow",
                "slow",
                serde_json::json!({}),
            ))
        } else {
            Ok(ModelResponse::text("after tool"))
        }
    });
    let observed_token = Arc::new(Mutex::new(None));
    let observed_token_for_tool = observed_token.clone();
    let (started_sender, mut started_receiver) = tokio::sync::mpsc::channel(1);
    let slow_tool = FunctionTool::new(
        "slow",
        Some("Slow tool".to_string()),
        serde_json::json!({"type":"object"}),
        move |ctx: ToolContext, _args: serde_json::Value| {
            let observed_token_for_tool = observed_token_for_tool.clone();
            let started_sender = started_sender.clone();
            async move {
                *observed_token_for_tool.lock().unwrap() = Some(ctx.cancellation_token());
                let _ = started_sender.send(()).await;
                ctx.cancellation_token.cancelled().await;
                Ok(ToolResult::new(serde_json::json!({"done": true})))
            }
        },
    );
    let mut session = AgentSession::new(
        AgentBuilder::new(Arc::new(model))
            .tool(Arc::new(slow_tool))
            .build(),
    );

    let mut handle = session.stream_with_stream_options(
        "hello",
        AgentStreamOptions::new()
            .buffer_size(16)
            .drop_policy(AgentStreamDropPolicy::Backpressure),
    );
    for _ in 0..100 {
        while handle.try_recv().is_ok() {}
        if started_receiver.try_recv().is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let Some(token) = observed_token.lock().unwrap().clone() else {
        panic!("running tool should receive a cancellation token");
    };
    assert!(!token.is_cancelled());

    handle.interrupt();
    assert!(handle.cancel_requested());
    assert!(token.is_cancelled());

    let completion = handle.complete().await;
    assert!(completion.is_err());
    assert!(matches!(
        completion.error,
        Some(AgentStreamError::Interrupted)
    ));
}

#[tokio::test]
async fn session_propagates_trace_context_to_model_requests() {
    let observed = Arc::new(Mutex::new(Vec::<TraceContext>::new()));
    let observed_model = observed.clone();
    let model = FunctionModel::new(move |_messages, _settings, info| {
        observed_model
            .lock()
            .unwrap()
            .push(info.context.trace_context);
        Ok(ModelResponse {
            usage: Usage {
                requests: 1,
                ..Usage::default()
            },
            ..ModelResponse::text("traced")
        })
    });
    let trace_context = TraceContext::from_trace_id("trace-session")
        .with_span_id("span-session")
        .with_parent_span_id("root-span");
    let mut session = AgentSession::new(AgentBuilder::new(Arc::new(model)).build())
        .with_trace_context(trace_context.clone());

    let result = session.run("hello").await.unwrap();

    assert_eq!(result.output, "traced");
    assert_eq!(session.context().trace_context, trace_context);
    assert_eq!(observed.lock().unwrap().as_slice(), &[trace_context]);
}

#[tokio::test]
async fn session_accepts_w3c_trace_parent_header() {
    let mut session =
        AgentSession::new(AgentBuilder::new(Arc::new(reusable_text_model("ok"))).build())
            .with_trace_parent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01");

    let result = session.run("hello").await.unwrap();

    assert_eq!(result.output, "ok");
    assert_eq!(
        session.context().trace_context.trace_id.as_deref(),
        Some("4bf92f3577b34da6a3ce929d0e0e4736")
    );
    assert_eq!(
        session.context().trace_context.parent_span_id.as_deref(),
        Some("00f067aa0ba902b7")
    );
}

#[tokio::test]
async fn runtime_durable_records_preserve_trace_correlation() {
    let store = Arc::new(InMemorySessionStore::new());
    let session_id = starweaver_agent::SessionId::from_string("session-trace-correlation");
    let trace_context = TraceContext::from_trace_id("trace-durable-correlation")
        .with_span_id("span-durable-parent")
        .with_parent_span_id("span-root");
    let context = AgentContext {
        trace_context: trace_context.clone(),
        ..AgentContext::default()
    };
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(reusable_text_model("traced durable")))
        .durable_session_id(session_id.clone())
        .session_store(store.clone())
        .context(context)
        .build();

    let result = runtime.run("hello traced durable").await.unwrap();
    let run_id = result.state.run_id.clone();

    let session = store.load_session(&session_id).await.unwrap();
    assert_eq!(
        session.trace_context.trace_id.as_deref(),
        Some("trace-durable-correlation")
    );
    let run = store.load_run(&session_id, &run_id).await.unwrap();
    assert_eq!(
        run.trace_context.trace_id.as_deref(),
        Some("trace-durable-correlation")
    );
    let checkpoints = store.load_checkpoints(&session_id, &run_id).await.unwrap();
    assert!(checkpoints.iter().any(|checkpoint| {
        checkpoint.resume.trace_context.trace_id.as_deref() == Some("trace-durable-correlation")
    }));
}

#[test]
fn session_helpers_update_metadata_notes_state_and_bus() {
    let mut session =
        AgentSession::new(AgentBuilder::new(Arc::new(reusable_text_model("ok"))).build());

    session.set_state("workspace", serde_json::json!({"root": "/repo"}));
    session.set_note("language", "Chinese");
    session.enqueue_message("task", serde_json::json!({"id": 1}));
    session.set_metadata("owner", serde_json::json!("sdk"));

    assert_eq!(
        session.context().state.get("workspace").unwrap()["root"],
        "/repo"
    );
    assert_eq!(session.context().notes.get("language"), Some("Chinese"));
    assert_eq!(session.context().messages.len(), 1);
    assert_eq!(session.context().metadata["owner"], "sdk");
}
