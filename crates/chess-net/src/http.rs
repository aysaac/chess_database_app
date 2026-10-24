//! The HTTP client both sites share: one request at a time (both ask for
//! that), a user agent that says what we are, and waits when a server says
//! it's had too many requests (429).

use std::io::{self, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use ureq::http::Response;
use ureq::tls::{RootCerts, TlsConfig};
use ureq::{Agent, Body};

use crate::time::now_ms;
use crate::{NetError, Site};

const USER_AGENT: &str = concat!("chess-database-app/", env!("CARGO_PKG_VERSION"), " (desktop app)");

/// Talks to Lichess and Chess.com (or, in tests, to servers standing in for
/// them). Cheap to clone.
#[derive(Clone)]
pub struct Client {
    agent: Agent,
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
        }
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
        let wait = match site {
            Site::Lichess => self.lichess_wait,
            Site::ChessCom => self.chess_com_wait,
        };
        for attempt in 0.. {
            let mut req = self.agent.get(url).header("Accept", accept);
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

/// Reads a JSON body (up to `limit` bytes), noting its bytes in `alive`.
pub(crate) fn read_json<T: serde::de::DeserializeOwned>(
    site: Site,
    resp: &mut Response<Body>,
    limit: u64,
    alive: &Arc<AtomicI64>,
) -> Result<T, NetError> {
    let mut bytes = Vec::new();
    let reader = resp.body_mut().with_config().limit(limit).reader();
    Alive { inner: reader, at: alive.clone() }.read_to_end(&mut bytes).map_err(|e| {
        let too_big = e.get_ref().and_then(|i| i.downcast_ref::<ureq::Error>());
        match too_big {
            Some(ureq::Error::BodyExceedsLimit(_)) => NetError::BadData { site, reason: io_reason(&e) },
            _ => NetError::Unreachable { site, reason: io_reason(&e) },
        }
    })?;
    serde_json::from_slice(&bytes).map_err(|e| NetError::BadData { site, reason: e.to_string() })
}
