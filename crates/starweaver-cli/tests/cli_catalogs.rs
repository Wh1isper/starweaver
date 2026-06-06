#![allow(missing_docs, clippy::unwrap_used)]

use std::{fs, process::Command};

fn cli(temp: &tempfile::TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_starweaver-cli"));
    command.env("STARWEAVER_PROJECT_DIR", temp.path().join(".starweaver"));
    command.env("STARWEAVER_CONFIG_DIR", temp.path().join("global"));
    command
}

#[test]
fn setup_creates_config_catalogs_and_directories() {
    let temp = tempfile::tempdir().unwrap();
    let output = cli(&temp).arg("setup").output().unwrap();

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(temp.path().join("global/config.toml").exists());
    assert!(temp.path().join("global/tools.toml").exists());
    assert!(temp.path().join("global/mcp.json").exists());
    let global_gitignore = fs::read_to_string(temp.path().join("global/.gitignore")).unwrap();
    assert!(global_gitignore.contains("worktrees/"));
    assert!(global_gitignore.contains("message_history/"));
    assert!(temp.path().join("global/skills").is_dir());
    assert!(temp.path().join("global/subagents").is_dir());
    assert!(!temp.path().join(".starweaver/config.toml").exists());

    let rows = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(rows.iter().any(|row| row["kind"] == "tools"));
    assert!(rows.iter().any(|row| row["kind"] == "mcp"));
    assert!(rows.iter().any(|row| row["kind"] == "global-state-ignore"));
    assert!(!rows.iter().any(|row| row["kind"] == "state-ignore"));

    let project = cli(&temp).args(["setup", "--project"]).output().unwrap();
    assert!(project.status.success());
    assert!(temp.path().join(".starweaver/config.toml").exists());
    assert!(temp.path().join(".starweaver/tools.toml").exists());
    assert!(temp.path().join(".starweaver/mcp.json").exists());
    assert!(temp.path().join(".starweaver/skills").is_dir());
    assert!(temp.path().join(".starweaver/subagents").is_dir());
    let gitignore = fs::read_to_string(temp.path().join(".starweaver/.gitignore")).unwrap();
    assert!(gitignore.contains("state.json"));
    assert!(gitignore.contains("starweaver.sqlite"));
    assert!(gitignore.contains("store/"));

    let second = cli(&temp).arg("setup").output().unwrap();
    assert!(second.status.success());
    let second_rows = String::from_utf8(second.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(second_rows.iter().any(|row| row["status"] == "exists"));
}

#[test]
#[allow(clippy::too_many_lines)]
fn catalog_commands_list_show_and_doctor_configured_assets() {
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join(".starweaver");
    fs::create_dir_all(project_dir.join("skills/example")).unwrap();
    fs::create_dir_all(project_dir.join("subagents")).unwrap();
    fs::write(
        project_dir.join("config.toml"),
        r#"
[skills]
dirs = ["skills"]

[subagents]
dirs = ["subagents"]
"#,
    )
    .unwrap();
    fs::write(
        project_dir.join("skills/example/SKILL.md"),
        r"---
name: example
description: Example skill
---
# Example skill

Use this skill for catalog tests.
",
    )
    .unwrap();
    fs::write(
        project_dir.join("subagents/helper.md"),
        r"---
name: helper
description: Helper subagent
model: inherit
tools: [note, note_get]
optional_tools: [search]
---
You are a helper.
",
    )
    .unwrap();
    fs::write(
        project_dir.join("mcp.json"),
        r#"{
  "servers": {
    "docs": {
      "transport": "stdio",
      "command": "npx",
      "args": ["-y", "@example/docs-mcp"],
      "tools": [
        {"name": "lookup", "description": "Look up docs", "parameters": {"type": "object"}}
      ]
    }
  }
}
"#,
    )
    .unwrap();
    fs::write(
        project_dir.join("tools.toml"),
        r#"
[tools]
need_approval = ["shell", "write"]
"#,
    )
    .unwrap();

    let skills = cli(&temp).args(["skill", "list"]).output().unwrap();
    assert!(skills.status.success());
    assert!(String::from_utf8(skills.stdout)
        .unwrap()
        .contains("\"name\":\"example\""));

    let skill = cli(&temp)
        .args(["skill", "show", "example"])
        .output()
        .unwrap();
    assert!(skill.status.success());
    assert!(String::from_utf8(skill.stdout)
        .unwrap()
        .contains("Use this skill for catalog tests."));

    let subagents = cli(&temp).args(["subagent", "list"]).output().unwrap();
    assert!(subagents.status.success());
    assert!(String::from_utf8(subagents.stdout)
        .unwrap()
        .contains("\"name\":\"helper\""));

    let mcp = cli(&temp).args(["mcp", "list"]).output().unwrap();
    assert!(mcp.status.success());
    assert!(String::from_utf8(mcp.stdout)
        .unwrap()
        .contains("\"name\":\"docs\""));

    let tools = cli(&temp).args(["tools", "list"]).output().unwrap();
    assert!(tools.status.success());
    let tools_stdout = String::from_utf8(tools.stdout).unwrap();
    assert!(tools_stdout.contains("\"name\":\"shell_exec\""));
    assert!(tools_stdout.contains("\"approval_configured\":true"));
    assert!(tools_stdout.contains("\"name\":\"note\""));
    assert!(tools_stdout.contains("\"toolset\":\"mcp_docs\""));

    let diagnostics = cli(&temp).arg("diagnostics").output().unwrap();
    assert!(diagnostics.status.success());
    let diagnostics_stdout = String::from_utf8(diagnostics.stdout).unwrap();
    assert!(diagnostics_stdout.contains("skills=1"));
    assert!(diagnostics_stdout.contains("subagents=1"));
    assert!(diagnostics_stdout.contains("mcp_servers=1"));
    assert!(diagnostics_stdout.contains("tools.need_approval=shell,write"));

    for args in [
        ["skill", "doctor"],
        ["subagent", "doctor"],
        ["tools", "doctor"],
    ] {
        let output = cli(&temp).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8(output.stdout)
            .unwrap()
            .contains("status=ok"));
    }
    let mcp_doctor = cli(&temp).args(["mcp", "doctor"]).output().unwrap();
    assert!(mcp_doctor.status.success());
    let mcp_doctor_stdout = String::from_utf8(mcp_doctor.stdout).unwrap();
    assert!(mcp_doctor_stdout.contains("\"status\":\"ok\""));
}

#[test]
fn mcp_doctor_reports_invalid_server_config() {
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join(".starweaver");
    fs::create_dir_all(&project_dir).unwrap();
    fs::write(
        project_dir.join("mcp.json"),
        r#"{
  "servers": {
    "broken": {"transport": "stdio"}
  }
}
"#,
    )
    .unwrap();

    let output = cli(&temp).args(["mcp", "doctor"]).output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("stdio transport requires command"));
}

#[test]
fn auth_status_and_logout_use_local_oauth_store() {
    let temp = tempfile::tempdir().unwrap();
    let auth_path = temp.path().join("auth.json");
    fs::write(
        &auth_path,
        r#"{
  "version": 1,
  "providers": {
    "codex": {
      "type": "oauth2",
      "issuer": "https://auth.openai.com",
      "client_id": "client",
      "token_endpoint": "https://auth.openai.com/token",
      "scopes": ["openid"],
      "tokens": {"access_token": "access", "refresh_token": "refresh"},
      "account": {"email": "user@example.com"}
    }
  }
}
"#,
    )
    .unwrap();

    let status = cli(&temp)
        .env("STARWEAVER_OAUTH_AUTH_FILE", &auth_path)
        .args(["auth", "status"])
        .output()
        .unwrap();
    assert!(status.status.success());
    let value: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(value["provider"], "codex");
    assert_eq!(value["logged_in"], true);
    assert_eq!(value["record"]["has_access_token"], true);
    assert_eq!(value["record"]["has_refresh_token"], true);

    let logout = cli(&temp)
        .env("STARWEAVER_OAUTH_AUTH_FILE", &auth_path)
        .args(["auth", "logout"])
        .output()
        .unwrap();
    assert!(logout.status.success());
    assert!(String::from_utf8(logout.stdout)
        .unwrap()
        .contains("removed=true"));

    let status = cli(&temp)
        .env("STARWEAVER_OAUTH_AUTH_FILE", &auth_path)
        .args(["auth", "status"])
        .output()
        .unwrap();
    assert!(status.status.success());
    let value: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(value["logged_in"], false);
}
