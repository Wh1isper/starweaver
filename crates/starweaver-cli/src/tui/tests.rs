#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{collections::BTreeMap, ffi::OsStr, path::Path};

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseEvent, MouseEventKind,
};
use serde_json::json;
use starweaver_context::{AgentEvent, TASK_SNAPSHOT_EVENT_KIND};
use starweaver_core::{AgentId, ConversationId, Metadata, RunId, SessionId, TaskId};
use starweaver_model::{
    ModelResponse, ModelResponsePart, ModelResponseStreamEvent, PartDelta, PartEnd, PartStart,
    ProviderPartInfo, StreamDelta, ToolCallPart, ToolReturnPart,
};
use starweaver_runtime::{
    AgentExecutionNode, AgentStreamEvent, AgentStreamRecord, AgentStreamSource, RunStatus,
};
use starweaver_session::{ApprovalRecord, DeferredToolRecord, ExecutionStatus};
use starweaver_stream::{DisplayMessage, DisplayMessageKind};
use starweaver_usage::{PricingEstimate, Usage, UsageAgentTotal, UsageSnapshot};

use crate::{
    args::TuiRenderMode, prompt_input::PromptAttachment, slash_commands::SlashCommandDefinition,
};

use super::{
    markdown::{
        ASSISTANT_CONTENT_PREFIX, CONCISE_TOOL_SUMMARY_PREFIX, render_markdown_lines,
        render_transcript_lines,
    },
    render::{
        SegmentStyle, StyledLine, color_output_enabled, composer_cursor_column,
        composer_cursor_position_wrapped, composer_input_width, input_tail_lines,
        input_viewport_lines, input_viewport_lines_wrapped, input_visual_line_count,
        render_composer_lines, render_footer_lines, render_live_history_lines,
        render_shortcut_overlay, truncate_line, visible_width,
    },
    state::{
        FooterMode, InteractiveTuiState, ModelChoice, SessionChoice,
        display_lines_for_stream_record,
    },
    terminal::{
        InteractiveTuiEvent, handle_key_event, handle_mouse_event, responsive_frame_budget,
        should_capture_mouse, visible_body_bounds,
    },
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
fn projection_batch_materializes_stream_changes_once_at_the_end() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("hello");
    state.begin_projection_batch();
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
            event: ModelResponseStreamEvent::PartDelta(PartDelta::text(0, "batched output")),
        },
    ));

    assert!(
        !state
            .body
            .iter()
            .any(|line| line.contains("batched output"))
    );
    state.end_projection_batch();
    assert!(
        state
            .body
            .iter()
            .any(|line| line.contains("batched output"))
    );
}

#[test]
fn model_transport_selected_updates_status_bar_only() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("hello");
    let initial_body_len = state.body.len();

    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "model_transport_selected",
                json!({
                    "transport": "websocket",
                    "message": "model transport: websocket"
                }),
            ),
        },
    ));

    assert_eq!(state.body.len(), initial_body_len);
    let footer = line_texts(&render_footer_lines(&state, 120)).join("\n");
    assert!(footer.contains("Transport: websocket"));
}

#[test]
fn model_transport_fallback_updates_status_bar_and_transcript() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("hello");

    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "model_transport_fallback",
                json!({
                    "from": "websocket",
                    "to": "http",
                    "reason": "websocket_transport_error",
                    "detail": "websocket closed before response.completed",
                    "message": "model transport: websocket -> http fallback (websocket_transport_error)"
                }),
            ),
        },
    ));

    let footer = line_texts(&render_footer_lines(&state, 160)).join("\n");
    assert!(footer.contains("Transport: websocket -> http"));
    assert!(footer.contains("websocket_transport_error"));
    assert!(state.body.iter().any(|line| {
        line.contains("Transport: websocket -> http (websocket_transport_error)")
            && line.contains("websocket closed before response.completed")
    }));
}

#[test]
fn codex_style_opening_renders_header_composer_and_footer() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.workspace_dir = "/tmp/starweaver".to_string();
    let history = render_live_history_lines(&state, 80);
    assert!(
        line_texts(&history)
            .iter()
            .any(|line| line.starts_with("╭"))
    );
    assert!(has_segment(&history, "Starweaver", SegmentStyle::BOLD));
    assert!(
        line_texts(&history)
            .iter()
            .any(|line| line.contains("model:"))
    );
    assert!(
        line_texts(&history)
            .iter()
            .any(|line| line.contains("/model"))
    );
    assert!(
        line_texts(&history)
            .iter()
            .any(|line| line.contains("directory:"))
    );
    assert!(
        line_texts(&history)
            .iter()
            .any(|line| line.contains("To get started"))
    );

    let composer = render_composer_lines(&state, 80);
    let composer_text = line_texts(&composer).join("\n");
    assert!(composer_text.contains("> Ask Starweaver to do anything"));

    let footer_lines = render_footer_lines(&state, 120);
    let footer_text = line_texts(&footer_lines).join("\n");
    assert!(!footer_text.contains("Steering messages"));
    assert!(footer_text.contains(" READY  | State: IDLE"));
    assert!(footer_text.contains("Model: local_echo"));
    assert!(footer_text.contains("Context: 0%"));
    assert!(footer_text.contains("Enter: Send"));
    assert!(footer_text.contains("History"));
    assert!(footer_text.contains("Scroll"));
    assert!(has_segment(&footer_lines, " READY ", SegmentStyle::MODE_BG));
    assert!(footer_lines.iter().any(|line| {
        line.segments
            .iter()
            .any(|segment| segment.style.contains(SegmentStyle::STATUS_BG))
    }));
}

#[test]
fn codex_style_shortcut_overlay_matches_footer_model() {
    let overlay = render_shortcut_overlay(100);
    let text = line_texts(&overlay).join("\n");
    assert!(text.contains("Available Commands"));
    assert!(text.contains("/help"));
    assert!(text.contains("Print this help in the transcript"));
    assert!(text.contains("/model [profile]"));
    assert!(text.contains("/session [id]"));
    assert!(text.contains("/goal <task>"));
    assert!(text.contains("Run task toward a verified goal until complete"));
    assert!(text.contains("/paste-image"));
    assert!(text.contains("Attach image from system clipboard"));
    assert!(text.contains("Key Bindings"));
    assert!(text.contains("Ctrl+C"));
    assert!(text.contains("Alt+Left/Right"));
    assert!(text.contains("Command+Left/Right"));
    assert!(text.contains("Scroll transcript"));
    assert!(text.contains("Mouse wheel"));
}

#[test]
fn shortcut_overlay_wraps_narrow_command_rows_without_truncating_content() {
    let overlay = render_shortcut_overlay(18);
    let texts = line_texts(&overlay);
    assert!(texts.iter().all(|line| visible_width(line) <= 18));

    let compact_text = texts.join("").replace(' ', "");
    assert!(compact_text.contains("/model[profile]Openselectororselectmodelprofile"));
    assert!(compact_text.contains("Ctrl+VAttachimagefromsystemclipboard"));
}

#[test]
#[allow(clippy::too_many_lines)]
fn key_handler_covers_input_modes_history_scroll_and_interrupt() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    assert_eq!(handle_key_event(&mut state, key_char('h')), None);
    assert_eq!(handle_key_event(&mut state, key_char('i')), None);
    assert_eq!(state.input, "hi");
    assert_eq!(
        submit_text(handle_key_event(&mut state, key_code(KeyCode::Enter))),
        Some("hi".to_string())
    );
    assert!(state.input.is_empty());

    assert_eq!(
        handle_key_event(&mut state, key_modified('p', KeyModifiers::CONTROL)),
        None
    );
    assert_eq!(state.input, "hi");
    assert_eq!(
        handle_key_event(&mut state, key_modified('n', KeyModifiers::CONTROL)),
        None
    );
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
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "[SYS] Usage: /goal <task description>")
    );

    state.set_custom_commands(BTreeMap::new());
    state.input = "/COMMIT staged files".to_string();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert!(state.input.is_empty());
    assert!(
        state
            .body
            .iter()
            .any(|line| line.contains("Unknown command: /COMMIT staged files"))
    );

    let mut custom_commands = BTreeMap::new();
    let command = SlashCommandDefinition {
        name: "commit".to_string(),
        prompt: "Write a clear git commit.".to_string(),
        description: Some("Create a commit".to_string()),
        aliases: vec!["ci".to_string()],
    };
    custom_commands.insert("commit".to_string(), command.clone());
    custom_commands.insert("ci".to_string(), command);
    state.set_custom_commands(custom_commands);
    state.input = "/CI staged files".to_string();
    assert_eq!(
        submit_text(handle_key_event(&mut state, key_code(KeyCode::Enter))),
        Some("/CI staged files".to_string())
    );
    assert!(
        state
            .body
            .iter()
            .any(|line| line.contains("Expanded /commit custom command (alias /ci)"))
    );
    assert_eq!(
        state.take_pending_submission_display_prompt(),
        Some("Write a clear git commit.\n\nUser instruction: staged files".to_string())
    );

    state.input = "/help".to_string();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert!(
        state
            .body
            .iter()
            .any(|line| line.contains("Custom commands"))
    );
    assert!(state.body.iter().any(|line| {
        line.contains("/commit [instruction]") && line.contains("Create a commit")
    }));

    state.input = "/session session_test".to_string();
    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::Enter)),
        Some(InteractiveTuiEvent::Session(Some(
            "session_test".to_string()
        )))
    );
    assert!(state.input.is_empty());

    assert_eq!(
        handle_key_event(&mut state, key_modified('v', KeyModifiers::CONTROL)),
        Some(InteractiveTuiEvent::PasteImage)
    );
    state.input = "/paste-image".to_string();
    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::Enter)),
        Some(InteractiveTuiEvent::PasteImage)
    );
    assert!(state.input.is_empty());

    state.input = "/goal migrate tui".to_string();
    assert_eq!(
        submit_text(handle_key_event(&mut state, key_code(KeyCode::Enter))),
        Some("migrate tui".to_string())
    );
    assert!(state.goal_active);
    assert!(
        state
            .body
            .iter()
            .any(|line| line.contains("[Goal] Starting goal mode"))
    );
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
    state.update_render_metrics(rendered_len, 10);
    assert_eq!(
        visible_body_bounds(&state, rendered_len, 10),
        (rendered_len - 10, rendered_len)
    );
    state.push_history("old prompt".to_string());
    state.input.clear();
    assert_eq!(
        handle_key_event(&mut state, key_modified('p', KeyModifiers::CONTROL)),
        None
    );
    assert_eq!(state.input, "old prompt");
    assert!(state.is_at_bottom());
    assert_eq!(
        handle_key_event(&mut state, key_modified('n', KeyModifiers::CONTROL)),
        None
    );
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
    assert!(state.body.iter().any(|line| line == "Steering: steer now"));
    assert!(
        !line_texts(&render_footer_lines(&state, 120))
            .join("\n")
            .contains("steer now")
    );

    state.input = "/paste-image".to_string();
    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::Enter)),
        Some(InteractiveTuiEvent::PasteImage)
    );

    state.input = "queued".to_string();
    assert!(state.enter_sends());
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Tab)), None);
    assert!(!state.enter_sends());
    assert_eq!(state.input, "queued");
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert_eq!(state.input, "queued\n");
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Tab)), None);
    assert!(state.enter_sends());
    state.input.clear();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Esc)), None);
    assert!(state.selection_mode_visible());
    assert!(!should_capture_mouse(&state));
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Esc)), None);
    assert!(!state.selection_mode_visible());
    assert_eq!(
        handle_key_event(&mut state, key_modified('c', KeyModifiers::CONTROL)),
        Some(InteractiveTuiEvent::Cancel)
    );
    assert_eq!(state.status, "INTERRUPT");
    assert!(state.cancel_requested);

    let mut cursor_state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    cursor_state.input = "alpha beta\ngamma_delta".to_string();
    assert_eq!(
        handle_key_event(
            &mut cursor_state,
            key_code_modified(KeyCode::Left, KeyModifiers::ALT),
        ),
        None
    );
    assert_eq!(cursor_state.composer_cursor_byte(), "alpha beta\n".len());
    assert_eq!(
        handle_key_event(
            &mut cursor_state,
            key_code_modified(KeyCode::Left, KeyModifiers::CONTROL),
        ),
        None
    );
    assert_eq!(cursor_state.composer_cursor_byte(), "alpha ".len());
    assert_eq!(
        handle_key_event(
            &mut cursor_state,
            key_code_modified(KeyCode::Right, KeyModifiers::ALT),
        ),
        None
    );
    assert_eq!(cursor_state.composer_cursor_byte(), "alpha beta".len());
    assert_eq!(
        handle_key_event(
            &mut cursor_state,
            key_code_modified(KeyCode::Left, KeyModifiers::SUPER),
        ),
        None
    );
    assert_eq!(cursor_state.composer_cursor_byte(), 0);
    assert_eq!(
        handle_key_event(
            &mut cursor_state,
            key_code_modified(KeyCode::Right, KeyModifiers::META),
        ),
        None
    );
    assert_eq!(cursor_state.composer_cursor_byte(), "alpha beta".len());
    assert_eq!(
        handle_key_event(&mut cursor_state, key_modified('e', KeyModifiers::CONTROL),),
        None
    );
    assert_eq!(cursor_state.composer_cursor_byte(), "alpha beta".len());
    assert_eq!(
        handle_key_event(
            &mut cursor_state,
            key_code_modified(KeyCode::Right, KeyModifiers::CONTROL),
        ),
        None
    );
    assert_eq!(
        cursor_state.composer_cursor_byte(),
        "alpha beta\ngamma_delta".len()
    );
    assert_eq!(
        handle_key_event(&mut cursor_state, key_modified('a', KeyModifiers::CONTROL),),
        None
    );
    assert_eq!(cursor_state.composer_cursor_byte(), "alpha beta\n".len());
    assert_eq!(
        handle_key_event(&mut cursor_state, key_modified('b', KeyModifiers::ALT),),
        None
    );
    assert_eq!(cursor_state.composer_cursor_byte(), "alpha ".len());
    assert_eq!(
        handle_key_event(&mut cursor_state, key_modified('f', KeyModifiers::ALT),),
        None
    );
    assert_eq!(cursor_state.composer_cursor_byte(), "alpha beta".len());

    let mut running_overlay_state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    running_overlay_state.begin_run("long running prompt");
    running_overlay_state.open_selection_mode();
    assert!(running_overlay_state.selection_mode_visible());
    assert_eq!(
        handle_key_event(
            &mut running_overlay_state,
            key_modified('c', KeyModifiers::CONTROL),
        ),
        Some(InteractiveTuiEvent::Cancel)
    );
    assert!(running_overlay_state.cancel_requested);
    assert_eq!(running_overlay_state.status, "INTERRUPT");
    assert!(running_overlay_state.selection_mode_visible());
    assert_eq!(
        running_overlay_state
            .body
            .iter()
            .filter(|line| line.as_str() == "Interrupt requested. Cancelling active run.")
            .count(),
        1
    );
    assert_eq!(
        handle_key_event(
            &mut running_overlay_state,
            key_modified('c', KeyModifiers::CONTROL),
        ),
        Some(InteractiveTuiEvent::Cancel)
    );
    assert_eq!(
        running_overlay_state
            .body
            .iter()
            .filter(|line| line.as_str() == "Interrupt requested. Cancelling active run.")
            .count(),
        1
    );
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
fn streaming_events_preserve_scroll_while_selection_mode_is_active() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("stream while selecting");
    state
        .body
        .extend((0..20).map(|line| format!("line {line}")));
    state.scroll_offset = 3;

    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Esc)), None);
    assert!(state.selection_mode_visible());
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartStart(PartStart {
                index: 0,
                part_kind: "text".to_string(),
            }),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::text(0, "live delta")),
        },
    ));
    assert_eq!(state.scroll_offset, 3);
    assert!(!state.is_at_bottom());

    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Esc)), None);
    assert!(!state.selection_mode_visible());
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::text(0, " next")),
        },
    ));
    assert_eq!(state.scroll_offset, 3);
    assert!(!state.is_at_bottom());
    assert!(state.unread_output_lines > 0);
    let footer = line_texts(&render_footer_lines(&state, 100)).join("\n");
    assert!(footer.contains("Output paused"));
    assert!(footer.contains("new"));
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
    assert!(
        !state
            .body
            .iter()
            .any(|line| body_line_text(line).starts_with('>'))
    );
    assert!(!body_has_line(&state, "Thinking"));

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
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "Tool call: lookup {\"query\":\"starweaver\"}")
    );

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
    assert!(
        state
            .body
            .iter()
            .any(|line| line.contains("Tool result: lookup"))
    );

    state.apply_stream_record(&AgentStreamRecord::new(
        8,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(call.id, "lookup", json!("permission denied"))
                .with_error(true),
        },
    ));
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "Tool error: lookup permission denied")
    );

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
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "Suspended: approval required")
    );

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
fn edit_tool_return_renders_structured_diff() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let edit_call = ToolCallPart {
        id: "edit_1".to_string(),
        name: "edit".to_string(),
        arguments: json!({
            "file_path": "src/lib.rs",
            "old_string": "fn old() {}",
            "new_string": "fn new() {}"
        })
        .into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: edit_call.clone(),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(edit_call.id, "edit", json!("updated")),
        },
    ));

    assert!(body_has_line(&state, "Tool result: edit"));
    assert!(body_has_line(&state, "  Editing file: src/lib.rs"));
    assert!(body_has_line(&state, "    @@ -1,1 +1,1 @@"));
    assert!(body_has_line(&state, "    -fn old() {}"));
    assert!(body_has_line(&state, "    +fn new() {}"));

    let rendered = render_transcript_lines(&state.body, 80);
    assert!(has_segment(&rendered, "src/lib.rs", SegmentStyle::CYAN));
    assert!(has_segment(&rendered, "    -", SegmentStyle::RED));
    assert!(has_segment(&rendered, "    +", SegmentStyle::GREEN));
}

#[test]
fn edit_tool_diff_focuses_changed_hunk_and_preserves_eof_newline() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let old_lines = (0..30)
        .map(|index| format!("line {index}"))
        .chain(["old".to_string(), "tail".to_string()])
        .collect::<Vec<_>>()
        .join("\n");
    let new_lines = (0..30)
        .map(|index| format!("line {index}"))
        .chain(["new".to_string(), "tail".to_string()])
        .collect::<Vec<_>>()
        .join("\n");
    let call = ToolCallPart {
        id: "edit_hunk".to_string(),
        name: "edit".to_string(),
        arguments: json!({
            "file_path": "src/lib.rs",
            "old_string": old_lines,
            "new_string": new_lines
        })
        .into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: call.clone(),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(call.id, "edit", json!("updated")),
        },
    ));

    assert!(body_has_line(
        &state,
        "     ... (28 unchanged lines before)"
    ));
    assert!(body_has_line(&state, "    -old"));
    assert!(body_has_line(&state, "    +new"));
    assert!(!body_has_line(&state, "     line 0"));

    let mut newline_state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let newline_call = ToolCallPart {
        id: "edit_newline".to_string(),
        name: "edit".to_string(),
        arguments: json!({
            "file_path": "src/lib.rs",
            "old_string": "fn main() {}",
            "new_string": "fn main() {}\n"
        })
        .into(),
    };
    newline_state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: newline_call.clone(),
        },
    ));
    newline_state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(newline_call.id, "edit", json!("updated")),
        },
    ));

    assert!(body_has_line(&newline_state, "    +<EOF newline>"));
}

#[test]
fn edit_tool_return_falls_back_to_result_metadata_without_cached_arguments() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                "edit_missing",
                "edit",
                json!({"file_path": "src/lib.rs", "edited": true}),
            ),
        },
    ));

    assert!(body_has_line(&state, "Tool result: edit"));
    assert!(body_has_line(&state, "  Editing file: src/lib.rs"));
    assert!(body_has_line(&state, "  Status: edited"));
}

#[test]
fn multi_edit_summary_counts_only_first_empty_old_string_as_new_file() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let call = ToolCallPart {
        id: "multi_edit_1".to_string(),
        name: "multi_edit".to_string(),
        arguments: json!({
            "file_path": "src/lib.rs",
            "edits": [
                {"old_string": "", "new_string": "created"},
                {"old_string": "", "new_string": "invalid follow-up"}
            ]
        })
        .into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: call.clone(),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(call.id, "multi_edit", json!({"edited": true})),
        },
    ));

    assert!(body_has_line(
        &state,
        "  Summary: 2 edits (1 new file, 1 modification, 0 replace-all operations)"
    ));
    assert!(body_has_line(&state, "  Edit #2: Content modification"));
    assert!(body_has_line(&state, "    Empty match string"));
}

#[test]
fn summarize_tool_return_renders_summary_handoff() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                "summarize_1",
                "summarize",
                json!({
                    "operation": "summarize",
                    "payload": {
                        "content": "Implemented compact rendering behavior.\nNext: run tests.",
                        "auto_load_files": ["AGENTS.md", "crates/starweaver-cli/src/tui/state.rs"]
                    }
                }),
            ),
        },
    ));

    assert!(body_has_line(&state, "Tool result: summarize"));
    assert!(body_has_line(
        &state,
        "  Summary: Progress summarized, continuing with fresh context"
    ));
    assert!(body_has_line(
        &state,
        "    │ Implemented compact rendering behavior."
    ));
    assert!(body_has_line(
        &state,
        "  Files to inspect: AGENTS.md, crates/starweaver-cli/src/tui/state.rs"
    ));

    let rendered = render_transcript_lines(&state.body, 100);
    assert!(has_segment(
        &rendered,
        "Progress summarized, continuing with fresh context",
        SegmentStyle::GREEN
    ));
    assert!(has_segment(
        &rendered,
        "AGENTS.md, crates/starweaver-cli/src/tui/state.rs",
        SegmentStyle::CYAN
    ));
}

#[test]
fn compact_custom_events_render_status_lines() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::Custom {
            event: AgentEvent::new("compaction_started", json!({"message_count": 50})),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "starweaver.compaction_completed",
                json!({
                    "payload": {
                        "original_message_count": 50,
                        "compacted_message_count": 12,
                        "summary": "Kept implementation notes."
                    }
                }),
            ),
        },
    ));

    assert!(body_has_line(&state, "Context compacting 50 messages..."));
    assert!(body_has_line(&state, "Context compacted"));
    assert!(body_has_line(
        &state,
        "  Summary: 50 -> 12 messages (76% reduction)"
    ));
    assert!(body_has_line(&state, "    │ Kept implementation notes."));

    let long_error = r#"provider status 400: {"error":{"message":"[openai-subs-workspace] invalid request: request body contained unsupported cache setting","type":"invalid_request_error","param":"cache_control","code":"bad_request_tail_marker"}}"#;
    state.apply_stream_record(&AgentStreamRecord::new(
        3,
        AgentStreamEvent::Custom {
            event: AgentEvent::new("compact_failed", json!({"message": long_error})),
        },
    ));

    let rendered_error = render_transcript_lines(&state.body, 72);
    let rendered_error_text = line_texts(&rendered_error).join("\n");
    assert!(rendered_error_text.contains("provider status 400"));
    assert!(rendered_error_text.contains("bad_request_tail_marker"));
    assert!(!rendered_error_text.contains('…'));

    let rendered = render_transcript_lines(&state.body, 100);
    assert!(has_segment(
        &rendered,
        "  Context compacting 50 messages...",
        SegmentStyle::YELLOW
    ));
}

#[test]
fn compact_custom_events_render_full_summary_content_without_truncation() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.set_render_mode(TuiRenderMode::Concise);
    let long_line = format!("{}END_MARKER", "compact-summary-".repeat(24));
    let lines = (1..=14)
        .map(|index| format!("line-{index}: {long_line}"))
        .collect::<Vec<_>>();
    let content = lines.join("\n");

    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "compaction_completed",
                json!({
                    "payload": {
                        "compacted_message_count": 12,
                        "summary": content
                    }
                }),
            ),
        },
    ));

    assert!(body_has_line(&state, "Context compacted"));
    assert!(body_has_line(
        &state,
        &("    │ line-14: ".to_string() + &long_line)
    ));
    assert!(!state.body.iter().any(|line| line.contains('…')));
    assert!(!state.body.iter().any(|line| line.contains("more lines")));

    let rendered = render_transcript_lines(&state.body, 72);
    let rendered_text = line_texts(&rendered).join("\n");
    let rendered_content = rendered_content_text(&rendered);
    assert!(rendered_content.contains("line-14:"));
    assert!(rendered_content.contains("END_MARKER"));
    assert!(!rendered_text.contains('…'));
    assert!(!rendered_text.contains("more lines"));
}

#[test]
fn handoff_custom_events_render_summary_lines() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::Custom {
            event: AgentEvent::new("handoff_start", json!({"messages": 42})),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "summary_complete",
                json!({"content": "Current state preserved.\nContinue from tests."}),
            ),
        },
    ));

    assert!(body_has_line(
        &state,
        "Summarizing progress (42 messages)..."
    ));
    assert!(body_has_line(&state, "Summary complete"));
    assert!(body_has_line(
        &state,
        "  Summary: Progress summarized, continuing with fresh context"
    ));
    assert!(body_has_line(&state, "    │ Current state preserved."));
}

#[test]
fn summarize_tool_return_renders_full_content_without_truncation() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.set_render_mode(TuiRenderMode::Concise);
    let long_line = format!("{}END_MARKER", "summary-content-".repeat(24));
    let lines = (1..=14)
        .map(|index| format!("line-{index}: {long_line}"))
        .collect::<Vec<_>>();
    let content = lines.join("\n");

    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                "summarize_1",
                "summarize",
                json!({
                    "operation": "summarize",
                    "payload": { "content": content }
                }),
            ),
        },
    ));

    assert!(body_has_line(&state, "Summary complete"));
    assert!(!body_has_line(&state, "Tool result: summarize"));
    assert!(body_has_line(
        &state,
        &("    │ line-14: ".to_string() + &long_line)
    ));
    assert!(!state.body.iter().any(|line| line.contains('…')));
    assert!(!state.body.iter().any(|line| line.contains("more lines")));

    let rendered = render_transcript_lines(&state.body, 72);
    let rendered_text = line_texts(&rendered).join("\n");
    let rendered_content = rendered_content_text(&rendered);
    assert!(rendered_content.contains("line-14:"));
    assert!(rendered_content.contains("END_MARKER"));
    assert!(!rendered_text.contains('…'));
    assert!(!rendered_text.contains("more lines"));
}

#[test]
fn shell_tool_return_renders_full_command_card_and_output_preview() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let command_tail = "0123456789".repeat(32);
    let long_command = format!(
        "printf 'alpha\\nbeta' && cargo test -p starweaver-cli tui::tests:: -- --nocapture && echo {command_tail}"
    );
    let call = ToolCallPart {
        id: "shell_1".to_string(),
        name: "shell_exec".to_string(),
        arguments: json!({
            "command": long_command,
            "cwd": "crates/starweaver-cli",
            "timeout_seconds": 120
        })
        .into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: call.clone(),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                call.id,
                "shell_exec",
                json!({
                    "return_code": 0,
                    "stdout": "alpha\nbeta\ngamma",
                    "stderr": "warning: none"
                }),
            ),
        },
    ));

    assert!(body_has_line(&state, "Tool result: shell_exec"));
    assert!(body_has_line(&state, "  Command:"));
    let command_start = state
        .body
        .iter()
        .position(|line| line == "  Command:")
        .expect("command section should render");
    let command_end = state
        .body
        .iter()
        .skip(command_start + 1)
        .position(|line| !line.starts_with("    │ "))
        .map_or(state.body.len(), |offset| command_start + 1 + offset);
    let rendered_command = state.body[command_start + 1..command_end]
        .iter()
        .map(|line| line.trim_start_matches("    │ "))
        .collect::<String>();
    assert_eq!(rendered_command, long_command);
    assert!(body_has_line(&state, "  Cwd: crates/starweaver-cli"));
    assert!(body_has_line(&state, "  Status: exit 0"));
    assert!(body_has_line(&state, "  stdout:"));
    assert!(body_has_line(&state, "    │ alpha"));
    assert!(body_has_line(&state, "    │ beta"));
    assert!(body_has_line(&state, "  stderr:"));
    assert!(body_has_line(&state, "    │ warning: none"));

    let rendered = render_transcript_lines(&state.body, 120);
    let rendered_text = line_texts(&rendered).join("\n");
    assert!(rendered_text.contains("Complete: shell_exec"));
    assert!(rendered_text.contains("0123456789"));
}

#[test]
fn shell_tool_return_truncates_long_stdout_and_stderr_previews() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let stdout = (0..10)
        .map(|index| format!("stdout {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let stderr = (0..8)
        .map(|index| format!("stderr {index}"))
        .collect::<Vec<_>>()
        .join("\n");

    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                "shell_long",
                "shell_exec",
                json!({
                    "command": "for i in $(seq 1 10); do echo $i; done",
                    "return_code": 0,
                    "stdout": stdout,
                    "stderr": stderr,
                    "stdout_file_path": "/tmp/stdout.log",
                    "stderr_file_path": "/tmp/stderr.log"
                }),
            ),
        },
    ));

    assert!(body_has_line(&state, "    │ stdout 0"));
    assert!(body_has_line(&state, "    │ stdout 5"));
    assert!(body_has_line(&state, "    │ ... (4 more lines)"));
    assert!(!body_has_line(&state, "    │ stdout 6"));
    assert!(body_has_line(&state, "    │ stderr 5"));
    assert!(body_has_line(&state, "    │ ... (2 more lines)"));
    assert!(!body_has_line(&state, "    │ stderr 6"));
    assert!(body_has_line(&state, "  stdout_file_path: /tmp/stdout.log"));
    assert!(body_has_line(&state, "  stderr_file_path: /tmp/stderr.log"));
}

#[test]
fn shell_tool_call_and_error_render_background_args_duration_and_context_errors() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let call = ToolCallPart {
        id: "shell_bg".to_string(),
        name: "shell_exec".to_string(),
        arguments: json!({
            "background": true,
            "command": "cargo test -p starweaver-cli --lib",
            "environment": null,
            "timeout_seconds": 0
        })
        .into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: call.clone(),
        },
    ));
    let mut metadata = Metadata::default();
    metadata.insert("duration_ms".to_string(), json!(0));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                call.id,
                "shell_exec",
                json!({
                    "error": "tool shell_exec user error: ProcessShellHandle is missing from AgentContext",
                    "kind": "user_error"
                }),
            )
            .with_error(true)
            .with_metadata(metadata),
        },
    ));

    assert!(body_has_line(&state, "  Duration: 0ms"));
    let body_text = state.body.join("\n");
    assert!(body_text.contains("background"));
    assert!(body_text.contains("environment"));
    assert!(body_text.contains("timeout_seconds"));
    assert!(body_text.contains("cargo test -p starweaver-cli --lib"));
    let rendered_text = line_texts(&render_transcript_lines(&state.body, 200)).join("\n");
    assert!(rendered_text.contains("Calling: shell_exec"));
    assert!(rendered_text.contains("cargo test -p starweaver-cli --lib"));
    assert!(rendered_text.contains("x Error: shell_exec"));
    assert!(rendered_text.contains("ProcessShellHandle"));
    assert!(rendered_text.contains("AgentContext"));
    assert!(rendered_text.contains("Duration: 0ms"));
}

#[test]
fn tool_result_previews_escape_control_characters() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                "shell_escape",
                "shell_exec",
                json!({
                    "command": "printf '\u{1b}[31mred'",
                    "return_code": 0,
                    "stdout": "\u{1b}[31mred\u{7}"
                }),
            ),
        },
    ));

    let text = state.body.join("\n");
    assert!(text.contains("\\x1b[31mred"));
    assert!(text.contains("\\x07"));
    assert!(!text.contains('\u{1b}'));
    assert!(!text.contains('\u{7}'));
}

#[test]
fn generic_tool_return_uses_multiline_content_preview() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                "lookup_1",
                "lookup",
                json!("first useful line\nsecond useful line\nthird useful line"),
            ),
        },
    ));

    assert!(body_has_line(&state, "Tool result: lookup"));
    assert!(body_has_line(&state, "    │ first useful line"));
    assert!(body_has_line(&state, "    │ second useful line"));
    assert!(body_has_line(&state, "    │ third useful line"));

    let rendered_text = line_texts(&render_transcript_lines(&state.body, 42)).join("\n");
    assert!(rendered_text.contains("Complete: lookup"));
    assert!(rendered_text.contains("first useful line"));
    assert!(rendered_text.contains("second useful line"));
}

#[test]
fn view_tool_return_renders_file_preview() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                "view_1",
                "view",
                json!({
                    "file_path": "README.md",
                    "content": "line 1\nline 2",
                    "metadata": {"truncation_info": {"content_truncated": false}}
                }),
            ),
        },
    ));
    assert!(body_has_line(&state, "Tool result: view"));
    assert!(body_has_line(&state, "  Viewing file: README.md"));
    assert!(body_has_line(&state, "    │ line 1"));
    assert!(body_has_line(&state, "    │ line 2"));
    assert!(body_has_line(
        &state,
        "  Truncation: {\"content_truncated\":false}"
    ));

    let rendered = render_transcript_lines(&state.body, 80);
    assert!(has_segment(&rendered, "README.md", SegmentStyle::CYAN));
    assert!(has_segment(&rendered, "line 1", SegmentStyle::CYAN));

    let view_call = ToolCallPart {
        id: "view_2".to_string(),
        name: "view".to_string(),
        arguments: json!({"file_path": "plain.txt"}).into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: view_call.clone(),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        3,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(view_call.id, "view", json!("plain content")),
        },
    ));
    assert!(body_has_line(&state, "  Viewing file: plain.txt"));
    assert!(body_has_line(&state, "    │ plain content"));

    state.apply_stream_record(&AgentStreamRecord::new(
        4,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                "view_3",
                "view",
                json!({
                    "content": "page content",
                    "metadata": {
                        "file_path": "paged.txt",
                        "truncation_info": {"lines_truncated": true}
                    }
                }),
            ),
        },
    ));
    assert!(body_has_line(&state, "  Viewing file: paged.txt"));
    assert!(body_has_line(&state, "    │ page content"));
    assert!(body_has_line(
        &state,
        "  Truncation: {\"lines_truncated\":true}"
    ));

    state.apply_stream_record(&AgentStreamRecord::new(
        5,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                "view_4",
                "view",
                json!({"file_path": "empty.txt", "content": ""}),
            ),
        },
    ));
    assert!(body_has_line(&state, "  Viewing file: empty.txt"));
    assert!(body_has_line(&state, "    Empty file"));
}

#[test]
fn write_tool_return_renders_preview_and_styles() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let call = ToolCallPart {
        id: "write_1".to_string(),
        name: "write".to_string(),
        arguments: json!({
            "file_path": "src/generated.rs",
            "content": "pub fn generated() {}\n",
            "mode": "w"
        })
        .into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: call.clone(),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(call.id, "write", json!({"written": true})),
        },
    ));

    assert!(body_has_line(&state, "Tool result: write"));
    assert!(body_has_line(&state, "  Writing file: src/generated.rs"));
    assert!(body_has_line(&state, "  Mode: overwrite"));
    assert!(body_has_line(&state, "  Edit #1: File content"));
    assert!(body_has_line(&state, "    +pub fn generated() {}"));
    assert!(body_has_line(&state, "  Status: written"));

    let rendered = render_transcript_lines(&state.body, 80);
    assert!(has_segment(
        &rendered,
        "src/generated.rs",
        SegmentStyle::CYAN
    ));
    assert!(has_segment(&rendered, "    +", SegmentStyle::GREEN));
    assert!(has_segment(&rendered, "written", SegmentStyle::GREEN));
}

#[test]
fn special_tool_rendering_truncates_lines_and_releases_arguments() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let long_line = "a".repeat(400);
    let edit_call = ToolCallPart {
        id: "edit_long".to_string(),
        name: "edit".to_string(),
        arguments: json!({
            "file_path": "long.txt",
            "old_string": "old",
            "new_string": long_line
        })
        .into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: edit_call.clone(),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(edit_call.id.clone(), "edit", json!("updated")),
        },
    ));
    assert!(body_has_line(&state, "Tool result: edit"));
    assert!(body_has_line(&state, "  Editing file: long.txt"));
    assert!(body_has_line(&state, "    -old"));
    assert!(
        state
            .body
            .iter()
            .any(|line| line.starts_with("    +") && line.contains('…'))
    );
    assert!(
        !state
            .body
            .iter()
            .any(|line| line.starts_with("    +") && line.len() > 280)
    );

    state.apply_stream_record(&AgentStreamRecord::new(
        3,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(edit_call.id, "edit", json!("duplicate")),
        },
    ));
    assert_eq!(
        state
            .body
            .iter()
            .filter(|line| line.as_str() == "Tool result: edit")
            .count(),
        2
    );
    assert!(body_has_line(&state, "  Result: duplicate"));
}

#[test]
fn task_create_tool_renders_dedicated_full_output_without_generic_noise() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let call = ToolCallPart {
        id: "task_create_1".to_string(),
        name: "task_create".to_string(),
        arguments: json!({
            "subject": "Review CLI structure before release",
            "description": "Review every CLI command, configuration path, and release note before cutting the release.",
            "active_form": "Reviewing CLI structure before release"
        })
        .into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: call.clone(),
        },
    ));

    let mut duration_metadata = Metadata::default();
    duration_metadata.insert("duration_ms".to_string(), json!(1));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                call.id,
                "task_create",
                json!({
                    "operation": "task_create",
                    "payload": {
                        "id": "42",
                        "subject": "Review CLI structure before release",
                        "description": "Review every CLI command, configuration path, and release note before cutting the release.",
                        "active_form": "Reviewing CLI structure before release"
                    }
                }),
            )
            .with_user_content(json!("Task created: Review CLI structure before release"))
            .with_metadata(duration_metadata),
        },
    ));

    assert!(body_has_line(&state, "Task request: task_create"));
    assert!(body_has_line(&state, "Task result: task_create"));
    assert!(body_has_line(&state, "  Summary: Task created"));
    assert!(body_has_line(&state, "  Task ID: 42"));
    assert!(body_has_line(
        &state,
        "  Subject: Review CLI structure before release"
    ));
    assert!(body_has_line(
        &state,
        "  Description: Review every CLI command, configuration path, and release note before cutting the release."
    ));
    assert!(body_has_line(
        &state,
        "  Active form: Reviewing CLI structure before release"
    ));
    let body = state.body.join("\n");
    assert!(!body.contains("Tool call: task_create"));
    assert!(!body.contains("Tool result: task_create"));
    assert!(!body.contains("Duration: 1ms"));

    let rendered_text = line_texts(&render_transcript_lines(&state.body, 100)).join("\n");
    assert!(rendered_text.contains("Task request: task_create"));
    assert!(rendered_text.contains("Task result: task_create"));
    assert!(!rendered_text.contains("Calling: task_create"));
    assert!(!rendered_text.contains("Complete: task_create"));
}

#[test]
fn task_list_tool_return_renders_progress_when_statuses_are_present() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                "task_1",
                "task_list",
                json!("#1 [completed] Done\n#2 [in_progress: Working] Work"),
            ),
        },
    ));
    assert!(body_has_line(&state, "Task result: task_list"));
    assert!(body_has_line(&state, "  Output:"));
    assert!(body_has_line(&state, "    │ #1 [completed] Done"));
    assert!(body_has_line(&state, "  Progress: 1/2 (1 in progress)"));

    let progress_count_before_placeholder = state
        .body
        .iter()
        .filter(|line| line.starts_with("  Progress:"))
        .count();
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                "task_2",
                "task_list",
                json!({"operation": "task_list", "payload": {}}),
            )
            .with_user_content(json!("Task list requested")),
        },
    ));
    assert!(body_has_line(&state, "  Summary: Task list requested"));
    assert_eq!(
        state
            .body
            .iter()
            .filter(|line| line.starts_with("  Progress:"))
            .count(),
        progress_count_before_placeholder
    );
}

#[test]
fn tool_duration_hitl_and_task_panels_render_runtime_metadata() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let mut duration_metadata = Metadata::default();
    duration_metadata.insert("duration_ms".to_string(), json!(1_500));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new("call_duration", "lookup", json!("ok"))
                .with_metadata(duration_metadata),
        },
    ));
    assert!(body_has_line(&state, "  Duration: 1.50s"));

    let mut approval_metadata = Metadata::default();
    approval_metadata.insert("control_flow".to_string(), json!("approval_required"));
    approval_metadata.insert(
        "approval".to_string(),
        json!({
            "command": "rm -rf target/tmp",
            "risk_level": "high",
            "reason": "destructive command needs review"
        }),
    );
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new("shell_call", "shell", json!("approval required"))
                .with_metadata(approval_metadata),
        },
    ));
    assert_eq!(state.status, "WAITING");
    assert!(state.pending_hitl().is_some());
    state.wait_run(Some("session_waiting".to_string()));
    assert_eq!(state.status, "WAITING");
    assert_eq!(state.phase, "waiting");
    assert_eq!(state.session_id.as_deref(), Some("session_waiting"));
    assert!(
        state.pending_hitl().is_some(),
        "durable waiting must retain approval details"
    );
    let footer = line_texts(&render_footer_lines(&state, 120)).join("\n");
    assert!(footer.contains("Tool Approval Required"));
    assert!(footer.contains("rm -rf target/tmp"));
    assert!(footer.contains("Approval required"));

    state.apply_stream_record(&AgentStreamRecord::new(
        3,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                TASK_SNAPSHOT_EVENT_KIND,
                json!({
                    "tasks": [
                        {"id": "1", "subject": "Done", "description": "", "status": "completed"},
                        {"id": "2", "subject": "Work", "description": "", "status": "in_progress", "active_form": "Working", "blocked_by": ["1"]}
                    ]
                }),
            ),
        },
    ));
    let footer = line_texts(&render_footer_lines(&state, 120)).join("\n");
    assert!(footer.contains("Tasks"));
    assert!(footer.contains("Progress: 1/2 (1 in progress)"));
    assert!(footer.contains("Done"));
    assert!(footer.contains("Work"));

    state.apply_stream_record(&AgentStreamRecord::new(
        5,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                "task_return",
                "task_list",
                json!({
                    "operation": "task_list",
                    "payload": {
                        "tasks": [
                            {"id": "tool", "subject": "Tool-return task", "description": "", "status": "completed"}
                        ]
                    }
                }),
            )
            .with_user_content(json!("#tool [completed] Tool-return task")),
        },
    ));
    let footer = line_texts(&render_footer_lines(&state, 120)).join("\n");
    assert!(footer.contains("Progress: 1/1"));
    assert!(footer.contains("Tool-return task"));
}

#[test]
fn begin_run_preserves_task_panel_items_for_next_prompt() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                TASK_SNAPSHOT_EVENT_KIND,
                json!({
                    "tasks": [
                        {"id": "1", "subject": "Existing task", "description": "", "status": "in_progress", "active_form": "Working"}
                    ]
                }),
            ),
        },
    ));

    state.begin_run("next prompt");

    let footer = line_texts(&render_footer_lines(&state, 120)).join("\n");
    assert!(footer.contains("Tasks"));
    assert!(footer.contains("Existing task"));
    assert!(footer.contains("Progress: 0/1 (1 in progress)"));
}

#[test]
fn clear_command_defers_context_reset_until_the_service_accepts_it() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.session_id = Some("session_clear".to_string());
    state.context_tokens = Some(42);
    state.body.push("old transcript".to_string());
    state.input = "/clear".to_string();

    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::Enter)),
        Some(InteractiveTuiEvent::Clear)
    );
    assert_eq!(state.session_id.as_deref(), Some("session_clear"));
    assert_eq!(state.body, ["old transcript"]);
    assert_eq!(state.context_tokens, Some(42));
    assert_eq!(state.input_status_text(), "clearing context");
}

#[test]
fn clear_during_a_run_emits_a_rejectable_event_without_leaving_stale_pending_state() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.session_id = Some("session_running".to_string());
    state.begin_run("active prompt");
    state.input = "/clear".to_string();

    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::Enter)),
        Some(InteractiveTuiEvent::Clear)
    );
    assert!(state.running);
    assert_eq!(state.session_id.as_deref(), Some("session_running"));
    assert_eq!(state.input_status_text(), "clearing context");
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
}

#[test]
fn accepted_clear_resets_conversation_state_and_preserves_process_state() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.session_id = Some("session_clear".to_string());
    let session_affinity_id = state.session_affinity_id.clone();
    state.context_tokens = Some(42);
    state.latest_request_total_tokens = Some(21);
    state.current_run_id = Some("run_clear".to_string());
    state.current_run_usage = Some(Usage {
        input_tokens: 7,
        output_tokens: 3,
        ..Usage::default()
    });
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "usage_snapshot",
                serde_json::to_value(UsageSnapshot {
                    run_id: "run_clear".to_string(),
                    model_usages: BTreeMap::from([(
                        "test:model".to_string(),
                        Usage {
                            input_tokens: 7,
                            output_tokens: 3,
                            total_tokens: 10,
                            ..Usage::default()
                        },
                    )]),
                    ..UsageSnapshot::default()
                })
                .unwrap(),
            ),
        },
    ));
    state.push_history("old prompt".to_string());
    state.attach_image(PromptAttachment::image(
        1,
        b"image-bytes".to_vec(),
        "image/png",
    ));
    state.goal_task = Some("old goal".to_string());
    state.goal_active = true;
    state.goal_iteration = 4;
    state.open_selection_mode();

    state.clear_context_view();

    assert!(state.session_id.is_none());
    assert!(state.body.is_empty());
    assert_eq!(state.context_tokens, None);
    assert_eq!(state.latest_request_total_tokens, None);
    assert_eq!(state.current_run_id, None);
    assert_eq!(state.current_run_usage, None);
    assert_eq!(state.pasted_image_count(), 0);
    assert!(state.history.is_empty());
    assert_eq!(state.history_index, None);
    assert!(state.history_draft.is_empty());
    assert!(!state.selection_mode_visible());
    assert!(!state.goal_active);
    assert_eq!(state.goal_task, None);
    assert_eq!(state.goal_iteration, 0);
    assert!(!state.running);
    assert_eq!(state.phase, "cleared");
    assert_eq!(state.input_status_text(), "context cleared");
    assert_eq!(state.session_affinity_id, session_affinity_id);

    state.input = "/cost".to_string();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert!(body_has_line(&state, "[SYS]   Input:  7 tokens"));
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

    assert!(
        state
            .body
            .iter()
            .any(|line| body_line_text(line) == "> hidden chain")
    );
    assert!(body_has_line(&state, "visible answer"));
}

#[test]
fn text_delta_after_unfinished_thinking_starts_visible_line() {
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
            event: ModelResponseStreamEvent::PartDelta(PartDelta::thinking(0, "hidden chain")),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartStart(PartStart {
                index: 1,
                part_kind: "text".to_string(),
            }),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        3,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::text(1, "visible answer")),
        },
    ));

    assert!(
        state
            .body
            .iter()
            .any(|line| body_line_text(line) == "> hidden chain")
    );
    assert!(body_has_line(&state, "visible answer"));
    assert!(
        !state
            .body
            .iter()
            .any(|line| body_line_text(line) == "> hidden chainvisible answer")
    );
}

#[test]
fn background_continuation_does_not_render_an_empty_user_prompt() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_background_continuation();

    assert!(state.running);
    assert!(body_has_line(
        &state,
        "Background subagent result received; continuing session."
    ));
    assert!(!body_has_line(&state, "User:"));
}

#[test]
fn begin_run_does_not_precreate_empty_assistant_transcript_item() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("respond");

    assert!(body_has_line(&state, "User: respond"));
    assert!(!body_has_line(&state, "Assistant:"));
    assert!(!body_has_line(&state, ""));
}

#[test]
fn text_after_thinking_preserves_event_order_in_normal_mode() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("respond");
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::thinking(0, "first reasoning")),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::text(1, "later answer")),
        },
    ));

    assert!(body_line_index(&state, "> first reasoning") < body_line_index(&state, "Assistant:"));
    assert!(body_line_index(&state, "> first reasoning") < body_line_index(&state, "later answer"));
}

#[test]
fn text_after_thinking_preserves_event_order_in_concise_mode() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.set_render_mode(TuiRenderMode::Concise);
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
            event: ModelResponseStreamEvent::PartDelta(PartDelta::thinking(0, "first reasoning")),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartStart(PartStart {
                index: 1,
                part_kind: "text".to_string(),
            }),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        3,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::text(1, "later answer")),
        },
    ));

    assert!(body_line_index(&state, "> first reasoning") < body_line_index(&state, "Assistant:"));
    assert!(body_line_index(&state, "> first reasoning") < body_line_index(&state, "later answer"));
}

#[test]
fn text_tool_text_preserves_event_order_without_merging_across_tool() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.set_render_mode(TuiRenderMode::Concise);
    state.begin_run("respond");
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::text(0, "before")),
        },
    ));
    let call = ToolCallPart {
        id: "call_between_text".to_string(),
        name: "lookup".to_string(),
        arguments: json!({"query":"order"}).into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: call.clone(),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(call.id, "lookup", json!("ok")),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        3,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::text(0, "after")),
        },
    ));

    assert!(
        body_line_index(&state, "before")
            < body_line_index(&state, "Called lookup {\"query\":\"order\"}")
    );
    assert!(
        body_line_index(&state, "Called lookup {\"query\":\"order\"}")
            < body_line_index(&state, "after")
    );
    assert!(!body_has_line(&state, "beforeafter"));
}

#[test]
fn thinking_tool_thinking_preserves_event_order_without_merging_across_tool() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.set_render_mode(TuiRenderMode::Concise);
    state.begin_run("respond");
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::thinking(0, "before")),
        },
    ));
    let call = ToolCallPart {
        id: "call_between_thinking".to_string(),
        name: "lookup".to_string(),
        arguments: json!({"query":"thinking"}).into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: call.clone(),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(call.id, "lookup", json!("ok")),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        3,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::thinking(0, "after")),
        },
    ));

    assert!(
        body_line_index(&state, "> before")
            < body_line_index(&state, "Called lookup {\"query\":\"thinking\"}")
    );
    assert!(
        body_line_index(&state, "Called lookup {\"query\":\"thinking\"}")
            < body_line_index(&state, "> after")
    );
    assert!(!body_has_line(&state, "> beforeafter"));
}

#[test]
fn text_context_text_preserves_event_order_without_merging_across_context_event() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("respond");
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::text(0, "before")),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::Custom {
            event: AgentEvent::new("summary_completed", json!({"content": "handoff"})),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::text(0, "after")),
        },
    ));

    assert!(body_line_index(&state, "before") < body_line_index(&state, "Summary complete"));
    assert!(body_line_index(&state, "Summary complete") < body_line_index(&state, "after"));
    assert!(!body_has_line(&state, "beforeafter"));
}

#[test]
fn final_result_appends_mixed_parts_in_response_order() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("respond");
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::FinalResult(Box::new(ModelResponse {
                parts: vec![
                    ModelResponsePart::Thinking {
                        text: "think 1".to_string(),
                        signature: None,
                    },
                    ModelResponsePart::Text {
                        text: "answer 1".to_string(),
                    },
                    ModelResponsePart::Thinking {
                        text: "think 2".to_string(),
                        signature: None,
                    },
                    ModelResponsePart::Text {
                        text: "answer 2".to_string(),
                    },
                ],
                usage: Usage::default(),
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

    assert!(body_line_index(&state, "> think 1") < body_line_index(&state, "answer 1"));
    assert!(body_line_index(&state, "answer 1") < body_line_index(&state, "> think 2"));
    assert!(body_line_index(&state, "> think 2") < body_line_index(&state, "answer 2"));
}

#[test]
fn part_start_only_does_not_create_transcript_content() {
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

    assert!(body_has_line(&state, "User: respond"));
    assert!(!body_has_line(&state, "Assistant:"));
    assert!(!body_has_line(&state, "> "));
}

#[test]
fn concise_exploration_groups_do_not_cross_thinking_segments() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.set_render_mode(TuiRenderMode::Concise);
    let view_call = ToolCallPart {
        id: "view_before_thinking".to_string(),
        name: "view".to_string(),
        arguments: json!({"file_path":"src/lib.rs"}).into(),
    };
    let grep_call = ToolCallPart {
        id: "grep_after_thinking".to_string(),
        name: "grep".to_string(),
        arguments: json!({"pattern":"ActiveModelSegment"}).into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: view_call,
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
        AgentStreamEvent::ToolCall {
            step: 1,
            call: grep_call,
        },
    ));

    assert_eq!(body_line_count(&state, "Exploring"), 2);
    assert!(body_line_index(&state, "  Read src/lib.rs") < body_line_index(&state, "> reasoning"));
    assert!(
        body_line_index(&state, "> reasoning")
            < body_line_index(&state, "  Searched ActiveModelSegment")
    );
}

#[test]
fn rendered_text_after_thinking_is_not_blockquoted() {
    let lines = vec![
        format!("{ASSISTANT_CONTENT_PREFIX}> hidden chain"),
        format!("{ASSISTANT_CONTENT_PREFIX}visible answer"),
    ];
    let rendered = render_transcript_lines(&lines, 80);

    let rendered_texts = line_texts(&rendered);
    assert!(rendered_texts.iter().any(|line| line == "│ hidden chain"));
    assert!(rendered_texts.iter().any(|line| line == "visible answer"));
    assert!(!rendered_texts.iter().any(|line| line == "│ visible answer"));
}

#[test]
fn render_modes_reproject_tool_visibility_and_active_tool_status() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("use tool");
    let call = ToolCallPart {
        id: "call_concise".to_string(),
        name: "lookup".to_string(),
        arguments: json!({"query":"mode"}).into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: call.clone(),
        },
    ));

    assert!(body_has_line(
        &state,
        "Tool call: lookup {\"query\":\"mode\"}"
    ));
    state.set_render_mode(TuiRenderMode::Concise);
    assert!(body_has_line(&state, "Calling lookup {\"query\":\"mode\"}"));
    let rendered_concise = render_transcript_lines(&state.body, 120);
    assert!(has_segment(
        &rendered_concise,
        "Calling lookup {\"query\":\"mode\"}",
        SegmentStyle::DIM
    ));
    assert!(!body_has_line(
        &state,
        "Tool call: lookup {\"query\":\"mode\"}"
    ));
    assert_eq!(
        state.active_tool_label().as_deref(),
        Some("Calling lookup {\"query\":\"mode\"}")
    );
    let footer = line_texts(&render_footer_lines(&state, 140)).join("\n");
    assert!(footer.contains("Calling lookup"));

    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(call.id, "lookup", json!("ok")),
        },
    ));
    assert!(state.active_tool_label().is_none());
    assert!(body_has_line(&state, "Called lookup {\"query\":\"mode\"}"));
    assert!(
        !state
            .body
            .iter()
            .any(|line| line.contains("Tool result: lookup"))
    );

    state.set_render_mode(TuiRenderMode::Normal);
    assert!(body_has_line(
        &state,
        "Tool call: lookup {\"query\":\"mode\"}"
    ));
    assert!(
        state
            .body
            .iter()
            .any(|line| line.contains("Tool result: lookup"))
    );
}

#[test]
fn concise_keeps_context_events_and_summarizes_approval_required_tools() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.set_render_mode(TuiRenderMode::Concise);

    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::Custom {
            event: AgentEvent::new("summary_completed", json!({"content": "Important handoff"})),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "compaction_completed",
                json!({"summary": "Compact detail", "compacted_count": 3}),
            ),
        },
    ));
    assert!(body_has_line(&state, "Summary complete"));
    assert!(
        state
            .body
            .iter()
            .any(|line| line.contains("Important handoff"))
    );
    assert!(body_has_line(&state, "Context compacted"));
    assert!(
        state
            .body
            .iter()
            .any(|line| line.contains("Compact detail"))
    );

    let mut approval_metadata = Metadata::default();
    approval_metadata.insert("control_flow".to_string(), json!("approval_required"));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new("approval_1", "shell_exec", json!("needs review"))
                .with_metadata(approval_metadata),
        },
    ));
    assert!(body_has_line(&state, "Ran shell_exec"));
    assert!(body_has_line(&state, "  needs review"));
    assert!(
        !state
            .body
            .iter()
            .any(|line| body_line_text(line).starts_with("Tool call: shell_exec"))
    );
    assert!(
        !state
            .body
            .iter()
            .any(|line| body_line_text(line).starts_with("Tool result: shell_exec"))
    );
    assert_eq!(state.status, "WAITING");
}

#[test]
fn concise_mode_summarizes_deferred_tools_without_full_payload() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.set_render_mode(TuiRenderMode::Concise);

    let mut metadata = Metadata::default();
    metadata.insert("control_flow".to_string(), json!("call_deferred"));
    metadata.insert("deferred".to_string(), json!({"id": "deferred_1"}));
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                "deferred_1",
                "fetch",
                json!({"message": "waiting for browser result", "url": "https://example.com"}),
            )
            .with_metadata(metadata),
        },
    ));

    assert!(body_has_line(&state, "Fetched URL"));
    assert!(body_has_line(
        &state,
        "  {\"message\":\"waiting for browser result\",\"url\":\"https://example.com\"}"
    ));
    assert!(
        !state
            .body
            .iter()
            .any(|line| body_line_text(line).starts_with("Tool call: fetch"))
    );
    assert!(
        !state
            .body
            .iter()
            .any(|line| body_line_text(line).starts_with("Tool result: fetch"))
    );
}

#[test]
fn display_command_switches_mode_and_reprojects_existing_timeline() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("use display command");
    let call = ToolCallPart {
        id: "call_display".to_string(),
        name: "lookup".to_string(),
        arguments: json!({"query":"display"}).into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ToolCall { step: 1, call },
    ));
    assert!(body_has_line(
        &state,
        "Tool call: lookup {\"query\":\"display\"}"
    ));

    state.input = "/display concise".to_string();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert_eq!(state.render_mode(), TuiRenderMode::Concise);
    assert!(body_has_line(
        &state,
        "Calling lookup {\"query\":\"display\"}"
    ));
    assert!(!body_has_line(
        &state,
        "Tool call: lookup {\"query\":\"display\"}"
    ));
    assert_eq!(state.input_status_text(), "display: concise");

    state.input = "/display normal".to_string();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert_eq!(state.render_mode(), TuiRenderMode::Normal);
    assert!(body_has_line(
        &state,
        "Tool call: lookup {\"query\":\"display\"}"
    ));
}

#[test]
fn concise_mode_streams_thinking_as_blockquote() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.set_render_mode(TuiRenderMode::Concise);

    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ModelResponse {
            step: 0,
            response: ModelResponse {
                parts: vec![ModelResponsePart::Thinking {
                    text: "checking the render projection before answering".to_string(),
                    signature: None,
                }],
                usage: starweaver_usage::Usage::default(),
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

    assert!(body_has_line(
        &state,
        "> checking the render projection before answering"
    ));
    assert!(!body_has_line(&state, "Thinking"));
    assert!(
        !state
            .body
            .iter()
            .any(|line| body_line_text(line).starts_with("Reasoned"))
    );
}

#[test]
fn concise_mode_updates_thinking_during_streaming() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.set_render_mode(TuiRenderMode::Concise);
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
    assert!(
        !state
            .body
            .iter()
            .any(|line| body_line_text(line).starts_with('>'))
    );
    assert!(!body_has_line(&state, "Thinking"));

    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::thinking(0, "checking")),
        },
    ));
    assert!(body_has_line(&state, "> checking"));

    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::thinking(0, " render")),
        },
    ));
    assert!(body_has_line(&state, "> checking render"));
    assert!(
        !state
            .body
            .iter()
            .any(|line| body_line_text(line).starts_with("Reasoned"))
    );
}

#[test]
fn concise_mode_keeps_assistant_text_streaming_normally() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.set_render_mode(TuiRenderMode::Concise);
    state.begin_run("respond");

    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartStart(PartStart {
                index: 0,
                part_kind: "text".to_string(),
            }),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::text(0, "visible")),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::text(0, " answer")),
        },
    ));

    assert!(body_has_line(&state, "Assistant:"));
    assert!(body_has_line(&state, "visible answer"));
    assert!(
        !state
            .body
            .iter()
            .any(|line| body_line_text(line).starts_with("Called text"))
    );
}

#[test]
fn concise_mode_summarizes_shell_failure_with_bounded_output() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.set_render_mode(TuiRenderMode::Concise);
    let shell_call = ToolCallPart {
        id: "shell_fail".to_string(),
        name: "shell_exec".to_string(),
        arguments: json!({"command":"cargo check -p starweaver-cli"}).into(),
    };

    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: shell_call.clone(),
        },
    ));
    assert!(body_has_line(
        &state,
        "Running cargo check -p starweaver-cli"
    ));

    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                shell_call.id,
                "shell_exec",
                json!({
                    "command":"cargo check -p starweaver-cli",
                    "return_code": 101,
                    "stderr":"error[E0425]: missing symbol\nextra line"
                }),
            )
            .with_error(true),
        },
    ));

    assert!(body_has_line(
        &state,
        "Ran cargo check -p starweaver-cli — failed exit 101"
    ));
    assert!(body_has_line(&state, "  error[E0425]: missing symbol"));
}

#[test]
fn concise_mode_summarizes_mutations_and_task_tools() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.set_render_mode(TuiRenderMode::Concise);
    let edit_call = ToolCallPart {
        id: "edit_concise".to_string(),
        name: "multi_edit".to_string(),
        arguments: json!({
            "file_path": "src/lib.rs",
            "edits": [
                {"old_string": "old", "new_string": "new"},
                {"old_string": "left", "new_string": "right"}
            ]
        })
        .into(),
    };

    state.apply_stream_record(&AgentStreamRecord::new(
        3,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: edit_call.clone(),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        4,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(edit_call.id, "multi_edit", json!({"edited": true})),
        },
    ));
    assert!(body_has_line(&state, "Edited src/lib.rs — 2 edits"));
    assert!(!body_has_line(&state, "  {\"edited\":true}"));

    let task_call = ToolCallPart {
        id: "task_concise".to_string(),
        name: "task_create".to_string(),
        arguments: json!({"subject":"Review concise UX"}).into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        5,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: task_call.clone(),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        6,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(task_call.id, "task_create", json!({"id":"1"})),
        },
    ));
    assert!(body_has_line(&state, "Created task"));
    assert!(
        !state
            .body
            .iter()
            .any(|line| line.contains("Tool result: task_create"))
    );
}

#[test]
fn concise_mode_groups_adjacent_exploration_summaries() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.set_render_mode(TuiRenderMode::Concise);

    let view_call = ToolCallPart {
        id: "view_1".to_string(),
        name: "view".to_string(),
        arguments: json!({"file_path":"src/lib.rs"}).into(),
    };
    let grep_call = ToolCallPart {
        id: "grep_1".to_string(),
        name: "grep".to_string(),
        arguments: json!({"pattern":"TuiRenderMode", "root":"crates/starweaver-cli"}).into(),
    };
    for (sequence, call) in [(0, view_call.clone()), (2, grep_call.clone())] {
        state.apply_stream_record(&AgentStreamRecord::new(
            sequence,
            AgentStreamEvent::ToolCall { step: 1, call },
        ));
    }
    assert!(body_has_line(&state, "Exploring"));
    assert!(body_has_line(&state, "  Read src/lib.rs"));
    assert!(body_has_line(&state, "  Searched TuiRenderMode"));

    state.apply_stream_record(&AgentStreamRecord::new(
        3,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                view_call.id,
                "view",
                json!({"file_path":"src/lib.rs", "content":"fn main() {}"}),
            ),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        4,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(grep_call.id, "grep", json!({"matches":[]})),
        },
    ));
    assert!(body_has_line(&state, "Explored"));
    assert!(body_has_line(&state, "  Read src/lib.rs"));
    assert!(body_has_line(&state, "  Searched TuiRenderMode"));

    let edit_call = ToolCallPart {
        id: "edit_boundary".to_string(),
        name: "write".to_string(),
        arguments: json!({"file_path":"src/lib.rs", "content":"updated"}).into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        5,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: edit_call.clone(),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        6,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(edit_call.id, "write", json!({"written": true})),
        },
    ));
    assert!(body_has_line(&state, "Wrote src/lib.rs"));
}

#[test]
fn subagent_output_is_full_markdown_in_normal_and_concise() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let source = AgentStreamSource::subagent(
        AgentId::from_string("writer-1"),
        "writer",
        TaskId::from_string("task-writer"),
        Some(RunId::from_string("run-child")),
        Some(RunId::from_string("run-parent")),
        0,
    );
    state.apply_stream_record(
        &AgentStreamRecord::new(
            0,
            AgentStreamEvent::RunComplete {
                run_id: RunId::from_string("run-child"),
                output: "- **done**\n```rust\nfn main() {}\n```".to_string(),
            },
        )
        .with_source(source),
    );

    assert!(body_has_line(&state, "- **done**"));
    assert!(body_has_line(&state, "```rust"));
    assert!(body_has_line(&state, "fn main() {}"));
    let rendered = render_transcript_lines(&state.body, 80);
    assert!(has_segment(&rendered, "done", SegmentStyle::BOLD));
    assert!(rendered.iter().any(|line| {
        line_text(line) == "fn main() {}"
            && line
                .segments
                .iter()
                .all(|segment| segment.style.contains(SegmentStyle::CODE_BG))
    }));

    state.set_render_mode(TuiRenderMode::Concise);
    assert!(body_has_line(&state, "- **done**"));
    assert!(body_has_line(&state, "fn main() {}"));
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
                usage: starweaver_usage::Usage::default(),
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
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "Tool call: lookup {\"query\":\"starweaver\"}")
    );

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
                usage: starweaver_usage::Usage::default(),
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
fn consecutive_streamed_tool_calls_with_same_name_do_not_reuse_first_line() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("use tools");
    for (sequence, index, query) in [(0, 2, "first"), (3, 3, "second")] {
        state.apply_stream_record(&AgentStreamRecord::new(
            sequence,
            AgentStreamEvent::ModelStream {
                step: 0,
                event: ModelResponseStreamEvent::PartStart(PartStart {
                    index,
                    part_kind: "tool_call".to_string(),
                }),
            },
        ));
        state.apply_stream_record(&AgentStreamRecord::new(
            sequence + 1,
            AgentStreamEvent::ModelStream {
                step: 0,
                event: ModelResponseStreamEvent::PartDelta(PartDelta {
                    index,
                    delta: StreamDelta::ToolCallName {
                        name: "lookup".to_string(),
                    },
                }),
            },
        ));
        state.apply_stream_record(&AgentStreamRecord::new(
            sequence + 2,
            AgentStreamEvent::ModelStream {
                step: 0,
                event: ModelResponseStreamEvent::PartDelta(PartDelta {
                    index,
                    delta: StreamDelta::ToolCallArguments {
                        arguments_delta: format!("{{\"query\":\"{query}\"}}"),
                    },
                }),
            },
        ));
    }

    let first_call = ToolCallPart {
        id: "call_first".to_string(),
        name: "lookup".to_string(),
        arguments: json!({"query":"first"}).into(),
    };
    let second_call = ToolCallPart {
        id: "call_second".to_string(),
        name: "lookup".to_string(),
        arguments: json!({"query":"second"}).into(),
    };
    state.apply_stream_record(&AgentStreamRecord::new(
        6,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::FinalResult(Box::new(ModelResponse {
                parts: vec![
                    ModelResponsePart::ToolCall(first_call.clone()),
                    ModelResponsePart::ToolCall(second_call.clone()),
                ],
                usage: starweaver_usage::Usage::default(),
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
        7,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: first_call,
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        8,
        AgentStreamEvent::ToolCall {
            step: 1,
            call: second_call,
        },
    ));

    let tool_call_lines = state
        .body
        .iter()
        .filter(|line| line.starts_with("Tool call: lookup"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(tool_call_lines.len(), 2);
    assert!(
        tool_call_lines
            .iter()
            .any(|line| line == "Tool call: lookup {\"query\":\"first\"}")
    );
    assert!(
        tool_call_lines
            .iter()
            .any(|line| line == "Tool call: lookup {\"query\":\"second\"}")
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
    assert!(
        !state
            .body
            .iter()
            .any(|line| line.contains("Tool result: lookup") && line.contains("final answer"))
    );
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
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "[SYS] Switched model to Coding (openai-responses:gpt-5)")
    );

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
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "[SYS] Unknown model profile: missing")
    );
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
fn session_command_opens_picker_selects_directly_and_blocks_while_running() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.set_session_choices(vec![
        SessionChoice {
            session_id: "session_alpha".to_string(),
            title: Some("Alpha".to_string()),
            profile: Some("general".to_string()),
            status: "active".to_string(),
            run_count: 1,
            last_output_preview: Some("hello".to_string()),
            updated_at: "2026-06-08T09:00:00Z".to_string(),
        },
        SessionChoice {
            session_id: "session_beta".to_string(),
            title: Some("Beta".to_string()),
            profile: Some("coding".to_string()),
            status: "active".to_string(),
            run_count: 2,
            last_output_preview: Some("long output preview".to_string()),
            updated_at: "2026-06-08T10:00:00Z".to_string(),
        },
    ]);

    state.input = "/session".to_string();
    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::Enter)),
        Some(InteractiveTuiEvent::Session(None))
    );
    assert!(!state.session_picker_visible());

    state.open_session_picker();
    assert!(state.session_picker_visible());
    assert_eq!(state.input_mode_label(), "SESSION");
    assert_eq!(state.session_picker_index(), 0);
    let picker_text = line_texts(&render_footer_lines(&state, 120)).join("\n");
    assert!(picker_text.contains("Sessions"));
    assert!(picker_text.contains("Enter: reload"));
    assert!(picker_text.contains("session_alpha"));
    assert!(picker_text.contains("session_beta"));
    assert!(picker_text.contains("Highlighted session"));
    assert!(picker_text.contains("preview:"));
    assert!(picker_text.contains("hello"));

    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Up)), None);
    assert_eq!(state.session_picker_index(), 1);
    let beta_picker_text = line_texts(&render_footer_lines(&state, 120)).join("\n");
    assert!(beta_picker_text.contains("session_beta"));
    assert!(beta_picker_text.contains("coding"));
    assert!(beta_picker_text.contains("long output preview"));
    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::Enter)),
        Some(InteractiveTuiEvent::Session(Some(
            "session_beta".to_string()
        )))
    );
    assert!(!state.session_picker_visible());

    state.open_session_picker();
    assert!(state.session_picker_visible());
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Esc)), None);
    assert!(!state.session_picker_visible());

    state.input = "/session session_alpha".to_string();
    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::Enter)),
        Some(InteractiveTuiEvent::Session(Some(
            "session_alpha".to_string()
        )))
    );

    state.running = true;
    state.input = "/session session_beta".to_string();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert!(!state.session_picker_visible());
    assert!(state.body.iter().any(|line| {
        line == "[SYS] Session selection is available after the current run finishes."
    }));
}

#[test]
#[allow(clippy::too_many_lines)]
fn footer_context_uses_latest_usage_snapshot_not_model_response_or_high_water() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.context_window = Some(10_000);

    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ModelResponse {
            step: 0,
            response: ModelResponse {
                parts: Vec::new(),
                usage: Usage {
                    requests: 1,
                    input_tokens: 2_000,
                    cache_write_tokens: 0,
                    cache_read_tokens: 0,
                    output_tokens: 1_000,
                    total_tokens: 3_000,
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
    assert_eq!(state.context_percent_label(), "0%");

    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "usage_snapshot",
                serde_json::to_value(UsageSnapshot {
                    run_id: "run_context".to_string(),
                    latest_usage: None,
                    total_usage: Usage {
                        requests: 1,
                        input_tokens: 2_000,
                        cache_write_tokens: 0,
                        cache_read_tokens: 0,
                        output_tokens: 1_000,
                        total_tokens: 3_000,
                        tool_calls: 0,
                    },
                    estimate_pricing: Some(PricingEstimate::from_micros_usd(1_000_000)),
                    entries: Vec::new(),
                    agent_usages: BTreeMap::new(),
                    model_usages: BTreeMap::new(),
                    model_estimate_pricing: BTreeMap::new(),
                })
                .unwrap(),
            ),
        },
    ));
    assert_eq!(state.context_percent_label(), "30%");

    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::ModelResponse {
            step: 1,
            response: ModelResponse {
                parts: Vec::new(),
                usage: Usage {
                    requests: 1,
                    input_tokens: 100,
                    cache_write_tokens: 0,
                    cache_read_tokens: 0,
                    output_tokens: 100,
                    total_tokens: 200,
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
    assert_eq!(state.context_percent_label(), "30%");

    state.apply_stream_record(&AgentStreamRecord::new(
        3,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "usage_snapshot",
                serde_json::to_value(UsageSnapshot {
                    run_id: "run_context".to_string(),
                    latest_usage: Some(Usage {
                        requests: 1,
                        input_tokens: 100,
                        cache_write_tokens: 0,
                        cache_read_tokens: 0,
                        output_tokens: 100,
                        total_tokens: 200,
                        tool_calls: 0,
                    }),
                    total_usage: Usage {
                        requests: 2,
                        input_tokens: 2_100,
                        cache_write_tokens: 0,
                        cache_read_tokens: 0,
                        output_tokens: 1_100,
                        total_tokens: 3_200,
                        tool_calls: 0,
                    },
                    estimate_pricing: Some(PricingEstimate::from_micros_usd(2_000_000)),
                    entries: Vec::new(),
                    agent_usages: BTreeMap::new(),
                    model_usages: BTreeMap::new(),
                    model_estimate_pricing: BTreeMap::new(),
                })
                .unwrap(),
            ),
        },
    ));
    assert_eq!(state.context_percent_label(), "2%");

    state.input = "/cost".to_string();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert!(body_has_line(
        &state,
        "[SYS] Latest request total tokens: 200"
    ));
    assert!(body_has_line(
        &state,
        "[SYS] Displayed context high-water: 3,000"
    ));
    assert!(body_has_line(
        &state,
        "[SYS] Latest request context used: 2%"
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
fn cost_command_shows_accumulated_usage_snapshots() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "usage_snapshot",
                serde_json::to_value(UsageSnapshot {
                    run_id: "run_cost_1".to_string(),
                    latest_usage: None,
                    total_usage: Usage {
                        requests: 1,
                        input_tokens: 1_000,
                        cache_write_tokens: 100,
                        cache_read_tokens: 200,
                        output_tokens: 500,
                        total_tokens: 1_500,
                        tool_calls: 0,
                    },
                    estimate_pricing: None,
                    entries: Vec::new(),
                    agent_usages: BTreeMap::from([(
                        "main".to_string(),
                        UsageAgentTotal {
                            agent_name: "main".to_string(),
                            model_id: "test:test".to_string(),
                            usage: Usage {
                                requests: 1,
                                input_tokens: 1_000,
                                cache_write_tokens: 100,
                                cache_read_tokens: 200,
                                output_tokens: 500,
                                total_tokens: 1_500,
                                tool_calls: 0,
                            },
                            estimate_pricing: Some(PricingEstimate::from_micros_usd(1_000_000)),
                            usage_id: None,
                            source: "model_request".to_string(),
                        },
                    )]),
                    model_estimate_pricing: BTreeMap::from([(
                        "test:test".to_string(),
                        PricingEstimate::from_micros_usd(1_000_000),
                    )]),
                    model_usages: BTreeMap::from([(
                        "test:test".to_string(),
                        Usage {
                            requests: 1,
                            input_tokens: 1_000,
                            cache_write_tokens: 100,
                            cache_read_tokens: 200,
                            output_tokens: 500,
                            total_tokens: 1_500,
                            tool_calls: 0,
                        },
                    )]),
                })
                .unwrap(),
            ),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "usage_snapshot",
                serde_json::to_value(UsageSnapshot {
                    run_id: "run_cost_2".to_string(),
                    latest_usage: None,
                    total_usage: Usage {
                        requests: 2,
                        input_tokens: 2_000,
                        cache_write_tokens: 300,
                        cache_read_tokens: 400,
                        output_tokens: 700,
                        total_tokens: 2_700,
                        tool_calls: 0,
                    },
                    estimate_pricing: None,
                    entries: Vec::new(),
                    agent_usages: BTreeMap::from([(
                        "debugger".to_string(),
                        UsageAgentTotal {
                            agent_name: "debugger".to_string(),
                            model_id: "test:test".to_string(),
                            usage: Usage {
                                requests: 2,
                                input_tokens: 2_000,
                                cache_write_tokens: 300,
                                cache_read_tokens: 400,
                                output_tokens: 700,
                                total_tokens: 2_700,
                                tool_calls: 0,
                            },
                            estimate_pricing: Some(PricingEstimate::from_micros_usd(2_000_000)),
                            usage_id: None,
                            source: "model_request".to_string(),
                        },
                    )]),
                    model_estimate_pricing: BTreeMap::from([(
                        "test:test".to_string(),
                        PricingEstimate::from_micros_usd(2_000_000),
                    )]),
                    model_usages: BTreeMap::from([(
                        "test:test".to_string(),
                        Usage {
                            requests: 2,
                            input_tokens: 2_000,
                            cache_write_tokens: 300,
                            cache_read_tokens: 400,
                            output_tokens: 700,
                            total_tokens: 2_700,
                            tool_calls: 0,
                        },
                    )]),
                })
                .unwrap(),
            ),
        },
    ));

    state.input = "/cost".to_string();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);

    assert!(body_has_line(&state, "[SYS] Token Usage Summary:"));
    assert!(body_has_line(&state, "[SYS] By Model:"));
    assert!(body_has_line(&state, "[SYS]   test:test:"));
    assert!(body_has_line(&state, "[SYS]   Input:  3,000 tokens"));
    assert!(body_has_line(&state, "[SYS]   Output: 1,200 tokens"));
    assert!(body_has_line(&state, "[SYS]   Cache Write: 400 tokens"));
    assert!(body_has_line(&state, "[SYS]   Cache Read:  600 tokens"));
    assert!(body_has_line(&state, "[SYS]   Total:  4,200 tokens"));
    assert!(body_has_line(&state, "[SYS]   Requests: 3"));
    assert!(body_has_line(
        &state,
        "[SYS]   Estimated pricing: $3.000000 USD"
    ));
    assert!(body_has_line(
        &state,
        "[SYS]     Estimated pricing: $3.000000 USD"
    ));
    assert!(body_has_line(&state, "[SYS]   main:"));
    assert!(body_has_line(&state, "[SYS]   debugger:"));
}

#[test]
#[allow(clippy::too_many_lines)]
fn interactive_state_covers_model_response_finish_and_failure() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("respond");
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::ModelResponse {
            step: 0,
            response: ModelResponse {
                parts: vec![
                    ModelResponsePart::ProviderText {
                        text: "answer".to_string(),
                        provider: ProviderPartInfo::new("openai").with_id("msg_1"),
                    },
                    ModelResponsePart::ProviderThinking {
                        text: "reasoning".to_string(),
                        signature: Some("sig".to_string()),
                        provider: ProviderPartInfo::new("openai").with_id("rs_1"),
                    },
                    ModelResponsePart::ProviderToolCall {
                        call: ToolCallPart {
                            id: "call_2".to_string(),
                            name: "search".to_string(),
                            arguments: json!({}).into(),
                        },
                        provider: ProviderPartInfo::new("openai").with_id("fc_1"),
                    },
                ],
                usage: starweaver_usage::Usage {
                    requests: 1,
                    input_tokens: 1_000,
                    cache_write_tokens: 0,
                    cache_read_tokens: 0,
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
    assert_eq!(state.context_percent_label(), "0%");
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::ModelResponse {
            step: 1,
            response: ModelResponse {
                parts: Vec::new(),
                usage: starweaver_usage::Usage {
                    requests: 1,
                    input_tokens: 6_000,
                    cache_write_tokens: 0,
                    cache_read_tokens: 0,
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
    assert_eq!(state.context_percent_label(), "0%");

    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "usage_snapshot",
                serde_json::to_value(UsageSnapshot {
                    run_id: "run_test".to_string(),
                    latest_usage: Some(Usage {
                        requests: 1,
                        input_tokens: 7_000,
                        cache_write_tokens: 0,
                        cache_read_tokens: 0,
                        output_tokens: 500,
                        total_tokens: 7_500,
                        tool_calls: 0,
                    }),
                    total_usage: Usage {
                        requests: 2,
                        input_tokens: 7_000,
                        cache_write_tokens: 0,
                        cache_read_tokens: 0,
                        output_tokens: 1_000,
                        total_tokens: 8_000,
                        tool_calls: 0,
                    },
                    estimate_pricing: None,
                    entries: Vec::new(),
                    agent_usages: BTreeMap::new(),
                    model_usages: BTreeMap::new(),
                    model_estimate_pricing: BTreeMap::new(),
                })
                .unwrap(),
            ),
        },
    ));
    assert_eq!(state.context_percent_label(), "4%");

    state.apply_stream_record(&AgentStreamRecord::new(
        3,
        AgentStreamEvent::RunComplete {
            run_id: RunId::from_string("run_test"),
            output: "unused because streamed".to_string(),
        },
    ));
    assert_eq!(state.phase, "completed");
    assert!(
        !state
            .body
            .iter()
            .any(|line| body_line_text(line) == "unused because streamed")
    );

    state.finish_run(Some("session_complete".to_string()));
    assert_eq!(state.session_id.as_deref(), Some("session_complete"));
    assert_eq!(state.status, "IDLE");

    state.fail_run("boom");
    assert_eq!(state.status, "ERROR");
    assert_eq!(state.phase, "failed");
    assert!(state.body.iter().any(|line| line == "Error: boom"));
}

#[test]
fn subagent_lifecycle_events_render_as_folded_status_lines() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let started = AgentStreamRecord::new(
        0,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "subagent_started",
                json!({
                    "kind": "started",
                    "agent_id": "explorer-abc",
                    "agent_name": "explorer",
                    "prompt_preview": "inspect files"
                }),
            ),
        },
    );
    state.apply_stream_record(&started);
    assert!(body_has_line(&state, "[explorer] running"));
    assert_eq!(
        display_lines_for_stream_record(&started),
        vec!["[explorer-abc] Running...".to_string()]
    );

    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "subagent_completed",
                json!({
                    "kind": "completed",
                    "agent_id": "explorer-abc",
                    "agent_name": "explorer",
                    "success": true,
                    "duration_seconds": 12.34,
                    "request_count": 2,
                    "result_preview": "found the owner"
                }),
            ),
        },
    ));
    assert!(!body_has_line(&state, "[explorer] running"));
    assert!(body_has_line(&state, "[explorer] done (12.3s) | 2 reqs"));
    assert!(body_has_line(&state, "found the owner"));

    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "subagent_failed",
                json!({
                    "kind": "failed",
                    "agent_id": "debugger-def",
                    "agent_name": "debugger",
                    "success": false,
                    "duration_seconds": 1.2,
                    "error": "missing_subagent"
                }),
            ),
        },
    ));
    assert!(body_has_line(&state, "[debugger] failed (1.2s)"));
    assert!(body_has_line(&state, "missing_subagent"));
    assert!(!state.body.iter().any(|line| line.contains("inspect files")));
}

#[test]
fn source_attributed_subagent_records_update_one_collapsed_line() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.begin_run("delegate research");
    let source = AgentStreamSource::subagent(
        AgentId::from_string("researcher-1"),
        "researcher",
        TaskId::from_string("task-research"),
        Some(RunId::from_string("run-child")),
        Some(RunId::from_string("run-parent")),
        0,
    );
    let sourced_record = |sequence, event: AgentStreamEvent| {
        AgentStreamRecord::new(sequence, event).with_source(source.clone())
    };

    state.apply_stream_record(&sourced_record(
        0,
        AgentStreamEvent::ModelRequest { step: 0 },
    ));
    state.apply_stream_record(&sourced_record(
        1,
        AgentStreamEvent::ToolCall {
            step: 0,
            call: ToolCallPart {
                id: "call-search".to_string(),
                name: "search".to_string(),
                arguments: json!({"query": "owner"}).into(),
            },
        },
    ));
    state.apply_stream_record(&sourced_record(
        2,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::text(0, "found ")),
        },
    ));
    state.apply_stream_record(&sourced_record(
        3,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::text(0, "owner")),
        },
    ));
    state.apply_stream_record(&sourced_record(
        4,
        AgentStreamEvent::RunComplete {
            run_id: RunId::from_string("run-child"),
            output: "found owner".to_string(),
        },
    ));

    assert!(body_has_line(
        &state,
        "[researcher] done | 1 reqs | tools: search"
    ));
    assert!(body_has_line(&state, "found owner"));
    assert_eq!(
        state
            .body
            .iter()
            .filter(|line| line.starts_with("[researcher]"))
            .count(),
        1
    );
    assert!(
        !state
            .body
            .iter()
            .any(|line| line.starts_with("Tool call: search"))
    );
    assert!(body_has_line(&state, "found owner"));
}

#[test]
fn subagent_lifecycle_start_reuses_source_attributed_collapsed_line() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let source = AgentStreamSource::subagent(
        AgentId::from_string("explorer-1"),
        "explorer",
        TaskId::from_string("task-explorer"),
        Some(RunId::from_string("run-child")),
        Some(RunId::from_string("run-parent")),
        0,
    );

    state.apply_stream_record(
        &AgentStreamRecord::new(0, AgentStreamEvent::ModelRequest { step: 0 }).with_source(source),
    );
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "subagent_started",
                json!({
                    "kind": "started",
                    "name": "explorer",
                    "task_id": "task-explorer",
                    "metadata": {"agent_id": "explorer-1"}
                }),
            ),
        },
    ));

    assert_eq!(
        state
            .body
            .iter()
            .filter(|line| line.starts_with("[explorer]"))
            .count(),
        1
    );
    assert!(body_has_line(&state, "[explorer] running | 1 reqs"));
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
    assert!(
        rendered[0].segments[0]
            .style
            .contains(SegmentStyle::UNDERLINED)
    );
    assert!(rendered.iter().any(|line| {
        line.segments
            .first()
            .is_some_and(|segment| segment.text == "• ")
    }));
    assert!(has_segment(&rendered, "bold", SegmentStyle::BOLD));
    assert!(has_segment(&rendered, "em", SegmentStyle::ITALIC));
    assert!(has_segment(&rendered, "code", SegmentStyle::CYAN));
    assert!(has_segment(&rendered, "│ ", SegmentStyle::GREEN));
    assert!(has_segment(&rendered, "docs", SegmentStyle::UNDERLINED));
    let Some(code_line) = rendered
        .iter()
        .find(|line| line_text(line) == "fn main() {}")
    else {
        panic!("rendered Rust code line is missing");
    };
    assert!(
        code_line
            .segments
            .iter()
            .all(|segment| segment.style.contains(SegmentStyle::CODE_BG))
    );
    let syntax_colors = code_line
        .segments
        .iter()
        .filter_map(|segment| segment.style.foreground_rgb())
        .collect::<Vec<_>>();
    assert!(
        syntax_colors
            .windows(2)
            .any(|colors| colors[0] != colors[1])
    );
    assert!(!line_text(code_line).starts_with('│'));
    assert!(rendered.iter().any(|line| {
        line.segments
            .first()
            .is_some_and(|segment| segment.text == "────────────")
    }));
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
    assert!(text.contains("plain code"));
    assert!(!text.contains("╭─ code"));
    assert!(!text.contains("╰────"));
    assert!(!text.contains("│ plain code"));
    assert!(text.contains("Small"));
    assert!(has_segment(&rendered, "gone", SegmentStyle::DIM));
    assert!(has_segment(&rendered, "Small", SegmentStyle::ITALIC));
    assert!(
        rendered
            .iter()
            .any(|line| line.segments.iter().any(|segment| segment.text == "   "))
    );

    let empty = render_markdown_lines(&[], 0);
    assert_eq!(line_texts(&empty), vec![String::new()]);
}

#[test]
fn transcript_renderer_batches_live_assistant_markdown_fences() {
    let lines = vec![
        format!("{ASSISTANT_CONTENT_PREFIX}Created commit:"),
        format!("{ASSISTANT_CONTENT_PREFIX}"),
        format!("{ASSISTANT_CONTENT_PREFIX}```text"),
        format!(
            "{ASSISTANT_CONTENT_PREFIX}b238274 feat: add OAuth refresh and interactive CLI flows"
        ),
        format!("{ASSISTANT_CONTENT_PREFIX}```"),
        format!("{ASSISTANT_CONTENT_PREFIX}"),
        format!("{ASSISTANT_CONTENT_PREFIX}Commit body:"),
        format!("{ASSISTANT_CONTENT_PREFIX}"),
        format!("{ASSISTANT_CONTENT_PREFIX}```markdown"),
        format!(
            "{ASSISTANT_CONTENT_PREFIX}• Add OAuth token stores, refresh supervisors, and provider hooks."
        ),
        format!("{ASSISTANT_CONTENT_PREFIX}```"),
        "Run completed: run_test status=completed".to_string(),
    ];

    let rendered = render_transcript_lines(&lines, 100);
    let text = line_texts(&rendered).join("\n");
    assert!(text.contains("Created commit:"));
    assert!(text.contains("b238274 feat: add OAuth refresh and interactive CLI flows"));
    assert!(text.contains("• Add OAuth token stores, refresh supervisors, and provider hooks."));
    assert!(!text.contains("│ b238274"));
    assert!(!text.contains("│ • Add OAuth"));
    assert!(!text.contains("╭─"));
    assert!(!text.contains("╰────"));
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
        "Context compacting 3 messages...".to_string(),
        "Context compacted".to_string(),
        "Compact failed: no space".to_string(),
        "Summarizing progress (3 messages)...".to_string(),
        "Summary complete".to_string(),
        "Summary failed: no summary".to_string(),
        String::new(),
    ];
    let rendered = render_transcript_lines(&lines, 80);
    let text = line_texts(&rendered).join("\n");
    assert!(text.contains("plain setup"));
    assert!(text.contains("hello"));
    assert!(text.contains("world"));
    assert!(text.contains("Calling: lookup"));
    assert!(text.contains("Complete: ok"));
    assert!(text.contains("x Error: lookup | Error: permission denied"));
    let long_error =
        "Tool error: shell_exec provider status 400: this error should keep its tail marker";
    let long_error_text =
        line_texts(&render_transcript_lines(&[long_error.to_string()], 120)).join("\n");
    assert!(long_error_text.contains("provider status 400"));
    assert!(long_error_text.contains("tail"));
    assert!(long_error_text.contains("marker"));
    assert!(!long_error_text.contains('…'));
    assert!(text.contains("✕ error boom"));
    assert!(text.contains("◌ thinking hidden"));
    assert!(text.contains("◷ waiting wait"));
    assert!(text.contains("↻ retry 1"));
    assert!(text.contains("Context compacting 3 messages..."));
    assert!(text.contains("Context compacted"));
    assert!(text.contains("x compact failed no space"));
    assert!(text.contains("Summarizing progress (3 messages)..."));
    assert!(text.contains("Summary complete"));
    assert!(text.contains("x summary failed no summary"));
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
    assert!(
        rendered
            .iter()
            .any(|line| line_text(line) == "› # raw prompt")
    );
    assert!(has_segment(&rendered, "Title", SegmentStyle::BOLD));
    assert!(rendered.iter().any(|line| {
        line.segments
            .first()
            .is_some_and(|segment| segment.text == "• ")
    }));
    assert!(
        rendered
            .iter()
            .any(|line| line_text(line).contains("User: literal assistant content"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text(line).contains("Assistant:"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text(line).contains("Tool call: literal text"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text(line) == "  ✓ completed run_test status=completed")
    );
}

#[test]
fn transcript_renderer_wraps_long_user_prompt_lines() {
    let lines = vec!["User: abcdefghijklmnopqrstuvwxyz".to_string()];

    let rendered = render_transcript_lines(&lines, 12);
    let texts = line_texts(&rendered);

    assert_eq!(
        texts,
        vec![
            "› abcdefghij".to_string(),
            "  klmnopqrst".to_string(),
            "  uvwxyz".to_string(),
        ]
    );
    assert!(rendered.iter().all(|line| line.visible_width() <= 12));
    assert!(rendered.iter().all(|line| {
        line.segments
            .iter()
            .all(|segment| segment.style.contains(SegmentStyle::BOLD))
    }));
}

#[test]
fn composer_viewport_scrolls_multiline_input_and_resets_on_edit() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.input = (1..=7)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n");

    let bottom = line_texts(&render_composer_lines(&state, 80)).join("\n");
    assert!(!bottom.contains("line 1"));
    assert!(bottom.contains("line 3"));
    assert!(bottom.contains("line 7"));
    assert_eq!(
        input_viewport_lines(&state.input, 5, 0).first().unwrap(),
        "line 3"
    );

    assert_eq!(
        handle_key_event(
            &mut state,
            key_code_modified(KeyCode::Up, KeyModifiers::ALT)
        ),
        None
    );
    assert_eq!(state.composer_scroll_offset(), 1);
    let scrolled = line_texts(&render_composer_lines(&state, 80)).join("\n");
    assert!(scrolled.contains("line 2"));
    assert!(!scrolled.contains("line 7"));

    assert_eq!(handle_key_event(&mut state, key_char('x')), None);
    assert_eq!(state.composer_scroll_offset(), 0);
    let edited = line_texts(&render_composer_lines(&state, 80)).join("\n");
    assert!(edited.contains("line 7x"));
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
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "  /help             Show this help")
    );
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "  !<command>        Run a shell command inline")
    );
    assert!(
        state
            .body
            .iter()
            .any(|line| { line == "  /paste-image      Attach image from system clipboard" })
    );
    assert!(state.body.iter().any(|line| line == "Shortcuts"));
    assert!(
        !state
            .body
            .iter()
            .any(|line| line.starts_with("[SYS] /help"))
    );
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

    state.input = "!echo hello".to_string();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert!(state.input.is_empty());
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "Shell command: echo hello")
    );
    assert!(state.body.iter().any(|line| line == "Shell stdout:"));
    assert!(state.body.iter().any(|line| line == "  hello"));
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "Shell completed: exit 0")
    );
}

#[test]
fn goal_mode_reports_total_tokens_on_goal_complete() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.input = "/goal migrate tui".to_string();
    assert_eq!(
        submit_text(handle_key_event(&mut state, key_code(KeyCode::Enter))),
        Some("migrate tui".to_string())
    );
    assert_eq!(
        state.take_pending_goal_submission(),
        Some(("migrate tui".to_string(), 10))
    );
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::RunStart {
            run_id: RunId::from_string("run_goal"),
            conversation_id: ConversationId::from_string("conversation_goal"),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "usage_snapshot",
                serde_json::to_value(UsageSnapshot {
                    run_id: "run_goal".to_string(),
                    total_usage: Usage {
                        requests: 2,
                        input_tokens: 10_000,
                        cache_write_tokens: 222,
                        cache_read_tokens: 111,
                        output_tokens: 2_345,
                        total_tokens: 12_345,
                        ..Usage::default()
                    },
                    ..UsageSnapshot::default()
                })
                .unwrap(),
            ),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        3,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "goal_iteration",
                json!({"iteration": 1, "max_iterations": 10, "task": "migrate tui"}),
            ),
        },
    ));
    assert_eq!(state.goal_iteration, 1);
    assert!(state.goal_active);
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "[Goal] Iteration 1/10")
    );

    state.apply_stream_record(&AgentStreamRecord::new(
        4,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "goal_complete",
                json!({"iteration": 1, "max_iterations": 10, "reason": "verified", "task": "migrate tui"}),
            ),
        },
    ));
    assert!(!state.goal_active);
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "[Goal] Completed: verified after 1 iteration(s)")
    );
    assert!(state
        .body
        .iter()
        .any(|line| line == "[Goal] Total tokens: 12,345 (input: 10,000, cache read: 111, cache write: 222, output: 2,345)"));
}

#[test]
fn goal_mode_reports_total_tokens_at_max_iterations() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.input = "/goal hard task".to_string();
    assert_eq!(
        submit_text(handle_key_event(&mut state, key_code(KeyCode::Enter))),
        Some("hard task".to_string())
    );
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "usage_snapshot",
                serde_json::to_value(UsageSnapshot {
                    run_id: String::new(),
                    total_usage: Usage {
                        requests: 1,
                        input_tokens: 7_000,
                        cache_write_tokens: 90,
                        cache_read_tokens: 80,
                        output_tokens: 890,
                        total_tokens: 7_890,
                        ..Usage::default()
                    },
                    ..UsageSnapshot::default()
                })
                .unwrap(),
            ),
        },
    ));
    state.apply_stream_record(&AgentStreamRecord::new(
        2,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "goal_complete",
                json!({"iteration": 1, "max_iterations": 1, "reason": "max_iterations", "task": "hard task"}),
            ),
        },
    ));
    assert!(!state.goal_active);
    assert!(
        state
            .body
            .iter()
            .any(|line| line.contains("max iterations reached"))
    );
    assert!(state
        .body
        .iter()
        .any(|line| line == "[Goal] Total tokens: 7,890 (input: 7,000, cache read: 80, cache write: 90, output: 890)"));
}

#[test]
fn goal_mode_reports_total_tokens_when_run_finishes_without_goal_event() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.input = "/goal interrupted task".to_string();
    assert_eq!(
        submit_text(handle_key_event(&mut state, key_code(KeyCode::Enter))),
        Some("interrupted task".to_string())
    );
    state.apply_stream_record(&AgentStreamRecord::new(
        1,
        AgentStreamEvent::Custom {
            event: AgentEvent::new(
                "usage_snapshot",
                serde_json::to_value(UsageSnapshot {
                    run_id: "run_unverified".to_string(),
                    total_usage: Usage {
                        requests: 3,
                        input_tokens: 90_000,
                        cache_write_tokens: 3_000,
                        cache_read_tokens: 2_000,
                        output_tokens: 1_234,
                        total_tokens: 91_234,
                        ..Usage::default()
                    },
                    ..UsageSnapshot::default()
                })
                .unwrap(),
            ),
        },
    ));

    state.finish_run(Some("session_goal".to_string()));

    assert!(!state.goal_active);
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "[Goal] Completed: unverified_stop")
    );
    assert!(state
        .body
        .iter()
        .any(|line| line == "[Goal] Total tokens: 91,234 (input: 90,000, cache read: 2,000, cache write: 3,000, output: 1,234)"));
}

#[test]
fn fullscreen_composer_tracks_paste_images_and_steering_status() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.attach_image(PromptAttachment::image(
        1,
        b"image-bytes".to_vec(),
        "image/png",
    ));
    assert_eq!(state.pasted_image_count(), 1);
    assert!(state.input_status_text().contains("image attached"));
    let composer_text = line_texts(&render_composer_lines(&state, 80)).join("\n");
    assert!(composer_text.contains("Attached image 1"));
    assert!(composer_text.contains("image/png"));
    let footer_text = line_texts(&render_footer_lines(&state, 120)).join("\n");
    assert!(footer_text.contains("images:1"));
    assert!(footer_text.contains("Ctrl+U: Clear"));

    let submitted = state.take_submission_prompt().unwrap();
    assert!(submitted.text.contains("Attached image 1"));
    assert_eq!(submitted.attachments.len(), 1);
    assert_eq!(submitted.attachments[0].media_type, "image/png");
    assert_eq!(state.pasted_image_count(), 0);

    state.attach_image(PromptAttachment::image(1, vec![1, 2, 3], "image/png"));
    state.backspace_composer();
    assert_eq!(state.pasted_image_count(), 0);
    assert!(!state.input.contains("Attached image 1"));

    state.attach_image(PromptAttachment::image(1, vec![1, 2, 3], "image/png"));
    state.input = "text only".to_string();
    let text_only = state.take_submission_prompt().unwrap();
    assert_eq!(text_only.text, "text only");
    assert!(text_only.attachments.is_empty());

    state.running = true;
    state.apply_paste("tighten this section");
    assert_eq!(state.input_mode_label(), "STEER");
    let steer_footer = line_texts(&render_footer_lines(&state, 120)).join("\n");
    assert!(steer_footer.contains("STEER"));
    assert!(!steer_footer.contains("Steering messages"));
    let steering = state.take_steering_prompt().unwrap();
    assert_eq!(steering.id, "steer_0");
    assert_eq!(steering.text, "tighten this section");
    assert!(state.input_status_text().contains("steer sent"));
    let pending_steer_footer = line_texts(&render_footer_lines(&state, 120)).join("\n");
    assert!(!pending_steer_footer.contains("tighten this section"));
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
    assert!(!acked_steer_footer.contains("tighten this section"));
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "Steering: tighten this section")
    );
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "Steering received: tighten this section")
    );
}

#[test]
fn snapshot_preserves_canonical_order_across_run_local_sequences() {
    let session_id = SessionId::from_string("session_multi_run");
    let first_run = RunId::from_string("run_first");
    let second_run = RunId::from_string("run_second");
    let messages = vec![
        DisplayMessage::new(
            0,
            session_id.clone(),
            first_run.clone(),
            DisplayMessageKind::AssistantTextDelta,
        )
        .with_payload(json!({"delta": "first-0|"})),
        DisplayMessage::new(
            1,
            session_id.clone(),
            first_run,
            DisplayMessageKind::AssistantTextDelta,
        )
        .with_payload(json!({"delta": "first-1|"})),
        DisplayMessage::new(
            0,
            session_id,
            second_run,
            DisplayMessageKind::AssistantTextDelta,
        )
        .with_payload(json!({"delta": "second-0"})),
    ];

    let snapshot = super::snapshot::TuiSnapshot::from_parts(
        "session_multi_run".to_string(),
        messages,
        &[],
        &[],
    );

    assert_eq!(snapshot.assistant_text, "first-0|first-1|second-0");
    assert_eq!(
        snapshot.transcript_lines,
        [format!(
            "{ASSISTANT_CONTENT_PREFIX}first-0|first-1|second-0"
        )]
    );
}

#[test]
fn snapshot_run_parts_merge_historical_user_prompts() {
    let session_id = SessionId::from_string("session_prompts");
    let first_run = RunId::from_string("run_prompt_first");
    let second_run = RunId::from_string("run_prompt_second");
    let snapshot = super::snapshot::TuiSnapshot::from_run_parts(
        "session_prompts".to_string(),
        vec![
            (
                Some("first prompt".to_string()),
                vec![
                    DisplayMessage::new(
                        0,
                        session_id.clone(),
                        first_run,
                        DisplayMessageKind::AssistantTextDelta,
                    )
                    .with_payload(json!({"delta": "first answer"})),
                ],
            ),
            (
                Some("second\nprompt".to_string()),
                vec![
                    DisplayMessage::new(
                        0,
                        session_id,
                        second_run,
                        DisplayMessageKind::AssistantTextDelta,
                    )
                    .with_payload(json!({"delta": "second answer"})),
                ],
            ),
        ],
        &[],
        &[],
    );

    assert_eq!(
        snapshot.transcript_lines,
        [
            "User: first prompt".to_string(),
            format!("{ASSISTANT_CONTENT_PREFIX}first answer"),
            "User: second".to_string(),
            "  prompt".to_string(),
            format!("{ASSISTANT_CONTENT_PREFIX}second answer"),
        ]
    );
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
    let mut messages = vec![
        DisplayMessage::new(
            2,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::ToolCallStart,
        )
        .with_payload(json!({"tool_name": "lookup", "arguments": {"query": "starweaver"}})),
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
        .with_payload(json!({"tool_name": "lookup", "content": "ok"})),
        DisplayMessage::new(
            5,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::ToolResult,
        )
        .with_payload(
            json!({"tool_name": "lookup", "content": "permission denied", "is_error": true}),
        ),
        DisplayMessage::new(
            6,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::SteeringSubmitted,
        )
        .with_payload(json!({"text": "try another path"})),
        DisplayMessage::new(
            7,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::SteeringReceived,
        )
        .with_payload(json!({"text": "try another path"})),
        DisplayMessage::new(
            8,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::TaskSnapshot,
        )
        .with_payload(json!({
            "tasks": [
                {"id": "1", "subject": "Replay task", "description": "Restore task", "status": "in_progress", "active_form": "Restoring task", "blocked_by": ["2"]}
            ]
        })),
        DisplayMessage::new(
            9,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::HandoffCompleted,
        )
        .with_payload(json!({"content": "Replay handoff summary.\nKeep exact handoff detail."})),
        DisplayMessage::new(
            10,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::CompactionCompleted,
        )
        .with_payload(json!({
            "original_message_count": 8,
            "compacted_message_count": 3,
            "summary": "Replay compaction summary.\nKeep exact compaction detail."
        })),
        DisplayMessage::new(
            11,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::RunCompleted,
        ),
        DisplayMessage::new(
            12,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::RunFailed,
        ),
        DisplayMessage::new(13, session_id, run_id, DisplayMessageKind::RunCancelled),
    ];
    messages.sort_by_key(|message| message.sequence);
    let snapshot = super::snapshot::TuiSnapshot::from_parts(
        "session_snapshot".to_string(),
        messages,
        &approvals,
        &deferred,
    );
    assert_eq!(snapshot.messages, 14);
    assert_eq!(snapshot.pending_approvals, 1);
    assert_eq!(snapshot.pending_deferred, 1);
    assert_eq!(snapshot.assistant_text, "hello world");
    assert!(
        snapshot
            .transcript_lines
            .iter()
            .any(|line| line == &format!("{ASSISTANT_CONTENT_PREFIX}hello world"))
    );
    assert!(
        !snapshot
            .transcript_lines
            .iter()
            .any(|line| line == &format!("{ASSISTANT_CONTENT_PREFIX} world"))
    );
    assert_eq!(
        snapshot.tool_calls,
        vec![
            "lookup {\"query\":\"starweaver\"}",
            "fallback_tool",
            "result:lookup ok",
            "result:error:lookup permission denied"
        ]
    );
    assert_eq!(
        snapshot.steering,
        vec![
            "submitted:try another path".to_string(),
            "received:try another path".to_string(),
        ]
    );
    assert_eq!(snapshot.terminal_status.as_deref(), Some("cancelled"));
    assert_eq!(snapshot.tasks.len(), 1);
    assert_eq!(snapshot.tasks[0].subject, "Replay task");
    let text = snapshot.render_text();
    assert!(text.contains("pending_approvals=1"));
    assert!(text.contains("terminal_status=cancelled"));
    assert!(text.contains("Assistant"));
    assert!(text.contains("- result:lookup ok"));
    assert!(text.contains("- result:error:lookup permission denied"));
    assert!(text.contains("Steering"));
    assert!(text.contains("- submitted:try another path"));

    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let session_affinity_id = state.session_affinity_id.clone();
    state.set_snapshot(&snapshot);
    assert_eq!(state.session_affinity_id, session_affinity_id);
    assert_eq!(state.status, "CANCELLED");
    assert_eq!(state.phase, "replay");
    let assistant_index = state
        .body
        .iter()
        .position(|line| line.contains("hello"))
        .expect("assistant text should be restored");
    let tool_index = state
        .body
        .iter()
        .position(|line| line.starts_with("Tool call: lookup"))
        .expect("tool call should be restored");
    assert!(
        assistant_index < tool_index,
        "restore should preserve display-message sequence order"
    );
    assert!(state.body.iter().any(|line| line == "Tool result: lookup"));
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "Tool error: lookup permission denied")
    );
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "Steering: try another path")
    );
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "Steering received: try another path")
    );
    assert!(body_has_line(&state, "Summary complete"));
    assert!(body_has_line(&state, "    │ Replay handoff summary."));
    assert!(body_has_line(&state, "    │ Keep exact handoff detail."));
    assert!(body_has_line(&state, "Context compacted"));
    assert!(body_has_line(
        &state,
        "  Summary: 8 -> 3 messages (63% reduction)"
    ));
    assert!(body_has_line(&state, "    │ Replay compaction summary."));
    assert!(body_has_line(&state, "    │ Keep exact compaction detail."));
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "Run completed: session_snapshot status=cancelled")
    );
    let footer = line_texts(&render_footer_lines(&state, 120)).join("\n");
    assert!(footer.contains("Tasks"));
    assert!(footer.contains("Replay task"));
    assert!(footer.contains("blocked by #2"));
}

#[test]
fn render_helpers_cover_footer_and_truncation_branches() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.set_profile("coding", "gpt-test");
    state.workspace_dir = "/a/very/long/path/with/中文/segments/and/file".to_string();
    let tiny_history = render_live_history_lines(&state, 3);
    assert!(
        line_texts(&tiny_history)
            .iter()
            .any(|line| line.contains("To get started"))
    );

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
    let composer_text = line_texts(&composer).join(
        "
",
    );
    assert!(composer_text.contains("> draf"));
    assert!(composer_text.contains("  t"));

    let footer_overlay_text = line_texts(&render_footer_lines(&state, 72)).join("\n");
    assert!(!footer_overlay_text.contains("Available Commands"));

    let overlay = render_shortcut_overlay(12);
    let overlay_text = line_texts(&overlay).join("\n");
    let compact_overlay_text = overlay_text.replace(['\n', ' '], "");
    assert!(compact_overlay_text.contains("AvailableCommands"));
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
    assert!(!state.enter_sends());
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Enter)), None);
    assert_eq!(state.input, "\n");
    state.input.clear();
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Tab)), None);
    assert!(state.enter_sends());
    assert_eq!(
        handle_key_event(&mut state, key_modified('p', KeyModifiers::CONTROL)),
        None
    );
    assert_eq!(
        handle_key_event(&mut state, key_modified('n', KeyModifiers::CONTROL)),
        None
    );
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
    assert_eq!(handle_key_event(&mut state, key_char('a')), None);
    assert_eq!(handle_key_event(&mut state, key_char('b')), None);
    assert_eq!(handle_key_event(&mut state, key_char('c')), None);
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Left)), None);
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Left)), None);
    assert_eq!(handle_key_event(&mut state, key_char('X')), None);
    assert_eq!(state.input, "aXbc");
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Right)), None);
    assert_eq!(handle_key_event(&mut state, key_char('Y')), None);
    assert_eq!(state.input, "aXbYc");
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::Home)), None);
    assert_eq!(handle_key_event(&mut state, key_char('^')), None);
    assert_eq!(handle_key_event(&mut state, key_code(KeyCode::End)), None);
    assert_eq!(handle_key_event(&mut state, key_char('!')), None);
    assert_eq!(state.input, "^aXbYc!");
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
    assert_eq!(handle_key_event(&mut state, key_char('q')), None);
    assert_eq!(state.input, "q");
    state.running = true;
    assert_eq!(handle_key_event(&mut state, key_char('q')), None);
    assert_eq!(state.input, "qq");
    state.input.clear();
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
    escape_state
        .body
        .extend(["first".to_string(), "second".to_string()]);
    assert_eq!(
        handle_key_event(&mut escape_state, key_code(KeyCode::Esc)),
        None
    );
    assert!(escape_state.selection_mode_visible());
    assert_eq!(escape_state.input_mode_label(), "SELECT");
    assert_eq!(
        escape_state.selected_line_preview().as_deref(),
        Some("second")
    );
    let select_footer = line_texts(&render_footer_lines(&escape_state, 120)).join("\n");
    assert!(select_footer.contains("SELECT"));
    assert!(select_footer.contains("Enter/Esc: Close selection"));
    assert!(!should_capture_mouse(&escape_state));
    assert_eq!(
        handle_key_event(&mut escape_state, key_code(KeyCode::Up)),
        None
    );
    assert_eq!(
        escape_state.selected_line_preview().as_deref(),
        Some("first")
    );
    assert_eq!(
        handle_key_event(&mut escape_state, key_code(KeyCode::Esc)),
        None
    );
    assert!(!escape_state.selection_mode_visible());
    assert!(should_capture_mouse(&escape_state));
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
fn composer_soft_wraps_long_input_and_tracks_visual_cursor() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.input = "abcdefghij".to_string();

    let rendered = render_composer_lines(&state, 6);
    let texts = line_texts(&rendered);
    assert_eq!(texts[1], "> abcd");
    assert_eq!(texts[2], "  efgh");
    assert_eq!(texts[3], "  ij");
    assert!(rendered.iter().all(|line| line.visible_width() <= 6));

    let input_width = composer_input_width(6);
    assert_eq!(input_width, 4);
    assert_eq!(input_visual_line_count(&state.input, input_width), 3);
    assert_eq!(
        input_viewport_lines_wrapped(&state.input, 2, 0, input_width),
        vec!["efgh".to_string(), "ij".to_string()]
    );
    assert_eq!(
        input_viewport_lines_wrapped(&state.input, 2, 1, input_width),
        vec!["abcd".to_string(), "efgh".to_string()]
    );
    assert_eq!(
        composer_cursor_position_wrapped(&state.input, state.input.len(), input_width),
        (2, 2)
    );
}

#[test]
fn composer_adds_cursor_row_when_input_ends_on_wrap_boundary() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.input = "abcdefgh".to_string();

    let width = 6;
    let input_width = composer_input_width(width);
    let rendered = render_composer_lines(&state, width);
    let texts = line_texts(&rendered);
    assert_eq!(texts[1], "> abcd");
    assert_eq!(texts[2], "  efgh");
    assert_eq!(texts[3], "");
    assert!(rendered.iter().all(|line| line.visible_width() <= width));
    assert_eq!(input_visual_line_count(&state.input, input_width), 3);
    assert_eq!(
        input_viewport_lines_wrapped(&state.input, 5, 0, input_width),
        vec!["abcd".to_string(), "efgh".to_string(), String::new()]
    );
    assert_eq!(
        composer_cursor_position_wrapped(&state.input, state.input.len(), input_width),
        (2, 0)
    );
}

#[test]
fn composer_soft_wraps_wide_characters_by_display_width() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.input = "中文abc".to_string();

    let rendered = render_composer_lines(&state, 6);
    let texts = line_texts(&rendered);
    assert_eq!(texts[1], "> 中文");
    assert_eq!(texts[2], "  abc");
    assert!(rendered.iter().all(|line| line.visible_width() <= 6));
    assert_eq!(
        composer_cursor_position_wrapped(&state.input, state.input.len(), 4),
        (1, 3)
    );
}

#[test]
fn composer_paste_normalizes_terminal_control_sequences_and_preserves_multiline_text() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.apply_paste("first\r\nsecond\tthird\x1b[31m!\x1b[0m\x07");

    assert_eq!(state.input, "first\nsecond    third!");
    assert_eq!(state.composer_cursor_byte(), state.input.len());
    let rendered = render_composer_lines(&state, 20);
    let texts = line_texts(&rendered);
    assert_eq!(texts[1], "> first");
    assert_eq!(texts[2], "  second    third!");
    assert!(rendered.iter().all(|line| line.visible_width() <= 20));
}

#[test]
fn composer_paste_keeps_mixed_text_with_image_paths_as_text() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let pasted = "Please inspect\n/tmp/screenshot.png\nthen summarize";
    state.apply_paste(pasted);

    assert_eq!(state.input, pasted);
    assert_eq!(state.pasted_image_count(), 0);
}

#[test]
fn composer_paste_keeps_image_only_path_paste_behavior() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.apply_paste("'/tmp/screenshot.png' /tmp/second.webp");

    assert_eq!(state.input, "/tmp/screenshot.png /tmp/second.webp");
    assert_eq!(state.pasted_image_count(), 0);
}

#[test]
fn status_bar_secondary_uses_compact_text_on_narrow_widths() {
    let state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let text = line_texts(&render_footer_lines(&state, 36)).join(
        "
",
    );
    assert!(text.contains("Enter Send"));
    assert!(text.contains("Esc Select"));
    assert!(text.contains("Ctrl+C Exit"));
    assert!(!text.contains("Attach clipboard image"));
}

#[test]
fn status_bar_secondary_keeps_pgup_pgdn_hint_untruncated_when_it_fits() {
    let state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let width = 60;
    let lines = render_footer_lines(&state, width);
    assert!(lines.iter().all(|line| line.visible_width() <= width));
    let text = line_texts(&lines).join("\n");
    assert!(text.contains("PgUp/PgDn Scroll"));
    assert!(!text.contains("PgUp/PgDo"));
}

#[test]
fn status_bar_keeps_context_visible_with_long_labels_on_narrow_widths() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.model = "openai-responses:gpt-5-with-a-very-long-routing-profile-name".to_string();
    state.profile = "coding-profile-with-a-very-long-name".to_string();
    state.session_id =
        Some("session_with_a_very_long_identifier_that_should_not_clip_help".to_string());
    state.context_tokens = Some(90_000);
    state.context_window = Some(200_000);

    let width = 48;
    let lines = render_footer_lines(&state, width);
    assert!(
        lines.iter().all(|line| line.visible_width() <= width),
        "line widths: {:?}",
        lines
            .iter()
            .map(|line| (line.visible_width(), line_text(line)))
            .collect::<Vec<_>>()
    );
    let text = line_texts(&lines).join("\n");
    assert!(text.contains("Context: 45%"));
    assert!(text.contains("Enter Send"));
    assert!(text.contains("Ctrl+C Exit"));
    assert!(!text.contains("very-long-routing-profile-name"));
}

#[test]
fn terminal_width_helpers_handle_wide_characters() {
    assert_eq!(visible_width("中文a"), 5);
    assert_eq!(composer_cursor_column(&["中文a".to_string()]), 7);

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

#[test]
fn composer_edits_graphemes_and_moves_across_visual_lines() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.input = "a👨‍👩‍👧‍👦e\u{301}".to_string();

    state.move_composer_cursor_left();
    assert_eq!(state.composer_cursor_byte(), "a👨‍👩‍👧‍👦".len());
    state.backspace_composer();
    assert_eq!(state.input, "ae\u{301}");

    state.input = "abcd\nx\nwxyz".to_string();
    state.move_composer_cursor_to_line_start();
    state.move_composer_cursor_right();
    state.move_composer_cursor_right();
    state.move_composer_cursor_right();
    state.update_composer_content_width(4);
    state.move_composer_cursor_vertical(-1);
    assert_eq!(state.composer_cursor_byte(), "abcd\nx".len());
    state.move_composer_cursor_vertical(-1);
    assert_eq!(state.composer_cursor_byte(), 4);
    state.move_composer_cursor_vertical(1);
    state.move_composer_cursor_vertical(1);
    assert_eq!(state.composer_cursor_byte(), "abcd\nx\nwxy".len());
}

#[test]
fn attachment_placeholder_is_a_cursor_aware_atomic_span() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.input = "prefix ".to_string();
    state.attach_image(PromptAttachment::image(1, vec![1, 2, 3], "image/png"));
    let placeholder = state.pending_attachments[0].placeholder.clone();

    state.move_composer_cursor_to_line_start();
    state.move_composer_cursor_right();
    state.move_composer_cursor_right();
    state.backspace_composer();
    assert_eq!(state.pasted_image_count(), 1);
    assert!(state.input.contains(&placeholder));

    state.move_composer_cursor_to_line_end();
    state.move_composer_cursor_left();
    assert_eq!(
        state.composer_cursor_byte(),
        state.input.find(&placeholder).unwrap()
    );
    state.move_composer_cursor_to_line_end();
    state.backspace_composer();
    assert_eq!(state.pasted_image_count(), 0);
    assert!(!state.input.contains(&placeholder));
}

#[test]
fn notifications_and_paused_output_are_visible_and_follow_can_resume() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    state.toggle_enter_mode();
    let notification = line_texts(&render_footer_lines(&state, 100)).join("\n");
    assert!(notification.contains("Notice: Enter inserts newline"));

    state.scroll_offset = 2;
    state.push_transcript_notice("new output while paused");
    assert!(state.unread_output_lines > 0);
    let paused = line_texts(&render_footer_lines(&state, 100)).join("\n");
    assert!(paused.contains("Output paused"));
    assert!(paused.contains("new"));

    state.scroll_to_bottom();
    assert!(state.is_at_bottom());
    assert_eq!(state.unread_output_lines, 0);
}

#[test]
fn responsive_budget_never_exceeds_real_terminal_height() {
    for height in 1..=20 {
        let budget = responsive_frame_budget(height, 6, 40);
        assert_eq!(
            budget.body + budget.panels + budget.status + budget.composer,
            height
        );
        assert!(budget.composer >= 1);
        if height >= 2 {
            assert!(budget.status >= 1);
        }
        if height >= 3 {
            assert!(budget.body >= 1);
        }
    }
    assert_eq!(responsive_frame_budget(1, 6, 40).composer, 1);
    assert_eq!(responsive_frame_budget(2, 6, 40).status, 1);
}

#[test]
fn color_policy_and_truncation_degrade_explicitly() {
    assert!(!color_output_enabled(Some(OsStr::new("")), None));
    assert!(!color_output_enabled(None, Some(OsStr::new("dumb"))));
    assert!(color_output_enabled(
        None,
        Some(OsStr::new("xterm-256color"))
    ));
    assert_eq!(truncate_line("averylongtoken", 6), "avery…");
    assert_eq!(truncate_line("👨‍👩‍👧‍👦family", 3), "👨‍👩‍👧‍👦…");
}

fn submit_text(event: Option<InteractiveTuiEvent>) -> Option<String> {
    match event {
        Some(InteractiveTuiEvent::Submit(input)) => Some(input.display_text()),
        _ => None,
    }
}

fn body_has_line(state: &InteractiveTuiState, expected: &str) -> bool {
    state
        .body
        .iter()
        .any(|line| body_line_text(line) == expected)
}

fn body_line_index(state: &InteractiveTuiState, expected: &str) -> usize {
    state
        .body
        .iter()
        .position(|line| body_line_text(line) == expected)
        .unwrap_or_else(|| panic!("expected body line: {expected:?}; body={:?}", state.body))
}

fn body_line_count(state: &InteractiveTuiState, expected: &str) -> usize {
    state
        .body
        .iter()
        .filter(|line| body_line_text(line) == expected)
        .count()
}

fn body_line_text(line: &str) -> &str {
    line.strip_prefix(ASSISTANT_CONTENT_PREFIX)
        .or_else(|| line.strip_prefix(CONCISE_TOOL_SUMMARY_PREFIX))
        .unwrap_or(line)
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

fn rendered_content_text(lines: &[StyledLine]) -> String {
    lines
        .iter()
        .flat_map(|line| {
            line.segments
                .iter()
                .filter(|segment| segment.style.contains(SegmentStyle::CYAN))
                .map(|segment| segment.text.as_str())
        })
        .collect::<Vec<_>>()
        .join("")
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

fn key_code_modified(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent {
        code,
        modifiers,
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
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "Steering update pending; continuing run.")
    );
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
                usage: starweaver_usage::Usage::default(),
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

    assert!(
        state
            .body
            .iter()
            .any(|line| body_line_text(line) == "> inspect tools")
    );
    assert!(
        state
            .body
            .iter()
            .any(|line| line == "Tool call: lookup {\"query\":\"starweaver\"}")
    );
    assert!(body_has_line(&state, "done"));
}
