#![allow(clippy::missing_errors_doc)]
//! CLI-first local product surface for Starweaver.

mod args;
mod config;
mod environment;
mod error;
pub mod launcher;
mod local_store;
mod profiles;
mod runner;
mod service;

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
    let service = CliService::open(config)?;
    service.execute(cli)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::path::Path;

    use super::*;

    fn output(root: &Path, raw_args: &[&str]) -> CliResult<String> {
        let mut command_args = vec!["starweaver-cli".to_string()];
        command_args.extend(raw_args.iter().map(|arg| (*arg).to_string()));
        let cli = args::parse(command_args)?;
        let config = ConfigResolver::for_tests(root).resolve(&cli)?;
        CliService::open(config)?.execute(cli)
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
        assert!(diagnostics.contains("wal=true"));
    }

    #[test]
    fn headless_run_creates_session_and_run() {
        let temp = tempfile::tempdir().unwrap();
        let first = output(temp.path(), &["-p", "hello"]).unwrap();
        let first_message: serde_json::Value =
            serde_json::from_str(first.lines().next().unwrap()).unwrap();
        assert_eq!(first_message["schema"], "starweaver.display.v1");
        assert_eq!(first_message["type"], "RUN_QUEUED");
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
