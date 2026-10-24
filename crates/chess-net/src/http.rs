//! The HTTP client both sites share: one request at a time (both ask for
//! that), a user agent that says what we are, and waits when a server says
//! it's had too many requests (429).

use std::time::{Duration, Instant};

use ureq::http::Response;
use ureq::tls::{RootCerts, TlsConfig};
use ureq::{Agent, Body};

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
        }
    }

    /// The same, waiting `wait` after a 429 (tests).
    pub fn with_retry_wait(mut self, wait: Duration) -> Client {
        self.lichess_wait = wait;
        self.chess_com_wait = wait;
        self
    }

    /// The same, giving up at the first 429 (a lookup the user waits for).
    pub fn without_retries(&self) -> Client {
        Client { retries: 0, ..self.clone() }
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
            let resp = req.call().map_err(|e| unreachable(site, e))?;
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

pub(crate) fn unreachable(site: Site, e: ureq::Error) -> NetError {
    NetError::Unreachable { site, reason: e.to_string() }
}

/// An unexpected status as an error.
pub(crate) fn status_error(site: Site, resp: &Response<Body>) -> NetError {
    NetError::Status { site, code: resp.status().as_u16() }
}

/// Reads a JSON body (up to `limit` bytes).
pub(crate) fn read_json<T: serde::de::DeserializeOwned>(
    site: Site,
    resp: &mut Response<Body>,
    limit: u64,
) -> Result<T, NetError> {
    let bytes = resp.body_mut().with_config().limit(limit).read_to_vec().map_err(|e| unreachable(site, e))?;
    serde_json::from_slice(&bytes).map_err(|e| NetError::BadData { site, reason: e.to_string() })
}
