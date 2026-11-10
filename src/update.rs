//! Checks the public GitHub Releases page for a newer build.
//!
//! The check follows the `/releases/latest` redirect instead of the REST API:
//! the API allows only 60 unauthenticated requests per hour per IP, which
//! shared connections exhaust, while the web redirect carries no such quota.
//! This module never downloads or runs anything: it only reports the newest
//! stable release so the UI can offer a link to its release page.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use semver::Version;
use windows::core::PCWSTR;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

pub const RELEASES_URL: &str =
    "https://github.com/november-saiful/peerPrint/releases/latest";
pub const CHECK_INTERVAL_SECS: i64 = 24 * 60 * 60;

/// Path under github.com that identifies a release tag of this repository.
const TAG_PREFIX: &str = "/november-saiful/peerPrint/releases/tag/";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub name: String,
    pub html_url: String,
    pub published_at: Option<String>,
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Returns the newest stable release when it is newer than this build.
///
/// `/releases/latest` redirects to the newest non-draft, non-prerelease tag,
/// so the redirect target already has the right semantics.
pub fn check() -> Result<Option<Release>> {
    // Overridable so the checker can be tested against a local fixture server
    // that answers with a Location header pointing at a release tag.
    let url = std::env::var("PEERPRINT_UPDATE_API")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| RELEASES_URL.to_string());
    check_url(&url)
}

fn check_url(url: &str) -> Result<Option<Release>> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .redirects(0)
        .build();
    let response = match agent
        .get(&url)
        .set("User-Agent", &format!("peerPrint/{}", current_version()))
        .call()
    {
        Ok(response) => response,
        // A repository without any release has no redirect target.
        Err(ureq::Error::Status(404, _)) => return Ok(None),
        Err(err) => return Err(err).context("asking GitHub for the latest release"),
    };
    if response.status() == 404 {
        return Ok(None);
    }
    if !(300..400).contains(&response.status()) {
        bail!("the releases page answered {}", response.status());
    }
    let location = response
        .header("Location")
        .context("the releases page did not redirect")?;
    Ok(newer_release(location, current_version()))
}

fn tag_from_release_url(url: &str) -> Option<&str> {
    let path = url
        .strip_prefix("https://github.com")
        .or_else(|| url.strip_prefix("https://www.github.com"))?;
    let tag = path.strip_prefix(TAG_PREFIX)?;
    Some(tag.split(['?', '#']).next().unwrap_or(tag))
}

fn newer_release(location: &str, current: &str) -> Option<Release> {
    let tag = tag_from_release_url(location)?;
    let latest = parse_version(tag)?;
    let current = Version::parse(current).ok()?;
    if latest <= current {
        return None;
    }
    Some(Release {
        version: latest.to_string(),
        name: format!("peerPrint {latest}"),
        html_url: location.to_string(),
        published_at: None,
    })
}

/// Rebuilds a release from the cached config entry. The link must still point
/// at a tag of this repository that is newer than this build.
pub fn cached(version: &str, html_url: &str) -> Option<Release> {
    let release = newer_release(html_url, current_version())?;
    (release.version == version).then_some(release)
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

    fn tag_url(tag: &str) -> String {
        format!("https://github.com/november-saiful/peerPrint/releases/tag/{tag}")
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
        let newer = newer_release(&tag_url("v0.2.0"), "0.1.0").expect("0.2.0 is newer");
        assert_eq!(newer.version, "0.2.0");
        assert_eq!(newer.name, "peerPrint 0.2.0");
        assert_eq!(newer.html_url, tag_url("v0.2.0"));
        assert!(newer_release(&tag_url("v0.1.0"), "0.1.0").is_none());
        assert!(newer_release(&tag_url("v0.0.9"), "0.1.0").is_none());
        assert!(newer_release(&tag_url("v0.2.0?notice=1"), "0.1.0").is_some());
    }

    #[test]
    fn release_links_must_point_at_this_repository() {
        assert!(newer_release(
            "https://example.com/peerPrint/releases/tag/v9.9.9",
            "0.1.0"
        )
        .is_none());
        assert!(newer_release(
            "https://github.com/other/repo/releases/tag/v9.9.9",
            "0.1.0"
        )
        .is_none());
        assert!(newer_release(
            "https://github.com/november-saiful/peerPrint/releases",
            "0.1.0"
        )
        .is_none());
        assert!(newer_release(
            "http://github.com/november-saiful/peerPrint/releases/tag/v9.9.9",
            "0.1.0"
        )
        .is_none());
    }

    #[test]
    fn malformed_tags_are_ignored() {
        assert!(newer_release(&tag_url("nightly"), "0.1.0").is_none());
    }

    #[test]
    fn redirect_location_is_read_without_following_it() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request);
            socket
                .write_all(
                    b"HTTP/1.1 302 Found\r\n\
                      Location: https://github.com/november-saiful/peerPrint/releases/tag/v99.0.0\r\n\
                      Content-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        });

        let release = check_url(&format!("http://{addr}/latest"))
            .unwrap()
            .expect("the redirect target is newer");
        assert_eq!(release.version, "99.0.0");
        server.join().unwrap();
    }

    #[test]
    fn missing_releases_page_is_not_an_error() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request);
            socket
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
        });

        assert!(check_url(&format!("http://{addr}/latest")).unwrap().is_none());
        server.join().unwrap();
    }

    #[test]
    fn cached_releases_are_revalidated() {
        let url = tag_url("v0.2.0");
        let release = cached("0.2.0", &url).expect("0.2.0 is newer than this build");
        assert_eq!(release.version, "0.2.0");
        assert_eq!(release.html_url, url);
        assert!(cached("0.1.0", &url).is_none());
        assert!(cached("0.3.0", &url).is_none());
        assert!(cached(
            "0.2.0",
            "https://github.com/other/repo/releases/tag/v0.2.0"
        )
        .is_none());
        assert!(cached("0.2.0", "javascript:alert(1)").is_none());
    }
}
