//! Health, first-run setup, logging in and out, signing up (with an invite,
//! or open sign-up), reset links and "Forgot password", one's password,
//! e-mail address and API tokens.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Json;
use axum::extract::{ConnectInfo, Extension, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::audit::{ClientAddr, Target, note};
use crate::auth::{Auth, client_ip, session_cookie};
use crate::error::{ApiError, ApiResult};
use crate::routes::{check_name, check_username};
use crate::secret::{self, check_new_password, ct_eq, token_digest};
use crate::store::{User, invalid_invite, now};
use crate::wire::{
    AccountDto, EmailChange, ForgotRequest, InviteInfoDto, LoginRequest, MeDto, NewTokenDto, PasswordChange,
    ResetRequest, SetupRequest, SetupStatusDto, SignupRequest, TokenDto, TokenRequest,
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
    Ok(Json(SetupStatusDto {
        needed: users == 0 && state.setup_token().is_some(),
        open_signup: state.config.open_signup,
        email_resets: state.config.email_resets(),
    }))
}

/// Makes the first admin, with the setup token from the log.
pub async fn setup(
    State(state): State<Arc<AppState>>,
    Extension(client): Extension<ClientAddr>,
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
    let u = user.clone();
    note(&state, move |s| {
        let detail = Some("the first admin, with the setup token".to_owned());
        s.audit("user.create", Some(&u), Target::User(u.id, &u.username), detail, Some(&client.0));
    })
    .await;
    Ok((StatusCode::CREATED, Json(me_dto(&user, None))))
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> ApiResult<Response> {
    let ip = client_ip(&state.config, &headers, peer);
    let account_key = format!("user:{}", req.username.to_lowercase());
    let keys = [account_key.clone(), format!("ip:{ip}")];
    state.throttle.check(&keys)?;
    let found = {
        let _permit = state.hashing.acquire().await.map_err(ApiError::internal)?;
        let st = state.clone();
        let (username, password) = (req.username.clone(), req.password);
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
    let Some(user) = found else {
        state.throttle.fail(&keys);
        log::info!("failed login as {:?} from {ip}", req.username);
        let (name, client) = (req.username.clone(), ip.clone());
        note(&state, move |s| s.audit("login.failed", None, Target::Name(&name), None, Some(&client))).await;
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "badLogin", "Wrong username or password"));
    };
    if user.disabled {
        let (u, client) = (user.clone(), ip.clone());
        note(&state, move |s| {
            let detail = Some("the account is disabled".to_owned());
            s.audit("login.failed", None, Target::User(u.id, &u.username), detail, Some(&client));
        })
        .await;
        return Err(ApiError::forbidden("This account is disabled"));
    }
    state.throttle.clear(&account_key);
    log::info!("{} logged in from {ip}", user.username);
    let (u, client) = (user.clone(), ip.clone());
    note(&state, move |s| s.audit("login", Some(&u), Target::None, None, Some(&client))).await;
    start_session(&state, &headers, &user, StatusCode::OK).await
}

/// Logs `user` in: a new session, its cookie, and the user with the
/// session's CSRF token.
async fn start_session(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    user: &User,
    status: StatusCode,
) -> ApiResult<Response> {
    let (token, csrf) = (secret::new_token(), secret::new_token());
    let lifetime = state.config.session_lifetime.as_secs();
    let st = state.clone();
    let (hash, id, c) = (token_digest(&token), user.id, csrf.clone());
    blocking(move || st.store.create_session(&hash, id, &c, lifetime as i64)).await?;
    let cookie = session_cookie(&state.config, headers, Some(&token), lifetime);
    Ok((status, [(header::SET_COOKIE, cookie)], Json(me_dto(user, Some(csrf)))).into_response())
}

/// A link to a page of the web UI taking a one-time token: under the public
/// URL when there is one, else a path to put after the server's address.
pub fn page_link(config: &crate::config::Config, page: &str, token: &str) -> String {
    format!("{}/#/{page}/{token}", config.public_url.as_deref().unwrap_or(""))
}

/// Counts a request against `keys` (each one counts, not only failures)
/// and refuses it past the limit, as failed logins are.
fn throttle_every(state: &AppState, keys: &[String]) -> ApiResult<()> {
    state.throttle.check(keys)?;
    state.throttle.fail(keys);
    Ok(())
}

/// What an invite makes, while it can be used (no login). Wrong tokens are
/// throttled per client address.
pub async fn invite_info(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> ApiResult<Json<InviteInfoDto>> {
    let keys = [format!("token-ip:{}", client_ip(&state.config, &headers, peer))];
    state.throttle.check(&keys)?;
    let st = state.clone();
    let found = blocking(move || st.store.open_invite(&token_digest(token.trim()))).await?;
    match found {
        Some((role, _)) => Ok(Json(InviteInfoDto { role })),
        None => {
            state.throttle.fail(&keys);
            Err(invalid_invite())
        }
    }
}

fn check_email(email: Option<String>) -> ApiResult<Option<String>> {
    let email = email.map(|e| e.trim().to_owned()).filter(|e| !e.is_empty());
    if let Some(e) = &email
        && !crate::mail::plausible_address(e)
    {
        return Err(ApiError::bad_request("That isn't an e-mail address"));
    }
    Ok(email)
}

/// Makes an account and logs it in: with an invite (its role and quota;
/// used up), or without one when open sign-up is on (a user with the
/// sign-up quota). Throttled per client address, every attempt counting.
pub async fn signup(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<SignupRequest>,
) -> ApiResult<Response> {
    let ip = client_ip(&state.config, &headers, peer);
    let invite = req.invite.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(token_digest);
    // Every open sign-up counts against the address; with an invite, only
    // wrong ones do (a right one makes one account and is used up).
    let keys = [format!("signup-ip:{ip}")];
    match &invite {
        None if !state.config.open_signup => {
            return Err(ApiError::forbidden(
                "Making an account needs an invite: ask the server's admin for one",
            ));
        }
        None => throttle_every(&state, &keys)?,
        Some(_) => state.throttle.check(&keys)?,
    }
    check_username(&req.username)?;
    let email = check_email(req.email)?;
    // A wrong invite is refused before the password is hashed.
    if let Some(hash) = &invite {
        let st = state.clone();
        let h = hash.clone();
        if blocking(move || st.store.open_invite(&h)).await?.is_none() {
            state.throttle.fail(&keys);
            return Err(invalid_invite());
        }
    }
    let hash = hash(&state, req.password).await?;
    let st = state.clone();
    let quota = state.config.signup_quota;
    let username = req.username.clone();
    let user =
        blocking(move || st.store.sign_up(invite.as_deref(), &username, &hash, email.as_deref(), quota))
            .await?;
    log::info!(
        "{} signed up from {ip} ({}, {})",
        user.username,
        user.role.as_str(),
        if req.invite.is_some() { "invited" } else { "open sign-up" }
    );
    let (u, invited) = (user.clone(), req.invite.is_some());
    note(&state, move |s| {
        let how = if invited { "with an invite" } else { "open sign-up" };
        let detail = Some(format!("{}, {how}", u.role.as_str()));
        s.audit("user.create", Some(&u), Target::User(u.id, &u.username), detail, Some(&ip));
    })
    .await;
    start_session(&state, &headers, &user, StatusCode::CREATED).await
}

fn bad_reset() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "badReset",
        "This link is used up, has expired or doesn't exist: ask for another",
    )
}

/// Sets a new password with a reset link's token (no login); every session
/// of the account ends. Wrong tokens are throttled per client address.
pub async fn reset_password(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<ResetRequest>,
) -> ApiResult<StatusCode> {
    let ip = client_ip(&state.config, &headers, peer);
    let keys = [format!("token-ip:{ip}")];
    state.throttle.check(&keys)?;
    let hash = hash(&state, req.password).await?;
    let st = state.clone();
    let token = token_digest(req.token.trim());
    match blocking(move || st.store.reset_password(&token, &hash)).await? {
        Some(user) => {
            log::info!("{} set a new password with a reset link", user.username);
            note(&state, move |s| s.audit("password.reset", Some(&user), Target::None, None, Some(&ip)))
                .await;
            Ok(StatusCode::NO_CONTENT)
        }
        None => {
            state.throttle.fail(&keys);
            Err(bad_reset())
        }
    }
}

/// "Forgot password": e-mails a reset link to the account with this
/// username or address, if it has an address and e-mail is set up. Always
/// the same answer, sent before anything is looked up, so it never tells
/// whether an account or an address exists; throttled per client address
/// and per name asked, every request counting.
pub async fn forgot_password(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<ForgotRequest>,
) -> ApiResult<StatusCode> {
    let login = req.login.trim().to_lowercase();
    let ip = client_ip(&state.config, &headers, peer);
    throttle_every(&state, &[format!("forgot-ip:{ip}"), format!("forgot:{login}")])?;
    if login.is_empty() || login.len() > 254 {
        return Err(ApiError::bad_request("Give your username or e-mail address"));
    }
    if state.config.email_resets() {
        let st = state.clone();
        // Not awaited: the answer takes as long whether a message goes or not.
        tokio::task::spawn_blocking(move || {
            if let Err(e) = send_reset(&st, &login, &ip) {
                log::warn!("couldn't e-mail a reset link: {e}");
            }
        });
    }
    Ok(StatusCode::ACCEPTED)
}

fn send_reset(state: &AppState, login: &str, client: &str) -> Result<(), String> {
    let Some((user, Some(email))) = state.store.user_by_login(login).map_err(|e| e.message)? else {
        return Ok(());
    };
    let (Some(smtp), true) = (&state.config.smtp, state.config.public_url.is_some()) else { return Ok(()) };
    let token = secret::new_token();
    let lifetime = state.config.email_reset_lifetime.as_secs();
    state
        .store
        .create_reset(user.id, &token_digest(&token), now() + lifetime as i64)
        .map_err(|e| e.message)?;
    crate::mail::send_reset_link(
        smtp,
        &email,
        &user.username,
        &page_link(&state.config, "reset", &token),
        lifetime / 60,
    )?;
    log::info!("e-mailed {} a reset link", user.username);
    state.audit("password.email", None, Target::User(user.id, &user.username), None, Some(client));
    Ok(())
}

/// One's own account: name, role, address, quota and use.
pub async fn account(State(state): State<Arc<AppState>>, auth: Auth) -> ApiResult<Json<AccountDto>> {
    blocking(move || {
        let used_bytes =
            state.store.databases_owned_by(auth.user.id)?.iter().map(|d| state.pool.stats(&d.id).1).sum();
        Ok(Json(AccountDto {
            username: auth.user.username.clone(),
            role: auth.user.role,
            email: state.store.email_of(auth.user.id)?,
            quota_bytes: auth.user.quota_bytes,
            used_bytes,
            email_resets: state.config.email_resets(),
        }))
    })
    .await
}

/// Sets one's e-mail address (or removes it), with one's password: it's
/// where reset links go.
pub async fn change_email(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Json(req): Json<EmailChange>,
) -> ApiResult<StatusCode> {
    let email = check_email(req.email)?;
    let ok = {
        let _permit = state.hashing.acquire().await.map_err(ApiError::internal)?;
        let st = state.clone();
        let (name, current) = (auth.user.username.clone(), req.current_password);
        blocking(move || {
            Ok(st
                .store
                .user_by_name(&name)?
                .is_some_and(|(_, hash)| secret::verify_password(&current, &hash)))
        })
        .await?
    };
    if !ok {
        return Err(ApiError::forbidden("The current password is wrong"));
    }
    let id = auth.user.id;
    let st = state.clone();
    blocking(move || st.store.set_email(id, email.as_deref())).await?;
    log::info!("{} changed their e-mail address", auth.user.username);
    note(&state, move |s| s.audit("email.change", Some(&auth.user), Target::None, None, Some(&auth.client)))
        .await;
    Ok(StatusCode::NO_CONTENT)
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

/// Changes one's password; one's other sessions end.
pub async fn change_password(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Json(req): Json<PasswordChange>,
) -> ApiResult<StatusCode> {
    let ok = {
        let _permit = state.hashing.acquire().await.map_err(ApiError::internal)?;
        let st = state.clone();
        let (name, current) = (auth.user.username.clone(), req.current_password);
        blocking(move || {
            Ok(st
                .store
                .user_by_name(&name)?
                .is_some_and(|(_, hash)| secret::verify_password(&current, &hash)))
        })
        .await?
    };
    if !ok {
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
    note(&state, move |s| {
        s.audit("password.change", Some(&auth.user), Target::None, None, Some(&auth.client))
    })
    .await;
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
    let row = blocking(move || {
        let row = state.store.create_token(auth.user.id, &name, &hash)?;
        state.audit(
            "token.create",
            Some(&auth.user),
            Target::Token(row.id, &row.name),
            None,
            Some(&auth.client),
        );
        Ok(row)
    })
    .await?;
    Ok((StatusCode::CREATED, Json(NewTokenDto { id: row.id, name: row.name, token })))
}

pub async fn delete_token(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let id: i64 = id.parse().map_err(|_| ApiError::not_found())?;
    let deleted = blocking(move || {
        let deleted = state.store.delete_token(auth.user.id, id)?;
        if deleted {
            state.audit("token.delete", Some(&auth.user), Target::Token(id, ""), None, Some(&auth.client));
        }
        Ok(deleted)
    })
    .await?;
    if deleted { Ok(StatusCode::NO_CONTENT) } else { Err(ApiError::not_found()) }
}
