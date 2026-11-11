//! The TWIC updater (NET-06): The Week in Chess's new issues into a
//! database the admin owns, on a schedule (weekly by default) or when they
//! ask. The issues taken are kept in the store, so none is imported twice;
//! the first check takes the newest issue only (no backfill of history).
//!
//! A check is a job (`twic`): it finds the newest issue (`chess_net::twic`),
//! and each new one is downloaded, noted as taken, written to `tmp/` and
//! imported by an ordinary import job, which goes on after a restart like
//! any other. The job waits for each import before the next issue. An
//! issue whose import couldn't start, or failed before any of its games
//! went in, is given back, to be taken at the next check; one that brought
//! some of its games stays taken (taking it again would bring those twice).
//!
//! Only a check that worked counts for the schedule (`LAST_CHECK`); one
//! that failed (TWIC unreachable, an import that failed) is tried again an
//! hour after it started (`LAST_TRY`), not a week later.

use std::sync::Arc;
use std::time::Duration;

use chess_jobs::{JobContext, JobError, JobId};
use chess_net::twic::Twic;

use crate::AppState;
use crate::access::require;
use crate::audit::Target;
use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::pool::writer_busy;
use crate::routes::imports::{TempFile, start as start_import};
use crate::store::{DbMeta, User, now};
use crate::wire::{Access, ImportKind, TwicDto, TwicIssueDto};

const DATABASE: &str = "twic.database";
const USER: &str = "twic.user";
const EVERY: &str = "twic.every_days";
/// When the last check that worked started.
const LAST_CHECK: &str = "twic.last_check";
/// When the last check started, whether it worked or not.
const LAST_TRY: &str = "twic.last_try";
/// Seconds before a scheduled check that failed is tried again.
const RETRY_SECS: i64 = 3600;
/// Issues taken in one check at most (a server off for months catches up
/// over a few checks).
const MAX_PER_CHECK: u32 = 10;
/// Days between checks by default.
pub const DEFAULT_EVERY_DAYS: u32 = 7;

/// The settings and the issues taken, for the admin.
pub fn view(state: &AppState) -> ApiResult<TwicDto> {
    let database = state.store.setting(DATABASE)?;
    let database_name = match &database {
        Some(id) => state.store.database(id)?.map(|m| m.name),
        None => None,
    };
    let every_days = state.store.setting(EVERY)?.and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_EVERY_DAYS);
    let last_check = state.store.setting(LAST_CHECK)?.and_then(|v| v.parse().ok());
    let issues = state
        .store
        .twic_issues(20)?
        .into_iter()
        .map(|(issue, _, at, games)| TwicIssueDto {
            issue,
            imported_at: at,
            games: (games >= 0).then_some(games as u64),
        })
        .collect();
    Ok(TwicDto {
        database,
        database_name,
        every_days,
        last_check,
        last_issue: state.store.twic_last()?,
        issues,
    })
}

/// Sets the target database (one `admin` owns; none turns updates off) and
/// the days between checks (0: only when asked).
pub fn configure(
    state: &AppState,
    admin: &Auth,
    database: Option<String>,
    every_days: Option<u32>,
) -> ApiResult<()> {
    if let Some(id) = &database {
        // Theirs only: another user's database is "not found" or "forbidden", as anywhere.
        require(&state.store, &admin.user, id, Access::Owner)?;
    }
    let every = every_days.unwrap_or(DEFAULT_EVERY_DAYS).min(365);
    state.store.set_setting(DATABASE, database.as_deref())?;
    state.store.set_setting(USER, database.as_ref().map(|_| admin.user.id.to_string()).as_deref())?;
    state.store.set_setting(EVERY, Some(&every.to_string()))?;
    let detail = match &database {
        Some(_) => format!("every {every} days"),
        None => "off".to_owned(),
    };
    let meta = match &database {
        Some(id) => state.store.database(id)?,
        None => None,
    };
    let target = meta.as_ref().map_or(Target::None, Target::Database);
    state.audit("twic.settings", Some(&admin.user), target, Some(detail), Some(&admin.client));
    Ok(())
}

/// Whether a scheduled check is due: the days passed since the last one
/// that worked, and an hour since the last try.
pub fn due(state: &AppState) -> bool {
    let set = state.store.setting(DATABASE).ok().flatten().is_some();
    let every: u32 =
        state.store.setting(EVERY).ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_EVERY_DAYS);
    let at = |key: &str| -> i64 {
        state.store.setting(key).ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(0)
    };
    let (last, tried) = (at(LAST_CHECK), at(LAST_TRY));
    set && every > 0 && now() - last >= i64::from(every) * 24 * 3600 && now() - tried >= RETRY_SECS
}

/// Starts a check, unless one runs. `by`: who asked (none: the schedule).
pub fn start(state: &Arc<AppState>, by: Option<&Auth>) -> ApiResult<u64> {
    let db = state
        .store
        .setting(DATABASE)?
        .ok_or_else(|| ApiError::conflict("Choose the database TWIC goes into first"))?;
    let meta = state
        .store
        .database(&db)?
        .ok_or_else(|| ApiError::conflict("TWIC's database is gone: choose another"))?;
    if state.jobs.manager.list().iter().any(|s| s.kind == "twic" && !s.state.is_finished()) {
        return Err(ApiError::conflict("A TWIC check runs already"));
    }
    let owner = state
        .store
        .setting(USER)?
        .and_then(|u| u.parse().ok())
        .map(|id| state.store.user(id))
        .transpose()?
        .flatten()
        .filter(|u| !u.disabled && u.id == meta.owner_id)
        .ok_or_else(|| ApiError::conflict("TWIC's database isn't its admin's any more: choose another"))?;
    let started = now();
    state.store.set_setting(LAST_TRY, Some(&started.to_string()))?;
    let st = state.clone();
    let runner = owner.clone();
    let job = state.jobs.spawn(
        "twic",
        "Checking The Week in Chess".into(),
        (&meta.id, &meta.name),
        &owner,
        move |ctx| {
            let message = run(&st, &runner, ctx)?;
            st.store
                .set_setting(LAST_CHECK, Some(&started.to_string()))
                .map_err(|e| JobError::failed(e.message))?;
            Ok(message)
        },
    );
    let (actor, client) = match by {
        Some(a) => (Some(&a.user), Some(a.client.as_str())),
        None => (None, None),
    };
    state.audit("twic.check", actor, Target::Database(&meta), None, client);
    Ok(job)
}

fn run(state: &Arc<AppState>, user: &User, ctx: &JobContext) -> Result<String, JobError> {
    let failed = |e: &dyn std::fmt::Display| JobError::failed(e);
    let db = state
        .store
        .setting(DATABASE)
        .map_err(|e| failed(&e.message))?
        .ok_or_else(|| failed(&"no database set"))?;
    let twic = Twic::with_base(&state.config.twic_url);
    let latest = twic.latest_issue().map_err(|e| failed(&e))?;
    let issues: Vec<u32> = match state.store.twic_last().map_err(|e| failed(&e.message))? {
        None => vec![latest],
        Some(last) if last >= latest => Vec::new(),
        Some(last) => (last + 1..=latest).take(MAX_PER_CHECK as usize).collect(),
    };
    if issues.is_empty() {
        return Ok(format!("No new issue (the newest is {latest})"));
    }
    let auth = Auth { user: user.clone(), session: None, client: String::new() };
    let mut games = 0u64;
    let mut taken = Vec::new();
    for (i, &n) in issues.iter().enumerate() {
        ctx.check_cancelled()?;
        ctx.progress_msg(i as u64, issues.len() as u64, format!("Issue {n}"));
        let pgn = match twic.issue_pgn(n) {
            Ok(pgn) => pgn,
            // A number skipped by the site: on to the next.
            Err(chess_net::twic::TwicError::NoSuchIssue(_)) => continue,
            Err(e) => return Err(failed(&e)),
        };
        let meta = state
            .store
            .database(&db)
            .map_err(|e| failed(&e.message))?
            .ok_or_else(|| failed(&"its database is gone"))?;
        let Some(job) = start_issue(state, &auth, &meta, n, &pgn, ctx)? else { continue };
        let done = state.jobs.manager.wait(job);
        let finished = done.is_some_and(|s| s.state == chess_jobs::JobState::Done);
        let imported = state.jobs.report(job).map(|r| r.imported);
        if !finished && imported == Some(0) {
            // None of its games went in: the next check takes it again.
            if let Err(e) = state.store.twic_untake(n) {
                log::warn!("TWIC issue {n} stays taken: {}", e.message);
            }
            return Err(failed(&format!(
                "the import of issue {n} failed (job {job}); the next check tries again"
            )));
        }
        // Without a report it's shown as unfinished.
        if let Some(imported) = imported {
            let _ = state.store.twic_games(n, imported);
            state.audit(
                "twic.import",
                None,
                Target::Database(&meta),
                Some(format!("issue {n}, {imported} games")),
                None,
            );
            games += imported;
        }
        taken.push(n);
        if !finished {
            return Err(failed(&format!(
                "the import of issue {n} didn't finish (job {job}): the games it brought stay, and it isn't \
                 taken again"
            )));
        }
    }
    Ok(match taken.as_slice() {
        [] => "No new issue".to_owned(),
        [n] => format!("Imported issue {n}: {games} games"),
        _ => format!("Imported issues {} to {}: {games} games", taken[0], taken[taken.len() - 1]),
    })
}

/// Notes issue `n` as taken and starts its import, waiting while another
/// write has the database. `None` if it was taken already. An import that
/// can't start gives the issue back; so does stopping the check while it
/// waits (nothing is taken then).
fn start_issue(
    state: &Arc<AppState>,
    auth: &Auth,
    meta: &DbMeta,
    n: u32,
    pgn: &[u8],
    ctx: &JobContext,
) -> Result<Option<JobId>, JobError> {
    loop {
        let busy = state.pool.get_open(&meta.id).is_some_and(|open| {
            open.is_compacting()
                || writer_busy(&open.writer.lock().expect("writer lock"), &state.jobs.manager)
        });
        if !busy {
            // Noted first: an issue is never taken twice, even after a crash.
            if !state.store.twic_take(n, &meta.id).map_err(|e| JobError::failed(e.message))? {
                return Ok(None);
            }
            match import_issue(state, auth, meta, n, pgn) {
                Ok(job) => return Ok(Some(job)),
                Err(e) => {
                    if let Err(u) = state.store.twic_untake(n) {
                        log::warn!("TWIC issue {n} stays taken: {}", u.message);
                    }
                    // Another write came first: wait for it, as for any.
                    if !matches!(e.status.as_u16(), 409 | 503) {
                        return Err(JobError::failed(format!("issue {n}: {}", e.message)));
                    }
                }
            }
        }
        ctx.check_cancelled()?;
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// Writes issue `n` to `tmp/` and starts importing it as an upload (the
/// file goes once the import ends, or at once if it can't start).
fn import_issue(state: &Arc<AppState>, auth: &Auth, meta: &DbMeta, n: u32, pgn: &[u8]) -> ApiResult<JobId> {
    let path = state.config.tmp_dir().join(format!("twic-{n}-{}.pgn", crate::secret::new_token()));
    let temp = TempFile::upload(state, path.clone());
    std::fs::write(&path, pgn)?;
    start_import(state, auth, meta.clone(), ImportKind::Pgn, path, format!("TWIC {n}"), temp)
}
