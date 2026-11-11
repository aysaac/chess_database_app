//! Who is asking: a browser session (cookie) or an API token (bearer).
//!
//! - Sessions: an HttpOnly, SameSite=Strict cookie (Secure when the public
//!   URL is https, or a trusted proxy says the request came over https).
//!   State-changing requests on a session must send its CSRF token in
//!   `X-CSRF-Token` (`GET /api/me` and the login give it).
//! - API tokens: `Authorization: Bearer cdb_…`, for the desktop client and
//!   scripts. A token isn't sent by a browser on its own, so it needs no
//!   CSRF token.
//!
//! A disabled user's sessions and tokens stop working at once: every
//! lookup checks the flag.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, header};

use crate::config::Config;
use crate::error::{ApiError, ApiResult};
use crate::secret::{ct_eq, token_digest};
use crate::store::User;
use crate::{AppState, blocking};

pub const SESSION_COOKIE: &str = "chessdb_session";
pub const CSRF_HEADER: &str = "x-csrf-token";

/// The browser session a request came with.
#[derive(Debug, Clone)]
pub struct Session {
    pub token_hash: Vec<u8>,
    pub csrf: String,
}

/// An authenticated request (the extractor answers 401 otherwise).
#[derive(Debug, Clone)]
pub struct Auth {
    pub user: User,
    /// None for an API token.
    pub session: Option<Session>,
}

/// An authenticated admin (403 for other users).
#[derive(Debug, Clone)]
pub struct AdminAuth(pub Auth);

fn bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim().to_owned())
}

/// The value of cookie `name`.
pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|c| c.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_owned())
}

fn is_safe(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

impl FromRequestParts<Arc<AppState>> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &Arc<AppState>) -> ApiResult<Auth> {
        if let Some(token) = bearer(&parts.headers) {
            let st = state.clone();
            let user = blocking(move || st.store.token_user(&token_digest(&token))).await?;
            return user.map(|user| Auth { user, session: None }).ok_or_else(ApiError::unauthorized);
        }
        let token = cookie(&parts.headers, SESSION_COOKIE).ok_or_else(ApiError::unauthorized)?;
        let token_hash = token_digest(&token);
        let st = state.clone();
        let hash = token_hash.clone();
        let (user, csrf) =
            blocking(move || st.store.session(&hash)).await?.ok_or_else(ApiError::unauthorized)?;
        if !is_safe(&parts.method) {
            let sent = parts.headers.get(CSRF_HEADER).and_then(|v| v.to_str().ok()).unwrap_or("");
            if !ct_eq(sent.as_bytes(), csrf.as_bytes()) {
                return Err(ApiError::forbidden("Missing or wrong CSRF token (X-CSRF-Token)"));
            }
        }
        Ok(Auth { user, session: Some(Session { token_hash, csrf }) })
    }
}

impl FromRequestParts<Arc<AppState>> for AdminAuth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &Arc<AppState>) -> ApiResult<AdminAuth> {
        let auth = Auth::from_request_parts(parts, state).await?;
        if !auth.user.is_admin() {
            return Err(ApiError::forbidden("Only an admin can do that"));
        }
        Ok(AdminAuth(auth))
    }
}

/// The client's address: the peer's, or with a trusted proxy in front the
/// last address in `X-Forwarded-For` (the one the proxy added).
pub fn client_ip(config: &Config, headers: &HeaderMap, peer: SocketAddr) -> String {
    if config.trust_proxy
        && let Some(last) = headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .map(str::trim)
            .rfind(|a| !a.is_empty())
    {
        return last.to_owned();
    }
    peer.ip().to_string()
}

fn secure(config: &Config, headers: &HeaderMap) -> bool {
    config.secure_cookies()
        || (config.trust_proxy
            && headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https"))
}

/// `Set-Cookie` for a new session (`max_age` seconds), or for ending one
/// (`None`).
pub fn session_cookie(
    config: &Config,
    headers: &HeaderMap,
    token: Option<&str>,
    max_age: u64,
) -> HeaderValue {
    let secure = if secure(config, headers) { "; Secure" } else { "" };
    let value = match token {
        Some(t) => {
            format!("{SESSION_COOKIE}={t}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}")
        }
        None => format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure}"),
    };
    HeaderValue::from_str(&value).expect("cookie is ASCII")
}

/// Failed logins per key (an account, a client address).
pub struct Throttle {
    failures: Mutex<HashMap<String, (u32, Instant)>>,
    limit: u32,
    window: Duration,
}

/// Keys kept at most; the oldest go first.
const THROTTLE_KEYS: usize = 100_000;

impl Throttle {
    pub fn new(limit: u32, window: Duration) -> Throttle {
        Throttle { failures: Mutex::default(), limit, window }
    }

    /// Refuses (with the seconds to wait) when a key had `limit` failures
    /// in the window.
    pub fn check(&self, keys: &[String]) -> ApiResult<()> {
        let now = Instant::now();
        let failures = self.failures.lock().expect("throttle lock");
        for key in keys {
            if let Some(&(count, first)) = failures.get(key) {
                let age = now.duration_since(first);
                if count >= self.limit && age < self.window {
                    return Err(ApiError::too_many((self.window - age).as_secs().max(1)));
                }
            }
        }
        Ok(())
    }

    pub fn fail(&self, keys: &[String]) {
        let now = Instant::now();
        let mut failures = self.failures.lock().expect("throttle lock");
        if failures.len() >= THROTTLE_KEYS {
            failures.retain(|_, (_, first)| now.duration_since(*first) < self.window);
        }
        for key in keys {
            let entry = failures.entry(key.clone()).or_insert((0, now));
            if now.duration_since(entry.1) >= self.window {
                *entry = (0, now);
            }
            entry.0 += 1;
        }
    }

    pub fn clear(&self, key: &str) {
        self.failures.lock().expect("throttle lock").remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_blocks_after_the_limit() {
        let t = Throttle::new(2, Duration::from_secs(60));
        let keys = ["user:a".to_owned(), "ip:1".to_owned()];
        t.check(&keys).unwrap();
        t.fail(&keys);
        t.check(&keys).unwrap();
        t.fail(&keys);
        assert_eq!(t.check(&keys).unwrap_err().status, 429);
        // Another account from the same address is blocked too.
        assert!(t.check(&["user:b".to_owned(), "ip:1".to_owned()]).is_err());
        t.clear("user:a");
        t.clear("ip:1");
        t.check(&keys).unwrap();
        let t = Throttle::new(1, Duration::ZERO);
        t.fail(&keys);
        t.check(&keys).unwrap();
    }

    #[test]
    fn cookies_and_bearer() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("a=1; chessdb_session=tok; b=2"));
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer cdb_x"));
        assert_eq!(cookie(&h, SESSION_COOKIE).as_deref(), Some("tok"));
        assert_eq!(cookie(&h, "c"), None);
        assert_eq!(bearer(&h).as_deref(), Some("cdb_x"));
    }

    #[test]
    fn forwarded_addresses_only_from_a_trusted_proxy() {
        let mut c = Config::new("d".into());
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_static("6.6.6.6, 10.0.0.1"));
        let peer: SocketAddr = "127.0.0.1:5".parse().unwrap();
        assert_eq!(client_ip(&c, &h, peer), "127.0.0.1");
        c.trust_proxy = true;
        assert_eq!(client_ip(&c, &h, peer), "10.0.0.1");
        let v = session_cookie(&c, &h, Some("t"), 60);
        assert!(!v.to_str().unwrap().contains("Secure"));
        c.public_url = Some("https://x".into());
        let v = session_cookie(&c, &h, Some("t"), 60);
        assert!(v.to_str().unwrap().ends_with("; Secure"));
    }
}
