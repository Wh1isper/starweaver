#![allow(clippy::unwrap_used)]

use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use starweaver_core::{ConversationId, RunId};
use starweaver_model::{ModelResponseStreamEvent, PartDelta, PartStart};
use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord};

use super::{
    markdown::{render_markdown_lines, render_transcript_lines},
    render::{
        composer_cursor_column, input_tail_lines, render_composer_lines, render_footer_line,
        render_live_history_lines, render_shortcut_overlay, visible_width, SegmentStyle,
        StyledLine,
    },
    state::{InteractiveTuiState, RunMode},
    terminal::{handle_key_event, InteractiveTuiEvent},
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
            event: ModelResponseStreamEvent::PartDelta(PartDelta {
                index: 0,
                delta: "hello\nworld".to_string(),
            }),
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
        .any(|line| line.contains("/model to change")));
    assert!(line_texts(&history)
        .iter()
        .any(|line| line.contains("directory:")));
    assert!(line_texts(&history)
        .iter()
        .any(|line| line.contains("To get started")));

    let composer = render_composer_lines(&state, 80);
    let composer_text = line_texts(&composer).join("\n");
    assert!(composer_text.contains("› Ask Starweaver to do anything"));

    let footer = render_footer_line(&state, 80);
    let footer_text = line_text(&footer);
    assert!(footer_text.starts_with("  ? for shortcuts"));
    assert!(footer_text.ends_with("100% context left"));
}

#[test]
fn codex_style_shortcut_overlay_matches_footer_model() {
    let overlay = render_shortcut_overlay(100);
    let text = line_texts(&overlay).join("\n");
    assert!(text.contains("enter to submit message"));
    assert!(text.contains("tab to submit or queue"));
    assert!(text.contains("ctrl + r previous prompt"));
    assert!(text.contains("shift + tab to change mode"));
    assert!(text.contains("shortcuts match the active Starweaver TUI"));
}

#[test]
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

    assert!(!state.footer_mode.is_shortcuts());
    assert_eq!(handle_key_event(&mut state, key_char('?')), None);
    assert!(state.footer_mode.is_shortcuts());

    state
        .body
        .extend((0..30).map(|line| format!("line {line}")));
    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::PageUp)),
        None
    );
    assert_eq!(state.scroll_offset, 10);
    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::PageDown)),
        None
    );
    assert_eq!(state.scroll_offset, 0);
    state.scroll_offset = 3;
    assert_eq!(
        handle_key_event(&mut state, key_modified('l', KeyModifiers::CONTROL)),
        None
    );
    assert_eq!(state.scroll_offset, 0);

    assert_eq!(state.run_mode, RunMode::Act);
    assert_eq!(
        handle_key_event(&mut state, key_code(KeyCode::BackTab)),
        None
    );
    assert_eq!(state.run_mode, RunMode::Plan);

    state.running = true;
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
fn checkpoint_events_update_phase_without_output_noise() {
    let mut state = InteractiveTuiState::welcome(Path::new("/tmp/config"));
    let initial_lines = state.body.len();
    state.apply_stream_record(&AgentStreamRecord::new(
        0,
        AgentStreamEvent::Checkpoint {
            node: starweaver_runtime::AgentExecutionNode::RunStart,
            step: 0,
        },
    ));
    assert_eq!(state.phase, "checkpoint:runstart");
    assert_eq!(state.body.len(), initial_lines);
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
fn transcript_renderer_renders_only_assistant_markdown() {
    let lines = vec![
        "User: # raw prompt".to_string(),
        "Assistant:".to_string(),
        "# Title".to_string(),
        "- item".to_string(),
        "Run completed: run_test status=completed".to_string(),
    ];
    let rendered = render_transcript_lines(&lines, 40);
    assert!(rendered.iter().any(|line| line
        .segments
        .first()
        .is_some_and(|segment| segment.text == "User: # raw prompt")));
    assert!(has_segment(&rendered, "Title", SegmentStyle::BOLD));
    assert!(rendered.iter().any(|line| line
        .segments
        .first()
        .is_some_and(|segment| segment.text == "• ")));
    assert!(rendered.iter().any(|line| line
        .segments
        .first()
        .is_some_and(|segment| segment.text == "Run completed: run_test status=completed")));
}

#[test]
fn input_tail_preserves_trailing_empty_line() {
    assert_eq!(input_tail_lines("a\nb\n", 3), vec!["a", "b", ""]);
    assert_eq!(input_tail_lines("", 3), vec![""]);
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

fn has_segment(lines: &[StyledLine], text: &str, style: u8) -> bool {
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
