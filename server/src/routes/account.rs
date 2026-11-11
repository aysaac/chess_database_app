//! Health, first-run setup, logging in and out, one's password and API
//! tokens.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Json;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::auth::{Auth, client_ip, session_cookie};
use crate::error::{ApiError, ApiResult};
use crate::routes::{check_name, check_username};
use crate::secret::{self, check_new_password, ct_eq, token_digest};
use crate::store::User;
use crate::wire::{
    LoginRequest, MeDto, NewTokenDto, PasswordChange, SetupRequest, SetupStatusDto, TokenDto, TokenRequest,
};
use crate::{AppState, blocking};

pub async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

fn me_dto(user: &User, csrf: Option<String>) -> MeDto {
    MeDto { id: user.id, username: user.username.clone(), role: user.role, csrf_token: csrf }
}

/// Hashes a password on the blocking pool, a few at a time.
pub async fn hash(state: &Arc<AppState>, password: String) -> ApiResult<String> {
    check_new_password(&password).map_err(ApiError::bad_request)?;
    let _permit = state.hashing.acquire().await.map_err(ApiError::internal)?;
    blocking(move || secret::hash_password(&password).map_err(ApiError::internal)).await
}

pub async fn setup_status(State(state): State<Arc<AppState>>) -> ApiResult<Json<SetupStatusDto>> {
    let st = state.clone();
    let users = blocking(move || st.store.user_count()).await?;
    Ok(Json(SetupStatusDto { needed: users == 0 && state.setup_token().is_some() }))
}

/// Makes the first admin, with the setup token from the log.
pub async fn setup(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetupRequest>,
) -> ApiResult<(StatusCode, Json<MeDto>)> {
    let expected = state.setup_token().ok_or_else(|| ApiError::conflict("The server is set up already"))?;
    if !ct_eq(req.token.trim().as_bytes(), expected.as_bytes()) {
        return Err(ApiError::forbidden("Wrong setup token: it's in the server's log"));
    }
    check_username(&req.username)?;
    let hash = hash(&state, req.password).await?;
    let st = state.clone();
    let user = blocking(move || {
        st.store
            .create_first_admin(&req.username, &hash)?
            .ok_or_else(|| ApiError::conflict("The server is set up already"))
    })
    .await?;
    *state.setup_token.lock().expect("setup token lock") = None;
    log::info!("first admin {} made with the setup token", user.username);
    Ok((StatusCode::CREATED, Json(me_dto(&user, None))))
}

/// User `username` if `password` is theirs (disabled or not), throttled as
/// logins are, per account and per client address (`ip`): the attempt
/// counts as a failure before it's hashed, so concurrent guesses each count
/// and a burst gets no more than the limit. A right password takes it back
/// and clears the account's failures. Every check of a password goes
/// through this.
pub async fn verify_throttled(
    state: &Arc<AppState>,
    username: &str,
    password: String,
    ip: &str,
) -> ApiResult<Option<User>> {
    let account_key = format!("user:{}", username.to_lowercase());
    let keys = [account_key.clone(), format!("ip:{ip}")];
    state.throttle.attempt(&keys)?;
    let found = {
        let _permit = state.hashing.acquire().await.map_err(ApiError::internal)?;
        let st = state.clone();
        let username = username.to_owned();
        blocking(move || {
            Ok(match st.store.user_by_name(&username)? {
                Some((user, hash)) => secret::verify_password(&password, &hash).then_some(user),
                None => {
                    secret::verify_nothing(&password);
                    None
                }
            })
        })
        .await?
    };
    if found.is_some() {
        state.throttle.undo(&keys);
        state.throttle.clear(&account_key);
    }
    Ok(found)
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> ApiResult<Response> {
    let ip = client_ip(&state.config, &headers, peer);
    let Some(user) = verify_throttled(&state, &req.username, req.password, &ip).await? else {
        log::info!("failed login as {:?} from {ip}", req.username);
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "badLogin", "Wrong username or password"));
    };
    if user.disabled {
        return Err(ApiError::forbidden("This account is disabled"));
    }
    let (token, csrf) = (secret::new_token(), secret::new_token());
    let lifetime = state.config.session_lifetime.as_secs();
    let st = state.clone();
    let (hash, id, c) = (token_digest(&token), user.id, csrf.clone());
    blocking(move || st.store.create_session(&hash, id, &c, lifetime as i64)).await?;
    log::info!("{} logged in from {ip}", user.username);
    let cookie = session_cookie(&state.config, &headers, Some(&token), lifetime);
    Ok(([(header::SET_COOKIE, cookie)], Json(me_dto(&user, Some(csrf)))).into_response())
}

pub async fn logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    auth: Auth,
) -> ApiResult<Response> {
    if let Some(session) = auth.session {
        let st = state.clone();
        blocking(move || st.store.delete_session(&session.token_hash)).await?;
    }
    let cookie = session_cookie(&state.config, &headers, None, 0);
    Ok(([(header::SET_COOKIE, cookie)], StatusCode::NO_CONTENT).into_response())
}

pub async fn me(auth: Auth) -> Json<MeDto> {
    Json(me_dto(&auth.user, auth.session.map(|s| s.csrf)))
}

/// Changes one's password; one's other sessions end. Wrong current
/// passwords are throttled as failed logins are (the same account's).
pub async fn change_password(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    auth: Auth,
    Json(req): Json<PasswordChange>,
) -> ApiResult<StatusCode> {
    let ip = client_ip(&state.config, &headers, peer);
    if verify_throttled(&state, &auth.user.username, req.current_password, &ip).await?.is_none() {
        return Err(ApiError::forbidden("The current password is wrong"));
    }
    let hash = hash(&state, req.new_password).await?;
    let st = state.clone();
    let keep = auth.session.map(|s| s.token_hash);
    let id = auth.user.id;
    blocking(move || {
        st.store.set_password(id, &hash)?;
        st.store.delete_sessions_of(id, keep.as_deref())
    })
    .await?;
    log::info!("{} changed their password", auth.user.username);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn tokens(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<Json<Vec<TokenDto>>> {
    let rows = blocking(move || state.store.tokens(auth.user.id)).await?;
    Ok(Json(
        rows.into_iter()
            .map(|t| TokenDto {
                id: t.id,
                name: t.name,
                created_at: t.created_at,
                last_used_at: t.last_used_at,
            })
            .collect(),
    ))
}

/// A new API token for the caller; the response is the only time it's shown.
pub async fn create_token(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Json(req): Json<TokenRequest>,
) -> ApiResult<(StatusCode, Json<NewTokenDto>)> {
    let name = check_name(&req.name)?;
    let token = secret::new_api_token();
    let hash = token_digest(&token);
    let row = blocking(move || state.store.create_token(auth.user.id, &name, &hash)).await?;
    Ok((StatusCode::CREATED, Json(NewTokenDto { id: row.id, name: row.name, token })))
}

pub async fn delete_token(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let id: i64 = id.parse().map_err(|_| ApiError::not_found())?;
    if blocking(move || state.store.delete_token(auth.user.id, id)).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}
