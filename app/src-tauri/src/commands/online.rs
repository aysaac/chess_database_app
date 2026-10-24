//! Downloading a player's games from Lichess or Chess.com (NET-01): looking
//! the player up, then the download into an open database as a job.

use std::sync::OnceLock;

use chess_jobs::JobError;
use chess_net::{Client, DownloadSummary, NetError};
use tauri::{AppHandle, Manager, State};

use crate::commands::db::{thousands, to_job_error};
use crate::dto::{DownloadRequestDto, ImportReportDto, OnlinePlayerDto, OnlineSiteDto, PeriodDto};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::settings::DownloadMark;
use crate::state::AppState;

/// One client for the app, so connections are kept between requests.
fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(Client::new)
}

/// Looks a player up: whether the site has them, and their name as it
/// writes it. A site that's had too many requests, or takes over 30
/// seconds, is an error at once, not a wait behind "Looking up…".
#[tauri::command]
pub async fn online_player(site: OnlineSiteDto, username: String) -> AppResult<OnlinePlayerDto> {
    tauri::async_runtime::spawn_blocking(move || {
        chess_net::player(&client().for_lookup(), site.into(), &username, &|| false)
    })
    .await
    .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))?
    .map(|p| OnlinePlayerDto::from(&p))
    .map_err(AppError::from)
}

fn games(n: u64) -> String {
    match n {
        1 => "1 game".to_owned(),
        n => format!("{} games", thousands(n)),
    }
}

/// What the download did, for the notice: "Downloaded 12 games of
/// DrNykterstein from Lichess (1,200 were in the database already)".
/// `first`: "new ones only" with no earlier download of these games into
/// the database, so all of them.
fn describe(s: &DownloadSummary, first: bool) -> String {
    let (who, site) = (&s.player.username, s.player.site.name());
    let mut msg = match (s.import.imported, s.already) {
        (0, 0) if s.since.is_none() => format!("No games of {who} on {site} to download"),
        (0, _) => format!("No new games of {who} on {site}"),
        (n, 0) => format!("Downloaded {} of {who} from {site}", games(n)),
        (n, _) => format!(
            "Downloaded {} new {} of {who} from {site}",
            thousands(n),
            if n == 1 { "game" } else { "games" }
        ),
    };
    let mut notes = Vec::new();
    if first {
        notes.push("all of them: the first download of these games into this database".to_owned());
    }
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

/// Frees the one download's place (`AppState::download`) when its job ends.
struct DownloadSlot(AppHandle);

impl Drop for DownloadSlot {
    fn drop(&mut self) {
        *self.0.state::<AppState>().download.lock().expect("download lock") = None;
    }
}

/// Downloads the player's games into the open database `handle` as a job,
/// one download at a time. Games it has already are left out. "New ones
/// only" starts where the last download of the same games into it got to
/// (all of them if none did), and every download notes how far it got.
/// When it had warnings, `import_report` returns them afterwards.
#[tauri::command]
pub async fn download_games(
    app: AppHandle,
    state: State<'_, AppState>,
    handle: u32,
    request: DownloadRequestDto,
) -> AppResult<u64> {
    let name = chess_net::check_username(&request.username)?.to_owned();
    let db = state.db(handle)?;
    let wanted = DownloadMark::new(request.site, &name, &request.speeds, request.rated_only, 0);
    let new_from = state.db_view(handle).ok().and_then(|v| v.download_mark(&wanted));
    let first = request.period == PeriodDto::NewOnly && new_from.is_none();
    let req = request.to_request(new_from)?;
    let label = format!("Downloading {name}'s {} games", req.site.name());
    {
        let mut running = state.download.lock().expect("download lock");
        if let Some(other) = running.as_deref() {
            return Err(AppError::invalid(format!(
                "Another download is running ({other}). Downloads go one at a time: wait for it to end, or stop it in the status bar"
            )));
        }
        *running = Some(label.clone());
    }
    let slot = DownloadSlot(app.clone());
    state.import_reports.lock().expect("import reports lock").remove(&handle);
    Ok(state.jobs.spawn("download", label, move |ctx| {
        let _slot = slot;
        let s = chess_net::download_into(client(), &db, &req, ctx).map_err(|e| match e {
            NetError::Cancelled => JobError::Cancelled,
            NetError::Db(e) => to_job_error(e),
            e => JobError::failed(e),
        })?;
        let state = app.state::<AppState>();
        // How far it got (a cancel or a failure partway too), for the next
        // "new ones only".
        if let Some(reached) = s.covered
            && let Err(e) =
                state.update_db_view(handle, |v| v.note_download(DownloadMark { reached, ..wanted }))
        {
            log::warn!("couldn't note how far the download got: {}", e.message);
        }
        if s.import.warning_count > 0 {
            let what = format!("{}'s games from {}", s.player.username, s.player.site.name());
            let report = ImportReportDto::new(what, &s.import);
            state.import_reports.lock().expect("import reports lock").insert(handle, report);
        }
        if s.cancelled {
            log::info!("download of {name}'s games cancelled after {} games", s.import.imported);
            return Err(JobError::Cancelled);
        }
        match &s.stopped {
            // The games before the failure are in; the rest can follow.
            Some(e) => {
                let kept = match s.import.imported {
                    0 => String::new(),
                    1 => " The game downloaded before stays in the database.".to_owned(),
                    n => format!(" The {} games downloaded before stay in the database.", thousands(n)),
                };
                Err(JobError::failed(format!("{e}.{kept} Download the new ones only to get the rest.")))
            }
            None => Ok(describe(&s, first)),
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_db::ImportSummary;
    use chess_net::{Player, Site};

    fn summary(imported: u64, already: u64) -> DownloadSummary {
        DownloadSummary {
            player: Player { site: Site::Lichess, username: "DrNykterstein".into(), joined: 0, games: None },
            import: ImportSummary { imported, ..Default::default() },
            already,
            since: None,
            covered: None,
            stopped: None,
            cancelled: false,
        }
    }

    #[test]
    fn describes_the_download() {
        assert_eq!(describe(&summary(0, 0), false), "No games of DrNykterstein on Lichess to download");
        assert_eq!(
            describe(&summary(0, 1), false),
            "No new games of DrNykterstein on Lichess (1 was in the database already)"
        );
        assert_eq!(
            describe(&summary(1234, 0), false),
            "Downloaded 1,234 games of DrNykterstein from Lichess"
        );
        assert_eq!(
            describe(&summary(1, 1200), false),
            "Downloaded 1 new game of DrNykterstein from Lichess (1,200 were in the database already)"
        );
        let mut s = summary(5, 0);
        s.import.skipped = 1;
        s.import.warning_count = 2;
        assert_eq!(
            describe(&s, false),
            "Downloaded 5 games of DrNykterstein from Lichess (1 skipped, 2 warnings)"
        );
        let mut s = summary(0, 0);
        s.since = Some(1);
        assert_eq!(describe(&s, false), "No new games of DrNykterstein on Lichess");
        assert_eq!(
            describe(&summary(3, 0), true),
            "Downloaded 3 games of DrNykterstein from Lichess (all of them: the first download of these games into this database)"
        );
    }
}
