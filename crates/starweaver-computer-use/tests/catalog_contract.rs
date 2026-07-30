#![allow(clippy::expect_used)]

//! Canonical Computer Use tool catalog fixture tests.

use starweaver_computer_use::{
    COMPUTER_CLICK_TOOL, COMPUTER_DRAG_TOOL, COMPUTER_MOVE_POINTER_TOOL, COMPUTER_OBSERVE_TOOL,
    COMPUTER_PRESS_KEYS_TOOL, COMPUTER_SCROLL_TOOL, COMPUTER_STATUS_TOOL, COMPUTER_TYPE_TEXT_TOOL,
    ComputerToolCatalog, ComputerToolGrant,
};

#[test]
fn checked_in_catalog_matches_generated_canonical_catalog() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/tool-catalog-v1.json"))
            .expect("catalog fixture should be valid JSON");
    assert_eq!(fixture, ComputerToolCatalog::canonical_fixture());
}

#[test]
fn accessibility_schema_exposes_bounded_snapshot_metadata() {
    let fixture = ComputerToolCatalog::canonical_fixture().to_string();
    assert!(fixture.contains("captured_at_monotonic_ms"));
    assert!(fixture.contains("truncation_reasons"));
    assert!(fixture.contains("node_limit"));
    assert!(fixture.contains("total_string_limit"));
    assert!(fixture.contains("protected"));
}

#[test]
fn catalog_order_and_capability_filtering_are_stable() {
    let full = ComputerToolCatalog::definitions(ComputerToolGrant {
        observe: true,
        pointer: true,
        keyboard: true,
    });
    let names: Vec<&str> = full
        .iter()
        .map(|definition| definition.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            COMPUTER_STATUS_TOOL,
            COMPUTER_OBSERVE_TOOL,
            COMPUTER_CLICK_TOOL,
            COMPUTER_MOVE_POINTER_TOOL,
            COMPUTER_DRAG_TOOL,
            COMPUTER_SCROLL_TOOL,
            COMPUTER_TYPE_TEXT_TOOL,
            COMPUTER_PRESS_KEYS_TOOL,
        ]
    );
    assert!(full[0].input_schema.is_object());
    assert!(
        full.iter()
            .all(|definition| definition.output_schema.is_object())
    );

    let observe_only = ComputerToolCatalog::definitions(ComputerToolGrant::observe_only());
    assert_eq!(
        observe_only
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>(),
        [COMPUTER_STATUS_TOOL, COMPUTER_OBSERVE_TOOL]
    );
}
