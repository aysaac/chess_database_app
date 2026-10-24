//! Reading chosen extra tags of every game, with its list row: what a
//! database already holds of a player's online games (NET-01), so
//! downloading them again only adds the new ones.

use std::sync::Mutex;

use crate::blob::extra_tags;
use crate::positions::for_each_game;
use crate::{Database, DbError, GameId, GameRow, Progress};

/// One game as [`scan_tags`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedGame {
    pub row: GameRow,
    /// The values of the tags asked for, in that order.
    pub tags: Vec<Option<String>>,
}

/// Gives `each` every game's row (deleted games too) and the values of the
/// extra tags `names`, in id order. Games whose blob can't be decoded are
/// left out (and logged); failing to read the file ends the scan.
pub fn scan_tags(
    db: &Mutex<Database>,
    names: &[&str],
    progress: &dyn Progress,
    each: &mut dyn FnMut(TaggedGame),
) -> Result<(), DbError> {
    let ids: Vec<GameId> = (0..db.lock().expect("db lock").count()).collect();
    let work = |id: GameId, rec: &crate::record::IndexRecord, blob: &[u8]| {
        extra_tags(blob, names)
            .inspect_err(|e| log::warn!("tags: skipping game {id}: {e}"))
            .ok()
            .map(|tags| (id, *rec, tags))
    };
    for_each_game(db, &ids, progress, &work, &mut |batch| {
        let db = db.lock().expect("db lock");
        for (id, rec, tags) in batch.into_iter().flatten() {
            each(TaggedGame { row: db.row_from_record(id, &rec), tags });
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoProgress;
    use chess_core::pgn::read_pgn_str;

    #[test]
    fn reads_rows_and_tags_of_every_game() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Database::create(&dir.path().join("Tags")).unwrap();
        let pgn = "[White \"Hikaru\"]\n[Site \"Chess.com\"]\n[Link \"https://www.chess.com/game/live/7\"]\n\n1. e4 *\n\n\
                   [White \"DrNykterstein\"]\n[Site \"https://lichess.org/abcdefgh\"]\n\n1. d4 *\n";
        for g in read_pgn_str(pgn) {
            db.append(&g).unwrap();
        }
        db.set_deleted(1, true).unwrap();
        let db = Mutex::new(db);
        let mut seen = Vec::new();
        scan_tags(&db, &["Link"], &NoProgress, &mut |g| {
            seen.push((g.row.id, g.row.white, g.row.site, g.tags))
        })
        .unwrap();
        assert_eq!(
            seen,
            [
                (
                    0,
                    "Hikaru".into(),
                    "Chess.com".into(),
                    vec![Some("https://www.chess.com/game/live/7".into())]
                ),
                (1, "DrNykterstein".into(), "https://lichess.org/abcdefgh".into(), vec![None]),
            ]
        );
    }
}
