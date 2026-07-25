//! Generated exhaustive server boundary.

use super::{errors::*, types::*};
use async_trait::async_trait;
#[async_trait]
pub trait HostServer: Send + Sync {
    type Context: Send + Sync;
    async fn approval_decide(
        &self,
        context: &Self::Context,
        params: ApprovalDecideParams,
    ) -> Result<ApprovalDecideResult, ApprovalDecideError>;
    async fn approval_list(
        &self,
        context: &Self::Context,
        params: InteractionListParams,
    ) -> Result<ApprovalListResult, ApprovalListError>;
    async fn approval_show(
        &self,
        context: &Self::Context,
        params: ApprovalShowParams,
    ) -> Result<ApprovalShowResult, ApprovalShowError>;
    async fn catalog_list(
        &self,
        context: &Self::Context,
        params: CatalogListParams,
    ) -> Result<CatalogListResult, CatalogListError>;
    async fn clarification_list(
        &self,
        context: &Self::Context,
        params: InteractionListParams,
    ) -> Result<ClarificationListResult, ClarificationListError>;
    async fn clarification_resolve(
        &self,
        context: &Self::Context,
        params: ClarificationResolveParams,
    ) -> Result<ClarificationResolveResult, ClarificationResolveError>;
    async fn clarification_show(
        &self,
        context: &Self::Context,
        params: ClarificationShowParams,
    ) -> Result<ClarificationShowResult, ClarificationShowError>;
    async fn config_activate(
        &self,
        context: &Self::Context,
        params: ConfigActivateParams,
    ) -> Result<ConfigActivateResult, ConfigActivateError>;
    async fn config_discard(
        &self,
        context: &Self::Context,
        params: ConfigDiscardParams,
    ) -> Result<ConfigDiscardResult, ConfigDiscardError>;
    async fn config_get(
        &self,
        context: &Self::Context,
        params: ConfigGetParams,
    ) -> Result<ConfigGetResult, ConfigGetError>;
    async fn config_reload(
        &self,
        context: &Self::Context,
        params: ConfigReloadParams,
    ) -> Result<ConfigReloadResult, ConfigReloadError>;
    async fn config_update(
        &self,
        context: &Self::Context,
        params: ConfigUpdateParams,
    ) -> Result<ConfigUpdateResult, ConfigUpdateError>;
    async fn config_validate(
        &self,
        context: &Self::Context,
        params: ConfigValidateParams,
    ) -> Result<ConfigValidateResult, ConfigValidateError>;
    async fn deferred_complete(
        &self,
        context: &Self::Context,
        params: DeferredCompleteParams,
    ) -> Result<DeferredCompleteResult, DeferredCompleteError>;
    async fn deferred_fail(
        &self,
        context: &Self::Context,
        params: DeferredFailParams,
    ) -> Result<DeferredFailResult, DeferredFailError>;
    async fn deferred_list(
        &self,
        context: &Self::Context,
        params: InteractionListParams,
    ) -> Result<DeferredListResult, DeferredListError>;
    async fn deferred_show(
        &self,
        context: &Self::Context,
        params: DeferredShowParams,
    ) -> Result<DeferredShowResult, DeferredShowError>;
    async fn diagnostics_get(
        &self,
        context: &Self::Context,
        params: DiagnosticsGetParams,
    ) -> Result<DiagnosticsGetResult, DiagnosticsGetError>;
    async fn environment_attach(
        &self,
        context: &Self::Context,
        params: EnvironmentAttachParams,
    ) -> Result<EnvironmentAttachResult, EnvironmentAttachError>;
    async fn environment_detach(
        &self,
        context: &Self::Context,
        params: EnvironmentDetachParams,
    ) -> Result<EnvironmentDetachResult, EnvironmentDetachError>;
    async fn environment_health(
        &self,
        context: &Self::Context,
        params: EnvironmentHealthParams,
    ) -> Result<EnvironmentHealthResult, EnvironmentHealthError>;
    async fn environment_list(
        &self,
        context: &Self::Context,
        params: EnvironmentListParams,
    ) -> Result<EnvironmentListResult, EnvironmentListError>;
    async fn environment_mount(
        &self,
        context: &Self::Context,
        params: EnvironmentMountParams,
    ) -> Result<EnvironmentMountResult, EnvironmentMountError>;
    async fn environment_mounts_list(
        &self,
        context: &Self::Context,
        params: EnvironmentMountListParams,
    ) -> Result<EnvironmentMountListResult, EnvironmentMountsListError>;
    async fn environment_unmount(
        &self,
        context: &Self::Context,
        params: EnvironmentUnmountParams,
    ) -> Result<EnvironmentUnmountResult, EnvironmentUnmountError>;
    async fn events_replay(
        &self,
        context: &Self::Context,
        params: EventsReplayParams,
    ) -> Result<EventsReplayResult, EventsReplayError>;
    async fn events_subscribe(
        &self,
        context: &Self::Context,
        params: EventsSubscribeParams,
    ) -> Result<EventsSubscribeResult, EventsSubscribeError>;
    async fn events_unsubscribe(
        &self,
        context: &Self::Context,
        params: EventsUnsubscribeParams,
    ) -> Result<EventsUnsubscribeResult, EventsUnsubscribeError>;
    async fn initialize(
        &self,
        context: &Self::Context,
        params: InitializeParams,
    ) -> Result<InitializeResult, InitializeError>;
    async fn model_select(
        &self,
        context: &Self::Context,
        params: ModelSelectParams,
    ) -> Result<ModelSelectResult, ModelSelectError>;
    async fn model_selection_get(
        &self,
        context: &Self::Context,
        params: ModelSelectionGetParams,
    ) -> Result<ModelSelectionGetResult, ModelSelectionGetError>;
    async fn profile_get(
        &self,
        context: &Self::Context,
        params: ProfileGetParams,
    ) -> Result<ProfileGetResult, ProfileGetError>;
    async fn run_interrupt(
        &self,
        context: &Self::Context,
        params: RunInterruptParams,
    ) -> Result<RunInterruptResult, RunInterruptError>;
    async fn run_list(
        &self,
        context: &Self::Context,
        params: RunListParams,
    ) -> Result<RunListResult, RunListError>;
    async fn run_resume(
        &self,
        context: &Self::Context,
        params: RunResumeParams,
    ) -> Result<RunResumeResult, RunResumeError>;
    async fn run_start(
        &self,
        context: &Self::Context,
        params: RunStartParams,
    ) -> Result<RunStartResult, RunStartError>;
    async fn run_status(
        &self,
        context: &Self::Context,
        params: RunStatusParams,
    ) -> Result<RunStatusResult, RunStatusError>;
    async fn run_steer(
        &self,
        context: &Self::Context,
        params: RunSteerParams,
    ) -> Result<RunSteerResult, RunSteerError>;
    async fn session_create(
        &self,
        context: &Self::Context,
        params: SessionCreateParams,
    ) -> Result<SessionCreateResult, SessionCreateError>;
    async fn session_delete(
        &self,
        context: &Self::Context,
        params: SessionDeleteParams,
    ) -> Result<SessionDeleteResult, SessionDeleteError>;
    async fn session_fork(
        &self,
        context: &Self::Context,
        params: SessionForkParams,
    ) -> Result<SessionForkResult, SessionForkError>;
    async fn session_get(
        &self,
        context: &Self::Context,
        params: SessionGetParams,
    ) -> Result<SessionGetResult, SessionGetError>;
    async fn session_list(
        &self,
        context: &Self::Context,
        params: SessionListParams,
    ) -> Result<SessionListResult, SessionListError>;
    async fn session_search(
        &self,
        context: &Self::Context,
        params: SessionSearchParams,
    ) -> Result<SessionSearchResult, SessionSearchError>;
    async fn shutdown(
        &self,
        context: &Self::Context,
        params: ShutdownParams,
    ) -> Result<ShutdownResult, ShutdownError>;
    async fn workspace_list(
        &self,
        context: &Self::Context,
        params: WorkspaceListParams,
    ) -> Result<WorkspaceListResult, WorkspaceListError>;
    async fn workspace_register(
        &self,
        context: &Self::Context,
        params: WorkspaceRegisterParams,
    ) -> Result<WorkspaceRegisterResult, WorkspaceRegisterError>;
    async fn workspace_remove(
        &self,
        context: &Self::Context,
        params: WorkspaceRemoveParams,
    ) -> Result<WorkspaceRemoveResult, WorkspaceRemoveError>;
}
