//! Databases kept up to date (NET-01, IO-10, NET-03): choosing where a
//! database's games come from, and the update that brings the new ones
//! in as one job. Also the sign-in to the self-hosted server.

use std::path::Path;
use std::sync::Mutex;

use chess_db::Database;
use chess_jobs::{JobContext, JobError};
use chess_net::server::{self, CopyMark, CopySource};
use chess_net::study::{self, StudyRef};
use chess_net::{NetError, Period, Request};
use tauri::{AppHandle, Manager, State};

use crate::commands::db::{thousands, to_job_error};
use crate::commands::online::claim_download;
use crate::dto::sync::{DbSyncDto, ServerAccountDto, ServerDbDto, SyncSourceDto};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

fn internal(e: impl ToString) -> AppError {
    AppError::new(crate::error::ErrorCode::Internal, e.to_string())
}

// ---- The server sign-in ------------------------------------------------------

#[tauri::command]
pub async fn server_account(state: State<'_, AppState>) -> AppResult<ServerAccountDto> {
    Ok(crate::server_login::account(&state))
}

/// Signs in to the server at `url` with a username and password, which are
/// used once: the app keeps the API token the server makes for this computer.
#[tauri::command]
pub async fn server_sign_in(
    app: AppHandle,
    url: String,
    username: String,
    password: String,
) -> AppResult<ServerAccountDto> {
    let url = server::server_url(&url)?;
    let name = crate::server_login::token_name();
    let user = username.trim().to_owned();
    let signed = {
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let state = app.state::<AppState>();
            server::sign_in(state.server.client(), &url, &user, &password, &name).map(|s| (url, s))
        })
        .await
        .map_err(internal)??
    };
    let state = app.state::<AppState>();
    let (url, signed) = signed;
    let (_, replaced) = crate::server_login::keep(&state, &url, signed);
    if let Some(old) = replaced {
        crate::server_login::revoke_in_background(state.server.client(), old);
    }
    Ok(crate::server_login::account(&state))
}

/// Signs out: the app forgets the token, and the server revokes it.
#[tauri::command]
pub async fn server_sign_out(app: AppHandle) -> AppResult<ServerAccountDto> {
    let state = app.state::<AppState>();
    if let Some(old) = crate::server_login::forget(&state) {
        crate::server_login::revoke_in_background(state.server.client(), old);
    }
    Ok(crate::server_login::account(&state))
}

/// The databases the signed-in account may read on the server.
#[tauri::command]
pub async fn server_databases(app: AppHandle) -> AppResult<Vec<ServerDbDto>> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let login = crate::server_login::login(&state)?
            .ok_or_else(|| AppError::invalid("Sign in to the server first"))?;
        match server::databases(state.server.client(), &login.url, &login.token) {
            Ok(list) => Ok(list.into_iter().map(ServerDbDto::from).collect()),
            Err(e) => {
                if matches!(e, NetError::ServerSignInRejected) {
                    crate::server_login::forget_rejected(&state, &login.token);
                }
                Err(e.into())
            }
        }
    })
    .await
    .map_err(internal)?
}

// ---- Choosing the sources ----------------------------------------------------

/// A source as the app keeps it: a study's link read, a server's address
/// written one way, a username checked.
fn normalized(source: SyncSourceDto) -> AppResult<SyncSourceDto> {
    Ok(match source {
        SyncSourceDto::Player { site, username, speeds, rated_only, reached } => {
            let username = chess_net::check_username(&username)?.to_owned();
            SyncSourceDto::Player { site, username, speeds, rated_only, reached }
        }
        SyncSourceDto::Study { study, chapter, name, last_modified } => {
            // The study field may hold a pasted link.
            let r = study::parse_study_link(&study)?;
            let chapter = chapter.or(r.chapter);
            SyncSourceDto::Study { study: r.study, chapter, name, last_modified }
        }
        SyncSourceDto::Server { url, username, database, name, next, epoch } => {
            SyncSourceDto::Server { url: server::server_url(&url)?, username, database, name, next, epoch }
        }
    })
}

#[tauri::command]
pub async fn db_sync(state: State<'_, AppState>, handle: u32) -> AppResult<Option<DbSyncDto>> {
    let Some(path) = state.list_path(handle) else { return Ok(None) };
    Ok(state.settings.lock().expect("settings lock").synced(&path).cloned())
}

/// Sets where the database gets its games and whether it updates on its own
/// (no sources: it's no longer kept up to date). Sources it had keep how
/// far their updates got.
#[tauri::command]
pub async fn set_db_sync(
    state: State<'_, AppState>,
    handle: u32,
    sources: Vec<SyncSourceDto>,
    auto: bool,
) -> AppResult<Option<DbSyncDto>> {
    let path = state.list_path(handle).ok_or_else(|| AppError::not_found(format!("database {handle}")))?;
    let sources = sources.into_iter().map(normalized).collect::<AppResult<Vec<_>>>()?;
    Ok(state.update_settings(|s| {
        s.set_sources(&path, sources, auto);
        s.synced(&path).cloned()
    }))
}

// ---- The update --------------------------------------------------------------

/// What one source's update did, for the notice.
struct Outcome {
    text: String,
    failed: bool,
}

fn games(n: u64) -> String {
    if n == 1 { "1 new game".to_owned() } else { format!("{} new games", thousands(n)) }
}

/// Brings a player's new games in.
fn update_player(
    state: &AppState,
    db: &Mutex<Database>,
    path: &Path,
    source: &SyncSourceDto,
    ctx: &JobContext,
) -> Result<(SyncSourceDto, u64), NetError> {
    let SyncSourceDto::Player { site, username, speeds, rated_only, reached } = source else {
        unreachable!("a player's source")
    };
    let period = reached.map_or(Period::All, Period::Since);
    let req = Request {
        site: (*site).into(),
        username: username.clone(),
        speeds: crate::dto::speeds_of(speeds),
        rated_only: *rated_only,
        period,
    };
    let own = match req.site {
        chess_net::Site::Lichess => crate::lichess::login_of(state, username).unwrap_or_else(|e| {
            log::warn!("updating {username}'s games without the Lichess login: {e}");
            None
        }),
        chess_net::Site::ChessCom => None,
    };
    let token = own.as_ref().map(|l| l.token.clone());
    let client = match own {
        Some(l) => crate::commands::online::client().clone().with_lichess_login(&l.username, l.token),
        None => crate::commands::online::client().clone(),
    };
    let s = chess_net::download_into(&client, db, &req, ctx);
    if let Some(token) = &token
        && client.lichess_login_rejected()
    {
        crate::lichess::forget_rejected(state, token);
    }
    let s = s?;
    let mut next = source.clone();
    if let (SyncSourceDto::Player { reached, .. }, Some(covered)) = (&mut next, s.covered) {
        *reached = Some(reached.map_or(covered, |r| r.max(covered)));
    }
    state.update_settings(|st| st.note_source(path, &next));
    if let Some(e) = s.stopped {
        return Err(e);
    }
    if s.cancelled {
        return Err(NetError::Cancelled);
    }
    Ok((next, s.import.imported))
}

/// Brings a study's new chapters in.
fn update_study(
    state: &AppState,
    db: &Mutex<Database>,
    path: &Path,
    source: &SyncSourceDto,
    ctx: &JobContext,
) -> Result<(SyncSourceDto, u64), NetError> {
    let SyncSourceDto::Study { study, chapter, last_modified, .. } = source else {
        unreachable!("a study's source")
    };
    let link = StudyRef { study: study.clone(), chapter: chapter.clone() };
    let login = crate::lichess::login(state).ok().flatten();
    let token = login.as_ref().map(|l| &l.token);
    let client = state.lichess.client();
    let done = study::refresh_study_into(client, db, &link, token, last_modified.as_deref(), ctx);
    if matches!(done, Err(NetError::TokenRejected))
        && let Some(l) = &login
    {
        crate::lichess::forget_rejected(state, &l.token);
    }
    let done = done?;
    let mut next = source.clone();
    if let SyncSourceDto::Study { last_modified, .. } = &mut next {
        *last_modified = done.last_modified;
    }
    let imported = done.import.as_ref().map_or(0, |i| i.import.imported);
    state.update_settings(|st| st.note_source(path, &next));
    if done.import.is_some_and(|i| i.cancelled) {
        return Err(NetError::Cancelled);
    }
    Ok((next, imported))
}

/// Brings a server database's new games in.
fn update_server(
    state: &AppState,
    db: &Mutex<Database>,
    path: &Path,
    source: &SyncSourceDto,
    ctx: &JobContext,
) -> Result<(SyncSourceDto, u64), NetError> {
    let SyncSourceDto::Server { url, username, database, next, epoch, .. } = source else {
        unreachable!("a server's source")
    };
    let login = crate::server_login::login(state)
        .map_err(|e| NetError::ServerBadData { server: url.clone(), reason: e.message })?
        .filter(|l| l.url == *url && l.username.eq_ignore_ascii_case(username))
        .ok_or(NetError::ServerSignInRejected)?;
    let from = match (next, epoch) {
        (Some(next), Some(epoch)) => Some(CopyMark { next: *next, epoch: *epoch }),
        _ => None,
    };
    let src = CopySource { url, token: &login.token, id: database };
    let mut latest = source.clone();
    let result = server::copy_into(state.server.client(), src, from, db, ctx, &mut |m| {
        if let SyncSourceDto::Server { next, epoch, .. } = &mut latest {
            (*next, *epoch) = (Some(m.next), Some(m.epoch));
        }
        state.update_settings(|st| st.note_source(path, &latest));
    });
    if matches!(result, Err(NetError::ServerSignInRejected)) {
        crate::server_login::forget_rejected(state, &login.token);
    }
    let copy = result?;
    if let Some(e) = copy.stopped {
        return Err(e);
    }
    if copy.cancelled {
        return Err(NetError::Cancelled);
    }
    Ok((latest, copy.import.imported))
}

/// Updates every source of the database in order, noting after each how
/// far it got. A source that fails doesn't stop the others.
fn run_update(
    state: &AppState,
    db: &Mutex<Database>,
    path: &Path,
    sources: &[SyncSourceDto],
    ctx: &JobContext,
) -> Result<Vec<Outcome>, JobError> {
    let mut out = Vec::new();
    for source in sources {
        ctx.check_cancelled()?;
        let what = source.describe();
        let result = match source {
            SyncSourceDto::Player { .. } => update_player(state, db, path, source, ctx),
            SyncSourceDto::Study { .. } => update_study(state, db, path, source, ctx),
            SyncSourceDto::Server { .. } => update_server(state, db, path, source, ctx),
        };
        out.push(match result {
            Ok((_, 0)) => Outcome { text: format!("no new games from {what}"), failed: false },
            Ok((_, n)) => Outcome { text: format!("{} from {what}", games(n)), failed: false },
            Err(NetError::Cancelled) => return Err(JobError::Cancelled),
            Err(NetError::Db(e)) => return Err(to_job_error(e)),
            Err(e) => Outcome { text: format!("{what}: {e}"), failed: true },
        });
    }
    Ok(out)
}

/// Updates the database from its sources as a job, one download at a time.
/// Returns the job; its message says what came. A database with no sources
/// is an error.
#[tauri::command]
pub async fn sync_database(app: AppHandle, state: State<'_, AppState>, handle: u32) -> AppResult<u64> {
    let db = state.db(handle)?;
    let path = state.list_path(handle).ok_or_else(|| AppError::not_found(format!("database {handle}")))?;
    let sources = state
        .settings
        .lock()
        .expect("settings lock")
        .synced(&path)
        .map(|d| d.sources.clone())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::invalid("This database has no sources to update from"))?;
    let label = "Updating the database".to_owned();
    let slot = claim_download(&app, &label)?;
    state.import_reports.lock().expect("import reports lock").remove(&handle);
    Ok(state.jobs.spawn("sync", label, move |ctx| {
        let _slot = slot;
        let state = app.state::<AppState>();
        let result = run_update(&state, &db, &path, &sources, ctx);
        let (message, failed) = match &result {
            Ok(outcomes) => {
                let failed = outcomes.iter().any(|o| o.failed);
                let text: Vec<_> = outcomes.iter().map(|o| o.text.as_str()).collect();
                let mut m = text.join("; ");
                if let Some(c) = m.get_mut(..1) {
                    c.make_ascii_uppercase();
                }
                (m, failed)
            }
            Err(JobError::Cancelled) => ("Cancelled".to_owned(), true),
            Err(e) => (e.to_string(), true),
        };
        let now = chess_net::time::now_ms();
        state.update_settings(|s| {
            if s.synced(&path).is_some() {
                let sync = s.synced_mut(&path);
                sync.last_update = Some(now);
                sync.last_result = Some(message.clone());
                sync.last_failed = failed;
            }
        });
        result?;
        if failed { Err(JobError::failed(message)) } else { Ok(message) }
    }))
}
