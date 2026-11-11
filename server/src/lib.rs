//! The self-hosted server (NET-03, phase 1): several users' databases,
//! searched over a JSON API, with an admin web UI. `docs/server.md` is the
//! reference: architecture, data directory, API, roles and what "private"
//! guarantees.
//!
//! The core crates are synchronous; every call into them (and into the
//! metadata store) runs on tokio's blocking pool ([`blocking`]).

pub mod access;
pub mod audit;
pub mod auth;
pub mod backup;
pub mod config;
pub mod error;
pub mod housekeeping;
pub mod jobs;
pub mod listing;
pub mod mail;
pub mod metrics;
pub mod pool;
pub mod ratelimit;
mod routes;
pub mod secret;
pub mod store;
pub mod twic;
mod ui;
pub mod wire;

use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};

use crate::auth::Throttle;
use crate::config::Config;
use crate::error::{ApiError, ApiResult};
use crate::jobs::Jobs;
use crate::pool::Pool;
use crate::store::Store;

pub struct AppState {
    pub config: Config,
    pub store: Arc<Store>,
    pub pool: Pool,
    pub jobs: Jobs,
    pub throttle: Throttle,
    pub limits: ratelimit::RateLimits,
    pub metrics: metrics::Metrics,
    /// Argon2id hashes computed at once.
    pub hashing: tokio::sync::Semaphore,
    /// While no user exists: the one-time token that makes the first admin
    /// (`POST /api/setup`), printed to the log at start.
    pub setup_token: Mutex<Option<String>>,
    /// The data directory's lock (`config::LOCK_FILE`), held until
    /// [`serve`] ends: one server per data directory.
    data_lock: Mutex<Option<std::fs::File>>,
}

impl AppState {
    /// Locks and prepares the data directory and opens the metadata store;
    /// imports a stop interrupted go on, other jobs it interrupted are
    /// marked so, and uploads no import goes on with are deleted. Refused
    /// while another server has the data directory (it would take that
    /// one's running jobs for interrupted ones).
    pub fn new(config: Config) -> Result<Arc<AppState>, String> {
        let dir = config.data_dir.display().to_string();
        let data_lock = config::lock_data_dir(&config).map_err(|e| format!("data directory {dir}: {e}"))?;
        config::prepare_data_dir(&config).map_err(|e| format!("data directory {dir}: {e}"))?;
        let store =
            Arc::new(Store::open(&config.store_path()).map_err(|e| format!("metadata store in {dir}: {e}"))?);
        let no_users = store.user_count().map_err(|e| e.message)? == 0;
        let state = Arc::new(AppState {
            pool: Pool::new(config.dbs_dir()),
            jobs: Jobs::new(store.clone()).map_err(|e| e.message)?,
            throttle: Throttle::new(config.login_attempts, config.login_window),
            limits: ratelimit::RateLimits::default(),
            metrics: metrics::Metrics::default(),
            hashing: tokio::sync::Semaphore::new(config.hash_concurrency.max(1)),
            setup_token: Mutex::new(no_users.then(secret::new_token)),
            data_lock: Mutex::new(Some(data_lock)),
            store,
            config,
        });
        routes::imports::resume_interrupted(&state).map_err(|e| format!("data directory {dir}: {e}"))?;
        Ok(state)
    }

    /// The setup token, while the first admin is still to be made.
    pub fn setup_token(&self) -> Option<String> {
        self.setup_token.lock().expect("setup token lock").clone()
    }

    /// Open database `id` (opened on first use) for a request that reads or
    /// changes its games: "busy" (503) while it's being compacted, rather
    /// than waiting minutes for it. Its access must be checked first.
    pub fn open_db(&self, id: &str) -> ApiResult<Arc<pool::OpenDb>> {
        let open = self.pool.get(id)?;
        if open.is_compacting() {
            return Err(ApiError::busy("The database is being compacted: try again in a moment"));
        }
        Ok(open)
    }
}

/// Runs `f` on the blocking pool: the core crates and the metadata store
/// are synchronous.
pub async fn blocking<T: Send + 'static>(f: impl FnOnce() -> ApiResult<T> + Send + 'static) -> ApiResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(ApiError::internal)?
}

const CSP: &str = "default-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; \
                   connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

async fn security_headers(req: Request, next: Next) -> Response {
    let api = req.uri().path().starts_with("/api/");
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    if api {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}

/// Whether an `Origin` header names this server: the public URL's origin
/// when one is set, else the host the request was sent to.
fn same_origin(config: &Config, origin: &str, headers: &HeaderMap) -> bool {
    let origin = origin.to_ascii_lowercase();
    if let Some(public) = config.public_origin() {
        return origin == public;
    }
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).map(str::to_ascii_lowercase);
    origin.split_once("://").map(|(_, h)| h.to_owned()) == host
}

/// Refuses state-changing requests from other origins (on top of the
/// SameSite cookie and the CSRF token).
async fn check_origin(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    if !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS)
        && let Some(origin) = req.headers().get(header::ORIGIN)
    {
        let ok = origin.to_str().is_ok_and(|o| same_origin(&state.config, o, req.headers()));
        if !ok {
            return ApiError::forbidden("Cross-origin request refused").into_response();
        }
    }
    next.run(req).await
}

/// Whether a request comes with credentials, working or not.
fn has_credentials(headers: &HeaderMap) -> bool {
    headers.contains_key(header::AUTHORIZATION) || auth::cookie(headers, auth::SESSION_COOKIE).is_some()
}

/// Takes one request from the client address's bucket (the limit for
/// requests without a login).
fn take_anon(state: &AppState, addr: &str) -> ApiResult<()> {
    match state.config.rate_anon {
        Some(n) => state.limits.take(&format!("ip:{addr}"), n),
        None => Ok(()),
    }
}

/// Notes the client's address in the request (`audit::ClientAddr`): the
/// peer's, or a trusted proxy's `X-Forwarded-For`.
async fn client_addr(State(state): State<Arc<AppState>>, mut req: Request, next: Next) -> Response {
    let peer = req
        .extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map_or_else(|| SocketAddr::from(([0, 0, 0, 0], 0)), |c| c.0);
    let addr = auth::client_ip(&state.config, req.headers(), peer);
    // API requests without credentials count against the address. With
    // them, `Auth` counts them against the user, or against the address
    // when they don't work; routes that never look at them count them
    // against the address too (`anonymous_limit`).
    let api = req.uri().path().starts_with("/api/") && req.uri().path() != "/api/health";
    if api
        && !has_credentials(req.headers())
        && let Err(limited) = take_anon(&state, &addr)
    {
        return limited.into_response();
    }
    req.extensions_mut().insert(audit::ClientAddr(addr));
    next.run(req).await
}

/// For the routes anyone may call without logging in (login, setup,
/// sign-up, invites, password resets), which never check credentials: a
/// request that comes with some counts against the address all the same
/// (`client_addr` counted those without). Else any `Authorization` header
/// would get round the address's limit there.
pub(crate) async fn anonymous_limit(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    if has_credentials(req.headers()) {
        let addr = req.extensions().get::<audit::ClientAddr>().map(|c| c.0.clone()).unwrap_or_default();
        if let Err(limited) = take_anon(&state, &addr) {
            return limited.into_response();
        }
    }
    next.run(req).await
}

async fn log_request(req: Request, next: Next) -> Response {
    let (method, path) = (req.method().clone(), req.uri().path().to_owned());
    let start = Instant::now();
    let res = next.run(req).await;
    log::debug!("{method} {path} {} {} ms", res.status().as_u16(), start.elapsed().as_millis());
    res
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .nest("/api", routes::api(state.clone()))
        .route("/metrics", axum::routing::get(metrics::serve))
        .fallback(ui::serve)
        .layer(middleware::from_fn_with_state(state.clone(), check_origin))
        .layer(middleware::from_fn_with_state(state.clone(), client_addr))
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn(log_request))
        .with_state(state)
}

/// Serves until `shutdown` resolves, then cancels the running jobs and
/// waits for them (imports stop between two batches, keeping what they
/// committed), and lets the data directory go.
pub async fn serve(
    listener: tokio::net::TcpListener,
    state: Arc<AppState>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    if let Some(token) = state.setup_token() {
        let addr = listener.local_addr()?;
        log::warn!(
            "No users yet. Make the first admin in the web UI (http://{addr}/) with the setup token {token}, \
             or run `chess-server admin create NAME`."
        );
    }
    let housekeeping = tokio::spawn(housekeeping::run(state.clone()));
    let app = router(state.clone()).into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, app).with_graceful_shutdown(shutdown).await?;
    housekeeping.abort();
    let st = state.clone();
    let _ = tokio::task::spawn_blocking(move || st.jobs.stop_all()).await;
    state.data_lock.lock().expect("data lock").take();
    Ok(())
}
