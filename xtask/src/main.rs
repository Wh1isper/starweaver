#![allow(
    missing_docs,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::cast_precision_loss,
    clippy::too_many_lines
)]

use std::{env, process::ExitCode};

mod api;
mod architecture;
mod capabilities;
mod common;
mod coverage;
mod docs;
mod fixtures;
mod release;
mod rpc_contracts;
mod rpc_interop_e2e;
mod smoke;

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
    let mut args: Vec<String> = env::args().skip(1).collect();
    let command = args.first().cloned().ok_or_else(usage)?;
    args.remove(0);
    match command.as_str() {
        "check-agent-api" => api::check_agent_api(&args),
        "check-architecture" => architecture::check_boundaries(),
        "check-capabilities" => capabilities::check(&args),
        "check-cli-examples" => smoke::check_cli_examples(),
        "check-docs-examples" => docs::check_docs_examples(&args),
        "check-rpc-contracts" => rpc_contracts::check(&args),
        "check-rpc-transports" => rpc_contracts::check_transports(&args),
        "check-rpc-interop-e2e" => rpc_interop_e2e::check(),
        "generate-rpc-contracts" => rpc_contracts::generate(&args),
        "check-install-script" => smoke::check_install_script(),
        "check-repository-scripts" => smoke::check_repository_scripts(),
        "smoke-cli-release" => smoke::smoke_cli_release(),
        "coverage-gate" => coverage::coverage_gate(&args),
        "finalize-docs-site" => docs::finalize_docs_site(),
        "import-model-cassettes" => fixtures::import_model_cassettes(&args),
        "publish" => release::publish(&args),
        "publish-dry-run" => release::publish_dry_run(),
        "record-model-cassette" => fixtures::record_model_cassette(&args),
        "scrub-model-cassette" => fixtures::scrub_model_cassette(&args),
        "summarize-model-fixtures" => fixtures::summarize_model_fixtures(&args),
        "upversion" => release::upversion(&args),
        "workspace-version" => release::workspace_version(&args),
        _ => Err(usage()),
    }
}

fn usage() -> String {
    "usage: cargo run -p xtask -- <command> [args]".to_string()
}
