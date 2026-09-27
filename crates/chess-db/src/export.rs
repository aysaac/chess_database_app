//! PGN export. Writes to a temporary file and renames it into place, so a
//! failed or cancelled export never leaves a half-written file behind.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chess_core::pgn::write_game;

use crate::{Database, DbError, GameId, Progress};

const CHUNK: usize = 500;

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

/// Exports `ids` (or all non-deleted games when `None`). Returns the number
/// of games written.
pub fn export_pgn(
    db: &Mutex<Database>,
    ids: Option<Vec<GameId>>,
    path: &Path,
    progress: &dyn Progress,
) -> Result<u64, DbError> {
    let ids = match ids {
        Some(ids) => ids,
        None => {
            let db = db.lock().expect("db lock");
            let count = db.count();
            let mut ids = Vec::with_capacity(count as usize);
            let mut start = 0;
            while start < count {
                let recs = db.records(start, 64 * 1024)?;
                ids.extend(recs.iter().zip(start..).filter(|(r, _)| !r.is_deleted()).map(|(_, id)| id));
                start += 64 * 1024;
            }
            ids
        }
    };

    let tmp = temp_path(path);
    let result = (|| {
        let mut out = BufWriter::new(File::create(&tmp)?);
        let total = ids.len() as u64;
        let mut written = 0u64;
        for chunk in ids.chunks(CHUNK) {
            progress.check_cancelled()?;
            let games = {
                let db = db.lock().expect("db lock");
                chunk.iter().map(|&id| db.read_game(id)).collect::<Result<Vec<_>, _>>()?
            };
            for g in &games {
                out.write_all(write_game(g).as_bytes())?;
            }
            written += games.len() as u64;
            progress.report(written, total);
        }
        let file = out.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        Ok(written)
    })();

    match result {
        Ok(written) => {
            std::fs::rename(&tmp, path)?;
            Ok(written)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}
