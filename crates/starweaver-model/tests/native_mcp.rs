#![allow(missing_docs, clippy::unwrap_used)]

use serde_json::json;
use starweaver_model::{providers::openai_responses::OpenAiResponsesAdapter, NativeToolDefinition};

#[test]
fn maps_provider_native_mcp_tool_to_openai_responses_request() {
    let mut config = serde_json::Map::new();
    config.insert("server_label".to_string(), json!("deepwiki"));
    config.insert(
        "server_url".to_string(),
        json!("https://mcp.deepwiki.com/mcp"),
    );
    config.insert("require_approval".to_string(), json!("never"));
    config.insert("allowed_tools".to_string(), json!(["ask_question"]));

    let request = OpenAiResponsesAdapter::build_request(
        "gpt-4.1-mini",
        &[],
        None,
        &[],
        &[NativeToolDefinition::new("mcp").with_config(config)],
    )
    .unwrap();

    assert_eq!(request["tools"][0]["type"], "mcp");
    assert_eq!(request["tools"][0]["server_label"], "deepwiki");
    assert_eq!(
        request["tools"][0]["server_url"],
        "https://mcp.deepwiki.com/mcp"
    );
    assert_eq!(request["tools"][0]["require_approval"], "never");
    assert_eq!(request["tools"][0]["allowed_tools"][0], "ask_question");
}
