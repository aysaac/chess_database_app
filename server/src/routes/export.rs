//! PGN export (IO-02, IO-11): a database's games, or those matching the
//! games route's criteria, streamed as one PGN file. Games are read and
//! written a few hundred at a time on the blocking pool and passed on
//! through a small channel, so the file is never held in memory, and a
//! client that goes away stops the export. A compaction meanwhile
//! renumbers the games: the export stops then, with an error (the client
//! sees a broken download rather than a file mixing games from before and
//! after it).
//!
//! A copy of the database on another computer (NET-03: the desktop app's
//! "From my server") takes the games added since its last update by their
//! numbers (`ExportRange`): every answer says, in `X-Next-Id`, the first
//! game it didn't cover, and in `X-Id-Epoch`, the numbering it used.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chess_core::pgn::write_game;
use http_body::Frame;
use tokio::sync::mpsc;

use crate::access::require;
use crate::audit::Target;
use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::listing::{GameList, ListFor, Listing, compacted_since, listing, renumbered};
use crate::routes::maintenance::stale_ids;
use crate::wire::{Access, ExportRange, GamesAnswer, GamesQuery, SortKeyDto};
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

/// The first game an answer didn't cover (`ExportRange`).
pub const NEXT_ID_HEADER: &str = "x-next-id";
/// The database's `idEpoch` the answer's numbers are at.
pub const ID_EPOCH_HEADER: &str = "x-id-epoch";

/// The first position in `list`, which is in id order, whose game is `id`
/// or after it.
fn position_of(list: &GameList, id: u32) -> usize {
    let (mut lo, mut hi) = (0, list.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if list.slice(mid, mid + 1).first().is_some_and(|&g| g < id) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

/// `GET /api/databases/{id}/export.pgn`: the games the games route would
/// list for the same criteria (all of them by default; never the deleted
/// ones unless asked), in its sort order, as PGN. With `idFrom` or `idTo`,
/// only those numbered in that range.
pub async fn export(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
    Query(q): Query<GamesQuery>,
    Query(range): Query<ExportRange>,
) -> ApiResult<Response> {
    let ranged = range.id_from.is_some() || range.id_to.is_some();
    if ranged && (q.sort.is_some_and(|s| s != SortKeyDto::Id) || q.desc) {
        return Err(ApiError::bad_request("idFrom and idTo take the games in id order: leave out sort"));
    }
    let st = state.clone();
    let (meta, open, ids, next, epoch) = blocking(move || {
        let (meta, _) = require(&st.store, &auth.user, &id, Access::Read)?;
        let open = st.open_db(&id)?;
        // The games there were before the list was made: a range ends
        // there, so a game added meanwhile comes with the next one.
        let (count, compactions) = {
            let db = open.lock();
            (db.count(), db.compactions())
        };
        let listing = listing(&st, &open, &meta, &auth.user, &q, ListFor::Page)?;
        // The numbering now, holding the database: a compaction bumps the
        // epoch holding it too, before it numbers the games again.
        let epoch = {
            let db = open.lock();
            if db.compactions() != compactions {
                return Err(renumbered());
            }
            st.store.database(&id)?.ok_or_else(ApiError::not_found)?.id_epoch
        };
        if range.id_epoch.is_some_and(|e| e != epoch) {
            return Err(stale_ids());
        }
        let from = range.id_from.unwrap_or(0);
        let next = range.id_to.unwrap_or(u32::MAX).min(count).max(from);
        log::info!("{} exports database {}", auth.user.username, meta.id);
        st.audit("export", Some(&auth.user), Target::Database(&meta), None, Some(&auth.client));
        Ok((meta, open, listing, if ranged { Some((from, next)) } else { None }, epoch))
    })
    .await?;
    let ids = match ids {
        Listing::Ready(ids) => ids,
        Listing::Job(job) => {
            return Ok((StatusCode::ACCEPTED, Json(GamesAnswer::Job { job })).into_response());
        }
    };
    // Positions in the list: all of it, or the range's games.
    let (start, end) = match next {
        Some((from, to)) => (position_of(&ids, from), position_of(&ids, to)),
        None => (0, ids.len()),
    };
    let (tx, rx) = mpsc::channel(CHUNKS_QUEUED);
    tokio::task::spawn_blocking(move || {
        let mut buf = Vec::with_capacity(CHUNK_BYTES + 64 * 1024);
        let mut from = start;
        while from < end {
            let part = ids.slice(from, (from + GAMES_PER_READ).min(end));
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
    h.insert(HeaderName::from_static(ID_EPOCH_HEADER), HeaderValue::from(epoch));
    if let Some((_, to)) = next {
        h.insert(HeaderName::from_static(NEXT_ID_HEADER), HeaderValue::from(to));
    }
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
