//! Imports a classic ChessBase database (`.cbh` + companions) into our own
//! format, through the same `PreparedGame`/batching path PGN import uses.
//! See `docs/io-03-cbh.md`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chess_core::game::{GameResult, Headers};

use crate::cb::annotations::{self, CbaFile};
use crate::cb::entities::{self, EntityTable};
use crate::cb::header::{CbEntry, CbResult, CbhFile, GameRecord};
use crate::cb::moves::{CbgFile, DecodedGame, TranslateTable, decode_record};
use crate::import::{ImportSummary, ImportWarning, game_label};
use crate::{Database, DbError, GameId, PreparedGame, Progress};

const MAX_BATCH: usize = 2_000;
const MAX_BATCH_AGE: Duration = Duration::from_millis(300);
const MAX_WARNINGS: usize = 200;

/// The files import needs (`.cba`, annotations, is optional).
const REQUIRED: [&str; 5] = ["cbh", "cbg", "cbp", "cbt", "cbc"];

/// The database's file with extension `ext`, next to `cbh` and in the same
/// case as its extension (`.CBH` goes with `.CBG`).
fn sibling(cbh: &Path, ext: &str) -> PathBuf {
    let upper = cbh.extension().is_some_and(|e| e.to_string_lossy().chars().any(|c| c.is_ascii_uppercase()));
    cbh.with_extension(if upper { ext.to_ascii_uppercase() } else { ext.to_owned() })
}

struct Entities {
    players: EntityTable,
    tournaments: EntityTable,
    annotators: EntityTable,
}

fn build_headers(g: &GameRecord, e: &Entities) -> Headers {
    let mut h = Headers::default();
    if let Ok(data) = e.players.data(g.white) {
        h.white = entities::player_name(&data);
    }
    if let Ok(data) = e.players.data(g.black) {
        h.black = entities::player_name(&data);
    }
    if let Ok(data) = e.tournaments.data(g.tournament) {
        h.event = entities::tournament_title(&data);
        h.site = entities::tournament_place(&data);
    }
    if let Ok(data) = e.annotators.data(g.annotator) {
        h.annotator = entities::annotator_name(&data);
    }
    h.date = g.date_pgn.clone();
    h.eco = g.eco.pgn_str();
    h.round = match (g.round, g.subround) {
        (0, _) => String::new(),
        (r, 0) => r.to_string(),
        (r, s) => format!("{r}.{s}"),
    };
    h.white_elo = (g.white_elo != 0).then_some(g.white_elo);
    h.black_elo = (g.black_elo != 0).then_some(g.black_elo);
    h.result = match g.result {
        Some(CbResult::WhiteWins | CbResult::WhiteForfeit) => GameResult::WhiteWins,
        Some(CbResult::BlackWins | CbResult::BlackForfeit) => GameResult::BlackWins,
        Some(CbResult::Draw | CbResult::DrawForfeit) => GameResult::Draw,
        _ => GameResult::Unknown,
    };
    h
}

/// Reads game `id` (1-based) into a `chess_core::game::Game`, with the
/// problems that cut some of its lines short, or `None` for a guiding text,
/// a deleted record, or past the end — the caller doesn't count those at
/// all. `Err` means the game can't be read (unsupported encoding, a bad
/// start position, …): it's skipped, with a warning.
fn read_game(
    id: GameId,
    cbh: &CbhFile,
    cbg: &CbgFile,
    cba: Option<&CbaFile>,
    entities: &Entities,
    table: &TranslateTable,
) -> Result<Option<(chess_core::game::Game, Vec<String>)>, DbError> {
    let g = match cbh.read(id)? {
        CbEntry::Game(g) if !g.deleted => g,
        _ => return Ok(None),
    };
    let record = cbg.read_record(g.moves_offset)?;
    let DecodedGame { mut game, order, mut problems } = decode_record(&record, table)?;
    game.headers = build_headers(&g, entities);
    if g.annotations_offset != 0
        && let Some(cba) = cba
    {
        // Bad annotations lose the rest of the annotations, not the game.
        let applied = cba
            .read_record(g.annotations_offset)
            .and_then(|ann_record| annotations::apply_annotations(&mut game, &order, &ann_record));
        if let Err(e) = applied {
            problems.push(format!("Some of the game's annotations couldn't be read ({e})."));
        }
    }
    Ok(Some((game, problems)))
}

/// Imports every game of the classic ChessBase database whose `.cbh` file
/// is `cbh_path` into `db`; its other files are found next to it.
pub fn import_cb(
    db: &Mutex<Database>,
    cbh_path: &Path,
    progress: &dyn Progress,
) -> Result<ImportSummary, DbError> {
    import_cb_with(db, cbh_path, &TranslateTable::mode0(), progress)
}

fn import_cb_with(
    db: &Mutex<Database>,
    cbh_path: &Path,
    table: &TranslateTable,
    progress: &dyn Progress,
) -> Result<ImportSummary, DbError> {
    for ext in REQUIRED {
        let path = sibling(cbh_path, ext);
        if !path.is_file() {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            return Err(DbError::Invalid(format!(
                "{name} is missing. A ChessBase database needs its .cbh, .cbg, .cbp, .cbt and .cbc files \
                 in one folder: copy them there and import again."
            )));
        }
    }
    let cbh = CbhFile::open(cbh_path)?;
    let cbg = CbgFile::open(&sibling(cbh_path, "cbg"))?;
    let cba = CbaFile::open(&sibling(cbh_path, "cba")).ok();
    let entities = Entities {
        players: EntityTable::open(&sibling(cbh_path, "cbp"), 58)?,
        tournaments: EntityTable::open(&sibling(cbh_path, "cbt"), 90)?,
        annotators: EntityTable::open(&sibling(cbh_path, "cbc"), 53)?,
    };

    let total = u64::from(cbh.game_count);
    let mut summary = ImportSummary::default();
    let mut batch: Vec<PreparedGame> = Vec::with_capacity(MAX_BATCH);
    let mut batch_started = Instant::now();

    // Warnings of games in `batch`: (position in the batch, warning index),
    // given their game id once the batch is committed.
    let mut unplaced: Vec<(usize, usize)> = Vec::new();

    let commit = |batch: &mut Vec<PreparedGame>,
                  unplaced: &mut Vec<(usize, usize)>,
                  summary: &mut ImportSummary|
     -> Result<(), DbError> {
        if !batch.is_empty() {
            let ids = db.lock().expect("db lock").append_prepared(batch)?;
            for (pos, i) in unplaced.drain(..) {
                summary.warnings[i].game = Some(ids.start + pos as GameId);
            }
            summary.imported += batch.len() as u64;
            batch.clear();
        }
        Ok(())
    };

    for id in 1..=cbh.game_count {
        // Like PGN import's, a warning's index is zero-based.
        let game_index = id as usize - 1;
        match read_game(id, &cbh, &cbg, cba.as_ref(), &entities, table) {
            Ok(Some((game, problems))) => {
                summary.warning_count += problems.len() as u64;
                let label = if problems.is_empty() { String::new() } else { game_label(&game.headers) };
                for message in problems {
                    if summary.warnings.len() < MAX_WARNINGS {
                        unplaced.push((batch.len(), summary.warnings.len()));
                        summary.warnings.push(ImportWarning {
                            game_index,
                            game: None,
                            label: label.clone(),
                            message,
                        });
                    }
                }
                batch.push(PreparedGame::new(&game));
            }
            Ok(None) => {}
            Err(e) => {
                summary.skipped += 1;
                summary.warning_count += 1;
                if summary.warnings.len() < MAX_WARNINGS {
                    let label = match cbh.read(id) {
                        Ok(CbEntry::Game(g)) => game_label(&build_headers(&g, &entities)),
                        _ => String::new(),
                    };
                    summary.warnings.push(ImportWarning {
                        game_index,
                        game: None,
                        label,
                        message: e.to_string(),
                    });
                }
            }
        }

        if batch.len() >= MAX_BATCH || batch_started.elapsed() >= MAX_BATCH_AGE {
            commit(&mut batch, &mut unplaced, &mut summary)?;
            batch_started = Instant::now();
            progress.report(u64::from(id), total);
            if progress.is_cancelled() {
                summary.cancelled = true;
                return Ok(summary);
            }
        }
    }
    commit(&mut batch, &mut unplaced, &mut summary)?;
    progress.report(total, total);
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoProgress;

    fn entity_table(data_size: u32, records: &[Vec<u8>]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(records.len() as u32).to_le_bytes()); // record count
        buf.extend_from_slice(&0u32.to_le_bytes()); // tree root
        buf.extend_from_slice(&1_234_567_890u32.to_le_bytes());
        buf.extend_from_slice(&data_size.to_le_bytes());
        buf.extend_from_slice(&(-1i32).to_le_bytes()); // first deleted
        buf.extend_from_slice(&(records.len() as u32).to_le_bytes()); // entity count
        buf.extend_from_slice(&4u32.to_le_bytes()); // extra header bytes
        buf.extend_from_slice(&[0u8; 4]);
        for record in records {
            buf.extend_from_slice(&[0u8; 9]); // tree node, unused by our reader
            assert_eq!(record.len(), data_size as usize);
            buf.extend_from_slice(record);
        }
        buf
    }

    fn fixed(s: &str, len: usize) -> Vec<u8> {
        let mut v = s.as_bytes().to_vec();
        v.push(0);
        v.resize(len, 0);
        v
    }

    /// A single player/tournament/annotator record's `data_size` bytes:
    /// `fields` first, zero-padded to `data_size - 8`, then the trailing
    /// reference-count + first-game ints every entity type ends with.
    fn entity_data(fields: &[u8], data_size: u32) -> Vec<u8> {
        let mut v = fields.to_vec();
        v.resize(data_size as usize - 8, 0);
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v
    }

    #[test]
    fn imports_one_synthetic_game_with_headers() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("Test.cbh");

        // `.cbg`: a 26-byte header, then one record holding "1.e4" (mode 0,
        // identity table): opcode 128 (e-pawn piece_no 4, two steps), then
        // the end-of-game marker — both bytes carry the running modifier,
        // same as `decode_mode0`'s own doc comment explains.
        let mut cbg = vec![0u8; 26];
        cbg[0..2].copy_from_slice(&26u16.to_be_bytes());
        let moves = [128u8, 0u8]; // 0 = (255 + 1) mod 256, modifier after one move
        let mut record = vec![0x00u8];
        record.extend_from_slice(&((4 + moves.len()) as u32).to_be_bytes()[1..]); // be24
        record.extend_from_slice(&moves);
        cbg.extend_from_slice(&record);
        std::fs::write(sibling(&base, "cbg"), &cbg).unwrap();

        // `.cbh`: a 46-byte header, then one game record pointing at the
        // `.cbg` record right after its header (offset 26).
        let mut cbh = vec![0u8; 46];
        cbh[0x03..0x05].copy_from_slice(&46u16.to_be_bytes());
        cbh[0x05] = 1;
        let mut rec = vec![0u8; 46];
        rec[0] = 0x01; // game, not deleted, not a guiding text
        rec[0x01..0x05].copy_from_slice(&26u32.to_be_bytes()); // moves offset
        rec[0x1b] = 2; // result: white wins
        cbh.extend_from_slice(&rec);
        std::fs::write(&base, &cbh).unwrap();

        std::fs::write(
            sibling(&base, "cbp"),
            entity_table(58, &[entity_data(&[&fixed("Doe", 30)[..], &fixed("Jane", 20)[..]].concat(), 58)]),
        )
        .unwrap();
        std::fs::write(
            sibling(&base, "cbt"),
            entity_table(
                90,
                &[entity_data(&[&fixed("Test Open", 40)[..], &fixed("Testville", 30)[..]].concat(), 90)],
            ),
        )
        .unwrap();
        std::fs::write(sibling(&base, "cbc"), entity_table(53, &[entity_data(&[], 53)])).unwrap();

        let db = Mutex::new(Database::create(&dir.path().join("Imported")).unwrap());
        let summary = import_cb_with(&db, &base, &TranslateTable::identity(), &NoProgress).unwrap();
        assert_eq!(summary.imported, 1);
        assert_eq!(summary.skipped, 0);

        let db = db.lock().unwrap();
        let game = db.read_game(0).unwrap();
        assert_eq!(game.headers.white, "Doe, Jane");
        assert_eq!(game.headers.event, "Test Open");
        assert_eq!(game.headers.site, "Testville");
        assert_eq!(game.headers.result, GameResult::WhiteWins);
        assert_eq!(game.mainline_len(), 1);
    }
}
