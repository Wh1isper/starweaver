//! CLI argument parsing.

use std::ffi::OsString;

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use serde::{Deserialize, Serialize};

use crate::{CliError, CliResult};

/// Top-level CLI.
#[derive(Clone, Debug, Parser)]
#[command(name = "starweaver-cli", version, about = "Starweaver local CLI")]
pub struct Cli {
    /// Prompt shorthand for headless runs.
    #[arg(short = 'p', long = "prompt", global = false)]
    pub prompt: Option<String>,
    /// Append a run to the selected session.
    #[arg(
        short = 's',
        long,
        global = false,
        conflicts_with_all = ["new_session", "continue_session"]
    )]
    pub session: Option<String>,
    /// Continue the latest local session.
    #[arg(long = "continue", global = false, conflicts_with = "new_session")]
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
    /// Agent profile name or YAML path.
    #[arg(long, global = false)]
    pub profile: Option<String>,
    /// Enable worker mode or set an optional worker label.
    #[arg(long, global = false, num_args = 0..=1, default_missing_value = "true")]
    pub worker: Option<String>,
    /// Explicit worker label.
    #[arg(long = "worker-label", global = false)]
    pub worker_label: Option<String>,
    /// Enable a git worktree or set an optional worktree name/path.
    #[arg(
        short = 'w',
        long,
        global = false,
        num_args = 0..=1,
        default_missing_value = "true"
    )]
    pub worktree: Option<String>,
    /// Explicit worktree name/path.
    #[arg(long = "worktree-name", global = false)]
    pub worktree_name: Option<String>,
    /// Git branch for worktree metadata.
    #[arg(long, global = false)]
    pub branch: Option<String>,
    /// Output mode.
    #[arg(long, global = false)]
    pub output: Option<OutputMode>,
    /// Headless human-in-the-loop policy for prompt shorthand.
    #[arg(long, global = false)]
    pub hitl: Option<HitlPolicy>,
    /// Override local store database path.
    #[arg(long, global = true)]
    pub store: Option<String>,
    /// Optional subcommand.
    #[command(subcommand)]
    pub command: Option<CliCommand>,
}

/// CLI command families.
#[allow(clippy::large_enum_variant)]
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
    /// Manage agent profiles.
    Profile {
        /// Profile subcommand.
        #[command(subcommand)]
        command: ProfileCommand,
    },
    /// Initialize local CLI configuration and catalogs.
    Setup(SetupCommand),
    /// Inspect OAuth-backed provider authentication.
    Auth {
        /// Auth subcommand.
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Inspect configured skills.
    Skill {
        /// Skill subcommand.
        #[command(subcommand)]
        command: CatalogCommand,
    },
    /// Inspect configured subagents.
    Subagent {
        /// Subagent subcommand.
        #[command(subcommand)]
        command: CatalogCommand,
    },
    /// Inspect configured MCP servers.
    Mcp {
        /// MCP subcommand.
        #[command(subcommand)]
        command: CatalogCommand,
    },
    /// Inspect default CLI tool catalog and policy.
    Tools {
        /// Tools subcommand.
        #[command(subcommand)]
        command: ToolsCommand,
    },
    /// Render a retained terminal UI from local session display messages.
    Tui(TuiCommand),
    /// Manage persisted approval requests.
    Approval {
        /// Approval subcommand.
        #[command(subcommand)]
        command: ApprovalCommand,
    },
    /// Manage deferred tool calls.
    Deferred {
        /// Deferred subcommand.
        #[command(subcommand)]
        command: DeferredCommand,
    },
    /// Resume a waiting session by appending a continuation run.
    Resume(ResumeCommand),
    /// Remove runtime session state while preserving configuration.
    Reset(ResetCommand),
    /// Print diagnostics.
    Diagnostics,
    /// Print replay-check guidance.
    ReplayCheck,
    /// Update installed Starweaver components.
    Update(UpdateCommand),
    /// Get or set configuration values.
    Config {
        /// Config subcommand.
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Generate shell completion scripts.
    Completion {
        /// Target shell.
        shell: Shell,
    },
}

/// Prompt run command.
#[derive(Clone, Debug, Args)]
pub struct RunCommand {
    /// Prompt text.
    #[arg(short = 'p', long = "prompt")]
    pub prompt: Option<String>,
    /// Positional prompt text.
    pub prompt_parts: Vec<String>,
    /// Append a run to the selected session.
    #[arg(short = 's', conflicts_with_all = ["new_session", "continue_session"], long)]
    pub session: Option<String>,
    /// Continue the latest local session.
    #[arg(long = "continue", conflicts_with = "new_session")]
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
    /// Agent profile name or YAML path.
    #[arg(long)]
    pub profile: Option<String>,
    /// Enable worker mode or set an optional worker label.
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    pub worker: Option<String>,
    /// Explicit worker label.
    #[arg(long = "worker-label")]
    pub worker_label: Option<String>,
    /// Enable a git worktree or set an optional worktree name/path.
    #[arg(short = 'w', long, num_args = 0..=1, default_missing_value = "true")]
    pub worktree: Option<String>,
    /// Explicit worktree name/path.
    #[arg(long = "worktree-name")]
    pub worktree_name: Option<String>,
    /// Git branch for worktree metadata.
    #[arg(long)]
    pub branch: Option<String>,
    /// Output mode.
    #[arg(long)]
    pub output: Option<OutputMode>,
    /// Headless human-in-the-loop policy.
    #[arg(long)]
    pub hitl: Option<HitlPolicy>,
    /// Internal runtime goal-mode options.
    #[arg(skip)]
    pub goal: Option<GoalCommandOptions>,
    /// Internal stable provider-routing affinity id.
    #[arg(skip)]
    pub session_affinity_id: Option<String>,
    /// Internal host RPC environment attachments.
    #[arg(skip)]
    pub environment_attachments: Vec<starweaver_rpc_core::EnvironmentAttachmentRef>,
}

/// Internal goal-mode options attached by product surfaces such as the TUI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalCommandOptions {
    /// Goal objective.
    pub objective: String,
    /// Maximum runtime goal retry iterations.
    pub max_iterations: usize,
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
    /// Search local sessions by metadata and approved text projections.
    Search(SessionSearchCommand),
    /// Show one session with recent runs.
    Show(SessionShowCommand),
    /// Replay stored display messages.
    Replay(SessionReplayCommand),
    /// Delete one local session and its retained evidence.
    Delete(SessionDeleteCommand),
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

/// Session search command.
#[derive(Clone, Debug, Args)]
pub struct SessionSearchCommand {
    /// Optional case-insensitive literal text. Omit it to browse metadata.
    pub text: Option<String>,
    /// Exact session status.
    #[arg(long)]
    pub status: Option<SessionSearchStatusArg>,
    /// Exact profile name.
    #[arg(long)]
    pub profile: Option<String>,
    /// Exact workspace display value.
    #[arg(long)]
    pub workspace: Option<String>,
    /// Search source; repeat to select multiple sources.
    #[arg(long = "source", value_enum)]
    pub sources: Vec<SessionSearchSourceArg>,
    /// Result grouping level.
    #[arg(long, value_enum, default_value = "session")]
    pub granularity: SessionSearchGranularityArg,
    /// Maximum hits to return.
    #[arg(long, default_value_t = 20)]
    pub limit: u32,
    /// Opaque next-page cursor.
    #[arg(long = "after")]
    pub cursor: Option<String>,
    /// Output mode.
    #[arg(long, default_value = "text")]
    pub output: OutputMode,
}

/// Session status accepted by search.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum SessionSearchStatusArg {
    /// Active session.
    Active,
    /// Archived session.
    Archived,
    /// Failed session.
    Failed,
}

/// Search source accepted by the CLI.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum SessionSearchSourceArg {
    /// Session title and approved metadata.
    SessionMetadata,
    /// Canonical text input.
    RunInput,
    /// Bounded run output preview.
    RunOutputPreview,
    /// User-visible display messages.
    #[value(alias = "display")]
    DisplayMessage,
}

/// Search grouping accepted by the CLI.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum SessionSearchGranularityArg {
    /// One hit per session.
    Session,
    /// One hit per session/run pair.
    Run,
    /// Every stable projected occurrence.
    Occurrence,
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

/// Session delete command.
#[derive(Clone, Debug, Args)]
pub struct SessionDeleteCommand {
    /// Session id or unique prefix.
    pub session_id: String,
    /// Confirm deletion.
    #[arg(long)]
    pub yes: bool,
    /// Output mode.
    #[arg(long, default_value = "text")]
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
    /// Trim runs older than a duration such as 7d, 24h, or 3600s.
    #[arg(long)]
    pub older_than: Option<String>,
    /// Preview trim results.
    #[arg(long)]
    pub dry_run: bool,
    /// Output mode.
    #[arg(long, default_value = "display-jsonl")]
    pub output: OutputMode,
}

/// Profile commands.
#[derive(Clone, Debug, Subcommand)]
pub enum ProfileCommand {
    /// List built-in and configured profiles.
    List,
    /// Show one built-in or configured profile.
    Show { name: String },
}

/// Setup command.
#[derive(Clone, Debug, Args)]
pub struct SetupCommand {
    /// Initialize global configuration only.
    #[arg(long, conflicts_with = "project")]
    pub global: bool,
    /// Initialize project configuration only.
    #[arg(long)]
    pub project: bool,
    /// Replace existing generated files.
    #[arg(long)]
    pub force: bool,
}

/// Auth commands.
#[derive(Clone, Debug, Subcommand)]
pub enum AuthCommand {
    /// Log in to an OAuth provider.
    Login(AuthProviderCommand),
    /// Print provider auth status.
    Status(AuthStatusCommand),
    /// Refresh provider credentials.
    Refresh(AuthProviderCommand),
    /// Remove provider credentials from the local auth store.
    Logout(AuthLogoutCommand),
    /// Inspect OAuth store health without printing tokens.
    Doctor(AuthDoctorCommand),
}

/// Provider-scoped auth command.
#[derive(Clone, Debug, Args)]
pub struct AuthProviderCommand {
    /// Provider name.
    #[arg(default_value = "codex", value_parser = ["codex"])]
    pub provider: String,
    /// Auth file path. Defaults to ~/.starweaver/auth.json.
    #[arg(long = "auth-file")]
    pub auth_file: Option<String>,
    /// Device authorization timeout in seconds.
    #[arg(long, default_value_t = 15 * 60)]
    pub timeout_seconds: u64,
}

/// Auth status command.
#[derive(Clone, Debug, Args)]
pub struct AuthStatusCommand {
    /// Provider name.
    #[arg(default_value = "codex", value_parser = ["codex"])]
    pub provider: Option<String>,
    /// Auth file path. Defaults to ~/.starweaver/auth.json.
    #[arg(long = "auth-file")]
    pub auth_file: Option<String>,
}

/// Auth logout command.
#[derive(Clone, Debug, Args)]
pub struct AuthLogoutCommand {
    /// Provider name.
    #[arg(default_value = "codex", value_parser = ["codex"])]
    pub provider: String,
    /// Auth file path. Defaults to ~/.starweaver/auth.json.
    #[arg(long = "auth-file")]
    pub auth_file: Option<String>,
    /// Revoke provider tokens before deleting local credentials.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub revoke: bool,
}

/// Auth doctor command.
#[derive(Clone, Debug, Args)]
pub struct AuthDoctorCommand {
    /// Auth file path. Defaults to ~/.starweaver/auth.json.
    #[arg(long = "auth-file")]
    pub auth_file: Option<String>,
}

/// Catalog inspection commands.
#[derive(Clone, Debug, Subcommand)]
pub enum CatalogCommand {
    /// List configured entries.
    List,
    /// Show one configured entry.
    Show { name: String },
    /// Validate configured entries and print findings.
    Doctor,
}

/// Tool catalog commands.
#[derive(Clone, Debug, Subcommand)]
pub enum ToolsCommand {
    /// List default first-party tools.
    List,
    /// Validate tool policy and default catalog.
    Doctor,
}

/// TUI command.
#[derive(Clone, Debug, Args)]
pub struct TuiCommand {
    /// Session id to render. Omit for a clean welcome screen.
    #[arg(long)]
    pub session: Option<String>,
    /// Optional run id to render.
    #[arg(long)]
    pub run: Option<String>,
    /// Render only messages after this display cursor.
    #[arg(long)]
    pub after: Option<usize>,
    /// Force interactive terminal UI when stdout is a TTY.
    #[arg(long)]
    pub interactive: bool,
    /// Force deterministic snapshot output for scripts and tests.
    #[arg(long, conflicts_with = "interactive")]
    pub snapshot: bool,
    /// Output mode for non-interactive TUI snapshots.
    #[arg(long, default_value = "text")]
    pub output: OutputMode,
    /// Transcript rendering mode for interactive TUI.
    #[arg(long = "render-mode")]
    pub render_mode: Option<TuiRenderMode>,
}

/// Approval commands.
#[derive(Clone, Debug, Subcommand)]
pub enum ApprovalCommand {
    /// List persisted approval records.
    List(ApprovalListCommand),
    /// Show one approval record.
    Show { approval_id: String },
    /// Approve one pending approval record.
    Approve(ApprovalDecisionCommand),
    /// Reject one pending approval record.
    Reject(ApprovalDecisionCommand),
}

/// Approval list command.
#[derive(Clone, Debug, Args)]
pub struct ApprovalListCommand {
    /// Filter by session id.
    #[arg(long)]
    pub session: Option<String>,
    /// Filter by run id.
    #[arg(long)]
    pub run: Option<String>,
    /// Output mode.
    #[arg(long, default_value = "display-jsonl")]
    pub output: OutputMode,
}

/// Approval decision command.
#[derive(Clone, Debug, Args)]
pub struct ApprovalDecisionCommand {
    /// Approval id.
    pub approval_id: String,
    /// Decision reason.
    #[arg(long)]
    pub reason: Option<String>,
    /// Output mode.
    #[arg(long, default_value = "text")]
    pub output: OutputMode,
}

/// Deferred tool commands.
#[derive(Clone, Debug, Subcommand)]
pub enum DeferredCommand {
    /// List persisted deferred tool records.
    List(DeferredListCommand),
    /// Show one deferred tool record.
    Show { deferred_id: String },
    /// Complete one deferred tool record with a JSON result payload.
    Complete(DeferredCompleteCommand),
    /// Fail one deferred tool record with an error message.
    Fail(DeferredFailCommand),
}

/// Deferred list command.
#[derive(Clone, Debug, Args)]
pub struct DeferredListCommand {
    /// Filter by session id.
    #[arg(long)]
    pub session: Option<String>,
    /// Filter by run id.
    #[arg(long)]
    pub run: Option<String>,
    /// Output mode.
    #[arg(long, default_value = "display-jsonl")]
    pub output: OutputMode,
}

/// Deferred complete command.
#[derive(Clone, Debug, Args)]
pub struct DeferredCompleteCommand {
    /// Deferred id.
    pub deferred_id: String,
    /// JSON result payload.
    #[arg(long)]
    pub result: String,
    /// Output mode.
    #[arg(long, default_value = "text")]
    pub output: OutputMode,
}

/// Deferred failure command.
#[derive(Clone, Debug, Args)]
pub struct DeferredFailCommand {
    /// Deferred id.
    pub deferred_id: String,
    /// Error message.
    #[arg(long)]
    pub error: String,
    /// Output mode.
    #[arg(long, default_value = "text")]
    pub output: OutputMode,
}

/// Resume command.
#[derive(Clone, Debug, Args)]
pub struct ResumeCommand {
    /// Session id to resume. Defaults to current or latest session.
    #[arg(long)]
    pub session: Option<String>,
    /// Run id to resume from. Defaults to the session active or head run.
    #[arg(long)]
    pub run: Option<String>,
    /// Prompt to append for the continuation run.
    #[arg(short = 'p', long = "prompt", default_value = "resume waiting run")]
    pub prompt: String,
    /// Output mode.
    #[arg(long)]
    pub output: Option<OutputMode>,
    /// Headless human-in-the-loop policy.
    #[arg(long)]
    pub hitl: Option<HitlPolicy>,
}

/// Reset command.
#[derive(Clone, Debug, Args)]
pub struct ResetCommand {
    /// Confirm runtime state removal.
    #[arg(long)]
    pub yes: bool,
    /// Output mode.
    #[arg(long, default_value = "text")]
    pub output: OutputMode,
}

/// Update command.
#[derive(Clone, Debug, Args)]
pub struct UpdateCommand {
    /// Update target, defaults to cli.
    #[arg(default_value = "cli")]
    pub target: String,
    /// Print the update plan without downloading or installing.
    #[arg(long)]
    pub dry_run: bool,
    /// Reinstall even when the selected release matches the current version.
    #[arg(long, short = 'f')]
    pub force: bool,
}

/// Config commands.
#[derive(Clone, Debug, Subcommand)]
pub enum ConfigCommand {
    /// Initialize a Starweaver config file.
    Init {
        /// Write the global config file.
        #[arg(long, conflicts_with = "project")]
        global: bool,
        /// Write the project config file.
        #[arg(long)]
        project: bool,
        /// Replace an existing config file.
        #[arg(long)]
        force: bool,
    },
    /// Get a resolved config value.
    Get { key: String },
    /// Set a config value.
    Set {
        /// Write the global config file.
        #[arg(long, conflicts_with = "project")]
        global: bool,
        /// Write the project config file.
        #[arg(long)]
        project: bool,
        /// Config key.
        key: String,
        /// Config value.
        value: String,
    },
}

/// TUI transcript render mode.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum TuiRenderMode {
    /// Show assistant text, reasoning, tool calls, and tool returns.
    #[default]
    Normal,
    /// Hide ordinary tool calls from transcript while keeping high-level events.
    Concise,
    /// Show detailed diagnostic rendering.
    Debug,
}

/// Output mode.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum OutputMode {
    /// Human-readable text.
    Text,
    /// Starweaver durable `DisplayMessage` JSON lines.
    #[default]
    DisplayJsonl,
    /// Starweaver/AGUI top-level event JSON lines.
    AguiJsonl,
    /// Compact JSON command result.
    Json,
    /// Persist and print compact status.
    Silent,
}

/// HITL policy.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum HitlPolicy {
    /// Deny approvals.
    Deny,
    /// Defer approvals.
    Defer,
    /// Fail on approvals.
    Fail,
    /// Prompt interactively.
    #[default]
    Prompt,
}

/// Build the clap command schema.
#[must_use]
pub fn command() -> clap::Command {
    Cli::command()
}

/// Parse CLI arguments.
pub fn parse(args: impl IntoIterator<Item = String>) -> CliResult<Cli> {
    Cli::try_parse_from(args).map_err(|error| clap_error(&error))
}

/// Parse CLI arguments from OS strings.
#[allow(dead_code)]
pub fn parse_os(args: impl IntoIterator<Item = OsString>) -> CliResult<Cli> {
    Cli::try_parse_from(args).map_err(|error| clap_error(&error))
}

fn clap_error(error: &clap::Error) -> CliError {
    match error.kind() {
        clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => {
            CliError::Display(error.to_string())
        }
        _ => CliError::Usage(error.to_string()),
    }
}
