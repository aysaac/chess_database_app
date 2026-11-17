//! The Sync button (NET-13): a database kept in step with a Lichess study.
//! Its links, a preview of what a sync would do (a job: it asks Lichess),
//! the sync itself on the one confirm (a job, one at a time with the other
//! downloads), and the games it left for the user. The work is
//! `chess_net::study::sync`'s; here are the login, the games open in tabs,
//! and the words.

use std::collections::HashMap;

use chess_db::GameUid;
use chess_jobs::JobError;
use chess_net::study::sync::{self, Choice, Options, Outcome, Plan, Step, Target};
use chess_net::study::{StudyRef, check_study_name, parse_study_link};
use chess_net::{NetError, Token};
use tauri::{AppHandle, Manager, State};

use crate::commands::db::{thousands, to_job_error};
use crate::commands::lichess::checked;
use crate::commands::online::claim_download;
use crate::dto::study_sync::{
    CopiedStudyDto, LinkedStudyDto, PendingGameDto, StudySyncStatusDto, SyncChoiceDto, SyncConflictDto,
    SyncCountDto, SyncItemDto, SyncModeDto, SyncPreviewDto, SyncQuestionDto, SyncReportDto, SyncSideDto,
    SyncStepDto, SyncTargetDto,
};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::lichess;
use crate::state::AppState;

/// The most items a preview lists (a study has 64 chapters; a database
/// following one may have many other games).
const MOST_ITEMS: usize = 300;

/// Runs blocking work off the async runtime.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))?
}

fn job_error(e: NetError) -> JobError {
    match e {
        NetError::Cancelled => JobError::Cancelled,
        NetError::Db(e) => to_job_error(e),
        e => JobError::failed(e),
    }
}

fn target_of(t: SyncTargetDto) -> AppResult<Target> {
    Ok(match t {
        SyncTargetDto::Study { study, mode } => {
            Target::Study { study: parse_study_link(&study)?.study, mode: mode.into() }
        }
        SyncTargetDto::New { name, visibility, mode } => Target::New {
            name: check_study_name(&name)?.to_owned(),
            visibility: visibility.into(),
            mode: mode.into(),
        },
    })
}

fn study_url(state: &AppState, study: &str) -> String {
    StudyRef { study: study.to_owned(), chapter: None }.url(state.lichess.client())
}

/// The open database's links to studies, and the study a static copy came
/// from when it has none.
#[tauri::command]
pub async fn study_sync_status(app: AppHandle, handle: u32) -> AppResult<StudySyncStatusDto> {
    blocking(move || {
        let state = app.state::<AppState>();
        status(&state, handle)
    })
    .await
}

fn status(state: &AppState, handle: u32) -> AppResult<StudySyncStatusDto> {
    let db = state.db(handle)?;
    let mut db = db.lock().expect("db lock");
    let studies = sync::linked_studies(&mut db)?;
    let copied_from = if studies.is_empty() {
        sync::copied_from(&db)?.map(|(study, name)| CopiedStudyDto {
            link: StudyRef { study: study.clone(), chapter: None }.link(),
            study,
            name,
        })
    } else {
        None
    };
    let studies = studies
        .into_iter()
        .map(|s| LinkedStudyDto {
            url: study_url(state, &s.study),
            study: s.study,
            name: s.name,
            mode: s.mode.into(),
            games: s.games as u32,
            pending: s
                .pending
                .into_iter()
                .map(|(uid, game, name, kind)| PendingGameDto {
                    uid: uid.to_string(),
                    game,
                    name,
                    kind: kind.into(),
                })
                .collect(),
        })
        .collect();
    Ok(StudySyncStatusDto { studies, copied_from })
}

/// The login, or none; a keyring that can't be read fails the job.
fn login_of(state: &AppState) -> Result<Option<chess_net::Login>, JobError> {
    lichess::login(state).map_err(JobError::failed)
}

/// Looks at the study and the database and works out what a sync with
/// `target` would do, as a job; `study_sync_preview` returns it afterwards.
/// Changes nothing, here or on Lichess.
#[tauri::command]
pub async fn study_sync_plan(
    app: AppHandle,
    state: State<'_, AppState>,
    handle: u32,
    target: SyncTargetDto,
) -> AppResult<u64> {
    let target = target_of(target)?;
    if target.mode() == chess_db::SyncMode::TwoWay && lichess::account(&state).username.is_none() {
        return Err(NetError::SyncNeedsLogin.into());
    }
    let db = state.db(handle)?;
    let label = "Looking at the study on Lichess".to_owned();
    let slot = claim_download(&app, &label)?;
    state.lichess.sync_plans.lock().expect("lichess lock").remove(&handle);
    Ok(state.jobs.spawn("study-sync-plan", label, move |ctx| {
        let _slot = slot;
        let state = app.state::<AppState>();
        let login = login_of(&state)?;
        let token = login.as_ref().map(|l| &l.token);
        let open = || state.open_games(&db);
        let plan = sync::plan(state.lichess.client(), token, &db, &target, &open, ctx);
        let plan = checked(&state, login.as_ref(), plan).map_err(job_error)?;
        state.lichess.sync_plans.lock().expect("lichess lock").insert(handle, plan);
        // The dialog shows the preview: no notice.
        Ok(String::new())
    }))
}

fn counts(steps: impl Iterator<Item = Step>) -> Vec<SyncCountDto> {
    let mut by: HashMap<Step, u32> = HashMap::new();
    let mut order: Vec<Step> = Vec::new();
    for s in steps {
        if !by.contains_key(&s) {
            order.push(s);
        }
        *by.entry(s).or_default() += 1;
    }
    order.into_iter().map(|s| SyncCountDto { step: s.into(), count: by[&s] }).collect()
}

fn preview_of(state: &AppState, plan: &Plan) -> SyncPreviewDto {
    let changes = |s: Step| !matches!(s, Step::Unchanged);
    let items = plan
        .items
        .iter()
        .filter(|i| changes(i.step) || i.note.is_some())
        .take(MOST_ITEMS)
        .map(|i| SyncItemDto { name: i.name.clone(), step: i.step.into(), note: i.note.clone() })
        .collect();
    // Nothing a confirm would do: what's left waits for the user, a tab,
    // room in the study, or stays here.
    let idle = |s: Step| {
        matches!(
            s,
            Step::Unchanged
                | Step::NotSent
                | Step::DeletedHere
                | Step::Pending
                | Step::KeptHere
                | Step::Open
                | Step::NoRoom
        )
    };
    let nothing = plan.items.iter().all(|i| idle(i.step));
    SyncPreviewDto {
        study: plan.study.clone(),
        name: plan.name.clone(),
        url: plan.study.as_deref().map(|s| study_url(state, s)),
        mode: plan.mode().into(),
        counts: counts(plan.items.iter().map(|i| i.step)),
        items,
        room: plan.room as u32,
        unlisted: plan.unlisted as u32,
        nothing,
    }
}

/// What the last `study_sync_plan` of the database found, once its job
/// ended (none while it runs, or if it failed).
#[tauri::command]
pub async fn study_sync_preview(
    state: State<'_, AppState>,
    handle: u32,
) -> AppResult<Option<SyncPreviewDto>> {
    let plans = state.lichess.sync_plans.lock().expect("lichess lock");
    Ok(plans.get(&handle).map(|p| preview_of(&state, p)))
}

fn games(n: usize) -> String {
    if n == 1 { "1 game".to_owned() } else { format!("{} games", thousands(n as u64)) }
}

fn chapters(n: usize) -> String {
    if n == 1 { "1 chapter".to_owned() } else { format!("{} chapters", thousands(n as u64)) }
}

/// What a sync did, for the notice: "Synced with Openings: 2 games sent,
/// 1 brought in; 1 game waits for you (Sync)".
fn describe(o: &Outcome) -> String {
    let n = |s: Step| o.count(s);
    let mut parts = Vec::new();
    let sent = n(Step::NewHere);
    if sent > 0 {
        parts.push(format!("{} sent as new {}", games(sent), if sent == 1 { "chapter" } else { "chapters" }));
    }
    let pushed = n(Step::Push);
    if pushed > 0 {
        parts.push(format!("{} changed here sent", games(pushed)));
    }
    let pulled = n(Step::Pull);
    if pulled > 0 {
        parts.push(format!("{} changed on Lichess brought in", games(pulled)));
    }
    if n(Step::Merge) > 0 {
        parts.push(format!("{} merged", games(n(Step::Merge))));
    }
    if n(Step::NewThere) > 0 {
        parts.push(format!("{} new on Lichess brought in", chapters(n(Step::NewThere))));
    }
    if n(Step::Adopt) > 0 {
        parts.push(format!("{} linked to their chapters", games(n(Step::Adopt))));
    }
    if n(Step::RemovedThere) > 0 {
        parts.push(format!("{} removed on Lichess marked as deleted here", games(n(Step::RemovedThere))));
    }
    if n(Step::DeleteChapter) > 0 {
        parts.push(format!("{} removed on Lichess", chapters(n(Step::DeleteChapter))));
    }
    let what = if o.made { format!("{}, a new study on Lichess", o.name) } else { o.name.clone() };
    let waiting = n(Step::Conflict);
    let mut msg = match (parts.is_empty(), waiting) {
        (true, 0) => format!("{what} and this database were in step already"),
        (true, _) => format!("Synced with {what}"),
        (false, _) => format!("Synced with {what}: {}", parts.join(", ")),
    };
    if waiting > 0 {
        let verb = if waiting == 1 { "waits" } else { "wait" };
        msg.push_str(&format!(
            ". {} {verb} for you: both sides changed (Sync shows {})",
            games(waiting),
            if waiting == 1 { "it" } else { "them" }
        ));
    }
    if !o.skipped.is_empty() {
        msg.push_str(&format!(". {} left as they were (the dialog says why)", games(o.skipped.len())));
    }
    if let Some(e) = &o.stopped {
        msg = match (parts.is_empty() && waiting == 0, o.made) {
            // Nothing done but the study: say where it is.
            (true, true) => format!(
                "The sync stopped: {e}. The study {} was made on Lichess: link to it with One of your studies",
                o.name
            ),
            (true, false) => format!("The sync stopped: {e}. Nothing changed; Sync again"),
            (false, _) => format!("The sync stopped: {e}. Before that: {msg}"),
        };
    }
    msg
}

fn report_of(state: &AppState, o: &Outcome) -> SyncReportDto {
    let mut steps = Vec::new();
    for (s, n) in &o.done {
        steps.extend(std::iter::repeat_n(*s, *n));
    }
    steps.sort_by_key(|s| format!("{s:?}"));
    SyncReportDto {
        study: o.study.clone(),
        name: o.name.clone(),
        url: study_url(state, &o.study),
        made: o.made,
        counts: counts(steps.into_iter()),
        skipped: o
            .skipped
            .iter()
            .map(|(name, why)| SyncItemDto {
                name: name.clone(),
                step: SyncStepDto::Unchanged,
                note: Some(why.clone()),
            })
            .collect(),
        stopped: o.stopped.as_ref().map(|e| e.to_string()),
        cancelled: o.cancelled,
    }
}

/// Does what the database's last preview showed, as Lichess and the
/// database are now, as a job; `study_sync_report` says afterwards what it
/// did. `delete_chapters`: chapters of games deleted here go on Lichess too.
#[tauri::command]
pub async fn study_sync_apply(
    app: AppHandle,
    state: State<'_, AppState>,
    handle: u32,
    delete_chapters: bool,
) -> AppResult<u64> {
    let plan = state
        .lichess
        .sync_plans
        .lock()
        .expect("lichess lock")
        .get(&handle)
        .cloned()
        .ok_or_else(|| AppError::invalid("Look at the study again first: the preview is gone"))?;
    let db = state.db(handle)?;
    let label = format!("Syncing with {}", plan.name);
    let slot = claim_download(&app, &label)?;
    state.lichess.sync_reports.lock().expect("lichess lock").remove(&handle);
    Ok(state.jobs.spawn("study-sync", label, move |ctx| {
        let _slot = slot;
        let state = app.state::<AppState>();
        let login = login_of(&state)?;
        let token: Option<&Token> = login.as_ref().map(|l| &l.token);
        let open = || state.open_games(&db);
        let options = Options { delete_chapters };
        let o = sync::apply(state.lichess.client(), token, &db, &plan, options, &open, ctx);
        let o = checked(&state, login.as_ref(), o).map_err(job_error)?;
        if let (Some(login), Some(NetError::TokenRejected)) = (&login, &o.stopped) {
            lichess::forget_rejected(&state, &login.token);
        }
        state.lichess.sync_plans.lock().expect("lichess lock").remove(&handle);
        state.lichess.sync_reports.lock().expect("lichess lock").insert(handle, report_of(&state, &o));
        if o.cancelled {
            return Err(JobError::Cancelled);
        }
        let msg = describe(&o);
        if o.stopped.is_some() { Err(JobError::failed(msg)) } else { Ok(msg) }
    }))
}

/// What the database's last sync did, once its job ended.
#[tauri::command]
pub async fn study_sync_report(state: State<'_, AppState>, handle: u32) -> AppResult<Option<SyncReportDto>> {
    Ok(state.lichess.sync_reports.lock().expect("lichess lock").get(&handle).cloned())
}

/// How the database follows `study` from now on: every link's mode.
#[tauri::command]
pub async fn study_sync_set_mode(
    app: AppHandle,
    handle: u32,
    study: String,
    mode: SyncModeDto,
) -> AppResult<StudySyncStatusDto> {
    blocking(move || {
        let state = app.state::<AppState>();
        sync::set_mode(&mut state.db(handle)?.lock().expect("db lock"), &study, mode.into())?;
        status(&state, handle)
    })
    .await
}

/// Unlinks the database from `study`: its games stay as they are, a static
/// copy (Link to a study… links them again, by their chapters).
#[tauri::command]
pub async fn study_sync_unlink(app: AppHandle, handle: u32, study: String) -> AppResult<StudySyncStatusDto> {
    blocking(move || {
        let state = app.state::<AppState>();
        sync::unlink(&mut state.db(handle)?.lock().expect("db lock"), &study)?;
        status(&state, handle)
    })
    .await
}

fn uid_of(uid: &str) -> AppResult<GameUid> {
    uid.parse().map_err(|_| AppError::invalid(format!("\"{uid}\" isn't a game's id")))
}

/// A game waiting for the user, for the resolve dialog: what's in conflict,
/// and each side in short.
#[tauri::command]
pub async fn study_sync_conflict(
    app: AppHandle,
    handle: u32,
    uid: String,
) -> AppResult<Option<SyncConflictDto>> {
    blocking(move || {
        let state = app.state::<AppState>();
        let uid = uid_of(&uid)?;
        let db = state.db(handle)?;
        let mut db = db.lock().expect("db lock");
        let Some(w) = sync::waiting(&mut db, uid)? else { return Ok(None) };
        let game = db.game_by_uid(uid)?.unwrap_or_default();
        let side = |s: &sync::Side| SyncSideDto {
            line: s.line.clone(),
            moves: s.moves as u32,
            comments: s.comments as u32,
        };
        let chapter = StudyRef { study: w.study.clone(), chapter: Some(w.chapter.clone()) };
        Ok(Some(SyncConflictDto {
            uid: uid.to_string(),
            game,
            name: w.name.clone(),
            study: w.study.clone(),
            url: chapter.url(state.lichess.client()),
            kind: w.pending.kind.into(),
            two_way: w.mode == chess_db::SyncMode::TwoWay,
            found: chess_net::time::ymd(w.pending.found),
            questions: w
                .questions
                .iter()
                .map(|q| SyncQuestionDto {
                    what: q.describe(),
                    mine: q.mine.clone(),
                    theirs: q.theirs.clone(),
                })
                .collect(),
            mine: side(&w.mine),
            theirs: w.theirs.as_ref().map(side),
        }))
    })
    .await
}

/// Settles a game waiting for the user, as a job (keeping this database's
/// version sends it to Lichess). Refused for a game open in a tab.
#[tauri::command]
pub async fn study_sync_resolve(
    app: AppHandle,
    state: State<'_, AppState>,
    handle: u32,
    uid: String,
    choice: SyncChoiceDto,
) -> AppResult<u64> {
    let uid = uid_of(&uid)?;
    let db = state.db(handle)?;
    let choice = match choice {
        SyncChoiceDto::Mine => Choice::Mine,
        SyncChoiceDto::Theirs => Choice::Theirs,
        SyncChoiceDto::Both => Choice::Both,
    };
    let label = "Settling a game with Lichess".to_owned();
    let slot = claim_download(&app, &label)?;
    Ok(state.jobs.spawn("study-sync", label, move |ctx| {
        let _slot = slot;
        let state = app.state::<AppState>();
        let login = login_of(&state)?;
        let token = login.as_ref().map(|l| &l.token);
        let open = || state.open_games(&db);
        let done = sync::resolve(state.lichess.client(), token, &db, uid, choice, &open, ctx);
        checked(&state, login.as_ref(), done).map_err(job_error)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_a_sync() {
        let mut o = Outcome { name: "Openings".into(), ..Outcome::default() };
        assert_eq!(describe(&o), "Openings and this database were in step already");
        o.done.insert(Step::Conflict, 2);
        assert_eq!(
            describe(&o),
            "Synced with Openings. 2 games wait for you: both sides changed (Sync shows them)"
        );
        o.done.clear();
        o.done.insert(Step::NewHere, 2);
        o.done.insert(Step::Pull, 1);
        o.done.insert(Step::Conflict, 1);
        assert_eq!(
            describe(&o),
            "Synced with Openings: 2 games sent as new chapters, 1 game changed on Lichess brought in. \
             1 game waits for you: both sides changed (Sync shows it)"
        );
        o.made = true;
        o.done.clear();
        o.done.insert(Step::NewHere, 1);
        o.stopped = Some(NetError::RateLimited(chess_net::Site::Lichess));
        let msg = describe(&o);
        assert!(msg.starts_with("The sync stopped: Lichess has had too many requests"), "{msg}");
        assert!(
            msg.ends_with(
                "Before that: Synced with Openings, a new study on Lichess: 1 game sent as new chapter"
            ),
            "{msg}"
        );
        // Stopped with only the study made: where it is.
        o.done.clear();
        let msg = describe(&o);
        assert!(
            msg.ends_with("The study Openings was made on Lichess: link to it with One of your studies"),
            "{msg}"
        );
        o.made = false;
        assert!(describe(&o).ends_with("Nothing changed; Sync again"));
    }
}
