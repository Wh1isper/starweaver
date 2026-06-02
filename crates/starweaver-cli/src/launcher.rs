//! Product launcher for `starweaver` and `sw`.

use std::{env, process::Command};

use starweaver_core::sdk_name;

use crate::{CliError, CliResult};

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
            "sdk={}\nlauncher=starweaver\ncli=starweaver-cli\n",
            sdk_name()
        )),
        Some("update") => Ok(
            "update=github-release\nstatus=manual\nmessage=download the latest release asset from https://github.com/Wh1isper/starweaver/releases\n"
                .to_string(),
        ),
        Some("cli") => {
            let cli_args = std::iter::once("starweaver-cli".to_string()).chain(args);
            crate::command_output(cli_args)
        }
        Some(command) => dispatch_external(command, args.collect()),
    }
}

fn dispatch_external(command: &str, args: Vec<String>) -> CliResult<String> {
    let binary = format!("starweaver-{command}");
    let output = Command::new(&binary)
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
        ];
        let output = command_output(args).unwrap();
        let first: serde_json::Value =
            serde_json::from_str(output.lines().next().unwrap()).unwrap();
        assert_eq!(first["schema"], "starweaver.display.v1");
        assert_eq!(first["type"], "RUN_STARTED");
    }

    #[test]
    fn launcher_reports_version_and_doctor() {
        assert_eq!(
            command_output(["sw".to_string(), "version".to_string()]).unwrap(),
            "starweaver-agent-sdk\n"
        );
        let doctor = command_output(["starweaver".to_string(), "doctor".to_string()]).unwrap();
        assert!(doctor.contains("launcher=starweaver"));
        assert!(doctor.contains("cli=starweaver-cli"));
        let update = command_output(["starweaver".to_string(), "update".to_string()]).unwrap();
        assert!(update.contains("update=github-release"));
    }
}
