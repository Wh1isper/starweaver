#![allow(missing_docs, clippy::unwrap_used)]

use std::collections::BTreeMap;

use starweaver_context::{AgentContext, NoteStore, ResumableState};

#[test]
fn note_store_sets_gets_updates_and_deletes() {
    let mut notes = NoteStore::new();
    notes.set("lang", "Chinese");
    assert_eq!(notes.get("lang"), Some("Chinese"));
    assert_eq!(notes.get("missing"), None);

    notes.set("lang", "English");
    assert_eq!(notes.get("lang"), Some("English"));

    assert!(notes.delete("lang"));
    assert_eq!(notes.get("lang"), None);
    assert!(!notes.delete("lang"));
}

#[test]
fn note_store_lists_entries_sorted_by_key() {
    let mut notes = NoteStore::new();
    notes.set("z-key", "last");
    notes.set("a-key", "first");
    notes.set("m-key", "middle");

    assert_eq!(
        notes.list_all(),
        vec![
            ("a-key".to_string(), "first".to_string()),
            ("m-key".to_string(), "middle".to_string()),
            ("z-key".to_string(), "last".to_string()),
        ]
    );
}

#[test]
fn note_store_exports_and_restores() {
    let mut notes = NoteStore::new();
    notes.set("lang", "Chinese");
    notes.set("os", "macOS");

    let exported = notes.export_notes();
    let restored = NoteStore::from_exported(exported.clone());

    assert_eq!(
        exported,
        BTreeMap::from([
            ("lang".to_string(), "Chinese".to_string()),
            ("os".to_string(), "macOS".to_string()),
        ])
    );
    assert_eq!(restored.get("lang"), Some("Chinese"));
    assert_eq!(restored.get("os"), Some("macOS"));
}

#[test]
fn notes_round_trip_through_resumable_state() {
    let mut state = ResumableState::default();
    state.notes.set("lang", "Chinese");
    state.notes.set("os", "macOS");

    let encoded = serde_json::to_string(&state).unwrap();
    let restored: ResumableState = serde_json::from_str(&encoded).unwrap();

    assert_eq!(restored.notes.get("lang"), Some("Chinese"));
    assert_eq!(restored.notes.get("os"), Some("macOS"));
}

#[test]
fn context_export_and_restore_includes_notes() {
    let mut context = AgentContext::default();
    context.notes.set("lang", "Chinese");
    context.notes.set("os", "macOS");

    let restored = AgentContext::from_state(context.export_state());

    assert_eq!(restored.notes.get("lang"), Some("Chinese"));
    assert_eq!(restored.notes.get("os"), Some("macOS"));
}

#[test]
fn context_instructions_include_note_keys_for_user_prompts() {
    let mut context = AgentContext::default();
    context.notes.set("lang", "Chinese");
    context.notes.set("os", "macOS");

    let instructions = context.context_instructions(true).unwrap();

    assert!(instructions.contains("<notes "));
    assert!(instructions.contains("key=\"lang\""));
    assert!(instructions.contains("key=\"os\""));
    assert!(!instructions.contains("Chinese"));
    assert!(!instructions.contains("macOS"));
}

#[test]
fn context_instructions_escape_note_keys() {
    let mut context = AgentContext::default();
    context.notes.set("evil\"'><tag>&", "hidden");

    let instructions = context.context_instructions(true).unwrap();

    assert!(instructions.contains("evil&quot;&apos;&gt;&lt;tag&gt;&amp;"));
    assert!(!instructions.contains("evil\"'><tag>&"));
}

#[test]
fn context_instructions_include_runtime_context_without_leaking_tool_response_notes() {
    let mut context = AgentContext::default();
    let empty_user_context = context.context_instructions(true).unwrap();
    assert!(empty_user_context.contains("<runtime-context>"));
    assert!(empty_user_context.contains("<current-time>"));
    assert!(!empty_user_context.contains("<notes "));

    context.notes.set("lang", "Chinese");
    let tool_context = context.context_instructions(false).unwrap();
    assert!(tool_context.contains("<runtime-context>"));
    assert!(!tool_context.contains("<notes "));
    assert!(!tool_context.contains("lang"));
    assert!(!tool_context.contains("Chinese"));
}
