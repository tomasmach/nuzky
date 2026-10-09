//! Whether a newer CapOpen was released. Only the version is read from `latest.json` on the newest
//! published GitHub release; nothing is downloaded or run. Download opens the release page.
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use semver::Version;
use serde::Deserialize;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

use crate::{CmdResult, err};

/// GitHub serves this from the newest release that is neither a draft nor a prerelease, so
/// publishing a draft is what tells installed copies about it.
const LATEST_JSON: &str = "https://github.com/tomasmach/capopen/releases/latest/download/latest.json";
/// Download opens this fixed page, never an address from the file.
const RELEASE_PAGE: &str = "https://github.com/tomasmach/capopen/releases/latest";
/// Tests serve the file locally through `CAPOPEN_UPDATE_URL`; no other address replaces GitHub.
const TEST_SERVER: &str = "http://127.0.0.1:";
const TIMEOUT: Duration = Duration::from_secs(10);
/// The file holds a version and a date; anything this big is not it.
const MAX_BYTES: u64 = 64 * 1024;

/// `CAPOPEN_NO_UPDATE_CHECK=1` keeps update checks off the network, for tests and packagers.
pub fn enabled() -> bool {
    std::env::var("CAPOPEN_NO_UPDATE_CHECK").as_deref() != Ok("1")
}

#[derive(Deserialize)]
struct Latest {
    version: String,
}

/// The released version when it is newer than `current`. Other fields are ignored, so a later
/// release can add what an in-app installer needs without silencing this version.
fn newer(json: &str, current: &Version) -> Result<Option<Version>> {
    let latest: Latest = serde_json::from_str(json).context("Reading latest.json")?;
    let text = latest.version.strip_prefix('v').unwrap_or(&latest.version);
    ensure!(text.len() <= 32, "Released version is too long");
    let version = Version::parse(text).context("Reading the released version")?;
    Ok(version.cmp_precedence(current).is_gt().then_some(version))
}

fn fetch() -> Result<String> {
    let test = std::env::var("CAPOPEN_UPDATE_URL").ok().filter(|url| url.starts_with(TEST_SERVER));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(test.is_none())
        .max_redirects(5)
        .timeout_global(Some(TIMEOUT))
        .user_agent(concat!("CapOpen/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut response = agent.get(test.as_deref().unwrap_or(LATEST_JSON)).call().context("Requesting latest.json")?;
    Ok(response.body_mut().with_config().limit(MAX_BYTES).read_to_string()?)
}

/// The newer version, or None when this one is the latest.
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> CmdResult<Option<String>> {
    if !enabled() {
        return Err("UPDATES_OFF: update checks are turned off".into());
    }
    let current = app.package_info().version.clone();
    let found = tauri::async_runtime::spawn_blocking(move || newer(&fetch()?, &current)).await.map_err(err)?;
    found.map(|version| version.map(|v| v.to_string())).map_err(|error| {
        log::warn!("Update check failed: {error:#}");
        format!("{error:#}")
    })
}

#[tauri::command]
pub fn open_release_page(app: AppHandle) -> CmdResult<()> {
    app.opener().open_url(RELEASE_PAGE, None::<&str>).map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(json: &str, current: &str) -> Option<String> {
        newer(json, &Version::parse(current).unwrap()).unwrap().map(|v| v.to_string())
    }

    #[test]
    fn offers_only_a_newer_version() {
        assert_eq!(check(r#"{"version":"0.2.0"}"#, "0.1.0").as_deref(), Some("0.2.0"));
        assert_eq!(check(r#"{"version":"0.1.10"}"#, "0.1.9").as_deref(), Some("0.1.10"));
        assert_eq!(check(r#"{"version":"0.1.0"}"#, "0.1.0"), None);
        assert_eq!(check(r#"{"version":"0.1.0"}"#, "0.2.0"), None);
        // A prerelease comes before its release; build metadata says nothing about which is newer.
        assert_eq!(check(r#"{"version":"0.2.0-beta.1"}"#, "0.2.0"), None);
        assert_eq!(check(r#"{"version":"0.2.0"}"#, "0.2.0-beta.1").as_deref(), Some("0.2.0"));
        assert_eq!(check(r#"{"version":"0.1.0+ci.7"}"#, "0.1.0"), None);
    }

    #[test]
    fn reads_tags_and_ignores_what_later_releases_add() {
        assert_eq!(check(r#"{"version":"v0.2.0"}"#, "0.1.0").as_deref(), Some("0.2.0"));
        let installer = r#"{"version":"0.3.0","notes":"x","pub_date":"2027-01-01T00:00:00Z",
            "platforms":{"linux-x86_64":{"url":"https://example.com/a","signature":"s"}}}"#;
        assert_eq!(check(installer, "0.1.0").as_deref(), Some("0.3.0"));
    }

    #[test]
    fn refuses_what_is_not_a_version() {
        let current = Version::new(0, 1, 0);
        for json in
            [r#"{"version":"latest"}"#, r#"{"version":"1.2"}"#, r#"{"tag":"0.2.0"}"#, "<html>Not Found</html>", ""]
        {
            assert!(newer(json, &current).is_err(), "{json}");
        }
        let long = format!(r#"{{"version":"1.0.0-{}"}}"#, "a".repeat(40));
        assert!(newer(&long, &current).is_err());
    }
}
