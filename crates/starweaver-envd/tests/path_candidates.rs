//! Path candidate regression tests for envd-backed environment providers.

use std::sync::Arc;

use starweaver_envd::LocalEnvd;
use starweaver_environment::{
    EnvdEnvironmentProvider, EnvironmentProvider, LocalEnvironmentProvider,
};

#[tokio::test]
async fn envd_provider_preserves_shell_context_path_candidates()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    std::fs::create_dir_all(temp.path().join("src"))?;
    std::fs::write(temp.path().join("src/lib.rs"), "lib")?;
    let local = Arc::new(LocalEnvironmentProvider::new(temp.path())?);
    let envd = Arc::new(LocalEnvd::new(local.clone()));
    let provider = EnvdEnvironmentProvider::new(envd.clone(), envd.environment_id())
        .with_shell_review_context(local.shell_review_context());

    let candidates = provider.path_match_candidates("src/lib.rs");
    assert!(candidates.iter().any(|candidate| {
        (std::path::Path::new(candidate).is_absolute() || candidate.starts_with('/'))
            && candidate.replace('\\', "/").ends_with("/src/lib.rs")
    }));
    Ok(())
}
