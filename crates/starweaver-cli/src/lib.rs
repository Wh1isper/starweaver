#![allow(clippy::missing_errors_doc)]
//! CLI-first local product surface for Starweaver.

mod args;
mod client_state;
mod clipboard;
mod config;
mod display_preview;
mod environment;
mod error;
pub mod launcher;
mod local_store;
mod oauth;
mod profiles;
mod prompt_input;
mod runner;
pub(crate) mod runtime_coordinator;
pub(crate) mod service;
mod slash_commands;
mod tui;
mod update_check;

use std::env;

pub use args::{Cli, CliCommand, OutputMode, SessionCommand};
pub use config::{CliConfig, ConfigResolver};
pub use error::{CliError, CliResult};
pub use local_store::{
    DisplayReplayWindow, LocalSessionStore, LocalStore, LocalStreamArchive, TrimReport,
};
pub use service::CliService;
pub use slash_commands::SlashCommandDefinition;

/// Run the CLI from process arguments.
pub fn run_from_env() -> CliResult<()> {
    run(env::args())
}

/// Run the CLI from an argument iterator.
pub fn run(args: impl IntoIterator<Item = String>) -> CliResult<()> {
    let cli = match args::parse(args) {
        Ok(cli) => cli,
        Err(CliError::Display(output)) => {
            print!("{output}");
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let config = ConfigResolver::default().resolve(&cli)?;
    let output = command_output_from_parts(cli, config)?;
    print!("{output}");
    Ok(())
}

/// Return command output for tests and host integrations.
pub fn command_output(args: impl IntoIterator<Item = String>) -> CliResult<String> {
    let cli = match args::parse(args) {
        Ok(cli) => cli,
        Err(CliError::Display(output)) => return Ok(output),
        Err(error) => return Err(error),
    };
    let config = ConfigResolver::default().resolve(&cli)?;
    command_output_from_parts(cli, config)
}

fn command_output_from_parts(cli: Cli, config: CliConfig) -> CliResult<String> {
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

    fn write_trim_test_config(root: &Path, auto_after_run: bool) {
        let global = root.join("global");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(
            global.join("config.toml"),
            format!(
                r#"
[general]
model = "local_echo"

[trim]
auto_after_run = {auto_after_run}
current_session_keep_recent_runs = 1
all_sessions_keep_recent_runs = 1
all_sessions_keep_days = 1
all_sessions_interval_hours = 1
"#
            ),
        )
        .unwrap();
    }

    fn first_session_from_list_json(output: &str) -> serde_json::Value {
        serde_json::from_str::<serde_json::Value>(output).unwrap()["sessions"][0].clone()
    }

    fn session_run_count(root: &Path, session_id: &str) -> usize {
        let show = output(root, &["session", "show", session_id, "--output", "json"]).unwrap();
        serde_json::from_str::<serde_json::Value>(&show).unwrap()["runs"]
            .as_array()
            .unwrap()
            .len()
    }

    fn stored_run_dir_count(config: &CliConfig, session_id: &str) -> usize {
        config
            .file_store_path
            .join("sessions")
            .join(session_id)
            .join("runs")
            .read_dir()
            .unwrap()
            .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_dir())
            .count()
    }

    fn create_current_retention_session(root: &Path) -> String {
        write_trim_test_config(root, true);
        output(root, &["-p", "one", "--output", "silent"]).unwrap();
        output(root, &["-p", "two", "--continue", "--output", "silent"]).unwrap();
        output(root, &["-p", "three", "--continue", "--output", "silent"]).unwrap();
        let session = first_session_from_list_json(
            &output(root, &["session", "list", "--output", "json"]).unwrap(),
        )["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(session_run_count(root, &session), 1);
        session
    }

    fn create_archive_retention_session(root: &Path) -> String {
        write_trim_test_config(root, false);
        output(
            root,
            &["-p", "archive-one", "--new-session", "--output", "silent"],
        )
        .unwrap();
        let session = first_session_from_list_json(
            &output(root, &["session", "list", "--output", "json"]).unwrap(),
        )["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        output(
            root,
            &[
                "-p",
                "archive-two",
                "--session",
                &session,
                "--output",
                "silent",
            ],
        )
        .unwrap();
        output(
            root,
            &[
                "-p",
                "archive-three",
                "--session",
                &session,
                "--output",
                "silent",
            ],
        )
        .unwrap();
        session
    }

    fn mark_session_as_retention_eligible(config: &CliConfig, session_id: &str) {
        let old = chrono::DateTime::parse_from_rfc3339("2000-01-01T00:00:00+00:00")
            .unwrap()
            .to_utc();
        let storage = starweaver_storage::SqliteStorage::open(&config.database_path).unwrap();
        let session_id = starweaver_core::SessionId::from_string(session_id);
        let runs = storage.list_runs(&session_id).unwrap();
        let run_ids = runs
            .iter()
            .map(|run| run.run_id.clone())
            .collect::<Vec<_>>();
        storage.prune_runs(&session_id, &run_ids).unwrap();
        for mut run in runs {
            run.metadata
                .remove(starweaver_storage::RunEvidenceCommit::DIGEST_METADATA_KEY);
            run.created_at = old;
            run.updated_at = old;
            storage.begin_run(run.clone()).unwrap();
            storage
                .commit_run_evidence(starweaver_storage::RunEvidenceCommit::new(
                    run,
                    starweaver_context::ResumableState::default(),
                ))
                .unwrap();
        }

        let state_path = config.project_dir.join("state.json");
        let mut state = serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(&state_path).unwrap(),
        )
        .unwrap();
        state["last_retention_maintenance_at"] = serde_json::json!(old);
        std::fs::write(&state_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
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
            goal: None,
            worker: None,
            worker_label: None,
            worktree: None,
            worktree_name: None,
            branch: None,
            session_affinity_id: None,
            environment_attachments: Vec::new(),
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
            goal: None,
            worker: None,
            worker_label: None,
            worktree: None,
            worktree_name: None,
            branch: None,
            session_affinity_id: None,
            environment_attachments: Vec::new(),
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
            goal: None,
            worker: None,
            worker_label: None,
            worktree: None,
            worktree_name: None,
            branch: None,
            session_affinity_id: None,
            environment_attachments: Vec::new(),
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
            OsString::from("--profile"),
            OsString::from("coding"),
            OsString::from("--worker"),
            OsString::from("off"),
            OsString::from("--worktree"),
            OsString::from("feature"),
            OsString::from("--branch"),
            OsString::from("feature/work"),
        ])
        .unwrap();
        assert_eq!(parsed.session.as_deref(), Some("session_test"));
        assert_eq!(parsed.profile.as_deref(), Some("coding"));
        assert_eq!(parsed.worker.as_deref(), Some("off"));
        assert_eq!(parsed.worktree.as_deref(), Some("feature"));
        assert_eq!(parsed.branch.as_deref(), Some("feature/work"));

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

        assert!(
            format!(
                "{}",
                CliError::from(serde_json::from_str::<serde_json::Value>("{").unwrap_err())
            )
            .contains("serialization error")
        );
        assert!(
            format!(
                "{}",
                CliError::from(toml::from_str::<toml::Value>("=").unwrap_err())
            )
            .contains("configuration error")
        );
        assert!(
            format!(
                "{}",
                CliError::from(toml::to_string(&f64::NAN).unwrap_err())
            )
            .contains("configuration error")
        );
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

[oauth_refresh]
enabled = true
interval_seconds = 42
failure_retry_seconds = 7
refresh_on_startup = false

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
        assert_eq!(
            output(
                temp.path(),
                &["config", "get", "oauth_refresh.interval_seconds"]
            )
            .unwrap(),
            "42\n"
        );
        assert_eq!(
            output(
                temp.path(),
                &["config", "get", "oauth_refresh.refresh_on_startup"]
            )
            .unwrap(),
            "false\n"
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
    fn configured_slash_commands_layer_aliases_and_redact_unmapped_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let global = temp.path().join("global");
        let project = temp.path().join("project/.starweaver");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            global.join("config.toml"),
            r#"
[commands.review]
description = "Global review"
aliases = ["rv", "bad alias", "model"]
prompt = "global secret prompt"

[commands.other]
aliases = ["review"]
prompt = "Other command"
"#,
        )
        .unwrap();
        std::fs::write(
            project.join("config.toml"),
            r#"
[commands.review]
description = "Project review"
aliases = ["pr"]
prompt = "Project review prompt"

[commands.bad_name]
prompt = "ignored because underscore is valid"

[commands."bad name"]
prompt = "ignored invalid name"
"#,
        )
        .unwrap();

        let cli = args::parse(["starweaver-cli".to_string(), "diagnostics".to_string()]).unwrap();
        let config = ConfigResolver::for_tests(temp.path())
            .resolve(&cli)
            .unwrap();
        let review = config.slash_commands.get("review").unwrap();
        assert_eq!(review.prompt, "Project review prompt");
        assert_eq!(review.aliases, vec!["pr".to_string()]);
        assert!(config.slash_commands.contains_key("pr"));
        assert!(!config.slash_commands.contains_key("rv"));
        assert!(!config.slash_commands.contains_key("bad alias"));
        assert!(!config.slash_commands.contains_key("model"));
        assert!(config.slash_commands.contains_key("bad_name"));
        assert!(!config.slash_commands.contains_key("bad name"));
        let unmapped = output(temp.path(), &["config", "get", "metadata.unmapped"]).unwrap();
        assert!(!unmapped.contains("global secret prompt"));
        assert!(!unmapped.contains("Project review prompt"));
        assert!(!unmapped.contains("commands"));
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

        let worker_agent = profile
            .build_agent_with_delegation(starweaver_agent::SubagentDelegationMode::Disabled, None)
            .unwrap();
        assert!(
            !worker_agent
                .tools()
                .names()
                .contains(&"delegate".to_string())
        );
        assert!(
            !worker_agent
                .tools()
                .names()
                .contains(&"subagent_info".to_string())
        );

        let supervisor = std::sync::Arc::new(starweaver_agent::BackgroundSubagentSupervisor::new());
        let interactive_agent = profile
            .build_agent_with_delegation(
                starweaver_agent::SubagentDelegationMode::Async,
                Some(supervisor),
            )
            .unwrap();
        let interactive_tools = interactive_agent.tools().names();
        assert!(interactive_tools.contains(&"delegate".to_string()));
        assert!(interactive_tools.contains(&"steer_subagent".to_string()));
        assert!(interactive_tools.contains(&"cancel_subagent".to_string()));
        assert!(interactive_tools.contains(&"wait_subagent".to_string()));
        assert!(interactive_tools.contains(&"__delegate_backend".to_string()));

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
    fn headless_run_expands_configured_slash_commands() {
        let temp = tempfile::tempdir().unwrap();
        let global = temp.path().join("global");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(
            global.join("config.toml"),
            r#"
[general]
model = "local_echo"

[commands.review]
description = "Review the current changes"
aliases = ["rv"]
prompt = "Review carefully."
"#,
        )
        .unwrap();

        let run = output(
            temp.path(),
            &[
                "-p",
                "/rv staged diff",
                "--profile",
                "default_model",
                "--output",
                "text",
            ],
        )
        .unwrap();
        assert!(run.contains("local echo: Review carefully."));
        assert!(run.contains("User instruction: staged diff"));

        let sessions = output(temp.path(), &["session", "list"]).unwrap();
        let session: serde_json::Value =
            serde_json::from_str(sessions.lines().next().unwrap()).unwrap();
        let session_id = session["session_id"].as_str().unwrap();
        let cli = args::parse([
            "starweaver-cli".to_string(),
            "session".to_string(),
            "list".to_string(),
        ])
        .unwrap();
        let config = ConfigResolver::for_tests(temp.path())
            .resolve(&cli)
            .unwrap();
        let store = LocalStore::open(&config).unwrap();
        let run_id = session["head_run_id"].as_str().unwrap();
        let run_record = store.load_run(session_id, run_id).unwrap();
        let run_value = serde_json::to_value(&run_record).unwrap();
        assert_eq!(
            run_value["input"][0]["text"],
            "Review carefully.\n\nUser instruction: staged diff"
        );
        assert_eq!(run_value["metadata"]["cli.slash_command.name"], "review");
        assert_eq!(run_value["metadata"]["cli.slash_command.invoked"], "rv");
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
        assert!(
            agui_events
                .iter()
                .any(|event| event["type"] == "RUN_STARTED")
        );
        assert!(
            agui_events
                .iter()
                .any(|event| event["type"] == "TEXT_MESSAGE_CHUNK")
        );
        assert!(
            agui_events
                .iter()
                .any(|event| event["type"] == "RUN_FINISHED")
        );
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
    fn display_replay_window_uses_scoped_cursors() {
        let temp = tempfile::tempdir().unwrap();
        output(temp.path(), &["-p", "one"]).unwrap();
        output(temp.path(), &["-p", "two", "--continue"]).unwrap();
        let sessions = output(temp.path(), &["session", "list"]).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(sessions.lines().next().unwrap()).unwrap();
        let session_id = value["session_id"].as_str().unwrap();
        let cli = args::parse([
            "starweaver-cli".to_string(),
            "session".to_string(),
            "list".to_string(),
        ])
        .unwrap();
        let config = ConfigResolver::for_tests(temp.path())
            .resolve(&cli)
            .unwrap();
        let store = LocalStore::open(&config).unwrap();

        let session_window = store.replay_display_window(session_id, None, None).unwrap();
        assert_eq!(
            session_window.scope,
            starweaver_stream::ReplayScope::session(session_id)
        );
        assert_eq!(session_window.next_sequence, session_window.events.len());
        for (sequence, event) in session_window.events.iter().enumerate() {
            assert_eq!(
                event.scope,
                starweaver_stream::ReplayScope::session(session_id)
            );
            assert_eq!(event.sequence, sequence);
        }

        let session_cursor =
            starweaver_stream::ReplayCursor::display(session_window.scope.clone(), 0);
        let session_tail = store
            .replay_display_window(session_id, None, Some(&session_cursor))
            .unwrap();
        assert!(session_tail.events.iter().all(|event| event.sequence > 0));
        assert_eq!(session_tail.next_sequence, session_window.next_sequence);

        let first_run = store.list_runs(session_id, 10).unwrap().remove(0);
        let run_window = store
            .replay_display_window(session_id, Some(&first_run.run_id), None)
            .unwrap();
        assert_eq!(
            run_window.scope,
            starweaver_stream::ReplayScope::run(&first_run.run_id)
        );
        assert!(run_window.next_sequence > 0);
        assert!(
            run_window
                .events
                .iter()
                .all(|event| event.scope == starweaver_stream::ReplayScope::run(&first_run.run_id))
        );
    }

    #[test]
    fn local_stream_archive_implements_shared_stream_contract() {
        use starweaver_stream::StreamArchive as _;

        let temp = tempfile::tempdir().unwrap();
        output(temp.path(), &["-p", "archive"]).unwrap();
        let sessions = output(temp.path(), &["session", "list"]).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(sessions.lines().next().unwrap()).unwrap();
        let session_id = value["session_id"].as_str().unwrap();
        let cli = args::parse([
            "starweaver-cli".to_string(),
            "session".to_string(),
            "list".to_string(),
        ])
        .unwrap();
        let config = ConfigResolver::for_tests(temp.path())
            .resolve(&cli)
            .unwrap();
        let store = LocalStore::open(&config).unwrap();
        let run = store.list_runs(session_id, 10).unwrap().remove(0);
        let archive = LocalStreamArchive::new(config).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let run_scope = starweaver_stream::ReplayScope::run(&run.run_id);
        let session_scope = starweaver_stream::ReplayScope::session(session_id);

        let run_messages = runtime
            .block_on(archive.replay_display_after(&run_scope, None))
            .unwrap();
        assert!(!run_messages.is_empty());
        let run_range = runtime.block_on(archive.cursor_range(&run_scope)).unwrap();
        assert!(run_range.is_some());

        let session_messages = runtime
            .block_on(archive.replay_display_after(
                &session_scope,
                Some(starweaver_stream::ReplayCursor::display(
                    session_scope.clone(),
                    0,
                )),
            ))
            .unwrap();
        assert!(session_messages.len() < run_messages.len());
        let session_range = runtime
            .block_on(archive.cursor_range(&session_scope))
            .unwrap()
            .unwrap();
        assert_eq!(session_range.0.sequence, 0);

        let extra = starweaver_stream::DisplayMessage::new(
            999,
            starweaver_core::SessionId::from_string(session_id),
            starweaver_core::RunId::from_string(&run.run_id),
            starweaver_stream::DisplayMessageKind::HostEvent,
        )
        .with_preview("extra archive message");
        runtime
            .block_on(archive.append_display_messages(run_scope.clone(), vec![extra]))
            .unwrap();
        let appended = runtime
            .block_on(archive.replay_display_after(
                &run_scope,
                Some(starweaver_stream::ReplayCursor::display(
                    run_scope.clone(),
                    998,
                )),
            ))
            .unwrap();
        assert_eq!(appended.len(), 1);
        assert_eq!(
            appended[0].preview.as_deref(),
            Some("extra archive message")
        );

        let snapshot = starweaver_stream::ReplaySnapshot {
            scope: Some(run_scope.clone()),
            revision: 7,
            cursor: Some(starweaver_stream::ReplayCursor::display(
                run_scope.clone(),
                999,
            )),
            display_messages: appended,
            metadata: serde_json::Map::default(),
        };
        runtime
            .block_on(archive.append_snapshot(run_scope.clone(), snapshot.clone()))
            .unwrap();
        let latest = runtime
            .block_on(archive.latest_snapshot(&run_scope))
            .unwrap()
            .unwrap();
        assert_eq!(latest.revision, snapshot.revision);

        let _raw_records = runtime
            .block_on(archive.replay_raw_after(
                &starweaver_core::SessionId::from_string(session_id),
                &starweaver_core::RunId::from_string(&run.run_id),
                None,
            ))
            .unwrap();
    }

    #[test]
    fn local_session_and_stream_adapters_back_agent_runtime_builder() {
        use std::sync::Arc;

        use starweaver_session::SessionStore as _;
        use starweaver_stream::StreamArchive as _;

        let temp = tempfile::tempdir().unwrap();
        let cli = args::parse([
            "starweaver-cli".to_string(),
            "session".to_string(),
            "list".to_string(),
        ])
        .unwrap();
        let config = ConfigResolver::for_tests(temp.path())
            .resolve(&cli)
            .unwrap();
        let session_id = starweaver_core::SessionId::from_string("session_local_runtime");
        let session_store = Arc::new(LocalSessionStore::new(config.clone()).unwrap());
        let stream_archive = Arc::new(LocalStreamArchive::new(config).unwrap());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut agent_runtime = starweaver_agent::AgentRuntimeBuilder::new(Arc::new(
            starweaver_agent::TestModel::with_text("ok"),
        ))
        .durable_session_id(session_id.clone())
        .session_store(session_store.clone())
        .stream_archive(stream_archive.clone())
        .build();

        let result = runtime.block_on(agent_runtime.run_stream("hello")).unwrap();
        assert_eq!(result.result.output, "ok");

        let runs = runtime
            .block_on(session_store.list_runs(&session_id))
            .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, starweaver_session::RunStatus::Completed);
        let run_scope = starweaver_stream::ReplayScope::run(runs[0].run_id.as_str());
        let display_messages = runtime
            .block_on(stream_archive.replay_display_after(&run_scope, None))
            .unwrap();
        assert!(!display_messages.is_empty());
        let trace = runtime
            .block_on(session_store.compact_session_trace(&session_id))
            .unwrap();
        assert_eq!(trace.runs, 1);
    }

    #[test]
    fn automatic_retention_prunes_old_runs_without_deleting_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let current_session = create_current_retention_session(temp.path());
        let archive_session = create_archive_retention_session(temp.path());
        let config = ConfigResolver::for_tests(temp.path())
            .resolve(&args::parse(["starweaver-cli".to_string()]).unwrap())
            .unwrap();
        mark_session_as_retention_eligible(&config, &archive_session);
        assert_eq!(session_run_count(temp.path(), &archive_session), 3);
        assert_eq!(stored_run_dir_count(&config, &archive_session), 3);

        write_trim_test_config(temp.path(), true);
        output(
            temp.path(),
            &[
                "-p",
                "trigger",
                "--session",
                &current_session,
                "--output",
                "silent",
            ],
        )
        .unwrap();

        assert_eq!(session_run_count(temp.path(), &current_session), 1);
        assert_eq!(session_run_count(temp.path(), &archive_session), 1);
        assert_eq!(stored_run_dir_count(&config, &archive_session), 1);
        let sessions = output(temp.path(), &["session", "list", "--output", "json"]).unwrap();
        let sessions = serde_json::from_str::<serde_json::Value>(&sessions).unwrap()["sessions"]
            .as_array()
            .unwrap()
            .len();
        assert_eq!(sessions, 2);
        assert!(
            std::fs::read_to_string(config.project_dir.join("state.json"))
                .unwrap()
                .contains("last_retention_maintenance_at")
        );
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
