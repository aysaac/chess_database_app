//! The TWIC updater (NET-06): The Week in Chess's new issues into a
//! database the admin owns, on a schedule (weekly by default) or when they
//! ask. The issues taken are kept in the store, so none is imported twice;
//! the first check takes the newest issue only (no backfill of history).
//!
//! A check is a job (`twic`): it finds the newest issue (`chess_net::twic`),
//! and each new one is noted as taken, downloaded into `tmp/` and imported
//! by an ordinary import job, which goes on after a restart like any other.
//! The job waits for each import before the next issue.

use std::sync::Arc;
use std::time::Duration;

use chess_jobs::{JobContext, JobError};
use chess_net::twic::Twic;

use crate::AppState;
use crate::access::require;
use crate::audit::Target;
use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::routes::imports::{TempFile, start as start_import};
use crate::store::{User, now};
use crate::wire::{Access, ImportKind, TwicDto, TwicIssueDto};

const DATABASE: &str = "twic.database";
const USER: &str = "twic.user";
const EVERY: &str = "twic.every_days";
const LAST_CHECK: &str = "twic.last_check";
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

/// Whether a scheduled check is due.
pub fn due(state: &AppState) -> bool {
    let set = state.store.setting(DATABASE).ok().flatten().is_some();
    let every: u32 =
        state.store.setting(EVERY).ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_EVERY_DAYS);
    let last: i64 = state.store.setting(LAST_CHECK).ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(0);
    set && every > 0 && now() - last >= i64::from(every) * 24 * 3600
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
    state.store.set_setting(LAST_CHECK, Some(&now().to_string()))?;
    let st = state.clone();
    let runner = owner.clone();
    let job = state.jobs.spawn(
        "twic",
        "Checking The Week in Chess".into(),
        (&meta.id, &meta.name),
        &owner,
        move |ctx| run(&st, &runner, ctx),
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
        // Noted first: an issue is never taken twice, even after a crash.
        if !state.store.twic_take(n, &db).map_err(|e| failed(&e.message))? {
            continue;
        }
        let path = state.config.tmp_dir().join(format!("twic-{n}-{}.pgn", crate::secret::new_token()));
        std::fs::write(&path, &pgn).map_err(|e| failed(&e))?;
        let temp = TempFile::upload(state, path.clone());
        let mut temp = Some(temp);
        // The database may be busy with another write: wait for it.
        let job = loop {
            match start_import(
                state,
                &auth,
                meta.clone(),
                ImportKind::Pgn,
                path.clone(),
                format!("TWIC {n}"),
                temp.take().expect("one try at a time"),
            ) {
                Ok(job) => break job,
                Err(e) if e.status.as_u16() == 409 || e.status.as_u16() == 503 => {
                    ctx.check_cancelled()?;
                    std::thread::sleep(Duration::from_secs(2));
                    // The refused import deleted the file with its TempFile.
                    std::fs::write(&path, &pgn).map_err(|e| failed(&e))?;
                    temp = Some(TempFile::upload(state, path.clone()));
                }
                Err(e) => return Err(failed(&e.message)),
            }
        };
        let done = state.jobs.manager.wait(job);
        let report = state.jobs.report(job);
        let imported = report.map_or(0, |r| r.imported);
        let _ = state.store.twic_games(n, imported);
        state.audit(
            "twic.import",
            None,
            Target::Database(&meta),
            Some(format!("issue {n}, {imported} games")),
            None,
        );
        games += imported;
        taken.push(n);
        if done.is_none_or(|s| s.state != chess_jobs::JobState::Done) {
            return Err(failed(&format!("the import of issue {n} didn't finish (job {job})")));
        }
    }
    Ok(match taken.as_slice() {
        [] => "No new issue".to_owned(),
        [n] => format!("Imported issue {n}: {games} games"),
        _ => format!("Imported issues {} to {}: {games} games", taken[0], taken[taken.len() - 1]),
    })
}
