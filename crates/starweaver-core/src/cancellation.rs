//! Shared cooperative cancellation token.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Debug)]
struct CancellationInner {
    cancelled: AtomicBool,
    notify: tokio::sync::Notify,
}

impl Default for CancellationInner {
    fn default() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
        }
    }
}

/// Shared cooperative cancellation token for runs, model requests, and tools.
#[derive(Clone, Debug)]
pub struct CancellationToken {
    inner: Arc<CancellationInner>,
}

impl CancellationToken {
    /// Create an uncancelled token.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(CancellationInner::default()),
        }
    }

    /// Request cancellation.
    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::SeqCst) {
            self.inner.notify.notify_waiters();
        }
    }

    /// Return whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Wait until cancellation is requested.
    pub async fn cancelled(&self) {
        while !self.is_cancelled() {
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for CancellationToken {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner) || self.is_cancelled() == other.is_cancelled()
    }
}

impl Eq for CancellationToken {}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::CancellationToken;

    #[tokio::test]
    async fn cancellation_token_notifies_waiters() {
        let token = CancellationToken::new();
        let waiter = token.clone();
        let task = tokio::spawn(async move {
            waiter.cancelled().await;
            waiter.is_cancelled()
        });

        assert!(!token.is_cancelled());
        token.cancel();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), task).await,
            Ok(Ok(true))
        ));
    }
}
