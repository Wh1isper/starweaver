use std::{env, fs, process::Command};

use serde_json::{json, Value};

use crate::{
    common::{read_json, run_capture, run_command, write_json},
    docs::check_docs_examples,
};

pub fn check_cli_examples() -> Result<(), String> {
    let root = crate::common::root()?;
    let examples = root.join("examples/cli");
    let required = [
        "README.md",
        "global-config.toml",
        "project-config.toml",
        "provider-gateway-config.toml",
        "tools.toml",
        "mcp.json",
    ];
    for name in required {
        let path = examples.join(name);
        if !path.exists() {
            return Err(format!("missing CLI example: {}", path.display()));
        }
    }
    for name in [
        "global-config.toml",
        "project-config.toml",
        "provider-gateway-config.toml",
        "tools.toml",
    ] {
        let path = examples.join(name);
        let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        text.parse::<toml::Value>()
            .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    let mcp = examples.join("mcp.json");
    let mcp_value = read_json(&mcp)?;
    if mcp_value.pointer("/servers/docs/transport") != Some(&Value::String("stdio".to_string())) {
        return Err("examples/cli/mcp.json must include a stdio docs server".to_string());
    }
    println!("CLI examples validated");
    Ok(())
}

pub fn check_install_script() -> Result<(), String> {
    let root = crate::common::root()?;
    let install_path = root.join("scripts/install.sh");
    run_command(Command::new("sh").arg("-n").arg(&install_path))?;
    let install_script = fs::read_to_string(&install_path).map_err(|error| error.to_string())?;
    for required in [
        "STARWEAVER_COMPONENTS:-cli",
        "starweaver-cli-$tag-$target",
        "starweaver-claw-$tag-$target",
        "archive missing expected binary",
        "checksums.txt",
        "ln -s \"starweaver\" \"$INSTALL_DIR/sw\"",
    ] {
        if !install_script.contains(required) {
            return Err(format!(
                "installer missing required update/install behavior: {required}"
            ));
        }
    }
    if install_script.contains("need tar\n  tag=") {
        return Err("installer should require tar only for tar archives".to_string());
    }
    println!("install script validated");
    Ok(())
}

pub fn check_repository_scripts() -> Result<(), String> {
    let tmp = env::temp_dir().join(format!("starweaver-script-check-{}", std::process::id()));
    if tmp.exists() {
        fs::remove_dir_all(&tmp).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&tmp).map_err(|error| error.to_string())?;
    let result = (|| {
        check_install_script()?;
        check_cli_examples()?;
        let request = tmp.join("request.json");
        let response = tmp.join("response.json");
        let cassette = tmp.join("cassette.json");
        let scrubbed = tmp.join("cassette.scrubbed.json");
        let fixtures = tmp.join("fixtures");
        write_json(
            &request,
            &json!({
                "provider": "openai_chat",
                "name": "script_smoke",
                "model": "gpt-test",
                "history": [{"role": "user", "content": "hello"}],
                "expected_provider_request": {"model": "gpt-test", "messages": [{"role": "user", "content": "hello"}]},
                "expected_response": {"parts": [{"type": "text", "text": "ok"}], "usage": {"requests": 1}}
            }),
            false,
        )?;
        write_json(
            &response,
            &json!({"id": "chatcmpl-secret123", "created": 123, "choices": [{"message": {"content": "ok"}}]}),
            false,
        )?;
        let dry = run_capture(
            Command::new(env::current_exe().map_err(|error| error.to_string())?)
                .arg("record-model-cassette")
                .arg(&request)
                .arg("--mock-response")
                .arg(&response)
                .arg("--dry-run"),
        )?;
        if !dry.contains("\"provider\": \"openai_chat\"") {
            return Err("record dry run did not include provider".to_string());
        }
        run_command(
            Command::new(env::current_exe().map_err(|error| error.to_string())?)
                .arg("record-model-cassette")
                .arg(&request)
                .arg("--mock-response")
                .arg(&response)
                .arg("--output")
                .arg(&cassette),
        )?;
        let recorded = read_json(&cassette)?;
        if recorded.pointer("/provider_response/id")
            != Some(&Value::String("chatcmpl_REDACTED".to_string()))
        {
            return Err("recorded cassette id was not scrubbed".to_string());
        }
        run_command(
            Command::new(env::current_exe().map_err(|error| error.to_string())?)
                .arg("scrub-model-cassette")
                .arg(&cassette)
                .arg("--output")
                .arg(&scrubbed),
        )?;
        run_command(
            Command::new(env::current_exe().map_err(|error| error.to_string())?)
                .arg("import-model-cassettes")
                .arg(&scrubbed)
                .arg("--fixtures-root")
                .arg(&fixtures),
        )?;
        if !fixtures.join("openai_chat/script_smoke.json").exists() {
            return Err("imported fixture missing".to_string());
        }
        let summary = run_capture(
            Command::new(env::current_exe().map_err(|error| error.to_string())?)
                .arg("summarize-model-fixtures")
                .arg("--fixtures-root")
                .arg(&fixtures),
        )?;
        let summary_json: Value =
            serde_json::from_str(&summary).map_err(|error| error.to_string())?;
        if summary_json.pointer("/providers/openai_chat/count") != Some(&Value::from(1)) {
            return Err("summary count mismatch".to_string());
        }
        check_docs_examples(&[])?;
        Ok(())
    })();
    let _ = fs::remove_dir_all(&tmp);
    result?;
    println!("repository scripts validated");
    Ok(())
}
