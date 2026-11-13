//! The HTTP client both sites share: one request at a time (both ask for
//! that), a user agent that says what we are, and waits when a server says
//! it's had too many requests (429). Writes (NET-11, NET-12) are sent once:
//! a 429 there is an answer for the caller, never a reason to send again.

use std::io::{self, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::{Duration, Instant};

use ureq::http::Response;
use ureq::tls::{RootCerts, TlsConfig};
use ureq::{Agent, Body};

use crate::oauth::Token;
use crate::time::now_ms;
use crate::{NetError, Site};

const USER_AGENT: &str = concat!("chess-database-app/", env!("CARGO_PKG_VERSION"), " (desktop app)");

/// Talks to Lichess and Chess.com (or, in tests, to servers standing in for
/// them). Cheap to clone.
#[derive(Clone)]
pub struct Client {
    pub(crate) agent: Agent,
    pub(crate) lichess: String,
    pub(crate) chess_com: String,
    /// How long to wait after a 429, per site: Lichess asks for a full
    /// minute; Chess.com says nothing.
    lichess_wait: Duration,
    chess_com_wait: Duration,
    /// Times a request answered with 429 is asked again before giving up.
    retries: u32,
    /// The longest a whole request may take (none for downloads, which can
    /// take hours).
    timeout: Option<Duration>,
    lookup_timeout: Duration,
    /// A download that gets no bytes for this long has stopped.
    pub(crate) stall: Duration,
    /// The longest a write (a study made, games sent to it) may take.
    write_timeout: Duration,
    /// How long a login waits for the browser to come back (NET-11).
    pub(crate) login_wait: Duration,
    /// The Lichess user logged in, and their token: a download of their own
    /// games sends it, which makes Lichess send them 3 times as fast.
    pub(crate) lichess_login: Option<OwnLogin>,
}

/// The login a download of the user's own Lichess games sends (NET-11).
#[derive(Clone)]
pub(crate) struct OwnLogin {
    pub username: String,
    pub token: Token,
    /// Lichess turned the token down (a 401: revoked or expired).
    pub rejected: Arc<AtomicBool>,
}

impl Default for Client {
    fn default() -> Client {
        Client::new()
    }
}

impl Client {
    /// The real servers.
    pub fn new() -> Client {
        Client::with_servers("https://lichess.org", "https://api.chess.com")
    }

    /// Other servers, given as `scheme://host[:port]` (tests).
    pub fn with_servers(lichess: &str, chess_com: &str) -> Client {
        let agent = Agent::config_builder()
            .user_agent(USER_AGENT)
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(120)))
            // The OS's certificates, so a proxy the system trusts works too.
            .tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build())
            .build()
            .into();
        Client {
            agent,
            lichess: lichess.trim_end_matches('/').to_owned(),
            chess_com: chess_com.trim_end_matches('/').to_owned(),
            lichess_wait: Duration::from_secs(60),
            chess_com_wait: Duration::from_secs(10),
            retries: 3,
            timeout: None,
            lookup_timeout: Duration::from_secs(30),
            stall: Duration::from_secs(300),
            write_timeout: Duration::from_secs(120),
            login_wait: Duration::from_secs(600),
            lichess_login: None,
        }
    }

    /// The same, sending `token` when downloading the games of `username`
    /// from Lichess: their own games, which Lichess then sends 3 times as
    /// fast (NET-11).
    pub fn with_lichess_login(mut self, username: &str, token: Token) -> Client {
        let rejected = Arc::new(AtomicBool::new(false));
        self.lichess_login = Some(OwnLogin { username: username.to_owned(), token, rejected });
        self
    }

    /// Whether Lichess turned down the login given with
    /// [`with_lichess_login`](Client::with_lichess_login) (revoked on its
    /// site, or expired): the download went on without it, and the caller
    /// should forget it.
    pub fn lichess_login_rejected(&self) -> bool {
        self.lichess_login.as_ref().is_some_and(|l| l.rejected.load(Ordering::Relaxed))
    }

    /// The same, a login giving up when the browser hasn't come back after
    /// `wait` (tests; 10 minutes otherwise).
    pub fn with_login_wait(mut self, wait: Duration) -> Client {
        self.login_wait = wait;
        self
    }

    /// The Lichess server, as `scheme://host[:port]`.
    pub fn lichess_url(&self) -> &str {
        &self.lichess
    }

    /// The same, waiting `wait` after a 429 (tests).
    pub fn with_retry_wait(mut self, wait: Duration) -> Client {
        self.lichess_wait = wait;
        self.chess_com_wait = wait;
        self
    }

    /// The same, taking a download that gets nothing for `stall` as stopped
    /// (tests; 5 minutes otherwise).
    pub fn with_stall_timeout(mut self, stall: Duration) -> Client {
        self.stall = stall;
        self
    }

    /// The same, with lookups (see [`for_lookup`](Client::for_lookup))
    /// giving up after `timeout` (tests; 30 seconds otherwise).
    pub fn with_lookup_timeout(mut self, timeout: Duration) -> Client {
        self.lookup_timeout = timeout;
        self
    }

    /// The same for a lookup the user waits for: it gives up at the first
    /// 429, and after 30 seconds whatever the server does.
    pub fn for_lookup(&self) -> Client {
        Client { retries: 0, timeout: Some(self.lookup_timeout), ..self.clone() }
    }

    /// The same, sending every request once: a GET answered with 429 is
    /// [`NetError::RateLimited`] at once, never waited on and asked again
    /// (NET-13's round-trip check against the real Lichess).
    pub fn without_retries(mut self) -> Client {
        self.retries = 0;
        self
    }

    /// GETs `url`, waiting and asking again (up to 3 times) when the site
    /// answers 429, until `stop` says to give up. Any other status comes
    /// back for the caller to judge.
    pub(crate) fn get(
        &self,
        site: Site,
        url: &str,
        query: &[(&str, String)],
        accept: &str,
        stop: &dyn Fn() -> bool,
    ) -> Result<Response<Body>, NetError> {
        self.get_with(site, url, query, accept, &[], stop)
    }

    /// [`get`](Client::get) with more headers (a token, `If-Modified-Since`).
    pub(crate) fn get_with(
        &self,
        site: Site,
        url: &str,
        query: &[(&str, String)],
        accept: &str,
        headers: &[(&str, String)],
        stop: &dyn Fn() -> bool,
    ) -> Result<Response<Body>, NetError> {
        let wait = match site {
            Site::Lichess => self.lichess_wait,
            Site::ChessCom => self.chess_com_wait,
        };
        for attempt in 0.. {
            let mut req = self.agent.get(url).header("Accept", accept);
            for (k, v) in headers {
                req = req.header(*k, v);
            }
            for (k, v) in query {
                req = req.query(*k, v);
            }
            if let Some(t) = self.timeout {
                req = req.config().timeout_global(Some(t)).build();
            }
            let resp = req.call().map_err(|e| unreachable(site, &e))?;
            if resp.status().as_u16() != 429 {
                return Ok(resp);
            }
            if attempt == self.retries {
                break;
            }
            log::info!("{}: 429 for {url}, waiting {} s", site.name(), wait.as_secs());
            let until = Instant::now() + wait;
            while Instant::now() < until {
                if stop() {
                    return Err(NetError::Cancelled);
                }
                std::thread::sleep(Duration::from_millis(100).min(wait));
            }
        }
        Err(NetError::RateLimited(site))
    }

    /// POSTs a form once: a 429 comes back like any other status, never
    /// sent again. It gives up after 2 minutes.
    pub(crate) fn post_form(
        &self,
        site: Site,
        url: &str,
        token: Option<&Token>,
        fields: &[(&str, &str)],
    ) -> Result<Response<Body>, NetError> {
        let mut req = self.agent.post(url).header("Accept", "application/json");
        if let Some(token) = token {
            req = req.header("Authorization", token.bearer());
        }
        let req = req.config().timeout_global(Some(self.write_timeout)).build();
        req.send_form(fields.iter().copied()).map_err(|e| unreachable(site, &e))
    }

    /// POSTs plain text once, like [`post_form`](Client::post_form).
    pub(crate) fn post_text(&self, site: Site, url: &str, text: &str) -> Result<Response<Body>, NetError> {
        let req = self.agent.post(url).header("Accept", "application/json").content_type("text/plain");
        let req = req.config().timeout_global(Some(self.write_timeout)).build();
        req.send(text).map_err(|e| unreachable(site, &e))
    }

    /// DELETEs once, with `token`, like [`post_form`](Client::post_form).
    pub(crate) fn delete(&self, site: Site, url: &str, token: &Token) -> Result<Response<Body>, NetError> {
        let req = self.agent.delete(url).header("Accept", "application/json");
        let req = req.header("Authorization", token.bearer());
        let req = req.config().timeout_global(Some(self.write_timeout)).build();
        req.call().map_err(|e| unreachable(site, &e))
    }
}

fn unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

/// `text` percent-encoded: everything but letters, digits and `-._~`.
pub(crate) fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if unreserved(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// How long `text` is once percent-encoded: at least what a form body
/// takes for it (ureq's `send_form` leaves a few more characters as they
/// are, and a space is a `+`).
pub(crate) fn encoded_len(text: &str) -> usize {
    text.bytes().map(|b| if unreserved(b) { 1 } else { 3 }).sum()
}

/// A percent-encoded query value decoded (`+` is a space).
pub(crate) fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex =
            bytes.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
                continue;
            }
            (b'+', _) => out.push(b' '),
            (b, _) => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The pairs of a query string (`a=1&b=x%20y`), decoded.
pub(crate) fn query_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (decode(k), decode(v))
        })
        .collect()
}

/// The `error` (or `error_description`) of a JSON error body, if it has one.
pub(crate) fn error_message(site: Site, resp: &mut Response<Body>) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Error {
        error: Option<serde_json::Value>,
        error_description: Option<String>,
    }
    let e: Error = read_json(site, resp, 1 << 16).ok()?;
    let error = e.error.map(|v| match v {
        serde_json::Value::String(s) => s,
        // A form's errors: {"name": ["…"]}.
        other => other.to_string(),
    });
    e.error_description.filter(|d| !d.is_empty()).or(error).filter(|m| !m.trim().is_empty())
}

/// What went wrong, in words for the user (ureq's own start with "io: ").
fn reason(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Io(e) => io_reason(e),
        ureq::Error::Timeout(_) => "no answer in time".to_owned(),
        ureq::Error::HostNotFound => "its address can't be found".to_owned(),
        ureq::Error::ConnectionFailed => "the connection failed".to_owned(),
        ureq::Error::BodyExceedsLimit(n) => format!("an answer over {} MB", n >> 20),
        e => e.to_string(),
    }
}

/// The same for an error reading a body, which wraps ureq's.
pub(crate) fn io_reason(e: &io::Error) -> String {
    match e.get_ref().and_then(|inner| inner.downcast_ref::<ureq::Error>()) {
        Some(e) => reason(e),
        None => e.to_string(),
    }
}

pub(crate) fn unreachable(site: Site, e: &ureq::Error) -> NetError {
    NetError::Unreachable { site, reason: reason(e) }
}

/// An unexpected status as an error.
pub(crate) fn status_error(site: Site, resp: &Response<Body>) -> NetError {
    NetError::Status { site, code: resp.status().as_u16() }
}

/// A body that notes when its last bytes came (ms since the epoch), so a
/// download that's slow but going isn't taken for stalled.
pub(crate) struct Alive<R> {
    pub inner: R,
    pub at: Arc<AtomicI64>,
}

impl<R: Read> Read for Alive<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.at.store(now_ms(), Ordering::Relaxed);
        }
        Ok(n)
    }
}

/// Reads a JSON body (up to `limit` bytes).
pub(crate) fn read_json<T: serde::de::DeserializeOwned>(
    site: Site,
    resp: &mut Response<Body>,
    limit: u64,
) -> Result<T, NetError> {
    let bytes = read_bytes(site, resp, limit)?;
    serde_json::from_slice(&bytes).map_err(|e| NetError::BadData { site, reason: e.to_string() })
}

/// The same, noting its bytes in `alive` as they come (a download, which
/// is stalled only when nothing comes for a while).
pub(crate) fn read_json_noting<T: serde::de::DeserializeOwned>(
    site: Site,
    resp: &mut Response<Body>,
    limit: u64,
    alive: &Arc<AtomicI64>,
) -> Result<T, NetError> {
    let mut bytes = Vec::new();
    let reader = resp.body_mut().with_config().limit(limit).reader();
    Alive { inner: reader, at: alive.clone() }.read_to_end(&mut bytes).map_err(|e| body_error(site, &e))?;
    serde_json::from_slice(&bytes).map_err(|e| NetError::BadData { site, reason: e.to_string() })
}

/// An error reading a body: one over its limit is bad data, the rest a
/// connection that broke.
fn body_error(site: Site, e: &io::Error) -> NetError {
    match e.get_ref().and_then(|i| i.downcast_ref::<ureq::Error>()) {
        Some(ureq::Error::BodyExceedsLimit(_)) => NetError::BadData { site, reason: io_reason(e) },
        _ => NetError::Unreachable { site, reason: io_reason(e) },
    }
}

/// Reads a whole body (up to `limit` bytes).
pub(crate) fn read_bytes(site: Site, resp: &mut Response<Body>, limit: u64) -> Result<Vec<u8>, NetError> {
    let mut bytes = Vec::new();
    let mut reader = resp.body_mut().with_config().limit(limit).reader();
    reader.read_to_end(&mut bytes).map_err(|e| body_error(site, &e))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encoding_round_trips() {
        let text = "study:read study:write é&=+/?#[]\"{}\n";
        assert_eq!(encode("a-b_c.d~9"), "a-b_c.d~9");
        assert_eq!(encode("a b&c"), "a%20b%26c");
        assert_eq!(decode(&encode(text)), text);
        assert_eq!(encoded_len(text), encode(text).len());
        assert_eq!(decode("a+b%2"), "a b%2", "a cut escape stays as it is");
        assert_eq!(
            query_pairs("code=liu_x%2By&state=s+1&&flag"),
            [
                ("code".into(), "liu_x+y".into()),
                ("state".into(), "s 1".into()),
                ("flag".into(), String::new())
            ]
        );
    }
}
