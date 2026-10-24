//! Downloading a player's games from Lichess or Chess.com (NET-01): looking
//! the player up, then the download into an open database as a job.

use std::sync::OnceLock;

use chess_jobs::JobError;
use chess_net::{Client, DownloadSummary, NetError};
use tauri::{AppHandle, Manager, State};

use crate::commands::db::{thousands, to_job_error};
use crate::dto::{DownloadRequestDto, ImportReportDto, OnlinePlayerDto, OnlineSiteDto};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::state::AppState;

/// One client for the app, so connections are kept between requests.
fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(Client::new)
}

/// Looks a player up: whether the site has them, and their name as it
/// writes it. A site that's had too many requests is an error at once,
/// not a wait of minutes behind "Looking up…".
#[tauri::command]
pub async fn online_player(site: OnlineSiteDto, username: String) -> AppResult<OnlinePlayerDto> {
    tauri::async_runtime::spawn_blocking(move || {
        chess_net::player(&client().without_retries(), site.into(), &username, &|| false)
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
fn describe(s: &DownloadSummary) -> String {
    let (who, site) = (&s.player.username, s.player.site.name());
    let mut msg = match (s.import.imported, s.already) {
        (0, 0) => format!("No games of {who} on {site} to download"),
        (0, _) => format!("No new games of {who} on {site}"),
        (n, 0) => format!("Downloaded {} of {who} from {site}", games(n)),
        (n, _) => format!(
            "Downloaded {} new {} of {who} from {site}",
            thousands(n),
            if n == 1 { "game" } else { "games" }
        ),
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

/// Downloads the player's games into the open database `handle` as a job.
/// Games it has already are left out. When the download had warnings,
/// `import_report` returns them afterwards.
#[tauri::command]
pub async fn download_games(
    app: AppHandle,
    state: State<'_, AppState>,
    handle: u32,
    request: DownloadRequestDto,
) -> AppResult<u64> {
    let request = request.to_request()?;
    let name = chess_net::check_username(&request.username)?.to_owned();
    let db = state.db(handle)?;
    let label = format!("Downloading {name}'s {} games", request.site.name());
    state.import_reports.lock().expect("import reports lock").remove(&handle);
    Ok(state.jobs.spawn("download", label, move |ctx| {
        let s = chess_net::download_into(client(), &db, &request, ctx).map_err(|e| match e {
            NetError::Cancelled => JobError::Cancelled,
            NetError::Db(e) => to_job_error(e),
            e => JobError::failed(e),
        })?;
        if s.import.warning_count > 0 {
            let what = format!("{}'s games from {}", s.player.username, s.player.site.name());
            let report = ImportReportDto::new(what, &s.import);
            app.state::<AppState>()
                .import_reports
                .lock()
                .expect("import reports lock")
                .insert(handle, report);
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
                Err(JobError::failed(format!("{e}.{kept} Download the new ones only to get the rest")))
            }
            None => Ok(describe(&s)),
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
            stopped: None,
            cancelled: false,
        }
    }

    #[test]
    fn describes_the_download() {
        assert_eq!(describe(&summary(0, 0)), "No games of DrNykterstein on Lichess to download");
        assert_eq!(
            describe(&summary(0, 1)),
            "No new games of DrNykterstein on Lichess (1 was in the database already)"
        );
        assert_eq!(describe(&summary(1234, 0)), "Downloaded 1,234 games of DrNykterstein from Lichess");
        assert_eq!(
            describe(&summary(1, 1200)),
            "Downloaded 1 new game of DrNykterstein from Lichess (1,200 were in the database already)"
        );
        let mut s = summary(5, 0);
        s.import.skipped = 1;
        s.import.warning_count = 2;
        assert_eq!(describe(&s), "Downloaded 5 games of DrNykterstein from Lichess (1 skipped, 2 warnings)");
    }
}
