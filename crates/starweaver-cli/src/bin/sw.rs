//! Short `sw` launcher alias.

use std::process::ExitCode;

fn main() -> ExitCode {
    match starweaver_cli::launcher::run_from_env() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}
