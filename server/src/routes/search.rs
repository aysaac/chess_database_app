//! Searching several databases at once (SRCH-13): `GET /api/search` with
//! `db=` for each database (or `all=true` for every one the caller may
//! read) and the games route's criteria, sort and paging. Each database's
//! list is made and cached as its own game list is (`listing`), in true
//! sort order even while a write job adds games to it (its own list then
//! puts the games added since its sort order was built last; merging
//! can't), then they are merged in the sort order (`chess_db::merge_page`)
//! without copying.

use std::sync::Arc;

use axum::Json;
use axum::extract::{RawQuery, State};
use chess_db::{CrossSource, merge_page, merged_total};

use crate::access::{access, require};
use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::listing::{GameList, ListFor, Listing, compacted_since, listing, renumbered};
use crate::pool::OpenDb;
use crate::store::{DbMeta, User};
use crate::wire::{
    Access, GameRowDto, GamesQuery, SearchAnswer, SearchDatabaseDto, SearchPage, SearchRowDto,
};
use crate::{AppState, blocking};

/// Databases searched at once at most.
pub const MAX_DATABASES: usize = 64;

/// The `db=` values, `all=`, and the rest of the query (the games route's).
/// Query pairs, as given.
type Pairs = Vec<(String, String)>;

fn split(raw: &str) -> ApiResult<(Vec<String>, bool, Pairs)> {
    let pairs: Pairs =
        serde_urlencoded::from_str(raw).map_err(|e| ApiError::bad_request(format!("Bad query: {e}")))?;
    let mut dbs = Vec::new();
    let mut all = false;
    let mut rest = Vec::new();
    for (k, v) in pairs {
        match k.as_str() {
            "db" => {
                if !dbs.contains(&v) {
                    dbs.push(v);
                }
            }
            "all" => all = matches!(v.as_str(), "true" | "1"),
            _ => rest.push((k, v)),
        }
    }
    Ok((dbs, all, rest))
}

fn games_query(rest: &[(String, String)]) -> ApiResult<GamesQuery> {
    let rest = serde_urlencoded::to_string(rest).map_err(ApiError::internal)?;
    serde_urlencoded::from_str(&rest).map_err(|e| ApiError::bad_request(format!("Bad query: {e}")))
}

/// `GET /api/search?db=A&db=B&white=carlsen&sort=date&desc=true`: a page of
/// the games of A and B together. Naming a database the caller may not read
/// answers 404, as every route does for it; `all=true` searches every
/// database they may read.
pub async fn search(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<SearchAnswer>> {
    blocking(move || {
        let (ids, all, rest) = split(raw.as_deref().unwrap_or(""))?;
        // Access first, before looking at anything else of the request.
        let mut metas: Vec<DbMeta> = Vec::with_capacity(ids.len());
        for id in &ids {
            metas.push(require(&state.store, &auth.user, id, Access::Read)?.0);
        }
        let q = games_query(&rest)?;
        if all {
            if !ids.is_empty() {
                return Err(ApiError::bad_request("Give either db= or all=true, not both"));
            }
            metas = state
                .store
                .databases_for(auth.user.id)?
                .into_iter()
                .filter(|(meta, share)| access(&auth.user, meta, *share).is_some())
                .map(|(meta, _)| meta)
                .collect();
        } else if metas.is_empty() {
            return Err(ApiError::bad_request("Name the databases to search (db=ID, repeated), or all=true"));
        }
        if metas.len() > MAX_DATABASES {
            return Err(ApiError::bad_request(format!(
                "Search at most {MAX_DATABASES} databases at once: name them with db="
            )));
        }
        let opens: Vec<Arc<OpenDb>> = metas.iter().map(|m| state.open_db(&m.id)).collect::<ApiResult<_>>()?;
        // Made again once if a compaction renumbered a database's games meanwhile.
        for _ in 0..2 {
            if let Some(answer) = page_of(&state, &auth.user, &metas, &opens, &q)? {
                return Ok(Json(answer));
            }
        }
        Err(renumbered())
    })
    .await
}

/// The page `q` asks for of the databases' games together; None when a
/// compaction came meanwhile (the ids changed). Blocking.
fn page_of(
    state: &Arc<AppState>,
    user: &User,
    metas: &[DbMeta],
    opens: &[Arc<OpenDb>],
    q: &GamesQuery,
) -> ApiResult<Option<SearchAnswer>> {
    // Each database's list on a thread of its own: the first page takes
    // about as long as the slowest database's. Each in true sort order
    // (`ListFor::Merge`), as merging needs.
    let lists: Vec<ApiResult<Listing>> = std::thread::scope(|scope| {
        let handles: Vec<_> = metas
            .iter()
            .zip(opens)
            .map(|(meta, open)| scope.spawn(move || listing(state, open, meta, user, q, ListFor::Merge)))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Err(ApiError::internal("a search panicked"))))
            .collect()
    });
    let mut ready: Vec<GameList> = Vec::with_capacity(lists.len());
    for (meta, list) in metas.iter().zip(lists) {
        match list? {
            Listing::Ready(list) => ready.push(list),
            Listing::Job(job) => return Ok(Some(SearchAnswer::Job { job, database: meta.id.clone() })),
        }
    }
    let moved = || opens.iter().zip(&ready).any(|(open, list)| compacted_since(&open.lock(), list));
    let sources: Vec<CrossSource<'_>> =
        opens.iter().zip(&ready).map(|(open, list)| CrossSource { db: &open.db, ids: &list.ids }).collect();
    // A list made across a compaction may hold ids past the end: a
    // compaction is checked for before its error is.
    let page = match merge_page(&sources, q.sort_spec(), u64::from(q.offset), q.page_limit() as usize) {
        Ok(page) => page,
        Err(_) if moved() => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut rows = Vec::with_capacity(page.len());
    for (s, id) in page {
        let db = opens[s].lock();
        if compacted_since(&db, &ready[s]) {
            return Ok(None);
        }
        rows.push(SearchRowDto {
            database: metas[s].id.clone(),
            database_name: metas[s].name.clone(),
            row: GameRowDto::from(db.row(id)?),
        });
    }
    if moved() {
        return Ok(None);
    }
    let databases = metas
        .iter()
        .zip(&ready)
        .map(|(m, list)| SearchDatabaseDto {
            id: m.id.clone(),
            name: m.name.clone(),
            total: list.len() as u32,
        })
        .collect();
    Ok(Some(SearchAnswer::Ready(SearchPage {
        total: merged_total(&sources),
        offset: q.offset,
        databases,
        rows,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries() {
        let (dbs, all, rest) = split("db=a&white=Carlsen&db=b&db=a&sort=date&desc=true&limit=5").unwrap();
        assert_eq!((dbs, all), (vec!["a".to_owned(), "b".to_owned()], false));
        let q = games_query(&rest).unwrap();
        assert_eq!((q.white.as_str(), q.desc, q.limit), ("Carlsen", true, Some(5)));
        let (dbs, all, _) = split("all=true").unwrap();
        assert!(dbs.is_empty() && all);
        assert!(games_query(&split("sort=nonsense").unwrap().2).is_err());
    }
}
