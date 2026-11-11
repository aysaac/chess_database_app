//! Metrics in Prometheus' text format at `/metrics` (NET-03), written by
//! hand: there only when `CHESS_SERVER_METRICS_TOKEN` is set, and only for
//! a request bearing it.
//!
//! Requests are counted by route template (`/api/databases/{db}/games`,
//! never a path with an id in it), method and status, with their latency;
//! besides, the open databases, the running jobs by kind, the sizes of the
//! store and the databases, and the games, databases and users there are.
//! **No label holds a database name or id, or a username.**

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::secret::ct_eq;
use crate::store::Store;
use crate::{AppState, blocking};

/// Upper bounds of the latency histogram's buckets, in seconds.
const BUCKETS: [f64; 12] = [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0];

#[derive(Default)]
struct Histogram {
    counts: [u64; BUCKETS.len()],
    count: u64,
    sum: f64,
}

#[derive(Default)]
pub struct Metrics {
    /// (route, method, status) → requests.
    requests: Mutex<BTreeMap<(String, String, u16), u64>>,
    /// (route, method) → latency.
    latency: Mutex<BTreeMap<(String, String), Histogram>>,
}

impl Metrics {
    fn record(&self, route: &str, method: &str, status: u16, seconds: f64) {
        *self
            .requests
            .lock()
            .expect("metrics lock")
            .entry((route.into(), method.into(), status))
            .or_default() += 1;
        let mut latency = self.latency.lock().expect("metrics lock");
        let h = latency.entry((route.into(), method.into())).or_default();
        h.count += 1;
        h.sum += seconds;
        for (i, &le) in BUCKETS.iter().enumerate() {
            if seconds <= le {
                h.counts[i] += 1;
            }
        }
    }
}

/// Counts every request by its route template (a middleware of every route).
pub async fn record(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let route = req.extensions().get::<MatchedPath>().map_or("unmatched", |p| p.as_str()).to_owned();
    let method = req.method().as_str().to_owned();
    let start = Instant::now();
    let res = next.run(req).await;
    state.metrics.record(&route, &method, res.status().as_u16(), start.elapsed().as_secs_f64());
    res
}

/// A label value, escaped.
fn label(v: &str) -> String {
    v.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

fn gauge(out: &mut String, name: &str, help: &str, value: impl std::fmt::Display) {
    let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {value}");
}

/// The text page. Blocking: it reads the store and the databases' sizes.
fn render(state: &AppState) -> String {
    let mut out = String::new();
    {
        let requests = state.metrics.requests.lock().expect("metrics lock");
        let _ = writeln!(
            out,
            "# HELP chess_server_http_requests_total HTTP requests, by route template, method and status."
        );
        let _ = writeln!(out, "# TYPE chess_server_http_requests_total counter");
        for ((route, method, status), n) in requests.iter() {
            let _ = writeln!(
                out,
                "chess_server_http_requests_total{{route=\"{}\",method=\"{}\",status=\"{status}\"}} {n}",
                label(route),
                label(method)
            );
        }
    }
    {
        let latency = state.metrics.latency.lock().expect("metrics lock");
        let name = "chess_server_http_request_duration_seconds";
        let _ = writeln!(out, "# HELP {name} How long requests took, by route template and method.");
        let _ = writeln!(out, "# TYPE {name} histogram");
        for ((route, method), h) in latency.iter() {
            let labels = format!("route=\"{}\",method=\"{}\"", label(route), label(method));
            for (le, n) in BUCKETS.iter().zip(h.counts) {
                let _ = writeln!(out, "{name}_bucket{{{labels},le=\"{le}\"}} {n}");
            }
            let _ = writeln!(out, "{name}_bucket{{{labels},le=\"+Inf\"}} {}", h.count);
            let _ = writeln!(out, "{name}_sum{{{labels}}} {}", h.sum);
            let _ = writeln!(out, "{name}_count{{{labels}}} {}", h.count);
        }
    }
    gauge(&mut out, "chess_server_open_databases", "Databases open now.", state.pool.open_count());
    let mut running: BTreeMap<String, u64> = BTreeMap::new();
    for s in state.jobs.manager.list().iter().filter(|s| !s.state.is_finished()) {
        *running.entry(s.kind.clone()).or_default() += 1;
    }
    let _ = writeln!(out, "# HELP chess_server_jobs_running Jobs running now, by kind.");
    let _ = writeln!(out, "# TYPE chess_server_jobs_running gauge");
    for kind in ["import", "index", "lines", "duplicates", "compact", "backup", "twic"] {
        running.entry(kind.into()).or_default();
    }
    for (kind, n) in &running {
        let _ = writeln!(out, "chess_server_jobs_running{{kind=\"{}\"}} {n}", label(kind));
    }
    gauge(
        &mut out,
        "chess_server_metadata_store_bytes",
        "Size of the metadata store (server.sqlite and its WAL).",
        Store::size_bytes(&state.config.store_path()),
    );
    let dbs = state.store.all_databases().unwrap_or_default();
    let (mut games, mut bytes) = (0u64, 0u64);
    for d in &dbs {
        let (g, b) = state.pool.stats(&d.id);
        games += u64::from(g);
        bytes += b;
    }
    gauge(&mut out, "chess_server_databases", "Databases on the server.", dbs.len());
    gauge(&mut out, "chess_server_databases_bytes", "Size of all databases on disk.", bytes);
    gauge(&mut out, "chess_server_games", "Games in all databases.", games);
    gauge(&mut out, "chess_server_users", "Accounts.", state.store.user_count().unwrap_or(0));
    out
}

/// `GET /metrics` with `Authorization: Bearer <CHESS_SERVER_METRICS_TOKEN>`;
/// "not found" when no token is set.
pub async fn serve(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some(token) = state.config.metrics_token.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let sent = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if !ct_eq(sent.trim().as_bytes(), token.as_bytes()) {
        let mut res = StatusCode::UNAUTHORIZED.into_response();
        res.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        return res;
    }
    match blocking(move || Ok(render(&state))).await {
        Ok(text) => {
            let mut res = text.into_response();
            res.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
            );
            res
        }
        Err(e) => e.into_response(),
    }
}
