#![allow(missing_docs, clippy::expect_used)]

use serde_json::Value;
use starweaver_context::{AgentCheckpoint, AgentRunState};
use starweaver_core::{
    AgentExecutionNode, ConversationId, RunId, VersionedRecordError, from_versioned_json,
    to_versioned_json, to_versioned_value,
};
use starweaver_model::{
    CONTEXT_ORIGIN_METADATA, CONTEXT_ORIGIN_TOOL_RETURN_MEDIA, ContentPart, ModelMessage,
    ModelRequest, ModelRequestPart, ToolReturnPart,
};

const CHECKPOINT_V0: &str = include_str!("fixtures/contracts/checkpoint-v0.json");
const CHECKPOINT_V1: &str = include_str!("fixtures/contracts/checkpoint-v1.json");
const CHECKPOINT_UNKNOWN: &str = include_str!("fixtures/contracts/checkpoint-unknown-version.json");
const CHECKPOINT_WRONG_SCHEMA: &str =
    include_str!("fixtures/contracts/checkpoint-wrong-schema.json");

#[test]
fn checkpoint_owner_reads_v0_and_v1_and_writes_current_envelope() {
    let legacy = from_versioned_json::<AgentCheckpoint>(CHECKPOINT_V0).expect("read v0 checkpoint");
    let current =
        from_versioned_json::<AgentCheckpoint>(CHECKPOINT_V1).expect("read v1 checkpoint");

    assert_eq!(legacy, current);
    assert_eq!(
        to_versioned_value(&legacy).expect("write current checkpoint"),
        serde_json::from_str::<Value>(CHECKPOINT_V1).expect("parse current fixture")
    );
}

#[test]
fn checkpoint_owner_rejects_unknown_versions_and_wrong_schemas() {
    assert!(matches!(
        from_versioned_json::<AgentCheckpoint>(CHECKPOINT_UNKNOWN),
        Err(VersionedRecordError::UnsupportedVersion { actual: 2, .. })
    ));
    assert!(matches!(
        from_versioned_json::<AgentCheckpoint>(CHECKPOINT_WRONG_SCHEMA),
        Err(VersionedRecordError::WrongSchema { .. })
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
fn checkpoint_projection_excludes_process_local_computer_use_screenshot_only() {
    const SCREENSHOT_DATA_URL: &str = "data:image/png;base64,U0NSRUVOU0hPVF9CWVRFU19TRU5USU5FTA==";
    const ORDINARY_DATA_URL: &str = "data:image/png;base64,T1JESU5BUllfTUVESUE=";
    let screenshot_metadata = serde_json::Map::from_iter([
        (
            "starweaver_tool_return_content_parts".to_string(),
            serde_json::json!([{
                "kind": "data_url",
                "data_url": SCREENSHOT_DATA_URL,
                "media_type": "image/png"
            }]),
        ),
        (
            "starweaver_tool_return_prompt".to_string(),
            serde_json::json!("exact process-local screenshot"),
        ),
        (
            "starweaver_geometry_bound_immutable_media".to_string(),
            serde_json::json!(true),
        ),
        (
            "application_private_metadata".to_string(),
            serde_json::json!({"preserve": true}),
        ),
    ]);
    let ordinary_metadata = serde_json::Map::from_iter([
        (
            "starweaver_tool_return_content_parts".to_string(),
            serde_json::json!([{
                "kind": "data_url",
                "data_url": ORDINARY_DATA_URL,
                "media_type": "image/png"
            }]),
        ),
        (
            "application_private_metadata".to_string(),
            serde_json::json!({"preserve": "ordinary"}),
        ),
    ]);
    let screenshot_return = ToolReturnPart::new(
        "call-screenshot",
        "computer_observe",
        serde_json::json!({
            "observation_id": "observation-1"
        }),
    )
    .with_private_metadata(screenshot_metadata);
    let ordinary_return = ToolReturnPart::new(
        "call-ordinary",
        "ordinary_media_tool",
        serde_json::json!({"ok": true}),
    )
    .with_private_metadata(ordinary_metadata);
    let media_prompt = ModelRequestPart::UserPrompt {
        content: vec![
            ContentPart::Text {
                text: "exact attached screenshot".to_string(),
            },
            ContentPart::DataUrl {
                data_url: SCREENSHOT_DATA_URL.to_string(),
                media_type: "image/png".to_string(),
            },
        ],
        name: None,
        metadata: serde_json::Map::from_iter([
            (
                CONTEXT_ORIGIN_METADATA.to_string(),
                serde_json::json!(CONTEXT_ORIGIN_TOOL_RETURN_MEDIA),
            ),
            (
                "starweaver_geometry_bound_immutable_media".to_string(),
                serde_json::json!(true),
            ),
        ]),
    };
    let mut state = AgentRunState::new(
        RunId::from_string("run-screenshot-projection"),
        ConversationId::from_string("conversation-screenshot-projection"),
    );
    state
        .message_history
        .push(ModelMessage::Request(ModelRequest {
            parts: vec![
                ModelRequestPart::ToolReturn(screenshot_return.clone()),
                media_prompt,
                ModelRequestPart::ToolReturn(ordinary_return.clone()),
            ],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        }));
    state.pending_tool_returns.push(screenshot_return);

    let checkpoint = AgentCheckpoint::new(AgentExecutionNode::ToolReturn, &state);
    let encoded = to_versioned_json(&checkpoint).expect("serialize projected checkpoint");
    assert!(!encoded.contains(SCREENSHOT_DATA_URL));
    assert!(encoded.contains(ORDINARY_DATA_URL));
    assert!(
        serde_json::to_string(&state)
            .expect("serialize live state")
            .contains(SCREENSHOT_DATA_URL)
    );

    let restored =
        from_versioned_json::<AgentCheckpoint>(&encoded).expect("restore projected checkpoint");
    let ModelMessage::Request(request) = &restored.state.message_history[0] else {
        panic!("request history expected");
    };
    assert_eq!(
        request.parts.len(),
        2,
        "generated screenshot carrier is ephemeral"
    );
    let ModelRequestPart::ToolReturn(projected_screenshot) = &request.parts[0] else {
        panic!("projected screenshot tool return expected");
    };
    assert!(
        !projected_screenshot
            .private_metadata
            .contains_key("starweaver_tool_return_content_parts")
    );
    assert_eq!(
        projected_screenshot
            .private_metadata
            .get("application_private_metadata"),
        Some(&serde_json::json!({"preserve": true}))
    );
    assert_eq!(
        projected_screenshot
            .private_metadata
            .get("starweaver_tool_return_prompt"),
        Some(&serde_json::json!("exact process-local screenshot"))
    );
    let ModelRequestPart::ToolReturn(projected_ordinary) = &request.parts[1] else {
        panic!("ordinary tool return expected");
    };
    assert_eq!(projected_ordinary, &ordinary_return);
    assert!(
        !restored.state.pending_tool_returns[0]
            .private_metadata
            .contains_key("starweaver_tool_return_content_parts")
    );
}
