//! The game list's look per database (QOL-07): sort order, column widths
//! and filter are saved in the settings as they change and put back when
//! the database is opened again.

use std::collections::BTreeMap;
use std::sync::Mutex;

use chess_core::position;
use chess_db::{
    Database, DuplicateOptions, GameId, GameSet, NameKind, Progress, SortSpec, find_duplicates,
    find_position, games_with_name, scan_position, search_headers,
};
use chess_jobs::JobError;
use tauri::{AppHandle, Manager, State};

use crate::commands::db::{thousands, to_job_error};
use crate::dto::{DbViewDto, SortSpecDto};
use crate::error::AppResult;
use crate::settings::FilterStep;
use crate::state::{ActiveFilter, AppState};

/// Column widths kept per database, at most.
const MAX_COLUMNS: usize = 32;
/// Column widths are kept within this range (CSS pixels).
const MIN_WIDTH: u32 = 16;
const MAX_WIDTH: u32 = 2000;

/// How the list of `handle` looked last time.
#[tauri::command]
pub async fn db_view(state: State<'_, AppState>, handle: u32) -> AppResult<DbViewDto> {
    Ok(DbViewDto::from(&state.db_view(handle)?))
}

fn column_id_ok(id: &str) -> bool {
    (1..=32).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Saves the sort order and column widths of the list of `handle`.
#[tauri::command]
pub async fn save_db_layout(
    state: State<'_, AppState>,
    handle: u32,
    sort: SortSpecDto,
    columns: BTreeMap<String, u32>,
) -> AppResult<()> {
    let natural = SortSpec::from(sort).is_natural_order();
    let columns: BTreeMap<String, u32> = columns
        .into_iter()
        .filter(|(id, _)| column_id_ok(id))
        .take(MAX_COLUMNS)
        .map(|(id, w)| (id, w.clamp(MIN_WIDTH, MAX_WIDTH)))
        .collect();
    state.update_db_view(handle, |v| {
        v.sort = (!natural).then_some(sort);
        v.columns = columns;
    })
}

/// Makes the saved filter of `handle` again in a background job, which
/// installs it unless the list's filter changed meanwhile. `None` when no
/// filter is saved. A filter that can't be made again (a player who isn't
/// in the database any more) is forgotten, as is a cancelled one.
#[tauri::command]
pub async fn restore_filter(
    app: AppHandle,
    state: State<'_, AppState>,
    handle: u32,
) -> AppResult<Option<u64>> {
    // Before reading the saved filter: a newer one set in between wins.
    let generation = state.filter_generation();
    let Some(saved) = state.db_view(handle)?.filter.filter(|f| !f.steps.is_empty()) else {
        return Ok(None);
    };
    let db = state.db(handle)?;
    Ok(Some(state.jobs.spawn("filter", "Restoring the filter", move |ctx| {
        let state = app.state::<AppState>();
        match run_steps(&db, &saved.steps, ctx) {
            Ok(ids) => {
                let n = ids.len() as u64;
                let filter = ActiveFilter::new(ids, saved.description, saved.steps);
                state.finish_restore(handle, Some(filter), generation);
                Ok(format!("Restored the filter: {} games", thousands(n)))
            }
            Err(e) => {
                state.finish_restore(handle, None, generation);
                Err(e)
            }
        }
    })))
}

fn noun(kind: NameKind) -> &'static str {
    match kind {
        NameKind::Player => "player",
        NameKind::Event => "event",
        NameKind::Site => "site",
        NameKind::Annotator => "annotator",
    }
}

/// Makes a saved filter again: the ascending ids of the games its steps
/// match now, each step within the results of the ones before.
pub fn run_steps(
    db: &Mutex<Database>,
    steps: &[FilterStep],
    progress: &dyn Progress,
) -> Result<Vec<GameId>, JobError> {
    let mut ids: Option<Vec<GameId>> = None;
    for step in steps {
        let within = ids.as_deref().map(GameSet::from_ids);
        let mut found = match step {
            FilterStep::Header { query, .. } => {
                // Searches within the earlier results itself.
                search_headers(db, &query.clone().into(), within.as_ref(), progress).map_err(to_job_error)?
            }
            FilterStep::Name { table, name } => {
                let kind: NameKind = (*table).into();
                let id = db.lock().expect("db lock").names(kind).id_of(name).ok_or_else(|| {
                    JobError::failed(format!(
                        "the {} “{name}” isn't in the database any more, so all games are shown",
                        noun(kind)
                    ))
                })?;
                games_with_name(db, kind, id, progress).map_err(to_job_error)?
            }
            FilterStep::Position { fen, full } => {
                let pos = position::from_fen(fen).map_err(|_| {
                    JobError::failed("the saved position isn't valid any more, so all games are shown")
                })?;
                let hits = if *full { None } else { Some(find_position(db, &pos).map_err(to_job_error)?) };
                match hits {
                    Some(hits) if !hits.partial => hits.entries.iter().map(|e| e.game).collect(),
                    // The index is far behind (it's being built): read every
                    // game instead, which also finds the position deeper
                    // than the index goes.
                    _ => scan_position(db, &pos, progress).map_err(to_job_error)?,
                }
            }
            FilterStep::Duplicates { fuzzy } => {
                find_duplicates(db, DuplicateOptions { fuzzy: *fuzzy }, progress)
                    .map_err(to_job_error)?
                    .listed()
            }
        };
        if let Some(set) = &within
            && !matches!(step, FilterStep::Header { .. })
        {
            found.retain(|&id| set.contains(id));
        }
        ids = Some(found);
    }
    Ok(ids.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use chess_db::NoProgress;

    use super::*;
    use crate::dto::{HeaderQueryDto, NameKindDto, ResultFilterDto};

    fn fixture_db(dir: &Path) -> Mutex<Database> {
        let db = Mutex::new(Database::create(&dir.join("View")).unwrap());
        let pgn = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn");
        for f in ["variations.pgn", "setup.pgn", "lichess.pgn"] {
            chess_db::import_pgn(&db, &pgn.join(f), &NoProgress).unwrap();
        }
        db
    }

    fn header(query: HeaderQueryDto, quick: bool) -> FilterStep {
        FilterStep::Header { query, quick }
    }

    #[test]
    fn saved_filters_are_made_again() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture_db(dir.path());
        assert_eq!(db.lock().unwrap().count(), 6);
        let run = |steps: &[FilterStep]| run_steps(&db, steps, &NoProgress);

        let quick = header(HeaderQueryDto { text: "CASTLING".into(), ..Default::default() }, true);
        assert_eq!(run(std::slice::from_ref(&quick)).unwrap(), [2, 4]);

        // A search within the results of another.
        let decisive =
            header(HeaderQueryDto { result: Some(ResultFilterDto::Decisive), ..Default::default() }, false);
        assert_eq!(run(std::slice::from_ref(&decisive)).unwrap(), [0, 2, 5]);
        let player = header(HeaderQueryDto { white: "player".into(), ..Default::default() }, false);
        assert_eq!(run(&[decisive.clone(), player]).unwrap(), [5]);

        let kasparov = FilterStep::Name { table: NameKindDto::Player, name: "Kasparov, Garry".into() };
        assert_eq!(run(std::slice::from_ref(&kasparov)).unwrap(), [0]);
        // Within: a name step keeps only the earlier results.
        assert_eq!(run(&[quick.clone(), kasparov]).unwrap(), Vec::<GameId>::new());

        let start = position::to_fen(&chess_core::shakmaty::Chess::default());
        let from_start = run(&[FilterStep::Position { fen: start.clone(), full: false }]).unwrap();
        assert_eq!(from_start, run(&[FilterStep::Position { fen: start.clone(), full: true }]).unwrap());
        assert!(!from_start.is_empty() && !from_start.contains(&2), "the set-up games don't start there");
        assert_eq!(
            run(&[FilterStep::Position { fen: start, full: false }, decisive]).unwrap(),
            from_start.iter().copied().filter(|id| [0, 2, 5].contains(id)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_filter_that_cant_be_made_again_fails_in_plain_words() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture_db(dir.path());
        let gone = FilterStep::Name { table: NameKindDto::Event, name: "Nowhere Open".into() };
        let err = run_steps(&db, &[gone], &NoProgress).unwrap_err();
        assert!(err.to_string().contains("the event “Nowhere Open” isn't in the database"), "{err}");
        let bad = FilterStep::Position { fen: "not a fen".into(), full: false };
        assert!(run_steps(&db, &[bad], &NoProgress).unwrap_err().to_string().contains("position"));
        assert!(run_steps(&db, &[], &NoProgress).unwrap().is_empty());
    }

    #[test]
    fn column_ids_are_checked() {
        assert!(column_id_ok("whiteElo") && column_id_ok("flags"));
        assert!(!column_id_ok("") && !column_id_ok("a b") && !column_id_ok(&"x".repeat(33)));
    }
}
