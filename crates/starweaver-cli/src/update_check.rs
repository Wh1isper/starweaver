//! Lightweight update hint cache.

use std::{env, fs, path::Path, thread, time::Duration};

use chrono::{DateTime, Utc};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{config::CliConfig, error::io_error, CliResult};

const LATEST_RELEASE_API: &str = "https://api.github.com/repos/Wh1isper/starweaver/releases/latest";
const CHECK_INTERVAL_SECONDS: i64 = 24 * 60 * 60;

/// Cached update check result.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateCheckCache {
    /// Last check timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<DateTime<Utc>>,
    /// Latest release version without a leading `v`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    /// Latest release URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_url: Option<String>,
    /// Last check error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Start a background update check when the cache is stale.
pub fn spawn_update_check_if_due(config: &CliConfig) {
    if update_check_disabled() {
        return;
    }
    let path = cache_path(config);
    if cache_is_fresh(&path) {
        return;
    }
    thread::spawn(move || {
        let cache = fetch_latest_release().unwrap_or_else(|error| UpdateCheckCache {
            checked_at: Some(Utc::now()),
            latest_version: None,
            release_url: None,
            error: Some(error),
        });
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(payload) = serde_json::to_vec_pretty(&cache) {
            let _ = fs::write(path, payload);
        }
    });
}

/// Return a human update hint from the existing cache.
pub fn update_hint(config: &CliConfig) -> Option<String> {
    if update_check_disabled() {
        return None;
    }
    let cache = read_cache(config).ok()?;
    let latest = cache.latest_version?.trim_start_matches('v').to_string();
    let current = env!("CARGO_PKG_VERSION");
    update_is_newer(current, &latest).then(|| {
        format!("Update available: starweaver {current} -> {latest}. Run `starweaver update`.\n")
    })
}

fn update_is_newer(current: &str, latest: &str) -> bool {
    let current_trimmed = current.trim_start_matches('v');
    let latest_trimmed = latest.trim_start_matches('v');
    let Ok(current_version) = Version::parse(current_trimmed) else {
        return latest_trimmed != current_trimmed;
    };
    let Ok(latest_version) = Version::parse(latest_trimmed) else {
        return false;
    };
    latest_version > current_version
}

/// Read cached update metadata.
pub fn read_cache(config: &CliConfig) -> CliResult<UpdateCheckCache> {
    let path = cache_path(config);
    let content = fs::read_to_string(&path).map_err(|error| io_error(&path, error))?;
    serde_json::from_str(&content).map_err(Into::into)
}

fn update_check_disabled() -> bool {
    update_check_disabled_value(env::var("STARWEAVER_UPDATE_CHECK").ok().as_deref())
}

fn update_check_disabled_value(value: Option<&str>) -> bool {
    matches!(value, Some("0" | "false" | "off" | "never"))
}

fn cache_path(config: &CliConfig) -> std::path::PathBuf {
    config.global_dir.join("update-check.json")
}

fn cache_is_fresh(path: &Path) -> bool {
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(cache) = serde_json::from_str::<UpdateCheckCache>(&content) else {
        return false;
    };
    let Some(checked_at) = cache.checked_at else {
        return false;
    };
    Utc::now().signed_duration_since(checked_at).num_seconds() < CHECK_INTERVAL_SECONDS
}

fn fetch_latest_release() -> Result<UpdateCheckCache, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async {
        let response = reqwest::Client::new()
            .get(LATEST_RELEASE_API)
            .header(reqwest::header::USER_AGENT, "starweaver-cli")
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status();
        let body = response.text().await.map_err(|error| error.to_string())?;
        if !status.is_success() {
            return Err(body.trim().to_string());
        }
        let json = serde_json::from_str::<Value>(&body).map_err(|error| error.to_string())?;
        let latest_version = json
            .get("tag_name")
            .and_then(Value::as_str)
            .map(|tag| tag.trim_start_matches('v').to_string());
        let release_url = json
            .get("html_url")
            .and_then(Value::as_str)
            .map(str::to_string);
        Ok(UpdateCheckCache {
            checked_at: Some(Utc::now()),
            latest_version,
            release_url,
            error: None,
        })
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use chrono::TimeDelta;

    use crate::{args, ConfigResolver};

    use super::*;

    fn test_config(root: &Path) -> CliConfig {
        let cli = args::parse(["starweaver-cli".to_string()]).unwrap();
        ConfigResolver::for_tests(root).resolve(&cli).unwrap()
    }

    #[test]
    fn update_version_comparison_handles_semver_and_fallbacks() {
        assert!(update_is_newer("0.1.0", "0.2.0"));
        assert!(update_is_newer("v0.1.0", "v0.1.1"));
        assert!(!update_is_newer("0.2.0", "0.1.0"));
        assert!(!update_is_newer("0.1.0", "latest"));
        assert!(update_is_newer("dev", "0.1.0"));
        assert!(!update_is_newer("dev", "dev"));
    }

    #[test]
    fn update_cache_and_hint_cover_fresh_stale_and_disabled_paths() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_config(temp.path());
        let path = cache_path(&config);
        assert!(!cache_is_fresh(&path));

        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();
        assert!(!cache_is_fresh(&path));

        let stale = UpdateCheckCache {
            checked_at: Some(Utc::now() - TimeDelta::seconds(CHECK_INTERVAL_SECONDS + 1)),
            latest_version: Some("999.0.0".to_string()),
            release_url: Some("https://example.com/release".to_string()),
            error: None,
        };
        std::fs::write(&path, serde_json::to_vec(&stale).unwrap()).unwrap();
        assert!(!cache_is_fresh(&path));
        assert!(update_hint(&config).unwrap().contains("999.0.0"));

        let fresh = UpdateCheckCache {
            checked_at: Some(Utc::now()),
            latest_version: Some("0.0.0".to_string()),
            release_url: None,
            error: None,
        };
        std::fs::write(&path, serde_json::to_vec(&fresh).unwrap()).unwrap();
        assert!(cache_is_fresh(&path));
        assert!(update_hint(&config).is_none());
        assert_eq!(
            read_cache(&config).unwrap().latest_version.as_deref(),
            Some("0.0.0")
        );

        assert!(update_check_disabled_value(Some("0")));
        assert!(update_check_disabled_value(Some("false")));
        assert!(update_check_disabled_value(Some("off")));
        assert!(update_check_disabled_value(Some("never")));
        assert!(!update_check_disabled_value(Some("1")));
        assert!(!update_check_disabled_value(None));
    }
}
