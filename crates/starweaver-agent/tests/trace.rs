#![allow(missing_docs, clippy::unwrap_used)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    sync::Arc,
};

use starweaver_agent::{
    AgentBuilder, AgentRuntimePolicy, ModelConfig, PerThousandRatio, ShellReviewConfig,
    SubagentConfig, SubagentRegistry, attach_environment, attach_shell_review, shell_tools,
};
use starweaver_core::Metadata;
use starweaver_environment::{ShellOutput, VirtualEnvironmentProvider};
use starweaver_model::{
    FunctionModel, FunctionModelInfo, ModelMessage, ModelRequest, ModelRequestPart, ModelResponse,
    ModelResponsePart, ModelResponseStreamEvent, ModelSettings, ToolCallPart,
};
use starweaver_runtime::{InMemoryTraceRecorder, RecordedSpan, SpanStatus};
use starweaver_usage::Usage;

#[tokio::test]
async fn compact_model_records_compact_agent_subtree() {
    let recorder = Arc::new(InMemoryTraceRecorder::new());
    let compact_model = FunctionModel::streaming(
        |_messages: Vec<ModelMessage>,
         _settings: Option<ModelSettings>,
         _info: FunctionModelInfo| {
            Ok(vec![ModelResponseStreamEvent::FinalResult(Box::new(
                ModelResponse::text(
                    "## Condensed conversation summary\n\n### Analysis\n\nCompact trace summary.",
                ),
            ))])
        },
    );
    let main_model = FunctionModel::new(|messages, _settings, _info| {
        assert!(format!("{messages:?}").contains("Compact trace summary"));
        Ok(ModelResponse::text("main done"))
    });
    let agent = AgentBuilder::new(Arc::new(main_model))
        .compact_model(Arc::new(compact_model))
        .trace_recorder(recorder.clone())
        .build();
    let mut context = agent.new_context();
    context.model_config = ModelConfig {
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
    context.message_history = vec![
        ModelMessage::Request(ModelRequest::user_text("old request")),
        ModelMessage::Response(prior_response),
    ];

    let result = agent
        .run_with_context("continue", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "main done");
    let spans = recorder.spans();
    print_trace_tree_if_requested("compact", &spans);
    assert!(spans.iter().all(|span| span.status == SpanStatus::Ok));
    let compact_span = span_with_name(&spans, "starweaver.history.compaction");
    let compact_agent = spans
        .iter()
        .find(|span| {
            span.name == "gen_ai.invoke_agent"
                && span.attributes.get("gen_ai.agent.name")
                    == Some(&serde_json::json!("Compact-Agent"))
        })
        .unwrap();
    assert_eq!(
        compact_agent.parent_span_id.as_deref(),
        Some(compact_span.span_id.as_str())
    );
    let compact_model_span = spans
        .iter()
        .find(|span| {
            span.name == "gen_ai.inference"
                && span.parent_span_id.as_deref() == Some(compact_agent.span_id.as_str())
        })
        .unwrap();
    assert_eq!(
        compact_model_span.attributes["gen_ai.agent.id"],
        serde_json::json!("starweaver.compact")
    );
    assert!(
        compact_model_span
            .events
            .iter()
            .any(|event| event.name == "starweaver.model.request")
    );
    assert!(
        compact_model_span
            .events
            .iter()
            .any(|event| event.name == "starweaver.model.response")
    );
}

#[tokio::test]
async fn delegate_tool_records_child_agent_under_tool_span() {
    let recorder = Arc::new(InMemoryTraceRecorder::new());
    let child = Arc::new(
        AgentBuilder::new(Arc::new(FunctionModel::new(
            |_messages, _settings, _info| Ok(ModelResponse::text("child done")),
        )))
        .build(),
    );
    let registry =
        Arc::new(SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child)));
    let parent_model = Arc::new(FunctionModel::new(|messages, _settings, _info| {
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
                    arguments: serde_json::json!({
                        "name": "child",
                        "prompt": "help"
                    })
                    .into(),
                })],
                ..ModelResponse::text("")
            })
        }
    }));
    let parent = AgentBuilder::new(parent_model)
        .policy(AgentRuntimePolicy {
            max_steps: 4,
            ..AgentRuntimePolicy::default()
        })
        .tool(registry.delegate_tool())
        .trace_recorder(recorder.clone())
        .build();

    let result = parent.run("delegate").await.unwrap();

    assert_eq!(result.output, "parent done");
    let spans = recorder.spans();
    print_trace_tree_if_requested("delegate", &spans);
    assert!(spans.iter().all(|span| span.status == SpanStatus::Ok));
    let delegate_tool = spans
        .iter()
        .find(|span| {
            span.name == "gen_ai.execute_tool"
                && span.attributes.get("gen_ai.tool.name") == Some(&serde_json::json!("delegate"))
        })
        .unwrap();
    let child_agent = spans
        .iter()
        .find(|span| {
            span.name == "gen_ai.invoke_agent"
                && span.attributes.get("gen_ai.agent.name") == Some(&serde_json::json!("child"))
        })
        .unwrap();
    assert_eq!(
        child_agent.parent_span_id.as_deref(),
        Some(delegate_tool.span_id.as_str())
    );
    let child_step = spans
        .iter()
        .find(|span| {
            span.name == "starweaver.loop.step"
                && span.parent_span_id.as_deref() == Some(child_agent.span_id.as_str())
        })
        .unwrap();
    assert!(spans.iter().any(|span| {
        span.name == "gen_ai.inference"
            && span.parent_span_id.as_deref() == Some(child_step.span_id.as_str())
    }));
}

#[tokio::test]
async fn shell_review_records_nested_model_request_under_tool_span() {
    let recorder = Arc::new(InMemoryTraceRecorder::new());
    let provider = Arc::new(VirtualEnvironmentProvider::new("trace").with_shell_output(
        "echo traced",
        ShellOutput {
            status: 0,
            stdout: "traced\n".to_string(),
            stderr: String::new(),
            metadata: Metadata::default(),
        },
    ));
    let review_model = Arc::new(FunctionModel::new(|_messages, _settings, _info| {
        Ok(ModelResponse::text(
            r#"{"risk_level":"low","reason":"read-only trace test"}"#,
        ))
    }));
    let parent_model = Arc::new(FunctionModel::new(|messages, _settings, _info| {
        if messages.iter().any(message_has_tool_return) {
            return Ok(ModelResponse::text("shell done"));
        }
        Ok(ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: "shell-call".to_string(),
                name: "shell_exec".to_string(),
                arguments: serde_json::json!({"command": "echo traced"}).into(),
            })],
            ..ModelResponse::text("")
        })
    }));
    let agent = AgentBuilder::new(parent_model)
        .policy(AgentRuntimePolicy {
            max_steps: 4,
            ..AgentRuntimePolicy::default()
        })
        .toolset(&shell_tools())
        .trace_recorder(recorder.clone())
        .build();
    let mut context = agent.new_context();
    attach_environment(&mut context, provider);
    attach_shell_review(&mut context, ShellReviewConfig::enabled(review_model));

    let result = agent
        .run_with_context("run shell", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "shell done");
    let spans = recorder.spans();
    print_trace_tree_if_requested("shell-review", &spans);
    assert!(spans.iter().all(|span| span.status == SpanStatus::Ok));
    let shell_tool = spans
        .iter()
        .find(|span| {
            span.name == "gen_ai.execute_tool"
                && span.attributes.get("gen_ai.tool.name") == Some(&serde_json::json!("shell_exec"))
        })
        .unwrap();
    let review_model = spans
        .iter()
        .find(|span| {
            span.name == "gen_ai.inference"
                && span.attributes.get("gen_ai.agent.id")
                    == Some(&serde_json::json!("shell_review"))
        })
        .unwrap();
    assert_eq!(
        review_model.parent_span_id.as_deref(),
        Some(shell_tool.span_id.as_str())
    );
    assert!(
        review_model
            .events
            .iter()
            .any(|event| event.name == "starweaver.model.request")
    );
    assert!(
        review_model
            .events
            .iter()
            .any(|event| event.name == "starweaver.model.response")
    );
}

fn span_with_name<'a>(spans: &'a [RecordedSpan], name: &str) -> &'a RecordedSpan {
    spans.iter().find(|span| span.name == name).unwrap()
}

fn message_has_tool_return(message: &ModelMessage) -> bool {
    matches!(
        message,
        ModelMessage::Request(request)
            if request
                .parts
                .iter()
                .any(|part| matches!(part, ModelRequestPart::ToolReturn(_)))
    )
}

fn print_trace_tree_if_requested(label: &str, spans: &[RecordedSpan]) {
    if std::env::var_os("STARWEAVER_TRACE_PRINT").is_none() {
        return;
    }
    eprintln!("trace tree: {label}");
    eprintln!("{}", render_trace_tree(spans));
}

fn render_trace_tree(spans: &[RecordedSpan]) -> String {
    let span_ids = spans
        .iter()
        .map(|span| span.span_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut children = BTreeMap::<Option<&str>, Vec<&RecordedSpan>>::new();
    for span in spans {
        let parent = span
            .parent_span_id
            .as_deref()
            .filter(|parent| span_ids.contains(parent));
        children.entry(parent).or_default().push(span);
    }
    let mut lines = Vec::new();
    render_trace_children(None, 0, &children, &mut lines);
    lines.join("\n")
}

fn render_trace_children<'a>(
    parent: Option<&'a str>,
    depth: usize,
    children: &BTreeMap<Option<&'a str>, Vec<&'a RecordedSpan>>,
    lines: &mut Vec<String>,
) {
    let Some(spans) = children.get(&parent) else {
        return;
    };
    for span in spans {
        lines.push(format!("{}- {}", "  ".repeat(depth), span_label(span)));
        render_trace_children(Some(span.span_id.as_str()), depth + 1, children, lines);
    }
}

fn span_label(span: &RecordedSpan) -> String {
    let mut label = span.name.clone();
    if let Some(agent_name) = span
        .attributes
        .get("gen_ai.agent.name")
        .and_then(serde_json::Value::as_str)
    {
        let _ = write!(&mut label, " agent={agent_name}");
    }
    if let Some(tool_name) = span
        .attributes
        .get("gen_ai.tool.name")
        .and_then(serde_json::Value::as_str)
    {
        let _ = write!(&mut label, " tool={tool_name}");
    }
    label
}
