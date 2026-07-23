#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use starweaver_agent::{
    AgentBuilder, AgentCapability, AgentControlDeliveryState, AgentControlError,
    AgentControlErrorCode, AgentControlKind, AgentRunState, AgentRuntimePolicy,
    AgentStreamDropPolicy, AgentStreamError, AgentStreamEvent, AgentStreamHandle,
    AgentStreamLiveState, AgentStreamOptions, AgentStreamRecord, AgentStreamSourceKind, BusMessage,
    CapabilityError, CapabilityResult, FunctionTool, StaticCapabilityBundle, SubagentConfig,
    SubagentRegistry, TestModel, ToolContext, ToolResult,
};
use starweaver_model::{
    ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart, ToolCallPart,
};
use starweaver_usage::Usage;

fn stable_session_id(session: &starweaver_agent::AgentSession) -> String {
    session.context().session_id().map_or_else(
        || panic!("SDK session should have a stable id"),
        |session_id| session_id.as_str().to_string(),
    )
}

#[tokio::test]
async fn facade_reexports_stream_event_types() {
    let stream = AgentBuilder::new(Arc::new(TestModel::with_text("ok")))
        .build()
        .run_stream("hello")
        .await
        .unwrap();

    assert_eq!(stream.result().output, "ok");
    assert!(matches!(
        stream.events()[0].event,
        AgentStreamEvent::RunStart { .. }
    ));
    assert!(matches!(
        stream.events().last().unwrap().event,
        AgentStreamEvent::RunComplete { .. }
    ));
}

#[tokio::test]
async fn live_stream_handle_receives_attributed_subagent_records() {
    let child = Arc::new(
        AgentBuilder::new(Arc::new(TestModel::with_responses(vec![ModelResponse {
            usage: Usage {
                requests: 1,
                input_tokens: 1,
                output_tokens: 1,
                total_tokens: 2,
                ..Usage::default()
            },
            ..ModelResponse::text("child output")
        }])))
        .build(),
    );
    let registry =
        Arc::new(SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child)));
    let model = Arc::new(starweaver_agent::FunctionModel::new(
        |messages, _settings, _info| {
            let has_tool_return = messages.iter().any(|message| {
                matches!(
                    message,
                    ModelMessage::Request(request)
                        if request
                            .parts
                            .iter()
                            .any(|part| matches!(part, ModelRequestPart::ToolReturn(_)))
                )
            });
            if has_tool_return {
                Ok(ModelResponse::text("parent done"))
            } else {
                Ok(ModelResponse {
                    parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                        id: "delegate-call".to_string(),
                        name: "delegate".to_string(),
                        arguments: serde_json::json!({"name": "child", "prompt": "help"}).into(),
                    })],
                    ..ModelResponse::text("")
                })
            }
        },
    ));
    let app = AgentBuilder::new(model)
        .policy(AgentRuntimePolicy {
            max_steps: 4,
            ..AgentRuntimePolicy::default()
        })
        .tool(registry.delegate_tool())
        .build_app();

    let mut live = app.stream_with_stream_options(
        "delegate",
        AgentStreamOptions::new().drop_policy(AgentStreamDropPolicy::Backpressure),
    );
    let mut observed = Vec::new();
    while let Some(record) = live.recv().await {
        observed.push(record);
    }
    let result = live.join().await.unwrap();

    assert_eq!(result.result.output, "parent done");
    assert_eq!(observed, result.events);
    let Some(source_index) = observed.iter().position(|record| record.source.is_some()) else {
        panic!("attributed child stream record");
    };
    let source = observed[source_index].source.as_ref().unwrap();
    assert_eq!(&source.kind, &AgentStreamSourceKind::Subagent);
    assert_eq!(source.agent_name, "child");
    assert!(source.run_id.is_some());
    assert!(source.parent_run_id.is_some());
    let Some(delegate_return_index) = observed.iter().position(|record| {
        matches!(
            &record.event,
            AgentStreamEvent::ToolReturn { tool_return, .. } if tool_return.name == "delegate"
        )
    }) else {
        panic!("parent delegate tool return should be streamed");
    };
    assert!(
        source_index < delegate_return_index,
        "child stream records must be forwarded before parent delegate ToolReturn"
    );
    assert!(observed.iter().any(|record| record.source.is_some()
        && matches!(record.event, AgentStreamEvent::RunComplete { .. })));
}

#[tokio::test]
async fn live_stream_completion_repairs_dangling_tool_call_on_interrupt() {
    let model = Arc::new(starweaver_agent::FunctionModel::new(
        |_messages, _settings, _info| {
            Ok(ModelResponse {
                parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                    id: "call_interrupt".to_string(),
                    name: "lookup".to_string(),
                    arguments: serde_json::json!({"query": "docs"}).into(),
                })],
                ..ModelResponse::text("")
            })
        },
    ));
    let lookup = Arc::new(FunctionTool::new(
        "lookup",
        Some("Lookup docs".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    ));
    let bundle = StaticCapabilityBundle::new("cancel-on-tool-call")
        .with_stream_observer(Arc::new(CancelOnToolCall));
    let app = AgentBuilder::new(model)
        .tool(lookup)
        .capability_bundle(Arc::new(bundle))
        .build_app();

    let completion = app.stream("trigger tool").complete().await;

    assert!(matches!(
        completion.error,
        Some(AgentStreamError::Interrupted { .. })
    ));
    let Some(recorded_call) = completion.state.message_history.iter().find_map(|message| {
        let ModelMessage::Response(response) = message else {
            return None;
        };
        response.parts.iter().find_map(|part| {
            let ModelResponsePart::ToolCall(call) = part else {
                return None;
            };
            Some(call)
        })
    }) else {
        panic!("interrupted stream should record response tool call");
    };
    let Some(ModelMessage::Request(request)) = completion.state.message_history.last() else {
        panic!("repair should append request");
    };
    let Some(ModelRequestPart::ToolReturn(tool_return)) = request.parts.first() else {
        panic!("repair should append tool return");
    };
    assert_eq!(tool_return.tool_call_id, recorded_call.id);
    assert_eq!(tool_return.name, recorded_call.name);
    assert!(tool_return.is_error);
    assert_eq!(tool_return.content["error"], "tool_call_interrupted");
    assert_eq!(
        tool_return.metadata["starweaver.repaired_dangling_tool_call"],
        true
    );
}

#[tokio::test]
async fn live_stream_status_reports_running_and_cancelling() {
    let model = Arc::new(starweaver_agent::FunctionModel::new(
        |messages, _settings, _info| {
            let has_tool_return = messages.iter().any(|message| {
                matches!(
                    message,
                    ModelMessage::Request(request)
                        if request
                            .parts
                            .iter()
                            .any(|part| matches!(part, ModelRequestPart::ToolReturn(_)))
                )
            });
            if has_tool_return {
                Ok(ModelResponse::text("done"))
            } else {
                Ok(ModelResponse {
                    parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                        id: "slow_call".to_string(),
                        name: "slow".to_string(),
                        arguments: serde_json::json!({}).into(),
                    })],
                    ..ModelResponse::text("")
                })
            }
        },
    ));
    let slow = Arc::new(FunctionTool::new(
        "slow",
        Some("Slow tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            Ok(ToolResult::new(serde_json::json!({"ok": true})))
        },
    ));
    let app = AgentBuilder::new(model)
        .tool(slow)
        .policy(AgentRuntimePolicy {
            max_steps: 4,
            ..AgentRuntimePolicy::default()
        })
        .build_app();
    let mut handle = app.stream("status");
    while let Some(record) = handle.recv().await {
        if matches!(record.event, AgentStreamEvent::ToolCall { .. }) {
            break;
        }
    }

    let running = handle.status();
    assert_eq!(running.live_state, AgentStreamLiveState::Active);
    assert!(!running.cancel_requested);

    handle.interrupt();
    let cancelling = handle.status();
    assert_eq!(cancelling.live_state, AgentStreamLiveState::Cancelling);
    assert!(cancelling.cancel_requested);

    let completion = handle.complete().await;
    assert!(matches!(
        completion.error,
        Some(AgentStreamError::Interrupted { .. })
    ));
}

#[tokio::test]
async fn live_control_contract_exposes_stable_status_receipts_and_error_codes() {
    let release = Arc::new(tokio::sync::Notify::new());
    let model = Arc::new(starweaver_agent::FunctionModel::new(
        |messages, _settings, _info| {
            let has_tool_return = messages.iter().any(|message| {
                matches!(
                    message,
                    ModelMessage::Request(request)
                        if request
                            .parts
                            .iter()
                            .any(|part| matches!(part, ModelRequestPart::ToolReturn(_)))
                )
            });
            if has_tool_return {
                Ok(ModelResponse::text("done"))
            } else {
                Ok(ModelResponse {
                    parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                        id: "call_wait".to_string(),
                        name: "wait".to_string(),
                        arguments: serde_json::json!({}).into(),
                    })],
                    ..ModelResponse::text("")
                })
            }
        },
    ));
    let release_tool = Arc::clone(&release);
    let wait = Arc::new(FunctionTool::new(
        "wait",
        Some("Wait before returning".to_string()),
        serde_json::json!({"type": "object"}),
        move |_ctx: ToolContext, _args: serde_json::Value| {
            let release = Arc::clone(&release_tool);
            async move {
                release.notified().await;
                Ok(ToolResult::new(serde_json::json!({"ok": true})))
            }
        },
    ));
    let app = AgentBuilder::new(model)
        .tool(wait)
        .policy(AgentRuntimePolicy {
            max_steps: 4,
            ..AgentRuntimePolicy::default()
        })
        .build_app();
    let mut handle = app.stream("contract");
    recv_until_tool_call(&mut handle).await;

    let status = handle.status();
    assert_eq!(status.live_state, AgentStreamLiveState::Active);
    assert_eq!(status.live_state.as_str(), "active");
    assert!(!status.live_state.is_terminal());
    let status_json = serde_json::to_value(status).unwrap();
    assert_eq!(status_json["live_state"], "active");
    assert_eq!(status_json["is_terminal"], false);
    assert_eq!(status_json["receiver_closed"], false);
    assert!(status_json["buffer_size"].is_number());
    assert_eq!(status_json["drop_policy"], "drop_newest");

    let control = handle.control_handle();
    let receipt = control
        .send_message(BusMessage::text("active note", "user").with_id("active-note"))
        .await
        .unwrap();
    assert_eq!(
        receipt.delivery_state(),
        AgentControlDeliveryState::PendingDelivery
    );
    assert_eq!(receipt.delivery_state_str(), "pending_delivery");
    assert_eq!(receipt.kind.as_str(), "message");
    assert!(receipt.pending_delivery);
    let receipt_json = serde_json::to_value(&receipt).unwrap();
    assert_eq!(receipt_json["kind"], "message");
    assert_eq!(receipt_json["pending_delivery"], true);
    assert_eq!(receipt_json["delivery_state"], "pending_delivery");

    handle.close_receiver();
    assert!(handle.status().receiver_closed);
    let error = control
        .send_message(BusMessage::text("too late", "user").with_id("after-close"))
        .await
        .unwrap_err();
    assert!(matches!(error, AgentControlError::ReceiverClosed));
    assert_eq!(error.code(), AgentControlErrorCode::ReceiverClosed);
    assert_eq!(error.code_str(), "receiver_closed");
    assert_eq!(
        serde_json::to_value(error.code()).unwrap(),
        "receiver_closed"
    );

    release.notify_one();
    let result = handle.join().await.unwrap();
    assert_eq!(result.result.output, "done");
    let terminal_error = control
        .send_message(BusMessage::text("after terminal", "user").with_id("after-terminal"))
        .await
        .unwrap_err();
    assert!(matches!(terminal_error, AgentControlError::TerminalRun));
    assert_eq!(terminal_error.code_str(), "already_finished");
}

#[tokio::test]
async fn live_control_handle_reports_context_and_accepts_messages() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let app = live_control_test_app(&captured);
    let mut session = app.session();
    let session_id = stable_session_id(&session);
    let mut handle = session.stream("deploy");
    recv_until_tool_call(&mut handle).await;

    assert_eq!(AgentControlKind::Message.as_str(), "message");
    assert_eq!(AgentControlKind::Steering.as_str(), "steering");
    assert_eq!(AgentControlKind::Interrupt.as_str(), "interrupt");

    let controller = handle.controller();
    assert!(!controller.cancel_requested());
    let controller_state = controller.recoverable_state().await;
    assert_recoverable_session(&controller_state, &session_id);
    let control = controller.control_handle();
    let control_state = control.recoverable_state().await;
    assert_recoverable_session(&control_state, &session_id);
    let handle_state = handle.recoverable_state().await;
    assert_recoverable_session(&handle_state, &session_id);
    assert_eq!(
        handle
            .latest_context()
            .await
            .session_id()
            .map(starweaver_agent::SessionId::as_str),
        Some(session_id.as_str())
    );

    let message_receipt = control
        .send_message(BusMessage::text("Review note from UI.", "user").with_id("ui-msg-1"))
        .await
        .unwrap();
    assert_eq!(message_receipt.id, "ui-msg-1");
    assert_eq!(message_receipt.kind, AgentControlKind::Message);
    assert!(message_receipt.pending_delivery);
    assert_eq!(
        message_receipt.session_id.as_deref(),
        Some(session_id.as_str())
    );

    while handle.recv().await.is_some() {}
    assert_eq!(handle.status().live_state, AgentStreamLiveState::Closed);
    let result = handle.join().await.unwrap();
    assert!(matches!(
        control
            .send_message(BusMessage::text("too late", "user").with_id("ui-msg-late"))
            .await,
        Err(starweaver_agent::AgentControlError::TerminalRun)
    ));

    assert_eq!(result.result.output, "done");
    assert!(result.events.iter().any(|record| {
        matches!(
            &record.event,
            AgentStreamEvent::Custom { event } if event.kind == "message_submitted"
        )
    }));
    assert_eq!(captured.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn live_control_steering_reaches_active_runtime_context() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let app = live_control_test_app(&captured);
    let mut session = app.session();
    let session_id = stable_session_id(&session);
    let mut handle = session.stream("deploy");
    recv_until_tool_call(&mut handle).await;

    let control = handle.control_handle();
    let receipt = control
        .steer("ui-1", "Use the safe rollout path.")
        .await
        .unwrap();
    assert_eq!(receipt.id, "ui-1");
    assert_eq!(receipt.kind, AgentControlKind::Steering);
    assert!(receipt.pending_delivery);
    assert!(
        receipt
            .run_id
            .as_deref()
            .is_some_and(|id| id.starts_with("run_"))
    );
    assert_eq!(receipt.session_id.as_deref(), Some(session_id.as_str()));
    let duplicate = control
        .steer("ui-1", "This exact operation retry must not be injected.")
        .await
        .unwrap();
    assert_eq!(duplicate, receipt);
    assert!(duplicate.pending_delivery);

    while handle.recv().await.is_some() {}
    let result = handle.join().await.unwrap();

    assert_eq!(result.result.output, "done");
    assert_eq!(
        result
            .context
            .session_id()
            .map(starweaver_agent::SessionId::as_str),
        Some(session_id.as_str())
    );
    assert_eq!(
        result.context.steering_messages,
        vec!["Use the safe rollout path.".to_string()]
    );
    assert!(control.operation_consumed("ui-1"));
    let captured = captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 2);
    assert!(format!("{:?}", captured[1]).contains("Steering update from the user"));
    assert!(result.events.iter().any(|record| {
        matches!(
            &record.event,
            AgentStreamEvent::Custom { event } if event.kind == "steering_submitted"
        )
    }));
    assert!(result.events.iter().any(|record| {
        matches!(
            &record.event,
            AgentStreamEvent::Custom { event } if event.kind == "steering_received"
        )
    }));
}

fn live_control_test_app(
    captured: &Arc<Mutex<Vec<Vec<ModelMessage>>>>,
) -> starweaver_agent::AgentApp {
    let captured_model = Arc::clone(captured);
    let model = Arc::new(starweaver_agent::FunctionModel::new(
        move |messages, _settings, _info| {
            captured_model.lock().unwrap().push(messages.clone());
            let has_tool_return = messages.iter().any(|message| {
                matches!(
                    message,
                    ModelMessage::Request(request)
                        if request
                            .parts
                            .iter()
                            .any(|part| matches!(part, ModelRequestPart::ToolReturn(_)))
                )
            });
            if has_tool_return {
                Ok(ModelResponse::text("done"))
            } else {
                Ok(ModelResponse {
                    parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                        id: "call_wait".to_string(),
                        name: "wait".to_string(),
                        arguments: serde_json::json!({}).into(),
                    })],
                    ..ModelResponse::text("")
                })
            }
        },
    ));
    let wait = Arc::new(FunctionTool::new(
        "wait",
        Some("Wait before returning".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            Ok(ToolResult::new(serde_json::json!({"ok": true})))
        },
    ));
    AgentBuilder::new(model)
        .tool(wait)
        .policy(AgentRuntimePolicy {
            max_steps: 4,
            ..AgentRuntimePolicy::default()
        })
        .build_app()
}

async fn recv_until_tool_call(handle: &mut AgentStreamHandle) {
    while let Some(record) = handle.recv().await {
        if matches!(record.event, AgentStreamEvent::ToolCall { .. }) {
            return;
        }
    }
    panic!("expected tool call stream record");
}

fn assert_recoverable_session(state: &starweaver_agent::ResumableState, session_id: &str) {
    assert_eq!(
        state
            .session_id
            .as_ref()
            .map(starweaver_agent::SessionId::as_str),
        Some(session_id)
    );
}

#[tokio::test]
async fn live_control_late_steering_reaches_output_guard() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<ModelMessage>>::new()));
    let captured_model = Arc::clone(&captured);
    let model = Arc::new(starweaver_agent::FunctionModel::new(
        move |messages, _settings, _info| {
            let request_count = {
                let mut captured = captured_model.lock().unwrap();
                captured.push(messages);
                captured.len()
            };
            if request_count == 1 {
                Ok(ModelResponse::text("ready"))
            } else {
                Ok(ModelResponse::text("done"))
            }
        },
    ));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let app = AgentBuilder::new(model)
        .capability(Arc::new(PauseDuringOutputValidation {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
            paused: std::sync::atomic::AtomicBool::new(false),
        }))
        .policy(AgentRuntimePolicy {
            max_steps: 4,
            ..AgentRuntimePolicy::default()
        })
        .build_app();
    let mut handle = app.stream("finalize");

    entered.notified().await;
    let receipt = handle
        .control_handle()
        .steer("late-1", "Use the safe rollout path.")
        .await
        .unwrap();
    assert_eq!(receipt.id, "late-1");
    assert_eq!(receipt.kind, AgentControlKind::Steering);
    assert!(receipt.pending_delivery);
    release.notify_one();

    while handle.recv().await.is_some() {}
    let result = handle.join().await.unwrap();

    assert_eq!(result.result.output, "done");
    let captured = captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 2);
    assert!(format!("{:?}", captured[1]).contains("Steering update from the user"));
    assert!(
        result
            .events
            .iter()
            .any(|record| { matches!(&record.event, AgentStreamEvent::SteeringGuard { .. }) })
    );
    assert!(result.events.iter().any(|record| {
        matches!(
            &record.event,
            AgentStreamEvent::Custom { event } if event.kind == "steering_submitted"
        )
    }));
    assert!(result.events.iter().any(|record| {
        matches!(
            &record.event,
            AgentStreamEvent::Custom { event } if event.kind == "steering_received"
        )
    }));
}

struct CancelOnToolCall;

#[async_trait]
impl AgentCapability for CancelOnToolCall {
    async fn on_stream_event(
        &self,
        _state: &AgentRunState,
        event: &AgentStreamRecord,
    ) -> CapabilityResult<()> {
        if matches!(event.event, AgentStreamEvent::ToolCall { .. }) {
            return Err(CapabilityError::Cancelled {
                reason: "test interrupted at tool call".to_string(),
            });
        }
        Ok(())
    }
}

struct PauseDuringOutputValidation {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    paused: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl AgentCapability for PauseDuringOutputValidation {
    async fn validate_output_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut starweaver_agent::AgentContext,
        _output: &str,
    ) -> CapabilityResult<()> {
        if self.paused.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        self.entered.notify_one();
        self.release.notified().await;
        Ok(())
    }
}
