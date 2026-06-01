//! CLI argument parsing.

use std::ffi::OsString;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::{CliError, CliResult};

/// Top-level CLI.
#[derive(Clone, Debug, Parser)]
#[command(name = "starweaver-cli", version, about = "Starweaver local CLI")]
pub struct Cli {
    /// Prompt shorthand for headless runs.
    #[arg(short = 'p', long = "prompt", global = false)]
    pub prompt: Option<String>,
    /// Append a run to the selected session.
    #[arg(long, global = false, conflicts_with = "new_session")]
    pub session: Option<String>,
    /// Continue the latest local session.
    #[arg(
        long = "continue",
        alias = "continue-session",
        global = false,
        conflicts_with = "new_session"
    )]
    pub continue_session: bool,
    /// Create a fresh session.
    #[arg(long, global = false)]
    pub new_session: bool,
    /// Restore from a specific run before appending a run.
    #[arg(long, global = false)]
    pub run: Option<String>,
    /// Branch from a specific run before appending a run.
    #[arg(long, global = false, conflicts_with = "run")]
    pub branch_from: Option<String>,
    /// Output mode.
    #[arg(long, default_value = "display-jsonl", global = false)]
    pub output: OutputMode,
    /// Override local store database path.
    #[arg(long, global = true)]
    pub store: Option<String>,
    /// Optional subcommand.
    #[command(subcommand)]
    pub command: Option<CliCommand>,
}

/// CLI command families.
#[derive(Clone, Debug, Subcommand)]
pub enum CliCommand {
    /// Print SDK identity.
    Version,
    /// Run a prompt.
    Run(RunCommand),
    /// Manage local sessions.
    Session {
        /// Session subcommand.
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// Print diagnostics.
    Diagnostics,
    /// Print replay-check guidance.
    ReplayCheck,
    /// Get or set configuration values.
    Config {
        /// Config subcommand.
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

/// Prompt run command.
#[derive(Clone, Debug, Args)]
pub struct RunCommand {
    /// Prompt text.
    #[arg(short = 'p', long = "prompt")]
    pub prompt: Option<String>,
    /// Positional prompt text.
    #[arg(trailing_var_arg = true)]
    pub prompt_parts: Vec<String>,
    /// Append a run to the selected session.
    #[arg(long, conflicts_with = "new_session")]
    pub session: Option<String>,
    /// Continue the latest local session.
    #[arg(long, conflicts_with = "new_session")]
    pub continue_session: bool,
    /// Create a fresh session.
    #[arg(long)]
    pub new_session: bool,
    /// Restore from a specific run before appending a run.
    #[arg(long)]
    pub run: Option<String>,
    /// Branch from a specific run before appending a run.
    #[arg(long, conflicts_with = "run")]
    pub branch_from: Option<String>,
    /// Output mode.
    #[arg(long, default_value = "display-jsonl")]
    pub output: OutputMode,
    /// Headless HITL policy placeholder.
    #[arg(long, default_value = "deny")]
    pub hitl: HitlPolicy,
}

impl RunCommand {
    /// Return prompt text.
    pub fn prompt_text(&self) -> CliResult<String> {
        let prompt = self
            .prompt
            .clone()
            .unwrap_or_else(|| self.prompt_parts.join(" "));
        if prompt.trim().is_empty() {
            Err(CliError::Usage(
                "usage: starweaver-cli run -p <prompt>".to_string(),
            ))
        } else {
            Ok(prompt)
        }
    }
}

/// Compact session commands.
#[derive(Clone, Debug, Subcommand)]
pub enum SessionCommand {
    /// List local sessions.
    List(SessionListCommand),
    /// Show one session with recent runs.
    Show(SessionShowCommand),
    /// Replay stored display messages.
    Replay(SessionReplayCommand),
    /// Trim retained run evidence.
    Trim(SessionTrimCommand),
}

/// Session list command.
#[derive(Clone, Debug, Args)]
pub struct SessionListCommand {
    /// Output mode.
    #[arg(long, default_value = "display-jsonl")]
    pub output: OutputMode,
    /// Maximum sessions to show.
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
}

/// Session show command.
#[derive(Clone, Debug, Args)]
pub struct SessionShowCommand {
    /// Session id.
    pub session_id: String,
    /// Output mode.
    #[arg(long, default_value = "display-jsonl")]
    pub output: OutputMode,
    /// Recent run limit.
    #[arg(long, default_value_t = 20)]
    pub runs: usize,
}

/// Session replay command.
#[derive(Clone, Debug, Args)]
pub struct SessionReplayCommand {
    /// Session id.
    pub session_id: String,
    /// Optional run id.
    #[arg(long)]
    pub run: Option<String>,
    /// Cursor sequence to replay after.
    #[arg(long)]
    pub after: Option<usize>,
    /// Output mode.
    #[arg(long, default_value = "display-jsonl")]
    pub output: OutputMode,
}

/// Session trim command.
#[derive(Clone, Debug, Args)]
pub struct SessionTrimCommand {
    /// Trim current session.
    #[arg(long)]
    pub current: bool,
    /// Trim all sessions.
    #[arg(long)]
    pub all: bool,
    /// Trim a selected session.
    #[arg(long)]
    pub session: Option<String>,
    /// Retain this many recent runs per session.
    #[arg(long, default_value_t = 20)]
    pub keep_runs: usize,
    /// Preview trim results.
    #[arg(long)]
    pub dry_run: bool,
    /// Output mode.
    #[arg(long, default_value = "display-jsonl")]
    pub output: OutputMode,
}

/// Config commands.
#[derive(Clone, Debug, Subcommand)]
pub enum ConfigCommand {
    /// Get a resolved config value.
    Get { key: String },
    /// Set a project config value placeholder.
    Set { key: String, value: String },
}

/// Output mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum OutputMode {
    /// Starweaver AGUI-compatible `DisplayMessage` JSON lines.
    #[default]
    DisplayJsonl,
    /// Persist and print compact status.
    Silent,
}

/// HITL policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum HitlPolicy {
    /// Deny approvals.
    #[default]
    Deny,
    /// Defer approvals.
    Defer,
    /// Fail on approvals.
    Fail,
    /// Prompt interactively.
    Prompt,
}

/// Parse CLI arguments.
pub fn parse(args: impl IntoIterator<Item = String>) -> CliResult<Cli> {
    Cli::try_parse_from(args).map_err(|error| CliError::Usage(error.to_string()))
}

/// Parse CLI arguments from OS strings.
#[allow(dead_code)]
pub fn parse_os(args: impl IntoIterator<Item = OsString>) -> CliResult<Cli> {
    Cli::try_parse_from(args).map_err(|error| CliError::Usage(error.to_string()))
}
