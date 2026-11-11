//! Searching several open databases at once (SRCH-13). A search runs as a
//! job: each database's matching games (a header search, within those
//! reaching a position if asked) are found and kept, ascending, as a list's
//! filter is. Pages are then served in any sort order by merging each
//! database's results in that order (`chess_db::merge_page`), never by
//! copying them all into one list. Each database's results in a sort order
//! are made once, in a `sort` job (`cross_sort`), as the game list's are.
//!
//! A search holds its databases weakly: closing one, or compacting it (its
//! game ids change), makes the search say to search again.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use chess_core::position;
use chess_db::{
    CrossSource, Database, DbError, GameId, GameSet, HeaderQuery, Progress, SortSpec, SortedIds, ensure_sort,
    find_position, merge_page, merged_total, scan_position, search_headers, sorted_subset,
};
use tauri::{AppHandle, Manager, State};

use crate::commands::db::{describe, thousands, to_job_error};
use crate::commands::tree::position_of;
use crate::dto::{
    GameRowDto, HeaderQueryDto, OpenDbDto, SearchDbDto, SearchInfoDto, SearchPageDto, SearchRowDto,
    SearchStartDto, SortSpecDto,
};
use crate::error::{AppError, AppResult};
use crate::state::{AppState, SharedDb};

/// Searches kept at once; older ones go.
const KEPT: usize = 4;
/// Rows per page at most.
const MAX_PAGE: u32 = 1000;

#[derive(Clone)]
struct Source {
    handle: u32,
    name: String,
    db: Weak<Mutex<Database>>,
    /// The database's compactions when it was searched: another one
    /// renumbered its games.
    compactions: u64,
    /// The matching games, ascending.
    found: Arc<Vec<GameId>>,
    /// The found games in each sort order made so far, with the sort epoch
    /// they were made at.
    orders: HashMap<SortSpec, (u64, Arc<Vec<GameId>>)>,
}

struct CrossSearch {
    description: String,
    sources: Vec<Source>,
}

#[derive(Default)]
pub struct CrossSearches {
    next: u32,
    searches: Vec<(u32, CrossSearch)>,
}

impl CrossSearches {
    fn get(&self, id: u32) -> AppResult<&CrossSearch> {
        self.searches
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, s)| s)
            .ok_or_else(|| AppError::invalid("those search results are gone: search again"))
    }

    fn get_mut(&mut self, id: u32) -> AppResult<&mut CrossSearch> {
        self.searches
            .iter_mut()
            .find(|(i, _)| *i == id)
            .map(|(_, s)| s)
            .ok_or_else(|| AppError::invalid("those search results are gone: search again"))
    }
}

fn name_of(path: &std::path::Path) -> String {
    path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The open databases a search can read: those in any window's list, and
/// the reference database.
#[tauri::command]
pub async fn open_databases(state: State<'_, AppState>) -> AppResult<Vec<OpenDbDto>> {
    let mut out: Vec<OpenDbDto> = state
        .open_dbs()
        .into_iter()
        .map(|(handle, path, db, listed, reference)| OpenDbDto {
            handle,
            name: name_of(&path),
            games: db.lock().expect("db lock").count(),
            listed,
            reference,
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.handle.cmp(&b.handle)));
    Ok(out)
}

/// Passes one database's progress on as a part of the whole search.
struct Part<'a> {
    ctx: &'a chess_jobs::JobContext,
    index: u64,
    of: u64,
}

impl Progress for Part<'_> {
    fn report(&self, done: u64, total: u64) {
        let share = (done.min(total) * 1000).checked_div(total).unwrap_or(0);
        self.ctx.progress(self.index * 1000 + share, self.of * 1000);
    }

    fn is_cancelled(&self) -> bool {
        self.ctx.is_cancelled()
    }
}

/// Searches the open databases `handles` for `query`, within the games
/// reaching the position on screen in `session` when given (through each
/// database's position index; reading every game where it's far behind).
/// The results are `search` once the job is done.
#[tauri::command]
pub async fn search_databases(
    app: AppHandle,
    state: State<'_, AppState>,
    handles: Vec<u32>,
    query: HeaderQueryDto,
    session: Option<u32>,
) -> AppResult<SearchStartDto> {
    if handles.is_empty() {
        return Err(AppError::invalid("choose the databases to search"));
    }
    let mut dbs = Vec::with_capacity(handles.len());
    for &h in &handles {
        if dbs.iter().any(|(o, _, _)| *o == h) {
            continue;
        }
        let path = state.open_dbs().into_iter().find(|o| o.0 == h).map(|o| o.1);
        dbs.push((h, path.as_deref().map(name_of).unwrap_or_default(), state.db(h)?));
    }
    let pos = session.map(|s| position_of(&state, s)).transpose()?;
    let query: HeaderQuery = query.into();
    let mut description = describe(&query);
    if let Some(pos) = &pos {
        let fen = position::to_fen(pos);
        description = if query == HeaderQuery::default() {
            format!("Position {fen}")
        } else {
            format!("{description} · position {fen}")
        };
    }
    let search = {
        let mut cross = state.cross.lock().expect("cross lock");
        cross.next += 1;
        cross.next
    };
    let of = dbs.len() as u64;
    let job = state.jobs.spawn("search", format!("Searching {of} databases"), move |ctx| {
        let mut sources = Vec::with_capacity(dbs.len());
        for (i, (handle, name, db)) in dbs.into_iter().enumerate() {
            let part = Part { ctx, index: i as u64, of };
            let within = match &pos {
                Some(pos) => {
                    let hits = find_position(&db, pos).map_err(to_job_error)?;
                    let ids: Vec<GameId> = if hits.partial {
                        scan_position(&db, pos, &part).map_err(to_job_error)?
                    } else {
                        hits.entries.iter().map(|e| e.game).collect()
                    };
                    Some(GameSet::from_ids(&ids))
                }
                None => None,
            };
            let found = search_headers(&db, &query, within.as_ref(), &part).map_err(to_job_error)?;
            let compactions = db.lock().expect("db lock").compactions();
            sources.push(Source {
                handle,
                name,
                db: Arc::downgrade(&db),
                compactions,
                found: Arc::new(found),
                orders: HashMap::new(),
            });
        }
        let total: u64 = sources.iter().map(|s| s.found.len() as u64).sum();
        let state = app.state::<AppState>();
        let mut cross = state.cross.lock().expect("cross lock");
        cross.searches.push((search, CrossSearch { description, sources }));
        let excess = cross.searches.len().saturating_sub(KEPT);
        cross.searches.drain(..excess);
        Ok(match total {
            0 => format!("No games match the search in {of} databases"),
            1 => format!("Found 1 game in {of} databases"),
            n => format!("Found {} games in {of} databases", thousands(n)),
        })
    });
    Ok(SearchStartDto { search, job })
}

fn info_of(id: u32, s: &CrossSearch) -> SearchInfoDto {
    SearchInfoDto {
        search: id,
        description: s.description.clone(),
        total: s.sources.iter().map(|s| s.found.len() as u64).sum(),
        databases: s
            .sources
            .iter()
            .map(|s| SearchDbDto { handle: s.handle, name: s.name.clone(), count: s.found.len() as u32 })
            .collect(),
    }
}

#[tauri::command]
pub async fn cross_info(state: State<'_, AppState>, search: u32) -> AppResult<SearchInfoDto> {
    let cross = state.cross.lock().expect("cross lock");
    Ok(info_of(search, cross.get(search)?))
}

/// The database of a source, if it's still open as it was searched.
fn live(state: &AppState, s: &Source) -> AppResult<Arc<Mutex<Database>>> {
    let gone = || AppError::invalid(format!("{} was closed: search again", s.name));
    let db = s.db.upgrade().ok_or_else(gone)?;
    if !state.db(s.handle).is_ok_and(|open| Arc::ptr_eq(&open, &db)) {
        return Err(gone());
    }
    if db.lock().expect("db lock").compactions() != s.compactions {
        return Err(AppError::invalid(format!(
            "{} was compacted, which renumbers its games: search again",
            s.name
        )));
    }
    Ok(db)
}

/// Each source's results in `spec` order if they're all made (and still
/// valid): (database, ids).
fn ordered(state: &AppState, search: u32, spec: SortSpec) -> AppResult<Option<Vec<(SharedDb, SortedIds)>>> {
    let sources: Vec<Source> = state.cross.lock().expect("cross lock").get(search)?.sources.clone();
    let mut out = Vec::with_capacity(sources.len());
    for src in sources {
        let db = live(state, &src)?;
        let ids = if spec.is_natural_order() && !spec.descending {
            src.found.clone()
        } else {
            let epoch = db.lock().expect("db lock").sort_epoch();
            match src.orders.get(&spec) {
                Some((made, ids)) if spec.is_natural_order() || *made == epoch => ids.clone(),
                _ => return Ok(None),
            }
        };
        out.push((db, SortedIds::List(ids)));
    }
    Ok(Some(out))
}

/// Makes each database's results in `sort` order, as a job (`None` when
/// they're all made already). Then `cross_page` serves that order.
#[tauri::command]
pub async fn cross_sort(
    app: AppHandle,
    state: State<'_, AppState>,
    search: u32,
    sort: SortSpecDto,
) -> AppResult<Option<u64>> {
    let spec: SortSpec = sort.into();
    if ordered(&state, search, spec)?.is_some() {
        return Ok(None);
    }
    let sources: Vec<(usize, SharedDb, Arc<Vec<GameId>>)> = {
        let cross = state.cross.lock().expect("cross lock");
        let s = cross.get(search)?;
        let mut out = Vec::new();
        for (i, src) in s.sources.iter().enumerate() {
            out.push((i, live(&state, src)?, src.found.clone()));
        }
        out
    };
    let of = sources.len() as u64;
    Ok(Some(state.jobs.spawn("sort", "Sorting the search results", move |ctx| {
        let mut made = Vec::with_capacity(sources.len());
        for (i, db, found) in sources {
            let part = Part { ctx, index: i as u64, of };
            let ids = if spec.is_natural_order() {
                found.iter().rev().copied().collect()
            } else {
                ensure_sort(&db, spec, &part).map_err(to_job_error)?;
                sorted_subset(&db.lock().expect("db lock"), spec, &found).map_err(to_job_error)?
            };
            let epoch = db.lock().expect("db lock").sort_epoch();
            made.push((i, epoch, Arc::new(ids)));
        }
        let state = app.state::<AppState>();
        let mut cross = state.cross.lock().expect("cross lock");
        if let Ok(s) = cross.get_mut(search) {
            for (i, epoch, ids) in made {
                if let Some(src) = s.sources.get_mut(i) {
                    src.orders.insert(spec, (epoch, ids));
                }
            }
        }
        Ok("Sorted the search results".to_owned())
    })))
}

/// Rows `offset..offset + limit` of a search's results in `sort` order.
/// Fails with `notSorted` until `cross_sort` made that order.
#[tauri::command]
pub async fn cross_page(
    state: State<'_, AppState>,
    search: u32,
    sort: SortSpecDto,
    offset: u32,
    limit: u32,
) -> AppResult<SearchPageDto> {
    let spec: SortSpec = sort.into();
    let lists = ordered(&state, search, spec)?.ok_or(DbError::NotSorted)?;
    let names: Vec<(u32, String)> = {
        let cross = state.cross.lock().expect("cross lock");
        cross.get(search)?.sources.iter().map(|s| (s.handle, s.name.clone())).collect()
    };
    let sources: Vec<CrossSource<'_>> = lists.iter().map(|(db, ids)| CrossSource { db, ids }).collect();
    let page = merge_page(&sources, spec, u64::from(offset), limit.min(MAX_PAGE) as usize)?;
    let mut rows = Vec::with_capacity(page.len());
    for (s, id) in page {
        let row = lists[s].0.lock().expect("db lock").row(id)?;
        rows.push(SearchRowDto {
            handle: names[s].0,
            database: names[s].1.clone(),
            row: GameRowDto::from(row),
        });
    }
    Ok(SearchPageDto { total: merged_total(&sources), offset, rows })
}

/// Forgets a search's results.
#[tauri::command]
pub async fn close_search(state: State<'_, AppState>, search: u32) -> AppResult<()> {
    state.cross.lock().expect("cross lock").searches.retain(|(i, _)| *i != search);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_are_kept_a_few_at_a_time() {
        let mut c = CrossSearches::default();
        for i in 1..=6 {
            c.searches.push((i, CrossSearch { description: String::new(), sources: Vec::new() }));
            let excess = c.searches.len().saturating_sub(KEPT);
            c.searches.drain(..excess);
        }
        assert!(c.get(2).is_err());
        assert_eq!(info_of(6, c.get(6).unwrap()).total, 0);
        assert_eq!(c.searches.len(), KEPT);
    }
}
