//! The JSON API under `/api` (listed in `docs/server.md`).

mod account;
mod admin;
mod databases;
mod export;
pub(crate) mod games;
pub(crate) mod imports;
pub(crate) mod jobs;
mod maintenance;
mod search;
mod transfers;

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, patch, post, put};

use crate::AppState;
use crate::error::{ApiError, ApiResult};

pub fn api() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(account::health))
        .route("/setup", get(account::setup_status).post(account::setup))
        .route("/login", post(account::login))
        .route("/logout", post(account::logout))
        .route("/me", get(account::me))
        .route("/me/password", post(account::change_password))
        .route("/me/account", get(account::account))
        .route("/me/email", put(account::change_email))
        .route("/signup", post(account::signup))
        .route("/invites/{token}", get(account::invite_info))
        .route("/password-reset", post(account::reset_password))
        .route("/password-reset/request", post(account::forgot_password))
        .route("/transfers", get(transfers::list))
        .route("/transfers/{db}/accept", post(transfers::accept))
        .route("/transfers/{db}/decline", post(transfers::decline))
        .route("/tokens", get(account::tokens).post(account::create_token))
        .route("/tokens/{token}", delete(account::delete_token))
        .route("/databases", get(databases::list).post(databases::create))
        .route("/databases/{db}", get(databases::info).patch(databases::update).delete(databases::remove))
        .route("/databases/{db}/shares", get(databases::shares))
        .route("/databases/{db}/transfer", put(transfers::offer).delete(transfers::withdraw))
        .route("/databases/{db}/shares/{username}", put(databases::set_share).delete(databases::remove_share))
        .route("/databases/{db}/games", get(games::page))
        .route("/databases/{db}/games/{game}", get(games::game))
        .route("/databases/{db}/export.pgn", get(export::export))
        .route("/databases/{db}/deleted", post(maintenance::mark_deleted))
        .route("/databases/{db}/duplicates", post(maintenance::find))
        .route("/databases/{db}/duplicates/{job}", get(maintenance::groups))
        .route("/databases/{db}/duplicates/{job}/delete", post(maintenance::delete_found))
        .route("/databases/{db}/compact", post(maintenance::compact))
        .route("/databases/{db}/tree", get(games::tree))
        .route("/databases/{db}/lines", get(games::lines))
        // The upload limits itself (`Config::max_upload`).
        .route("/databases/{db}/import", post(imports::upload).layer(DefaultBodyLimit::disable()))
        .route("/databases/{db}/import-file", post(imports::import_file))
        .route("/search", get(search::search))
        .route("/jobs/{job}", get(jobs::job))
        .route("/jobs/{job}/cancel", post(jobs::cancel))
        .route("/admin/users", get(admin::users).post(admin::create_user))
        .route("/admin/users/{user}", patch(admin::update_user).delete(admin::delete_user))
        .route("/admin/users/{user}/password", post(admin::set_password))
        .route("/admin/databases", get(admin::databases))
        .route("/admin/databases/{db}", patch(admin::update_database).delete(admin::delete_database))
        .route("/admin/users/{user}/databases", get(admin::user_databases))
        .route("/admin/users/{user}/reset-link", post(admin::reset_link))
        .route("/admin/invites", get(admin::invites).post(admin::create_invite))
        .route("/admin/invites/{invite}", delete(admin::delete_invite))
        .route("/admin/jobs", get(admin::jobs))
        .route("/admin/jobs/{job}/cancel", post(admin::cancel_job))
        .route("/admin/disk", get(admin::disk))
        .route("/admin/import-files", get(admin::import_files))
        .fallback(|| async { ApiError::not_found() })
}

/// A JSON request body, read once the caller's access was checked (an
/// extractor would read it first); an empty body is `T`'s default.
pub fn json_body<T: serde::de::DeserializeOwned + Default>(body: &[u8]) -> ApiResult<T> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|e| ApiError::bad_request(format!("Bad request body: {e}")))
}

/// 1 to 32 letters, digits, `.`, `_` or `-`, starting with a letter or digit.
pub fn check_username(name: &str) -> ApiResult<()> {
    let ok = (1..=32).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && name.as_bytes()[0].is_ascii_alphanumeric();
    if ok {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "A username has 1 to 32 letters, digits, '.', '_' or '-', and starts with a letter or digit",
        ))
    }
}

/// A display name (of a database or a token): trimmed, 1 to 100
/// characters, no control characters.
pub fn check_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        return Err(ApiError::bad_request("A name has 1 to 100 characters, without control characters"));
    }
    Ok(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames_and_names() {
        for good in ["alice", "Bob.K", "a", "x_1-2"] {
            check_username(good).unwrap();
        }
        for bad in ["", ".a", "a b", "é", "a/b", &"a".repeat(33)] {
            assert!(check_username(bad).is_err(), "{bad}");
        }
        assert_eq!(check_name("  My games ").unwrap(), "My games");
        assert!(check_name(" ").is_err());
        assert!(check_name("a\nb").is_err());
    }
}
