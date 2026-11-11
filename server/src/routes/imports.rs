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
    Checkpoint, ImportResume, ImportSummary, Progress, import_cb_resumable, import_pgn_resumable,
    update_position_index,
};
use chess_jobs::{JobContext, JobError};
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::access::require;
use crate::audit::Target;
use crate::auth::{AdminAuth, Auth};
use crate::error::{ApiError, ApiResult};
use crate::jobs::{thousands, to_job_error};
use crate::pool::{OpenDb, dir_size, writer_busy};
use crate::secret;
use crate::store::{DbMeta, JobRow};
use crate::wire::{Access, ImportFileRequest, ImportKind, ImportReportDto, JobStarted, UploadQuery};
use crate::{AppState, blocking};

/// How much more the database may grow: the least of its own quota and its
/// owner's (less their other databases). None when neither has one.
/// Blocking.
fn room(state: &AppState, meta: &DbMeta) -> ApiResult<Option<u64>> {
    let size = dir_size(&state.pool.path(&meta.id));
    let mut room = meta.quota_bytes.map(|q| q.saturating_sub(size));
    let owner = state.store.user(meta.owner_id)?.ok_or_else(ApiError::not_found)?;
    if let Some(q) = owner.quota_bytes {
        let used: u64 =
            state.store.databases_owned_by(owner.id)?.iter().map(|d| state.pool.stats(&d.id).1).sum();
        let left = q.saturating_sub(used);
        room = Some(room.map_or(left, |r| r.min(left)));
    }
    Ok(room)
}

/// Passes progress to the job, and stops the import once the database
/// outgrows its quota (checked every half second).
struct Guarded<'a> {
    ctx: &'a JobContext,
    dir: PathBuf,
    /// The size the database may reach.
    limit: Option<u64>,
    over: AtomicBool,
    checked: Mutex<Instant>,
}

impl Progress for Guarded<'_> {
    fn report(&self, done: u64, total: u64) {
        self.ctx.progress(done, total);
        let Some(limit) = self.limit else { return };
        let mut checked = self.checked.lock().expect("quota check lock");
        if checked.elapsed() >= Duration::from_millis(500) {
            *checked = Instant::now();
            if dir_size(&self.dir) > limit {
                self.over.store(true, Ordering::Relaxed);
            }
        }
    }

    fn is_cancelled(&self) -> bool {
        self.ctx.is_cancelled() || self.over.load(Ordering::Relaxed)
    }
}

/// Deletes an uploaded file when the import is done with it, unless the
/// server is stopping: then the import goes on with it after a restart.
pub(crate) struct TempFile {
    path: Option<PathBuf>,
    stopping: Arc<AtomicBool>,
}

impl TempFile {
    /// A file of `tmp/` the import deletes once it ends (as an upload).
    pub(crate) fn upload(state: &AppState, path: PathBuf) -> TempFile {
        TempFile { path: Some(path), stopping: state.jobs.stopping() }
    }

    fn none(state: &AppState) -> TempFile {
        TempFile { path: None, stopping: state.jobs.stopping() }
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if let Some(path) = &self.path
            && !self.stopping.load(Ordering::Acquire)
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// What the database and the file are at, before a commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Mark {
    games: u32,
    read: u64,
}

/// How an import goes on after the server stopped in it (kept as JSON with
/// its job): the last two checkpoints noted before a commit, and the
/// database's game count tell where it left off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Resume {
    kind: ImportKind,
    /// The file: an upload in `tmp/`, or one in the import folder.
    path: PathBuf,
    /// What the job and its report call it.
    file: String,
    upload: bool,
    /// The database's games when the import began.
    base: u32,
    prev: Option<Mark>,
    next: Option<Mark>,
    /// The file is read: only the position index is left.
    #[serde(default)]
    read_all: bool,
}

impl Resume {
    /// Games of the file to pass over when the database has `count` games,
    /// if that's a state the import was in.
    fn skip(&self, count: u32) -> Option<u64> {
        match (self.prev, self.next) {
            (_, Some(n)) if n.games == count => Some(n.read),
            (Some(p), _) if p.games == count => Some(p.read),
            (None, None) if count == self.base => Some(0),
            _ => None,
        }
    }
}

struct Import {
    meta: DbMeta,
    open: Arc<OpenDb>,
    resume: Mutex<Resume>,
    /// Games of the file an earlier run went through.
    skip: u64,
    /// The upload to delete afterwards.
    _temp: TempFile,
    limit: Option<u64>,
}

impl Import {
    fn note(&self, state: &AppState, job: u64, change: impl FnOnce(&mut Resume)) {
        let mut resume = self.resume.lock().expect("resume lock");
        change(&mut resume);
        state.jobs.set_resume(job, &serde_json::to_string(&*resume).unwrap_or_default());
    }
}

fn run(state: &AppState, import: &Import, ctx: &JobContext) -> Result<String, JobError> {
    let guard = Guarded {
        ctx,
        dir: state.pool.path(&import.meta.id),
        limit: import.limit,
        over: AtomicBool::new(false),
        // The first report checks at once: a small file is one batch.
        checked: Mutex::new(Instant::now().checked_sub(Duration::from_secs(1)).unwrap_or_else(Instant::now)),
    };
    let (kind, path, file, read_all, base) = {
        let r = import.resume.lock().expect("resume lock");
        (r.kind, r.path.clone(), r.file.clone(), r.read_all, r.base)
    };
    import.note(state, ctx.id(), |_| {});
    let checkpoint = |c: Checkpoint| {
        import.note(state, ctx.id(), |r| {
            r.prev = r.next;
            r.next = Some(Mark { games: c.games, read: c.read });
        });
    };
    let resume = ImportResume { skip: import.skip, before_commit: Some(&checkpoint) };
    let mut summary: ImportSummary = if read_all {
        ImportSummary::default()
    } else {
        match kind {
            ImportKind::Pgn => import_pgn_resumable(&import.open.db, &path, resume, &guard),
            ImportKind::Cbh => import_cb_resumable(&import.open.db, &path, resume, &guard),
        }
        .map_err(to_job_error)?
    };
    // Counting what earlier runs imported too.
    summary.imported = u64::from(import.open.lock().count().saturating_sub(base));
    state.jobs.set_report(ctx.id(), ImportReportDto::new(file.clone(), &summary));
    let over = guard.over.load(Ordering::Relaxed);
    log::info!(
        "imported {} games from {} into {} ({} skipped{})",
        summary.imported,
        file,
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
    if !summary.cancelled {
        import.note(state, ctx.id(), |r| r.read_all = true);
    }
    ctx.progress_msg(0, 0, "Indexing positions");
    let indexed = update_position_index(&import.open.db, ctx).map_err(to_job_error)?;
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
        return Err(JobError::Failed(format!("{msg}, then stopped: the database reached its quota")));
    }
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
pub(crate) fn start(
    state: &Arc<AppState>,
    auth: &Auth,
    meta: DbMeta,
    kind: ImportKind,
    path: PathBuf,
    file: String,
    temp: TempFile,
) -> ApiResult<u64> {
    let limit_left = room(state, &meta)?;
    if limit_left == Some(0) {
        return Err(quota_exceeded());
    }
    let open = state.open_db(&meta.id)?;
    let size = dir_size(&state.pool.path(&meta.id));
    let mut writer = open.writer.lock().expect("writer lock");
    if writer_busy(&writer, &state.jobs.manager) {
        return Err(busy());
    }
    let label = format!("Importing {file} into {}", meta.name);
    let upload = temp.path.is_some();
    let resume = Resume {
        kind,
        path,
        file,
        upload,
        base: open.lock().count(),
        prev: None,
        next: None,
        read_all: false,
    };
    let import = Import {
        limit: limit_left.map(|l| size + l),
        open: open.clone(),
        meta,
        resume: Mutex::new(resume),
        skip: 0,
        _temp: temp,
    };
    let st = state.clone();
    let db = (import.meta.id.clone(), import.meta.name.clone());
    let job =
        state.jobs.spawn("import", label, (&db.0, &db.1), &auth.user, move |ctx| run(&st, &import, ctx));
    *writer = Some(job);
    let detail = match (upload, kind) {
        (true, _) => "an upload",
        (false, ImportKind::Pgn) => "a PGN file of the import folder",
        (false, ImportKind::Cbh) => "a ChessBase database of the import folder",
    };
    if let Some(meta) = state.store.database(&db.0)? {
        state.audit(
            "import.start",
            Some(&auth.user),
            Target::Database(&meta),
            Some(detail.to_owned()),
            Some(&auth.client),
        );
    }
    Ok(job)
}

/// At start: imports the server stopped in go on from what they committed
/// (their database's game count says which of their last two checkpoints
/// made it); the other jobs it stopped in are marked interrupted. Uploads
/// in `tmp/` that no import goes on with are deleted.
pub fn resume_interrupted(state: &Arc<AppState>) -> std::io::Result<()> {
    let mut keep = std::collections::HashSet::new();
    for row in state.jobs.interrupted() {
        let resume: Option<Resume> = row.resume.as_deref().and_then(|r| serde_json::from_str(r).ok());
        match go_on(state, &row, resume.clone()) {
            Ok(()) => {
                if let Some(r) = resume {
                    keep.insert(r.path);
                }
            }
            Err(why) => {
                log::info!("job {} ({}) was interrupted: {why}", row.id, row.kind);
                state.jobs.mark_interrupted(&row, &why);
            }
        }
    }
    let tmp = state.config.tmp_dir();
    for entry in std::fs::read_dir(&tmp)? {
        let path = entry?.path();
        if !keep.contains(&path) {
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
        }
    }
    Ok(())
}

fn go_on(state: &Arc<AppState>, row: &JobRow, resume: Option<Resume>) -> Result<(), String> {
    const STOPPED: &str = "The server stopped while it ran";
    let (Some(resume), true) = (resume, row.kind == "import") else { return Err(STOPPED.into()) };
    let gone = || format!("{STOPPED}, and its database or file is gone");
    let meta =
        state.store.database(row.db.as_deref().ok_or_else(gone)?).map_err(|e| e.message)?.ok_or_else(gone)?;
    let user = state.store.user(row.user.ok_or_else(gone)?).map_err(|e| e.message)?.ok_or_else(gone)?;
    if !resume.path.is_file() {
        return Err(gone());
    }
    let open = state.pool.get(&meta.id).map_err(|e| e.to_string())?;
    let count = open.lock().count();
    let skip = resume.skip(count).ok_or_else(|| format!("{STOPPED}, and the database changed since"))?;
    let limit_left = room(state, &meta).map_err(|e| e.message)?;
    let size = dir_size(&state.pool.path(&meta.id));
    let temp = TempFile { path: resume.upload.then(|| resume.path.clone()), stopping: state.jobs.stopping() };
    log::info!(
        "going on with job {} for {}: importing {} into {} from game {} of the file",
        row.id,
        user.username,
        resume.file,
        meta.id,
        skip + 1
    );
    let import = Import {
        limit: limit_left.map(|l| size + l),
        open: open.clone(),
        meta,
        resume: Mutex::new(resume),
        skip,
        _temp: temp,
    };
    let st = state.clone();
    let mut writer = open.writer.lock().expect("writer lock");
    let job = state.jobs.spawn_kept(row, move |ctx| run(&st, &import, ctx));
    *writer = Some(job);
    Ok(())
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
    // or into a database that can't take it now (`start` checks again).
    let st = state.clone();
    let (user, db) = (auth.user.clone(), id.clone());
    let meta = blocking(move || {
        let (meta, _) = require(&st.store, &user, &db, Access::Write)?;
        if room(&st, &meta)? == Some(0) {
            return Err(quota_exceeded());
        }
        if let Some(open) = st.pool.get_open(&meta.id)
            && writer_busy(&open.writer.lock().expect("writer lock"), &st.jobs.manager)
        {
            return Err(busy());
        }
        Ok(meta)
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
    let temp = TempFile { path: Some(path.clone()), stopping: state.jobs.stopping() };
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
    let job = blocking(move || start(&state, &auth, meta, ImportKind::Pgn, path, name, temp)).await?;
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
        let temp = TempFile::none(&state);
        start(&state, &auth, meta, kind, path, req.path.clone(), temp)
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
