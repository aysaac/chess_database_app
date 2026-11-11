//! The Week in Chess (NET-06): Mark Crowther's weekly issues of the games
//! played that week, at <https://theweekinchess.com>. The client finds the
//! newest issue (the highest `twicNNNNg.zip` its TWIC page links to) and
//! fetches issue N's PGN, a zip holding one `twicNNNN.pgn`. Blocking, like
//! the rest of the crate; one request at a time, with a user agent that
//! says what's asking. The server's updater (NET-03) asks once a week.
//!
//! Self-contained (its own agent and error type), so it can move beside the
//! app's downloads later without touching them.

use std::io::{Cursor, Read};
use std::time::Duration;

use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig};

/// Says what's asking, and how often.
const USER_AGENT: &str =
    concat!("chess-database-app/", env!("CARGO_PKG_VERSION"), " (TWIC updater; one issue a week)");
/// The TWIC page is a few hundred KB; an issue's zip a few MB.
const PAGE_LIMIT: u64 = 8 << 20;
const ZIP_LIMIT: u64 = 64 << 20;
/// An issue's PGN unpacked, at most.
const PGN_LIMIT: u64 = 256 << 20;

#[derive(Debug, thiserror::Error)]
pub enum TwicError {
    #[error("couldn't reach The Week in Chess ({0}). Check the internet connection and try again")]
    Unreachable(String),
    #[error("The Week in Chess answered with an error (HTTP {0}). Try again later")]
    Status(u16),
    #[error("The Week in Chess has no issue {0}")]
    NoSuchIssue(u32),
    #[error("The Week in Chess's page lists no issues: its layout may have changed")]
    NoIssues,
    #[error("issue {0} can't be read: {1}")]
    BadZip(u32, String),
}

/// Talks to The Week in Chess (or, in tests, a server standing in for it).
#[derive(Clone)]
pub struct Twic {
    agent: Agent,
    base: String,
}

impl Default for Twic {
    fn default() -> Twic {
        Twic::new()
    }
}

impl Twic {
    /// The real site.
    pub fn new() -> Twic {
        Twic::with_base("https://theweekinchess.com")
    }

    /// Another server, as `scheme://host[:port]` (tests).
    pub fn with_base(base: &str) -> Twic {
        let agent = Agent::config_builder()
            .user_agent(USER_AGENT)
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_global(Some(Duration::from_secs(300)))
            .tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build())
            .build()
            .into();
        Twic { agent, base: base.trim_end_matches('/').to_owned() }
    }

    fn get(&self, path: &str, accept: &str, limit: u64) -> Result<Option<Vec<u8>>, TwicError> {
        let url = format!("{}{path}", self.base);
        let mut resp = self
            .agent
            .get(&url)
            .header("Accept", accept)
            .header("Referer", &format!("{}/twic", self.base))
            .call()
            .map_err(|e| TwicError::Unreachable(e.to_string()))?;
        match resp.status().as_u16() {
            200 => {}
            404 => return Ok(None),
            code => return Err(TwicError::Status(code)),
        }
        let mut bytes = Vec::new();
        resp.body_mut()
            .with_config()
            .limit(limit)
            .reader()
            .read_to_end(&mut bytes)
            .map_err(|e| TwicError::Unreachable(e.to_string()))?;
        Ok(Some(bytes))
    }

    /// The newest issue: the highest one the TWIC page links to.
    pub fn latest_issue(&self) -> Result<u32, TwicError> {
        let page =
            self.get("/twic", "text/html,application/xhtml+xml", PAGE_LIMIT)?.ok_or(TwicError::NoIssues)?;
        issues_in(&String::from_utf8_lossy(&page)).into_iter().max().ok_or(TwicError::NoIssues)
    }

    /// Issue `n`'s games as PGN.
    pub fn issue_pgn(&self, n: u32) -> Result<Vec<u8>, TwicError> {
        let zip = self
            .get(&format!("/zips/twic{n}g.zip"), "application/zip,application/octet-stream", ZIP_LIMIT)?
            .ok_or(TwicError::NoSuchIssue(n))?;
        pgn_from_zip(n, &zip)
    }
}

/// The issue numbers linked to in a TWIC page: `twicNNNNg.zip` (the PGN
/// zips; `twicNNNNc6.zip` and others are other formats).
pub fn issues_in(html: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("twic") {
        rest = &rest[i + 4..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty()
            && rest[digits.len()..].starts_with("g.zip")
            && let Ok(n) = digits.parse()
        {
            out.push(n);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// The PGN in an issue's zip: its `.pgn` entry (the first, if several).
pub fn pgn_from_zip(n: u32, zip: &[u8]) -> Result<Vec<u8>, TwicError> {
    let bad = |e: &dyn std::fmt::Display| TwicError::BadZip(n, e.to_string());
    let mut archive = zip::ZipArchive::new(Cursor::new(zip)).map_err(|e| bad(&e))?;
    let is_pgn = |e: &zip::read::ZipFile<'_, Cursor<&[u8]>>| {
        e.name().is_ok_and(|n| n.to_ascii_lowercase().ends_with(".pgn"))
    };
    let index = (0..archive.len())
        .find(|&i| archive.by_index(i).is_ok_and(|e| is_pgn(&e)))
        .ok_or_else(|| bad(&"it holds no PGN file"))?;
    let entry = archive.by_index(index).map_err(|e| bad(&e))?;
    let mut pgn = Vec::new();
    entry.take(PGN_LIMIT + 1).read_to_end(&mut pgn).map_err(|e| bad(&e))?;
    if pgn.len() as u64 > PGN_LIMIT {
        return Err(bad(&"its PGN is too big"));
    }
    Ok(pgn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issues_are_read_from_the_page() {
        let page = r#"<a href="https://theweekinchess.com/zips/twic1600g.zip">PGN</a>
            <a href="/zips/twic1601g.zip">PGN</a> <a href="/zips/twic1601c6.zip">CBV</a>
            <a href="/html/twic1601.html">twic1601</a> twicg.zip twic12xg.zip"#;
        assert_eq!(issues_in(page), [1600, 1601]);
        assert!(issues_in("nothing here").is_empty());
    }

    #[test]
    fn a_zip_without_a_pgn_is_refused() {
        assert!(matches!(pgn_from_zip(1, b"not a zip"), Err(TwicError::BadZip(1, _))));
    }
}
