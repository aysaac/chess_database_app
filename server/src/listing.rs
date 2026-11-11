//! What a game list shows, in a sort order: the whole database, a header
//! search (SRCH-01), the games reaching a position (SRCH-03), games marked
//! as deleted or not, and any mix of them. One database's list is the
//! games route's, the export's, and a source of a search across databases
//! (SRCH-13). Results are cached per database until it changes (its
//! generation), as `Cache` keeps them.

use std::sync::Arc;

use chess_core::position;
use chess_core::shakmaty::Chess;
use chess_db::{
    DbError, GameId, GameSet, NoProgress, SortSpec, SortedIds, build_sort, ensure_sort, find_position,
    search_headers, sorted_subset,
};
use chess_jobs::JobId;

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::pool::{ListKey, OpenDb, writer_busy};
use crate::routes::games::ensure_index;
use crate::store::{DbMeta, User};
use crate::wire::{DeletedFilter, GamesQuery};

/// A list, or the job to wait for before it can be made.
pub enum Listing {
    Ready(SortedIds),
    /// The position index is behind (by more than a search replays): the
    /// job bringing it up to date.
    Job(JobId),
}

/// The position of `?fen=`, the standard start when there's none.
pub fn parse_fen(fen: Option<&str>) -> ApiResult<Chess> {
    match fen.map(str::trim).filter(|f| !f.is_empty()) {
        None => Ok(Chess::default()),
        Some(fen) => position::from_fen(fen).map_err(|e| ApiError::bad_request(format!("Bad FEN: {e}"))),
    }
}

/// The sort permutation for `spec`, covering every game unless a write job
/// adds games meanwhile: then the one built stays (games added since follow
/// it), so an import doesn't sort millions of games again at every batch.
fn sort_for(state: &AppState, open: &OpenDb, spec: SortSpec) -> Result<Arc<Vec<GameId>>, DbError> {
    let busy = writer_busy(&open.writer.lock().expect("writer lock"), &state.jobs.manager);
    if busy { build_sort(&open.db, spec, &NoProgress) } else { ensure_sort(&open.db, spec, &NoProgress) }
}

/// Whether some game of the database is marked as deleted.
fn any_deleted(open: &OpenDb, generation: u64) -> Result<bool, DbError> {
    if let Some(any) = open.cache.lock().expect("cache lock").any_deleted(generation) {
        return Ok(any);
    }
    let any = open.lock().game_stats()?.iter().any(|g| g.deleted);
    open.cache.lock().expect("cache lock").put_any_deleted(generation, any);
    Ok(any)
}

/// The list `q` asks for, in its sort order. Blocking: reads the database.
pub fn listing(
    state: &AppState,
    open: &Arc<OpenDb>,
    meta: &DbMeta,
    user: &User,
    q: &GamesQuery,
) -> ApiResult<Listing> {
    let spec = q.sort_spec();
    let pos = q.fen().map(|f| parse_fen(Some(f))).transpose()?;
    let key =
        ListKey { header: q.header_query(), position: pos.as_ref().map(position::hash), deleted: q.deleted };
    let generation = open.lock().generation();
    let no_criteria = (chess_db::HeaderQuery { skip_deleted: false, ..key.header.clone() })
        == chess_db::HeaderQuery::default()
        && pos.is_none();
    let everything = no_criteria
        && match q.deleted {
            DeletedFilter::Show => true,
            DeletedFilter::Hide => !any_deleted(open, generation)?,
            DeletedFilter::Only => false,
        };
    if everything {
        return Ok(Listing::Ready(if spec.is_natural_order() {
            SortedIds::Range { count: open.lock().count(), descending: spec.descending }
        } else {
            SortedIds::List(sort_for(state, open, spec)?)
        }));
    }

    let cached = open.cache.lock().expect("cache lock").search(&key, generation);
    let found = match cached {
        Some(ids) => ids,
        None => {
            let within = match &pos {
                Some(pos) => {
                    let hits = find_position(&open.db, pos)?;
                    if hits.partial {
                        ensure_index(state, open, meta, user);
                        let writer = *open.writer.lock().expect("writer lock");
                        // A job writes (and indexes after); with none, indexing failed to start.
                        return match writer {
                            Some(job) if writer_busy(&Some(job), &state.jobs.manager) => {
                                Ok(Listing::Job(job))
                            }
                            _ => {
                                Err(ApiError::conflict("The position index can't be brought up to date now"))
                            }
                        };
                    }
                    Some(GameSet::from_ids(&hits.entries.iter().map(|e| e.game).collect::<Vec<_>>()))
                }
                None => None,
            };
            let mut ids = search_headers(&open.db, &key.header, within.as_ref(), &NoProgress)?;
            if q.deleted == DeletedFilter::Only {
                let mut db = open.lock();
                let stats = db.game_stats()?;
                ids.retain(|&id| stats.get(id as usize).is_some_and(|g| g.deleted));
            }
            let ids = Arc::new(ids);
            open.cache.lock().expect("cache lock").put_search(key.clone(), generation, ids.clone());
            ids
        }
    };
    if spec.is_natural_order() && !spec.descending {
        return Ok(Listing::Ready(SortedIds::List(found)));
    }
    let perm = if spec.is_natural_order() { None } else { Some(sort_for(state, open, spec)?) };
    let db = open.lock();
    let epoch = db.sort_epoch();
    let cached = open.cache.lock().expect("cache lock").order(&key, spec, generation, epoch);
    let ordered = match cached {
        Some(ids) => ids,
        None => {
            let ids = Arc::new(match perm {
                None => found.iter().rev().copied().collect(),
                Some(_) => sorted_subset(&db, spec, &found)?,
            });
            open.cache.lock().expect("cache lock").put_order(key, spec, generation, epoch, ids.clone());
            ids
        }
    };
    Ok(Listing::Ready(SortedIds::List(ordered)))
}
