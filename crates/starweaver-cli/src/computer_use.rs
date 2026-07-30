//! CLI-owned process-local Computer Use composition.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use starweaver_agent::{
    ComputerUseToolsetPolicy, DynToolset, attach_computer_use, computer_use_tools,
};
use starweaver_computer_use::{
    CloseReason, ComputerSessionBinding, ComputerToolGrant, ComputerToolRouter, ComputerUsePolicy,
    DesktopSurfaceScope, DynComputerUseService, current_desktop_service,
    current_desktop_tool_grant,
};
use starweaver_context::AgentContext;

use crate::{CliError, CliResult, config::CliComputerUseConfig};

const CLI_COMPUTER_USE_SHUTDOWN_BUDGET: Duration = Duration::from_secs(5);

#[derive(Clone)]
enum ShutdownState {
    Open,
    Closed(Result<(), String>),
}

/// One process-local CLI coordinator shared across runs in this CLI service.
#[derive(Clone)]
pub(crate) struct CliComputerUseCoordinator {
    service: DynComputerUseService,
    router: Arc<ComputerToolRouter>,
    grant: ComputerToolGrant,
    shutdown: Arc<Mutex<ShutdownState>>,
}

impl CliComputerUseCoordinator {
    pub(crate) fn from_config(config: &CliComputerUseConfig) -> CliResult<Option<Self>> {
        if !config.enabled {
            return Ok(None);
        }
        let desktop_scope = match config.desktop_scope.as_str() {
            "primary_display" => DesktopSurfaceScope::PrimaryDisplay,
            "visible_desktop" => DesktopSurfaceScope::VisibleDesktop,
            value => {
                return Err(CliError::Config(format!(
                    "computer_use.desktop_scope must be primary_display or visible_desktop, got {value}"
                )));
            }
        };
        let requested = ComputerToolGrant::observe_only();
        let grant = current_desktop_tool_grant(requested);
        let policy = ComputerUsePolicy {
            desktop_scope,
            allowed_capabilities: starweaver_computer_use::ComputerCapabilityGrant {
                observe: grant.observe,
                pointer: false,
                keyboard: false,
                accessibility_snapshot: false,
            },
            ..ComputerUsePolicy::default()
        };
        let service = current_desktop_service(policy);
        Ok(Some(Self::new(service, grant)))
    }

    fn new(service: DynComputerUseService, grant: ComputerToolGrant) -> Self {
        let router = Arc::new(ComputerToolRouter::new(
            service.clone(),
            ComputerSessionBinding::ServiceOwnedLazy,
            grant,
        ));
        Self {
            service,
            router,
            grant,
            shutdown: Arc::new(Mutex::new(ShutdownState::Open)),
        }
    }

    pub(crate) fn toolset(&self) -> DynToolset {
        computer_use_tools(self.grant, ComputerUseToolsetPolicy::default())
    }

    pub(crate) fn configure_context(&self, context: &mut AgentContext) -> CliResult<()> {
        attach_computer_use(context, self.router.clone(), self.grant)
            .map_err(|error| CliError::Config(error.to_string()))
    }

    /// Stop admission and confirm native cleanup within the default process budget.
    pub(crate) fn shutdown(&self) -> CliResult<()> {
        self.shutdown_with_timeout(CLI_COMPUTER_USE_SHUTDOWN_BUDGET)
    }

    /// Stop admission within a caller-owned total deadline. Every clone observes
    /// the first cleanup result, including a mandatory failure.
    pub(crate) fn shutdown_with_timeout(&self, timeout: Duration) -> CliResult<()> {
        let mut state = self.shutdown.lock().map_err(|error| {
            CliError::Run(format!("computer use shutdown state poisoned: {error}"))
        })?;
        if let ShutdownState::Closed(result) = &*state {
            return result
                .clone()
                .map_err(|error| computer_use_cleanup_error(&error));
        }

        let deadline = Instant::now() + timeout;
        let result = self.shutdown_until(deadline);
        *state = ShutdownState::Closed(result.clone());
        drop(state);
        result.map_err(|error| computer_use_cleanup_error(&error))
    }

    fn shutdown_until(&self, deadline: Instant) -> Result<(), String> {
        let service = self.service.clone();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("starweaver-computer-use-shutdown".to_string())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| format!("failed to start cleanup runtime: {error}"));
                let result = match runtime {
                    Ok(runtime) => {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        let result = if remaining.is_zero() {
                            Err("cleanup deadline elapsed before service shutdown".to_string())
                        } else {
                            runtime.block_on(async move {
                                tokio::time::timeout(
                                    remaining,
                                    service.shutdown(CloseReason::HostShutdown),
                                )
                                .await
                                .map_err(|_| {
                                    "cleanup exceeded the CLI shutdown deadline".to_string()
                                })?
                                .map(|_| ())
                                .map_err(|error| error.to_string())
                            })
                        };
                        runtime
                            .shutdown_timeout(deadline.saturating_duration_since(Instant::now()));
                        result
                    }
                    Err(error) => Err(error),
                };
                let _ = sender.send(result);
            })
            .map_err(|error| format!("failed to start cleanup worker: {error}"))?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(remaining) {
            Ok(result) => {
                worker
                    .join()
                    .map_err(|_| "cleanup worker panicked".to_string())?;
                result
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                Err("cleanup exceeded the CLI shutdown deadline".to_string())
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let _ = worker.join();
                Err("cleanup worker exited without a result".to_string())
            }
        }
    }
}

fn computer_use_cleanup_error(error: &str) -> CliError {
    CliError::Run(format!(
        "mandatory Computer Use cleanup could not be confirmed: {error}"
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use starweaver_computer_use::{
        FakeComputerUseConfig, FakeComputerUseService, InputCleanupStatus,
    };
    use starweaver_core::CancellationToken;

    #[test]
    fn observe_only_coordinator_retains_service_and_confirms_shutdown_once() {
        let service: DynComputerUseService = Arc::new(FakeComputerUseService::new(
            ComputerUsePolicy::default(),
            FakeComputerUseConfig::default(),
        ));
        let coordinator =
            CliComputerUseCoordinator::new(service.clone(), ComputerToolGrant::observe_only());
        assert!(coordinator.grant.observe);
        assert!(!coordinator.grant.pointer);
        assert!(!coordinator.grant.keyboard);

        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime
            .block_on(service.open_current_desktop(CancellationToken::new()))
            .unwrap();
        drop(runtime);

        let coordinator_clone = coordinator.clone();
        coordinator.shutdown().unwrap();
        coordinator_clone.shutdown().unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn coordinated_shutdown_is_safe_from_an_existing_runtime() {
        let service: DynComputerUseService = Arc::new(FakeComputerUseService::new(
            ComputerUsePolicy::default(),
            FakeComputerUseConfig::default(),
        ));
        let coordinator =
            CliComputerUseCoordinator::new(service.clone(), ComputerToolGrant::observe_only());
        service
            .open_current_desktop(CancellationToken::new())
            .await
            .unwrap();

        coordinator.shutdown().unwrap();
    }

    #[test]
    fn mandatory_cleanup_failure_is_sticky_and_propagated() {
        let service: DynComputerUseService = Arc::new(FakeComputerUseService::new(
            ComputerUsePolicy::default(),
            FakeComputerUseConfig {
                close_cleanup: InputCleanupStatus::Failed,
                ..FakeComputerUseConfig::default()
            },
        ));
        let coordinator =
            CliComputerUseCoordinator::new(service.clone(), ComputerToolGrant::observe_only());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime
            .block_on(service.open_current_desktop(CancellationToken::new()))
            .unwrap();
        drop(runtime);

        let coordinator_clone = coordinator.clone();
        let first = coordinator.shutdown().unwrap_err().to_string();
        let second = coordinator_clone.shutdown().unwrap_err().to_string();
        assert!(first.contains("mandatory Computer Use cleanup"));
        assert_eq!(first, second);
    }
}
