use std::{fmt, sync::Arc};

use serde_json::Value;
use starweaver_computer_use::{
    COMPUTER_CLICK_TOOL, COMPUTER_DRAG_TOOL, COMPUTER_MOVE_POINTER_TOOL, COMPUTER_OBSERVE_TOOL,
    COMPUTER_PRESS_KEYS_TOOL, COMPUTER_SCROLL_TOOL, COMPUTER_STATUS_TOOL, COMPUTER_TYPE_TEXT_TOOL,
    ComputerToolCallResult, ComputerToolInvocation, ComputerToolRouter,
};
use starweaver_core::CancellationToken;

/// Stable named host capability for Computer Use observation.
pub const COMPUTER_OBSERVE_CAPABILITY: &str = "starweaver.computer_use.observe";
/// Stable named host capability for Computer Use pointer input.
pub const COMPUTER_POINTER_CAPABILITY: &str = "starweaver.computer_use.pointer";
/// Stable named host capability for Computer Use keyboard input.
pub const COMPUTER_KEYBOARD_CAPABILITY: &str = "starweaver.computer_use.keyboard";

/// Process-local, dynamically checked admission for Computer Use calls.
///
/// Products use this guard to make expiring and revocable run authority observable by an already
/// constructed runtime. The callback must fail closed when its authority registry is unavailable.
#[derive(Clone)]
pub struct ComputerUseAdmissionGuard {
    permits: Arc<dyn Fn() -> bool + Send + Sync>,
    revoked: CancellationToken,
}

impl ComputerUseAdmissionGuard {
    /// Build a guard from one process-local admission callback.
    #[must_use]
    pub fn new(permits: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self {
            permits: Arc::new(permits),
            revoked: CancellationToken::new(),
        }
    }

    /// Build a dynamically checked guard with cooperative in-flight revocation.
    #[must_use]
    pub fn with_revocation(
        permits: impl Fn() -> bool + Send + Sync + 'static,
        revoked: CancellationToken,
    ) -> Self {
        Self {
            permits: Arc::new(permits),
            revoked,
        }
    }

    /// Build the non-expiring guard used by direct, already-authorized in-process composition.
    #[must_use]
    pub fn allow_all() -> Self {
        Self::new(|| true)
    }

    fn permits(&self) -> bool {
        !self.revoked.is_cancelled() && (self.permits)()
    }

    async fn revoked(&self) {
        self.revoked.cancelled().await;
    }
}

impl fmt::Debug for ComputerUseAdmissionGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ComputerUseAdmissionGuard")
            .field("revoked", &self.revoked.is_cancelled())
            .finish_non_exhaustive()
    }
}

async fn guarded_call(
    admission: &ComputerUseAdmissionGuard,
    router: &ComputerToolRouter,
    invocation: ComputerToolInvocation,
    name: &str,
    arguments: Value,
    caller_cancel: CancellationToken,
) -> ComputerToolCallResult {
    if !admission.permits() {
        return ComputerToolCallResult::admission_denied(
            name,
            "process-local Computer Use admission is absent, expired, or revoked",
        );
    }
    let operation_cancel = CancellationToken::new();
    let dispatch = router.call(invocation, name, arguments, operation_cancel.clone());
    tokio::pin!(dispatch);
    tokio::select! {
        result = &mut dispatch => result,
        () = caller_cancel.cancelled() => {
            operation_cancel.cancel();
            dispatch.await
        }
        () = admission.revoked() => {
            operation_cancel.cancel();
            let _ = dispatch.await;
            ComputerToolCallResult::admission_denied(
                name,
                "process-local Computer Use admission was revoked during execution",
            )
        }
    }
}

/// Method-limited process-local observation handle.
#[derive(Clone)]
pub struct ComputerObserveHandle {
    router: Arc<ComputerToolRouter>,
    admission: ComputerUseAdmissionGuard,
}

impl ComputerObserveHandle {
    #[must_use]
    pub(crate) const fn new(
        router: Arc<ComputerToolRouter>,
        admission: ComputerUseAdmissionGuard,
    ) -> Self {
        Self { router, admission }
    }

    /// Dispatch canonical non-effectful status.
    pub async fn status(
        &self,
        invocation: ComputerToolInvocation,
        arguments: Value,
        cancel: CancellationToken,
    ) -> ComputerToolCallResult {
        guarded_call(
            &self.admission,
            &self.router,
            invocation,
            COMPUTER_STATUS_TOOL,
            arguments,
            cancel,
        )
        .await
    }

    /// Dispatch canonical desktop observation.
    pub async fn observe(
        &self,
        invocation: ComputerToolInvocation,
        arguments: Value,
        cancel: CancellationToken,
    ) -> ComputerToolCallResult {
        guarded_call(
            &self.admission,
            &self.router,
            invocation,
            COMPUTER_OBSERVE_TOOL,
            arguments,
            cancel,
        )
        .await
    }
}

/// Method-limited process-local pointer-input handle.
#[derive(Clone)]
pub struct ComputerPointerHandle {
    router: Arc<ComputerToolRouter>,
    admission: ComputerUseAdmissionGuard,
}

impl ComputerPointerHandle {
    #[must_use]
    pub(crate) const fn new(
        router: Arc<ComputerToolRouter>,
        admission: ComputerUseAdmissionGuard,
    ) -> Self {
        Self { router, admission }
    }

    pub(crate) async fn call(
        &self,
        invocation: ComputerToolInvocation,
        name: &str,
        arguments: Value,
        cancel: CancellationToken,
    ) -> ComputerToolCallResult {
        debug_assert!(matches!(
            name,
            COMPUTER_CLICK_TOOL
                | COMPUTER_MOVE_POINTER_TOOL
                | COMPUTER_DRAG_TOOL
                | COMPUTER_SCROLL_TOOL
        ));
        guarded_call(
            &self.admission,
            &self.router,
            invocation,
            name,
            arguments,
            cancel,
        )
        .await
    }
}

/// Method-limited process-local keyboard-input handle.
#[derive(Clone)]
pub struct ComputerKeyboardHandle {
    router: Arc<ComputerToolRouter>,
    admission: ComputerUseAdmissionGuard,
}

impl ComputerKeyboardHandle {
    #[must_use]
    pub(crate) const fn new(
        router: Arc<ComputerToolRouter>,
        admission: ComputerUseAdmissionGuard,
    ) -> Self {
        Self { router, admission }
    }

    pub(crate) async fn call(
        &self,
        invocation: ComputerToolInvocation,
        name: &str,
        arguments: Value,
        cancel: CancellationToken,
    ) -> ComputerToolCallResult {
        debug_assert!(matches!(
            name,
            COMPUTER_TYPE_TEXT_TOOL | COMPUTER_PRESS_KEYS_TOOL
        ));
        guarded_call(
            &self.admission,
            &self.router,
            invocation,
            name,
            arguments,
            cancel,
        )
        .await
    }
}
