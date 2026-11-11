//! A job's progress, for users who may read its database, and cancelling
//! it (who started it, or the database's owner). Whoever loses access to the
//! database (made private, unshared) loses its jobs too.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use crate::access::require;
use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::jobs::JobMeta;
use crate::store::User;
use crate::wire::{Access, JobDto, JobInfoDto};
use crate::{AppState, blocking};

/// What `user` may do with a job: `None` (it's "not found"), read it, or
/// also see its report and cancel it.
pub(crate) fn rights(state: &AppState, user: &User, meta: &JobMeta) -> ApiResult<Option<bool>> {
    let access = match require(&state.store, user, &meta.db, Access::Read) {
        Ok((_, a)) => a,
        Err(e) if e.status == StatusCode::NOT_FOUND => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(Some(meta.user == user.id || access == Access::Owner))
}

pub async fn job(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<JobInfoDto>> {
    let id: u64 = id.parse().map_err(|_| ApiError::not_found())?;
    blocking(move || {
        let (status, meta) = state.jobs.info(id).ok_or_else(ApiError::not_found)?;
        let full = rights(&state, &auth.user, &meta)?.ok_or_else(ApiError::not_found)?;
        Ok(Json(JobInfoDto {
            job: JobDto::from(&status),
            database: meta.db,
            report: if full { state.jobs.report(id) } else { None },
            duplicates: if full { state.jobs.duplicates(id).map(|d| d.report()) } else { None },
        }))
    })
    .await
}

pub async fn cancel(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let id: u64 = id.parse().map_err(|_| ApiError::not_found())?;
    blocking(move || {
        let (_, meta) = state.jobs.info(id).ok_or_else(ApiError::not_found)?;
        match rights(&state, &auth.user, &meta)? {
            None => Err(ApiError::not_found()),
            Some(false) => {
                Err(ApiError::forbidden("Only who started a job, or the database's owner, can cancel it"))
            }
            Some(true) => {
                state.jobs.manager.cancel(id);
                Ok(StatusCode::NO_CONTENT)
            }
        }
    })
    .await
}
