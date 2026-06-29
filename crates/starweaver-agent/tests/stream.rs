#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use async_trait::async_trait;
use starweaver_agent::{
    AgentBuilder, AgentCapability, AgentRunState, AgentRuntimePolicy, AgentStreamDropPolicy,
    AgentStreamError, AgentStreamEvent, AgentStreamOptions, AgentStreamRecord,
    AgentStreamRunStatus, AgentStreamSourceKind, CapabilityError, CapabilityResult, FunctionTool,
    StaticCapabilityBundle, SubagentConfig, SubagentRegistry, TestModel, ToolContext, ToolResult,
};
use starweaver_model::{
    ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart, ToolCallPart,
};
use starweaver_usage::Usage;

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
        Some(AgentStreamError::Interrupted)
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
    assert_eq!(running.run_status, AgentStreamRunStatus::Running);
    assert!(!running.cancel_requested);

    handle.interrupt();
    let cancelling = handle.status();
    assert_eq!(cancelling.run_status, AgentStreamRunStatus::Cancelling);
    assert!(cancelling.cancel_requested);

    let completion = handle.complete().await;
    assert!(matches!(
        completion.error,
        Some(AgentStreamError::Interrupted)
    ));
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
