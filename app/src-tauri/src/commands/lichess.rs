//! Lichess: logging in (NET-11), a study's chapters into a database
//! (IO-10), games out to studies (NET-12). The token is used only where
//! Lichess needs it: a private or unlisted study, the user's own list of
//! studies, sending games. A login Lichess no longer takes (revoked on its
//! site, expired) is forgotten when it says so.

use chess_db::GameId;
use chess_jobs::JobError;
use chess_net::study::{self, ExportSummary, ExportTarget, StudyImport};
use chess_net::{Login, NetError, Token, oauth};
use tauri::{AppHandle, Manager, State};

use crate::commands::db::{selected_ids, thousands, to_job_error};
use crate::commands::online::claim_download;
use crate::dto::ImportReportDto;
use crate::dto::ListSelectionDto;
use crate::dto::lichess::{
    LichessAccountDto, LogoutDto, SentStudyDto, StudyExportReportDto, StudyInfoDto, StudyPreviewDto,
    StudyTargetDto,
};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::lichess::{self, CLIENT_ID, open_browser};
use crate::state::AppState;

/// Runs blocking network work off the async runtime.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))?
}

/// `result`, forgetting the login when Lichess says it no longer takes it.
fn checked<T>(state: &AppState, login: Option<&Login>, result: Result<T, NetError>) -> Result<T, NetError> {
    if login.is_some() && matches!(result, Err(NetError::TokenRejected)) {
        log::info!("Lichess no longer takes the login: forgetting it");
        lichess::forget(state);
    }
    result
}

/// Who is logged in (without reading the OS's credential store).
#[tauri::command]
pub async fn lichess_account(state: State<'_, AppState>) -> AppResult<LichessAccountDto> {
    Ok(lichess::account(&state))
}

/// Logs in through the browser (OAuth with PKCE): waits until the user lets
/// the app in on Lichess's page, cancels it there, `lichess_login_cancel`
/// is called, or 10 minutes pass.
#[tauri::command]
pub async fn lichess_login(app: AppHandle) -> AppResult<LichessAccountDto> {
    blocking(move || {
        let state = app.state::<AppState>();
        let attempt = state.lichess.start_login();
        let stop = || state.lichess.login_stopped(attempt);
        let login = oauth::login(state.lichess.client(), CLIENT_ID, &open_browser, &stop)?;
        lichess::remember(&state, login);
        Ok(lichess::account(&state))
    })
    .await
}

/// Stops a login waiting for the browser.
#[tauri::command]
pub async fn lichess_login_cancel(state: State<'_, AppState>) -> AppResult<()> {
    state.lichess.stop_logins();
    Ok(())
}

/// Logs in with a personal access token pasted in, once Lichess says it's
/// one that can read and write studies.
#[tauri::command]
pub async fn lichess_use_token(app: AppHandle, token: String) -> AppResult<LichessAccountDto> {
    blocking(move || {
        let state = app.state::<AppState>();
        let client = state.lichess.client().for_lookup();
        let login = oauth::check_token(&client, &Token::new(&token), &|| false)?;
        lichess::remember(&state, login);
        Ok(lichess::account(&state))
    })
    .await
}

/// Logs out: forgets the token (in the credential store too) and revokes
/// it on Lichess. When Lichess can't be reached the token is forgotten all
/// the same, and `problem` says how to revoke it by hand.
#[tauri::command]
pub async fn lichess_logout(app: AppHandle) -> AppResult<LogoutDto> {
    blocking(move || {
        let state = app.state::<AppState>();
        state.lichess.stop_logins();
        let Some(login) = lichess::forget(&state) else {
            return Ok(LogoutDto { revoked: false, problem: None });
        };
        Ok(match oauth::revoke(state.lichess.client(), &login.token) {
            Ok(()) => LogoutDto { revoked: true, problem: None },
            Err(e) => {
                log::warn!("couldn't revoke the Lichess token: {e}");
                let page = format!("{}/account/security", state.lichess.client().lichess_url());
                let problem = format!("Lichess couldn't revoke the token ({e}): revoke it on {page}");
                LogoutDto { revoked: false, problem: Some(problem) }
            }
        })
    })
    .await
}

/// Opens a Lichess page in the system browser (never in the app's window):
/// a study, or where to make a token (`url` empty).
#[tauri::command]
pub async fn lichess_open(state: State<'_, AppState>, url: String) -> AppResult<()> {
    let client = state.lichess.client();
    let url = if url.is_empty() { oauth::token_page(client) } else { url };
    if !url.starts_with(&format!("{}/", client.lichess_url())) {
        return Err(AppError::invalid("only Lichess pages open from here"));
    }
    open_browser(&url).map_err(|e| AppError::invalid(format!("the browser didn't open ({e})")))
}

/// A user's studies, newest first: with their own login, the unlisted and
/// private ones too.
#[tauri::command]
pub async fn lichess_studies(app: AppHandle, username: String) -> AppResult<Vec<StudyInfoDto>> {
    blocking(move || {
        let state = app.state::<AppState>();
        let client = state.lichess.client().for_lookup();
        let login = lichess::login_of(&state, &username);
        let studies = study::studies_of(&client, &username, login.as_ref().map(|l| &l.token), &|| false);
        let studies = checked(&state, login.as_ref(), studies)?;
        Ok(studies
            .into_iter()
            .map(|s| StudyInfoDto {
                link: format!("https://lichess.org/study/{}", s.id),
                updated: chess_net::time::ymd(s.updated_at),
                id: s.id,
                name: s.name,
            })
            .collect())
    })
    .await
}

/// Downloads a study (or one chapter) to show what's in it, and keeps it
/// for the import. The login goes with it only if the study needs it
/// (private or unlisted).
#[tauri::command]
pub async fn lichess_study_lookup(app: AppHandle, link: String) -> AppResult<StudyPreviewDto> {
    blocking(move || {
        let state = app.state::<AppState>();
        let link = study::parse_study_link(&link)?;
        let client = state.lichess.client().for_lookup();
        let unasked = || NetError::BadData { site: chess_net::Site::Lichess, reason: "a 304 unasked".into() };
        let (found, private) = match study::fetch_study(&client, &link, None, None, &|| false) {
            Err(NetError::StudyNeedsLogin) => {
                let Some(login) = lichess::login(&state) else {
                    return Err(NetError::StudyNeedsLogin.into());
                };
                let found = study::fetch_study(&client, &link, Some(&login.token), None, &|| false);
                (checked(&state, Some(&login), found)?, true)
            }
            other => (other?, false),
        };
        let found = found.ok_or_else(unasked)?;
        let preview = StudyPreviewDto {
            link: found.link.link(),
            url: found.link.url(&client),
            name: found.name.clone(),
            one_chapter: found.link.chapter.is_some(),
            chapters: found.chapters.iter().map(|c| c.name.clone()).collect(),
            private,
        };
        *state.lichess.looked_up.lock().expect("lichess lock") = Some((found, private));
        Ok(preview)
    })
    .await
}

fn chapters(n: u64) -> String {
    if n == 1 { "1 chapter".to_owned() } else { format!("{} chapters", thousands(n)) }
}

/// What a study's import did, for the notice: "Imported 4 chapters of
/// Openings (1 was in the database already)".
fn describe_import(s: &StudyImport) -> String {
    let name = &s.name;
    let mut msg = match (s.import.imported, s.already) {
        (0, 0) => format!("{name} has no chapters to import"),
        (0, _) => format!("No new chapters in {name}"),
        (n, 0) => format!("Imported {} of {name}", chapters(n)),
        (n, _) => {
            format!("Imported {} new {} of {name}", thousands(n), if n == 1 { "chapter" } else { "chapters" })
        }
    };
    let mut notes = Vec::new();
    if s.already > 0 {
        let were = if s.already == 1 { "was" } else { "were" };
        notes.push(format!("{} {were} in the database already", thousands(s.already)));
    }
    if s.import.skipped > 0 || s.import.warning_count > 0 {
        notes.push(format!(
            "{} skipped, {} warnings",
            thousands(s.import.skipped),
            thousands(s.import.warning_count)
        ));
    }
    if !notes.is_empty() {
        msg.push_str(&format!(" ({})", notes.join("; ")));
    }
    msg
}

/// Imports a study's chapters (or one chapter's) into the open database
/// `handle` as a job, leaving out those it has (by their `ChapterURL`).
/// A study just looked up is only asked about again (`If-Modified-Since`).
/// When it had warnings, `import_report` returns them afterwards.
#[tauri::command]
pub async fn import_study(
    app: AppHandle,
    state: State<'_, AppState>,
    handle: u32,
    link: String,
) -> AppResult<u64> {
    let link = study::parse_study_link(&link)?;
    let db = state.db(handle)?;
    let looked =
        state.lichess.looked_up.lock().expect("lichess lock").clone().filter(|(s, _)| s.link == link);
    // A study whose look-up took the login gets it at once.
    let (looked, private) = looked.map_or((None, false), |(s, private)| (Some(s), private));
    let name = looked.as_ref().map_or_else(|| "a study".to_owned(), |s| s.name.clone());
    let label = format!("Importing {name} from Lichess");
    let slot = claim_download(&app, &label)?;
    state.import_reports.lock().expect("import reports lock").remove(&handle);
    Ok(state.jobs.spawn("study", label, move |ctx| {
        let _slot = slot;
        let state = app.state::<AppState>();
        let client = state.lichess.client();
        let download = |token: Option<&Token>| {
            study::download_study_into(client, &db, &link, token, looked.as_ref(), ctx)
        };
        let with_login = || {
            let login = lichess::login(&state);
            let s = download(login.as_ref().map(|l| &l.token));
            checked(&state, login.as_ref(), s)
        };
        let s = match if private { with_login() } else { download(None) } {
            Err(NetError::StudyNeedsLogin) if !private && lichess::account(&state).username.is_some() => {
                with_login()
            }
            other => other,
        };
        let s = s.map_err(|e| match e {
            NetError::Cancelled => JobError::Cancelled,
            NetError::Db(e) => to_job_error(e),
            e => JobError::failed(e),
        })?;
        if s.import.warning_count > 0 {
            let report = ImportReportDto::new(s.name.clone(), &s.import);
            state.import_reports.lock().expect("import reports lock").insert(handle, report);
        }
        if s.cancelled {
            log::info!("import of {} cancelled after {} chapters", s.name, s.import.imported);
            return Err(JobError::Cancelled);
        }
        Ok(describe_import(&s))
    }))
}

/// The most games one export takes: a day's new studies of 64 chapters.
const MOST_GAMES: usize = study::DAILY_STUDIES * study::MAX_CHAPTERS;

/// The games of the list as it is: its filter, or all of them; deleted
/// ones left out.
fn listed_ids(state: &AppState, handle: u32) -> AppResult<Vec<GameId>> {
    let db = state.db(handle)?;
    let filtered = state.filters.lock().expect("filters lock").get(&handle).map(|f| f.ids.clone());
    let db = db.lock().expect("db lock");
    let ids: Vec<GameId> = match &filtered {
        Some(f) => f.to_vec(),
        None => (0..db.count()).collect(),
    };
    if ids.len() > 100_000 {
        // Far more than an export takes, deleted or not: it's refused.
        return Ok(ids);
    }
    let mut keep = Vec::with_capacity(ids.len());
    for id in ids {
        if !db.record(id)?.is_deleted() {
            keep.push(id);
        }
    }
    Ok(keep)
}

/// A game as it goes to a study: the app's own PGN, without the tags of a
/// study it came from (Lichess writes the new study's).
fn study_pgn(mut game: chess_core::game::Game) -> String {
    game.headers.extra.retain(|(k, _)| !matches!(k.as_str(), "StudyName" | "ChapterURL"));
    chess_core::pgn::write_game(&game)
}

fn games(n: usize) -> String {
    if n == 1 { "1 game".to_owned() } else { format!("{} games", thousands(n as u64)) }
}

/// "A, B and C".
fn listing(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What an export did, for the notice: "Sent 130 games to 3 new private
/// studies on Lichess: A (1/3), A (2/3) and A (3/3)"; when it stopped,
/// what went where and why the rest didn't.
fn describe_export(s: &ExportSummary, visibility: Option<&str>) -> String {
    let names: Vec<&str> = s.studies.iter().filter(|st| st.sent > 0).map(|st| st.name.as_str()).collect();
    let sent = s.sent();
    let mut msg = match (sent, s.studies.iter().any(|st| st.made), visibility) {
        (0, _, _) => "No games were sent".to_owned(),
        (n, true, Some(v)) if names.len() == 1 => {
            format!("Sent {} to {}, a new {v} study on Lichess", games(n), names[0])
        }
        (n, true, Some(v)) => {
            format!("Sent {} to {} new {v} studies on Lichess: {}", games(n), names.len(), listing(&names))
        }
        (n, _, _) => format!("Sent {} to {} on Lichess", games(n), listing(&names)),
    };
    let mut notes = Vec::new();
    for st in s.studies.iter().filter(|st| st.chapters < st.sent) {
        let why = st.refused.as_deref().map(|r| format!(": {r}")).unwrap_or_default();
        notes.push(format!("{} took {} of them as chapters{why}", st.name, st.chapters));
    }
    if s.too_long > 0 {
        let what = if s.too_long == 1 { "1 game is" } else { "some games are" };
        notes.push(format!("{} not sent: {what} too long for Lichess (over 400 kB)", games(s.too_long)));
    }
    if !notes.is_empty() {
        msg.push_str(&format!(" ({})", notes.join("; ")));
    }
    if let Some(e) = &s.stopped {
        let rest = match s.not_sent() - s.too_long {
            1 => "the other game wasn't sent".to_owned(),
            n => format!("the other {} weren't sent", thousands(n as u64)),
        };
        msg = format!("{}. {msg}; {rest}", chess_net_sentence(e));
    }
    msg
}

/// An error as the start of a message.
fn chess_net_sentence(e: &NetError) -> String {
    let t = e.to_string();
    let mut c = t.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

impl From<&ExportSummary> for StudyExportReportDto {
    fn from(s: &ExportSummary) -> StudyExportReportDto {
        StudyExportReportDto {
            games: s.games as u32,
            sent: s.sent() as u32,
            not_sent: s.not_sent() as u32,
            too_long: s.too_long as u32,
            studies: s
                .studies
                .iter()
                .map(|st| SentStudyDto {
                    name: st.name.clone(),
                    url: st.url.clone(),
                    made: st.made,
                    sent: st.sent as u32,
                    chapters: st.chapters as u32,
                    refused: st.refused.clone(),
                })
                .collect(),
            stopped: s.stopped.as_ref().map(|e| e.to_string()),
            cancelled: s.cancelled,
        }
    }
}

/// Sends the selected games (or the list's: its filter, or all of them) to
/// Lichess studies as a job, each game a chapter: to new studies of 64
/// chapters at most ("Name (1/3)"…), or to a study of the user's that has
/// room. The games in the database don't change. `study_export_report`
/// says afterwards what went where.
#[tauri::command]
pub async fn send_to_study(
    app: AppHandle,
    state: State<'_, AppState>,
    handle: u32,
    selection: Option<ListSelectionDto>,
    target: StudyTargetDto,
) -> AppResult<u64> {
    let (target, visibility) = match target {
        StudyTargetDto::New { name, visibility } => {
            let name = study::check_study_name(&name)?.to_owned();
            let word = match visibility {
                crate::dto::lichess::StudyVisibilityDto::Public => "public",
                crate::dto::lichess::StudyVisibilityDto::Unlisted => "unlisted",
                crate::dto::lichess::StudyVisibilityDto::Private => "private",
            };
            (ExportTarget::New { name, visibility: visibility.into() }, Some(word))
        }
        StudyTargetDto::Existing { study } => {
            (ExportTarget::Existing(study::parse_study_link(&study)?.study), None)
        }
    };
    let ids = match &selection {
        Some(sel) => selected_ids(&state, handle, sel)?,
        None => listed_ids(&state, handle)?,
    };
    let most = match target {
        ExportTarget::New { .. } => MOST_GAMES,
        ExportTarget::Existing(_) => study::MAX_CHAPTERS,
    };
    if ids.is_empty() {
        return Err(AppError::invalid("there are no games to send"));
    }
    if ids.len() > most {
        return Err(AppError::from(match target {
            ExportTarget::New { .. } => {
                NetError::TooManyStudies { games: ids.len(), studies: study::studies_needed(ids.len()) }
            }
            ExportTarget::Existing(_) => {
                NetError::StudyFull { name: "A study".into(), room: study::MAX_CHAPTERS }
            }
        }));
    }
    if lichess::account(&state).username.is_none() {
        return Err(AppError::new(
            ErrorCode::Login,
            "log in to Lichess first: the games go to studies of yours",
        ));
    }
    let db = state.db(handle)?;
    let label = format!("Sending {} to Lichess", games(ids.len()));
    let slot = claim_download(&app, &label)?;
    *state.lichess.export_report.lock().expect("lichess lock") = None;
    Ok(state.jobs.spawn("study", label, move |ctx| {
        let _slot = slot;
        let state = app.state::<AppState>();
        let login = lichess::login(&state).ok_or_else(|| JobError::failed("log in to Lichess first"))?;
        let mut pgns = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(256) {
            if ctx.is_cancelled() {
                return Err(JobError::Cancelled);
            }
            let db = db.lock().expect("db lock");
            for &id in chunk {
                if db.record(id).map_err(to_job_error)?.is_deleted() {
                    continue;
                }
                pgns.push(study_pgn(db.read_game(id).map_err(to_job_error)?));
            }
        }
        if pgns.is_empty() {
            return Err(JobError::failed("the selected games are marked as deleted: nothing to send"));
        }
        let client = state.lichess.client();
        let s = study::export_to_studies(client, &login.token, &target, &pgns, ctx);
        let s = checked(&state, Some(&login), s).map_err(|e| match e {
            NetError::Cancelled => JobError::Cancelled,
            NetError::Db(e) => to_job_error(e),
            e => JobError::failed(e),
        })?;
        *state.lichess.export_report.lock().expect("lichess lock") = Some(StudyExportReportDto::from(&s));
        if s.cancelled {
            log::info!("sending games to Lichess cancelled after {}", games(s.sent()));
            return Err(JobError::Cancelled);
        }
        let msg = describe_export(&s, visibility);
        if s.stopped.is_some() { Err(JobError::failed(msg)) } else { Ok(msg) }
    }))
}

/// What the last export to studies did, once its job ended.
#[tauri::command]
pub async fn study_export_report(state: State<'_, AppState>) -> AppResult<Option<StudyExportReportDto>> {
    Ok(state.lichess.export_report.lock().expect("lichess lock").clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_db::ImportSummary;
    use chess_net::study::{SentStudy, StudyRef};

    fn import(imported: u64, already: u64) -> StudyImport {
        StudyImport {
            link: StudyRef { study: "abcd1234".into(), chapter: None },
            name: "Openings".into(),
            chapters: imported + already,
            import: ImportSummary { imported, ..Default::default() },
            already,
            unchanged: false,
            cancelled: false,
        }
    }

    #[test]
    fn describes_the_import() {
        assert_eq!(describe_import(&import(4, 0)), "Imported 4 chapters of Openings");
        assert_eq!(describe_import(&import(1, 0)), "Imported 1 chapter of Openings");
        assert_eq!(
            describe_import(&import(1, 4)),
            "Imported 1 new chapter of Openings (4 were in the database already)"
        );
        assert_eq!(
            describe_import(&import(0, 1)),
            "No new chapters in Openings (1 was in the database already)"
        );
        assert_eq!(describe_import(&import(0, 0)), "Openings has no chapters to import");
    }

    fn sent(name: &str, n: usize, made: bool) -> SentStudy {
        SentStudy {
            id: "x".into(),
            name: name.into(),
            url: "u".into(),
            made,
            sent: n,
            chapters: n,
            refused: None,
        }
    }

    fn summary(studies: Vec<SentStudy>, games: usize) -> ExportSummary {
        ExportSummary { studies, games, too_long: 0, stopped: None, cancelled: false }
    }

    #[test]
    fn describes_the_export() {
        let s = summary(vec![sent("A (1/2)", 64, true), sent("A (2/2)", 2, true)], 66);
        assert_eq!(
            describe_export(&s, Some("private")),
            "Sent 66 games to 2 new private studies on Lichess: A (1/2) and A (2/2)"
        );
        let s = summary(vec![sent("A", 1, true)], 1);
        assert_eq!(
            describe_export(&s, Some("unlisted")),
            "Sent 1 game to A, a new unlisted study on Lichess"
        );
        let mut s = summary(vec![sent("Repertoire", 4, false)], 5);
        s.too_long = 1;
        s.studies[0].chapters = 3;
        s.studies[0].refused = Some("Invalid PGN".into());
        assert_eq!(
            describe_export(&s, None),
            "Sent 4 games to Repertoire on Lichess (Repertoire took 3 of them as chapters: Invalid PGN; \
             1 game not sent: 1 game is too long for Lichess (over 400 kB))"
        );
        let mut s = summary(vec![sent("A (1/3)", 64, true)], 130);
        s.stopped = Some(NetError::StudyLimit);
        let msg = describe_export(&s, Some("private"));
        assert!(msg.starts_with("Lichess refused to make another study."), "{msg}");
        assert!(
            msg.ends_with(
                "Sent 64 games to A (1/3), a new private study on Lichess; the other 66 weren't sent"
            ),
            "{msg}"
        );
        assert_eq!(listing(&["a", "b", "c"]), "a, b and c");
    }

    #[test]
    fn a_study_game_loses_the_old_studys_tags() {
        let pgn = "[Event \"E\"]\n[StudyName \"Old\"]\n[ChapterName \"Ch\"]\n[ChapterURL \"https://lichess.org/study/abcd1234/efgh5678\"]\n\n1. e4 *\n";
        let game = chess_core::pgn::read_pgn_str(pgn).remove(0);
        let out = study_pgn(game);
        assert!(!out.contains("StudyName") && !out.contains("ChapterURL"), "{out}");
        assert!(out.contains("[ChapterName \"Ch\"]") && out.contains("1. e4"), "{out}");
    }
}
