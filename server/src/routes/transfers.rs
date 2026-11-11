//! Giving a database to another user: only its owner offers it, to a named
//! enabled user, who accepts or declines; nobody can fill someone else's
//! quota. Accepting is checked against the recipient's quota, and the old
//! owner keeps no access (a server-wide database stays readable by every
//! user, them too, as it was). The admin can transfer only their own
//! databases, like any owner.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use crate::access::require;
use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::routes::json_body;
use crate::store::DbMeta;
use crate::wire::{Access, OfferRequest, TransferDto, TransfersDto};
use crate::{AppState, blocking};

fn dto(state: &AppState, meta: DbMeta, from: String, to: String, created_at: i64) -> TransferDto {
    let (games, size_bytes) = state.pool.stats(&meta.id);
    TransferDto {
        database: meta.id,
        name: meta.name,
        from,
        to,
        visibility: meta.visibility,
        games,
        size_bytes,
        created_at,
    }
}

/// Offers the database to a user (owner only); an earlier offer of it goes.
pub async fn offer(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    blocking(move || {
        let (meta, _) = require(&state.store, &auth.user, &id, Access::Owner)?;
        let req: OfferRequest = json_body(&body)?;
        let no_such = || {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "noSuchUser",
                format!("No enabled user is called {}", req.username),
            )
        };
        let (to, _) = state.store.user_by_name(req.username.trim())?.ok_or_else(no_such)?;
        if to.disabled {
            return Err(no_such());
        }
        if to.id == auth.user.id {
            return Err(ApiError::bad_request("You own this database already"));
        }
        state.store.offer(&meta.id, auth.user.id, to.id)?;
        log::info!("{} offered database {} to {}", auth.user.username, meta.id, to.username);
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// Withdraws the offer (owner only).
pub async fn withdraw(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    blocking(move || {
        let (meta, _) = require(&state.store, &auth.user, &id, Access::Owner)?;
        if !state.store.withdraw_offer(&meta.id)? {
            return Err(ApiError::not_found());
        }
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// The offers made to the caller and by them.
pub async fn list(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<Json<TransfersDto>> {
    blocking(move || {
        let mut out = TransfersDto { incoming: Vec::new(), outgoing: Vec::new() };
        for (meta, from, to, at) in state.store.offers_for(auth.user.id)? {
            let mine = meta.owner_id == auth.user.id;
            let t = dto(&state, meta, from, to, at);
            if mine { out.outgoing.push(t) } else { out.incoming.push(t) }
        }
        Ok(Json(out))
    })
    .await
}

/// The recipient takes the database: refused (507) when it doesn't fit in
/// their quota. Any other user gets "not found", as for the database.
pub async fn accept(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    blocking(move || {
        if !crate::secret::is_db_id(&id) || state.store.offer_to(&id, auth.user.id)?.is_none() {
            return Err(ApiError::not_found());
        }
        if let Some(quota) = auth.user.quota_bytes {
            let used: u64 =
                state.store.databases_owned_by(auth.user.id)?.iter().map(|d| state.pool.stats(&d.id).1).sum();
            let size = state.pool.stats(&id).1;
            if used.saturating_add(size) > quota {
                return Err(ApiError::new(
                    StatusCode::INSUFFICIENT_STORAGE,
                    "quotaExceeded",
                    "The database doesn't fit in your quota: ask the admin for more room, or make room first",
                ));
            }
        }
        if !state.store.accept_offer(&id, auth.user.id)? {
            return Err(ApiError::not_found());
        }
        log::info!("{} took over database {id}", auth.user.username);
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// The recipient turns the offer down.
pub async fn decline(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    blocking(move || {
        if !crate::secret::is_db_id(&id) || state.store.offer_to(&id, auth.user.id)?.is_none() {
            return Err(ApiError::not_found());
        }
        state.store.withdraw_offer(&id)?;
        log::info!("{} declined database {id}", auth.user.username);
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}
