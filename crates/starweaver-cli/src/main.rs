//! Starweaver command-line entry point.

use std::{env, process::ExitCode, sync::Arc};

use starweaver_agent::{AgentBuilder, FunctionModel};
use starweaver_core::{sdk_name, TraceContext};
use starweaver_model::ModelResponse;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    let output = command_output(env::args().skip(1))?;
    print!("{output}");
    Ok(())
}

fn command_output(args: impl IntoIterator<Item = String>) -> Result<String, String> {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        None | Some("version") => Ok(format!("{}\n", sdk_name())),
        Some("run") => run_prompt(args.collect::<Vec<_>>().join(" ")),
        Some("diagnostics") => Ok(format!(
            "sdk={}\nworkspace_version={}\n",
            sdk_name(),
            env!("CARGO_PKG_VERSION")
        )),
        Some("session") => session_output(args),
        Some("replay-check") => {
            Ok("run `make replay-check` from the repository root\n".to_string())
        }
        Some(other) => Err(format!(
            "unknown command {other}; expected version, run, diagnostics, session, or replay-check"
        )),
    }
}

fn run_prompt(prompt: String) -> Result<String, String> {
    if prompt.is_empty() {
        return Err("usage: starweaver-cli run <prompt>".to_string());
    }
    let model = FunctionModel::new(move |_messages, _settings, _info| {
        Ok(ModelResponse::text(format!("local echo: {prompt}")))
    });
    let agent = AgentBuilder::new(Arc::new(model)).build();
    let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
    let result = runtime
        .block_on(agent.run("local cli run"))
        .map_err(|error| error.to_string())?;
    Ok(format!("{}\n", result.output))
}

fn session_output(mut args: impl Iterator<Item = String>) -> Result<String, String> {
    match args.next().as_deref() {
        Some("inspect") => {
            let session_id = args.next().unwrap_or_else(|| "local".to_string());
            let trace_context = TraceContext::from_trace_id(format!("trace-{session_id}"));
            Ok(format!(
                "session_id={session_id}\ntrace_id={}\nstore=in-memory\n",
                trace_context.trace_id.unwrap_or_default()
            ))
        }
        _ => Err("usage: starweaver-cli session inspect <session-id>".to_string()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn output(args: &[&str]) -> Result<String, String> {
        command_output(args.iter().map(|arg| (*arg).to_string()))
    }

    #[test]
    fn command_output_covers_version_and_diagnostics() {
        assert_eq!(output(&[]).unwrap(), "starweaver-agent-sdk\n");
        assert_eq!(output(&["version"]).unwrap(), "starweaver-agent-sdk\n");
        let diagnostics = output(&["diagnostics"]).unwrap();
        assert!(diagnostics.contains("sdk=starweaver-agent-sdk"));
        assert!(diagnostics.contains("workspace_version="));
    }

    #[test]
    fn command_output_covers_run_success_and_usage_error() {
        assert_eq!(output(&["run", "hello"]).unwrap(), "local echo: hello\n");
        assert_eq!(
            output(&["run"]).unwrap_err(),
            "usage: starweaver-cli run <prompt>"
        );
    }

    #[test]
    fn command_output_covers_session_inspect_and_usage_error() {
        assert_eq!(
            output(&["session", "inspect", "abc"]).unwrap(),
            "session_id=abc\ntrace_id=trace-abc\nstore=in-memory\n"
        );
        assert_eq!(
            output(&["session"]).unwrap_err(),
            "usage: starweaver-cli session inspect <session-id>"
        );
    }

    #[test]
    fn command_output_covers_replay_check_and_unknown_command() {
        assert_eq!(
            output(&["replay-check"]).unwrap(),
            "run `make replay-check` from the repository root\n"
        );
        assert!(output(&["wat"])
            .unwrap_err()
            .contains("unknown command wat"));
    }
}
