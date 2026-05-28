#![allow(missing_docs, clippy::unwrap_used)]

use std::{path::Path, process::Command};

use serde_json::{json, Value};

#[test]
fn cassette_tools_import_scrub_and_summarize_replay_fixtures() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir.parent().unwrap().parent().unwrap();
    let temp_dir = std::env::temp_dir().join(format!(
        "starweaver-model-replay-tooling-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let cassette_path = temp_dir.join("cassette.json");
    std::fs::write(
        &cassette_path,
        serde_json::to_string_pretty(&json!({
            "provider": "openai_chat",
            "name": "tooling_import_check",
            "model": "gpt-4.1-mini",
            "history": [{"kind": "request", "parts": []}],
            "expected_provider_request": {"messages": []},
            "provider_response": {
                "id": "chatcmpl-secret-123",
                "created": 123,
                "headers": {"authorization": "Bearer secret-token"}
            },
            "expected_error": {
                "kind": "response_parsing",
                "message": "missing choices[0]"
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let import_output = Command::new("python3")
        .arg(repo_root.join("scripts/import-model-cassettes.py"))
        .arg(&cassette_path)
        .arg("--fixtures-root")
        .arg(temp_dir.join("fixtures"))
        .arg("--dry-run")
        .output()
        .unwrap();
    assert!(import_output.status.success());
    assert!(String::from_utf8(import_output.stdout)
        .unwrap()
        .contains("tooling_import_check.json"));

    let scrub_output = Command::new("python3")
        .arg(repo_root.join("scripts/scrub-model-cassette.py"))
        .arg(&cassette_path)
        .output()
        .unwrap();
    assert!(scrub_output.status.success());
    let scrubbed: Value = serde_json::from_slice(&scrub_output.stdout).unwrap();
    assert_eq!(
        scrubbed["provider_response"]["headers"]["authorization"],
        "REDACTED"
    );
    assert_eq!(scrubbed["provider_response"]["created"], "NORMALIZED");
    assert_eq!(scrubbed["provider_response"]["id"], "chatcmpl_REDACTED");

    let summary_output = Command::new("python3")
        .arg(repo_root.join("scripts/summarize-model-fixtures.py"))
        .arg("--fixtures-root")
        .arg(manifest_dir.join("tests/fixtures"))
        .output()
        .unwrap();
    assert!(summary_output.status.success());
    let summary: Value = serde_json::from_slice(&summary_output.stdout).unwrap();
    assert!(summary["total"].as_u64().unwrap() >= 60);
    assert!(
        summary["providers"]["openai_chat"]["count"]
            .as_u64()
            .unwrap()
            >= 12
    );
    assert!(
        summary["providers"]["openai_responses"]["count"]
            .as_u64()
            .unwrap()
            >= 16
    );
}
