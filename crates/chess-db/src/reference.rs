//! The Reference tab (OPEN-14): what is played in a position, how often
//! lately, by whom and in which strong games, plus the main line and the
//! most played lines from it.
//!
//! [`reference_overview`] answers the first four from the position index
//! (SRCH-04) and the per-game stats kept in memory ([`GameStat`]); it reads
//! records only for the names and games it lists, so it's fast at any
//! position. [`reference_lines`] reads the next moves of every game that
//! reaches the position (the index only knows each game's next move), so
//! for common positions it runs as a job.

use std::cmp::Reverse;
use std::ops::Range;
use std::sync::Mutex;

use chess_core::shakmaty::{Chess, Move, Position};

use crate::blob::{mainline, mainline_window, move_at};
use crate::positions::{NO_MOVE, PositionHits, TreeStats, find_position, for_each_game, ranked_moves};
use crate::record::IndexRecord;
use crate::{Database, DbError, GameId, GameRow, GameStat, NameKind, Progress};

/// Hot: the years counted as recent, back from the newest year in the
/// database (3: 2016–2018 when the newest game is from 2018).
pub const RECENT_YEARS: u16 = 3;
/// Hot: ↑ needs this many recent games with the move; ↓ needs this many
/// expected at the move's share over all years.
pub const HOT_MIN_GAMES: u32 = 5;
/// Best players listed per move.
pub const BEST_PLAYERS: usize = 4;
/// Top games listed.
pub const TOP_GAMES: usize = 100;
/// The main line: at most this many plies…
pub const MAIN_LINE_PLIES: usize = 10;
/// …each played in at least this many games, and in a tenth of the
/// position's games.
pub const MAIN_LINE_MIN_GAMES: u32 = 20;
/// Lines: at most this many plies…
pub const LINE_PLIES: usize = 6;
/// …extended past the first move while at least this many games follow.
pub const LINE_MIN_GAMES: u32 = 5;
/// Lines listed at most, and per first move.
pub const MAX_LINES: usize = 12;
pub const LINES_PER_MOVE: usize = 3;
/// From about this many games, counting the lines of a position takes long
/// enough (it reads every game) to run as a job with progress and Cancel.
pub const LINES_JOB_GAMES: usize = 50_000;

/// Records read at most per move to find its best players, for moves whose
/// strongest games are nearly all by the same few players.
const BEST_SCAN: usize = 4096;
/// Candidates sorted at a time while looking for the best players.
const BEST_CHUNK: usize = 64;

/// Whether a move is played more (↑) or less (↓) lately than over all years.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trend {
    Up,
    Down,
}

/// How much a move is played lately (the Hot column).
#[derive(Debug, Clone, PartialEq)]
pub struct Hot {
    /// Games from the recent years with this move.
    pub recent: u32,
    /// The move's share of the position's recent games (the bar), 0…1.
    pub share: f64,
    /// Its share of the position's games over all years, 0…1.
    pub all_share: f64,
    pub trend: Option<Trend>,
}

/// One of the strongest players who chose a move, at their Elo in the game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BestPlayer {
    pub name: String,
    pub elo: u16,
    /// The game they played it in at that Elo.
    pub game: GameId,
}

/// A row of the Reference tab's moves table.
#[derive(Debug, Clone)]
pub struct ReferenceMove {
    /// `None`: games that ended in the position.
    pub mv: Option<Move>,
    pub stats: TreeStats,
    /// `None` for games that ended here, or when no game of the position is
    /// from the recent years.
    pub hot: Option<Hot>,
    /// Up to [`BEST_PLAYERS`] different players, strongest first.
    pub best: Vec<BestPlayer>,
}

/// A game reaching the position, and the ply where it first does.
#[derive(Debug, Clone)]
pub struct TopGame {
    pub row: GameRow,
    pub ply: u16,
}

/// The fast part of the Reference tab for a position.
#[derive(Debug, Clone, Default)]
pub struct ReferenceOverview {
    pub total: TreeStats,
    /// In the opening tree's order: most games first, then most recent.
    pub moves: Vec<ReferenceMove>,
    /// The years Hot counts as recent (first, last); `None` when no game in
    /// the database has a year.
    pub recent_years: Option<(u16, u16)>,
    /// Games reaching the position from those years.
    pub recent_games: u32,
    /// The strongest games (average Elo of both players, see [`top_key`]),
    /// at most [`TOP_GAMES`].
    pub top_games: Vec<TopGame>,
    /// Some games couldn't be checked (see [`PositionHits::partial`]).
    pub partial: bool,
    pub max_ply: u16,
}

/// The years Hot counts as recent: the last [`RECENT_YEARS`] up to the
/// newest year of a game in the database (deleted games left out).
fn recent_years(stats: &[GameStat]) -> Option<(u16, u16)> {
    let newest = stats.iter().filter(|g| !g.deleted).map(|g| g.year).max().filter(|&y| y > 0)?;
    Some((newest.saturating_sub(RECENT_YEARS - 1), newest))
}

/// Hot for a move with `games` games, `recent` of them recent, in a
/// position with `total` games, `recent_total` of them recent. The shares
/// are compared exactly, in integers.
fn hot(games: u32, recent: u32, total: u32, recent_total: u32) -> Option<Hot> {
    if recent_total == 0 || total == 0 {
        return None;
    }
    let (g, r, t, rt) = (u64::from(games), u64::from(recent), u64::from(total), u64::from(recent_total));
    let min = u64::from(HOT_MIN_GAMES);
    // ↑: share ≥ 1.25 × all_share, i.e. r/rt ≥ 5/4 · g/t; ↓: share ≤ 0.8 ×
    // all_share, when the move would have had `min` recent games at its old
    // share (g/t · rt), so a rare move isn't marked as falling.
    let trend = if r >= min && 4 * r * t >= 5 * g * rt {
        Some(Trend::Up)
    } else if g * rt >= min * t && 5 * r * t <= 4 * g * rt {
        Some(Trend::Down)
    } else {
        None
    };
    Some(Hot { recent, share: r as f64 / rt as f64, all_share: g as f64 / t as f64, trend })
}

/// Orders best-player candidates, highest first: Elo, then the newest
/// year, then the newest game.
fn best_key(elo: u16, year: u16, game: GameId) -> u64 {
    (u64::from(elo) << 48) | (u64::from(year) << 32) | u64::from(game)
}

/// Orders top games, highest first: the average Elo of both players (one
/// Elo when only one is known; games with neither come last), then the
/// newest year (unknown last), then the newest game. Carries the ply in its
/// low bits.
fn top_key(g: &GameStat, game: GameId, ply: u16) -> u128 {
    // Twice the average, so it stays whole.
    let strength = match (u32::from(g.white_elo), u32::from(g.black_elo)) {
        (0, 0) => 0,
        (w, 0) => 2 * w,
        (0, b) => 2 * b,
        (w, b) => w + b,
    };
    (u128::from(strength) << 64) | (u128::from(g.year) << 48) | (u128::from(game) << 16) | u128::from(ply)
}

/// The fast part of the Reference tab for `pos`: the moves with Hot and
/// Best players, and the top games. Deleted games don't count.
pub fn reference_overview(db: &Mutex<Database>, pos: &Chess) -> Result<ReferenceOverview, DbError> {
    let hits = find_position(db, pos)?;
    let white = pos.turn().is_white();
    let mut guard = db.lock().expect("db lock");
    guard.game_stats()?;
    let db: &Database = &guard;
    let stats = db.loaded_stats();
    let recent = recent_years(stats);
    let is_recent = |year: u16| recent.is_some_and(|(from, to)| (from..=to).contains(&year));

    let mut total = TreeStats::default();
    let mut by_move: [TreeStats; 256] = std::array::from_fn(|_| TreeStats::default());
    let mut recent_by_move = [0u32; 256];
    let mut recent_games = 0u32;
    let mut candidates: Vec<Vec<u64>> = vec![Vec::new(); 256];
    let mut top: Vec<u128> = Vec::with_capacity(hits.entries.len());
    for e in &hits.entries {
        let Some(g) = stats.get(e.game as usize) else { continue };
        if g.deleted {
            continue;
        }
        let next = usize::from(e.next);
        total.add(g, white);
        by_move[next].add(g, white);
        if is_recent(g.year) {
            recent_games += 1;
            recent_by_move[next] += 1;
        }
        let own = if white { g.white_elo } else { g.black_elo };
        if own != 0 && e.next != NO_MOVE {
            candidates[next].push(best_key(own, g.year, e.game));
        }
        top.push(top_key(g, e.game, e.ply));
    }

    let mut moves = Vec::new();
    for (next, mv) in ranked_moves(pos, &by_move) {
        let stats = by_move[next].clone();
        let (hot, best) = match mv {
            Some(_) => (
                hot(stats.games, recent_by_move[next], total.games, recent_games),
                best_players(db, std::mem::take(&mut candidates[next]), white)?,
            ),
            None => (None, Vec::new()),
        };
        moves.push(ReferenceMove { mv, stats, hot, best });
    }
    let top_games = top_games(db, top)?;
    Ok(ReferenceOverview {
        total,
        moves,
        recent_years: recent,
        recent_games,
        top_games,
        partial: hits.partial,
        max_ply: hits.max_ply,
    })
}

/// Up to [`BEST_PLAYERS`] different players among `candidates` (see
/// [`best_key`]), strongest first. Only the strongest candidates are sorted
/// and have their record read, a chunk at a time, until there are enough
/// players.
fn best_players(db: &Database, mut candidates: Vec<u64>, white: bool) -> Result<Vec<BestPlayer>, DbError> {
    let names = db.names(NameKind::Player);
    let mut out: Vec<BestPlayer> = Vec::new();
    let mut seen: Vec<u32> = Vec::new();
    let mut start = 0;
    while out.len() < BEST_PLAYERS && start < candidates.len().min(BEST_SCAN) {
        let rest = &mut candidates[start..];
        let chunk = rest.len().min(BEST_CHUNK);
        if rest.len() > chunk {
            rest.select_nth_unstable_by(chunk - 1, |a, b| b.cmp(a));
        }
        let head = &mut rest[..chunk];
        head.sort_unstable_by(|a, b| b.cmp(a));
        for &key in head.iter() {
            let game = key as GameId;
            let rec = db.record(game)?;
            let id = if white { rec.white } else { rec.black };
            let name = names.get(id);
            if id == 0 || name.trim().is_empty() || seen.contains(&id) {
                continue;
            }
            seen.push(id);
            out.push(BestPlayer { name: name.to_owned(), elo: (key >> 48) as u16, game });
            if out.len() == BEST_PLAYERS {
                break;
            }
        }
        start += chunk;
    }
    Ok(out)
}

/// The first [`TOP_GAMES`] of `keys` (see [`top_key`]), read.
fn top_games(db: &Database, mut keys: Vec<u128>) -> Result<Vec<TopGame>, DbError> {
    if keys.len() > TOP_GAMES {
        keys.select_nth_unstable_by(TOP_GAMES - 1, |a, b| b.cmp(a));
        keys.truncate(TOP_GAMES);
    }
    keys.sort_unstable_by(|a, b| b.cmp(a));
    keys.iter()
        .map(|&k| {
            let game = (k >> 16) as GameId;
            Ok(TopGame { row: db.row(game)?, ply: k as u16 })
        })
        .collect()
}

/// Statistics of the games following a line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineStats {
    pub games: u32,
    pub white_wins: u32,
    pub draws: u32,
    pub black_wins: u32,
    /// The Elos of both players, summed over those known.
    pub elo_sum: u64,
    pub elo_count: u32,
    /// Latest year a game followed the line (0 = unknown).
    pub last_year: u16,
}

impl LineStats {
    fn add(&mut self, g: &GameStat) {
        self.games += 1;
        match g.result {
            1 => self.white_wins += 1,
            2 => self.black_wins += 1,
            3 => self.draws += 1,
            _ => {}
        }
        for elo in [g.white_elo, g.black_elo].into_iter().filter(|&e| e != 0) {
            self.elo_sum += u64::from(elo);
            self.elo_count += 1;
        }
        self.last_year = self.last_year.max(g.year);
    }
}

/// Moves from the position, and the games that played all of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub moves: Vec<Move>,
    pub stats: LineStats,
}

/// The main line and the lines from a position (the Reference tab's
/// main variations).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReferenceLines {
    /// The most played move, then the most played reply and so on, while
    /// [`MAIN_LINE_MIN_GAMES`] games and a tenth of the position's follow,
    /// up to [`MAIN_LINE_PLIES`]. `None` when no first move qualifies.
    pub main_line: Option<Line>,
    /// Up to [`MAX_LINES`]: grouped by first move in the moves table's
    /// order, at most [`LINES_PER_MOVE`] each, most games first.
    pub lines: Vec<Line>,
    /// Games whose moves were counted: those reaching the position, deleted
    /// ones left out.
    pub games: u32,
}

/// A game's next moves from the position, packed: move byte + 1 (0 where
/// the game ended) in the top [`MAIN_LINE_PLIES`] bytes, first move
/// highest, so packed sequences sort like the lines; the game's index in
/// the low 32 bits.
type Packed = u128;

fn pack(moves: &[u8], index: usize) -> Packed {
    let mut k = index as u128;
    for (d, &b) in moves.iter().take(MAIN_LINE_PLIES).enumerate() {
        k |= (u128::from(b) + 1) << (8 * (15 - d));
    }
    k
}

/// Move byte + 1 at ply `d` of a packed sequence (0: the game ended).
fn at(k: Packed, d: usize) -> u8 {
    (k >> (8 * (15 - d))) as u8
}

fn index(k: Packed) -> usize {
    k as u32 as usize
}

/// The moves of `path` (move bytes + 1) from `pos`, as far as they're legal.
fn decode(pos: &Chess, path: &[u8]) -> Vec<Move> {
    let mut pos = pos.clone();
    let mut moves = Vec::with_capacity(path.len());
    for &b in path {
        // A hash collision could give an impossible move; stop there.
        let Some(m) = b.checked_sub(1).and_then(|b| move_at(&pos, b)) else { break };
        pos.play_unchecked(m);
        moves.push(m);
    }
    moves
}

fn line_stats(seqs: impl IntoIterator<Item = Packed>, stats: &[GameStat]) -> LineStats {
    let mut out = LineStats::default();
    for k in seqs {
        out.add(&stats[index(k)]);
    }
    out
}

/// The main line through `seqs` (see [`ReferenceLines::main_line`]): its
/// path (move bytes + 1) and the sequences that follow all of it.
fn main_line(seqs: &[Packed], stats: &[GameStat]) -> (Vec<u8>, Vec<Packed>) {
    let total = seqs.len() as u64;
    let mut members = seqs.to_vec();
    let mut path = Vec::new();
    for d in 0..MAIN_LINE_PLIES {
        let mut count = [0u32; 256];
        let mut year = [0u16; 256];
        for &k in &members {
            let b = usize::from(at(k, d));
            if b != 0 {
                count[b] += 1;
                year[b] = year[b].max(stats[index(k)].year);
            }
        }
        // Like the moves table: most games, then most recent, then the
        // lowest move byte.
        let best = (1..256).max_by_key(|&b| (count[b], year[b], Reverse(b))).expect("a non-empty range");
        let n = u64::from(count[best]);
        if n < u64::from(MAIN_LINE_MIN_GAMES) || n * 10 < total {
            break;
        }
        path.push(best as u8);
        members.retain(|&k| at(k, d) == best as u8);
    }
    (path, members)
}

/// The leaves of the lines' trie over the sorted sequences in `range`,
/// which share `path` (their first `path.len()` moves): a line goes on to
/// any first move, then to the replies at least [`LINE_MIN_GAMES`] games
/// follow, up to [`LINE_PLIES`]. A node none of whose continuations goes
/// on (the games end, or each continuation has too few) ends its line.
fn collect_leaves(
    sorted: &[Packed],
    range: Range<usize>,
    path: &mut Vec<u8>,
    leaves: &mut Vec<(Vec<u8>, Range<usize>)>,
) {
    let depth = path.len();
    if depth == LINE_PLIES {
        leaves.push((path.clone(), range));
        return;
    }
    let mut extended = false;
    let mut i = range.start;
    while i < range.end {
        let b = at(sorted[i], depth);
        let mut j = i + 1;
        while j < range.end && at(sorted[j], depth) == b {
            j += 1;
        }
        if b != 0 && (depth == 0 || (j - i) as u64 >= u64::from(LINE_MIN_GAMES)) {
            extended = true;
            path.push(b);
            collect_leaves(sorted, i..j, path, leaves);
            path.pop();
        }
        i = j;
    }
    if !extended && depth > 0 {
        leaves.push((path.clone(), range));
    }
}

/// The lines to show (see [`ReferenceLines::lines`]) from the trie's
/// leaves over `sorted`.
fn pick_lines(
    sorted: &[Packed],
    stats: &[GameStat],
    leaves: Vec<(Vec<u8>, Range<usize>)>,
) -> Vec<(Vec<u8>, LineStats)> {
    // First moves in the moves table's order: most games, then most recent.
    let mut firsts: Vec<(u8, u32, u16)> = Vec::new();
    for &k in sorted {
        let b = at(k, 0);
        if b == 0 {
            continue;
        }
        let year = stats[index(k)].year;
        match firsts.last_mut() {
            Some(f) if f.0 == b => {
                f.1 += 1;
                f.2 = f.2.max(year);
            }
            _ => firsts.push((b, 1, year)),
        }
    }
    firsts.sort_by_key(|&(b, n, year)| (Reverse(n), Reverse(year), b));

    let mut leaves: Vec<(Vec<u8>, LineStats)> = leaves
        .into_iter()
        .map(|(path, range)| {
            let stats = line_stats(sorted[range].iter().copied(), stats);
            (path, stats)
        })
        .collect();
    leaves.sort_by(|a, b| {
        (Reverse(a.1.games), Reverse(a.1.last_year), &a.0).cmp(&(
            Reverse(b.1.games),
            Reverse(b.1.last_year),
            &b.0,
        ))
    });
    let mut out = Vec::new();
    for (first, _, _) in firsts {
        let group = leaves.iter().filter(|(path, _)| path[0] == first).take(LINES_PER_MOVE);
        for (path, stats) in group {
            if out.len() == MAX_LINES {
                return out;
            }
            out.push((path.clone(), stats.clone()));
        }
    }
    out
}

/// The main line and lines from `pos`, from the games `hits` found there
/// (see [`find_position`]). Reads each game's next moves, so it's slow for
/// common positions: run it as a job from [`LINES_JOB_GAMES`] games.
pub fn reference_lines(
    db: &Mutex<Database>,
    pos: &Chess,
    hits: &PositionHits,
    progress: &dyn Progress,
) -> Result<ReferenceLines, DbError> {
    // The games (ascending, as `find_position` returns them), each with the
    // ply where it reaches the position and its stats.
    let mut ids: Vec<GameId> = Vec::with_capacity(hits.entries.len());
    let mut plies: Vec<u16> = Vec::with_capacity(hits.entries.len());
    let mut stats: Vec<GameStat> = Vec::with_capacity(hits.entries.len());
    {
        let mut db = db.lock().expect("db lock");
        let all = db.game_stats()?;
        for e in &hits.entries {
            if let Some(g) = all.get(e.game as usize).filter(|g| !g.deleted) {
                ids.push(e.game);
                plies.push(e.ply);
                stats.push(*g);
            }
        }
    }
    debug_assert!(ids.windows(2).all(|w| w[0] < w[1]), "hits come in game order");

    let mut seqs: Vec<Packed> = Vec::with_capacity(ids.len());
    let work = |id: GameId, _rec: &IndexRecord, blob: &[u8]| -> Option<Packed> {
        let i = ids.binary_search(&id).ok()?;
        let mut moves = Vec::with_capacity(MAIN_LINE_PLIES);
        match mainline_window(blob, usize::from(plies[i]), MAIN_LINE_PLIES, &mut moves) {
            Ok(()) => Some(pack(&moves, i)),
            Err(e) => {
                log::warn!("reference lines: skipping game {id}: {e}");
                None
            }
        }
    };
    for_each_game(db, &ids, progress, &work, &mut |batch| {
        seqs.extend(batch.into_iter().flatten());
        Ok(())
    })?;

    let (path, members) = main_line(&seqs, &stats);
    let main_line =
        (!path.is_empty()).then(|| Line { moves: decode(pos, &path), stats: line_stats(members, &stats) });

    seqs.sort_unstable();
    let mut leaves = Vec::new();
    collect_leaves(&seqs, 0..seqs.len(), &mut Vec::new(), &mut leaves);
    let lines = pick_lines(&seqs, &stats, leaves)
        .into_iter()
        .map(|(path, stats)| Line { moves: decode(pos, &path), stats })
        .collect();
    Ok(ReferenceLines { main_line, lines, games: seqs.len() as u32 })
}

/// A game's start position and its main line (the Reference tab's mini
/// board steps through it).
pub fn game_mainline(db: &Database, id: GameId) -> Result<(Chess, Vec<Move>), DbError> {
    let (setup, bytes) = mainline(&db.read_blob(id)?)?;
    let start = setup.unwrap_or_default();
    let mut pos = start.clone();
    let mut moves = Vec::with_capacity(bytes.len());
    for b in bytes {
        let m = move_at(&pos, b).ok_or_else(|| DbError::Corrupt(format!("game {id}: bad move")))?;
        pos.play_unchecked(m);
        moves.push(m);
    }
    Ok((start, moves))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NoProgress, import_pgn, update_position_index};
    use chess_core::notation::san;
    use chess_core::pgn::read_pgn_str;
    use chess_core::position::parse_uci;

    /// `testdata/pgn/reference.pgn`, indexed: 56 games from the start, made
    /// for these counts (groups of games with the same moves, in order).
    fn fixture() -> (tempfile::TempDir, Mutex<Database>) {
        let dir = tempfile::tempdir().unwrap();
        let db = Mutex::new(Database::create(&dir.path().join("Ref")).unwrap());
        let pgn =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn/reference.pgn");
        assert_eq!(import_pgn(&db, &pgn, &NoProgress).unwrap().imported, 56);
        update_position_index(&db, &NoProgress).unwrap();
        (dir, db)
    }

    fn db_of(pgn: &str) -> (tempfile::TempDir, Mutex<Database>) {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Database::create(&dir.path().join("Gen")).unwrap();
        for g in read_pgn_str(pgn) {
            db.append(&g).unwrap();
        }
        let db = Mutex::new(db);
        update_position_index(&db, &NoProgress).unwrap();
        (dir, db)
    }

    fn after(ucis: &[&str]) -> Chess {
        let mut pos = Chess::default();
        for u in ucis {
            let m = parse_uci(&pos, u).unwrap();
            pos.play_unchecked(m);
        }
        pos
    }

    fn sans(pos: &Chess, moves: &[Move]) -> String {
        let mut pos = pos.clone();
        let mut out = Vec::new();
        for &m in moves {
            out.push(san(&pos, m));
            pos.play_unchecked(m);
        }
        out.join(" ")
    }

    fn lines(db: &Mutex<Database>, pos: &Chess) -> ReferenceLines {
        let hits = find_position(db, pos).unwrap();
        reference_lines(db, pos, &hits, &NoProgress).unwrap()
    }

    /// Each line as "moves:games".
    fn line_list(pos: &Chess, lines: &ReferenceLines) -> Vec<String> {
        lines.lines.iter().map(|l| format!("{}:{}", sans(pos, &l.moves), l.stats.games)).collect()
    }

    fn move_list(pos: &Chess, o: &ReferenceOverview) -> Vec<String> {
        o.moves
            .iter()
            .map(|m| format!("{}:{}", m.mv.map_or("end".into(), |m| san(pos, m)), m.stats.games))
            .collect()
    }

    fn best(o: &ReferenceOverview, i: usize) -> Vec<String> {
        o.moves[i].best.iter().map(|p| format!("{} {}", p.name, p.elo)).collect()
    }

    fn trends(o: &ReferenceOverview) -> Vec<Option<Trend>> {
        o.moves.iter().map(|m| m.hot.as_ref().and_then(|h| h.trend)).collect()
    }

    #[test]
    fn overview_at_the_start() {
        let (_d, db) = fixture();
        let start = Chess::default();
        let o = reference_overview(&db, &start).unwrap();
        assert_eq!(o.total.games, 56);
        assert_eq!(move_list(&start, &o), ["e4:39", "d4:11", "c4:5", "Nf3:1"]);

        // Hot: the newest game is from 2020, so 2018–2020 are recent; 13 of
        // the games are from then (the 1.Nf3 game has no date).
        assert_eq!((o.recent_years, o.recent_games), (Some((2018, 2020)), 13));
        let e4 = o.moves[0].hot.as_ref().unwrap();
        assert_eq!((e4.recent, e4.share, e4.all_share), (5, 5.0 / 13.0, 39.0 / 56.0));
        // 1.d4: 8 of 13 lately against 11 of 56 overall. 1.c4 has no recent
        // game, but at its old share it would have had about one: too few
        // for ↓.
        assert_eq!(trends(&o), [Some(Trend::Down), Some(Trend::Up), None, None]);
        assert!(o.moves.iter().all(|m| m.hot.is_some()));

        // Best players: Carlsen once (his best Elo), and the tie at 2800
        // broken by the newer game (So's 2019 against Nakamura's 2015).
        assert_eq!(
            best(&o, 0),
            ["Carlsen, Magnus 2850", "Caruana, Fabiano 2820", "So, Wesley 2800", "Nakamura, Hikaru 2800"]
        );
        // Only three of the 1.d4 players have an Elo; Kramnik twice.
        assert_eq!(best(&o, 1), ["Kramnik, Vladimir 2810", "Anand, Viswanathan 2790", "Giri, Anish 2780"]);
        assert_eq!(best(&o, 2), ["Kasparov, Garry 2851", "Short, Nigel 2690", "Timman, Jan 2650"]);
        assert!(best(&o, 3).is_empty(), "no Elo, no best player");

        // Top games by the average Elo of both players; one Elo counts as
        // the average; ties newest first (2790: So 2019, Anand 2018 with
        // one Elo, Nakamura 2015, Caruana 2014).
        let whites: Vec<&str> = o.top_games.iter().take(8).map(|g| g.row.white.as_str()).collect();
        assert_eq!(
            whites,
            [
                "Carlsen, Magnus",
                "Kasparov, Garry",
                "Kramnik, Vladimir",
                "Carlsen, Magnus",
                "So, Wesley",
                "Anand, Viswanathan",
                "Nakamura, Hikaru",
                "Caruana, Fabiano"
            ]
        );
        assert_eq!(o.top_games.len(), 56);
        assert!(o.top_games.iter().all(|g| g.ply == 0));
        // Games without any Elo come last, newest first (two from 2020: the
        // later game first), the undated one last.
        let last: Vec<GameId> = o.top_games[46..].iter().map(|g| g.row.id).collect();
        assert_eq!(last, [47, 43, 41, 44, 49, 48, 11, 54, 19, 55]);
    }

    #[test]
    fn overview_with_black_to_move_and_deleted_games() {
        let (_d, db) = fixture();
        let pos = after(&["d2d4"]);
        let o = reference_overview(&db, &pos).unwrap();
        assert_eq!(move_list(&pos, &o), ["d5:6", "Nf6:5"]);
        // Black's Elos now; "Unknown, Player" has none.
        assert_eq!(best(&o, 0), ["Topalov, Veselin 2820", "Club player 69 2300"]);
        assert_eq!(best(&o, 1), ["Club player 74 2750", "Club player 75 2400"]);
        assert_eq!(o.recent_games, 8);
        assert_eq!(trends(&o), [None, None], "neither moves enough against the other");
        assert_eq!(o.top_games[0].row.white, "Kramnik, Vladimir");
        assert!(o.top_games.iter().all(|g| g.ply == 1));

        // Deleted games don't count anywhere.
        db.lock().unwrap().set_deleted_many(&[0, 50], true).unwrap();
        let start = Chess::default();
        let o = reference_overview(&db, &start).unwrap();
        assert_eq!(move_list(&start, &o), ["e4:38", "d4:11", "c4:4", "Nf3:1"]);
        assert_eq!(best(&o, 0)[0], "Carlsen, Magnus 2840", "his next best game");
        assert_eq!(best(&o, 2)[0], "Kasparov, Garry 2849");
        assert_eq!(o.top_games[0].row.white, "Kramnik, Vladimir");
        assert!(o.top_games.iter().all(|g| g.row.id != 0 && g.row.id != 50));
        assert_eq!(lines(&db, &start).games, 54);
    }

    #[test]
    fn lines_and_main_line_at_the_start() {
        let (_d, db) = fixture();
        let start = Chess::default();
        let l = lines(&db, &start);
        assert_eq!(l.games, 56);
        // 4.Ba4 (12 games) is under 20: the main line stops after 3...a6.
        let main = l.main_line.as_ref().unwrap();
        assert_eq!((sans(&start, &main.moves).as_str(), main.stats.games), ("e4 e5 Nf3 Nc6 Bb5 a6", 20));
        assert_eq!(
            line_list(&start, &l),
            [
                // Three per first move: the c6 and c5 lines tie at 5 games
                // and the newer one (c6, 2016) is kept.
                "e4 e5 Nf3 Nc6 Bb5 a6:20",
                "e4 e6 d4 d5 Nc3 Bb4:6",
                // Its games end after four plies.
                "e4 c6 d4 d5:5",
                "d4 d5 c4 e6 Nc3 Nf6:6",
                // 2.c4 (3) and 2.Nf3 (2) are both under five games.
                "d4 Nf6:5",
                "c4 e5 Nc3 Nf6:5",
                // A first move is always a line, however few its games.
                "Nf3:1",
            ]
        );
        let first = &l.lines[0].stats;
        assert_eq!((first.white_wins, first.draws, first.black_wins, first.last_year), (10, 5, 5, 2020));
        // Elos of both players where known: 20 games, 6 Elos missing.
        assert_eq!(first.elo_count, 34);
        assert_eq!(l.lines[6].stats, LineStats { games: 1, ..Default::default() }, "no result, Elo or year");
    }

    #[test]
    fn lines_from_a_later_position_and_games_ending_there() {
        let (_d, db) = fixture();
        let pos = after(&["e2e4", "e7e5"]);
        let l = lines(&db, &pos);
        assert_eq!(l.games, 23);
        let main = l.main_line.as_ref().unwrap();
        assert_eq!((sans(&pos, &main.moves).as_str(), main.stats.games), ("Nf3 Nc6 Bb5 a6", 20));
        // 2.Bc4's three games end right after it.
        assert_eq!(line_list(&pos, &l), ["Nf3 Nc6 Bb5 a6 Ba4 Nf6:12", "Nf3 Nc6 Bb5 a6 Bxc6 dxc6:8", "Bc4:3"]);
        // Where every game has ended there's nothing to show.
        let end = after(&["e2e4", "e7e5", "f1c4"]);
        let l = lines(&db, &end);
        assert_eq!((l.games, l.main_line.is_none(), l.lines.len()), (3, true, 0));
        let o = reference_overview(&db, &end).unwrap();
        assert_eq!(move_list(&end, &o), ["end:3"]);
        assert!(o.moves[0].hot.is_none() && o.moves[0].best.is_empty());
    }

    #[test]
    fn the_main_line_stops_at_ten_plies_and_needs_a_tenth_of_the_games() {
        let game = "1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 5. O-O Be7 6. Re1 b5 *\n\n";
        let (_d, db) = db_of(&game.repeat(25));
        let start = Chess::default();
        let l = lines(&db, &start);
        let main = l.main_line.as_ref().unwrap();
        assert_eq!(main.moves.len(), MAIN_LINE_PLIES);
        assert_eq!(main.stats.games, 25);
        assert_eq!(line_list(&start, &l), ["e4 e5 Nf3 Nc6 Bb5 a6:25"], "lines stop at six plies");

        // 25 games of 1.e4 among 289 are under a tenth: no main line.
        let others = ["d4", "c4", "Nf3", "g3", "b3", "f4", "Nc3", "b4", "e3", "d3", "c3"];
        let mut pgn = "1. e4 *\n\n".repeat(25);
        for m in others {
            pgn.push_str(&format!("1. {m} *\n\n").repeat(24));
        }
        let (_d, db) = db_of(&pgn);
        let l = lines(&db, &start);
        assert_eq!(l.games, 289);
        assert!(l.main_line.is_none());
    }

    #[test]
    fn at_most_twelve_lines_three_per_first_move() {
        // Five first moves, each with four replies played five times.
        let mut pgn = String::new();
        for (i, first) in ["a3", "b3", "c3", "d3", "e3"].iter().enumerate() {
            for reply in ["a6", "b6", "c6", "d6"] {
                // Older first moves later, so the order is a3, b3, c3, d3, e3.
                let year = 2020 - i;
                pgn.push_str(&format!("[Date \"{year}.01.01\"]\n\n1. {first} {reply} *\n\n").repeat(5));
            }
        }
        let (_d, db) = db_of(&pgn);
        let start = Chess::default();
        let l = lines(&db, &start);
        assert_eq!(l.games, 100);
        // Every first move has 20 games, a fifth: the newest leads, and no
        // reply has 20.
        let main = l.main_line.as_ref().unwrap();
        assert_eq!((sans(&start, &main.moves).as_str(), main.stats.games), ("a3", 20));
        assert_eq!(
            line_list(&start, &l),
            [
                "a3 a6:5", "a3 b6:5", "a3 c6:5", "b3 a6:5", "b3 b6:5", "b3 c6:5", "c3 a6:5", "c3 b6:5",
                "c3 c6:5", "d3 a6:5", "d3 b6:5", "d3 c6:5"
            ]
        );
    }

    #[test]
    fn hot_compares_shares_exactly() {
        // 5 of 10 recent games against 4 of 10 overall: exactly 1.25×.
        assert_eq!(hot(4, 5, 10, 10).unwrap().trend, Some(Trend::Up));
        // The same share with four recent games: too few for ↑.
        assert_eq!(hot(4, 4, 10, 8).unwrap().trend, None);
        // 4 of 10 against 10 of 20: exactly 0.8×, and 5 expected.
        assert_eq!(hot(10, 4, 20, 10).unwrap().trend, Some(Trend::Down));
        // Under 5 expected: no ↓, even with none lately.
        assert_eq!(hot(9, 0, 20, 10).unwrap().trend, None);
        assert_eq!(hot(5, 3, 10, 6).unwrap().trend, None, "the same share");
        assert!(hot(5, 0, 10, 0).is_none(), "no recent game at all");
    }

    #[test]
    fn without_years_there_is_no_hot() {
        let (_d, db) = db_of(&"1. e4 e5 *\n\n".repeat(6));
        let o = reference_overview(&db, &Chess::default()).unwrap();
        assert_eq!((o.recent_years, o.recent_games), (None, 0));
        assert!(o.moves[0].hot.is_none());
        assert!(o.moves[0].best.is_empty() && o.top_games.len() == 6);
    }

    #[test]
    fn top_keys_order_games() {
        let g =
            |white_elo, black_elo, year| GameStat { white_elo, black_elo, year, result: 0, deleted: false };
        let mut keys = [
            top_key(&g(0, 0, 2020), 1, 0),
            top_key(&g(2500, 0, 2000), 2, 0),
            top_key(&g(2400, 2600, 2010), 3, 0),
            top_key(&g(2400, 2600, 2012), 4, 0),
            top_key(&g(2600, 2600, 0), 5, 0),
            top_key(&g(0, 0, 0), 6, 0),
        ];
        keys.sort_unstable_by(|a, b| b.cmp(a));
        let ids: Vec<u32> = keys.iter().map(|&k| (k >> 16) as u32).collect();
        assert_eq!(ids, [5, 4, 3, 2, 1, 6]);
    }

    #[test]
    fn a_games_main_line_for_the_mini_board() {
        let (_d, db) = fixture();
        let (start, moves) = game_mainline(&db.lock().unwrap(), 0).unwrap();
        assert_eq!(start, Chess::default());
        assert_eq!(sans(&start, &moves), "e4 e5 Nf3 Nc6 Bb5 a6 Ba4 Nf6 O-O Be7");
        assert!(game_mainline(&db.lock().unwrap(), 999).is_err());
    }
}
