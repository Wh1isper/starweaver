//! RPC-owned process-local Computer Use composition and run admission.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use starweaver_agent::{
    ComputerUseAdmissionGuard, ComputerUseToolsetPolicy, DynToolset, attach_guarded_computer_use,
    computer_use_tools,
};
use starweaver_computer_use::{
    CloseReason, ComputerCapabilityGrant, ComputerSessionBinding, ComputerToolGrant,
    ComputerToolRouter, ComputerUsePolicy, DesktopSurfaceScope, DynComputerUseService,
    PermissionPromptPolicy, current_desktop_service, current_desktop_tool_grant,
};
use starweaver_context::AgentContext;
use starweaver_core::CancellationToken;
use starweaver_session::ManagedRunTarget;

use crate::{
    RpcComputerUseConfig, RpcComputerUseDesktopScope, RpcHostError, RpcHostResult, RpcTransport,
};

/// Ephemeral initiating-caller authority derived by a transport before one run admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RpcComputerUsePrincipal {
    authority_identity: String,
    connection_id: Option<String>,
    authorization_generation: u64,
    observe: bool,
}

impl RpcComputerUsePrincipal {
    pub(crate) fn new(
        authority_identity: impl Into<String>,
        connection_id: Option<String>,
        authorization_generation: u64,
        observe: bool,
    ) -> Self {
        Self {
            authority_identity: authority_identity.into(),
            connection_id,
            authorization_generation,
            observe,
        }
    }
}

#[derive(Clone, Debug)]
struct RunAdmission {
    principal_fingerprint: String,
    connection_id: Option<String>,
    authorization_generation: u64,
    admission_generation: u64,
    expires_at: Instant,
    grant: ComputerToolGrant,
    revoked: CancellationToken,
}

struct RpcComputerUseState {
    admissions: Mutex<HashMap<ManagedRunTarget, RunAdmission>>,
    next_generation: AtomicU64,
    /// Process-start authorization evidence. Runtime profile reloads do not mutate this value.
    authorization_generation: u64,
}

/// Lifetime owner for one run-local admission. Dropping it revokes that exact generation.
pub(crate) struct RpcComputerUseRunLease {
    state: Arc<RpcComputerUseState>,
    target: ManagedRunTarget,
    admission_generation: u64,
}

impl Drop for RpcComputerUseRunLease {
    fn drop(&mut self) {
        let Ok(mut admissions) = self.state.admissions.lock() else {
            return;
        };
        if admissions
            .get(&self.target)
            .is_some_and(|admission| admission.admission_generation == self.admission_generation)
            && let Some(admission) = admissions.remove(&self.target)
        {
            admission.revoked.cancel();
        }
    }
}

/// One process-local coordinator shared by every run in the standalone RPC host.
#[derive(Clone)]
pub(crate) struct RpcComputerUseCoordinator {
    config: RpcComputerUseConfig,
    service: Option<DynComputerUseService>,
    router: Option<Arc<ComputerToolRouter>>,
    grant: ComputerToolGrant,
    state: Arc<RpcComputerUseState>,
}

impl RpcComputerUseCoordinator {
    pub(crate) fn from_config(config: RpcComputerUseConfig, authorization_generation: u64) -> Self {
        let state = Arc::new(RpcComputerUseState {
            admissions: Mutex::new(HashMap::new()),
            next_generation: AtomicU64::new(1),
            authorization_generation,
        });
        if !config.enabled {
            return Self {
                config,
                service: None,
                router: None,
                grant: ComputerToolGrant::default(),
                state,
            };
        }
        let desktop_scope = match config.desktop_scope {
            RpcComputerUseDesktopScope::PrimaryDisplay => DesktopSurfaceScope::PrimaryDisplay,
            RpcComputerUseDesktopScope::VisibleDesktop => DesktopSurfaceScope::VisibleDesktop,
        };
        let grant = current_desktop_tool_grant(ComputerToolGrant::observe_only());
        let policy = ComputerUsePolicy {
            desktop_scope,
            allowed_capabilities: ComputerCapabilityGrant {
                observe: grant.observe,
                pointer: false,
                keyboard: false,
                accessibility_snapshot: true,
            },
            permission_prompts: PermissionPromptPolicy {
                capture_on_open: true,
                accessibility_on_observe: true,
            },
            ..ComputerUsePolicy::default()
        };
        let service = current_desktop_service(policy);
        let router = Arc::new(ComputerToolRouter::new(
            service.clone(),
            ComputerSessionBinding::ServiceOwnedLazy,
            grant,
        ));
        Self {
            config,
            service: Some(service),
            router: Some(router),
            grant,
            state,
        }
    }

    pub(crate) fn toolset() -> DynToolset {
        computer_use_tools(
            ComputerToolGrant::observe_only(),
            ComputerUseToolsetPolicy::default(),
        )
    }

    pub(crate) fn principal(
        &self,
        transport: RpcTransport,
        authority_identity: &str,
        connection_id: &str,
        authorization_generation: u64,
    ) -> RpcComputerUsePrincipal {
        let observe = match transport {
            RpcTransport::Stdio => self.config.stdio_observe,
            RpcTransport::Http => self.config.http_observe,
        };
        // Stdio is a persistent principal and loses authority on connection close. Unary HTTP has
        // no persistent connection lifetime, so its credential fingerprint and startup
        // authorization generation remain the revocation boundary until the short run TTL expires.
        let connection_id = (transport == RpcTransport::Stdio).then(|| connection_id.to_string());
        RpcComputerUsePrincipal::new(
            authority_identity,
            connection_id,
            authorization_generation,
            observe,
        )
    }

    pub(crate) fn attach_run(
        &self,
        context: &mut AgentContext,
        target: &ManagedRunTarget,
        principal: Option<&RpcComputerUsePrincipal>,
        profile_grants_toolset: bool,
    ) -> RpcHostResult<Option<RpcComputerUseRunLease>> {
        let Some(principal) = principal else {
            return Ok(None);
        };
        let Some(router) = self.router.as_ref() else {
            return Ok(None);
        };
        if !profile_grants_toolset
            || !principal.observe
            || !self.grant.observe
            || principal.authorization_generation != self.state.authorization_generation
        {
            return Ok(None);
        }
        let admission_generation = self.state.next_generation.fetch_add(1, Ordering::Relaxed);
        let revoked = CancellationToken::new();
        let admission = RunAdmission {
            principal_fingerprint: principal.authority_identity.clone(),
            connection_id: principal.connection_id.clone(),
            authorization_generation: principal.authorization_generation,
            admission_generation,
            expires_at: Instant::now() + Duration::from_millis(self.config.grant_ttl_ms),
            grant: ComputerToolGrant::observe_only(),
            revoked: revoked.clone(),
        };
        let replaced = self
            .state
            .admissions
            .lock()
            .map_err(|_| {
                RpcHostError::Runtime("Computer Use admission registry unavailable".into())
            })?
            .insert(target.clone(), admission);
        if let Some(replaced) = replaced {
            replaced.revoked.cancel();
        }
        let expiry_state = Arc::downgrade(&self.state);
        let expiry_target = target.clone();
        let grant_ttl = Duration::from_millis(self.config.grant_ttl_ms);
        tokio::spawn(async move {
            tokio::time::sleep(grant_ttl).await;
            revoke_generation(&expiry_state, &expiry_target, admission_generation);
        });
        let weak_state = Arc::downgrade(&self.state);
        let guarded_target = target.clone();
        let guard = ComputerUseAdmissionGuard::with_revocation(
            move || admission_is_current(&weak_state, &guarded_target, admission_generation),
            revoked,
        );
        if let Err(error) = attach_guarded_computer_use(
            context,
            router.clone(),
            ComputerToolGrant::observe_only(),
            guard,
        ) {
            self.revoke_target(target);
            return Err(RpcHostError::Invalid(error.to_string()));
        }
        Ok(Some(RpcComputerUseRunLease {
            state: Arc::clone(&self.state),
            target: target.clone(),
            admission_generation,
        }))
    }

    pub(crate) fn revoke_connection(&self, authority_identity: &str, connection_id: &str) {
        let Ok(mut admissions) = self.state.admissions.lock() else {
            return;
        };
        admissions.retain(|_, admission| {
            let keep = admission.principal_fingerprint != authority_identity
                || admission.connection_id.as_deref() != Some(connection_id);
            if !keep {
                admission.revoked.cancel();
            }
            keep
        });
    }

    pub(crate) fn revoke_target(&self, target: &ManagedRunTarget) {
        if let Ok(mut admissions) = self.state.admissions.lock()
            && let Some(admission) = admissions.remove(target)
        {
            admission.revoked.cancel();
        }
    }

    #[cfg(test)]
    pub(crate) fn with_service_for_tests(service: DynComputerUseService) -> Self {
        Self {
            config: RpcComputerUseConfig::default(),
            service: Some(service),
            router: None,
            grant: ComputerToolGrant::default(),
            state: Arc::new(RpcComputerUseState {
                admissions: Mutex::new(HashMap::new()),
                next_generation: AtomicU64::new(1),
                authorization_generation: 1,
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn active_admission_count(&self) -> usize {
        self.state
            .admissions
            .lock()
            .map_or(0, |admissions| admissions.len())
    }

    pub(crate) fn revoke_all(&self) {
        if let Ok(mut admissions) = self.state.admissions.lock() {
            for admission in admissions.drain().map(|(_, admission)| admission) {
                admission.revoked.cancel();
            }
        }
    }

    pub(crate) async fn shutdown(&self, deadline: tokio::time::Instant) -> RpcHostResult<()> {
        self.revoke_all();
        if let Some(service) = self.service.as_ref() {
            tokio::time::timeout_at(deadline, service.shutdown(CloseReason::HostShutdown))
                .await
                .map_err(|_| {
                    RpcHostError::Runtime(
                        "Computer Use shutdown exceeded the RPC shared shutdown deadline"
                            .to_string(),
                    )
                })?
                .map_err(|error| RpcHostError::Runtime(error.to_string()))?;
        }
        Ok(())
    }
}

fn revoke_generation(
    state: &Weak<RpcComputerUseState>,
    target: &ManagedRunTarget,
    admission_generation: u64,
) {
    let Some(state) = state.upgrade() else {
        return;
    };
    let Ok(mut admissions) = state.admissions.lock() else {
        return;
    };
    if admissions
        .get(target)
        .is_some_and(|admission| admission.admission_generation == admission_generation)
        && let Some(admission) = admissions.remove(target)
    {
        admission.revoked.cancel();
    }
}

fn admission_is_current(
    state: &Weak<RpcComputerUseState>,
    target: &ManagedRunTarget,
    admission_generation: u64,
) -> bool {
    let Some(state) = state.upgrade() else {
        return false;
    };
    let Ok(mut admissions) = state.admissions.lock() else {
        return false;
    };
    let Some(admission) = admissions.get(target) else {
        return false;
    };
    if admission.admission_generation != admission_generation {
        return false;
    }
    if Instant::now() >= admission.expires_at {
        if let Some(admission) = admissions.remove(target) {
            admission.revoked.cancel();
        }
        return false;
    }
    admission.grant.observe
        && !admission.principal_fingerprint.is_empty()
        && admission.authorization_generation == state.authorization_generation
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use starweaver_core::{RunId, SessionId};

    fn target() -> ManagedRunTarget {
        ManagedRunTarget::new("local", SessionId::new(), RunId::new())
    }

    fn coordinator(config: RpcComputerUseConfig) -> RpcComputerUseCoordinator {
        RpcComputerUseCoordinator::from_config(config, 1)
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn standalone_zero_authorization_generation_is_valid_startup_evidence() {
        let coordinator = RpcComputerUseCoordinator::from_config(
            RpcComputerUseConfig {
                enabled: true,
                stdio_observe: true,
                ..RpcComputerUseConfig::default()
            },
            0,
        );
        let target = target();
        let principal = coordinator.principal(
            RpcTransport::Stdio,
            "local-stdio",
            "standalone-connection",
            0,
        );
        let mut context = AgentContext::default();
        let lease = coordinator
            .attach_run(&mut context, &target, Some(&principal), true)
            .expect("standalone admission should be evaluated")
            .expect("standalone generation zero should be admitted");
        let admission_generation =
            coordinator.state.admissions.lock().expect("registry")[&target].admission_generation;
        assert!(admission_is_current(
            &Arc::downgrade(&coordinator.state),
            &target,
            admission_generation,
        ));
        drop(lease);
    }

    #[test]
    fn enabled_composition_allows_accessibility_prompts_without_widening_principals() {
        let coordinator = coordinator(RpcComputerUseConfig {
            enabled: true,
            ..RpcComputerUseConfig::default()
        });
        let policy = coordinator
            .service
            .as_ref()
            .expect("enabled service")
            .policy();

        assert!(policy.allowed_capabilities.accessibility_snapshot);
        assert!(policy.permission_prompts.capture_on_open);
        assert!(policy.permission_prompts.accessibility_on_observe);
        assert!(!coordinator.grant.pointer);
        assert!(!coordinator.grant.keyboard);
        assert!(
            !coordinator
                .principal(RpcTransport::Stdio, "stdio", "connection", 1)
                .observe
        );
        assert!(
            !coordinator
                .principal(RpcTransport::Http, "http", "connection", 1)
                .observe
        );
    }

    #[test]
    fn server_profile_and_principal_are_independent_required_gates() {
        let target = target();
        let mut context = AgentContext::default();
        let disabled = coordinator(RpcComputerUseConfig {
            enabled: false,
            stdio_observe: true,
            ..RpcComputerUseConfig::default()
        });
        let principal =
            disabled.principal(RpcTransport::Stdio, "local-stdio", "connection-disabled", 1);
        assert!(
            disabled
                .attach_run(&mut context, &target, Some(&principal), true)
                .unwrap()
                .is_none()
        );

        let enabled = coordinator(RpcComputerUseConfig {
            enabled: true,
            stdio_observe: true,
            ..RpcComputerUseConfig::default()
        });
        let principal =
            enabled.principal(RpcTransport::Stdio, "local-stdio", "connection-enabled", 1);
        assert!(
            enabled
                .attach_run(&mut context, &target, Some(&principal), false)
                .unwrap()
                .is_none()
        );
        let denied_principal = RpcComputerUsePrincipal::new(
            "local-stdio",
            Some("connection-enabled".to_string()),
            1,
            false,
        );
        assert!(
            enabled
                .attach_run(&mut context, &target, Some(&denied_principal), true)
                .unwrap()
                .is_none()
        );
        assert_eq!(enabled.active_admission_count(), 0);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn complete_intersection_attaches_observe_only_and_lease_drop_revokes() {
        let coordinator = coordinator(RpcComputerUseConfig {
            enabled: true,
            stdio_observe: true,
            ..RpcComputerUseConfig::default()
        });
        let target = target();
        let principal =
            coordinator.principal(RpcTransport::Stdio, "local-stdio", "connection-observe", 1);
        let mut context = AgentContext::default();
        let lease = coordinator
            .attach_run(&mut context, &target, Some(&principal), true)
            .unwrap()
            .expect("macOS observe admission");
        assert!(
            context
                .named_dependency::<starweaver_agent::ComputerObserveHandle>(
                    starweaver_agent::COMPUTER_OBSERVE_CAPABILITY,
                )
                .is_some()
        );
        assert!(
            context
                .named_dependency::<starweaver_agent::ComputerPointerHandle>(
                    starweaver_agent::COMPUTER_POINTER_CAPABILITY,
                )
                .is_none()
        );
        assert_eq!(coordinator.active_admission_count(), 1);
        drop(lease);
        assert_eq!(coordinator.active_admission_count(), 0);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn ttl_task_revokes_exact_active_generation() {
        let coordinator = coordinator(RpcComputerUseConfig {
            enabled: true,
            grant_ttl_ms: 5,
            stdio_observe: true,
            ..RpcComputerUseConfig::default()
        });
        let target = target();
        let principal =
            coordinator.principal(RpcTransport::Stdio, "local-stdio", "connection-ttl", 1);
        let mut context = AgentContext::default();
        let lease = coordinator
            .attach_run(&mut context, &target, Some(&principal), true)
            .unwrap()
            .expect("macOS observe admission");
        let revoked = coordinator.state.admissions.lock().unwrap()[&target]
            .revoked
            .clone();
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(revoked.is_cancelled());
        assert_eq!(coordinator.active_admission_count(), 0);
        drop(lease);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn replacement_admission_revokes_the_old_generation() {
        let coordinator = coordinator(RpcComputerUseConfig {
            enabled: true,
            stdio_observe: true,
            ..RpcComputerUseConfig::default()
        });
        let target = target();
        let principal =
            coordinator.principal(RpcTransport::Stdio, "local-stdio", "connection-replace", 1);
        let mut first_context = AgentContext::default();
        let first_lease = coordinator
            .attach_run(&mut first_context, &target, Some(&principal), true)
            .unwrap()
            .expect("first macOS observe admission");
        let first_revoked = coordinator.state.admissions.lock().unwrap()[&target]
            .revoked
            .clone();

        let mut second_context = AgentContext::default();
        let second_lease = coordinator
            .attach_run(&mut second_context, &target, Some(&principal), true)
            .unwrap()
            .expect("replacement macOS observe admission");

        assert!(first_revoked.is_cancelled());
        assert_eq!(coordinator.active_admission_count(), 1);
        drop(first_lease);
        assert_eq!(coordinator.active_admission_count(), 1);
        drop(second_lease);
        assert_eq!(coordinator.active_admission_count(), 0);
    }

    #[test]
    fn connection_revoke_and_wrong_authority_fail_closed() {
        let config = RpcComputerUseConfig {
            enabled: true,
            grant_ttl_ms: 1,
            stdio_observe: true,
            ..RpcComputerUseConfig::default()
        };
        let coordinator = coordinator(config);
        let target = target();
        let principal =
            coordinator.principal(RpcTransport::Stdio, "local-stdio", "connection-test", 1);
        let admission_generation = coordinator
            .state
            .next_generation
            .fetch_add(1, Ordering::Relaxed);
        coordinator.state.admissions.lock().unwrap().insert(
            target.clone(),
            RunAdmission {
                principal_fingerprint: principal.authority_identity.clone(),
                connection_id: principal.connection_id,
                authorization_generation: 1,
                admission_generation,
                expires_at: Instant::now() + Duration::from_secs(1),
                grant: ComputerToolGrant::observe_only(),
                revoked: CancellationToken::new(),
            },
        );
        assert!(admission_is_current(
            &Arc::downgrade(&coordinator.state),
            &target,
            admission_generation
        ));
        coordinator.revoke_connection("wrong-authority", "connection-test");
        assert!(admission_is_current(
            &Arc::downgrade(&coordinator.state),
            &target,
            admission_generation
        ));
        coordinator.revoke_connection("local-stdio", "connection-test");
        assert!(!admission_is_current(
            &Arc::downgrade(&coordinator.state),
            &target,
            admission_generation
        ));
    }

    #[test]
    fn expired_admission_is_removed() {
        let coordinator = coordinator(RpcComputerUseConfig::default());
        let target = target();
        let generation = 7;
        coordinator.state.admissions.lock().unwrap().insert(
            target.clone(),
            RunAdmission {
                principal_fingerprint: "principal".to_string(),
                connection_id: None,
                authorization_generation: 1,
                admission_generation: generation,
                expires_at: Instant::now(),
                grant: ComputerToolGrant::observe_only(),
                revoked: CancellationToken::new(),
            },
        );
        assert!(!admission_is_current(
            &Arc::downgrade(&coordinator.state),
            &target,
            generation
        ));
        assert_eq!(coordinator.active_admission_count(), 0);
    }
}
