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

/// Build release CLI binaries and exercise launcher, setup, run, session, completion, and update dry-run.
pub fn smoke_cli_release() -> Result<(), String> {
    let root = crate::common::root()?;
    run_command(Command::new("cargo").current_dir(&root).args([
        "build",
        "--release",
        "-p",
        "starweaver-cli",
        "--locked",
    ]))?;
    let release = root.join("target/release");
    let starweaver = release.join("starweaver");
    let sw = release.join("sw");
    let cli = release.join("starweaver-cli");
    for binary in [&starweaver, &sw, &cli] {
        if !binary.exists() {
            return Err(format!("missing release binary: {}", binary.display()));
        }
    }
    let version = run_capture(Command::new(&starweaver).arg("version"))?;
    if !version.contains("starweaver-agent-sdk") {
        return Err("launcher version smoke failed".to_string());
    }
    let sw_version = run_capture(Command::new(&sw).args(["cli", "version"]))?;
    if !sw_version.contains("starweaver-agent-sdk") {
        return Err("sw cli version smoke failed".to_string());
    }
    let bash_completion = run_capture(Command::new(&cli).args(["completion", "bash"]))?;
    if !bash_completion.contains("starweaver-cli") {
        return Err("completion smoke failed".to_string());
    }
    let tmp = env::temp_dir().join(format!("starweaver-cli-smoke-{}", std::process::id()));
    if tmp.exists() {
        fs::remove_dir_all(&tmp).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&tmp).map_err(|error| error.to_string())?;
    let result = (|| {
        let global = tmp.join("global");
        let project = tmp.join("project/.starweaver");
        run_command(
            Command::new(&cli)
                .env("STARWEAVER_CONFIG_DIR", &global)
                .env("STARWEAVER_PROJECT_DIR", &project)
                .arg("setup"),
        )?;
        let run = run_capture(
            Command::new(&cli)
                .env("STARWEAVER_CONFIG_DIR", &global)
                .env("STARWEAVER_PROJECT_DIR", &project)
                .args(["run", "hello", "--output", "silent"]),
        )?;
        if !run.contains("status=completed") {
            return Err("release run smoke failed".to_string());
        }
        let sessions = run_capture(
            Command::new(&cli)
                .env("STARWEAVER_CONFIG_DIR", &global)
                .env("STARWEAVER_PROJECT_DIR", &project)
                .args(["session", "list"]),
        )?;
        if !sessions.contains("session_") {
            return Err("release session list smoke failed".to_string());
        }
        let update = run_capture(
            Command::new(&starweaver)
                .env("STARWEAVER_UPDATE_DRY_RUN", "1")
                .env("STARWEAVER_INSTALL_DIR", tmp.join("install"))
                .arg("update"),
        )?;
        if !update.contains("status=dry-run") || !update.contains("target=cli") {
            return Err("release update dry-run smoke failed".to_string());
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&tmp);
    result?;
    println!("CLI release smoke validated");
    Ok(())
}
