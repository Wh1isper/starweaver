//! Lightweight update hint cache.

use std::{env, fs, path::Path, thread, time::Duration};

use chrono::{DateTime, Utc};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{config::CliConfig, error::io_error, CliResult};

const DEFAULT_REPO: &str = "Wh1isper/starweaver";
const RELEASE_TAG_PREFIX: &str = "/releases/tag/";
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

pub fn update_is_newer(current: &str, latest: &str) -> bool {
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

pub fn versions_match(left: &str, right: &str) -> bool {
    let left_trimmed = left.trim_start_matches('v');
    let right_trimmed = right.trim_start_matches('v');
    match (Version::parse(left_trimmed), Version::parse(right_trimmed)) {
        (Ok(left_version), Ok(right_version)) => left_version == right_version,
        _ => left_trimmed == right_trimmed,
    }
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

pub fn fetch_latest_release() -> Result<UpdateCheckCache, String> {
    let repo = release_repo();
    fetch_latest_release_for_repo(&repo)
}

fn fetch_latest_release_for_repo(repo: &str) -> Result<UpdateCheckCache, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async {
        let client = reqwest::Client::new();
        match fetch_latest_release_redirect(&client, repo).await {
            Ok(cache) => Ok(cache),
            Err(primary_error) => {
                fetch_release_page_metadata(&client, repo)
                    .await
                    .map_err(|fallback_error| {
                        format!("{primary_error}; prerelease fallback failed: {fallback_error}")
                    })
            }
        }
    })
}

async fn fetch_latest_release_redirect(
    client: &reqwest::Client,
    repo: &str,
) -> Result<UpdateCheckCache, String> {
    let response = client
        .get(latest_release_url(repo))
        .header(reqwest::header::USER_AGENT, "starweaver-cli")
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let final_url = response.url().to_string();
    let body = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(body.trim().to_string());
    }
    let tag = parse_release_tag_from_url(&final_url)
        .ok_or_else(|| format!("latest release redirect did not include a tag: {final_url}"))?;
    Ok(cache_from_tag(repo, &tag))
}

async fn fetch_release_page_metadata(
    client: &reqwest::Client,
    repo: &str,
) -> Result<UpdateCheckCache, String> {
    let response = client
        .get(releases_url(repo))
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
    let tag = parse_release_tag_from_page(&body)
        .ok_or_else(|| "releases page did not include a release tag link".to_string())?;
    Ok(cache_from_tag(repo, &tag))
}

fn release_repo() -> String {
    env::var("STARWEAVER_GITHUB_REPO")
        .or_else(|_| env::var("STARWEAVER_REPO"))
        .unwrap_or_else(|_| DEFAULT_REPO.to_string())
}

fn latest_release_url(repo: &str) -> String {
    format!("https://github.com/{repo}/releases/latest")
}

fn releases_url(repo: &str) -> String {
    format!("https://github.com/{repo}/releases")
}

fn cache_from_tag(repo: &str, tag: &str) -> UpdateCheckCache {
    let tag = tag.trim();
    let latest_version = Some(tag.trim_start_matches('v').to_string());
    let release_url = Some(format!("https://github.com/{repo}/releases/tag/{tag}"));
    UpdateCheckCache {
        checked_at: Some(Utc::now()),
        latest_version,
        release_url,
        error: None,
    }
}

fn parse_release_tag_from_url(url: &str) -> Option<String> {
    let tag = url.split(RELEASE_TAG_PREFIX).nth(1)?;
    take_tag_segment(tag)
}

fn parse_release_tag_from_page(page: &str) -> Option<String> {
    let tag = page.split(RELEASE_TAG_PREFIX).nth(1)?;
    take_tag_segment(tag)
}

fn take_tag_segment(value: &str) -> Option<String> {
    let tag = value
        .split(['"', '?', '#', '/', '<', '>'])
        .next()
        .unwrap_or_default();
    (!tag.is_empty()).then(|| tag.to_string())
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
        assert!(update_is_newer("0.0.1", "0.0.2"));
        assert!(update_is_newer("v0.0.1", "v0.0.2"));
        assert!(!update_is_newer("0.0.2", "0.0.1"));
        assert!(!update_is_newer("0.0.1", "latest"));
        assert!(update_is_newer("dev", "0.0.1"));
        assert!(!update_is_newer("dev", "dev"));
        assert!(versions_match("0.0.1", "v0.0.1"));
        assert!(!versions_match("0.0.1", "0.0.2"));
        assert!(versions_match("dev", "dev"));
    }

    #[test]
    fn release_metadata_parser_accepts_redirects_and_release_pages() {
        assert_eq!(
            parse_release_tag_from_url(
                "https://github.com/Wh1isper/starweaver/releases/tag/v0.0.2"
            ),
            Some("v0.0.2".to_string())
        );
        assert_eq!(
            parse_release_tag_from_url(
                "https://github.com/Wh1isper/starweaver/releases/tag/v0.0.2?expanded_assets=true"
            ),
            Some("v0.0.2".to_string())
        );
        assert_eq!(
            parse_release_tag_from_page(
                r#"<a href="/Wh1isper/starweaver/releases/tag/v0.0.1">v0.0.1</a>"#
            ),
            Some("v0.0.1".to_string())
        );

        let cache = cache_from_tag(DEFAULT_REPO, "v0.0.1");
        assert_eq!(cache.latest_version.as_deref(), Some("0.0.1"));
        assert_eq!(
            cache.release_url.as_deref(),
            Some("https://github.com/Wh1isper/starweaver/releases/tag/v0.0.1")
        );
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
