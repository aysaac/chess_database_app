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
use crate::dto::sync::{DbSyncDto, LimitPolicyDto, ServerAccountDto, ServerDbDto, SyncSourceDto};
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
        SyncSourceDto::Player {
            site,
            username,
            speeds,
            rated_only,
            color,
            since,
            until,
            latest,
            reached,
        } => {
            let username = chess_net::check_username(&username)?.to_owned();
            if let (Some(from), Some(to)) = (since, until)
                && from > to
            {
                return Err(AppError::invalid("The first day is after the last day"));
            }
            if latest == Some(0) {
                return Err(AppError::invalid("The latest games is at least 1 game"));
            }
            SyncSourceDto::Player { site, username, speeds, rated_only, color, since, until, latest, reached }
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
    limit: Option<u32>,
    limit_policy: LimitPolicyDto,
) -> AppResult<Option<DbSyncDto>> {
    if limit == Some(0) {
        return Err(AppError::invalid("A limit is at least 1 game"));
    }
    let path = state.list_path(handle).ok_or_else(|| AppError::not_found(format!("database {handle}")))?;
    let sources = sources.into_iter().map(normalized).collect::<AppResult<Vec<_>>>()?;
    Ok(state.update_settings(|s| {
        s.set_sources(&path, sources, auto, limit, limit_policy);
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
    max_games: Option<u64>,
    newest: Option<u64>,
    ctx: &JobContext,
) -> Result<(SyncSourceDto, u64), NetError> {
    let SyncSourceDto::Player { site, username, speeds, rated_only, color, since, until, latest, reached } =
        source
    else {
        unreachable!("a player's source")
    };
    // From where the last update got to, or the first day asked for.
    let from = match (reached, since) {
        (Some(a), Some(b)) => Some(*a.max(b)),
        (a, b) => a.or(*b),
    };
    // A first update that only wants the newest games asks for them first
    // (a download of the whole account would be wasted); later ones go on
    // from where it got to.
    let cap = match (reached, latest.map(u64::from), newest) {
        (None, Some(a), Some(b)) => Some(a.min(b)),
        (None, a, b) => a.or(b),
        _ => None,
    };
    let latest_first = cap.is_some();
    let max_games = match (max_games, cap) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let req = Request {
        site: (*site).into(),
        username: username.clone(),
        speeds: crate::dto::speeds_of(speeds),
        rated_only: *rated_only,
        period: from.map_or(Period::All, Period::Since),
        until: *until,
        color: color.map(Into::into),
        max_games,
        latest_first,
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
    sync: &DbSyncDto,
    ctx: &JobContext,
) -> Result<Vec<Outcome>, JobError> {
    let mut out = Vec::new();
    // The limit counts the live games of every source together, and is for
    // players' games.
    let limit = sync.limit.filter(|_| sync.sources.iter().any(|s| matches!(s, SyncSourceDto::Player { .. })));
    for source in &sync.sources {
        ctx.check_cancelled()?;
        let what = source.describe();
        let mut room = None;
        // Over a limit that keeps the latest, no source needs more than that many.
        let newest = limit.filter(|_| sync.limit_policy == LimitPolicyDto::KeepLatest).map(u64::from);
        if let (Some(limit), LimitPolicyDto::Stop, SyncSourceDto::Player { .. }) =
            (limit, sync.limit_policy, source)
        {
            let live = live_count(db).map_err(to_job_error)?;
            if live >= limit {
                out.push(Outcome {
                    text: format!(
                        "{what} not updated: the database has its {} games",
                        thousands(limit.into())
                    ),
                    failed: false,
                });
                continue;
            }
            room = Some(u64::from(limit - live));
        }
        let result = match source {
            SyncSourceDto::Player { .. } => update_player(state, db, path, source, room, newest, ctx),
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
    if let Some(limit) = limit {
        let thousands = |n: u32| thousands(n.into());
        match sync.limit_policy {
            LimitPolicyDto::KeepLatest => {
                let trimmed = trim_oldest(db, limit, ctx).map_err(to_job_error)?;
                if trimmed > 0 {
                    out.push(Outcome {
                        text: format!(
                            "the {} oldest games over the limit of {} were marked as deleted (Compact the database to free the space)",
                            thousands(trimmed),
                            thousands(limit)
                        ),
                        failed: false,
                    });
                }
            }
            LimitPolicyDto::Stop if live_count(db).map_err(to_job_error)? >= limit => out.push(Outcome {
                text: format!(
                    "the database has its limit of {} games: it stops updating until the limit is raised or games are removed",
                    thousands(limit)
                ),
                failed: false,
            }),
            LimitPolicyDto::Stop => {}
        }
    }
    Ok(out)
}

/// The games in the database, not counting those marked as deleted.
fn live_count(db: &Mutex<Database>) -> Result<u32, chess_db::DbError> {
    let mut db = db.lock().expect("db lock");
    Ok(db.game_stats()?.iter().filter(|g| !g.deleted).count() as u32)
}

/// Marks the oldest games (by date, then as they came) over `limit` as
/// deleted; how many.
fn trim_oldest(
    db: &Mutex<Database>,
    limit: u32,
    progress: &dyn chess_db::Progress,
) -> Result<u32, chess_db::DbError> {
    use chess_db::{SortKey, SortSpec, ensure_sort};
    let order = ensure_sort(db, SortSpec::new(SortKey::Date, false), progress)?;
    let mut db = db.lock().expect("db lock");
    let deleted: Vec<bool> = db.game_stats()?.iter().map(|g| g.deleted).collect();
    let live: Vec<chess_db::GameId> =
        order.iter().copied().filter(|&id| !deleted.get(id as usize).copied().unwrap_or(false)).collect();
    let over = live.len().saturating_sub(limit as usize);
    if over > 0 {
        db.set_deleted_many(&live[..over], true)?;
    }
    Ok(over as u32)
}

/// Updates the database from its sources as a job, one download at a time.
/// Returns the job; its message says what came. A database with no sources
/// is an error.
#[tauri::command]
pub async fn sync_database(app: AppHandle, state: State<'_, AppState>, handle: u32) -> AppResult<u64> {
    let db = state.db(handle)?;
    let path = state.list_path(handle).ok_or_else(|| AppError::not_found(format!("database {handle}")))?;
    let sync = state
        .settings
        .lock()
        .expect("settings lock")
        .synced(&path)
        .cloned()
        .filter(|d| !d.sources.is_empty())
        .ok_or_else(|| AppError::invalid("This database has no sources to update from"))?;
    let label = "Updating the database".to_owned();
    let slot = claim_download(&app, &label)?;
    state.import_reports.lock().expect("import reports lock").remove(&handle);
    Ok(state.jobs.spawn("sync", label, move |ctx| {
        let _slot = slot;
        let state = app.state::<AppState>();
        let result = run_update(&state, &db, &path, &sync, ctx);
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

#[cfg(test)]
mod tests {
    use super::*;
    use chess_db::{NoProgress, import_pgn};

    fn db_with(fixture: &str) -> (tempfile::TempDir, Mutex<Database>) {
        let dir = tempfile::tempdir().unwrap();
        let db = Mutex::new(Database::create(&dir.path().join("Sync")).unwrap());
        let file =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn").join(fixture);
        import_pgn(&db, &file, &NoProgress).unwrap();
        (dir, db)
    }

    /// The dates of the games not marked as deleted, in game order.
    fn live_dates(db: &Mutex<Database>) -> Vec<String> {
        let mut db = db.lock().unwrap();
        let gone: Vec<bool> = db.game_stats().unwrap().iter().map(|g| g.deleted).collect();
        (0..db.count()).filter(|&i| !gone[i as usize]).map(|i| db.row(i).unwrap().date).collect()
    }

    #[test]
    fn the_oldest_games_over_the_limit_are_marked_as_deleted() {
        let (_dir, db) = db_with("crosstable.pgn");
        let total = live_count(&db).unwrap();
        assert!(total > 4);
        let mut dates = live_dates(&db);
        dates.sort();
        // Nothing over the limit: nothing goes.
        assert_eq!(trim_oldest(&db, total, &NoProgress).unwrap(), 0);
        // A game already deleted doesn't count toward it.
        db.lock().unwrap().set_deleted_many(&[0], true).unwrap();
        assert_eq!(live_count(&db).unwrap(), total - 1);
        assert_eq!(trim_oldest(&db, total - 1, &NoProgress).unwrap(), 0);
        // Over it: the oldest by date go, and what's left is the newest.
        let left = trim_oldest(&db, 3, &NoProgress).unwrap();
        assert_eq!(left, total - 1 - 3);
        assert_eq!(live_count(&db).unwrap(), 3);
        let mut kept = live_dates(&db);
        kept.sort();
        assert_eq!(kept, dates[dates.len() - 3..], "the three newest dates");
        // Again: nothing more.
        assert_eq!(trim_oldest(&db, 3, &NoProgress).unwrap(), 0);
    }
}
