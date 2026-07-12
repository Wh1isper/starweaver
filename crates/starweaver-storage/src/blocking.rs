//! Blocking-task boundary for synchronous SQLite work.

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
