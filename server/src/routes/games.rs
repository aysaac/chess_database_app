//! Reading a database: pages of the game list (sorted, filtered by header
//! search), one game, and the position tree with its top games and lines
//! (the app's Reference tab, OPEN-14).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use chess_core::pgn::write_game;
use chess_core::position;
use chess_core::shakmaty::Chess;
use chess_db::reference::LINES_JOB_GAMES;
use chess_db::{
    DbError, GameId, HeaderQuery, IndexPlan, NoProgress, build_sort, find_position, page_ids,
    reference_lines, reference_overview, search_headers, sorted_subset, update_position_index,
};

use crate::access::require;
use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::jobs::{thousands, to_job_error};
use crate::pool::{LinesKey, OpenDb, writer_busy};
use crate::store::{DbMeta, User};
use crate::wire::{
    Access, FenQuery, GameDto, GamePage, GameRowDto, GamesQuery, HeadersDto, OpeningDto, ReferenceDto,
    ReferenceLinesDto, lines_dto, notation_of, reference_dto,
};
use crate::{AppState, blocking};

/// The ids of rows `offset..offset + limit` of the list `q` asks for, and
/// the list's length. Blocking.
fn page_of(open: &OpenDb, q: &GamesQuery) -> Result<(Vec<GameId>, u32), DbError> {
    let spec = q.sort_spec();
    if !open.lock().has_sort(spec) {
        build_sort(&open.db, spec, &NoProgress)?;
    }
    let (offset, limit) = (q.offset, q.page_limit());
    let header = q.header_query();
    if header == (HeaderQuery { skip_deleted: true, ..HeaderQuery::default() }) {
        let db = open.lock();
        return Ok((page_ids(&db, spec, offset, limit)?, db.count()));
    }
    let generation = open.lock().generation();
    let cached = open.cache.lock().expect("cache lock").search(&header, generation);
    let found = match cached {
        Some(ids) => ids,
        None => {
            let ids = Arc::new(search_headers(&open.db, &header, None, &NoProgress)?);
            open.cache.lock().expect("cache lock").put_search(header.clone(), generation, ids.clone());
            ids
        }
    };
    let db = open.lock();
    let epoch = db.sort_epoch();
    let cached = open.cache.lock().expect("cache lock").order(&header, spec, generation, epoch);
    let ordered = match cached {
        Some(ids) => ids,
        None => {
            let ids = Arc::new(sorted_subset(&db, spec, &found)?);
            open.cache.lock().expect("cache lock").put_order(header, spec, generation, epoch, ids.clone());
            ids
        }
    };
    let end = offset.saturating_add(limit).min(ordered.len() as u32);
    let ids = ordered.get(offset as usize..end as usize).map(<[GameId]>::to_vec).unwrap_or_default();
    Ok((ids, ordered.len() as u32))
}

/// A page of the game list: `?sort=white&desc=true&offset=0&limit=50`, plus
/// any header criteria (`white=`, `black=`, `text=`, `yearFrom=`…).
pub async fn page(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
    Query(q): Query<GamesQuery>,
) -> ApiResult<Json<GamePage>> {
    blocking(move || {
        require(&state.store, &auth.user, &id, Access::Read)?;
        let open = state.pool.get(&id)?;
        let (ids, total) = match page_of(&open, &q) {
            // A game replaced since the sort was built (not in phase 1, which only appends).
            Err(DbError::NotSorted) => page_of(&open, &q)?,
            other => other?,
        };
        let rows = open.lock().rows(&ids)?.into_iter().map(GameRowDto::from).collect();
        Ok(Json(GamePage { total, offset: q.offset, rows }))
    })
    .await
}

/// One game: its row, headers, PGN and move tree.
pub async fn game(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((id, game)): Path<(String, String)>,
) -> ApiResult<Json<GameDto>> {
    blocking(move || {
        require(&state.store, &auth.user, &id, Access::Read)?;
        let game: GameId = game.parse().map_err(|_| ApiError::not_found())?;
        let open = state.pool.get(&id)?;
        let (g, row) = {
            let db = open.lock();
            if game >= db.count() {
                return Err(ApiError::not_found());
            }
            (db.read_game(game)?, db.row(game)?)
        };
        Ok(Json(GameDto {
            id: game,
            row: row.into(),
            headers: HeadersDto::from(&g.headers),
            start_fen: g.setup.as_ref().map(position::to_fen),
            pgn: write_game(&g),
            notation: notation_of(&g),
            opening: chess_core::openings::classify(&g).map(OpeningDto::from),
        }))
    })
    .await
}

fn parse_fen(q: &FenQuery) -> ApiResult<Chess> {
    match q.fen.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
        None => Ok(Chess::default()),
        Some(fen) => position::from_fen(fen).map_err(|e| ApiError::bad_request(format!("Bad FEN: {e}"))),
    }
}

/// Starts bringing the position index up to date when it isn't (after a
/// cancelled import, or a crash in one), unless a job writes already.
/// Blocking.
pub fn ensure_index(state: &AppState, open: &Arc<OpenDb>, meta: &DbMeta, user: &User) {
    let mut writer = open.writer.lock().expect("writer lock");
    if writer_busy(&writer, &state.jobs.manager) {
        return;
    }
    let plan = {
        let db = open.lock();
        db.positions().plan(db.count())
    };
    if plan == IndexPlan::UpToDate {
        return;
    }
    let o = open.clone();
    let job =
        state.jobs.spawn("index", "Indexing positions".into(), (&meta.id, &meta.name), user, move |ctx| {
            let s = update_position_index(&o.db, ctx).map_err(to_job_error)?;
            Ok(format!("Indexed the positions of {} games", thousands(u64::from(s.games))))
        });
    *writer = Some(job);
}

/// The position tree at `?fen=` (the start when left out): each move with
/// its games, score, Hot and best players, and the top games. Fast at any
/// position: it reads the position index.
pub async fn tree(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
    Query(q): Query<FenQuery>,
) -> ApiResult<Json<ReferenceDto>> {
    let pos = parse_fen(&q)?;
    blocking(move || {
        let (meta, _) = require(&state.store, &auth.user, &id, Access::Read)?;
        let open = state.pool.get(&id)?;
        ensure_index(&state, &open, &meta, &auth.user);
        let overview = reference_overview(&open.db, &pos)?;
        let indexed = open.lock().positions().is_built();
        Ok(Json(reference_dto(&pos, overview, indexed)))
    })
    .await
}

/// The main line and the most played lines from `?fen=`. They're counted
/// from every game reaching the position, so from 50,000 games on this
/// starts a job and returns it (`kind: "job"`): ask again once it's done.
/// Results are kept until the database changes.
pub async fn lines(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
    Query(q): Query<FenQuery>,
) -> ApiResult<Json<ReferenceLinesDto>> {
    let pos = parse_fen(&q)?;
    blocking(move || {
        let (meta, _) = require(&state.store, &auth.user, &id, Access::Read)?;
        let open = state.pool.get(&id)?;
        ensure_index(&state, &open, &meta, &auth.user);
        let key = {
            let db = open.lock();
            LinesKey {
                position: position::hash(&pos),
                generation: db.generation(),
                indexed: db.positions().covered(),
            }
        };
        {
            let mut cache = open.cache.lock().expect("cache lock");
            if let Some(lines) = cache.lines(&key) {
                return Ok(Json(ReferenceLinesDto::Ready { lines: Box::new(lines) }));
            }
            if let Some((job, games)) = cache.lines_job(&key, &state.jobs.manager) {
                return Ok(Json(ReferenceLinesDto::Job { job, games }));
            }
        }
        let hits = find_position(&open.db, &pos)?;
        let games = hits.entries.len() as u32;
        if hits.entries.len() < LINES_JOB_GAMES {
            let lines = lines_dto(&pos, &reference_lines(&open.db, &pos, &hits, &NoProgress)?);
            open.cache.lock().expect("cache lock").put_lines(key, lines.clone());
            return Ok(Json(ReferenceLinesDto::Ready { lines: Box::new(lines) }));
        }
        let mut cache = open.cache.lock().expect("cache lock");
        // Asked twice at once: the first one's job.
        if let Some((job, games)) = cache.lines_job(&key, &state.jobs.manager) {
            return Ok(Json(ReferenceLinesDto::Job { job, games }));
        }
        let o = open.clone();
        let label = format!("Counting the lines of {} games", thousands(u64::from(games)));
        let job = state.jobs.spawn("lines", label, (&meta.id, &meta.name), &auth.user, move |ctx| {
            let lines = lines_dto(&pos, &reference_lines(&o.db, &pos, &hits, ctx).map_err(to_job_error)?);
            o.cache.lock().expect("cache lock").put_lines(key, lines);
            Ok("Counted the lines".to_owned())
        });
        cache.add_lines_job(key, job, games);
        Ok(Json(ReferenceLinesDto::Job { job, games }))
    })
    .await
}
