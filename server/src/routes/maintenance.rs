//! Keeping a database in order: games marked as deleted and restored
//! (DB-06), duplicate games found and their copies deleted (DB-08), and
//! compaction (DB-06), which removes deleted games for good.
//!
//! Compaction renumbers the games, as in the app: an id read before means
//! another game after. Each database has an `idEpoch`, bumped by each
//! compaction, that changes taking ids can name to be refused (409) when
//! the ids are stale. Ids aren't permanent; that's a later feature. The
//! compaction bumps it holding the database's lock, before it renumbers
//! anything, and changes taking ids check it holding the lock too: a
//! compaction can't come between the check and the change.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chess_db::{DuplicateOptions, delete_copies, find_duplicates, update_position_index};
use chess_jobs::JobError;

use crate::access::require;
use crate::audit::Target;
use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::jobs::{DuplicatesFound, thousands, to_job_error};
use crate::pool::writer_busy;
use crate::routes::jobs::rights;
use crate::routes::json_body;
use crate::store::DbMeta;
use crate::wire::{
    Access, ChangedDto, DeletedCopiesDto, DuplicateGroupDto, DuplicateGroupsDto, DuplicatesRequest,
    JobStarted, MarkDeleted, PageQuery,
};
use crate::{AppState, blocking};

/// Games marked or restored at once at most.
const MAX_GAMES: usize = 100_000;
/// Duplicate groups per page at most.
const MAX_GROUPS: u32 = 1000;

fn stale_ids() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "staleIds",
        "The database was compacted since, which renumbers its games: read them again",
    )
}

fn check_epoch(meta: &DbMeta, epoch: Option<u64>) -> ApiResult<()> {
    if epoch.is_some_and(|e| e != meta.id_epoch) { Err(stale_ids()) } else { Ok(()) }
}

/// The same with the database's `idEpoch` as it is now: called holding the
/// database's lock, which a compaction holds from bumping it to its end.
fn check_epoch_now(state: &AppState, id: &str, epoch: Option<u64>) -> ApiResult<()> {
    let Some(epoch) = epoch else { return Ok(()) };
    let meta = state.store.database(id)?.ok_or_else(ApiError::not_found)?;
    check_epoch(&meta, Some(epoch))
}

/// Marks games as deleted, or restores them (write access). They stay in
/// the database until it's compacted; the game list leaves them out unless
/// asked (`deleted=show`, `deleted=only`).
pub async fn mark_deleted(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Json<ChangedDto>> {
    blocking(move || {
        let (meta, _) = require(&state.store, &auth.user, &id, Access::Write)?;
        let req: MarkDeleted = json_body(&body)?;
        check_epoch(&meta, req.id_epoch)?;
        if req.games.len() > MAX_GAMES {
            return Err(ApiError::bad_request(format!("Mark at most {MAX_GAMES} games at once")));
        }
        let open = state.open_db(&id)?;
        let mut db = open.lock();
        check_epoch_now(&state, &id, req.id_epoch)?;
        let changed = db.set_deleted_many(&req.games, req.deleted).map_err(|e| match e {
            chess_db::DbError::NotFound(g) => ApiError::bad_request(format!("There is no game {g}")),
            other => other.into(),
        })?;
        drop(db);
        log::info!(
            "{} {} {changed} games of database {}",
            auth.user.username,
            if req.deleted { "deleted" } else { "restored" },
            meta.id
        );
        let action = if req.deleted { "games.delete" } else { "games.restore" };
        let detail = Some(format!("{changed} games"));
        state.audit(action, Some(&auth.user), Target::Database(&meta), detail, Some(&auth.client));
        Ok(Json(ChangedDto { changed }))
    })
    .await
}

/// Starts looking for duplicate games (read access): a job whose report
/// says how many it found; the groups are `duplicate_groups`.
pub async fn find(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<JobStarted>)> {
    let job = blocking(move || {
        let (meta, _) = require(&state.store, &auth.user, &id, Access::Read)?;
        let req: DuplicatesRequest = json_body(&body)?;
        let open = state.open_db(&id)?;
        let st = state.clone();
        let (fuzzy, epoch) = (req.fuzzy, meta.id_epoch);
        let label = if fuzzy { "Finding duplicate games, fuzzy too" } else { "Finding duplicate games" };
        let job =
            state.jobs.spawn("duplicates", label.into(), (&meta.id, &meta.name), &auth.user, move |ctx| {
                let found =
                    find_duplicates(&open.db, DuplicateOptions { fuzzy }, ctx).map_err(to_job_error)?;
                let (groups, copies) = (found.groups.len() as u64, found.removable() as u64);
                let db = open.id.clone();
                st.jobs.set_duplicates(
                    ctx.id(),
                    DuplicatesFound { db, fuzzy, id_epoch: epoch, found: Arc::new(found) },
                );
                Ok(match copies {
                    0 => "No duplicate games".to_owned(),
                    n => format!("{} copies of {} games", thousands(n), thousands(groups)),
                })
            });
        Ok(job)
    })
    .await?;
    Ok((StatusCode::ACCEPTED, Json(JobStarted { job })))
}

/// The search for duplicates `job` found on database `id`, for a user who
/// may see its report (who started it, or the owner).
fn found_by(state: &AppState, auth: &Auth, id: &str, job: &str) -> ApiResult<(DbMeta, DuplicatesFound)> {
    let (meta, _) = require(&state.store, &auth.user, id, Access::Read)?;
    let job: u64 = job.parse().map_err(|_| ApiError::not_found())?;
    let (_, jmeta) = state.jobs.info(job).ok_or_else(ApiError::not_found)?;
    let found = state.jobs.duplicates(job).filter(|f| f.db == meta.id).ok_or_else(ApiError::not_found)?;
    if rights(state, &auth.user, &jmeta)? != Some(true) {
        return Err(ApiError::forbidden(
            "Only who started the search, or the database's owner, can see its report",
        ));
    }
    Ok((meta, found))
}

/// A page of the duplicate groups a search found: each kept game and its
/// copies, by game id.
pub async fn groups(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((id, job)): Path<(String, String)>,
    Query(page): Query<PageQuery>,
) -> ApiResult<Json<DuplicateGroupsDto>> {
    blocking(move || {
        let (_, found) = found_by(&state, &auth, &id, &job)?;
        let limit = page.limit.unwrap_or(100).min(MAX_GROUPS) as usize;
        let groups = &found.found.groups;
        Ok(Json(DuplicateGroupsDto {
            total: groups.len() as u32,
            offset: page.offset,
            groups: groups
                .iter()
                .skip(page.offset as usize)
                .take(limit)
                .map(|g| DuplicateGroupDto { keep: g.keep, remove: g.remove.clone(), exact: g.exact })
                .collect(),
        }))
    })
    .await
}

/// Marks the copies a search found as deleted (write access), after giving
/// each kept game the fields it lacked of them. Refused when the database
/// changed since the search: a copy may be the only one left.
pub async fn delete_found(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((id, job)): Path<(String, String)>,
) -> ApiResult<Json<DeletedCopiesDto>> {
    blocking(move || {
        let (meta, found) = found_by(&state, &auth, &id, &job)?;
        require(&state.store, &auth.user, &id, Access::Write)?;
        if found.id_epoch != meta.id_epoch {
            return Err(stale_ids());
        }
        let open = state.open_db(&id)?;
        let mut db = open.lock();
        check_epoch_now(&state, &id, Some(found.id_epoch))?;
        if db.generation() != found.found.generation {
            return Err(ApiError::conflict(
                "The database changed since the search: find the duplicates again",
            ));
        }
        let done = delete_copies(&mut db, &found.found.groups)?;
        drop(db);
        log::info!(
            "{} deleted {} duplicate games of database {}",
            auth.user.username,
            done.marked.len(),
            meta.id
        );
        let detail = Some(format!("{} games", done.marked.len()));
        state.audit(
            "duplicates.delete",
            Some(&auth.user),
            Target::Database(&meta),
            detail,
            Some(&auth.client),
        );
        Ok(Json(DeletedCopiesDto {
            marked: done.marked.len() as u32,
            filled: done.filled.len() as u32,
            left: done.left,
        }))
    })
    .await
}

/// Compacts the database (owner only), as a job: games marked as deleted
/// go for good and the games are renumbered (`idEpoch` goes up); then the
/// position index is built again. Refused while another write job runs on
/// it; requests to it answer "busy" (503) until the compaction is done.
pub async fn compact(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
) -> ApiResult<(StatusCode, Json<JobStarted>)> {
    let job = blocking(move || {
        let (meta, _) = require(&state.store, &auth.user, &id, Access::Owner)?;
        let open = state.open_db(&id)?;
        let mut writer = open.writer.lock().expect("writer lock");
        if writer_busy(&writer, &state.jobs.manager) {
            return Err(ApiError::conflict(
                "An import or index build runs on this database: wait for it to finish",
            ));
        }
        let st = state.clone();
        let o = open.clone();
        let db_id = meta.id.clone();
        let job = state.jobs.spawn(
            "compact",
            "Compacting the database".into(),
            (&meta.id, &meta.name),
            &auth.user,
            move |ctx| {
                o.compacting.store(true, Ordering::Release);
                let result = {
                    let mut db = o.lock();
                    // Bumped first, holding the database: a change that took
                    // ids waiting for it then sees the new epoch, and ids read
                    // before it are refused even if the server stops halfway.
                    // A compaction that fails or is cancelled bumps it too.
                    match st.store.bump_id_epoch(&db_id) {
                        Ok(()) => db.compact(ctx).map_err(to_job_error),
                        Err(e) => {
                            Err(JobError::Failed(format!("couldn't note the compaction: {}", e.message)))
                        }
                    }
                };
                o.compacting.store(false, Ordering::Release);
                let summary = result?;
                if ctx.is_cancelled() {
                    return Err(JobError::Cancelled);
                }
                ctx.progress_msg(0, 0, "Indexing positions");
                update_position_index(&o.db, ctx).map_err(to_job_error)?;
                let mb = |b: u64| format!("{:.1} MB", b as f64 / 1e6);
                Ok(format!(
                    "Compacted: removed {} deleted games; {} → {}",
                    thousands(u64::from(summary.removed)),
                    mb(summary.bytes_before),
                    mb(summary.bytes_after)
                ))
            },
        );
        *writer = Some(job);
        log::info!("{} compacts database {}", auth.user.username, meta.id);
        state.audit("compact", Some(&auth.user), Target::Database(&meta), None, Some(&auth.client));
        Ok(job)
    })
    .await?;
    Ok((StatusCode::ACCEPTED, Json(JobStarted { job })))
}
