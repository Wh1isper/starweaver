use serde_json::{Value, json};

use crate::{
    adapter::{NativeToolDefinition, ToolDefinition},
    providers::{insert_nonempty_description, provider_tool_schema_without_meta},
};

pub(super) fn response_tool_defs(
    tools: &[ToolDefinition],
    native_tools: &[NativeToolDefinition],
) -> Vec<Value> {
    let mut definitions = tools
        .iter()
        .map(|tool| {
            let mut definition = serde_json::Map::new();
            definition.insert("type".to_string(), json!("function"));
            definition.insert("name".to_string(), json!(tool.name));
            insert_nonempty_description(&mut definition, tool.description.as_ref());
            definition.insert(
                "parameters".to_string(),
                provider_tool_schema_without_meta(&tool.parameters),
            );
            if let Some(strict) = tool.strict {
                definition.insert("strict".to_string(), json!(strict));
            }
            Value::Object(definition)
        })
        .collect::<Vec<_>>();
    definitions.extend(native_tools.iter().map(native_response_tool_def));
    definitions
}

fn native_response_tool_def(tool: &NativeToolDefinition) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("type".to_string(), json!(tool.tool_type));
    for (key, value) in &tool.config {
        object.insert(key.clone(), value.clone());
    }
    Value::Object(object)
}
