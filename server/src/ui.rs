//! The admin web UI (`server/admin/`), embedded by `build.rs`.

use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Response};

include!(concat!(env!("OUT_DIR"), "/admin_assets.rs"));

const NOT_BUILT: &str = "<!doctype html><meta charset=utf-8><title>chess-server</title>\
<h1>The admin UI isn't built into this binary</h1>\
<p>Build it with <code>npm ci &amp;&amp; npm run build</code> in <code>server/admin</code>, then build the server \
again. The API under <code>/api</code> works without it.</p>";

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if !BUILT && path == "index.html" {
        return Html(NOT_BUILT).into_response();
    }
    let Some((_, bytes)) = ASSETS.iter().find(|(p, _)| *p == path) else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };
    let mut res = (*bytes).into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(path)));
    // Vite puts a hash in the names under assets/; the page itself is always asked again.
    let cache = if path.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    res
}
