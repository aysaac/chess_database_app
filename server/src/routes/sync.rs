//! Editing games on the server, and keeping copies on other computers in
//! step with it (NET-03: the desktop app's offline copy), by the games'
//! permanent ids (`chess_db::GameUid`), which edits and compactions keep.
//!
//! - **Editing**: a game is read, replaced (with its deleted flag) by
//!   permanent id, or added. A replace can name the journal's change the
//!   client's copy of the game is at (`baseSeq`), and is refused (409
//!   `changedOnServer`) if the game changed after it: nobody's edit is
//!   overwritten unseen.
//! - **The journal of changed games** (the store's `game_changes`): every
//!   game edited, marked as deleted or restored after it came in gets a
//!   number there, noted holding the database's lock before the change.
//!   Games added (imports, `POST games`) aren't in it: they come at the end
//!   of the numbering, and a copy takes them by number (`sync/games`).
//! - **A copy** takes the games by number a part at a time, with their
//!   permanent ids (`sync/games`, deleted ones too), then from then on the
//!   games changed since the journal's change it's at (`sync/changes`),
//!   each once in its state now. Both read the journal and the games
//!   holding the database's lock, so a change is either in what they
//!   answer or after the `seq` they give. After a compaction, which
//!   renumbers the games, a copy finds where it got to by the permanent
//!   ids of the last games it took (`sync/locate`).

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chess_core::game::Game;
use chess_core::pgn::{PgnReader, write_game};
use chess_db::{Database, GameId, GameUid, PreparedGame};

use crate::access::require;
use crate::audit::Target;
use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::pool::writer_busy;
use crate::routes::games::game_dto;
use crate::routes::imports::{quota_exceeded, room};
use crate::routes::json_body;
use crate::routes::maintenance::{parse_uid, stale_ids};
use crate::wire::{
    Access, AddGame, ChangesDto, ChangesQuery, ExportRange, GameDto, LocateDto, LocateRequest, PutGame,
    SavedGameDto, SyncChangeDto, SyncGameDto, SyncPartDto,
};
use crate::{AppState, blocking};

/// Games in a part of a copy at most.
pub const PART_GAMES: u32 = 2000;
/// Changed games answered at once at most (500 by default).
const MAX_CHANGES: u32 = 2000;
/// Games looked for at once at most.
const MAX_LOCATE: usize = 1000;
/// The largest PGN of one game taken (a heavily annotated game is a few
/// hundred KB).
const MAX_GAME_PGN: usize = 2 << 20;

/// A permanent id in a path: one that isn't one is "not found".
fn path_uid(text: &str) -> ApiResult<GameUid> {
    text.parse().map_err(|_| ApiError::not_found())
}

fn no_such_game() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "noSuchGame", "The database has no such game (any more)")
}

/// The one game a request's PGN holds.
fn one_game(pgn: &str) -> ApiResult<Game> {
    if pgn.len() > MAX_GAME_PGN {
        return Err(ApiError::too_large(format!("A game's PGN is at most {} MB", MAX_GAME_PGN >> 20)));
    }
    let mut parsed: Vec<_> = PgnReader::new(pgn.as_bytes())
        .collect::<Result<_, _>>()
        .map_err(|e| ApiError::bad_request(format!("The PGN can't be read: {e}")))?;
    if parsed.len() != 1 {
        return Err(ApiError::bad_request(format!("The PGN must hold one game, not {}", parsed.len())));
    }
    let p = parsed.remove(0);
    p.game.ok_or_else(|| {
        let why: Vec<_> = p.warnings.iter().map(|w| w.message.as_str()).collect();
        ApiError::bad_request(format!("The game can't be read: {}", why.join("; ")))
    })
}

fn sync_game(db: &Database, id: GameId) -> ApiResult<SyncGameDto> {
    Ok(SyncGameDto {
        id,
        uid: db.uid(id)?.to_string(),
        deleted: db.record(id)?.is_deleted(),
        pgn: write_game(&db.read_game(id)?),
    })
}

/// One game by permanent id (read access): as `games/{n}`, with its number now.
pub async fn get(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((id, uid)): Path<(String, String)>,
) -> ApiResult<Json<GameDto>> {
    blocking(move || {
        require(&state.store, &auth.user, &id, Access::Read)?;
        let uid = path_uid(&uid)?;
        let open = state.open_db(&id)?;
        let mut db = open.lock();
        let game = db.game_by_uid(uid)?.ok_or_else(no_such_game)?;
        let (g, row) = (db.read_game(game)?, db.row(game)?);
        drop(db);
        Ok(Json(game_dto(game, uid, &g, row)))
    })
    .await
}

/// Replaces a game, by permanent id (write access): its moves, comments and
/// headers from one game's PGN, and its deleted flag if given. Refused (409
/// `changedOnServer`) when `baseSeq` is given and the game changed after it.
pub async fn put(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((id, uid)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult<Json<SavedGameDto>> {
    blocking(move || {
        let (meta, _) = require(&state.store, &auth.user, &id, Access::Write)?;
        let uid = path_uid(&uid)?;
        let req: PutGame = json_body(&body)?;
        let game = one_game(&req.pgn)?;
        if room(&state, &meta)? == Some(0) {
            return Err(quota_exceeded());
        }
        let open = state.open_db(&id)?;
        let mut db = open.lock();
        let slot = db.game_by_uid(uid)?.ok_or_else(no_such_game)?;
        if let Some(base) = req.base_seq
            && state.store.changed_since(&id, &uid.to_bytes(), base)?
        {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "changedOnServer",
                "The game changed on the server since your copy of it: read it again",
            ));
        }
        // The journal first, holding the database (see the module's doc).
        let seq = state.store.note_changes(&id, &[uid.to_bytes()])?;
        db.replace(slot, &game)?;
        if let Some(deleted) = req.deleted {
            db.set_deleted(slot, deleted)?;
        }
        drop(db);
        log::info!("{} edited a game of database {}", auth.user.username, meta.id);
        state.audit("games.edit", Some(&auth.user), Target::Database(&meta), None, Some(&auth.client));
        Ok(Json(SavedGameDto { id: slot, uid: uid.to_string(), seq }))
    })
    .await
}

/// Adds a game (write access) from one game's PGN, at the end of the
/// numbering. Refused (409) while an import or index build writes to the
/// database (an import's resuming after a restart counts its games), and
/// (507) at the quota.
pub async fn add(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<SavedGameDto>)> {
    let saved = blocking(move || {
        let (meta, _) = require(&state.store, &auth.user, &id, Access::Write)?;
        let req: AddGame = json_body(&body)?;
        let game = one_game(&req.pgn)?;
        if room(&state, &meta)? == Some(0) {
            return Err(quota_exceeded());
        }
        let open = state.open_db(&id)?;
        let writer = open.writer.lock().expect("writer lock");
        if writer_busy(&writer, &state.jobs.manager) {
            return Err(ApiError::conflict(
                "An import or index build runs on this database: add the game once it's done",
            ));
        }
        let mut db = open.lock();
        let slot = db.append_prepared(&[PreparedGame::new(&game)])?.start;
        let uid = db.uid(slot)?;
        drop(db);
        drop(writer);
        let seq = state.store.latest_change(&id)?;
        log::info!("{} added a game to database {}", auth.user.username, meta.id);
        state.audit("games.add", Some(&auth.user), Target::Database(&meta), None, Some(&auth.client));
        Ok(SavedGameDto { id: slot, uid: uid.to_string(), seq })
    })
    .await?;
    Ok((StatusCode::CREATED, Json(saved)))
}

/// A part of a copy (read access): the games numbered `idFrom..idTo` (at
/// most [`PART_GAMES`]), deleted ones too, with their permanent ids; `next`
/// is the first number not covered, `seq` the journal's latest change then.
/// `idEpoch` is refused (409 `staleIds`) once a compaction numbered the
/// games again.
pub async fn games(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
    Query(q): Query<ExportRange>,
) -> ApiResult<Json<SyncPartDto>> {
    blocking(move || {
        let (meta, _) = require(&state.store, &auth.user, &id, Access::Read)?;
        let from = q.id_from.unwrap_or(0);
        let to = q.id_to.unwrap_or(u32::MAX).min(from.saturating_add(PART_GAMES));
        let open = state.open_db(&id)?;
        let db = open.lock();
        // Read holding the database: a compaction bumps it holding it too.
        let epoch = state.store.database(&id)?.ok_or_else(ApiError::not_found)?.id_epoch;
        if q.id_epoch.is_some_and(|e| e != epoch) {
            return Err(stale_ids());
        }
        let seq = state.store.latest_change(&id)?;
        let end = to.min(db.count());
        let games = (from..end).map(|g| sync_game(&db, g)).collect::<ApiResult<Vec<_>>>()?;
        drop(db);
        log::debug!("{} copies games {from}..{end} of database {}", auth.user.username, meta.id);
        Ok(Json(SyncPartDto { epoch, next: end.max(from), seq, games }))
    })
    .await
}

/// The games changed after the journal's change `since` (read access),
/// each once in its state now (`game: null` once a compaction removed it),
/// in the order of their latest change; at most `limit` (500, at most
/// 2,000), `more` saying there are others after `latest`.
pub async fn changes(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
    Query(q): Query<ChangesQuery>,
) -> ApiResult<Json<ChangesDto>> {
    blocking(move || {
        require(&state.store, &auth.user, &id, Access::Read)?;
        let limit = q.limit.unwrap_or(500).clamp(1, MAX_CHANGES);
        let open = state.open_db(&id)?;
        let mut db = open.lock();
        let epoch = state.store.database(&id)?.ok_or_else(ApiError::not_found)?.id_epoch;
        let found = state.store.changes_since(&id, q.since, limit + 1)?;
        let more = found.len() > limit as usize;
        let latest =
            if more { found[limit as usize - 1].1 } else { state.store.latest_change(&id)?.max(q.since) };
        let mut games = Vec::with_capacity(found.len().min(limit as usize));
        for (bytes, seq) in found.into_iter().take(limit as usize) {
            let uid = GameUid::from_bytes(bytes);
            let game = match db.game_by_uid(uid)? {
                Some(slot) => Some(sync_game(&db, slot)?),
                None => None,
            };
            games.push(SyncChangeDto { uid: uid.to_string(), seq, game });
        }
        Ok(Json(ChangesDto { latest, more, epoch, games }))
    })
    .await
}

/// Where games are now, by permanent id (read access): a copy finding
/// where it got to after a compaction.
pub async fn locate(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Json<LocateDto>> {
    blocking(move || {
        require(&state.store, &auth.user, &id, Access::Read)?;
        let req: LocateRequest = json_body(&body)?;
        if req.uids.len() > MAX_LOCATE {
            return Err(ApiError::bad_request(format!("Look for at most {MAX_LOCATE} games at once")));
        }
        let uids = req.uids.iter().map(|u| parse_uid(u)).collect::<ApiResult<Vec<_>>>()?;
        let open = state.open_db(&id)?;
        let mut db = open.lock();
        let epoch = state.store.database(&id)?.ok_or_else(ApiError::not_found)?.id_epoch;
        let ids = uids.into_iter().map(|u| db.game_by_uid(u)).collect::<Result<Vec<_>, _>>()?;
        Ok(Json(LocateDto { epoch, ids }))
    })
    .await
}
