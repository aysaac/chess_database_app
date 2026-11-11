//! Importing games, the only write in phase 1: a PGN upload into a
//! database one may write to, or (the admin) a PGN or ChessBase `.cbh` file
//! from the server's import folder. Each import is a job; the position
//! index is brought up to date after it, so trees work.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use chess_db::{
    DbError, ImportSummary, IndexSummary, Progress, import_cb, import_pgn, update_position_index,
};
use chess_jobs::{JobContext, JobError};
use http_body_util::BodyExt;
use tokio::io::AsyncWriteExt;

use crate::access::require;
use crate::auth::{AdminAuth, Auth};
use crate::error::{ApiError, ApiResult};
use crate::jobs::{thousands, to_job_error};
use crate::pool::{OpenDb, dir_size, writer_busy};
use crate::secret;
use crate::store::{DbMeta, User};
use crate::wire::{Access, ImportFileRequest, ImportKind, ImportReportDto, JobStarted, UploadQuery};
use crate::{AppState, blocking};

/// How much more the database may grow: the least of its own quota and its
/// owner's (less all their databases, as big as they are now: imports
/// running into them included). None when neither has one. Sizes are read
/// from the folders, never from a database (an import checking this may
/// hold its lock). Blocking.
pub fn room(state: &AppState, meta: &DbMeta) -> ApiResult<Option<u64>> {
    let size = dir_size(&state.pool.path(&meta.id));
    let mut room = meta.quota_bytes.map(|q| q.saturating_sub(size));
    let owner = state.store.user(meta.owner_id)?.ok_or_else(ApiError::not_found)?;
    if let Some(q) = owner.quota_bytes {
        let used: u64 =
            state.store.databases_owned_by(owner.id)?.iter().map(|d| dir_size(&state.pool.path(&d.id))).sum();
        let left = q.saturating_sub(used);
        room = Some(room.map_or(left, |r| r.min(left)));
    }
    Ok(room)
}

/// Passes progress to the job, and stops it (an import, an index build)
/// once the database or its owner has no room left: [`room`] again every
/// half second, so imports running at once into one owner's databases share
/// the owner's quota instead of each counting on all of it.
pub struct Guarded<'a> {
    ctx: &'a JobContext,
    state: &'a AppState,
    meta: &'a DbMeta,
    over: AtomicBool,
    checked: Mutex<Instant>,
}

impl<'a> Guarded<'a> {
    pub fn new(state: &'a AppState, meta: &'a DbMeta, ctx: &'a JobContext) -> Guarded<'a> {
        // The first report checks at once: a small file is one batch.
        let long_ago = Instant::now().checked_sub(Duration::from_secs(1)).unwrap_or_else(Instant::now);
        Guarded { ctx, state, meta, over: AtomicBool::new(false), checked: Mutex::new(long_ago) }
    }

    /// Whether it stopped the job at the quota.
    pub fn over(&self) -> bool {
        self.over.load(Ordering::Relaxed)
    }
}

impl Progress for Guarded<'_> {
    fn report(&self, done: u64, total: u64) {
        self.ctx.progress(done, total);
        let mut checked = self.checked.lock().expect("quota check lock");
        if checked.elapsed() >= Duration::from_millis(500) {
            *checked = Instant::now();
            match room(self.state, self.meta) {
                Ok(Some(0)) => self.over.store(true, Ordering::Relaxed),
                Ok(_) => {}
                Err(e) => log::warn!("database {}: quota check: {}", self.meta.id, e.message),
            }
        }
    }

    fn is_cancelled(&self) -> bool {
        self.ctx.is_cancelled() || self.over()
    }
}

/// Brings the position index up to date under the quota ([`Guarded`]): the
/// index counts toward it. A build stopped there is noted, so
/// `games::ensure_index` doesn't start another until the database has more
/// room. Blocking.
pub fn index_within_quota(
    state: &AppState,
    open: &OpenDb,
    meta: &DbMeta,
    ctx: &JobContext,
) -> Result<IndexSummary, JobError> {
    let before = room(state, meta).map_err(|e| JobError::Failed(e.message))?;
    let guard = Guarded::new(state, meta, ctx);
    let result = update_position_index(&open.db, &guard);
    let mut cache = open.cache.lock().expect("cache lock");
    match result {
        Err(DbError::Cancelled) if guard.over() && !ctx.is_cancelled() => {
            cache.index_stopped_at = Some(before.unwrap_or(0));
            Err(JobError::Failed("Stopped indexing the positions: the database reached its quota".into()))
        }
        Ok(summary) => {
            cache.index_stopped_at = None;
            Ok(summary)
        }
        Err(e) => Err(to_job_error(e)),
    }
}

/// Deletes an uploaded file when the import is done with it.
struct TempFile(Option<PathBuf>);

impl Drop for TempFile {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

struct Import {
    meta: DbMeta,
    open: Arc<OpenDb>,
    path: PathBuf,
    kind: ImportKind,
    /// What the job and its report call the file.
    file: String,
    /// The upload to delete afterwards.
    _temp: TempFile,
}

fn run(state: &AppState, import: &Import, ctx: &JobContext) -> Result<String, JobError> {
    let guard = Guarded::new(state, &import.meta, ctx);
    let summary: ImportSummary = match import.kind {
        ImportKind::Pgn => import_pgn(&import.open.db, &import.path, &guard),
        ImportKind::Cbh => import_cb(&import.open.db, &import.path, &guard),
    }
    .map_err(to_job_error)?;
    state.jobs.set_report(ctx.id(), ImportReportDto::new(import.file.clone(), &summary));
    let over = guard.over();
    log::info!(
        "imported {} games from {} into {} ({} skipped{})",
        summary.imported,
        import.file,
        import.meta.id,
        summary.skipped,
        if over {
            ", stopped at the quota"
        } else if summary.cancelled {
            ", cancelled"
        } else {
            ""
        }
    );
    if ctx.is_cancelled() {
        // The games read so far stay; the next tree request indexes them.
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
    if over {
        // No room for their index either: it waits for more (`games::ensure_index`).
        return Err(JobError::Failed(format!("{msg}, then stopped: the database reached its quota")));
    }
    ctx.progress_msg(0, 0, "Indexing positions");
    let indexed = index_within_quota(state, &import.open, &import.meta, ctx).map_err(|e| match e {
        JobError::Failed(why) => JobError::Failed(format!("{msg}. {why}")),
        cancelled => cancelled,
    })?;
    if indexed.games > 0 {
        msg.push_str(&format!("; indexed the positions of {} games", thousands(u64::from(indexed.games))));
    }
    Ok(msg)
}

fn quota_exceeded() -> ApiError {
    ApiError::new(
        StatusCode::INSUFFICIENT_STORAGE,
        "quotaExceeded",
        "The database or its owner is at their quota",
    )
}

fn busy() -> ApiError {
    ApiError::conflict("An import or index build runs on this database: wait for it to finish")
}

/// Checks the database may take an import now, and starts it. Blocking.
fn start(
    state: &Arc<AppState>,
    user: &User,
    meta: DbMeta,
    kind: ImportKind,
    path: PathBuf,
    file: String,
    temp: TempFile,
) -> ApiResult<u64> {
    if room(state, &meta)? == Some(0) {
        return Err(quota_exceeded());
    }
    let open = state.pool.get(&meta.id)?;
    let mut writer = open.writer.lock().expect("writer lock");
    if writer_busy(&writer, &state.jobs.manager) {
        return Err(busy());
    }
    let label = format!("Importing {file} into {}", meta.name);
    let import = Import { open: open.clone(), meta, path, kind, file, _temp: temp };
    let st = state.clone();
    let db = (import.meta.id.clone(), import.meta.name.clone());
    let job = state.jobs.spawn("import", label, (&db.0, &db.1), user, move |ctx| run(&st, &import, ctx));
    *writer = Some(job);
    Ok(job)
}

/// The file name to show for an upload: its last component, or "upload.pgn".
fn upload_name(q: &UploadQuery) -> String {
    q.filename
        .as_deref()
        .and_then(|f| f.rsplit(['/', '\\']).next())
        .map(str::trim)
        .filter(|f| !f.is_empty() && f.chars().count() <= 200 && !f.chars().any(char::is_control))
        .unwrap_or("upload.pgn")
        .to_owned()
}

/// Imports the PGN sent as the request body (`?filename=` names it), up to
/// `CHESS_SERVER_MAX_UPLOAD_MB`. Answers 202 with the job.
pub async fn upload(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    UrlPath(id): UrlPath<String>,
    Query(q): Query<UploadQuery>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<(StatusCode, Json<JobStarted>)> {
    // Permissions first: nothing is read from someone who may not import,
    // or into a database that can't take it now. All of it is checked again
    // once the body is in (below).
    let st = state.clone();
    let (user, db) = (auth.user.clone(), id.clone());
    blocking(move || {
        let (meta, _) = require(&st.store, &user, &db, Access::Write)?;
        if room(&st, &meta)? == Some(0) {
            return Err(quota_exceeded());
        }
        if let Some(open) = st.pool.get_open(&meta.id)
            && writer_busy(&open.writer.lock().expect("writer lock"), &st.jobs.manager)
        {
            return Err(busy());
        }
        Ok(())
    })
    .await?;
    let max = state.config.max_upload;
    let too_large = || ApiError::too_large(format!("An upload can have at most {} MB", max / (1024 * 1024)));
    let declared =
        headers.get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > max) {
        return Err(too_large());
    }

    let path = state.config.tmp_dir().join(format!("upload-{}.pgn", secret::new_token()));
    let temp = TempFile(Some(path.clone()));
    let mut file = tokio::fs::File::create_new(&path).await?;
    let mut received = 0u64;
    let mut body = http_body_util::Limited::new(body, usize::try_from(max).unwrap_or(usize::MAX));
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| {
            if e.downcast_ref::<http_body_util::LengthLimitError>().is_some() {
                too_large()
            } else {
                ApiError::bad_request(format!("The upload broke off: {e}"))
            }
        })?;
        if let Ok(data) = frame.into_data() {
            received += data.len() as u64;
            file.write_all(&data).await?;
        }
    }
    file.sync_all().await?;
    drop(file);
    if received == 0 {
        return Err(ApiError::bad_request("The upload is empty: send the PGN as the request body"));
    }

    let name = upload_name(&q);
    let job = blocking(move || {
        // A long upload: the user may have been disabled, or lost their
        // share, since it began.
        let user =
            state.store.user(auth.user.id)?.filter(|u| !u.disabled).ok_or_else(ApiError::unauthorized)?;
        let (meta, _) = require(&state.store, &user, &id, Access::Write)?;
        start(&state, &user, meta, ImportKind::Pgn, path, name, temp)
    })
    .await?;
    Ok((StatusCode::ACCEPTED, Json(JobStarted { job })))
}

/// `rel` inside `root`, refusing anything that leaves it: absolute paths,
/// `..`, and links pointing out.
pub fn resolve_in(root: &Path, rel: &str) -> ApiResult<PathBuf> {
    let refuse = || ApiError::bad_request("Name a file inside the import folder");
    let rel = Path::new(rel);
    if rel.as_os_str().is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(refuse());
    }
    let root = root.canonicalize()?;
    let path = root
        .join(rel)
        .canonicalize()
        .map_err(|_| ApiError::bad_request("No such file in the import folder"))?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err(refuse());
    }
    Ok(path)
}

pub fn import_kind(path: &Path) -> Option<ImportKind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "pgn" => Some(ImportKind::Pgn),
        "cbh" => Some(ImportKind::Cbh),
        _ => None,
    }
}

/// The admin imports a file from the import folder (`CHESS_SERVER_DATA/import`)
/// into a database they may write to: a PGN, or a ChessBase database by its
/// `.cbh` (its `.cbg`, `.cbp`, `.cbt` and `.cbc` beside it).
pub async fn import_file(
    State(state): State<Arc<AppState>>,
    AdminAuth(auth): AdminAuth,
    UrlPath(id): UrlPath<String>,
    Json(req): Json<ImportFileRequest>,
) -> ApiResult<(StatusCode, Json<JobStarted>)> {
    let job = blocking(move || {
        let (meta, _) = require(&state.store, &auth.user, &id, Access::Write)?;
        let path = resolve_in(&state.config.import_dir(), &req.path)?;
        let kind = import_kind(&path).ok_or_else(|| ApiError::bad_request("Import a .pgn or a .cbh file"))?;
        start(&state, &auth.user, meta, kind, path, req.path.clone(), TempFile(None))
    })
    .await?;
    Ok((StatusCode::ACCEPTED, Json(JobStarted { job })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_in_the_import_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("import");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/a.pgn"), "x").unwrap();
        std::fs::write(dir.path().join("secret.pgn"), "x").unwrap();
        assert!(resolve_in(&root, "sub/a.pgn").unwrap().ends_with("sub/a.pgn"));
        for bad in
            ["", "../secret.pgn", "sub/../../secret.pgn", "/etc/passwd", "sub", "missing.pgn", "./sub/a.pgn"]
        {
            assert!(resolve_in(&root, bad).is_err(), "{bad}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path().join("secret.pgn"), root.join("link.pgn")).unwrap();
            assert!(resolve_in(&root, "link.pgn").is_err(), "a link out of the folder");
        }
        assert_eq!(import_kind(Path::new("a.PGN")), Some(ImportKind::Pgn));
        assert_eq!(import_kind(Path::new("Mega.cbh")), Some(ImportKind::Cbh));
        assert_eq!(import_kind(Path::new("a.txt")), None);
    }

    #[test]
    fn upload_names() {
        let name = |f: &str| upload_name(&UploadQuery { filename: Some(f.into()) });
        assert_eq!(name("C:\\games\\twic.pgn"), "twic.pgn");
        assert_eq!(name("../../x.pgn"), "x.pgn");
        assert_eq!(name(""), "upload.pgn");
        assert_eq!(upload_name(&UploadQuery::default()), "upload.pgn");
    }
}
