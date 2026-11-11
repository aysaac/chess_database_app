//! The admin's routes: users (deleting one with their databases, reset
//! links), invites, databases as metadata (name, owner, size,
//! game count, quota; deleting one), jobs, disk use and the import folder.
//!
//! Nothing here returns a database's games, positions or search results,
//! and nothing changes a database's visibility, shares or owner: those are
//! the owner's (`routes/databases.rs`), and the admin is an owner only of
//! their own databases.

use std::path::Path as FsPath;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;

use crate::access::access;
use crate::audit::Target;
use crate::auth::AdminAuth;
use crate::error::{ApiError, ApiResult};
use crate::routes::account::{hash, page_link};
use crate::routes::check_username;
use crate::routes::databases::delete_now;
use crate::routes::imports::import_kind;
use crate::secret::{is_db_id, new_token, token_digest};
use crate::store::{AuditFilter, DbMeta, Store, User, now};
use crate::wire::{
    AdminDatabaseDto, AdminJobDto, AdminUpdateDatabase, AuditEntryDto, AuditPage, AuditQuery, Confirm,
    CreateInvite, CreateUser, DiskDto, ImportFileDto, InviteDto, LinkDto, OwnedDatabaseDto, Role,
    SetPassword, UpdateUser, UserDto,
};
use crate::wire::{BackupsDto, JobStarted, TwicDto, TwicSettings};
use crate::{AppState, blocking};

fn user_dto(state: &AppState, user: User) -> ApiResult<UserDto> {
    let dbs = state.store.databases_owned_by(user.id)?;
    let used_bytes = dbs.iter().map(|d| state.pool.stats(&d.id).1).sum();
    Ok(UserDto {
        id: user.id,
        username: user.username,
        role: user.role,
        disabled: user.disabled,
        quota_bytes: user.quota_bytes,
        created_at: user.created_at,
        databases: dbs.len() as u32,
        used_bytes,
    })
}

pub async fn users(State(state): State<Arc<AppState>>, _: AdminAuth) -> ApiResult<Json<Vec<UserDto>>> {
    blocking(move || {
        let users = state.store.users()?;
        Ok(Json(users.into_iter().map(|u| user_dto(&state, u)).collect::<ApiResult<_>>()?))
    })
    .await
}

pub async fn create_user(
    State(state): State<Arc<AppState>>,
    AdminAuth(auth): AdminAuth,
    Json(req): Json<CreateUser>,
) -> ApiResult<(StatusCode, Json<UserDto>)> {
    check_username(&req.username)?;
    let hash = hash(&state, req.password).await?;
    blocking(move || {
        let user = state.store.create_user(&req.username, &hash, req.role.unwrap_or(Role::User))?;
        log::info!("{} made user {} ({})", auth.user.username, user.username, user.role.as_str());
        let detail = Some(user.role.as_str().to_owned());
        state.audit(
            "user.create",
            Some(&auth.user),
            Target::User(user.id, &user.username),
            detail,
            Some(&auth.client),
        );
        Ok((StatusCode::CREATED, Json(user_dto(&state, user)?)))
    })
    .await
}

fn target(store: &Store, id: &str) -> ApiResult<User> {
    let id: i64 = id.parse().map_err(|_| ApiError::not_found())?;
    store.user(id)?.ok_or_else(ApiError::not_found)
}

/// Changes a user's role, disables or enables them (their sessions and
/// tokens stop at once; their databases stay), or sets their quota.
pub async fn update_user(
    State(state): State<Arc<AppState>>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<String>,
    Json(req): Json<UpdateUser>,
) -> ApiResult<Json<UserDto>> {
    blocking(move || {
        let user = target(&state.store, &id)?;
        let demoted = req.role == Some(Role::User) && user.is_admin();
        let disabled = req.disabled == Some(true) && !user.disabled;
        if user.id == auth.user.id && (demoted || disabled) {
            return Err(ApiError::conflict("You can't disable yourself or take away your own admin role"));
        }
        if user.is_admin() && !user.disabled && (demoted || disabled) && state.store.active_admins()? <= 1 {
            return Err(ApiError::conflict("The server needs an enabled admin"));
        }
        state.store.update_user(user.id, req.role, req.disabled, req.quota_bytes)?;
        if disabled {
            state.store.delete_sessions_of(user.id, None)?;
        }
        log::info!("{} changed user {}: {:?}", auth.user.username, user.username, req);
        let who = || Target::User(user.id, &user.username);
        if let Some(role) = req.role.filter(|r| *r != user.role) {
            state.audit(
                "user.role",
                Some(&auth.user),
                who(),
                Some(role.as_str().to_owned()),
                Some(&auth.client),
            );
        }
        match req.disabled {
            Some(true) if !user.disabled => {
                state.audit("user.disable", Some(&auth.user), who(), None, Some(&auth.client))
            }
            Some(false) if user.disabled => {
                state.audit("user.enable", Some(&auth.user), who(), None, Some(&auth.client))
            }
            _ => {}
        }
        if let Some(q) = req.quota_bytes {
            let detail = Some(q.map_or_else(|| "none".to_owned(), |q| format!("{q} bytes")));
            state.audit("user.quota", Some(&auth.user), who(), detail, Some(&auth.client));
        }
        let user = state.store.user(user.id)?.ok_or_else(ApiError::not_found)?;
        Ok(Json(user_dto(&state, user)?))
    })
    .await
}

/// Sets a new password (the user's sessions end; their API tokens stay).
pub async fn set_password(
    State(state): State<Arc<AppState>>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<String>,
    Json(req): Json<SetPassword>,
) -> ApiResult<StatusCode> {
    let st = state.clone();
    let user = blocking(move || target(&st.store, &id)).await?;
    let hash = hash(&state, req.password).await?;
    blocking(move || {
        state.store.set_password(user.id, &hash)?;
        state.store.delete_sessions_of(user.id, None)?;
        log::info!("{} set a new password for {}", auth.user.username, user.username);
        state.audit(
            "password.set",
            Some(&auth.user),
            Target::User(user.id, &user.username),
            None,
            Some(&auth.client),
        );
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// The databases a user owns (metadata only): what deleting them deletes.
pub async fn user_databases(
    State(state): State<Arc<AppState>>,
    _: AdminAuth,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<OwnedDatabaseDto>>> {
    blocking(move || {
        let user = target(&state.store, &id)?;
        let dbs = state.store.databases_owned_by(user.id)?;
        Ok(Json(
            dbs.into_iter()
                .map(|d| {
                    let (games, size_bytes) = state.pool.stats(&d.id);
                    OwnedDatabaseDto { id: d.id, name: d.name, games, size_bytes }
                })
                .collect(),
        ))
    })
    .await
}

/// Deletes a user, confirmed with their username (`?confirm=NAME`): their
/// jobs are cancelled, the databases they own are deleted with them (they
/// can't go to the admin: that would let the admin read them), and their
/// sessions, tokens, shares, offers and reset links go. Not oneself, nor
/// the last enabled admin.
pub async fn delete_user(
    State(state): State<Arc<AppState>>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<String>,
    Query(confirm): Query<Confirm>,
) -> ApiResult<StatusCode> {
    blocking(move || {
        let user = target(&state.store, &id)?;
        if user.id == auth.user.id {
            return Err(ApiError::conflict("You can't delete yourself"));
        }
        if user.is_admin() && !user.disabled && state.store.active_admins()? <= 1 {
            return Err(ApiError::conflict("The server needs an enabled admin"));
        }
        if confirm.confirm.trim() != user.username {
            return Err(ApiError::bad_request(
                "To delete a user, confirm with their username (?confirm=NAME)",
            ));
        }
        // Their sessions and tokens stop at once, then their work.
        state.store.update_user(user.id, None, Some(true), None)?;
        state.jobs.stop_all_by(user.id);
        let dbs = state.store.databases_owned_by(user.id)?;
        for d in &dbs {
            state.jobs.stop_all_for(&d.id);
            state.store.delete_database(&d.id)?;
            state.pool.remove(&d.id)?;
        }
        state.store.delete_user(user.id)?;
        log::info!("{} deleted user {} and their {} databases", auth.user.username, user.username, dbs.len());
        let detail = Some(format!("with {} databases", dbs.len()));
        state.audit(
            "user.delete",
            Some(&auth.user),
            Target::User(user.id, &user.username),
            detail,
            Some(&auth.client),
        );
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// A one-time link that sets a new password for a user (the admin never
/// knows it); earlier links stop working. Using it ends their sessions.
pub async fn reset_link(
    State(state): State<Arc<AppState>>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<String>,
) -> ApiResult<(StatusCode, Json<LinkDto>)> {
    blocking(move || {
        let user = target(&state.store, &id)?;
        let token = new_token();
        let expires_at = now() + state.config.reset_lifetime.as_secs() as i64;
        state.store.create_reset(user.id, &token_digest(&token), expires_at)?;
        log::info!("{} made a reset link for {}", auth.user.username, user.username);
        state.audit(
            "password.reset_link",
            Some(&auth.user),
            Target::User(user.id, &user.username),
            None,
            Some(&auth.client),
        );
        let url = page_link(&state.config, "reset", &token);
        Ok((StatusCode::CREATED, Json(LinkDto { token, url, expires_at, id: None })))
    })
    .await
}

/// A single-use invite link, expiring after `days` (7 by default, at most
/// 30), that makes an account with `role` and `quotaBytes`.
pub async fn create_invite(
    State(state): State<Arc<AppState>>,
    AdminAuth(auth): AdminAuth,
    Json(req): Json<CreateInvite>,
) -> ApiResult<(StatusCode, Json<LinkDto>)> {
    let days = req.days.unwrap_or(0);
    if days > 30 {
        return Err(ApiError::bad_request("An invite works for 30 days at most"));
    }
    blocking(move || {
        let token = new_token();
        let lifetime =
            if days == 0 { state.config.invite_lifetime.as_secs() } else { u64::from(days) * 24 * 3600 };
        let expires_at = now() + lifetime as i64;
        let role = req.role.unwrap_or(Role::User);
        let id = state.store.create_invite(
            &token_digest(&token),
            role,
            req.quota_bytes,
            auth.user.id,
            expires_at,
        )?;
        log::info!("{} made an invite ({})", auth.user.username, role.as_str());
        state.audit(
            "invite.create",
            Some(&auth.user),
            Target::Invite(id),
            Some(role.as_str().to_owned()),
            Some(&auth.client),
        );
        let url = page_link(&state.config, "invite", &token);
        Ok((StatusCode::CREATED, Json(LinkDto { token, url, expires_at, id: Some(id) })))
    })
    .await
}

pub async fn invites(State(state): State<Arc<AppState>>, _: AdminAuth) -> ApiResult<Json<Vec<InviteDto>>> {
    blocking(move || {
        Ok(Json(
            state
                .store
                .invites()?
                .into_iter()
                .map(|i| InviteDto {
                    id: i.id,
                    role: i.role,
                    quota_bytes: i.quota_bytes,
                    created_by: i.created_by,
                    created_at: i.created_at,
                    expires_at: i.expires_at,
                    used_at: i.used_at,
                    used_by: i.used_by,
                })
                .collect(),
        ))
    })
    .await
}

pub async fn delete_invite(
    State(state): State<Arc<AppState>>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let id: i64 = id.parse().map_err(|_| ApiError::not_found())?;
    blocking(move || {
        if !state.store.delete_invite(id)? {
            return Err(ApiError::not_found());
        }
        log::info!("{} withdrew invite {id}", auth.user.username);
        state.audit("invite.delete", Some(&auth.user), Target::Invite(id), None, Some(&auth.client));
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

fn db_dto(state: &AppState, admin: &User, meta: DbMeta) -> ApiResult<AdminDatabaseDto> {
    let (games, size_bytes) = state.pool.stats(&meta.id);
    let share = if meta.owner_id == admin.id { None } else { state.store.share(&meta.id, admin.id)? };
    let access = access(admin, &meta, share);
    Ok(AdminDatabaseDto {
        id: meta.id,
        name: meta.name,
        owner: meta.owner,
        owner_id: meta.owner_id,
        visibility: meta.visibility,
        games,
        size_bytes,
        quota_bytes: meta.quota_bytes,
        created_at: meta.created_at,
        access,
    })
}

fn database(store: &Store, id: &str) -> ApiResult<DbMeta> {
    if !is_db_id(id) {
        return Err(ApiError::not_found());
    }
    store.database(id)?.ok_or_else(ApiError::not_found)
}

/// Every database, as metadata.
pub async fn databases(
    State(state): State<Arc<AppState>>,
    AdminAuth(auth): AdminAuth,
) -> ApiResult<Json<Vec<AdminDatabaseDto>>> {
    blocking(move || {
        let all = state.store.all_databases()?;
        Ok(Json(all.into_iter().map(|m| db_dto(&state, &auth.user, m)).collect::<ApiResult<_>>()?))
    })
    .await
}

/// Sets a database's quota, the one thing the admin changes on any
/// database (unknown fields, such as `visibility` or `owner`, are refused).
pub async fn update_database(
    State(state): State<Arc<AppState>>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<String>,
    Json(req): Json<AdminUpdateDatabase>,
) -> ApiResult<Json<AdminDatabaseDto>> {
    blocking(move || {
        let meta = database(&state.store, &id)?;
        if let Some(q) = req.quota_bytes {
            state.store.set_database_quota(&meta.id, q)?;
            log::info!("{} set the quota of database {} to {q:?}", auth.user.username, meta.id);
            let detail = Some(q.map_or_else(|| "none".to_owned(), |q| format!("{q} bytes")));
            state.audit(
                "database.quota",
                Some(&auth.user),
                Target::Database(&meta),
                detail,
                Some(&auth.client),
            );
        }
        Ok(Json(db_dto(&state, &auth.user, database(&state.store, &id)?)?))
    })
    .await
}

/// Deletes any database, confirmed with its name (`?confirm=NAME`).
pub async fn delete_database(
    State(state): State<Arc<AppState>>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<String>,
    Query(confirm): Query<Confirm>,
) -> ApiResult<StatusCode> {
    blocking(move || {
        let meta = database(&state.store, &id)?;
        delete_now(&state, &meta, &auth.user, &confirm.confirm, Some(&auth.client))?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// Every job: what runs on which database for whom, and how far it is, but
/// not its label, message or report (they can name a file or count games).
pub async fn jobs(State(state): State<Arc<AppState>>, _: AdminAuth) -> ApiResult<Json<Vec<AdminJobDto>>> {
    blocking(move || {
        Ok(Json(
            state
                .jobs
                .all()
                .into_iter()
                .map(|(s, m)| AdminJobDto {
                    id: s.id,
                    kind: s.kind,
                    state: s.state,
                    done: s.done,
                    total: s.total,
                    database: m.db,
                    database_name: m.db_name,
                    started_by: m.username,
                    started_at: m.started_at,
                })
                .collect(),
        ))
    })
    .await
}

pub async fn cancel_job(
    State(state): State<Arc<AppState>>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let id: u64 = id.parse().map_err(|_| ApiError::not_found())?;
    if state.jobs.info(id).is_none() {
        return Err(ApiError::not_found());
    }
    state.jobs.manager.cancel(id);
    log::info!("{} cancelled job {id}", auth.user.username);
    crate::audit::note(&state, move |s| {
        s.audit("job.cancel", Some(&auth.user), Target::Job(id), None, Some(&auth.client));
    })
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// The backups on the server's disk: names, sizes and dates only.
pub async fn backups(State(state): State<Arc<AppState>>, _: AdminAuth) -> ApiResult<Json<BackupsDto>> {
    blocking(move || {
        Ok(Json(BackupsDto {
            configured: state.config.backup_dir.is_some(),
            every_hours: state.config.backup_every.map(|d| d.as_secs() / 3600),
            keep: state.config.backup_keep,
            backups: crate::backup::list(&state),
        }))
    })
    .await
}

/// Makes a backup now, as a job.
pub async fn backup_now(
    State(state): State<Arc<AppState>>,
    AdminAuth(auth): AdminAuth,
) -> ApiResult<(StatusCode, Json<JobStarted>)> {
    let job = blocking(move || crate::backup::start(&state, Some(&auth.user), Some(&auth.client))).await?;
    Ok((StatusCode::ACCEPTED, Json(JobStarted { job })))
}

/// The TWIC updater's settings and the issues it took (NET-06).
pub async fn twic(State(state): State<Arc<AppState>>, _: AdminAuth) -> ApiResult<Json<TwicDto>> {
    blocking(move || Ok(Json(crate::twic::view(&state)?))).await
}

/// Sets the database TWIC goes into (one the admin owns; `null` turns it
/// off) and the days between checks.
pub async fn set_twic(
    State(state): State<Arc<AppState>>,
    AdminAuth(auth): AdminAuth,
    body: axum::body::Bytes,
) -> ApiResult<Json<TwicDto>> {
    blocking(move || {
        let req: TwicSettings = crate::routes::json_body(&body)?;
        crate::twic::configure(&state, &auth, req.database, req.every_days)?;
        Ok(Json(crate::twic::view(&state)?))
    })
    .await
}

/// Checks for new issues now, as a job.
pub async fn twic_check(
    State(state): State<Arc<AppState>>,
    AdminAuth(auth): AdminAuth,
) -> ApiResult<(StatusCode, Json<JobStarted>)> {
    let job = blocking(move || crate::twic::start(&state, Some(&auth))).await?;
    Ok((StatusCode::ACCEPTED, Json(JobStarted { job })))
}

/// The audit log, newest first, filtered by action, username and dates.
pub async fn audit(
    State(state): State<Arc<AppState>>,
    _: AdminAuth,
    Query(q): Query<AuditQuery>,
) -> ApiResult<Json<AuditPage>> {
    blocking(move || {
        let blank = |s: Option<String>| s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
        let filter = AuditFilter { action: blank(q.action), who: blank(q.who), from: q.from, to: q.to };
        let (total, rows) = state.store.audit_entries(&filter, q.offset, q.limit.unwrap_or(100).min(500))?;
        Ok(Json(AuditPage {
            total,
            offset: q.offset,
            days: state.config.audit_days,
            entries: rows
                .into_iter()
                .map(|r| AuditEntryDto {
                    at: r.at,
                    actor: r.actor,
                    action: r.action,
                    target_kind: r.target_kind,
                    target_id: r.target_id,
                    target: r.target,
                    detail: r.detail,
                    client: r.client,
                })
                .collect(),
        }))
    })
    .await
}

/// Total and free bytes of the file system holding `path`.
#[cfg(unix)]
fn fs_space(path: &FsPath) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs fills the zeroed struct from a valid NUL-terminated path.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let block = s.f_frsize as u64;
    Some((s.f_blocks as u64 * block, s.f_bavail as u64 * block))
}

#[cfg(not(unix))]
fn fs_space(_path: &FsPath) -> Option<(u64, u64)> {
    None
}

/// Bytes under `dir`, its folders included.
fn tree_size(dir: &FsPath) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.filter_map(|e| e.ok())
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => tree_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

pub async fn disk(State(state): State<Arc<AppState>>, _: AdminAuth) -> ApiResult<Json<DiskDto>> {
    blocking(move || {
        let c = &state.config;
        let dbs = state.store.all_databases()?;
        let space = fs_space(&c.data_dir);
        Ok(Json(DiskDto {
            data_dir: c.data_dir.display().to_string(),
            fs_total_bytes: space.map(|s| s.0),
            fs_free_bytes: space.map(|s| s.1),
            databases: dbs.len() as u32,
            databases_bytes: tree_size(&c.dbs_dir()),
            metadata_bytes: Store::size_bytes(&c.store_path()),
            uploads_bytes: tree_size(&c.tmp_dir()),
            import_bytes: tree_size(&c.import_dir()),
        }))
    })
    .await
}

/// Files listed at most, and folders deep.
const MAX_FILES: usize = 5000;
const MAX_DEPTH: usize = 6;

fn walk(root: &FsPath, rel: &mut Vec<String>, out: &mut Vec<ImportFileDto>) {
    let Ok(rd) = std::fs::read_dir(root.join(rel.join("/"))) else { return };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if out.len() >= MAX_FILES {
            return;
        }
        let Some(name) = e.file_name().to_str().map(str::to_owned) else { continue };
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() && rel.len() < MAX_DEPTH {
            rel.push(name);
            walk(root, rel, out);
            rel.pop();
        } else if t.is_file()
            && let Some(kind) = import_kind(FsPath::new(&name))
        {
            let path = rel.iter().chain(std::iter::once(&name)).cloned().collect::<Vec<_>>().join("/");
            out.push(ImportFileDto { path, size_bytes: e.metadata().map_or(0, |m| m.len()), kind });
        }
    }
}

/// The PGN and `.cbh` files in the import folder (links aren't followed).
pub async fn import_files(
    State(state): State<Arc<AppState>>,
    _: AdminAuth,
) -> ApiResult<Json<Vec<ImportFileDto>>> {
    blocking(move || {
        let mut out = Vec::new();
        walk(&state.config.import_dir(), &mut Vec::new(), &mut out);
        Ok(Json(out))
    })
    .await
}
