#![allow(clippy::missing_errors_doc)]
//! CLI-first local product surface for Starweaver.

mod args;
mod config;
mod environment;
mod error;
pub mod launcher;
mod local_store;
mod oauth;
mod profiles;
mod runner;
mod service;
mod tui;
mod update_check;

use std::env;

pub use args::{Cli, CliCommand, OutputMode, SessionCommand};
pub use config::{CliConfig, ConfigResolver};
pub use error::{CliError, CliResult};
pub use local_store::{LocalStore, TrimReport};
pub use service::CliService;

/// Run the CLI from process arguments.
pub fn run_from_env() -> CliResult<()> {
    run(env::args())
}

/// Run the CLI from an argument iterator.
pub fn run(args: impl IntoIterator<Item = String>) -> CliResult<()> {
    let output = command_output(args)?;
    print!("{output}");
    Ok(())
}

/// Return command output for tests and host integrations.
pub fn command_output(args: impl IntoIterator<Item = String>) -> CliResult<String> {
    let cli = args::parse(args)?;
    let config = ConfigResolver::default().resolve(&cli)?;
    let show_update_hint = should_show_update_hint(&cli, &config);
    if show_update_hint {
        update_check::spawn_update_check_if_due(&config);
    }
    let hint = show_update_hint.then(|| update_check::update_hint(&config));
    let service = CliService::open(config)?;
    let mut output = service.execute(cli)?;
    if let Some(Some(hint)) = hint {
        output.push_str(&hint);
    }
    Ok(output)
}

const fn should_show_update_hint(cli: &Cli, config: &CliConfig) -> bool {
    matches!(config.default_output, OutputMode::Text | OutputMode::Silent)
        && matches!(
            &cli.command,
            None | Some(
                CliCommand::Version
                    | CliCommand::Diagnostics
                    | CliCommand::ReplayCheck
                    | CliCommand::Run(_),
            )
        )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::{ffi::OsString, io, path::Path};

    use super::*;

    fn output(root: &Path, raw_args: &[&str]) -> CliResult<String> {
        let mut command_args = vec!["starweaver-cli".to_string()];
        command_args.extend(raw_args.iter().map(|arg| (*arg).to_string()));
        let cli = args::parse(command_args)?;
        let config = ConfigResolver::for_tests(root).resolve(&cli)?;
        CliService::open(config)?.execute(cli)
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn args_and_error_helpers_cover_edge_branches() {
        let run = args::RunCommand {
            prompt: Some(" explicit ".to_string()),
            prompt_parts: vec!["ignored".to_string()],
            continue_session: false,
            session: None,
            new_session: false,
            run: None,
            branch_from: None,
            profile: None,
            output: None,
            hitl: None,
            worker: None,
            worker_label: None,
            worktree: None,
            worktree_name: None,
            branch: None,
        };
        assert_eq!(run.prompt_text().unwrap(), " explicit ");

        let joined = args::RunCommand {
            prompt: None,
            prompt_parts: vec!["hello".to_string(), "world".to_string()],
            continue_session: false,
            session: None,
            new_session: false,
            run: None,
            branch_from: None,
            profile: None,
            output: None,
            hitl: None,
            worker: None,
            worker_label: None,
            worktree: None,
            worktree_name: None,
            branch: None,
        };
        assert_eq!(joined.prompt_text().unwrap(), "hello world");

        let empty = args::RunCommand {
            prompt: Some("   ".to_string()),
            prompt_parts: Vec::new(),
            continue_session: false,
            session: None,
            new_session: false,
            run: None,
            branch_from: None,
            profile: None,
            output: None,
            hitl: None,
            worker: None,
            worker_label: None,
            worktree: None,
            worktree_name: None,
            branch: None,
        };
        assert!(
            matches!(empty.prompt_text(), Err(CliError::Usage(message)) if message.contains("run -p"))
        );

        let parsed = args::parse_os([
            OsString::from("starweaver-cli"),
            OsString::from("run"),
            OsString::from("hello"),
        ])
        .unwrap();
        assert!(matches!(parsed.command, Some(args::CliCommand::Run(_))));

        let parsed = args::parse_os([
            OsString::from("starweaver-cli"),
            OsString::from("-p"),
            OsString::from("hello"),
            OsString::from("-s"),
            OsString::from("session_test"),
            OsString::from("--model-profile"),
            OsString::from("coding"),
            OsString::from("--worker"),
            OsString::from("off"),
            OsString::from("--worktree"),
            OsString::from("feature"),
            OsString::from("--branch"),
            OsString::from("feature/parity"),
        ])
        .unwrap();
        assert_eq!(parsed.session.as_deref(), Some("session_test"));
        assert_eq!(parsed.profile.as_deref(), Some("coding"));
        assert_eq!(parsed.worker.as_deref(), Some("off"));
        assert_eq!(parsed.worktree.as_deref(), Some("feature"));
        assert_eq!(parsed.branch.as_deref(), Some("feature/parity"));

        let parsed = args::parse_os([
            OsString::from("starweaver-cli"),
            OsString::from("-p"),
            OsString::from("hello"),
            OsString::from("--worker"),
            OsString::from("-w"),
            OsString::from("--worker-label"),
            OsString::from("executor"),
            OsString::from("--worktree-name"),
            OsString::from("feature"),
        ])
        .unwrap();
        assert_eq!(parsed.worker.as_deref(), Some("true"));
        assert_eq!(parsed.worker_label.as_deref(), Some("executor"));
        assert_eq!(parsed.worktree.as_deref(), Some("true"));
        assert_eq!(parsed.worktree_name.as_deref(), Some("feature"));

        let parse_error =
            args::parse_os([OsString::from("starweaver-cli"), OsString::from("--bad")]);
        assert!(
            matches!(parse_error, Err(CliError::Usage(message)) if message.contains("unexpected argument"))
        );

        assert!(format!(
            "{}",
            CliError::from(serde_json::from_str::<serde_json::Value>("{").unwrap_err())
        )
        .contains("serialization error"));
        assert!(format!(
            "{}",
            CliError::from(toml::from_str::<toml::Value>("=").unwrap_err())
        )
        .contains("configuration error"));
        assert!(format!(
            "{}",
            CliError::from(toml::to_string(&f64::NAN).unwrap_err())
        )
        .contains("configuration error"));
        let io_error = error::io_error(
            "/tmp/missing",
            io::Error::new(io::ErrorKind::NotFound, "gone"),
        );
        assert!(format!("{io_error}").contains("filesystem error at /tmp/missing"));
    }

    #[test]
    fn version_and_diagnostics_work() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            output(temp.path(), &["version"]).unwrap(),
            "starweaver-agent-sdk\n"
        );
        let diagnostics = output(temp.path(), &["diagnostics"]).unwrap();
        assert!(diagnostics.contains("sdk=starweaver-agent-sdk"));
        assert!(diagnostics.contains("database_path="));
        assert!(diagnostics.contains("model_profiles="));
        assert!(diagnostics.contains("wal=true"));
    }

    #[test]
    fn config_model_profiles_work() {
        let temp = tempfile::tempdir().unwrap();
        let global = temp.path().join("global");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(
            global.join("config.toml"),
            r#"
[general]
model = "homelab@openai-responses:gpt-5.5"
model_settings = "openai_responses_high"
model_cfg = "gpt5_270k"

[model_profiles.codex-subs]
label = "Codex Subs"
model = "oauth@codex:gpt-5.5"
model_settings = "openai_responses_high"
model_cfg = "gpt5_270k"

[providers.homelab]
base_url = "https://gateway.example/v1"
max_tokens_parameter = "omit"

[env]
HOMELAB_API_KEY = "test-key"
"#,
        )
        .unwrap();
        let diagnostics = output(temp.path(), &["diagnostics"]).unwrap();
        assert!(diagnostics.contains("profile=default_model"));
        assert!(diagnostics.contains("model_profiles=1"));
        assert_eq!(
            output(
                temp.path(),
                &["config", "get", "providers.homelab.max_tokens_parameter"]
            )
            .unwrap(),
            "omit\n"
        );
        let profiles = output(temp.path(), &["profile", "list"]).unwrap();
        assert!(profiles.contains("default_model"));
        assert!(profiles.contains("codex-subs"));
        let default_profile = output(temp.path(), &["profile", "show", "default_model"]).unwrap();
        assert!(default_profile.contains("model_id: homelab@openai-responses:gpt-5.5"));
        assert!(default_profile.contains("settings_preset: openai_responses_high"));
        assert!(default_profile.contains("config_preset: gpt5_270k"));
        assert!(default_profile.contains("# source: config"));
    }

    #[test]
    fn configured_subagent_inherits_profile_model() {
        let temp = tempfile::tempdir().unwrap();
        let global = temp.path().join("global");
        let project = temp.path().join("project/.starweaver");
        std::fs::create_dir_all(global.join("subagents")).unwrap();
        std::fs::write(
            global.join("config.toml"),
            r#"
[general]
model = "local_echo"

[subagents]
dirs = ["subagents"]
"#,
        )
        .unwrap();
        std::fs::write(
            global.join("subagents/helper.md"),
            r"---
name: helper
description: Helper subagent
model: inherit
---
You are a helper.
",
        )
        .unwrap();

        let cli = args::parse([
            "starweaver-cli".to_string(),
            "-p".to_string(),
            "hello".to_string(),
            "--profile".to_string(),
            "default_model".to_string(),
        ])
        .unwrap();
        let config = ConfigResolver::for_tests(temp.path())
            .resolve(&cli)
            .unwrap();
        assert_eq!(config.project_dir, project);
        let profile = crate::profiles::resolve_profile(&config, Some("default_model")).unwrap();
        let agent = profile.build_agent().unwrap();
        let tools = agent.tools().names();
        assert!(tools.contains(&"delegate".to_string()));
        assert!(tools.contains(&"subagent_info".to_string()));

        let run = output(
            temp.path(),
            &[
                "-p",
                "hello",
                "--profile",
                "default_model",
                "--output",
                "silent",
            ],
        )
        .unwrap();
        assert!(run.contains("status=completed"));
    }

    #[test]
    fn headless_run_creates_session_and_run() {
        let temp = tempfile::tempdir().unwrap();
        let first = output(temp.path(), &["-p", "hello", "--output", "display-jsonl"]).unwrap();
        let first_message: serde_json::Value =
            serde_json::from_str(first.lines().next().unwrap()).unwrap();
        assert_eq!(first_message["schema"], "starweaver.display.v1");
        assert_eq!(first_message["type"], "RUN_QUEUED");
        let agui_temp = tempfile::tempdir().unwrap();
        let agui = output(agui_temp.path(), &["-p", "hello", "--output", "agui-jsonl"]).unwrap();
        let agui_events = agui
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert!(agui_events
            .iter()
            .any(|event| event["type"] == "RUN_STARTED"));
        assert!(agui_events
            .iter()
            .any(|event| event["type"] == "TEXT_MESSAGE_CHUNK"));
        assert!(agui_events
            .iter()
            .any(|event| event["type"] == "RUN_FINISHED"));
        let sessions = output(temp.path(), &["session", "list"]).unwrap();
        assert!(sessions.contains("session_"));
        let value: serde_json::Value =
            serde_json::from_str(sessions.lines().next().unwrap()).unwrap();
        assert_eq!(value["run_count"], 1);
        assert_eq!(value["head_success_run_id"], value["head_run_id"]);
    }

    #[test]
    fn continue_appends_run_under_existing_session() {
        let temp = tempfile::tempdir().unwrap();
        output(temp.path(), &["-p", "one"]).unwrap();
        output(temp.path(), &["-p", "two", "--continue"]).unwrap();
        let sessions = output(temp.path(), &["session", "list"]).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(sessions.lines().next().unwrap()).unwrap();
        assert_eq!(value["run_count"], 2);
        let session_id = value["session_id"].as_str().unwrap();
        let show = output(temp.path(), &["session", "show", session_id]).unwrap();
        assert_eq!(show.lines().count(), 3);
    }

    #[test]
    fn replay_and_trim_work() {
        let temp = tempfile::tempdir().unwrap();
        output(temp.path(), &["-p", "one"]).unwrap();
        output(temp.path(), &["-p", "two", "--continue"]).unwrap();
        output(temp.path(), &["-p", "three", "--continue"]).unwrap();
        let sessions = output(temp.path(), &["session", "list"]).unwrap();
        let session: serde_json::Value =
            serde_json::from_str(sessions.lines().next().unwrap()).unwrap();
        let session_id = session["session_id"].as_str().unwrap();
        let replay = output(temp.path(), &["session", "replay", session_id]).unwrap();
        assert!(replay.contains("RUN_FINISHED"));
        let dry = output(
            temp.path(),
            &[
                "session",
                "trim",
                "--session",
                session_id,
                "--keep-runs",
                "1",
                "--dry-run",
            ],
        )
        .unwrap();
        let report: serde_json::Value = serde_json::from_str(dry.trim()).unwrap();
        assert_eq!(report["runs_to_trim"], 2);
        output(
            temp.path(),
            &[
                "session",
                "trim",
                "--session",
                session_id,
                "--keep-runs",
                "1",
            ],
        )
        .unwrap();
        let show = output(temp.path(), &["session", "show", session_id]).unwrap();
        assert_eq!(show.lines().count(), 2);
    }
}
