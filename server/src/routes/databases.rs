//! Databases: the ones the caller can read, making and deleting one's own,
//! and its visibility and shares (owner only).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;

use crate::access::{access, require};
use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::routes::check_name;
use crate::secret::new_db_id;
use crate::store::{DbMeta, User};
use crate::wire::{
    Access, Confirm, CreateDatabase, DatabaseDto, IndexStatusDto, SetShare, ShareDto, UpdateDatabase,
    Visibility,
};
use crate::{AppState, blocking};

pub fn dto(state: &AppState, meta: DbMeta, access: Access) -> DatabaseDto {
    let (games, size_bytes) = state.pool.stats(&meta.id);
    DatabaseDto {
        id: meta.id,
        name: meta.name,
        owner: meta.owner,
        visibility: meta.visibility,
        access,
        games,
        size_bytes,
        quota_bytes: meta.quota_bytes,
        created_at: meta.created_at,
        id_epoch: meta.id_epoch,
        index: None,
    }
}

/// Only an admin makes a database every user reads.
fn check_visibility(user: &User, visibility: Visibility) -> ApiResult<()> {
    if visibility == Visibility::Server && !user.is_admin() {
        return Err(ApiError::forbidden("Only an admin can make a database server-wide"));
    }
    Ok(())
}

/// The databases the caller can read.
pub async fn list(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<Json<Vec<DatabaseDto>>> {
    blocking(move || {
        let rows = state.store.databases_for(auth.user.id)?;
        Ok(Json(
            rows.into_iter()
                .filter_map(|(meta, share)| {
                    let a = access(&auth.user, &meta, share)?;
                    Some(dto(&state, meta, a))
                })
                .collect(),
        ))
    })
    .await
}

/// A new, empty database owned by the caller (private unless asked).
pub async fn create(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Json(req): Json<CreateDatabase>,
) -> ApiResult<(StatusCode, Json<DatabaseDto>)> {
    let name = check_name(&req.name)?;
    let visibility = req.visibility.unwrap_or(Visibility::Private);
    check_visibility(&auth.user, visibility)?;
    blocking(move || {
        let id = new_db_id();
        drop(state.pool.create(&id)?);
        let meta = match state.store.create_database(&id, &name, auth.user.id, visibility) {
            Ok(meta) => meta,
            Err(e) => {
                let _ = state.pool.remove(&id);
                return Err(e);
            }
        };
        log::info!("{} made database {id} ({})", auth.user.username, visibility.as_str());
        Ok((StatusCode::CREATED, Json(dto(&state, meta, Access::Owner))))
    })
    .await
}

pub async fn info(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<DatabaseDto>> {
    blocking(move || {
        let (meta, access) = require(&state.store, &auth.user, &id, Access::Read)?;
        let open = state.open_db(&id)?;
        let index = {
            let db = open.lock();
            let p = db.positions();
            IndexStatusDto {
                built: p.is_built(),
                covered: p.covered(),
                up_to_date: p.plan(db.count()) == chess_db::IndexPlan::UpToDate,
                max_ply: p.max_ply(),
                size_bytes: p.size_bytes(),
            }
        };
        drop(open);
        Ok(Json(DatabaseDto { index: Some(index), ..dto(&state, meta, access) }))
    })
    .await
}

/// Renames the database or changes who can read it (owner only).
pub async fn update(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
    Json(req): Json<UpdateDatabase>,
) -> ApiResult<Json<DatabaseDto>> {
    blocking(move || {
        // Access first: a database the caller can't see stays unseen.
        require(&state.store, &auth.user, &id, Access::Owner)?;
        let name = req.name.as_deref().map(check_name).transpose()?;
        if let Some(v) = req.visibility {
            check_visibility(&auth.user, v)?;
        }
        if let Some(name) = &name {
            state.store.rename_database(&id, name)?;
        }
        if let Some(v) = req.visibility {
            state.store.set_visibility(&id, v)?;
            log::info!("{} made database {id} {}", auth.user.username, v.as_str());
        }
        let meta = state.store.database(&id)?.ok_or_else(ApiError::not_found)?;
        Ok(Json(dto(&state, meta, Access::Owner)))
    })
    .await
}

/// Deletes a database for good: its jobs are stopped, its row and files go.
/// Blocking.
pub fn delete_now(state: &AppState, meta: &DbMeta, by: &User, confirm: &str) -> ApiResult<()> {
    if confirm.trim() != meta.name {
        return Err(ApiError::bad_request("To delete a database, confirm with its name (?confirm=NAME)"));
    }
    state.jobs.stop_all_for(&meta.id);
    state.store.delete_database(&meta.id)?;
    state.pool.remove(&meta.id)?;
    log::info!("{} deleted database {} ({:?}, owner {})", by.username, meta.id, meta.name, meta.owner);
    Ok(())
}

pub async fn remove(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
    Query(confirm): Query<Confirm>,
) -> ApiResult<StatusCode> {
    blocking(move || {
        let (meta, _) = require(&state.store, &auth.user, &id, Access::Owner)?;
        delete_now(&state, &meta, &auth.user, &confirm.confirm)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

pub async fn shares(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<ShareDto>>> {
    blocking(move || {
        require(&state.store, &auth.user, &id, Access::Owner)?;
        let shares = state.store.shares(&id)?;
        Ok(Json(shares.into_iter().map(|(username, access)| ShareDto { username, access }).collect()))
    })
    .await
}

/// Shares the database with a user, or changes what the share gives. It
/// gives nothing while the database is private.
pub async fn set_share(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((id, username)): Path<(String, String)>,
    Json(req): Json<SetShare>,
) -> ApiResult<StatusCode> {
    blocking(move || {
        require(&state.store, &auth.user, &id, Access::Owner)?;
        let (user, _) = state.store.user_by_name(&username)?.ok_or_else(|| {
            ApiError::new(StatusCode::NOT_FOUND, "noSuchUser", format!("No user is called {username}"))
        })?;
        if user.id == auth.user.id {
            return Err(ApiError::bad_request("You own this database already"));
        }
        state.store.set_share(&id, user.id, req.access)?;
        log::info!(
            "{} shared database {id} with {} ({})",
            auth.user.username,
            user.username,
            req.access.as_str()
        );
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

pub async fn remove_share(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path((id, username)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    blocking(move || {
        require(&state.store, &auth.user, &id, Access::Owner)?;
        let (user, _) = state.store.user_by_name(&username)?.ok_or_else(ApiError::not_found)?;
        if !state.store.remove_share(&id, user.id)? {
            return Err(ApiError::not_found());
        }
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}
