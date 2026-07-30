//! Local stdio MCP process for external Computer Use harnesses.

use std::{io, process::ExitCode, time::Duration};

use clap::{ArgGroup, Parser, ValueEnum};
use rmcp::{ServiceExt as _, transport::stdio};
use serde::Serialize;
use starweaver_computer_use::{
    BoundedStdioTransport, CloseReason, ComputerCapabilityGrant, ComputerStatus, ComputerToolGrant,
    ComputerUseContractVersion, ComputerUseMcpServer, ComputerUsePolicy, DesktopSurfaceScope,
    DynComputerUseService, McpResourceLimits, PermissionPromptPolicy, PermissionRequest,
    PermissionRequestOutcome, ToolCatalogVersion, current_desktop_service,
    current_desktop_tool_grant,
};
use starweaver_core::CancellationToken;

const PROCESS_SHUTDOWN_BUDGET: Duration = Duration::from_secs(20);
const RUNTIME_SHUTDOWN_BUDGET: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DesktopScopeArg {
    PrimaryDisplay,
    VisibleDesktop,
}

impl From<DesktopScopeArg> for DesktopSurfaceScope {
    fn from(value: DesktopScopeArg) -> Self {
        match value {
            DesktopScopeArg::PrimaryDisplay => Self::PrimaryDisplay,
            DesktopScopeArg::VisibleDesktop => Self::VisibleDesktop,
        }
    }
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Parser)]
#[command(
    name = "starweaver-computer-use-mcp",
    about = "Attended current-desktop Computer Use over local stdio MCP",
    disable_version_flag = true,
    group = ArgGroup::new("mode")
        .required(true)
        .multiple(false)
        .args(["stdio", "doctor", "request_permissions", "version"])
)]
struct Args {
    /// Serve exactly one MCP client over stdin/stdout.
    #[arg(long)]
    stdio: bool,

    /// Probe current-desktop readiness without capturing pixels or injecting input.
    #[arg(long)]
    doctor: bool,

    /// Request attended Screen Recording and Accessibility permissions.
    #[arg(long)]
    request_permissions: bool,

    /// Print package, catalog, feature, and build-target versions.
    #[arg(long)]
    version: bool,

    /// Select the current primary display or complete visible desktop.
    #[arg(long, value_enum, default_value_t = DesktopScopeArg::PrimaryDisplay)]
    desktop_scope: DesktopScopeArg,

    /// Deprecated compatibility flag; pointer tools are included automatically.
    #[arg(long = "allow-pointer", hide = true)]
    legacy_allow_pointer: bool,

    /// Deprecated compatibility flag; keyboard tools are included automatically.
    #[arg(long = "allow-keyboard", hide = true)]
    legacy_allow_keyboard: bool,

    /// Emit doctor or onboarding output as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Serialize)]
struct DiagnosticReport {
    package_version: &'static str,
    build_target: &'static str,
    release_readiness: &'static str,
    service_contract: ComputerUseContractVersion,
    tool_catalog: ToolCatalogVersion,
    effective_tool_grant: ComputerToolGrant,
    status: ComputerStatus,
}

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("starweaver-computer-use-mcp: failed to start async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(run(Args::parse()));
    // Native capture may be executing on Tokio's blocking pool. Never let an
    // uninterruptible OS call make process teardown wait without a bound.
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_BUDGET);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("starweaver-computer-use-mcp: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    if args.version {
        print_version();
        return Ok(());
    }

    let effective = current_desktop_tool_grant(requested_tool_grant(&args));
    let policy = policy(&args, effective);

    if args.stdio {
        return serve_stdio(policy, effective).await;
    }

    let service = current_desktop_service(policy);
    if args.request_permissions {
        let outcome = request_attended_permissions(&service).await?;
        print_permission_outcome(&outcome, args.json)?;
    } else {
        // Doctor is deliberately status-only: it must never invoke native prompt APIs.
        let status = service.status(CancellationToken::new()).await?;
        let report = DiagnosticReport {
            package_version: env!("CARGO_PKG_VERSION"),
            build_target: env!("STARWEAVER_BUILD_TARGET"),
            release_readiness: "native_current_desktop",
            service_contract: ComputerUseContractVersion::V1,
            tool_catalog: ToolCatalogVersion::V1,
            effective_tool_grant: effective,
            status,
        };
        print_report(&report, args.json)?;
    }
    tokio::time::timeout(
        PROCESS_SHUTDOWN_BUDGET,
        service.shutdown(CloseReason::HostShutdown),
    )
    .await
    .map_err(|_| shutdown_timeout_error())??;
    Ok(())
}

const fn requested_tool_grant(args: &Args) -> ComputerToolGrant {
    // Keep accepting the old opt-in flags so existing launch scripts do not fail,
    // but they no longer alter the all-or-nothing Computer Use product grant.
    let _ = (args.legacy_allow_pointer, args.legacy_allow_keyboard);
    ComputerToolGrant::full()
}

fn policy(args: &Args, effective: ComputerToolGrant) -> ComputerUsePolicy {
    ComputerUsePolicy {
        desktop_scope: args.desktop_scope.into(),
        allowed_capabilities: ComputerCapabilityGrant {
            observe: effective.observe,
            pointer: effective.pointer,
            keyboard: effective.keyboard,
            accessibility_snapshot: true,
        },
        // The stdio service never triggers TCC prompts implicitly. Onboarding is
        // confined to the explicit --request-permissions host command.
        permission_prompts: PermissionPromptPolicy {
            capture_on_open: false,
            accessibility_on_observe: false,
        },
        ..ComputerUsePolicy::default()
    }
}

async fn request_attended_permissions(
    service: &DynComputerUseService,
) -> Result<PermissionRequestOutcome, starweaver_computer_use::ComputerUseError> {
    service
        .request_permissions(
            PermissionRequest {
                screen_recording: true,
                accessibility: true,
            },
            CancellationToken::new(),
        )
        .await
}

async fn serve_stdio(
    policy: ComputerUsePolicy,
    grant: ComputerToolGrant,
) -> Result<(), Box<dyn std::error::Error>> {
    let service = current_desktop_service(policy);
    let shutdown = CancellationToken::new();
    let server = ComputerUseMcpServer::with_resource_limits(
        service,
        grant,
        McpResourceLimits::default(),
        shutdown.clone(),
    );
    let (stdin, stdout) = stdio();
    let transport = BoundedStdioTransport::new(stdin, stdout, shutdown.clone());
    let running = server.clone().serve(transport).await?;
    let mut waiting = Box::pin(running.waiting());
    let mut signal = Box::pin(termination_signal());
    let mut trigger_error = None;
    let (initial_reason, close_reason) = tokio::select! {
        biased;
        () = shutdown.cancelled() => (None, CloseReason::ClientDisconnected),
        result = &mut signal => match result {
            Ok(()) => (None, CloseReason::HostShutdown),
            Err(error) => {
                trigger_error = Some(error);
                (None, CloseReason::HostShutdown)
            }
        },
        reason = &mut waiting => (Some(reason), CloseReason::ClientDisconnected),
    };

    // EOF, terminal transport failure, SIGINT, SIGTERM, and an rmcp close all
    // converge here. The one absolute deadline covers checked native cleanup
    // and handler/transport completion; no stage renews the budget.
    let deadline = tokio::time::Instant::now() + PROCESS_SHUTDOWN_BUDGET;
    tokio::time::timeout_at(deadline, server.shutdown_checked(close_reason))
        .await
        .map_err(|_| shutdown_timeout_error())??;
    let reason = match initial_reason {
        Some(reason) => reason,
        None => tokio::time::timeout_at(deadline, &mut waiting)
            .await
            .map_err(|_| shutdown_timeout_error())?,
    };
    if let Some(error) = trigger_error {
        return Err(Box::new(error));
    }
    match reason? {
        rmcp::service::QuitReason::JoinError(error) => Err(Box::new(error)),
        _ => Ok(()),
    }
}

#[cfg(unix)]
async fn termination_signal() -> io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        _ = interrupt.recv() => Ok(()),
        _ = terminate.recv() => Ok(()),
    }
}

#[cfg(not(unix))]
async fn termination_signal() -> io::Result<()> {
    tokio::signal::ctrl_c().await
}

fn shutdown_timeout_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "MCP shutdown exceeded the process shutdown budget",
    )
}

fn print_version() {
    println!(
        "starweaver-computer-use-mcp {}\ncomputer-use-contract {}.{}\ntool-catalog {}.{}\nfeatures mcp-server,observe,pointer,keyboard\nrelease-readiness native-current-desktop\ntarget {}",
        env!("CARGO_PKG_VERSION"),
        ComputerUseContractVersion::V1.major,
        ComputerUseContractVersion::V1.minor,
        ToolCatalogVersion::V1.major,
        ToolCatalogVersion::V1.minor,
        env!("STARWEAVER_BUILD_TARGET")
    );
}

fn print_permission_outcome(
    outcome: &PermissionRequestOutcome,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if json {
        println!("{}", serde_json::to_string_pretty(outcome)?);
        return Ok(());
    }
    println!(
        "permission_request=immediate_outcome\nrequested_screen_recording={} requested_accessibility={}\nscreen_recording={:?} accessibility={:?} restart_required={}\neffective_observe={} effective_accessibility_snapshot={}\ndiagnostics_code={}\nremediation:",
        outcome.requested.screen_recording,
        outcome.requested.accessibility,
        outcome.permissions.capture,
        outcome.permissions.accessibility,
        outcome.permissions.restart_required,
        outcome.effective_capabilities.observe,
        outcome.effective_capabilities.accessibility_snapshot,
        outcome.diagnostics_code,
    );
    for item in &outcome.permissions.remediation {
        println!("- {item}");
    }
    println!(
        "The outcome reflects status immediately after the native requests returned; restart if macOS requires it."
    );
    Ok(())
}

fn print_report(report: &DiagnosticReport, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    println!(
        "release_readiness={}\nbackend={:?} platform={:?} scope={:?} active_session={:?}\ncapture={:?} accessibility={:?} pointer={:?} keyboard={:?} user_presence={:?}\ndiagnostics_code={}\nremediation:",
        report.release_readiness,
        report.status.backend,
        report.status.platform,
        report.status.desktop_scope,
        report.status.active_session,
        report.status.permissions.capture,
        report.status.permissions.accessibility,
        report.status.permissions.pointer_input,
        report.status.permissions.keyboard_input,
        report.status.user_presence,
        report.status.diagnostics_code,
    );
    for item in &report.status.permissions.remediation {
        println!("- {item}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::sync::Arc;

    use super::*;
    use starweaver_computer_use::{FakeComputerUseConfig, FakeComputerUseService};

    #[test]
    fn stdio_policy_grants_all_tools_without_implicit_prompts() {
        let args = Args::try_parse_from(["starweaver-computer-use-mcp", "--stdio"])
            .expect("valid stdio arguments");
        let grant = requested_tool_grant(&args);
        let policy = policy(&args, grant);

        assert_eq!(grant, ComputerToolGrant::full());
        assert!(policy.allowed_capabilities.observe);
        assert!(policy.allowed_capabilities.accessibility_snapshot);
        assert!(policy.allowed_capabilities.pointer);
        assert!(policy.allowed_capabilities.keyboard);
        assert!(!policy.permission_prompts.capture_on_open);
        assert!(!policy.permission_prompts.accessibility_on_observe);
    }

    #[test]
    fn obsolete_input_flags_remain_accepted_but_do_not_change_the_full_grant() {
        let args = Args::try_parse_from([
            "starweaver-computer-use-mcp",
            "--stdio",
            "--allow-pointer",
            "--allow-keyboard",
        ])
        .expect("legacy input flags remain compatible");

        assert_eq!(requested_tool_grant(&args), ComputerToolGrant::full());
    }

    #[test]
    fn request_permissions_is_an_exclusive_explicit_mode() {
        let args = Args::try_parse_from([
            "starweaver-computer-use-mcp",
            "--request-permissions",
            "--json",
        ])
        .expect("valid onboarding arguments");
        assert!(args.request_permissions);
        assert!(args.json);
        assert!(
            Args::try_parse_from([
                "starweaver-computer-use-mcp",
                "--doctor",
                "--request-permissions",
            ])
            .is_err()
        );
    }

    #[tokio::test]
    async fn onboarding_contract_requests_both_permissions_and_is_json_serializable() {
        let policy = ComputerUsePolicy {
            allowed_capabilities: ComputerCapabilityGrant {
                observe: true,
                accessibility_snapshot: true,
                ..ComputerCapabilityGrant::default()
            },
            ..ComputerUsePolicy::default()
        };
        let service: DynComputerUseService = Arc::new(FakeComputerUseService::new(
            policy,
            FakeComputerUseConfig::default(),
        ));

        let outcome = request_attended_permissions(&service)
            .await
            .expect("fake onboarding request");
        assert_eq!(
            outcome.requested,
            PermissionRequest {
                screen_recording: true,
                accessibility: true,
            }
        );
        let value = serde_json::to_value(outcome).expect("machine-readable outcome");
        assert_eq!(value["requested"]["screen_recording"], true);
        assert_eq!(value["requested"]["accessibility"], true);
        assert!(value.get("permissions").is_some());
        assert!(value.get("effective_capabilities").is_some());
        assert!(value.get("diagnostics_code").is_some());
    }
}
