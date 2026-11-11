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
use chess_db::{GameId, IndexPlan, NoProgress, find_position, reference_lines, reference_overview};

use crate::access::require;
use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::jobs::{thousands, to_job_error};
use crate::listing::{ListFor, Listing, compacted_since, listing, parse_fen, renumbered};
use crate::pool::{LinesKey, OpenDb, writer_busy};
use crate::routes::imports::{index_within_quota, room};
use crate::store::{DbMeta, User};
use crate::wire::{
    Access, FenQuery, GameDto, GamePage, GameRowDto, GamesAnswer, GamesQuery, HeadersDto, OpeningDto,
    ReferenceDto, ReferenceLinesDto, lines_dto, notation_of, reference_dto,
};
use crate::{AppState, blocking};

/// A page of the game list: `?sort=white&desc=true&offset=0&limit=50`, plus
/// any header criteria (`white=`, `black=`, `text=`, `yearFrom=`…), `fen=`
/// for the games reaching a position (SRCH-03) and `deleted=`.
pub async fn page(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
    Query(q): Query<GamesQuery>,
) -> ApiResult<Json<GamesAnswer>> {
    blocking(move || {
        let (meta, _) = require(&state.store, &auth.user, &id, Access::Read)?;
        let open = state.open_db(&id)?;
        // Made again once if a compaction renumbered the games meanwhile.
        for _ in 0..2 {
            let list = match listing(&state, &open, &meta, &auth.user, &q, ListFor::Page)? {
                Listing::Ready(list) => list,
                Listing::Job(job) => return Ok(Json(GamesAnswer::Job { job })),
            };
            let offset = q.offset as usize;
            let page = list.slice(offset, offset.saturating_add(q.page_limit() as usize));
            let db = open.lock();
            if compacted_since(&db, &list) {
                continue;
            }
            let rows = db.rows(&page)?.into_iter().map(GameRowDto::from).collect();
            return Ok(Json(GamesAnswer::Ready(GamePage {
                total: list.len() as u32,
                offset: q.offset,
                rows,
            })));
        }
        Err(renumbered())
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
        let open = state.open_db(&id)?;
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

fn fen_position(q: &FenQuery) -> ApiResult<Chess> {
    parse_fen(q.fen.as_deref())
}

/// Starts bringing the position index up to date when it isn't (after a
/// cancelled import, or a crash in one), unless a job writes already or
/// there's no room for it: none at all, or no more than when the last build
/// stopped at the quota. It counts toward the quota. Blocking.
pub fn ensure_index(state: &Arc<AppState>, open: &Arc<OpenDb>, meta: &DbMeta, user: &User) {
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
    let room = match room(state, meta) {
        Ok(room) => room,
        Err(e) => {
            log::warn!("database {}: quota check: {}", meta.id, e.message);
            return;
        }
    };
    let stopped_at = open.cache.lock().expect("cache lock").index_stopped_at;
    if let Some(room) = room
        && (room == 0 || stopped_at.is_some_and(|at| room <= at))
    {
        return;
    }
    let (st, o, m) = (state.clone(), open.clone(), meta.clone());
    let job =
        state.jobs.spawn("index", "Indexing positions".into(), (&meta.id, &meta.name), user, move |ctx| {
            let s = index_within_quota(&st, &o, &m, ctx)?;
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
    let pos = fen_position(&q)?;
    blocking(move || {
        let (meta, _) = require(&state.store, &auth.user, &id, Access::Read)?;
        let open = state.open_db(&id)?;
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
    let pos = fen_position(&q)?;
    blocking(move || {
        let (meta, _) = require(&state.store, &auth.user, &id, Access::Read)?;
        let open = state.open_db(&id)?;
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
