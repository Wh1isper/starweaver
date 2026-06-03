//! Product launcher for `starweaver` and `sw`.

use std::{
    env,
    io::Write as _,
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};

use starweaver_core::sdk_name;

use crate::{CliError, CliResult};

const INSTALL_SCRIPT_URL: &str =
    "https://raw.githubusercontent.com/Wh1isper/starweaver/main/scripts/install.sh";

/// Run launcher from process arguments.
pub fn run_from_env() -> CliResult<()> {
    run(env::args())
}

/// Run launcher from arguments.
pub fn run(args: impl IntoIterator<Item = String>) -> CliResult<()> {
    let output = command_output(args)?;
    print!("{output}");
    Ok(())
}

/// Return launcher command output.
pub fn command_output(args: impl IntoIterator<Item = String>) -> CliResult<String> {
    let mut args = args.into_iter();
    let _program = args.next();
    match args.next().as_deref() {
        None | Some("version" | "--version" | "-V") => Ok(format!("{}\n", sdk_name())),
        Some("doctor") => Ok(format!(
            "sdk={}\nlauncher=starweaver\ncli=starweaver-cli\ninstall_script={}\n",
            sdk_name(),
            INSTALL_SCRIPT_URL
        )),
        Some("update") => update_component(args.next().as_deref().unwrap_or("cli")),
        Some("cli") => {
            let remaining = args.collect::<Vec<_>>();
            if remaining.first().is_some_and(|arg| arg == "update") {
                return update_component("cli");
            }
            let cli_args = std::iter::once("starweaver-cli".to_string()).chain(remaining);
            crate::command_output(cli_args)
        }
        Some("claw") => {
            let remaining = args.collect::<Vec<_>>();
            if remaining.first().is_some_and(|arg| arg == "update") {
                return update_component("claw");
            }
            dispatch_external("claw", remaining)
        }
        Some(command) => dispatch_external(command, args.collect()),
    }
}

/// Update an installed Starweaver component.
pub fn update_component(component: &str) -> CliResult<String> {
    let install_dir = env::var("STARWEAVER_INSTALL_DIR").unwrap_or_else(|_| default_install_dir());
    update_component_with_options(
        component,
        &install_dir,
        env::var_os("STARWEAVER_UPDATE_DRY_RUN").is_some(),
    )
}

pub(crate) fn update_component_with_options(
    component: &str,
    install_dir: &str,
    dry_run: bool,
) -> CliResult<String> {
    let normalized = match component {
        "cli" | "starweaver-cli" | "starweaver" | "launcher" => "cli",
        "claw" | "starweaver-claw" => "claw",
        other => return Err(CliError::Usage(format!("unknown update target {other}"))),
    };
    let command = format!(
        "download {INSTALL_SCRIPT_URL} | STARWEAVER_COMPONENTS={normalized} STARWEAVER_INSTALL_DIR={} sh",
        shell_quote(install_dir)
    );
    if dry_run {
        return Ok(format!(
            "update=github-release\ntarget={normalized}\nstatus=dry-run\ncommand={command}\n"
        ));
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
        Ok(format!(
            "update=github-release\ntarget={normalized}\nstatus=updated\n{stdout}"
        ))
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
        let doctor = command_output(["starweaver".to_string(), "doctor".to_string()]).unwrap();
        assert!(doctor.contains("launcher=starweaver"));
        assert!(doctor.contains("cli=starweaver-cli"));
        let temp = tempfile::tempdir().unwrap();
        let install_dir = temp.path().display().to_string();
        let update = update_component_with_options("cli", &install_dir, true).unwrap();
        assert!(update.contains("update=github-release"));
        assert!(update.contains("target=cli"));
        assert!(update.contains("status=dry-run"));
        let quoted = update_component_with_options("claw", "dir with ' quote", true).unwrap();
        assert!(quoted.contains("target=claw"));
        assert!(quoted.contains("'\\''"));
    }

    #[test]
    fn cli_update_uses_cli_target() {
        let temp = tempfile::tempdir().unwrap();
        let output = update_component_with_options(
            "starweaver-cli",
            &temp.path().display().to_string(),
            true,
        )
        .unwrap();
        assert!(output.contains("target=cli"));
        assert!(output.contains("STARWEAVER_COMPONENTS=cli"));
    }
}
