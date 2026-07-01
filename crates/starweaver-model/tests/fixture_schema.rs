#![allow(missing_docs, clippy::unwrap_used)]

use std::path::Path;

use serde_json::Value;
use starweaver_model::{ModelMessage, ModelResponse, ModelSettings};

#[test]
fn replay_fixtures_match_schema() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut fixture_count = 0;
    for provider in [
        "openai_chat",
        "openai_responses",
        "anthropic",
        "gemini",
        "bedrock",
    ] {
        let dir = root.join(provider);
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            fixture_count += 1;
            let raw = std::fs::read_to_string(&path).unwrap();
            let value: Value = serde_json::from_str(&raw).unwrap();
            if value.get("expected_error").is_some() {
                validate_error_fixture(provider, &path, &value);
            } else if value.get("provider_response").is_some() {
                validate_replay_fixture(provider, &path, &value);
            } else {
                validate_request_fixture(provider, &path, &value);
            }
        }
    }
    assert!(fixture_count >= 60, "expected expanded replay corpus");
}

fn validate_replay_fixture(provider: &str, path: &Path, value: &Value) {
    let Some(object) = value.as_object() else {
        panic!("{} fixture root must be object", path.display());
    };
    validate_request_fields(provider, path, object);
    require_object(object, "provider_response", path);
    require_object(object, "expected_response", path);
    serde_json::from_value::<ModelResponse>(object["expected_response"].clone()).unwrap();
}

fn validate_request_fixture(provider: &str, path: &Path, value: &Value) {
    let Some(object) = value.as_object() else {
        panic!("{} fixture root must be object", path.display());
    };
    validate_request_fields(provider, path, object);
}

fn validate_error_fixture(provider: &str, path: &Path, value: &Value) {
    let Some(object) = value.as_object() else {
        panic!("{} fixture root must be object", path.display());
    };
    validate_request_fields(provider, path, object);
    require_object(object, "provider_response", path);
    require_object(object, "expected_error", path);
    let expected_error = object["expected_error"].as_object().unwrap();
    require_string(expected_error, "kind", path);
    require_string(expected_error, "message", path);
}

fn validate_request_fields(provider: &str, path: &Path, object: &serde_json::Map<String, Value>) {
    require_string(object, "model", path);
    require_array(object, "history", path);
    require_object(object, "expected_provider_request", path);
    serde_json::from_value::<Vec<ModelMessage>>(object["history"].clone()).unwrap();
    if let Some(settings) = object.get("settings") {
        serde_json::from_value::<ModelSettings>(settings.clone()).unwrap();
    }
    if let Some(tools) = object.get("tools") {
        require_value_array(tools, "tools", path);
    }
    if let Some(native_tools) = object.get("native_tools") {
        require_value_array(native_tools, "native_tools", path);
    }
    match provider {
        "openai_chat" | "anthropic" | "bedrock" => assert!(
            object["expected_provider_request"]
                .get("messages")
                .is_some()
        ),
        "openai_responses" => assert!(object["expected_provider_request"].get("input").is_some()),
        "gemini" => assert!(
            object["expected_provider_request"]
                .get("contents")
                .is_some()
        ),
        _ => panic!("unknown provider for {}", path.display()),
    }
}

fn require_string(object: &serde_json::Map<String, Value>, key: &str, path: &Path) {
    assert!(
        object.get(key).and_then(Value::as_str).is_some(),
        "{} must include string {key}",
        path.display()
    );
}

fn require_array(object: &serde_json::Map<String, Value>, key: &str, path: &Path) {
    assert!(
        object.get(key).and_then(Value::as_array).is_some(),
        "{} must include array {key}",
        path.display()
    );
}

fn require_object(object: &serde_json::Map<String, Value>, key: &str, path: &Path) {
    assert!(
        object.get(key).and_then(Value::as_object).is_some(),
        "{} must include object {key}",
        path.display()
    );
}

fn require_value_array(value: &Value, key: &str, path: &Path) {
    assert!(
        value.as_array().is_some(),
        "{} must include array {key}",
        path.display()
    );
}
