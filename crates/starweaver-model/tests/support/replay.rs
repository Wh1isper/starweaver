use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde_json::Value;
use starweaver_model::{
    adapter::{ModelRequestParameters, NativeToolDefinition, ToolDefinition},
    message::{ModelMessage, ModelResponse},
    ModelSettings,
};

#[derive(Debug, serde::Deserialize)]
pub struct RequestFixture {
    pub model: String,
    #[serde(default)]
    pub history: Vec<ModelMessage>,
    #[serde(default)]
    pub settings: Option<ModelSettings>,
    #[serde(default)]
    pub request_parameters: ModelRequestParameters,
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    pub native_tools: Vec<NativeToolDefinition>,
    pub expected_provider_request: Value,
}

#[derive(Debug, serde::Deserialize)]
pub struct ReplayFixture {
    #[serde(flatten)]
    pub request: RequestFixture,
    pub provider_response: Value,
    pub expected_response: ModelResponse,
}

pub fn load_request_fixture(provider: &str, name: &str) -> RequestFixture {
    load_fixture(&fixture_path(provider, name))
}

pub fn load_replay_fixture(provider: &str, name: &str) -> ReplayFixture {
    load_fixture(&fixture_path(provider, name))
}

fn fixture_path(provider: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(provider)
        .join(format!("{name}.json"))
}

fn load_fixture<T: DeserializeOwned>(path: &Path) -> T {
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("failed to read fixture {}: {err}", path.display()));
    serde_json::from_str(&raw)
        .unwrap_or_else(|err| panic!("failed to parse fixture {}: {err}", path.display()))
}

pub fn assert_json_eq(actual: &Value, expected: &Value) {
    assert_eq!(canonical_json(actual), canonical_json(expected));
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonical_json).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), canonical_json(value)))
                .collect(),
        ),
        _ => value.clone(),
    }
}
