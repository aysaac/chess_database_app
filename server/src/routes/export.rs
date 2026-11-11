//! PGN export (IO-02, IO-11): a database's games, or those matching the
//! games route's criteria, streamed as one PGN file. Games are read and
//! written a few hundred at a time on the blocking pool and passed on
//! through a small channel, so the file is never held in memory, and a
//! client that goes away stops the export. A compaction meanwhile
//! renumbers the games: the export stops then, with an error (the client
//! sees a broken download rather than a file mixing games from before and
//! after it).

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chess_core::pgn::write_game;
use http_body::Frame;
use tokio::sync::mpsc;

use crate::access::require;
use crate::audit::Target;
use crate::auth::Auth;
use crate::error::ApiResult;
use crate::listing::{ListFor, Listing, compacted_since, listing};
use crate::wire::{Access, GamesAnswer, GamesQuery};
use crate::{AppState, blocking};

/// Games read per lock of the database.
const GAMES_PER_READ: usize = 200;
/// Bytes sent at once (a chunk of the response).
const CHUNK_BYTES: usize = 256 * 1024;
/// Chunks waiting to be sent at most: what an export holds in memory.
const CHUNKS_QUEUED: usize = 4;

/// A response body fed by the blocking thread writing the PGN.
struct ChannelBody(mpsc::Receiver<Result<Bytes, std::io::Error>>);

impl http_body::Body for ChannelBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, std::io::Error>>> {
        self.0.poll_recv(cx).map(|chunk| chunk.map(|r| r.map(Frame::data)))
    }
}

/// `Content-Disposition` for `name.pgn`: an ASCII file name, and the name
/// as typed for browsers that read `filename*` (RFC 6266).
pub fn attachment(name: &str) -> HeaderValue {
    let ascii: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || " -_.()".contains(c) { c } else { '_' })
        .collect();
    let ascii = ascii.trim();
    let ascii = if ascii.is_empty() { "games" } else { ascii };
    let encoded: String = format!("{name}.pgn")
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    HeaderValue::from_str(&format!("attachment; filename=\"{ascii}.pgn\"; filename*=UTF-8''{encoded}"))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment; filename=\"games.pgn\""))
}

/// `GET /api/databases/{id}/export.pgn`: the games the games route would
/// list for the same criteria (all of them by default; never the deleted
/// ones unless asked), in its sort order, as PGN.
pub async fn export(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
    Query(q): Query<GamesQuery>,
) -> ApiResult<Response> {
    let st = state.clone();
    let (meta, open, ids) = blocking(move || {
        let (meta, _) = require(&st.store, &auth.user, &id, Access::Read)?;
        let open = st.open_db(&id)?;
        let listing = listing(&st, &open, &meta, &auth.user, &q, ListFor::Page)?;
        log::info!("{} exports database {}", auth.user.username, meta.id);
        st.audit("export", Some(&auth.user), Target::Database(&meta), None, Some(&auth.client));
        Ok((meta, open, listing))
    })
    .await?;
    let ids = match ids {
        Listing::Ready(ids) => ids,
        Listing::Job(job) => {
            return Ok((StatusCode::ACCEPTED, Json(GamesAnswer::Job { job })).into_response());
        }
    };
    let (tx, rx) = mpsc::channel(CHUNKS_QUEUED);
    tokio::task::spawn_blocking(move || {
        let mut buf = Vec::with_capacity(CHUNK_BYTES + 64 * 1024);
        let mut from = 0;
        while from < ids.len() {
            let part = ids.slice(from, from + GAMES_PER_READ);
            from += GAMES_PER_READ;
            let games = {
                let db = open.lock();
                if compacted_since(&db, &ids) {
                    log::info!("export of {} stopped: the database was compacted meanwhile", open.id);
                    let why = "the database was compacted during the export, which renumbers its games";
                    let _ = tx.blocking_send(Err(std::io::Error::other(why)));
                    return;
                }
                part.iter().map(|&g| db.read_game(g)).collect::<Result<Vec<_>, _>>()
            };
            let games = match games {
                Ok(g) => g,
                Err(e) => {
                    log::error!("export of {}: {e}", open.id);
                    let _ = tx.blocking_send(Err(std::io::Error::other("the export failed")));
                    return;
                }
            };
            for g in &games {
                buf.extend_from_slice(write_game(g).as_bytes());
            }
            if buf.len() >= CHUNK_BYTES {
                let chunk =
                    Bytes::from(std::mem::replace(&mut buf, Vec::with_capacity(CHUNK_BYTES + 64 * 1024)));
                // The client went away: stop.
                if tx.blocking_send(Ok(chunk)).is_err() {
                    return;
                }
            }
        }
        if !buf.is_empty() {
            let _ = tx.blocking_send(Ok(Bytes::from(buf)));
        }
    });
    let mut res = Body::new(ChannelBody(rx)).into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/x-chess-pgn; charset=utf-8"));
    h.insert(header::CONTENT_DISPOSITION, attachment(&meta.name));
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names() {
        assert_eq!(
            attachment("Mega 2019").to_str().unwrap(),
            "attachment; filename=\"Mega 2019.pgn\"; filename*=UTF-8''Mega%202019.pgn"
        );
        let v = attachment("Réti \"x\"/y");
        assert!(v.to_str().unwrap().starts_with("attachment; filename=\"R_ti _x__y.pgn\""), "{v:?}");
        assert!(v.to_str().unwrap().ends_with("R%C3%A9ti%20%22x%22%2Fy.pgn"));
        assert!(attachment("ü").to_str().unwrap().contains("filename=\"_.pgn\""));
    }
}
