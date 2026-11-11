//! What a game list shows, in a sort order: the whole database, a header
//! search (SRCH-01), the games reaching a position (SRCH-03), games marked
//! as deleted or not, and any mix of them. One database's list is the
//! games route's, the export's, and a source of a search across databases
//! (SRCH-13). Results are cached per database until it changes (its
//! generation), as `Cache` keeps them.

use std::ops::Range;
use std::sync::Arc;

use chess_core::position;
use chess_core::shakmaty::Chess;
use chess_db::{
    DbError, GameId, GameSet, NoProgress, SortSpec, SortedIds, build_sort, ensure_sort, find_position,
    search_headers, subset_in,
};
use chess_jobs::JobId;

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::pool::{ListKey, OpenDb, writer_busy};
use crate::routes::games::ensure_index;
use crate::store::{DbMeta, User};
use crate::wire::{DeletedFilter, GamesQuery};

/// What a list is for, which decides where games a write job adds go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListFor {
    /// The game list and the export: while a write job adds games, the
    /// sort order isn't built again at every batch; the games added since
    /// it was built follow the sorted ones, in id order.
    Page,
    /// A search across databases (SRCH-13) merges the lists, so each one
    /// must be in true sort order: a sort order games were added to since
    /// is built again.
    Merge,
}

/// A list, in its order.
pub struct GameList {
    /// The games in the sort order. With [`ListFor::Page`] while a write
    /// job adds games, a search's games added since the sort order was
    /// built follow in id order.
    pub ids: SortedIds,
    /// The rest of the whole database's games, added since its sort order
    /// was built ([`ListFor::Page`] only), listed after `ids` in id order.
    pub appended: Range<GameId>,
    /// The database's compactions when the list was made: once that
    /// changed, its ids are other games ([`compacted_since`]).
    pub compactions: u64,
}

impl GameList {
    pub fn len(&self) -> usize {
        self.ids.len() + self.appended.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Positions `from..to` as game ids.
    pub fn slice(&self, from: usize, to: usize) -> Vec<GameId> {
        let mut out = self.ids.slice(from, to);
        let sorted = self.ids.len();
        let to = to.min(self.len());
        out.extend((from.max(sorted)..to).map(|i| self.appended.start + (i - sorted) as GameId));
        out
    }
}

/// A list, or the job to wait for before it can be made.
pub enum Listing {
    Ready(GameList),
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

/// Whether the database was compacted since `list` was made: its ids are
/// other games now. Checked holding the database's lock, before reading
/// games by them.
pub fn compacted_since(db: &chess_db::Database, list: &GameList) -> bool {
    db.compactions() != list.compactions
}

/// What a request answers when the database was compacted while it read
/// it (twice): its game ids changed meanwhile.
pub fn renumbered() -> ApiError {
    ApiError::busy("The database was compacted meanwhile, which renumbers its games: ask again")
}

/// The sort permutation for `spec`. It covers every game, unless the list
/// is a page's and a write job adds games meanwhile: then the one built
/// stays (games added since follow it), so an import doesn't sort millions
/// of games again at every batch.
fn sort_for(
    state: &AppState,
    open: &OpenDb,
    spec: SortSpec,
    purpose: ListFor,
) -> Result<Arc<Vec<GameId>>, DbError> {
    let quick = purpose == ListFor::Page
        && writer_busy(&open.writer.lock().expect("writer lock"), &state.jobs.manager);
    if quick { build_sort(&open.db, spec, &NoProgress) } else { ensure_sort(&open.db, spec, &NoProgress) }
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

/// What to answer when the position index is behind and a search can't
/// replay the games it lacks: the job bringing it up to date (started if
/// none writes), or "busy" while the writer slot holds a job of no
/// database of the caller's (a backup copying it: its id would be "not
/// found" to them).
fn index_behind(state: &Arc<AppState>, open: &Arc<OpenDb>, meta: &DbMeta, user: &User) -> ApiResult<Listing> {
    ensure_index(state, open, meta, user);
    let writer = *open.writer.lock().expect("writer lock");
    match writer {
        Some(job) if writer_busy(&Some(job), &state.jobs.manager) => {
            let ours = state.jobs.info(job).is_some_and(|(_, m)| m.db == open.id);
            if ours {
                Ok(Listing::Job(job))
            } else {
                Err(ApiError::busy("The database is being backed up: try again in a moment"))
            }
        }
        // With no job writing, indexing didn't start: no room for it (the
        // quota), or it failed to.
        _ => Err(ApiError::conflict(
            "The position index is behind and can't be brought up to date now: the database or its owner \
             may be at their quota",
        )),
    }
}

/// The list `q` asks for, in its sort order. Blocking: reads the database,
/// a chunk at a time, so a compaction can come between two chunks; the
/// caller checks [`compacted_since`] before reading games by the ids.
pub fn listing(
    state: &Arc<AppState>,
    open: &Arc<OpenDb>,
    meta: &DbMeta,
    user: &User,
    q: &GamesQuery,
    purpose: ListFor,
) -> ApiResult<Listing> {
    let spec = q.sort_spec();
    let pos = q.fen().map(|f| parse_fen(Some(f))).transpose()?;
    let key =
        ListKey { header: q.header_query(), position: pos.as_ref().map(position::hash), deleted: q.deleted };
    let (generation, epoch, compactions) = {
        let db = open.lock();
        (db.generation(), db.sort_epoch(), db.compactions())
    };
    let ready =
        |ids: SortedIds, appended: Range<GameId>| Ok(Listing::Ready(GameList { ids, appended, compactions }));
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
        if spec.is_natural_order() {
            let count = open.lock().count();
            return ready(SortedIds::Range { count, descending: spec.descending }, 0..0);
        }
        let perm = sort_for(state, open, spec, purpose)?;
        let appended = match purpose {
            ListFor::Page => {
                let count = open.lock().count();
                (perm.len() as GameId).min(count)..count
            }
            // The order covers every game there was when it was made: the list as of then.
            ListFor::Merge => 0..0,
        };
        return ready(SortedIds::List(perm), appended);
    }

    let cached = open.cache.lock().expect("cache lock").search(&key, generation);
    let found = match cached {
        Some(ids) => ids,
        None => {
            let within = match &pos {
                Some(pos) => {
                    let hits = find_position(&open.db, pos)?;
                    if hits.partial {
                        return index_behind(state, open, meta, user);
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
        return ready(SortedIds::List(found), 0..0);
    }
    let cached = open.cache.lock().expect("cache lock").order(&key, spec, generation, epoch);
    if let Some(ids) = cached {
        return ready(SortedIds::List(ids), 0..0);
    }
    let (ids, in_order) = if spec.is_natural_order() {
        (found.iter().rev().copied().collect(), true)
    } else {
        // The permutation just built, not one looked up again: it may not
        // have been kept (a change came while it was built).
        let perm = sort_for(state, open, spec, purpose)?;
        let in_order = found.last().is_none_or(|&last| (last as usize) < perm.len());
        (subset_in(&perm, &found), in_order)
    };
    let ids = Arc::new(ids);
    // Only lists in true sort order are kept, for any purpose to use.
    if in_order {
        open.cache.lock().expect("cache lock").put_order(key, spec, generation, epoch, ids.clone());
    }
    ready(SortedIds::List(ids), 0..0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appended_games_follow_the_sorted_ones() {
        let list = GameList { ids: SortedIds::List(Arc::new(vec![2, 0, 1])), appended: 3..6, compactions: 0 };
        assert_eq!(list.len(), 6);
        assert_eq!(list.slice(0, 100), [2, 0, 1, 3, 4, 5]);
        assert_eq!(list.slice(2, 4), [1, 3]);
        assert_eq!(list.slice(4, 5), [4]);
        assert!(list.slice(6, 9).is_empty());
        let none =
            GameList { ids: SortedIds::Range { count: 2, descending: true }, appended: 0..0, compactions: 0 };
        assert_eq!(none.slice(0, 10), [1, 0]);
    }
}
