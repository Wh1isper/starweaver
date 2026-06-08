#![allow(clippy::unwrap_used)]

use std::path::Path;

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseEvent, MouseEventKind,
};
use serde_json::json;
use starweaver_context::AgentEvent;
use starweaver_core::{ConversationId, Metadata, RunId, SessionId};
use starweaver_model::{
    ModelResponse, ModelResponsePart, ModelResponseStreamEvent, PartDelta, PartEnd, PartStart,
    StreamDelta, ToolCallPart, ToolReturnPart,
};
use starweaver_runtime::{AgentExecutionNode, AgentStreamEvent, AgentStreamRecord, RunStatus};
use starweaver_session::{ApprovalRecord, DeferredToolRecord, ExecutionStatus};
use starweaver_stream::{DisplayMessage, DisplayMessageKind};

use super::{
    markdown::{render_markdown_lines, render_transcript_lines, ASSISTANT_CONTENT_PREFIX},
    render::{
        composer_cursor_column, input_tail_lines, render_composer_lines, render_footer_lines,
        render_live_history_lines, render_shortcut_overlay, visible_width, SegmentStyle,
        StyledLine,
    },
    state::{FooterMode, InteractiveTuiState, ModelChoice, RunMode},
    terminal::{handle_key_event, handle_mouse_event, visible_body_bounds, InteractiveTuiEvent},
};

#[test]
fn interactive_state_applies_streaming_text() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("hello");
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::RunStart {
            run_id: RunId::from_string("run_test"),
            conversation_id: ConversationId::from_string("conversation_test"),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartStart(PartStart {
                index: 0,
                part_kind: "text".to_string(),
            }),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::text(0, "hello\nworld")),
        },
    ));
    assert!(state.body.iter().any(|line| line.contains("hello")));
    assert!(state.body.iter().any(|line| line.contains("world")));
    assert_eq!(state.phase, "streaming");
}

#[test]
fn codex_style_opening_renders_header_composer_and_footer() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.workspace_dir = "/tmp/starweaver".to_string();
    let history = render_live_history_lines(&state, 80);
    assert!(line_texts(&history)
        .iter()
        .any(|line| line.starts_with("╭")));
    assert!(has_segment(&history, "Starweaver", SegmentStyle::BOLD));
    assert!(line_texts(&history)
        .iter()
        .any(|line| line.contains("model:")));
    assert!(line_texts(&history)
        .iter()
        .any(|line| line.contains("/model")));
    assert!(line_texts(&history)
        .iter()
        .any(|line| line.contains("directory:")));
    assert!(line_texts(&history)
        .iter()
        .any(|line| line.contains("To get started")));

    let composer = render_composer_lines(&state, 80);
    let composer_text = line_texts(&composer).join("\n");
    assert!(composer_text.contains("[scroll] > Ask Starweaver to do anything"));

    let footer_lines = render_footer_lines(&state, 120);
    let footer_text = line_texts(&footer_lines).join("\n");
    assert!(footer_text.contains("[Steering messages will appear here during agent execution]"));
    assert!(footer_text.contains(" ACT  | State: IDLE"));
    assert!(footer_text.contains("Model: local_echo"));
    assert!(footer_text.contains("Context: 0%"));
    assert!(footer_text.contains("Enter: Send"));
    assert!(footer_text.contains("Up/Down: History"));
    assert!(footer_text.contains("PageUp/PageDown/Mouse: Scroll"));
    assert!(has_segment(&footer_lines, " ACT ", SegmentStyle::MODE_BG));
    assert!(footer_lines.iter().any(|line| line
        .segments
        .iter()
        .any(|segment| segment.style.contains(SegmentStyle::STATUS_BG))));
}

#[test]
fn codex_style_shortcut_overlay_matches_footer_model() {
    let overlay = render_shortcut_overlay(100);
    let text = line_texts(&overlay).join("\n");
    assert!(text.contains("Available Commands"));
    assert!(text.contains("/help"));
    assert!(text.contains("Print this help in the transcript"));
    assert!(text.contains("/model [profile]"));
    assert!(text.contains("/goal <task>"));
    assert!(text.contains("Run task toward a verified goal until complete"));
    assert!(text.contains("Key Bindings"));
    assert!(text.contains("Ctrl+C"));
    assert!(text.contains("Scroll transcript"));
    assert!(text.contains("Mouse wheel"));
}

#[test]
#[allow(clippy::too_many_lines)]
fn key_handler_covers_input_modes_history_scroll_and_interrupt() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    assert_eq!(handle_key_event(&mut state, key_char('h')), None);
    assert_eq!(handle_key_event(&mut state, key_char('i')), None);
    assert_eq!(state.input, "hi");
    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::Enter)),
        Some(InteractiveTuiEvent::Submit("hi".to_string()))
    );
    assert!(state.input.is_empty());

    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Up)), None);
    assert_eq!(state.input, "hi");
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Down)), None);
    assert!(state.input.is_empty());

    assert_eq!(handle_key_event(&mut state, key_char('x')), None);
    assert_eq!(
        handle_key_event(&mut state, key_modified('o', KeyModifiers::CONTROL)),
        None
    );
    assert_eq!(state.input, "x\n");
    assert_eq!(
        handle_key_event(&mut state, key_modified('u', KeyModifiers::CONTROL)),
        None
    );
    assert!(state.input.is_empty());

    assert!(!FooterMode::is_help());
    assert_eq!(handle_key_event(&mut state, key_char('?')), None);
    assert_eq!(state.input, "?");
    state.input.clear();

    state.input = "/goal".to_string();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert!(state
        .body
        .iter()
        .any(|line| line == "[SYS] Usage: /goal <task description>"));

    state.set_custom_commands(std::collections::BTreeMap::new());
    state.input = "/COMMIT staged files".to_string();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert!(state.input.is_empty());
    assert!(state
        .body
        .iter()
        .any(|line| line.contains("Unknown command: /COMMIT staged files")));

    state.input = "/goal migrate tui".to_string();
    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::Enter)),
        Some(InteractiveTuiEvent::Submit("migrate tui".to_string()))
    );
    assert!(state.goal_active);
    assert!(state
        .body
        .iter()
        .any(|line| line.contains("[Goal] Starting goal mode")));
    let goal_footer = line_texts(&render_footer_lines(&state, 120)).join("\n");
    assert!(goal_footer.contains("Goal: 0/10"));
    state.context_tokens = Some(10_000);
    state.context_window = Some(200_000);
    let context_footer = line_texts(&render_footer_lines(&state, 120)).join("\n");
    assert!(context_footer.contains("Context: 5%"));

    state
        .body
        .extend((0..30).map(|line| format!("line {line}")));
    let rendered_len = render_live_history_lines(&state, 80).len();
    assert_eq!(
        visible_body_bounds(&state, rendered_len, 10),
        (rendered_len - 10, rendered_len)
    );
    state.push_history("old prompt".to_string());
    state.input.clear();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Up)), None);
    assert_eq!(state.input, "old prompt");
    assert!(state.is_at_bottom());
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Down)), None);
    assert!(state.input.is_empty());
    assert!(state.is_at_bottom());
    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::PageUp)),
        None
    );
    assert!(!state.is_at_bottom());
    let (start_after_page_up, end_after_page_up) = visible_body_bounds(&state, rendered_len, 10);
    assert!(start_after_page_up < rendered_len - 10);
    assert_eq!(end_after_page_up, start_after_page_up + 10);
    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::PageDown)),
        None
    );
    assert!(state.is_at_bottom());
    assert_eq!(
        handle_mouse_event(&mut state, mouse_event(MouseEventKind::ScrollUp)),
        Some(InteractiveTuiEvent::Redraw)
    );
    assert!(!state.is_at_bottom());
    assert_eq!(
        handle_mouse_event(&mut state, mouse_event(MouseEventKind::ScrollDown)),
        Some(InteractiveTuiEvent::Redraw)
    );
    assert!(state.is_at_bottom());
    state.scroll_offset = 3;
    assert_eq!(
        handle_key_event(&mut state, key_modified('l', KeyModifiers::CONTROL)),
        None
    );
    assert!(state.is_at_bottom());

    assert_eq!(state.run_mode, RunMode::Act);
    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::BackTab)),
        None
    );
    assert_eq!(state.run_mode, RunMode::Plan);

    state.running = true;
    state.input = "steer now".to_string();
    let event = handle_key_event(&mut state, key_code(KeyCode::Enter));
    let steering = match event {
        Some(InteractiveTuiEvent::Steer(steering)) => steering,
        other => panic!("expected steering event, got {other:?}"),
    };
    assert_eq!(steering.id, "steer_0");
    assert_eq!(steering.text, "steer now");
    assert!(state.input.is_empty());
    assert!(line_texts(&render_footer_lines(&state, 120))
        .join("\n")
        .contains(">>> steer now"));

    state.input = "queued".to_string();
    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::Tab)),
        Some(InteractiveTuiEvent::Queue("queued".to_string()))
    );
    assert!(state.input.is_empty());
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Esc)), None);
    assert!(state.phase.contains("run active"));
    assert_eq!(
        handle_key_event(&mut state, key_modified('c', KeyModifiers::CONTROL)),
        Some(InteractiveTuiEvent::Cancel)
    );
    assert_eq!(state.status, "INTERRUPT");
    assert!(state.cancel_requested);
}

#[test]
fn scroll_bounds_move_visible_viewport_and_preserve_bottom_sentinel() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state
        .body
        .extend((0..50).map(|line| format!("line {line}")));
    assert_eq!(visible_body_bounds(&state, 50, 10), (40, 50));

    state.scroll_offset = 30;
    assert_eq!(visible_body_bounds(&state, 50, 10), (30, 40));

    state.scroll_offset = 0;
    assert_eq!(visible_body_bounds(&state, 50, 10), (0, 10));

    state.scroll_offset = 60;
    assert_eq!(visible_body_bounds(&state, 50, 10), (40, 50));

    state.scroll_to_bottom();
    assert!(state.is_at_bottom());
    assert_eq!(visible_body_bounds(&state, 50, 10), (40, 50));
}

#[test]
fn checkpoint_events_update_phase_without_output_noise() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let initial_lines = state.body.len();
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::Checkpoint {
            node: AgentExecutionNode::RunStart,
            step: 0,
        },
    ));
    assert_eq!(state.phase, "checkpoint:runstart");
    assert_eq!(state.body.len(), initial_lines);
}

#[test]
#[allow(clippy::too_many_lines)]
fn interactive_state_covers_runtime_event_branches() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("inspect tools");

    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::NodeStart {
            node: AgentExecutionNode::PrepareModelRequest,
            step: 0,
            status: RunStatus::Running,
        },
    ));
    assert_eq!(state.phase, "node:preparemodelrequest");

    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ModelRequest { step: 0 },
    ));
    assert_eq!(state.phase, "thinking");

    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartStart(PartStart {
                index: 1,
                part_kind: "thinking".to_string(),
            }),
        },
    ));
    assert_eq!(state.phase, "thinking");
    assert!(!state
        .body
        .iter()
        .any(|line| body_line_text(line).starts_with('>')));

    state.apply_stream_record(&AgentStreamRecord::new(
        3,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartEnd(PartEnd {
                index: 1,
                part_kind: Some("thinking".to_string()),
            }),
        },
    ));

    state.apply_stream_record(&AgentStreamRecord::new(
        4,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::FinalResult(Box::new(ModelResponse::text(
                "final text",
            ))),
        },
    ));
    assert_eq!(state.phase, "finalizing");
    assert!(body_has_line(&state, "final text"));

    state.apply_stream_record(&AgentStreamRecord::new(
        5,
        AgentStreamEvent::Custom {
            event: AgentEvent::new("custom_phase", json!({"ok": true})),
        },
    ));
    assert_eq!(state.phase, "custom_phase");

    let call = ToolCallPart {
        id: "call_1".to_string(),
        name: "lookup".to_string(),
        arguments: json!({"query": "starweaver"}).into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        6,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: call.clone(),
        },
    ));
    assert!(state
        .body
        .iter()
        .any(|line| line == "Tool call: lookup {\"query\":\"starweaver\"}"));

    state.apply_stream_record(&AgentStreamRecord::new(
        7,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                call.id.clone(),
                call.name,
                json!({"answer": "ok\nnext"}),
            ),
        },
    ));
    assert!(state
        .body
        .iter()
        .any(|line| line.contains("Tool result: lookup")));

    state.apply_stream_record(&AgentStreamRecord::new(
        8,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(call.id, "lookup", json!("permission denied"))
                .with_error(true),
        },
    ));
    assert!(state
        .body
        .iter()
        .any(|line| line == "Tool error: lookup permission denied"));

    state.apply_stream_record(&AgentStreamRecord::new(
        9,
        AgentStreamEvent::OutputRetry {
            retries: 2,
            prompt: "try again".to_string(),
        },
    ));
    assert_eq!(state.phase, "retry");
    assert!(state.body.iter().any(|line| line == "Output retry: 2"));

    state.apply_stream_record(&AgentStreamRecord::new(
        10,
        AgentStreamEvent::Suspended {
            node: AgentExecutionNode::ToolCall,
            reason: "approval required".to_string(),
        },
    ));
    assert_eq!(state.status, "WAITING");
    assert!(state
        .body
        .iter()
        .any(|line| line == "Suspended: approval required"));

    state.apply_stream_record(&AgentStreamRecord::new(
        11,
        AgentStreamEvent::NodeComplete {
            node: AgentExecutionNode::RunComplete,
            step: 1,
            status: RunStatus::Completed,
        },
    ));
    assert!(state.is_at_bottom());
}

#[test]
fn thinking_stream_delta_does_not_suppress_final_text() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("respond");
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartStart(PartStart {
                index: 0,
                part_kind: "thinking".to_string(),
            }),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::thinking(0, "reasoning")),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::FinalResult(Box::new(ModelResponse::text(
                "final answer",
            ))),
        },
    ));

    assert!(body_has_line(&state, "> reasoning"));
    assert!(body_has_line(&state, "final answer"));
}

#[test]
fn interleaved_thinking_part_does_not_mark_text_seen() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("respond");
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartStart(PartStart {
                index: 0,
                part_kind: "thinking".to_string(),
            }),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartStart(PartStart {
                index: 1,
                part_kind: "text".to_string(),
            }),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::thinking(0, "hidden chain")),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        3,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::FinalResult(Box::new(ModelResponse::text(
                "visible answer",
            ))),
        },
    ));

    assert!(state
        .body
        .iter()
        .any(|line| body_line_text(line) == "> hidden chain"));
    assert!(body_has_line(&state, "visible answer"));
}

#[test]
fn tool_call_from_model_response_and_tool_event_renders_once() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("use tool");
    let call = ToolCallPart {
        id: "call_once".to_string(),
        name: "lookup".to_string(),
        arguments: json!({"query":"once"}).into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ModelResponse {
            step: 0,
            response: ModelResponse {
                parts: vec![ModelResponsePart::ToolCall(call.clone())],
                usage: starweaver_core::Usage::default(),
                model_name: None,
                provider: None,
                finish_reason: None,
                timestamp: None,
                run_id: None,
                conversation_id: None,
                metadata: Metadata::default(),
            },
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolCall { step: 1, call },
    ));

    assert_eq!(
        state
            .body
            .iter()
            .filter(|line| line.starts_with("Tool call: lookup"))
            .count(),
        1
    );
}

#[test]
fn streaming_tool_call_delta_is_visible_and_deduped_by_final_call() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("use tool");
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartStart(PartStart {
                index: 2,
                part_kind: "tool_call".to_string(),
            }),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta {
                index: 2,
                delta: StreamDelta::ToolCallName {
                    name: "lookup".to_string(),
                },
            }),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta {
                index: 2,
                delta: StreamDelta::ToolCallArguments {
                    arguments_delta: "{\"query\":\"star".to_string(),
                },
            }),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        3,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta {
                index: 2,
                delta: StreamDelta::ToolCallArguments {
                    arguments_delta: "weaver\"}".to_string(),
                },
            }),
        },
    ));

    assert_eq!(state.phase, "tools");
    assert!(state
        .body
        .iter()
        .any(|line| line == "Tool call: lookup {\"query\":\"starweaver\"}"));

    let final_call = ToolCallPart {
        id: "call_streamed".to_string(),
        name: "lookup".to_string(),
        arguments: json!({"query":"starweaver"}).into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        4,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::FinalResult(Box::new(ModelResponse {
                parts: vec![ModelResponsePart::ToolCall(final_call.clone())],
                usage: starweaver_core::Usage::default(),
                model_name: None,
                provider: None,
                finish_reason: None,
                timestamp: None,
                run_id: None,
                conversation_id: None,
                metadata: Metadata::default(),
            })),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        5,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: final_call,
        },
    ));

    assert_eq!(
        state
            .body
            .iter()
            .filter(|line| line.starts_with("Tool call: lookup"))
            .count(),
        1
    );
}

#[test]
fn streamed_text_after_tool_return_starts_new_assistant_line() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("use tool");
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new("call_after_tool", "lookup", json!({"answer": "ok"})),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ModelStream {
            step: 1,
            event: ModelResponseStreamEvent::PartStart(PartStart {
                index: 3,
                part_kind: "text".to_string(),
            }),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ModelStream {
            step: 1,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::text(3, "final answer")),
        },
    ));

    let Some(tool_index) = state
        .body
        .iter()
        .position(|line| line.starts_with("Tool result: lookup"))
    else {
        panic!("tool result should be visible");
    };
    let Some(text_index) = state
        .body
        .iter()
        .position(|line| body_line_text(line) == "final answer")
    else {
        panic!("streamed text should be visible");
    };
    assert!(text_index > tool_index);
    assert!(!state
        .body
        .iter()
        .any(|line| line.contains("Tool result: lookup") && line.contains("final answer")));
}

#[test]
fn model_command_opens_picker_selects_directly_and_blocks_while_running() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.set_model_choices(vec![
        ModelChoice {
            profile: "general".to_string(),
            label: Some("General".to_string()),
            model_id: "local_echo".to_string(),
            model_settings: None,
            model_cfg: None,
            context_window: None,
            source: "builtin".to_string(),
        },
        ModelChoice {
            profile: "coding".to_string(),
            label: Some("Coding".to_string()),
            model_id: "openai-responses:gpt-5".to_string(),
            model_settings: Some("openai_responses_medium".to_string()),
            model_cfg: Some("gpt5_270k".to_string()),
            context_window: Some(270_000),
            source: "config".to_string(),
        },
    ]);

    state.input = "/model".to_string();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert!(state.model_picker_visible());
    assert_eq!(state.model_picker_index(), 0);
    assert!(!state.body.iter().any(|line| line == "[SYS] Model profiles"));
    let picker_text = line_texts(&render_footer_lines(&state, 120)).join("\n");
    assert!(picker_text.contains("Model Profiles"));
    assert!(picker_text.contains("Enter: select"));
    assert!(picker_text.contains("general"));
    assert!(picker_text.contains("coding"));
    assert!(picker_text.contains("Highlighted config"));
    assert!(picker_text.contains("model:"));
    assert!(picker_text.contains("local_echo"));
    assert!(picker_text.contains("model_settings:"));
    assert!(picker_text.contains("model_cfg:"));
    assert!(picker_text.contains("context:"));

    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Up)), None);
    assert_eq!(state.model_picker_index(), 1);
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Down)), None);
    assert_eq!(state.model_picker_index(), 0);
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Down)), None);
    assert_eq!(state.model_picker_index(), 1);
    let coding_picker_text = line_texts(&render_footer_lines(&state, 120)).join("\n");
    assert!(coding_picker_text.contains("openai-responses:gpt-5"));
    assert!(coding_picker_text.contains("openai_responses_medium"));
    assert!(coding_picker_text.contains("gpt5_270k"));
    assert!(coding_picker_text.contains("270000 tokens"));
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert!(!state.model_picker_visible());
    assert_eq!(state.profile, "coding");
    assert_eq!(state.model, "Coding (openai-responses:gpt-5)");
    assert!(state
        .body
        .iter()
        .any(|line| line == "[SYS] Switched model to Coding (openai-responses:gpt-5)"));

    state.input = "/model".to_string();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert!(state.model_picker_visible());
    assert_eq!(state.model_picker_index(), 1);
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Esc)), None);
    assert!(!state.model_picker_visible());
    assert_eq!(state.profile, "coding");

    state.input = "/model general".to_string();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert_eq!(state.profile, "general");
    assert_eq!(state.model, "General (local_echo)");

    state.input = "/model missing".to_string();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert_eq!(state.profile, "general");
    assert!(state
        .body
        .iter()
        .any(|line| line == "[SYS] Unknown model profile: missing"));
    assert!(state.body.iter().any(|line| line.contains("/model coding")));

    state.running = true;
    state.input = "/model coding".to_string();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert_eq!(state.profile, "general");
    assert!(!state.model_picker_visible());
    assert!(state.body.iter().any(|line| {
        line == "[SYS] Model selection is available after the current run finishes."
    }));
}

#[test]
fn interactive_state_covers_model_response_finish_and_failure() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("respond");
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ModelResponse {
            step: 0,
            response: ModelResponse {
                parts: vec![
                    ModelResponsePart::Text {
                        text: "answer".to_string(),
                    },
                    ModelResponsePart::Thinking {
                        text: "reasoning".to_string(),
                        signature: Some("sig".to_string()),
                    },
                    ModelResponsePart::ToolCall(ToolCallPart {
                        id: "call_2".to_string(),
                        name: "search".to_string(),
                        arguments: json!({}).into(),
                    }),
                ],
                usage: starweaver_core::Usage {
                    requests: 1,
                    input_tokens: 1_000,
                    output_tokens: 500,
                    total_tokens: 1_500,
                    tool_calls: 0,
                },
                model_name: None,
                provider: None,
                finish_reason: None,
                timestamp: None,
                run_id: None,
                conversation_id: None,
                metadata: Metadata::default(),
            },
        },
    ));
    assert_eq!(state.phase, "tools");
    assert!(body_has_line(&state, "answer"));
    assert!(body_has_line(&state, "> reasoning"));
    assert!(state.body.iter().any(|line| line == "Tool call: search"));
    assert_eq!(state.context_percent_label(), "1%");
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ModelResponse {
            step: 1,
            response: ModelResponse {
                parts: Vec::new(),
                usage: starweaver_core::Usage {
                    requests: 1,
                    input_tokens: 6_000,
                    output_tokens: 500,
                    total_tokens: 6_500,
                    tool_calls: 0,
                },
                model_name: None,
                provider: None,
                finish_reason: None,
                timestamp: None,
                run_id: None,
                conversation_id: None,
                metadata: Metadata::default(),
            },
        },
    ));
    assert_eq!(state.context_percent_label(), "4%");

    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::RunComplete {
            run_id: RunId::from_string("run_test"),
            output: "unused because streamed".to_string(),
        },
    ));
    assert_eq!(state.phase, "completed");
    assert!(!state
        .body
        .iter()
        .any(|line| body_line_text(line) == "unused because streamed"));

    state.finish_run(Some("session_complete".to_string()));
    assert_eq!(state.session_id.as_deref(), Some("session_complete"));
    assert_eq!(state.status, "IDLE");

    state.fail_run("boom");
    assert_eq!(state.status, "ERROR");
    assert_eq!(state.phase, "failed");
    assert!(state.body.iter().any(|line| line == "Error: boom"));
}

#[test]
fn markdown_renderer_styles_common_blocks_and_inline_spans() {
    let lines = vec![
        "# Title".to_string(),
        "- **bold** and *em* plus `code`".to_string(),
        "> [docs](https://example.com)".to_string(),
        "```rust".to_string(),
        "fn main() {}".to_string(),
        "```".to_string(),
        "---".to_string(),
    ];
    let rendered = render_markdown_lines(&lines, 12);
    assert_eq!(rendered[0].segments[0].text, "Title");
    assert!(rendered[0].segments[0].style.contains(SegmentStyle::BOLD));
    assert!(rendered[0].segments[0]
        .style
        .contains(SegmentStyle::UNDERLINED));
    assert!(rendered.iter().any(|line| line
        .segments
        .first()
        .is_some_and(|segment| segment.text == "• ")));
    assert!(has_segment(&rendered, "bold", SegmentStyle::BOLD));
    assert!(has_segment(&rendered, "em", SegmentStyle::ITALIC));
    assert!(has_segment(&rendered, "code", SegmentStyle::CYAN));
    assert!(has_segment(&rendered, "│ ", SegmentStyle::GREEN));
    assert!(has_segment(&rendered, "docs", SegmentStyle::UNDERLINED));
    assert!(rendered.iter().any(|line| line
        .segments
        .first()
        .is_some_and(|segment| segment.text == "│ fn main() {}"
            && segment.style.contains(SegmentStyle::CYAN))));
    assert!(rendered.iter().any(|line| line
        .segments
        .first()
        .is_some_and(|segment| segment.text == "────────────")));
}

#[test]
fn markdown_renderer_covers_extended_codex_blocks() {
    let lines = vec![
        "1. ordered item wraps to continuation text".to_string(),
        "2. second".to_string(),
        String::new(),
        "~~gone~~".to_string(),
        "line one  ".to_string(),
        "line two".to_string(),
        String::new(),
        "| left | right |".to_string(),
        "| --- | --- |".to_string(),
        "| a | b |".to_string(),
        String::new(),
        "<span>html</span>".to_string(),
        String::new(),
        "![alt](https://example.com/image.png)".to_string(),
        String::new(),
        "- [x] task".to_string(),
        String::new(),
        "```".to_string(),
        "plain code".to_string(),
        "```".to_string(),
        String::new(),
        "#### Small".to_string(),
    ];
    let rendered = render_markdown_lines(&lines, 18);
    let text = line_texts(&rendered).join("\n");
    assert!(text.contains("1. ordered item"));
    assert!(text.contains("2. second"));
    assert!(text.contains("left"));
    assert!(text.contains("right"));
    assert!(text.contains("<span>html</span>"));
    assert!(text.contains("╭─ code"));
    assert!(text.contains("│ plain code"));
    assert!(text.contains("Small"));
    assert!(has_segment(&rendered, "gone", SegmentStyle::DIM));
    assert!(has_segment(&rendered, "Small", SegmentStyle::ITALIC));
    assert!(rendered
        .iter()
        .any(|line| line.segments.iter().any(|segment| segment.text == "   ")));

    let empty = render_markdown_lines(&[], 0);
    assert_eq!(line_texts(&empty), vec![String::new()]);
}

#[test]
fn transcript_renderer_covers_status_styles_and_plain_lines() {
    let lines = vec![
        "plain setup".to_string(),
        "Assistant:".to_string(),
        "hello".to_string(),
        "Tool call: lookup".to_string(),
        "Assistant:".to_string(),
        "world".to_string(),
        "Tool result: ok".to_string(),
        "Tool error: lookup permission denied".to_string(),
        "Thinking: hidden".to_string(),
        "Error: boom".to_string(),
        "Suspended: wait".to_string(),
        "Output retry: 1".to_string(),
        String::new(),
    ];
    let rendered = render_transcript_lines(&lines, 1);
    let text = line_texts(&rendered).join("\n");
    assert!(text.contains("plain setup"));
    assert!(text.contains("hello"));
    assert!(text.contains("world"));
    assert!(text.contains("Calling: lookup"));
    assert!(text.contains("Complete: ok"));
    assert!(text.contains("x Error: lookup | Error: permission denied"));
    assert!(text.contains("✕ error boom"));
    assert!(text.contains("◌ thinking hidden"));
    assert!(text.contains("◷ waiting wait"));
    assert!(text.contains("↻ retry 1"));
}

#[test]
fn transcript_renderer_renders_only_assistant_markdown() {
    let lines = vec![
        "User: # raw prompt".to_string(),
        "Assistant:".to_string(),
        "# Title".to_string(),
        "- item".to_string(),
        format!("{ASSISTANT_CONTENT_PREFIX}User: literal assistant content"),
        format!("{ASSISTANT_CONTENT_PREFIX}Assistant:"),
        format!("{ASSISTANT_CONTENT_PREFIX}Tool call: literal text"),
        "Run completed: run_test status=completed".to_string(),
    ];
    let rendered = render_transcript_lines(&lines, 40);
    assert!(rendered
        .iter()
        .any(|line| line_text(line) == "› # raw prompt"));
    assert!(has_segment(&rendered, "Title", SegmentStyle::BOLD));
    assert!(rendered.iter().any(|line| line
        .segments
        .first()
        .is_some_and(|segment| segment.text == "• ")));
    assert!(rendered
        .iter()
        .any(|line| line_text(line).contains("User: literal assistant content")));
    assert!(rendered
        .iter()
        .any(|line| line_text(line).contains("Assistant:")));
    assert!(rendered
        .iter()
        .any(|line| line_text(line).contains("Tool call: literal text")));
    assert!(rendered
        .iter()
        .any(|line| line_text(line) == "  ✓ completed run_test status=completed"));
}

#[test]
fn input_tail_preserves_trailing_empty_line() {
    assert_eq!(input_tail_lines("a\nb\n", 3), vec!["a", "b", ""]);
    assert_eq!(input_tail_lines("", 3), vec![""]);
}

#[test]
fn help_command_prints_help_to_body_without_submitting() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    assert_eq!(handle_key_event(&mut state, key_char('/')), None);
    assert_eq!(handle_key_event(&mut state, key_char('h')), None);
    assert_eq!(handle_key_event(&mut state, key_char('e')), None);
    assert_eq!(handle_key_event(&mut state, key_char('l')), None);
    assert_eq!(handle_key_event(&mut state, key_char('p')), None);
    assert!(!InteractiveTuiState::help_panel_visible());
    let footer_text = line_texts(&render_footer_lines(&state, 100)).join("\n");
    assert!(!footer_text.contains("Available Commands"));
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert!(state.input.is_empty());
    assert!(!FooterMode::is_help());
    assert!(state.input_status_text().contains("help"));
    assert!(state.body.iter().any(|line| line == "Starweaver TUI help"));
    assert!(state.body.iter().any(|line| line == "Commands"));
    assert!(state
        .body
        .iter()
        .any(|line| line == "  /help             Show this help"));
    assert!(state
        .body
        .iter()
        .any(|line| line == "  !<command>        Run a shell command inline"));
    assert!(state.body.iter().any(|line| line == "Shortcuts"));
    assert!(!state
        .body
        .iter()
        .any(|line| line.starts_with("[SYS] /help")));
}

#[test]
fn bang_command_prints_natural_shell_transcript() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));

    state.input = "!".to_string();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert!(state.input.is_empty());
    assert_eq!(state.input_status_text(), "shell");
    assert!(state.body.iter().any(|line| {
        line == "[SYS] Shell command usage: !<command> (example: !git status --short)"
    }));

    state.input = "!printf 'hello\\n'".to_string();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert!(state.input.is_empty());
    assert!(state
        .body
        .iter()
        .any(|line| line == "Shell command: printf 'hello\\n'"));
    assert!(state.body.iter().any(|line| line == "Shell stdout:"));
    assert!(state.body.iter().any(|line| line == "  hello"));
    assert!(state
        .body
        .iter()
        .any(|line| line == "Shell completed: exit 0"));
}

#[test]
fn goal_mode_tracks_iterations_completion_and_max_iterations() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.input = "/goal migrate tui".to_string();
    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::Enter)),
        Some(InteractiveTuiEvent::Submit("migrate tui".to_string()))
    );
    let first = state.complete_goal_iteration("needs more work");
    match first {
        super::state::GoalIterationOutcome::Continue(prompt) => {
            assert!(prompt.contains("<objective>"));
            assert!(prompt.contains("migrate tui"));
            assert!(prompt.contains("[GOAL_COMPLETE]"));
        }
        other => panic!("expected continuation, got {other:?}"),
    }
    assert_eq!(state.goal_iteration, 1);
    assert!(state.goal_active);
    assert!(state
        .body
        .iter()
        .any(|line| line == "[SYS] [Goal] Iteration 1/10"));

    let complete = state.complete_goal_iteration("[GOAL_COMPLETE]\nfinished");
    assert_eq!(complete, super::state::GoalIterationOutcome::Complete);
    assert!(!state.goal_active);
    assert!(state
        .body
        .iter()
        .any(|line| line == "[SYS] [Goal] Task completed in 2 iteration(s)"));

    let mut max_state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    max_state.input = "/goal hard task".to_string();
    assert_eq!(
        handle_key_event(&mut max_state, key_code(KeyCode::Enter)),
        Some(InteractiveTuiEvent::Submit("hard task".to_string()))
    );
    max_state.goal_max_iterations = 1;
    assert_eq!(
        max_state.complete_goal_iteration("still open"),
        super::state::GoalIterationOutcome::MaxIterations
    );
    assert!(!max_state.goal_active);
    assert!(max_state
        .body
        .iter()
        .any(|line| line.contains("Reached max iterations")));
}

#[test]
fn fullscreen_composer_tracks_paste_images_and_steering_status() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.apply_paste("/tmp/screenshot.png");
    assert_eq!(state.pasted_image_count(), 1);
    assert!(state.input_status_text().contains("image attached"));
    let composer_text = line_texts(&render_composer_lines(&state, 80)).join("\n");
    assert!(composer_text.contains("image"));
    assert!(composer_text.contains("screenshot.png"));
    let footer_text = line_texts(&render_footer_lines(&state, 120)).join("\n");
    assert!(footer_text.contains("images:1"));
    assert!(footer_text.contains("Ctrl+U: Clear"));

    let submitted = state.take_submission_prompt().unwrap();
    assert_eq!(submitted, "[image: /tmp/screenshot.png]");
    assert_eq!(state.pasted_image_count(), 0);

    state.running = true;
    state.apply_paste("tighten this section");
    assert_eq!(state.input_mode_label(), "STEER");
    let steer_footer = line_texts(&render_footer_lines(&state, 120)).join("\n");
    assert!(steer_footer.contains("STEER"));
    assert!(steer_footer.contains("[Steering messages will appear here during agent execution]"));
    let steering = state.take_steering_prompt().unwrap();
    assert_eq!(steering.id, "steer_0");
    assert_eq!(steering.text, "tighten this section");
    assert!(state.input_status_text().contains("steer sent"));
    let pending_steer_footer = line_texts(&render_footer_lines(&state, 120)).join("\n");
    assert!(pending_steer_footer.contains(">>> tighten this section"));
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "steering_received",
                json!({"id": "steer_0", "text": "tighten this section"}),
            ),
        },
    ));
    let acked_steer_footer = line_texts(&render_footer_lines(&state, 120)).join("\n");
    assert!(acked_steer_footer.contains("[v] tighten this section"));
}

#[test]
#[allow(clippy::too_many_lines)]
fn snapshot_from_parts_covers_status_and_pending_counts() {
    let session_id = SessionId::from_string("session_snapshot");
    let run_id = RunId::from_string("run_snapshot");
    let mut approved = ApprovalRecord::new(
        "approval_done",
        session_id.clone(),
        run_id.clone(),
        "action_done",
        "shell",
    );
    approved.status = starweaver_session::ApprovalStatus::Approved;
    let approvals = vec![
        ApprovalRecord::new(
            "approval_pending",
            session_id.clone(),
            run_id.clone(),
            "action_pending",
            "edit",
        ),
        approved,
    ];
    let mut completed_deferred = DeferredToolRecord::new(
        "deferred_done",
        session_id.clone(),
        run_id.clone(),
        "call_done",
        "worker",
    );
    completed_deferred.status = ExecutionStatus::Completed;
    let mut waiting_deferred = DeferredToolRecord::new(
        "deferred_waiting",
        session_id.clone(),
        run_id.clone(),
        "call_waiting",
        "worker",
    );
    waiting_deferred.status = ExecutionStatus::Waiting;
    let deferred = vec![completed_deferred, waiting_deferred];
    let messages = vec![
        DisplayMessage::new(
            0,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::AssistantTextDelta,
        )
        .with_payload(json!({"delta": "hello"})),
        DisplayMessage::new(
            1,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::AssistantTextDelta,
        )
        .with_preview(" world"),
        DisplayMessage::new(
            2,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::ToolCallStart,
        )
        .with_payload(json!({"tool_name": "lookup", "arguments": {"query": "starweaver"}})),
        DisplayMessage::new(
            3,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::ToolCallStart,
        )
        .with_preview("fallback_tool"),
        DisplayMessage::new(
            4,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::ToolResult,
        )
        .with_preview("ok"),
        DisplayMessage::new(
            5,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::ToolResult,
        )
        .with_payload(json!({"content": "permission denied", "is_error": true})),
        DisplayMessage::new(
            6,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::RunCompleted,
        ),
        DisplayMessage::new(
            7,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::RunFailed,
        ),
        DisplayMessage::new(8, session_id, run_id, DisplayMessageKind::RunCancelled),
    ];
    let snapshot = super::snapshot::TuiSnapshot::from_parts(
        "session_snapshot".to_string(),
        messages,
        &approvals,
        &deferred,
    );
    assert_eq!(snapshot.messages, 9);
    assert_eq!(snapshot.pending_approvals, 1);
    assert_eq!(snapshot.pending_deferred, 1);
    assert_eq!(snapshot.assistant_text, "hello world");
    assert_eq!(
        snapshot.tool_calls,
        vec![
            "lookup {\"query\":\"starweaver\"}",
            "fallback_tool",
            "result:ok",
            "result:error:permission denied"
        ]
    );
    assert_eq!(snapshot.terminal_status.as_deref(), Some("cancelled"));
    let text = snapshot.render_text();
    assert!(text.contains("pending_approvals=1"));
    assert!(text.contains("terminal_status=cancelled"));
    assert!(text.contains("Assistant"));
    assert!(text.contains("- result:ok"));
    assert!(text.contains("- result:error:permission denied"));

    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.set_snapshot(&snapshot);
    assert_eq!(state.status, "CANCELLED");
    assert_eq!(state.phase, "replay");
    assert!(state.body.iter().any(|line| line == "Assistant:"));
    assert!(state.body.iter().any(|line| line == "Tool result: ok"));
    assert!(state
        .body
        .iter()
        .any(|line| line == "Tool error: permission denied"));
    assert!(state
        .body
        .iter()
        .any(|line| line == "Run completed: session_snapshot status=cancelled"));
}

#[test]
fn render_helpers_cover_footer_and_truncation_branches() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.set_profile("coding", "gpt-test");
    state.workspace_dir = "/a/very/long/path/with/中文/segments/and/file".to_string();
    let tiny_history = render_live_history_lines(&state, 3);
    assert!(line_texts(&tiny_history)
        .iter()
        .any(|line| line.contains("To get started")));

    let normal_history = render_live_history_lines(&state, 44);
    let normal_text = line_texts(&normal_history).join("\n");
    assert!(normal_text.contains("gpt-test"));
    assert!(normal_text.contains("…"));

    let mut running = state.clone();
    running.running = true;
    running.phase = "streaming".to_string();
    let running_footer = line_texts(&render_footer_lines(&running, 90)).join("\n");
    assert!(running_footer.contains("Ctrl+C: Interrupt"));
    assert!(running_footer.contains("RUNNING"));
    assert!(running_footer.contains("Running..."));

    let mut drafting = state.clone();
    drafting.input = "draft".to_string();
    let drafting_footer = line_texts(&render_footer_lines(&drafting, 90)).join("\n");
    assert!(drafting_footer.contains("Ctrl+U: Clear"));

    let composer = render_composer_lines(&drafting, 6);
    assert!(line_texts(&composer)
        .iter()
        .any(|line| line.contains("draft")));

    let footer_overlay_text = line_texts(&render_footer_lines(&state, 72)).join("\n");
    assert!(!footer_overlay_text.contains("Available Commands"));

    let overlay = render_shortcut_overlay(12);
    let overlay_text = line_texts(&overlay).join("\n");
    assert!(overlay_text.contains("Available Commands"));
    assert!(overlay_text.contains("/help"));

    let mut body_state = state.clone();
    body_state.body = vec![
        "Assistant:".to_string(),
        "# Rendered".to_string(),
        "plain".to_string(),
    ];
    let body_text = line_texts(&render_live_history_lines(&body_state, 40)).join("\n");
    assert!(body_text.contains("Rendered"));
    assert!(body_text.contains("plain"));
}

#[test]
#[allow(clippy::too_many_lines)]
fn key_handler_covers_quit_and_history_edges() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state
        .body
        .extend((0..5).map(|index| format!("line {index}")));
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Tab)), None);
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert_eq!(
        handle_key_event(&mut state, key_modified('p', KeyModifiers::CONTROL)),
        None
    );
    assert_eq!(
        handle_key_event(&mut state, key_modified('n', KeyModifiers::CONTROL)),
        None
    );
    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::BackTab)),
        None
    );
    assert_eq!(state.run_mode, RunMode::Plan);
    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::BackTab)),
        None
    );
    assert_eq!(state.run_mode, RunMode::Act);
    assert_eq!(
        handle_key_event(&mut state, key_modified('r', KeyModifiers::CONTROL)),
        None
    );
    assert_eq!(
        handle_key_event(&mut state, key_modified('x', KeyModifiers::CONTROL)),
        None
    );
    assert_eq!(
        handle_key_event(&mut state, key_modified('A', KeyModifiers::SHIFT)),
        None
    );
    assert_eq!(state.input, "A");
    state.input.clear();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Left)), None);
    assert!(state.input.is_empty());
    assert_eq!(
        handle_key_event(&mut state, key_modified('c', KeyModifiers::ALT)),
        None
    );
    assert_eq!(state.input, "c");
    state.input.clear();
    assert_eq!(
        handle_key_event(
            &mut state,
            key_modified('c', KeyModifiers::CONTROL | KeyModifiers::SHIFT)
        ),
        Some(InteractiveTuiEvent::Quit)
    );

    state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Up)), None);
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Down)), None);

    state.input = "draft".to_string();
    state.push_history("one".to_string());
    state.push_history("two".to_string());
    state.previous_history();
    assert_eq!(state.input, "two");
    state.previous_history();
    assert_eq!(state.input, "one");
    state.next_history();
    assert_eq!(state.input, "two");
    state.next_history();
    assert_eq!(state.input, "draft");

    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::Backspace)),
        None
    );
    assert_eq!(state.input, "draf");
    assert_eq!(handle_key_event(&mut state, key_char('z')), None);
    assert!(!FooterMode::is_help());

    state.input.clear();
    assert_eq!(
        handle_key_event(&mut state, key_char('q')),
        Some(InteractiveTuiEvent::Quit)
    );
    state.running = true;
    assert_eq!(handle_key_event(&mut state, key_char('q')), None);
    assert!(state.phase.contains("run active"));
    assert_eq!(
        handle_key_event(&mut state, key_modified('d', KeyModifiers::CONTROL)),
        None
    );
    state.running = false;
    assert_eq!(
        handle_key_event(&mut state, key_modified('d', KeyModifiers::CONTROL)),
        Some(InteractiveTuiEvent::Quit)
    );

    let mut escape_state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    assert_eq!(
        handle_key_event(&mut escape_state, key_code(KeyCode::Esc)),
        Some(InteractiveTuiEvent::Quit)
    );
    let mut ctrl_c_state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    ctrl_c_state.input = "clear me".to_string();
    assert_eq!(
        handle_key_event(&mut ctrl_c_state, key_modified('c', KeyModifiers::CONTROL)),
        None
    );
    assert!(ctrl_c_state.input.is_empty());
    assert_eq!(
        handle_key_event(&mut ctrl_c_state, key_modified('c', KeyModifiers::CONTROL)),
        Some(InteractiveTuiEvent::Quit)
    );
}

#[test]
fn terminal_width_helpers_handle_wide_characters() {
    assert_eq!(visible_width("中文a"), 5);
    assert_eq!(composer_cursor_column(&["中文a".to_string()]), 16);

    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.workspace_dir = "/workspace/项目/很长很长很长很长".to_string();
    let history = render_live_history_lines(&state, 32);
    assert!(line_texts(&history).iter().any(|line| line.contains("…")));
    for line in history.iter().filter(|line| {
        let text = line_text(line);
        text.starts_with('╭') || text.starts_with('│') || text.starts_with('╰')
    }) {
        assert!(line.visible_width() <= 32);
    }

    let rendered = render_markdown_lines(&["中文中文 hello".to_string()], 8);
    assert!(rendered.len() > 1);
    assert!(rendered.iter().all(|line| line.visible_width() <= 8));
}

fn body_has_line(state: &InteractiveTuiState, expected: &str) -> bool {
    state
        .body
        .iter()
        .any(|line| body_line_text(line) == expected)
}

fn body_line_text(line: &str) -> &str {
    line.strip_prefix(ASSISTANT_CONTENT_PREFIX).unwrap_or(line)
}

fn has_segment(lines: &[StyledLine], text: &str, style: u16) -> bool {
    lines.iter().any(|line| {
        line.segments
            .iter()
            .any(|segment| segment.text == text && segment.style.contains(style))
    })
}

fn line_texts(lines: &[StyledLine]) -> Vec<String> {
    lines.iter().map(line_text).collect()
}

fn line_text(line: &StyledLine) -> String {
    line.segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<String>()
        .trim_end()
        .to_string()
}

fn key_char(ch: char) -> KeyEvent {
    key_code(KeyCode::Char(ch))
}

fn key_code(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn key_modified(ch: char, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent {
        code: KeyCode::Char(ch),
        modifiers,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn mouse_event(kind: MouseEventKind) -> MouseEvent {
    MouseEvent {
        kind,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    }
}

#[test]
fn steering_guard_displays_as_steering_not_retry() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("respond");
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::SteeringGuard {
            step: 1,
            prompt: "continue with steering".to_string(),
        },
    ));

    assert_eq!(state.phase, "steering");
    assert!(state
        .body
        .iter()
        .any(|line| line == "Steering update pending; continuing run."));
    assert!(!state.body.iter().any(|line| line.contains("Output retry")));
}

#[test]
fn model_response_thinking_and_tool_call_are_visible() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("respond");
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ModelResponse {
            step: 1,
            response: ModelResponse {
                parts: vec![
                    ModelResponsePart::Thinking {
                        text: "inspect tools".to_string(),
                        signature: None,
                    },
                    ModelResponsePart::ToolCall(ToolCallPart {
                        id: "call_1".to_string(),
                        name: "lookup".to_string(),
                        arguments: json!({"query": "starweaver"}).into(),
                    }),
                    ModelResponsePart::Text {
                        text: "done".to_string(),
                    },
                ],
                usage: starweaver_core::Usage::default(),
                model_name: None,
                provider: None,
                finish_reason: None,
                timestamp: None,
                run_id: None,
                conversation_id: None,
                metadata: Metadata::default(),
            },
        },
    ));

    assert!(state
        .body
        .iter()
        .any(|line| body_line_text(line) == "> inspect tools"));
    assert!(state
        .body
        .iter()
        .any(|line| line == "Tool call: lookup {\"query\":\"starweaver\"}"));
    assert!(body_has_line(&state, "done"));
}
