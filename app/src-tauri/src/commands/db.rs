use std::path::PathBuf;

use chess_db::{
    Database, EloMode, GameSet, HeaderQuery, ResultFilter, SortSpec, build_sort, export_pgn as export_db,
    import_pgn as import_db, page_ids, search_headers,
};
use chess_jobs::JobError;
use tauri::{AppHandle, Manager, State};

use crate::dto::{DbInfoDto, FilterDto, GamePage, GameRowDto, HeaderQueryDto, IndexStatusDto, SortSpecDto};
use crate::error::{AppError, AppResult};
use crate::state::{ActiveFilter, AppState};

const MAX_PAGE: u32 = 1000;

/// `1234567` → `1,234,567` for messages shown to the user.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn info(state: &AppState, handle: u32) -> AppResult<DbInfoDto> {
    let db = state.db(handle)?;
    let filter = state
        .filters
        .lock()
        .expect("filters lock")
        .get(&handle)
        .map(|f| FilterDto { count: f.ids.len() as u32, description: f.description.clone() });
    let mut db = db.lock().expect("db lock");
    let index = index_status(&db);
    let deleted = db.game_stats()?.iter().filter(|g| g.deleted).count() as u32;
    Ok(DbInfoDto::new(handle, db.info(), filter, index, deleted))
}

pub fn index_status(db: &Database) -> IndexStatusDto {
    let p = db.positions();
    IndexStatusDto {
        built: p.is_built(),
        covered: p.covered(),
        up_to_date: p.plan(db.count()) == chess_db::IndexPlan::UpToDate,
        max_ply: p.max_ply(),
        size_bytes: p.size_bytes(),
    }
}

#[tauri::command]
pub async fn create_db(state: State<'_, AppState>, path: String) -> AppResult<DbInfoDto> {
    let db = Database::create(&PathBuf::from(path))?;
    log::info!("created database {}", db.path().display());
    let handle = state.add_db(db);
    info(&state, handle)
}

#[tauri::command]
pub async fn open_db(state: State<'_, AppState>, path: String) -> AppResult<DbInfoDto> {
    let path = PathBuf::from(path);
    let path = path.canonicalize().unwrap_or(path);
    if let Some(handle) = state.find_db(&path) {
        return info(&state, handle);
    }
    let db = Database::open(&path)?;
    log::info!("opened database {} ({} games)", path.display(), db.count());
    let handle = state.add_db(db);
    info(&state, handle)
}

#[tauri::command]
pub async fn close_db(state: State<'_, AppState>, handle: u32) -> AppResult<()> {
    if state.remove_db(handle) { Ok(()) } else { Err(AppError::not_found(format!("database {handle}"))) }
}

#[tauri::command]
pub async fn db_info(state: State<'_, AppState>, handle: u32) -> AppResult<DbInfoDto> {
    info(&state, handle)
}

/// One page of the game list. Fails with `notSorted` if `sort` needs a
/// permutation that hasn't been built (call `sort_games` first).
#[tauri::command]
pub async fn list_games(
    state: State<'_, AppState>,
    handle: u32,
    sort: SortSpecDto,
    offset: u32,
    limit: u32,
) -> AppResult<GamePage> {
    let db = state.db(handle)?;
    let mut filters = state.filters.lock().expect("filters lock");
    let db = db.lock().expect("db lock");
    let limit = limit.min(MAX_PAGE);
    let (ids, total) = match filters.get_mut(&handle) {
        Some(filter) => {
            let order = filter.ordered(&db, sort.into())?;
            let end = offset.saturating_add(limit).min(order.len() as u32);
            let ids = order.get(offset as usize..end as usize).map(<[u32]>::to_vec).unwrap_or_default();
            (ids, order.len() as u32)
        }
        None => (page_ids(&db, sort.into(), offset, limit)?, db.count()),
    };
    let rows = db.rows(&ids)?.into_iter().map(GameRowDto::from).collect();
    Ok(GamePage { total, offset, rows })
}

/// Starts building a sort order. Returns `None` when no job is needed.
#[tauri::command]
pub async fn sort_games(
    state: State<'_, AppState>,
    handle: u32,
    sort: SortSpecDto,
) -> AppResult<Option<u64>> {
    let spec: SortSpec = sort.into();
    let db = state.db(handle)?;
    if spec.is_natural_order() || db.lock().expect("db lock").has_sort(spec) {
        return Ok(None);
    }
    let id = state.jobs.spawn("sort", "Sorting game list", move |ctx| {
        let perm = build_sort(&db, spec, ctx).map_err(to_job_error)?;
        Ok(format!("Sorted {} games", thousands(perm.len() as u64)))
    });
    Ok(Some(id))
}

pub fn to_job_error(e: chess_db::DbError) -> JobError {
    match e {
        chess_db::DbError::Cancelled => JobError::Cancelled,
        other => JobError::failed(other),
    }
}

fn file_label(path: &std::path::Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

#[tauri::command]
pub async fn import_pgn(state: State<'_, AppState>, handle: u32, path: String) -> AppResult<u64> {
    let db = state.db(handle)?;
    let path = PathBuf::from(path);
    let label = format!("Importing {}", file_label(&path));
    Ok(state.jobs.spawn("import", label, move |ctx| {
        let summary = import_db(&db, &path, ctx).map_err(to_job_error)?;
        for w in summary.warnings.iter().take(20) {
            log::warn!("{}: game {}: {}", path.display(), w.game_index + 1, w.message);
        }
        if summary.cancelled {
            log::info!("import of {} cancelled after {} games", path.display(), summary.imported);
            return Err(JobError::Cancelled);
        }
        let mut msg = match summary.imported {
            1 => "Imported 1 game".to_owned(),
            n => format!("Imported {} games", thousands(n)),
        };
        if summary.skipped > 0 || summary.warning_count > 0 {
            msg.push_str(&format!(
                " ({} skipped, {} warnings)",
                thousands(summary.skipped),
                thousands(summary.warning_count)
            ));
        }
        Ok(msg)
    }))
}

/// A short description of a search, for the filter bar.
fn describe(q: &HeaderQuery) -> String {
    let mut parts = Vec::new();
    let t = |s: &str| s.trim().to_owned();
    if !t(&q.text).is_empty() {
        parts.push(format!("“{}”", t(&q.text)));
    }
    match (t(&q.white), t(&q.black)) {
        (w, b) if !w.is_empty() && !b.is_empty() => {
            parts.push(format!("{w} vs {b}{}", if q.ignore_colors { " (either colour)" } else { "" }))
        }
        (w, _) if !w.is_empty() => {
            parts.push(if q.ignore_colors { format!("Player: {w}") } else { format!("White: {w}") })
        }
        (_, b) if !b.is_empty() => {
            parts.push(if q.ignore_colors { format!("Player: {b}") } else { format!("Black: {b}") })
        }
        _ => {}
    }
    for (label, v) in [("Event", &q.event), ("Site", &q.site), ("Annotator", &q.annotator)] {
        if !t(v).is_empty() {
            parts.push(format!("{label}: {}", t(v)));
        }
    }
    match (q.year_from, q.year_to) {
        (Some(a), Some(b)) => parts.push(format!("{a}–{b}")),
        (Some(a), None) => parts.push(format!("from {a}")),
        (None, Some(b)) => parts.push(format!("until {b}")),
        _ => {}
    }
    match (t(&q.eco_from).to_uppercase(), t(&q.eco_to).to_uppercase()) {
        (a, b) if !a.is_empty() && !b.is_empty() && a != b => parts.push(format!("ECO {a}–{b}")),
        (a, _) if !a.is_empty() => parts.push(format!("ECO {a}")),
        (_, b) if !b.is_empty() => parts.push(format!("ECO up to {b}")),
        _ => {}
    }
    if let Some(r) = q.result {
        parts.push(
            match r {
                ResultFilter::WhiteWins => "White won",
                ResultFilter::BlackWins => "Black won",
                ResultFilter::Draw => "Draws",
                ResultFilter::Decisive => "Decisive games",
            }
            .to_owned(),
        );
    }
    let who = match q.elo_mode {
        EloMode::Both => "both players",
        EloMode::Either => "either player",
        EloMode::Average => "average",
    };
    match (q.elo_min, q.elo_max) {
        (Some(a), Some(b)) => parts.push(format!("Elo {a}–{b} ({who})")),
        (Some(a), None) => parts.push(format!("Elo ≥ {a} ({who})")),
        (None, Some(b)) => parts.push(format!("Elo ≤ {b} ({who})")),
        _ => {}
    }
    match (q.moves_min, q.moves_max) {
        (Some(a), Some(b)) => parts.push(format!("{a}–{b} moves")),
        (Some(a), None) => parts.push(format!("at least {a} moves")),
        (None, Some(b)) => parts.push(format!("at most {b} moves")),
        _ => {}
    }
    if q.annotated_only {
        parts.push("annotated".into());
    }
    if q.skip_deleted {
        parts.push("not deleted".into());
    }
    if parts.is_empty() { "All games".into() } else { parts.join(" · ") }
}

/// Starts a header search (SRCH-01) that becomes the list's filter when it
/// finishes. `within` searches the current results (SRCH-14); `quick` marks
/// quick-filter searches (SRCH-02), which report no completion notice.
/// An empty query clears the filter and starts no job (`None`).
#[tauri::command]
pub async fn search_games(
    app: AppHandle,
    state: State<'_, AppState>,
    handle: u32,
    query: HeaderQueryDto,
    within: bool,
    quick: bool,
) -> AppResult<Option<u64>> {
    let db = state.db(handle)?;
    let query: HeaderQuery = query.into();
    let base = within
        .then(|| {
            state
                .filters
                .lock()
                .expect("filters lock")
                .get(&handle)
                .map(|f| (f.ids.clone(), f.description.clone()))
        })
        .flatten();
    if query == HeaderQuery::default() && base.is_none() {
        state.filters.lock().expect("filters lock").remove(&handle);
        return Ok(None);
    }
    let mut description = describe(&query);
    if let Some((_, base_desc)) = &base {
        description = format!("{base_desc}, then {description}");
    }
    let (kind, label) = if quick { ("filter", "Filtering games") } else { ("search", "Searching games") };
    Ok(Some(state.jobs.spawn(kind, label, move |ctx| {
        let within = base.as_ref().map(|(ids, _)| GameSet::from_ids(ids));
        let ids = search_headers(&db, &query, within.as_ref(), ctx).map_err(to_job_error)?;
        let n = ids.len() as u64;
        app.state::<AppState>()
            .filters
            .lock()
            .expect("filters lock")
            .insert(handle, ActiveFilter::new(ids, description));
        Ok(match n {
            0 => "No games match the search".to_owned(),
            1 => "Found 1 game".to_owned(),
            n => format!("Found {} games", thousands(n)),
        })
    })))
}

/// Compacts the database (DB-06) in a background job: deleted games and
/// the space of old game versions are removed for good, games are
/// renumbered. The filter is cleared; open games follow their new ids.
#[tauri::command]
pub async fn compact_db(app: AppHandle, state: State<'_, AppState>, handle: u32) -> AppResult<u64> {
    let db = state.db(handle)?;
    state.filters.lock().expect("filters lock").remove(&handle);
    Ok(state.jobs.spawn("compact", "Compacting the database", move |ctx| {
        let summary = db.lock().expect("db lock").compact(ctx).map_err(to_job_error)?;
        app.state::<AppState>().remap_sources(handle, &summary.mapping);
        let mb = |b: u64| format!("{:.1} MB", b as f64 / 1e6);
        let games = match summary.removed {
            1 => "1 deleted game".to_owned(),
            n => format!("{} deleted games", thousands(u64::from(n))),
        };
        Ok(format!("Compacted: removed {games}; {} → {}", mb(summary.bytes_before), mb(summary.bytes_after)))
    }))
}

/// Shows all games again.
#[tauri::command]
pub async fn clear_filter(state: State<'_, AppState>, handle: u32) -> AppResult<DbInfoDto> {
    state.filters.lock().expect("filters lock").remove(&handle);
    info(&state, handle)
}

/// Exports `ids`, or the filtered games when a filter is active (IO-11), or
/// else all games. Deleted games are skipped unless listed in `ids`.
#[tauri::command]
pub async fn export_pgn(
    state: State<'_, AppState>,
    handle: u32,
    path: String,
    ids: Option<Vec<u32>>,
) -> AppResult<u64> {
    let db = state.db(handle)?;
    let ids = match ids {
        Some(ids) => Some(ids),
        None => {
            let filtered = state.filters.lock().expect("filters lock").get(&handle).map(|f| f.ids.clone());
            match filtered {
                Some(f) => {
                    let db = db.lock().expect("db lock");
                    let mut keep = Vec::with_capacity(f.len());
                    for &id in f.iter() {
                        if !db.record(id)?.is_deleted() {
                            keep.push(id);
                        }
                    }
                    Some(keep)
                }
                None => None,
            }
        }
    };
    let path = PathBuf::from(path);
    let label = format!("Exporting {}", file_label(&path));
    Ok(state.jobs.spawn("export", label, move |ctx| {
        let n = export_db(&db, ids, &path, ctx).map_err(to_job_error)?;
        Ok(format!("Exported {} games to {}", thousands(n), file_label(&path)))
    }))
}

#[tauri::command]
pub async fn set_deleted(
    state: State<'_, AppState>,
    handle: u32,
    ids: Vec<u32>,
    deleted: bool,
) -> AppResult<()> {
    let db = state.db(handle)?;
    let mut db = db.lock().expect("db lock");
    for id in ids {
        db.set_deleted(id, deleted)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn thousands_separators() {
        assert_eq!(super::thousands(0), "0");
        assert_eq!(super::thousands(999), "999");
        assert_eq!(super::thousands(1000), "1,000");
        assert_eq!(super::thousands(1_000_000), "1,000,000");
        assert_eq!(super::thousands(12_345_678), "12,345,678");
    }
}
