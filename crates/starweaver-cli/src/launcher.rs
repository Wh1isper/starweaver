//! Product launcher for `starweaver` and `sw`.

use std::{
    env,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use starweaver_core::sdk_name;

use crate::{update_check, CliError, CliResult};

const INSTALL_SCRIPT_URL: &str =
    "https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh";

const DIRECT_CLI_COMMANDS: &[&str] = &[
    "run",
    "session",
    "profile",
    "setup",
    "auth",
    "skill",
    "subagent",
    "mcp",
    "tools",
    "tui",
    "approval",
    "deferred",
    "resume",
    "reset",
    "diagnostics",
    "replay-check",
    "config",
    "completion",
];

/// Update command options shared by launcher and direct CLI paths.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct UpdateOptions {
    /// Print the update plan without downloading or installing.
    pub(crate) dry_run: bool,
    /// Reinstall even when the selected release matches the current version.
    pub(crate) force: bool,
}

impl UpdateOptions {
    fn with_env(self) -> Self {
        Self {
            dry_run: self.dry_run || env::var_os("STARWEAVER_UPDATE_DRY_RUN").is_some(),
            force: self.force || env::var_os("STARWEAVER_UPDATE_FORCE").is_some(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedUpdateCommand {
    target: String,
    options: UpdateOptions,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct UpdateTargetVersion {
    version: String,
    source: &'static str,
    explicit: bool,
}

enum LauncherDispatch<'a> {
    Cli(&'a [String]),
    External(&'a str, &'a [String]),
}

/// Run launcher from process arguments.
pub fn run_from_env() -> CliResult<()> {
    run(env::args())
}

/// Run launcher from arguments.
pub fn run(args: impl IntoIterator<Item = String>) -> CliResult<()> {
    let args = args.into_iter().collect::<Vec<_>>();
    match launcher_dispatch_parts(&args) {
        Some(LauncherDispatch::Cli(remaining)) => {
            let cli_args = std::iter::once("starweaver-cli".to_string()).chain(remaining.to_vec());
            return crate::run(cli_args);
        }
        Some(LauncherDispatch::External(command, remaining)) => {
            return run_external(command, remaining);
        }
        None => {}
    }
    let output = command_output(args)?;
    print!("{output}");
    Ok(())
}

/// Return launcher command output.
pub fn command_output(args: impl IntoIterator<Item = String>) -> CliResult<String> {
    let mut args = args.into_iter();
    let program = display_program_name(args.next().as_deref());
    match args.next().as_deref() {
        None | Some("--help" | "-h") => Ok(launcher_help(&program)),
        Some("help") => launcher_help_command(&program, args.collect()),
        Some("version" | "--version" | "-V") => {
            let remaining = args.collect::<Vec<_>>();
            version_output(&program, &remaining)
        }
        Some("doctor") => {
            let remaining = args.collect::<Vec<_>>();
            doctor_output(&program, &remaining)
        }
        Some("update") => update_component_from_args(&program, args.collect()),
        Some("cli") => {
            let remaining = args.collect::<Vec<_>>();
            if remaining.first().is_some_and(|arg| arg == "update") {
                return update_component_from_args(
                    &program,
                    remaining.into_iter().skip(1).collect(),
                );
            }
            let cli_args = std::iter::once("starweaver-cli".to_string()).chain(remaining);
            crate::command_output(cli_args)
        }
        Some(command) if is_direct_cli_arg(command) => {
            let cli_args = std::iter::once("starweaver-cli".to_string())
                .chain(std::iter::once(command.to_string()))
                .chain(args);
            crate::command_output(cli_args)
        }
        Some(command) => dispatch_external(command, args.collect()),
    }
}

fn launcher_dispatch_parts(args: &[String]) -> Option<LauncherDispatch<'_>> {
    let command = args.get(1)?;
    match command.as_str() {
        "help" | "--help" | "-h" | "version" | "--version" | "-V" | "doctor" | "update" => None,
        "cli" => Some(LauncherDispatch::Cli(&args[2..])),
        command if is_direct_cli_arg(command) => Some(LauncherDispatch::Cli(&args[1..])),
        _ => Some(LauncherDispatch::External(command, &args[2..])),
    }
}

fn is_direct_cli_arg(command: &str) -> bool {
    is_cli_option(command) || DIRECT_CLI_COMMANDS.contains(&command)
}

fn is_cli_option(command: &str) -> bool {
    command.starts_with('-') && !matches!(command, "--help" | "-h" | "--version" | "-V")
}

fn display_program_name(program: Option<&str>) -> String {
    program
        .and_then(|value| Path::new(value).file_name())
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("sw")
        .to_string()
}

fn launcher_help(program: &str) -> String {
    format!(
        "\
Starweaver product launcher

Usage:
  {program} [COMMAND] [ARGS...]
  {program} -p \"prompt\"
  {program} run \"prompt\"
  {program} cli [ARGS...]

Commands:
  cli          Open the local CLI/TUI or pass through CLI arguments
  run          Run a prompt directly
  session      Manage local sessions
  config       Get or set configuration
  rpc          Run the standalone JSON-RPC host
  update       Update installed CLI artifacts
  doctor       Print launcher diagnostics
  version      Print SDK identity
  help         Print this help or CLI command help

Examples:
  {program} -p \"summarize this repository\" --output text
  {program} run \"hello\" --output silent
  {program} session list
  {program} update --dry-run
  {program} rpc stdio

Use `{program} cli --help` for the full local CLI surface.
Use `{program} <command> --help` for direct CLI command help.
"
    )
}

fn launcher_help_command(program: &str, topics: Vec<String>) -> CliResult<String> {
    let Some(topic) = topics.first() else {
        return Ok(launcher_help(program));
    };
    if topic == "cli" {
        let cli_args = std::iter::once("starweaver-cli".to_string())
            .chain(topics.into_iter().skip(1))
            .chain(std::iter::once("--help".to_string()));
        return crate::command_output(cli_args);
    }
    if topic == "update" {
        return Ok(update_help(program));
    }
    if topic == "doctor" {
        return Ok(doctor_help(program));
    }
    if topic == "version" {
        return Ok(version_help(program));
    }
    if DIRECT_CLI_COMMANDS.contains(&topic.as_str()) {
        let cli_args = std::iter::once("starweaver-cli".to_string())
            .chain(std::iter::once("help".to_string()))
            .chain(topics);
        return crate::command_output(cli_args);
    }
    Ok(launcher_help(program))
}

fn version_output(program: &str, args: &[String]) -> CliResult<String> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        return Ok(version_help(program));
    }
    if args.is_empty() {
        Ok(format!("{}\n", sdk_name()))
    } else {
        Err(CliError::Usage(
            "version does not accept positional arguments".to_string(),
        ))
    }
}

fn version_help(program: &str) -> String {
    format!(
        "\
Print SDK identity.

Usage:
  {program} version
"
    )
}

fn doctor_output(program: &str, args: &[String]) -> CliResult<String> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        return Ok(doctor_help(program));
    }
    if !args.is_empty() {
        return Err(CliError::Usage(
            "doctor does not accept positional arguments".to_string(),
        ));
    }
    Ok(format!(
        "sdk={}\nlauncher=starweaver\ncli=starweaver-cli\ninstall_script={}\n",
        sdk_name(),
        INSTALL_SCRIPT_URL
    ))
}

fn doctor_help(program: &str) -> String {
    format!(
        "\
Print launcher diagnostics.

Usage:
  {program} doctor
"
    )
}

/// Update an installed Starweaver component.
pub fn update_component(component: &str) -> CliResult<String> {
    update_component_with_env_options(component, UpdateOptions::default())
}

pub(crate) fn update_component_with_env_options(
    component: &str,
    options: UpdateOptions,
) -> CliResult<String> {
    let install_dir = env::var("STARWEAVER_INSTALL_DIR").unwrap_or_else(|_| default_install_dir());
    update_component_with_options(component, &install_dir, options.with_env())
}

pub(crate) fn update_component_with_options(
    component: &str,
    install_dir: &str,
    options: UpdateOptions,
) -> CliResult<String> {
    let normalized = match component {
        "cli" | "starweaver-cli" | "starweaver" | "launcher" | "rpc" | "starweaver-rpc" => "cli",
        other => return Err(CliError::Usage(format!("unknown update target {other}"))),
    };
    let command = update_shell_command(normalized, install_dir);
    let current_version = env!("CARGO_PKG_VERSION");
    if options.dry_run {
        let mut output =
            update_output_prefix(normalized, current_version, None, options, "dry-run");
        output.push_str("command=");
        output.push_str(&command);
        output.push('\n');
        return Ok(output);
    }
    let target_version = resolve_update_target_version();
    if !options.force {
        if let Some(target_version) = &target_version {
            if update_should_skip(current_version, target_version) {
                return Ok(update_output_prefix(
                    normalized,
                    current_version,
                    Some(target_version),
                    options,
                    "up-to-date",
                ));
            }
        }
    }
    let script = fetch_install_script()?;
    let mut child = Command::new("sh")
        .env("STARWEAVER_COMPONENTS", normalized)
        .env("STARWEAVER_INSTALL_DIR", install_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| CliError::Run(error.to_string()))?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| CliError::Run("installer stdin unavailable".to_string()))?
        .write_all(script.as_bytes())
        .map_err(|error| CliError::Run(error.to_string()))?;
    let output = child
        .wait_with_output()
        .map_err(|error| CliError::Run(error.to_string()))?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut update = update_output_prefix(
            normalized,
            current_version,
            target_version.as_ref(),
            options,
            "updated",
        );
        update.push_str(&stdout);
        Ok(update)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(CliError::Run(format!(
            "update target {normalized} exited with status {}: {stderr}",
            output.status
        )))
    }
}

fn fetch_install_script() -> CliResult<String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::Run(error.to_string()))?;
    runtime.block_on(async {
        let response = reqwest::Client::new()
            .get(INSTALL_SCRIPT_URL)
            .header(reqwest::header::USER_AGENT, "starweaver-cli")
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|error| CliError::Run(error.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| CliError::Run(error.to_string()))?;
        if status.is_success() {
            Ok(body)
        } else {
            Err(CliError::Run(format!(
                "failed to download installer from {INSTALL_SCRIPT_URL}: {body}"
            )))
        }
    })
}

fn update_component_from_args(program: &str, args: Vec<String>) -> CliResult<String> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        return Ok(update_help(program));
    }
    let command = parse_update_args(args)?;
    update_component_with_env_options(&command.target, command.options)
}

fn update_help(program: &str) -> String {
    format!(
        "\
Update installed Starweaver CLI artifacts.

Usage:
  {program} update [TARGET] [--dry-run] [--force]

Arguments:
  TARGET       Update target, defaults to cli

Options:
  --dry-run    Print the update plan without downloading or installing
  -f, --force  Reinstall even when the selected release matches the current version
  -h, --help   Print help
"
    )
}

fn parse_update_args(args: Vec<String>) -> CliResult<ParsedUpdateCommand> {
    let mut target = None;
    let mut options = UpdateOptions::default();
    for arg in args {
        if arg == "--dry-run" {
            options.dry_run = true;
        } else if arg == "--force" || arg == "-f" {
            options.force = true;
        } else if arg.starts_with('-') {
            return Err(CliError::Usage(format!("unknown update option {arg}")));
        } else if target.replace(arg).is_some() {
            return Err(CliError::Usage(
                "update accepts at most one target".to_string(),
            ));
        }
    }
    Ok(ParsedUpdateCommand {
        target: target.unwrap_or_else(|| "cli".to_string()),
        options,
    })
}

fn resolve_update_target_version() -> Option<UpdateTargetVersion> {
    if let Some(version) = requested_version_from_env() {
        return Some(UpdateTargetVersion {
            version,
            source: "env",
            explicit: true,
        });
    }
    let cache = update_check::fetch_latest_release().ok()?;
    cache.latest_version.map(|version| UpdateTargetVersion {
        version,
        source: "latest",
        explicit: false,
    })
}

fn requested_version_from_env() -> Option<String> {
    let version = env::var("STARWEAVER_VERSION").ok()?;
    let version = version.trim();
    if version.is_empty() || version == "latest" {
        None
    } else {
        Some(version.trim_start_matches('v').to_string())
    }
}

fn update_should_skip(current_version: &str, target_version: &UpdateTargetVersion) -> bool {
    if target_version.explicit {
        update_check::versions_match(current_version, &target_version.version)
    } else {
        !update_check::update_is_newer(current_version, &target_version.version)
    }
}

fn update_output_prefix(
    normalized: &str,
    current_version: &str,
    target_version: Option<&UpdateTargetVersion>,
    options: UpdateOptions,
    status: &str,
) -> String {
    let mut output =
        format!("update=github-release\ntarget={normalized}\ncurrent_version={current_version}\n");
    if let Some(target_version) = target_version {
        output.push_str("target_version=");
        output.push_str(&target_version.version);
        output.push_str("\ntarget_version_source=");
        output.push_str(target_version.source);
        output.push('\n');
    }
    if options.force {
        output.push_str("force=true\n");
    }
    output.push_str("status=");
    output.push_str(status);
    output.push('\n');
    output
}

fn update_shell_command(normalized: &str, install_dir: &str) -> String {
    let mut env_parts = inherited_update_env();
    env_parts.push(format!("STARWEAVER_COMPONENTS={normalized}"));
    env_parts.push(format!(
        "STARWEAVER_INSTALL_DIR={}",
        shell_quote(install_dir)
    ));
    format!("download {INSTALL_SCRIPT_URL} | {} sh", env_parts.join(" "))
}

fn inherited_update_env() -> Vec<String> {
    [
        "STARWEAVER_VERSION",
        "STARWEAVER_GITHUB_REPO",
        "STARWEAVER_REPO",
    ]
    .iter()
    .filter_map(|key| {
        let value = env::var(key).ok()?;
        (!value.is_empty()).then(|| format!("{key}={}", shell_quote(&value)))
    })
    .collect()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn default_install_dir() -> String {
    env::var_os("HOME")
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join(".local/bin")
        .display()
        .to_string()
}

fn dispatch_external(command: &str, args: Vec<String>) -> CliResult<String> {
    let binary = format!("starweaver-{command}");
    let program = find_installed_binary(&binary).unwrap_or_else(|| PathBuf::from(&binary));
    let output = Command::new(&program)
        .args(args)
        .output()
        .map_err(|error| CliError::Usage(format!("unknown command {command}: {error}")))?;
    if output.status.success() {
        String::from_utf8(output.stdout).map_err(|error| CliError::Run(error.to_string()))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(CliError::Run(format!(
            "{binary} exited with status {}: {stderr}",
            output.status
        )))
    }
}

fn run_external(command: &str, args: &[String]) -> CliResult<()> {
    let binary = format!("starweaver-{command}");
    let program = find_installed_binary(&binary).unwrap_or_else(|| PathBuf::from(&binary));
    let status = Command::new(&program)
        .args(args)
        .status()
        .map_err(|error| CliError::Usage(format!("unknown command {command}: {error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::Run(format!(
            "{binary} exited with status {status}"
        )))
    }
}

fn find_installed_binary(binary: &str) -> Option<PathBuf> {
    let current = env::current_exe().ok()?;
    let install_dir = current.parent()?;
    let candidate = install_dir.join(binary);
    candidate.exists().then_some(candidate)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn launcher_dispatches_cli_in_process() {
        let temp = tempfile::tempdir().unwrap();
        let args = vec![
            "starweaver".to_string(),
            "cli".to_string(),
            "--store".to_string(),
            temp.path().join("sessions.sqlite").display().to_string(),
            "-p".to_string(),
            "hello".to_string(),
            "--profile".to_string(),
            "general".to_string(),
            "--output".to_string(),
            "display-jsonl".to_string(),
        ];
        let output = command_output(args).unwrap();
        let first: serde_json::Value =
            serde_json::from_str(output.lines().next().unwrap()).unwrap();
        assert_eq!(first["schema"], "starweaver.display.v1");
        assert_eq!(first["type"], "RUN_QUEUED");
    }

    #[test]
    fn launcher_reports_version_doctor_and_update_plan() {
        assert_eq!(
            command_output(["sw".to_string(), "version".to_string()]).unwrap(),
            "starweaver-agent-sdk\n"
        );
        let help = command_output(["sw".to_string()]).unwrap();
        assert!(help.contains("Starweaver product launcher"));
        assert!(help.contains("Usage:"));
        assert!(help.contains("sw run \"hello\""));
        let flag_help = command_output(["sw".to_string(), "--help".to_string()]).unwrap();
        assert!(flag_help.contains("Use `sw cli --help`"));
        let command_help = command_output(["sw".to_string(), "help".to_string()]).unwrap();
        assert!(command_help.contains("Commands:"));
        let doctor = command_output(["starweaver".to_string(), "doctor".to_string()]).unwrap();
        assert!(doctor.contains("launcher=starweaver"));
        assert!(doctor.contains("cli=starweaver-cli"));
        let doctor_help = command_output([
            "starweaver".to_string(),
            "doctor".to_string(),
            "--help".to_string(),
        ])
        .unwrap();
        assert!(doctor_help.contains("starweaver doctor"));
        let temp = tempfile::tempdir().unwrap();
        let install_dir = temp.path().display().to_string();
        let update = update_component_with_options("cli", &install_dir, dry_run_options()).unwrap();
        assert!(update.contains("update=github-release"));
        assert!(update.contains("target=cli"));
        assert!(update.contains("current_version="));
        assert!(update.contains("status=dry-run"));
        let quoted =
            update_component_with_options("cli", "dir with ' quote", dry_run_options()).unwrap();
        assert!(quoted.contains("target=cli"));
        assert!(quoted.contains("'\\''"));
        let update_help =
            command_output(["sw".to_string(), "update".to_string(), "--help".to_string()]).unwrap();
        assert!(update_help.contains("sw update [TARGET]"));
        let run_help =
            command_output(["sw".to_string(), "run".to_string(), "--help".to_string()]).unwrap();
        assert!(run_help.contains("Run a prompt"));
    }

    #[test]
    fn launcher_run_dispatches_long_lived_commands_without_capture() {
        let args = vec![
            "starweaver".to_string(),
            "rpc".to_string(),
            "stdio".to_string(),
        ];
        let Some(LauncherDispatch::External(command, remaining)) = launcher_dispatch_parts(&args)
        else {
            panic!("expected external dispatch");
        };
        assert_eq!(command, "rpc");
        assert_eq!(remaining, &["stdio".to_string()]);

        let direct_run = vec!["sw".to_string(), "run".to_string(), "hello".to_string()];
        let Some(LauncherDispatch::Cli(remaining)) = launcher_dispatch_parts(&direct_run) else {
            panic!("expected cli dispatch");
        };
        assert_eq!(remaining, &["run".to_string(), "hello".to_string()]);

        let prompt_flag = vec!["sw".to_string(), "-p".to_string(), "hello".to_string()];
        let Some(LauncherDispatch::Cli(remaining)) = launcher_dispatch_parts(&prompt_flag) else {
            panic!("expected cli dispatch");
        };
        assert_eq!(remaining, &["-p".to_string(), "hello".to_string()]);
    }

    #[test]
    fn cli_update_uses_cli_target() {
        let temp = tempfile::tempdir().unwrap();
        let output = update_component_with_options(
            "starweaver-cli",
            &temp.path().display().to_string(),
            dry_run_options(),
        )
        .unwrap();
        assert!(output.contains("target=cli"));
        assert!(output.contains("STARWEAVER_COMPONENTS=cli"));

        let starweaver =
            update_component_with_options("starweaver", "/tmp/install", dry_run_options()).unwrap();
        assert!(starweaver.contains("target=cli"));
        let launcher =
            update_component_with_options("launcher", "/tmp/install", dry_run_options()).unwrap();
        assert!(launcher.contains("target=cli"));
        let rpc =
            update_component_with_options("starweaver-rpc", "/tmp/install", dry_run_options())
                .unwrap();
        assert!(rpc.contains("target=cli"));
        assert!(matches!(
            update_component_with_options("unknown", "/tmp/install", dry_run_options()),
            Err(CliError::Usage(message)) if message.contains("unknown update target unknown")
        ));
    }

    #[test]
    fn launcher_update_args_parse_flags_and_target() {
        assert_eq!(
            parse_update_args(vec!["--dry-run".to_string(), "--force".to_string()]).unwrap(),
            ParsedUpdateCommand {
                target: "cli".to_string(),
                options: UpdateOptions {
                    dry_run: true,
                    force: true,
                },
            }
        );
        assert_eq!(
            parse_update_args(vec!["rpc".to_string(), "-f".to_string()]).unwrap(),
            ParsedUpdateCommand {
                target: "rpc".to_string(),
                options: UpdateOptions {
                    dry_run: false,
                    force: true,
                },
            }
        );
        assert!(matches!(
            parse_update_args(vec!["--unknown".to_string()]),
            Err(CliError::Usage(message)) if message.contains("unknown update option --unknown")
        ));
        assert!(matches!(
            parse_update_args(vec!["cli".to_string(), "rpc".to_string()]),
            Err(CliError::Usage(message)) if message.contains("at most one target")
        ));
    }

    #[test]
    fn launcher_update_skip_rule_distinguishes_latest_from_pinned_versions() {
        let latest_same = UpdateTargetVersion {
            version: "0.0.1".to_string(),
            source: "latest",
            explicit: false,
        };
        let latest_newer = UpdateTargetVersion {
            version: "0.0.2".to_string(),
            source: "latest",
            explicit: false,
        };
        let pinned_same = UpdateTargetVersion {
            version: "0.0.1".to_string(),
            source: "env",
            explicit: true,
        };
        let pinned_older = UpdateTargetVersion {
            version: "0.0.0".to_string(),
            source: "env",
            explicit: true,
        };
        assert!(update_should_skip("0.0.1", &latest_same));
        assert!(!update_should_skip("0.0.1", &latest_newer));
        assert!(update_should_skip("0.0.1", &pinned_same));
        assert!(!update_should_skip("0.0.1", &pinned_older));
    }

    #[test]
    fn launcher_command_output_covers_version_aliases_and_external_errors() {
        assert!(matches!(
            command_output(["starweaver".to_string(), "unknown".to_string()]),
            Err(CliError::Usage(message)) if message.contains("unknown command unknown")
        ));
        assert!(matches!(
            command_output([
                "starweaver".to_string(),
                "external".to_string(),
                "doctor".to_string(),
            ]),
            Err(CliError::Usage(message)) if message.contains("unknown command external")
        ));
        assert!(command_output(["sw".to_string()])
            .unwrap()
            .contains("Usage:"));
        assert!(command_output(["sw".to_string(), "--version".to_string()])
            .unwrap()
            .contains("starweaver-agent-sdk"));
        assert!(command_output(["sw".to_string(), "-V".to_string()])
            .unwrap()
            .contains("starweaver-agent-sdk"));
    }

    const fn dry_run_options() -> UpdateOptions {
        UpdateOptions {
            dry_run: true,
            force: false,
        }
    }
}
