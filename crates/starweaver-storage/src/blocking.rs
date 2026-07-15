//! Blocking-task boundary for synchronous SQLite work.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

/// Tracks blocking operations independently of the async callers awaiting them.
///
/// Dropping a caller future cannot cancel a `spawn_blocking` closure. The tracker
/// therefore increments before spawning and decrements inside the blocking
/// closure, allowing shutdown code to wait for the actual operation lifetime.
#[derive(Clone, Debug, Default)]
pub struct BlockingOperationTracker {
    inner: Arc<BlockingOperationTrackerInner>,
}

#[derive(Debug, Default)]
struct BlockingOperationTrackerInner {
    active: AtomicUsize,
    idle: tokio::sync::Notify,
    #[cfg(test)]
    started: AtomicUsize,
}

struct BlockingOperationGuard {
    inner: Arc<BlockingOperationTrackerInner>,
}

impl Drop for BlockingOperationGuard {
    fn drop(&mut self) {
        let previous = self.inner.active.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "blocking operation tracker underflow");
        if previous == 1 {
            self.inner.idle.notify_waiters();
        }
    }
}

impl BlockingOperationTracker {
    /// Run one blocking operation while retaining its lifetime in this tracker.
    pub(crate) async fn run<T>(
        &self,
        operation: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, String>
    where
        T: Send + 'static,
    {
        self.inner.active.fetch_add(1, Ordering::AcqRel);
        let guard = BlockingOperationGuard {
            inner: Arc::clone(&self.inner),
        };
        tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            guard.inner.started.fetch_add(1, Ordering::AcqRel);
            let _guard = guard;
            operation()
        })
        .await
        .map_err(|error| format!("blocking SQLite task failed: {error}"))
    }

    /// Wait until every blocking operation started through this tracker exits.
    pub(crate) async fn drain(&self) {
        loop {
            let notified = self.inner.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.inner.active.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }

    #[cfg(test)]
    pub(super) fn active(&self) -> usize {
        self.inner.active.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(super) fn started(&self) -> usize {
        self.inner.started.load(Ordering::Acquire)
    }
}

/// Run synchronous SQLite work on Tokio's blocking pool.
///
/// The operation owns its inputs, so no connection guard or transaction crosses
/// an await point and the boundary is safe on current-thread runtimes and inside
/// `LocalSet` tasks.
pub async fn run<T>(operation: impl FnOnce() -> T + Send + 'static) -> Result<T, String>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| format!("blocking SQLite task failed: {error}"))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{BlockingOperationTracker, run};

    #[tokio::test(flavor = "current_thread")]
    async fn blocking_work_is_safe_inside_a_local_set() {
        let local = tokio::task::LocalSet::new();
        let result = local
            .run_until(async {
                tokio::task::spawn_local(async { run(|| 42).await.expect("blocking task") })
                    .await
                    .expect("local task")
            })
            .await;
        assert_eq!(result, 42);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tracked_blocking_work_survives_a_dropped_caller_until_drain() {
        let tracker = BlockingOperationTracker::default();
        let operation_tracker = tracker.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let caller = tokio::spawn(async move {
            operation_tracker
                .run(move || {
                    let _ = started_tx.send(());
                    std::thread::sleep(Duration::from_millis(100));
                    42
                })
                .await
        });
        started_rx.await.expect("blocking operation started");
        assert_eq!(tracker.active(), 1);

        caller.abort();
        assert!(
            caller
                .await
                .expect_err("caller must be cancelled")
                .is_cancelled()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(20), tracker.drain())
                .await
                .is_err()
        );
        tokio::time::timeout(Duration::from_secs(1), tracker.drain())
            .await
            .expect("blocking operation must eventually drain");
        assert_eq!(tracker.active(), 0);
    }
}
