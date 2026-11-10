//! Checks the public GitHub Releases feed for a newer build.
//!
//! This module never downloads or runs anything: it only reports the newest
//! stable release so the UI can offer a link to its release page.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use semver::Version;
use windows::core::PCWSTR;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

pub const FEED_URL: &str =
    "https://api.github.com/repos/november-saiful/peerPrint/releases/latest";
pub const CHECK_INTERVAL_SECS: i64 = 24 * 60 * 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub name: String,
    pub html_url: String,
    pub published_at: Option<String>,
}

#[derive(serde::Deserialize)]
struct FeedRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    html_url: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Returns the newest stable release when it is newer than this build.
pub fn check() -> Result<Option<Release>> {
    // Overridable so the checker can be tested against a local fixture server.
    let url = std::env::var("PEERPRINT_UPDATE_API")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| FEED_URL.to_string());
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .build();
    let reply = agent
        .get(&url)
        .set("User-Agent", &format!("peerPrint/{}", current_version()))
        .set("Accept", "application/vnd.github+json")
        .call();
    let body = match reply {
        Ok(response) => response.into_string().context("reading the release feed")?,
        // A repository without any release answers 404; that is not an error.
        Err(ureq::Error::Status(404, _)) => return Ok(None),
        Err(err) => return Err(err).context("asking GitHub for the latest release"),
    };
    let feed: FeedRelease = serde_json::from_str(&body).context("parsing the release feed")?;
    Ok(newer_release(feed, current_version()))
}

fn newer_release(feed: FeedRelease, current: &str) -> Option<Release> {
    if feed.draft || feed.prerelease {
        return None;
    }
    let latest = parse_version(&feed.tag_name)?;
    let current = Version::parse(current).ok()?;
    if latest <= current {
        return None;
    }
    if !is_https_github(&feed.html_url) {
        return None;
    }
    Some(Release {
        version: latest.to_string(),
        name: feed.name.unwrap_or_else(|| format!("peerPrint {}", feed.tag_name)),
        html_url: feed.html_url,
        published_at: feed.published_at,
    })
}

/// Rebuilds a release from the cached config entry. Anything that is no longer
/// newer than this build, or is not a GitHub HTTPS link, is discarded.
pub fn cached(version: &str, html_url: &str) -> Option<Release> {
    let latest = Version::parse(version).ok()?;
    let current = Version::parse(current_version()).ok()?;
    if latest <= current || !is_https_github(html_url) {
        return None;
    }
    Some(Release {
        version: latest.to_string(),
        name: format!("peerPrint {latest}"),
        html_url: html_url.to_string(),
        published_at: None,
    })
}

fn parse_version(tag: &str) -> Option<Version> {
    Version::parse(tag.trim().trim_start_matches(['v', 'V'])).ok()
}

/// Only GitHub HTTPS links are opened, so a tampered feed cannot hand the
/// shell an arbitrary protocol handler.
fn is_https_github(url: &str) -> bool {
    url.starts_with("https://github.com/") || url.starts_with("https://www.github.com/")
}

pub fn open_release_page(release: &Release) -> Result<()> {
    if !is_https_github(&release.html_url) {
        bail!("refusing to open a non-GitHub link");
    }
    let url: Vec<u16> = release
        .html_url
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let verb: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let result = ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(url.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        if result.0 as isize <= 32 {
            bail!("the browser could not be opened");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(tag: &str) -> FeedRelease {
        FeedRelease {
            tag_name: tag.to_string(),
            name: None,
            html_url: format!("https://github.com/november-saiful/peerPrint/releases/tag/{tag}"),
            published_at: None,
            draft: false,
            prerelease: false,
        }
    }

    #[test]
    fn tags_may_carry_a_v_prefix() {
        assert_eq!(parse_version("v1.2.3").unwrap().to_string(), "1.2.3");
        assert_eq!(parse_version(" 1.2.3 ").unwrap().to_string(), "1.2.3");
        assert!(parse_version("latest").is_none());
        assert!(parse_version("1.2").is_none());
    }

    #[test]
    fn only_newer_stable_releases_are_offered() {
        let newer = newer_release(feed("v0.2.0"), "0.1.0").expect("0.2.0 is newer");
        assert_eq!(newer.version, "0.2.0");
        assert_eq!(newer.name, "peerPrint v0.2.0");
        assert!(newer_release(feed("v0.1.0"), "0.1.0").is_none());
        assert!(newer_release(feed("v0.0.9"), "0.1.0").is_none());
    }

    #[test]
    fn drafts_prereleases_and_foreign_links_are_ignored() {
        let mut draft = feed("v0.2.0");
        draft.draft = true;
        assert!(newer_release(draft, "0.1.0").is_none());

        let mut prerelease = feed("v0.2.0");
        prerelease.prerelease = true;
        assert!(newer_release(prerelease, "0.1.0").is_none());

        let mut foreign = feed("v0.2.0");
        foreign.html_url = "https://example.com/peerPrint/releases/tag/v0.2.0".to_string();
        assert!(newer_release(foreign, "0.1.0").is_none());
    }

    #[test]
    fn malformed_tags_are_ignored() {
        assert!(newer_release(feed("nightly"), "0.1.0").is_none());
    }

    #[test]
    fn cached_releases_are_revalidated() {
        let url = "https://github.com/november-saiful/peerPrint/releases/tag/v0.2.0";
        let release = cached("0.2.0", url).expect("0.2.0 is newer than this build");
        assert_eq!(release.version, "0.2.0");
        assert_eq!(release.html_url, url);
        assert!(cached("0.1.0", url).is_none());
        assert!(cached("0.2.0", "javascript:alert(1)").is_none());
        assert!(cached("latest", url).is_none());
    }
}
