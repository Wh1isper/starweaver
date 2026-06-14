use std::sync::atomic::{AtomicBool, Ordering};

static ALLOW_REAL_MODEL_REQUESTS: AtomicBool = AtomicBool::new(true);

/// Return whether production model requests are globally allowed.
#[must_use]
pub fn allow_real_model_requests() -> bool {
    ALLOW_REAL_MODEL_REQUESTS.load(Ordering::SeqCst)
}

/// Set whether production model requests are globally allowed.
pub fn set_allow_real_model_requests(allow: bool) {
    ALLOW_REAL_MODEL_REQUESTS.store(allow, Ordering::SeqCst);
}

/// Scoped guard that restores the previous production-request setting when dropped.
#[derive(Debug)]
pub struct RealModelRequestGuard {
    previous: bool,
}

impl RealModelRequestGuard {
    /// Set the production-request setting for this scope.
    #[must_use]
    pub fn set(allow: bool) -> Self {
        let previous = ALLOW_REAL_MODEL_REQUESTS.swap(allow, Ordering::SeqCst);
        Self { previous }
    }
}

impl Drop for RealModelRequestGuard {
    fn drop(&mut self) {
        ALLOW_REAL_MODEL_REQUESTS.store(self.previous, Ordering::SeqCst);
    }
}

/// Block production model requests until the returned guard is dropped.
#[must_use]
pub fn block_real_model_requests() -> RealModelRequestGuard {
    RealModelRequestGuard::set(false)
}

/// Allow production model requests until the returned guard is dropped.
#[must_use]
pub fn allow_real_model_requests_guard() -> RealModelRequestGuard {
    RealModelRequestGuard::set(true)
}
