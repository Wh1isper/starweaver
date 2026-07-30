//! Local stdio MCP process for external Computer Use harnesses.

use std::{io, process::ExitCode, time::Duration};

use clap::{ArgGroup, Parser, ValueEnum};
use rmcp::{ServiceExt as _, transport::stdio};
use serde::Serialize;
use starweaver_computer_use::{
    BoundedStdioTransport, CloseReason, ComputerCapabilityGrant, ComputerStatus, ComputerToolGrant,
    ComputerUseContractVersion, ComputerUseMcpServer, ComputerUsePolicy, DesktopSurfaceScope,
    McpResourceLimits, ToolCatalogVersion, current_desktop_service, current_desktop_tool_grant,
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

    /// Print attended permission-onboarding status and remediation.
    #[arg(long)]
    request_permissions: bool,

    /// Print package, catalog, feature, and build-target versions.
    #[arg(long)]
    version: bool,

    /// Select the current primary display or complete visible desktop.
    #[arg(long, value_enum, default_value_t = DesktopScopeArg::PrimaryDisplay)]
    desktop_scope: DesktopScopeArg,

    /// Request pointer tools in launch policy; unavailable release backends omit them.
    #[arg(long)]
    allow_pointer: bool,

    /// Request keyboard tools in launch policy; unavailable release backends omit them.
    #[arg(long)]
    allow_keyboard: bool,

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

    let requested = ComputerToolGrant {
        observe: true,
        pointer: args.allow_pointer,
        keyboard: args.allow_keyboard,
    };
    let effective = current_desktop_tool_grant(requested);
    let policy = policy(&args, effective);

    if args.stdio {
        if requested.pointer && !effective.pointer {
            eprintln!(
                "starweaver-computer-use-mcp: pointer input is not release-ready on this backend; pointer tools are omitted"
            );
        }
        if requested.keyboard && !effective.keyboard {
            eprintln!(
                "starweaver-computer-use-mcp: keyboard input is not release-ready on this backend; keyboard tools are omitted"
            );
        }
        return serve_stdio(policy, effective).await;
    }

    let service = current_desktop_service(policy);
    let status = service.status(CancellationToken::new()).await?;
    let report = DiagnosticReport {
        package_version: env!("CARGO_PKG_VERSION"),
        build_target: env!("STARWEAVER_BUILD_TARGET"),
        release_readiness: "provisional_unsigned_observe_only",
        service_contract: ComputerUseContractVersion::V1,
        tool_catalog: ToolCatalogVersion::V1,
        effective_tool_grant: effective,
        status,
    };
    if args.request_permissions {
        eprintln!(
            "This command does not synthesize input. Follow the reported remediation for the exact executable identity, then restart if macOS requires it."
        );
    }
    print_report(&report, args.json)?;
    tokio::time::timeout(
        PROCESS_SHUTDOWN_BUDGET,
        service.shutdown(CloseReason::HostShutdown),
    )
    .await
    .map_err(|_| shutdown_timeout_error())??;
    Ok(())
}

fn policy(args: &Args, effective: ComputerToolGrant) -> ComputerUsePolicy {
    ComputerUsePolicy {
        desktop_scope: args.desktop_scope.into(),
        allowed_capabilities: ComputerCapabilityGrant {
            observe: effective.observe,
            pointer: effective.pointer,
            keyboard: effective.keyboard,
            accessibility_snapshot: false,
        },
        ..ComputerUsePolicy::default()
    }
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
        "starweaver-computer-use-mcp {}\ncomputer-use-contract {}.{}\ntool-catalog {}.{}\nfeatures mcp-server,observe-only\nrelease-readiness provisional-unsigned-observe-only\ntarget {}",
        env!("CARGO_PKG_VERSION"),
        ComputerUseContractVersion::V1.major,
        ComputerUseContractVersion::V1.minor,
        ToolCatalogVersion::V1.major,
        ToolCatalogVersion::V1.minor,
        env!("STARWEAVER_BUILD_TARGET")
    );
}

fn print_report(report: &DiagnosticReport, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    println!(
        "release_readiness={}\nbackend={:?} platform={:?} scope={:?} active_session={:?}\ncapture={:?} pointer={:?} keyboard={:?} user_presence={:?}\ndiagnostics_code={}\nremediation:",
        report.release_readiness,
        report.status.backend,
        report.status.platform,
        report.status.desktop_scope,
        report.status.active_session,
        report.status.permissions.capture,
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
