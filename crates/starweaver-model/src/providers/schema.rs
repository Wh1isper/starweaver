//! Provider tool schema helpers.

use serde_json::{json, Map, Value};

pub fn provider_tool_parameters(parameters: &Value) -> Value {
    let mut schema = parameters.clone();
    remove_schema_meta(&mut schema);
    schema
}

pub fn insert_optional_description(object: &mut Map<String, Value>, description: Option<&String>) {
    if let Some(description) = description
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        object.insert("description".to_string(), json!(description));
    }
}

fn remove_schema_meta(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("$schema");
            for nested in object.values_mut() {
                remove_schema_meta(nested);
            }
        }
        Value::Array(items) => {
            for item in items {
                remove_schema_meta(item);
            }
        }
        _ => {}
    }
}
