//! Blocking-task boundary for synchronous local environment I/O.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

struct CancelOnDrop {
    cancelled: Arc<AtomicBool>,
    armed: bool,
}

impl CancelOnDrop {
    const fn new(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            cancelled,
            armed: true,
        }
    }

    const fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.cancelled.store(true, Ordering::Release);
        }
    }
}

/// Run synchronous work on Tokio's blocking pool.
///
/// The operation owns its inputs so this remains safe on current-thread
/// runtimes and inside `LocalSet` tasks. Join failures are returned as redacted
/// scheduler errors for mapping into the caller's domain error.
pub async fn run<T>(operation: impl FnOnce() -> T + Send + 'static) -> Result<T, String>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| format!("blocking environment task failed: {error}"))
}

/// Run cancellable synchronous work on Tokio's blocking pool.
///
/// Dropping the returned future signals the owned blocking operation. The
/// operation must poll the flag and perform its own synchronous cleanup before
/// returning because Tokio cannot abort a running blocking task.
pub async fn run_cancellable<T>(
    operation: impl FnOnce(Arc<AtomicBool>) -> T + Send + 'static,
) -> Result<T, String>
where
    T: Send + 'static,
{
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut cancel_on_drop = CancelOnDrop::new(Arc::clone(&cancelled));
    let result = tokio::task::spawn_blocking(move || operation(cancelled)).await;
    if result.is_ok() {
        cancel_on_drop.disarm();
    }
    result.map_err(|error| format!("blocking environment task failed: {error}"))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::run;

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
}
