#![allow(missing_docs, clippy::unwrap_used)]

use serde_json::{json, Map, Value};
use starweaver_model::{
    providers::{gemini::GeminiGenerateContentAdapter, openai_responses::OpenAiResponsesAdapter},
    NativeToolDefinition,
};

#[test]
fn openai_responses_maps_pydantic_ai_native_tool_kinds() {
    let native_tools = [
        native_tool(
            "web_search_preview",
            [("search_context_size", json!("medium"))],
        ),
        native_tool("code_interpreter", []),
        native_tool("image_generation", [("quality", json!("high"))]),
        native_tool("file_search", [("vector_store_ids", json!(["vs_123"]))]),
        native_tool("web_fetch", [("max_uses", json!(2))]),
        native_tool("memory", [("container", json!("default"))]),
    ];

    let request =
        OpenAiResponsesAdapter::build_request("gpt-4.1-mini", &[], None, &[], &native_tools)
            .unwrap();

    assert_tool(&request, 0, "web_search_preview");
    assert_eq!(request["tools"][0]["search_context_size"], "medium");
    assert_tool(&request, 1, "code_interpreter");
    assert_tool(&request, 2, "image_generation");
    assert_eq!(request["tools"][2]["quality"], "high");
    assert_tool(&request, 3, "file_search");
    assert_eq!(request["tools"][3]["vector_store_ids"][0], "vs_123");
    assert_tool(&request, 4, "web_fetch");
    assert_eq!(request["tools"][4]["max_uses"], 2);
    assert_tool(&request, 5, "memory");
    assert_eq!(request["tools"][5]["container"], "default");
}

#[test]
fn gemini_maps_native_google_search_code_execution_and_generic_tools() {
    let native_tools = [
        NativeToolDefinition::new("google_search"),
        NativeToolDefinition::new("code_execution"),
        native_tool("url_context", [("maxUses", json!(1))]),
    ];

    let request = GeminiGenerateContentAdapter::build_request_with_native_tools(
        &[],
        None,
        &[],
        &native_tools,
    )
    .unwrap();

    assert!(request["tools"].as_array().unwrap()[0]
        .as_object()
        .unwrap()
        .contains_key("googleSearch"));
    assert!(request["tools"].as_array().unwrap()[1]
        .as_object()
        .unwrap()
        .contains_key("codeExecution"));
    assert_eq!(request["tools"][2]["url_context"]["maxUses"], 1);
}

fn native_tool<const N: usize>(
    tool_type: &str,
    entries: [(&str, Value); N],
) -> NativeToolDefinition {
    NativeToolDefinition::new(tool_type).with_config(config(entries))
}

fn config<const N: usize>(entries: [(&str, Value); N]) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn assert_tool(request: &Value, index: usize, tool_type: &str) {
    assert_eq!(request["tools"][index]["type"], tool_type);
}
