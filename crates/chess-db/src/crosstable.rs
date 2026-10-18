//! Tournament cross-tables (DB-16): the games of one event (a name of the
//! DB-04 events list) as standings, with points, score, average opponent
//! rating, FIDE performance rating and tie-breaks, plus every player's
//! results for a round-robin grid or a row of rounds (Swiss, knock-out).
//! Only index records are read, never game blobs.
//!
//! - **Editions.** One event name can cover several editions (a yearly
//!   tournament that keeps its name, or "Casual game"). Games with full
//!   dates more than [`EDITION_GAP_DAYS`] apart start a new edition; games
//!   with only a year join the edition that covers that year; games without
//!   a year join the only edition, or form one of their own.
//! - **Who played.** A player named "bye" (any case or brackets) is no
//!   opponent: the game is a bye for the other side. Games without two
//!   named players, or with a player against himself, are left out
//!   (`skipped`). Deleted games are ignored. A game with a round that is in
//!   the database twice (an index record the same in all but where the game
//!   is stored: an event imported twice) counts once (`duplicates`).
//! - **Results.** 1-0, 0-1 and ½-½ count; unfinished games (`*`) are shown
//!   but count for nothing. A decisive game without a move (and not from a
//!   set-up position) is a forfeit: it counts for the points and the
//!   tie-breaks but not for the performance or the average opponent rating,
//!   as in FIDE rating rules.
//! - **Pairing.** Two players: a match. A knock-out has the shape of one:
//!   every player meets one opponent per round and each pair one round
//!   only, at least a quarter of the meetings are mini-matches of several
//!   games, and whoever loses a match plays no later round (but in a
//!   third-place match in the last round, between two losers of one round,
//!   or a loser and a player whose match was level on points). Round
//!   robin: at least [`ROUND_ROBIN_PAIRS`] percent of all pairs met, or half
//!   of them over as many rounds as there are players less one (some games
//!   missing); as many cycles as rounds a pair met in (an armageddon after a
//!   draw shares its round). Else Swiss when the games have rounds, or no
//!   rounds (online arenas).
//! - **Ranking.** Points, then the tie-breaks: Sonneborn-Berger and wins in a
//!   round robin; Buchholz cut 1 and Buchholz in a Swiss or without rounds;
//!   none in a match. Byes and forfeits count for Buchholz as an opponent
//!   with the player's own final score (FIDE's dummy opponent, without its
//!   special cases); rounds missing from the database count for nothing.
//!   Players level on all of them share a rank. A knock-out ranks by the
//!   round of the first match lost (never lost: the round reached), then
//!   whether the player won their last match (the third-place match), so
//!   players knocked out in the same round share places (points order them
//!   within). A match level on points (an armageddon draw, or tie-breaks
//!   missing) goes to the player who played on; when neither did, its
//!   players share their places and `level_matches` counts it.
//! - **Size.** Editions over [`MAX_PLAYERS`] players or [`MAX_GAMES`] games
//!   get no table. Tables that would have more than [`MAX_CELLS`] cells
//!   with their results get the standings only (`per_game` false, no
//!   results).

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ops::Bound;
use std::sync::Mutex;

use crate::database::NameKind;
use crate::record::{FLAG_SETUP, IndexRecord};
use crate::{Database, DbError, GameId};

/// Most players an edition gets a table for. Bigger ones (a month of online
/// games under one name) only get their counts.
pub const MAX_PLAYERS: u32 = 2000;
/// Most games an edition gets a table for.
pub const MAX_GAMES: u32 = 20_000;
/// Most cells a table with results may have: its rows times its columns
/// (those of results, the grid's or the rounds layout's: rounds, or each
/// player's games in an event without rounds, whichever is more, plus
/// [`STANDINGS_COLUMNS`]), plus the results in them. A bigger table gets
/// the standings only. What a table costs a webview grows with its cells,
/// not its games: one player's 6,000 online games against 1,500 opponents
/// make 1,501 rows of 6,000 columns, 9 million cells, which froze a whole
/// machine. In `tauri dev` on a headless display a cell or a result takes
/// 40–50 µs to lay out and paint, so a table up to this size about a second.
pub const MAX_CELLS: u64 = 20_000;
/// The columns a table has besides the results: place, name, Elo, points,
/// games, score, average opponent Elo, performance and two tie-breaks.
pub const STANDINGS_COLUMNS: u64 = 10;
/// Most players a round robin has; more are paired some other way.
pub const MAX_ROUND_ROBIN: usize = 64;
/// Percent of all pairs of players that must have met for a round robin
/// (fewer will do when the rounds were as many as the players less one).
/// A Swiss of r rounds meets r / (players − 1) of them.
pub const ROUND_ROBIN_PAIRS: usize = 85;
/// Days without a game that separate two editions of an event.
pub const EDITION_GAP_DAYS: u32 = 180;

/// Which edition of an event to tabulate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditionPick {
    /// The newest one with a known year.
    Latest,
    /// By its index in [`CrossTable::editions`].
    Index(usize),
    /// The one with this game (falls back to the newest).
    WithGame(GameId),
}

/// One edition of an event: the games of one stretch of dates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edition {
    /// Packed dates (see [`crate::record::pack_date`]) of its first and last
    /// games with a known year; 0 when none has one.
    pub first: u32,
    pub last: u32,
    pub games: u32,
}

/// How the players were paired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pairing {
    /// Everyone meets everyone `cycles` times (2: a double round robin).
    RoundRobin {
        cycles: u16,
    },
    Swiss,
    KnockOut,
    /// Two players.
    Match,
    /// No round numbers and not a round robin: online arenas, or games
    /// whose rounds weren't recorded.
    Unrounded,
}

/// A game's result for one of its players.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Win,
    Draw,
    Loss,
    /// `*` or unknown: shown, but counts for nothing.
    Unfinished,
}

impl Outcome {
    /// Half points, or `None` for an unfinished game.
    pub fn half_points(self) -> Option<u32> {
        match self {
            Outcome::Win => Some(2),
            Outcome::Draw => Some(1),
            Outcome::Loss => Some(0),
            Outcome::Unfinished => None,
        }
    }
}

/// The tie-breaks a table ranks by, after points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TieBreak {
    /// Sum of the scores of the opponents beaten, plus half of those drawn.
    SonnebornBerger,
    /// Games won (forfeits included, byes not).
    Wins,
    /// Buchholz without the opponent with the lowest score.
    BuchholzCut1,
    /// Sum of the opponents' scores.
    Buchholz,
}

/// One game of a player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossResult {
    pub game: GameId,
    pub round: u16,
    pub subround: u16,
    /// Column in the rounds layout: the round, or the player's n-th game
    /// when the event has no rounds; 0 for a game without a round in an
    /// event that has them.
    pub slot: u16,
    /// The opponent's index in [`Standings::players`]; `None` for a bye.
    pub opponent: Option<usize>,
    pub white: bool,
    pub outcome: Outcome,
    pub forfeit: bool,
    pub opponent_elo: Option<u16>,
}

/// A player's line in the standings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standing {
    /// Player name id.
    pub player: u32,
    /// Their Elo in the event (from their first game that has one).
    pub elo: Option<u16>,
    /// 1-based; players level on points and every tie-break share
    /// `rank..=rank_to`.
    pub rank: u32,
    pub rank_to: u32,
    pub half_points: u32,
    /// Games with a result, byes and forfeits included.
    pub games: u32,
    pub wins: u32,
    pub draws: u32,
    pub losses: u32,
    /// Games played over the board against an opponent with an Elo.
    pub rated_games: u32,
    pub avg_opponent: Option<u16>,
    /// FIDE performance rating over the rated games.
    pub performance: Option<i32>,
    /// In quarter points.
    pub sonneborn_berger: u32,
    /// In half points.
    pub buchholz: u32,
    /// In half points.
    pub buchholz_cut1: u32,
    /// In the order they were played (by round, then date).
    pub results: Vec<CrossResult>,
}

impl Standing {
    /// The value of a tie-break, in quarter points (wins count 4 each), so
    /// they compare as integers.
    pub fn tie_break(&self, t: TieBreak) -> u32 {
        match t {
            TieBreak::SonnebornBerger => self.sonneborn_berger,
            TieBreak::Wins => self.wins * 4,
            TieBreak::BuchholzCut1 => self.buchholz_cut1 * 2,
            TieBreak::Buchholz => self.buchholz * 2,
        }
    }
}

/// An edition's standings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standings {
    pub pairing: Pairing,
    pub tie_breaks: &'static [TieBreak],
    /// In rank order.
    pub players: Vec<Standing>,
    /// Whether slots are rounds (else each player's games in order).
    pub by_round: bool,
    /// The slots with games, ascending (0 excluded).
    pub slots: Vec<u16>,
    /// Some games have slot 0 (no round in an event with rounds).
    pub unslotted: bool,
    /// Games in the table, byes included.
    pub games: u32,
    pub unfinished: u32,
    pub forfeits: u32,
    pub byes: u32,
    /// Games left out: without two named players, or a player against himself.
    pub skipped: u32,
    /// Games left out because the same game is in the edition already.
    pub duplicates: u32,
    /// Knock-out matches level on points that the games don't settle (see
    /// the module docs): their players share their places.
    pub level_matches: u32,
    /// Cells the table takes with its results (see [`MAX_CELLS`]).
    pub cells: u64,
    /// Whether the players' `results` are there; false when `cells` is over
    /// [`MAX_CELLS`], and the table is the standings only.
    pub per_game: bool,
}

/// One edition of an event, tabulated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossTable {
    /// Event name id.
    pub event: u32,
    /// Oldest first; games without a year last.
    pub editions: Vec<Edition>,
    /// The one tabulated.
    pub edition: usize,
    /// Named players in the edition (byes not counted).
    pub players: u32,
    /// The edition's games.
    pub games: u32,
    /// Site name id most of the edition's games share; 0 when none does.
    pub site: u32,
    /// `None` when the edition is too big (more than [`MAX_PLAYERS`] players
    /// or [`MAX_GAMES`] games).
    pub standings: Option<Standings>,
}

const CHUNK: u32 = 64 * 1024;

/// The cross-table of event `event` (a name id of [`NameKind::Event`]),
/// for the edition `pick` chooses.
pub fn cross_table(db: &Mutex<Database>, event: u32, pick: EditionPick) -> Result<CrossTable, DbError> {
    let Scan { games, groups, editions } = scan(db, event)?;
    let latest = editions.iter().rposition(|e| e.first != 0).unwrap_or(0);
    let edition = match pick {
        EditionPick::Latest => latest,
        EditionPick::Index(i) => i.min(editions.len() - 1),
        EditionPick::WithGame(id) => {
            groups.iter().position(|g| g.iter().any(|&i| games[i].0 == id)).unwrap_or(latest)
        }
    };
    let chosen: Vec<(GameId, IndexRecord)> = groups[edition].iter().map(|&i| games[i]).collect();

    // Who each player id is.
    let ids: BTreeSet<u32> = chosen.iter().flat_map(|(_, r)| [r.white, r.black]).collect();
    let (who, names): (HashMap<u32, Who>, HashMap<u32, String>) = {
        let guard = db.lock().expect("db lock");
        let table = guard.names(NameKind::Player);
        let who = ids.iter().map(|&id| (id, Who::of(table.get(id)))).collect();
        let names = ids.iter().map(|&id| (id, crate::fold(table.get(id)))).collect();
        (who, names)
    };
    let players = who.values().filter(|w| **w == Who::Player).count() as u32;

    let mut sites: HashMap<u32, u32> = HashMap::new();
    for (_, r) in &chosen {
        *sites.entry(r.site).or_default() += 1;
    }
    let site = sites
        .into_iter()
        .filter(|&(id, n)| id != 0 && n * 2 >= chosen.len() as u32)
        .max_by_key(|&(id, n)| (n, std::cmp::Reverse(id)))
        .map_or(0, |(id, _)| id);

    let too_big = players > MAX_PLAYERS || chosen.len() as u32 > MAX_GAMES;
    let standings = (!too_big).then(|| {
        standings(
            &chosen,
            |id| who.get(&id).copied().unwrap_or(Who::Nobody),
            |id| names.get(&id).map_or("", String::as_str),
        )
    });
    Ok(CrossTable { event, editions, edition, players, games: chosen.len() as u32, site, standings })
}

/// Every edition of event `event` with its games' ids, ascending (what
/// Show games lists for the edition on screen).
pub fn edition_games(db: &Mutex<Database>, event: u32) -> Result<Vec<(Edition, Vec<GameId>)>, DbError> {
    let Scan { games, groups, editions } = scan(db, event)?;
    Ok(editions.into_iter().zip(groups).map(|(e, g)| (e, g.iter().map(|&i| games[i].0).collect())).collect())
}

/// An event's games, split into editions.
struct Scan {
    /// Not deleted, by id.
    games: Vec<(GameId, IndexRecord)>,
    /// Each edition's games, as indexes into `games`.
    groups: Vec<Vec<usize>>,
    editions: Vec<Edition>,
}

fn scan(db: &Mutex<Database>, event: u32) -> Result<Scan, DbError> {
    if event == 0 {
        return Err(DbError::Invalid("the game has no event, so there is no cross-table".into()));
    }
    let count = db.lock().expect("db lock").count();
    let mut games: Vec<(GameId, IndexRecord)> = Vec::new();
    let mut start = 0;
    while start < count {
        let records = db.lock().expect("db lock").records(start, CHUNK)?;
        games.extend(
            records
                .into_iter()
                .zip(start..)
                .filter_map(|(r, id)| (r.event == event && !r.is_deleted()).then_some((id, r))),
        );
        start = start.saturating_add(CHUNK);
    }
    if games.is_empty() {
        return Err(DbError::Invalid("this event has no games".into()));
    }
    let groups = editions_of(&games);
    let editions = groups
        .iter()
        .map(|g| {
            let dates = g.iter().map(|&i| games[i].1.date).filter(|d| d >> 9 != 0);
            Edition {
                first: dates.clone().min().unwrap_or(0),
                last: dates.max().unwrap_or(0),
                games: g.len() as u32,
            }
        })
        .collect();
    Ok(Scan { games, groups, editions })
}

/// An approximate day number (months of 31 days) for gaps between dates,
/// or `None` without a year and month.
fn day(date: u32) -> Option<u32> {
    let (y, m, d) = (date >> 9, (date >> 5) & 0xf, date & 0x1f);
    (y != 0 && m != 0).then(|| y * 372 + (m - 1) * 31 + d.saturating_sub(1))
}

/// Splits an event's games into editions (indexes into `games`, ascending),
/// oldest first; see the module docs.
fn editions_of(games: &[(GameId, IndexRecord)]) -> Vec<Vec<usize>> {
    let mut dated: Vec<(u32, usize)> =
        games.iter().enumerate().filter_map(|(i, (_, r))| day(r.date).map(|d| (d, i))).collect();
    dated.sort_unstable();
    // (first day, last day, games)
    let mut groups: Vec<(u32, u32, Vec<usize>)> = Vec::new();
    for (d, i) in dated {
        match groups.last_mut() {
            Some(g) if d - g.1 <= EDITION_GAP_DAYS => {
                g.1 = d;
                g.2.push(i);
            }
            _ => groups.push((d, d, vec![i])),
        }
    }
    let mut undated = Vec::new();
    for (i, (_, r)) in games.iter().enumerate() {
        if day(r.date).is_some() {
            continue;
        }
        let year = r.date >> 9;
        if year == 0 {
            undated.push(i);
            continue;
        }
        let (lo, hi) = (year * 372, year * 372 + 371);
        match groups.iter_mut().find(|g| g.0 <= hi && g.1 >= lo) {
            Some(g) => g.2.push(i),
            None => groups.push((lo, hi, vec![i])),
        }
    }
    groups.sort_by_key(|g| g.0);
    let mut out: Vec<Vec<usize>> = groups.into_iter().map(|g| g.2).collect();
    if !undated.is_empty() {
        match out.as_mut_slice() {
            [only] => only.extend(undated),
            _ => out.push(undated),
        }
    }
    for g in &mut out {
        g.sort_unstable();
    }
    out
}

/// What a player name stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Who {
    Player,
    /// "bye", "BYE", "(bye)": no opponent.
    Bye,
    /// Empty, "?", "-": nobody known.
    Nobody,
}

impl Who {
    fn of(name: &str) -> Who {
        let letters: String =
            name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect();
        match letters.as_str() {
            "" => Who::Nobody,
            "bye" => Who::Bye,
            _ => Who::Player,
        }
    }
}

/// A game as one player saw it, before players are ranked.
struct Raw {
    game: GameId,
    round: u16,
    subround: u16,
    date: u32,
    opponent: Option<u32>,
    white: bool,
    outcome: Outcome,
    forfeit: bool,
    own_elo: u16,
    opponent_elo: u16,
}

fn outcome(result: u8, white: bool) -> Outcome {
    match (result, white) {
        (1, true) | (2, false) => Outcome::Win,
        (1, false) | (2, true) => Outcome::Loss,
        (3, _) => Outcome::Draw,
        _ => Outcome::Unfinished,
    }
}

/// FIDE's rating differences for a percentage score of 50 to 100 (the
/// rating regulations' table 8.1(a)); below 50 they're negated.
const DP: [i32; 51] = [
    0, 7, 14, 21, 29, 36, 43, 50, 57, 65, 72, 80, 87, 95, 102, 110, 117, 125, 133, 141, 149, 158, 166, 175,
    184, 193, 202, 211, 220, 230, 240, 251, 262, 273, 284, 296, 309, 322, 336, 351, 366, 383, 401, 422, 444,
    470, 501, 538, 589, 677, 800,
];

/// FIDE performance rating: the average opponent rating plus the rating
/// difference for the score (rounded to a whole percent).
pub fn performance(avg_opponent: u16, half_points: u32, games: u32) -> Option<i32> {
    if games == 0 {
        return None;
    }
    let pct = ((100 * half_points + games) / (2 * games)).min(100) as usize;
    let dp = if pct >= 50 { DP[pct - 50] } else { -DP[50 - pct] };
    Some(i32::from(avg_opponent) + dp)
}

/// The rounded mean of `sum` over `n`.
fn rounded_mean(sum: u64, n: u64) -> u64 {
    (2 * sum + n) / (2 * n)
}

/// Builds the standings of one edition's games. `who` classifies a player
/// name id and `name` gives its folded name (for the order of players who
/// share a rank).
fn standings<'a>(
    games: &[(GameId, IndexRecord)],
    who: impl Fn(u32) -> Who,
    name: impl Fn(u32) -> &'a str,
) -> Standings {
    let mut raw: HashMap<u32, Vec<Raw>> = HashMap::new();
    let mut order: Vec<u32> = Vec::new(); // players in order of appearance
    let (mut used, mut unfinished, mut forfeits, mut byes, mut skipped) = (0, 0, 0, 0, 0);
    let mut has_rounds = false;
    let mut pair_games: Vec<PairGame> = Vec::new();
    let mut seen = HashSet::new();
    let mut duplicates = 0;

    for &(id, r) in games {
        let (w, b) = (who(r.white), who(r.black));
        let sides: Vec<(u32, bool, Option<u32>)> = match (w, b) {
            (Who::Player, Who::Player) if r.white != r.black => {
                vec![(r.white, true, Some(r.black)), (r.black, false, Some(r.white))]
            }
            (Who::Player, Who::Bye) => vec![(r.white, true, None)],
            (Who::Bye, Who::Player) => vec![(r.black, false, None)],
            _ => {
                skipped += 1;
                continue;
            }
        };
        // The same game again (an event imported twice). Not without a
        // round: online games between two players can look alike.
        if r.round != 0 && !seen.insert(same_game(&r)) {
            duplicates += 1;
            continue;
        }
        used += 1;
        has_rounds |= r.round != 0;
        let decisive = r.result == 1 || r.result == 2;
        let bye = sides.len() == 1;
        let forfeit = !bye && decisive && r.plies == 0 && r.flags & FLAG_SETUP == 0;
        if r.result == 0 {
            unfinished += 1;
        }
        if forfeit {
            forfeits += 1;
        }
        if bye {
            byes += 1;
        } else {
            let (a, b) = (r.white.min(r.black), r.white.max(r.black));
            let a_half = outcome(r.result, a == r.white).half_points();
            pair_games.push(PairGame { a, b, round: r.round, a_half });
        }
        for (player, white, opponent) in sides {
            let list = raw.entry(player).or_insert_with(|| {
                order.push(player);
                Vec::new()
            });
            let (own_elo, opponent_elo) =
                if white { (r.white_elo, r.black_elo) } else { (r.black_elo, r.white_elo) };
            list.push(Raw {
                game: id,
                round: r.round,
                subround: r.subround,
                date: r.date,
                opponent,
                white,
                outcome: outcome(r.result, white),
                forfeit,
                own_elo,
                opponent_elo: if opponent.is_some() { opponent_elo } else { 0 },
            });
        }
    }

    let n = order.len();
    let pairing = detect_pairing(n, has_rounds, &pair_games);
    let tie_breaks: &'static [TieBreak] = match pairing {
        Pairing::RoundRobin { .. } => &[TieBreak::SonnebornBerger, TieBreak::Wins],
        Pairing::Match | Pairing::KnockOut => &[],
        Pairing::Swiss | Pairing::Unrounded => &[TieBreak::BuchholzCut1, TieBreak::Buchholz],
    };

    // Each player's games in order, and their points.
    for list in raw.values_mut() {
        if has_rounds {
            list.sort_by_key(|g| (g.round == 0, g.round, g.subround, g.date, g.game));
        } else {
            list.sort_by_key(|g| (g.date, g.game));
        }
    }
    let points: HashMap<u32, u32> =
        raw.iter().map(|(&p, list)| (p, list.iter().filter_map(|g| g.outcome.half_points()).sum())).collect();

    let mut players: Vec<Standing> = order
        .iter()
        .map(|&p| {
            let list = &raw[&p];
            let half_points = points[&p];
            let count =
                |o: Outcome| list.iter().filter(|g| g.outcome == o && g.opponent.is_some()).count() as u32;
            let games = list.iter().filter(|g| g.outcome.half_points().is_some()).count() as u32;
            let rated: Vec<(u32, u16)> = list
                .iter()
                .filter(|g| g.opponent.is_some() && !g.forfeit && g.opponent_elo != 0)
                .filter_map(|g| g.outcome.half_points().map(|h| (h, g.opponent_elo)))
                .collect();
            let rated_games = rated.len() as u32;
            let avg_opponent = (rated_games > 0).then(|| {
                let sum: u64 = rated.iter().map(|&(_, e)| u64::from(e)).sum();
                rounded_mean(sum, u64::from(rated_games)) as u16
            });
            let performance = avg_opponent
                .and_then(|avg| performance(avg, rated.iter().map(|&(h, _)| h).sum(), rated_games));
            // Tie-breaks.
            let mut sb = 0;
            let mut contributions = Vec::new();
            for g in list {
                let Some(h) = g.outcome.half_points() else { continue };
                match g.opponent {
                    Some(o) => {
                        sb += points[&o] * h;
                        contributions.push(if g.forfeit { half_points } else { points[&o] });
                    }
                    None => contributions.push(half_points),
                }
            }
            let buchholz: u32 = contributions.iter().sum();
            let buchholz_cut1 = if contributions.len() >= 2 {
                buchholz - contributions.iter().min().expect("non-empty")
            } else {
                buchholz
            };
            Standing {
                player: p,
                elo: list.iter().map(|g| g.own_elo).find(|&e| e != 0),
                rank: 0,
                rank_to: 0,
                half_points,
                games,
                wins: count(Outcome::Win),
                draws: count(Outcome::Draw),
                losses: count(Outcome::Loss),
                rated_games,
                avg_opponent,
                performance,
                sonneborn_berger: sb,
                buchholz,
                buchholz_cut1,
                results: Vec::new(),
            }
        })
        .collect();

    let (stages, level_matches) =
        if pairing == Pairing::KnockOut { knock_out_stages(&raw) } else { (HashMap::new(), 0) };
    // Rank: points, then the tie-breaks (a knock-out: its stage); level
    // players by points, Elo, then name.
    let key = |s: &Standing| -> Vec<u32> {
        if pairing == Pairing::KnockOut {
            return stages.get(&s.player).map_or_else(Vec::new, |k| k.to_vec());
        }
        std::iter::once(s.half_points * 2).chain(tie_breaks.iter().map(|&t| s.tie_break(t))).collect()
    };
    players.sort_by(|a, b| {
        key(b)
            .cmp(&key(a))
            .then_with(|| b.half_points.cmp(&a.half_points))
            .then_with(|| b.elo.cmp(&a.elo))
            .then_with(|| name(a.player).cmp(name(b.player)))
            .then_with(|| a.player.cmp(&b.player))
    });
    let keys: Vec<Vec<u32>> = players.iter().map(key).collect();
    for i in 0..players.len() {
        let first = keys.iter().position(|k| *k == keys[i]).expect("own key");
        let last = keys.iter().rposition(|k| *k == keys[i]).expect("own key");
        players[i].rank = first as u32 + 1;
        players[i].rank_to = last as u32 + 1;
    }

    // Results, with opponents as indexes into the ranked players.
    let index: HashMap<u32, usize> = players.iter().enumerate().map(|(i, s)| (s.player, i)).collect();
    let mut slots = BTreeSet::new();
    let mut unslotted = false;
    for s in &mut players {
        s.results = raw[&s.player]
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let slot = if has_rounds { g.round } else { (i + 1).min(usize::from(u16::MAX)) as u16 };
                if slot == 0 {
                    unslotted = true;
                } else {
                    slots.insert(slot);
                }
                CrossResult {
                    game: g.game,
                    round: g.round,
                    subround: g.subround,
                    slot,
                    opponent: g.opponent.map(|o| index[&o]),
                    white: g.white,
                    outcome: g.outcome,
                    forfeit: g.forfeit,
                    opponent_elo: (g.opponent_elo != 0).then_some(g.opponent_elo),
                }
            })
            .collect();
    }

    // What the table takes on screen: a row per player, a column per slot
    // (or per player in the grid, which has room for up to MAX_ROUND_ROBIN)
    // and the standings, and the results in them. Too many, and the table
    // is the standings.
    let columns = slots.len() + usize::from(unslotted);
    let grid = if n <= MAX_ROUND_ROBIN { n } else { 0 };
    let results: usize = players.iter().map(|p| p.results.len()).sum();
    let cells = (n as u64) * (columns.max(grid) as u64 + STANDINGS_COLUMNS) + results as u64;
    let per_game = cells <= MAX_CELLS;
    if !per_game {
        for p in &mut players {
            p.results = Vec::new();
        }
    }

    Standings {
        pairing,
        tie_breaks,
        players,
        by_round: has_rounds,
        slots: slots.into_iter().collect(),
        unslotted,
        games: used,
        unfinished,
        forfeits,
        byes,
        skipped,
        duplicates,
        level_matches,
        cells,
        per_game,
    }
}

/// What makes two index records the same game: everything but where the
/// game is stored (see the module docs).
fn same_game(r: &IndexRecord) -> IndexRecord {
    IndexRecord { blob_offset: 0, ..*r }
}

/// A knock-out's rank keys and its unsettled level matches: for each
/// player, the stage reached (twice the round of the first match lost;
/// never lost: twice the last round, plus 1 when that match was won) and
/// whether the last match was won (the third-place match). See the module
/// docs.
fn knock_out_stages(raw: &HashMap<u32, Vec<Raw>>) -> (HashMap<u32, [u32; 2]>, u32) {
    // Each player's matches: round → (opponent, own half points, the opponent's).
    let mut matches: HashMap<u32, BTreeMap<u16, (u32, u32, u32)>> = HashMap::new();
    for (&p, list) in raw {
        let m = matches.entry(p).or_default();
        for g in list.iter().filter(|g| g.round != 0) {
            let Some(o) = g.opponent else { continue };
            let (own, theirs) = g.outcome.half_points().map_or((0, 0), |h| (h, 2 - h));
            let e = m.entry(g.round).or_insert((o, 0, 0));
            e.1 += own;
            e.2 += theirs;
        }
    }
    // Won (Greater), lost (Less) or level on points.
    let mut result: HashMap<(u32, u16), Ordering> = HashMap::new();
    for (&p, m) in &matches {
        for (&r, &(_, own, theirs)) in m {
            result.insert((p, r), own.cmp(&theirs));
        }
    }
    let next_opponent =
        |p: u32, r: u16| matches[&p].range((Bound::Excluded(r), Bound::Unbounded)).next().map(|(_, m)| m.0);
    // A level match goes to whoever played on; when both did (the semi-finals
    // before a third-place match), to the one whose next opponent won that
    // round as well, or whose opponent's next opponent lost it.
    let mut settled = Vec::new();
    let mut level = 0;
    for (&p, m) in &matches {
        for (&r, &(o, _, _)) in m {
            if result[&(p, r)] != Ordering::Equal || p > o {
                continue;
            }
            let won = match (next_opponent(p, r), next_opponent(o, r)) {
                (Some(_), None) => Some(true),
                (None, Some(_)) => Some(false),
                (Some(x), Some(y)) => match (result.get(&(x, r)), result.get(&(y, r))) {
                    (Some(Ordering::Greater), _) | (_, Some(Ordering::Less)) => Some(true),
                    (Some(Ordering::Less), _) | (_, Some(Ordering::Greater)) => Some(false),
                    _ => None,
                },
                (None, None) => None,
            };
            match won {
                Some(won) => {
                    let (mine, theirs) = if won {
                        (Ordering::Greater, Ordering::Less)
                    } else {
                        (Ordering::Less, Ordering::Greater)
                    };
                    settled.push(((p, r), mine));
                    settled.push(((o, r), theirs));
                }
                None => level += 1,
            }
        }
    }
    result.extend(settled);
    let stages = matches
        .iter()
        .map(|(&p, m)| {
            let first_lost = m.keys().find(|&&r| result[&(p, r)] == Ordering::Less);
            let last = m.keys().next_back().map(|&r| (r, result[&(p, r)] == Ordering::Greater));
            let stage = match (first_lost, last) {
                (Some(&r), _) => 2 * u32::from(r),
                (None, Some((r, won))) => 2 * u32::from(r) + u32::from(won),
                (None, None) => 0,
            };
            (p, [stage, u32::from(last.is_some_and(|(_, won)| won))])
        })
        .collect();
    (stages, level)
}

/// A game between two players, for telling how they were paired.
#[derive(Debug, Clone, Copy)]
struct PairGame {
    /// The lower player id, and the higher.
    a: u32,
    b: u32,
    round: u16,
    /// `a`'s half points; `None` while unfinished.
    a_half: Option<u32>,
}

/// How `n` players were paired, from the games between two of them; see
/// the module docs.
fn detect_pairing(n: usize, has_rounds: bool, games: &[PairGame]) -> Pairing {
    if n <= 2 {
        return Pairing::Match;
    }
    if has_rounds && is_knock_out(games) {
        return Pairing::KnockOut;
    }
    let pairs: HashSet<(u32, u32)> = games.iter().map(|g| (g.a, g.b)).collect();
    let all_pairs = n * (n - 1) / 2;
    let rounds = games.iter().filter(|g| g.round != 0).map(|g| g.round).collect::<HashSet<_>>().len();
    let round_robin = n <= MAX_ROUND_ROBIN
        && !pairs.is_empty()
        && (pairs.len() * 100 >= all_pairs * ROUND_ROBIN_PAIRS
            || (pairs.len() * 2 >= all_pairs && rounds + 1 >= n));
    if round_robin {
        // A pair's games in one round are one meeting (a mini-match, or an
        // armageddon after a draw); games without a round one each.
        let meetings =
            games.iter().filter(|g| g.round != 0).map(|g| (g.a, g.b, g.round)).collect::<HashSet<_>>().len()
                + games.iter().filter(|g| g.round == 0).count();
        let cycles = rounded_mean(meetings as u64, pairs.len() as u64).max(1);
        return Pairing::RoundRobin { cycles: cycles.min(u64::from(u16::MAX)) as u16 };
    }
    if has_rounds { Pairing::Swiss } else { Pairing::Unrounded }
}

/// Whether games with rounds have a knock-out's shape (see the module docs).
fn is_knock_out(games: &[PairGame]) -> bool {
    // Each meeting: (a, b, round) → (a's half points, b's, games).
    let mut meetings: HashMap<(u32, u32, u16), (u32, u32, u32)> = HashMap::new();
    for g in games.iter().filter(|g| g.round != 0) {
        let m = meetings.entry((g.a, g.b, g.round)).or_default();
        if let Some(h) = g.a_half {
            m.0 += h;
            m.1 += 2 - h;
        }
        m.2 += 1;
    }
    // One opponent per round, and one round per pair.
    let mut opponents: HashMap<(u32, u16), u32> = HashMap::new();
    let mut pairs: HashSet<(u32, u32)> = HashSet::new();
    for &(a, b, round) in meetings.keys() {
        if !pairs.insert((a, b))
            || opponents.insert((a, round), b).is_some()
            || opponents.insert((b, round), a).is_some()
        {
            return false;
        }
    }
    let several = meetings.values().filter(|m| m.2 >= 2).count();
    if several == 0 || several * 4 < meetings.len() {
        return false;
    }
    // Nobody plays on after losing a match, but in a third-place match in
    // the last round, between two losers of one round (or a loser and a
    // player whose match then was level on points: lost on tie-breaks
    // missing from the database).
    let last = meetings.keys().map(|k| k.2).max();
    let mut lost: HashMap<u32, u16> = HashMap::new();
    let mut level: HashSet<(u32, u16)> = HashSet::new();
    for (&(a, b, round), &(pa, pb, _)) in &meetings {
        let loser = match pa.cmp(&pb) {
            Ordering::Less => a,
            Ordering::Greater => b,
            Ordering::Equal => {
                level.extend([(a, round), (b, round)]);
                continue;
            }
        };
        let first = lost.entry(loser).or_insert(round);
        *first = (*first).min(round);
    }
    meetings.keys().all(|&(a, b, round)| {
        let out = |p: u32| lost.get(&p).copied().filter(|&r| r < round);
        match (out(a), out(b)) {
            (None, None) => true,
            (Some(ra), Some(rb)) => Some(round) == last && ra == rb,
            (Some(r), None) => Some(round) == last && level.contains(&(b, r)),
            (None, Some(r)) => Some(round) == last && level.contains(&(a, r)),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::pack_date;
    use chess_core::pgn::read_pgn_str;

    /// A game in PGN: event, date, round, white, black, result, Elos, moves.
    fn pgn(event: &str, date: &str, round: &str, white: &str, black: &str, result: &str) -> String {
        game(event, date, round, (white, 0), (black, 0), result, "1. e4 e5")
    }

    fn game(
        event: &str,
        date: &str,
        round: &str,
        (white, welo): (&str, u16),
        (black, belo): (&str, u16),
        result: &str,
        moves: &str,
    ) -> String {
        let mut s = format!(
            "[Event \"{event}\"]\n[Date \"{date}\"]\n[Round \"{round}\"]\n[White \"{white}\"]\n[Black \"{black}\"]\n[Result \"{result}\"]\n"
        );
        if welo != 0 {
            s += &format!("[WhiteElo \"{welo}\"]\n");
        }
        if belo != 0 {
            s += &format!("[BlackElo \"{belo}\"]\n");
        }
        s + &format!("\n{moves} {result}\n\n")
    }

    fn db_of(pgn: &str) -> (tempfile::TempDir, Mutex<Database>) {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Database::create(&dir.path().join("Cross")).unwrap();
        for g in read_pgn_str(pgn) {
            db.append(&g).unwrap();
        }
        (dir, Mutex::new(db))
    }

    fn event_id(db: &Mutex<Database>, name: &str) -> u32 {
        db.lock().unwrap().names(NameKind::Event).id_of(name).unwrap()
    }

    fn table(db: &Mutex<Database>, event: &str) -> CrossTable {
        cross_table(db, event_id(db, event), EditionPick::Latest).unwrap()
    }

    fn name(db: &Mutex<Database>, id: u32) -> String {
        db.lock().unwrap().names(NameKind::Player).get(id).to_owned()
    }

    /// Player names in rank order, with points as a string ("2½").
    fn ranking(db: &Mutex<Database>, s: &Standings) -> Vec<(String, String)> {
        s.players
            .iter()
            .map(|p| {
                let pts = match (p.half_points / 2, p.half_points % 2) {
                    (0, 1) => "½".to_owned(),
                    (whole, 1) => format!("{whole}½"),
                    (whole, _) => whole.to_string(),
                };
                (name(db, p.player), pts)
            })
            .collect()
    }

    fn by_name<'s>(db: &Mutex<Database>, s: &'s Standings, who: &str) -> &'s Standing {
        s.players.iter().find(|p| name(db, p.player) == who).unwrap()
    }

    #[test]
    fn round_robin_standings_performance_and_sonneborn_berger() {
        // Four players, three rounds: A wins every game, the others draw theirs.
        let e = "Club RR";
        let d = "2024.03.01";
        let all = [
            game(e, d, "1.1", ("A", 2400), ("D", 2100), "1-0", "1. e4 e5"),
            game(e, d, "1.2", ("B", 2300), ("C", 2200), "1/2-1/2", "1. d4 d5"),
            game(e, d, "2.1", ("C", 2200), ("A", 2400), "0-1", "1. c4 e5"),
            game(e, d, "2.2", ("D", 2100), ("B", 2300), "1/2-1/2", "1. Nf3 d5"),
            game(e, d, "3.1", ("A", 2400), ("B", 2300), "1-0", "1. e4 c5"),
            game(e, d, "3.2", ("C", 2200), ("D", 2100), "1/2-1/2", "1. e4 e6"),
        ]
        .concat();
        let (_d, db) = db_of(&all);
        let t = table(&db, e);
        assert_eq!((t.players, t.games, t.editions.len()), (4, 6, 1));
        let s = t.standings.unwrap();
        assert_eq!(s.pairing, Pairing::RoundRobin { cycles: 1 });
        assert_eq!(s.tie_breaks, &[TieBreak::SonnebornBerger, TieBreak::Wins]);
        // B (1) and C (1) and D (1) tie on points: SB B = ½·1 (C) + ½·1 (D) = 1,
        // C = ½·1 + ½·1 = 1, D = ½·1 + ½·1 = 1: all level; Elo orders them, one shared rank.
        assert_eq!(
            ranking(&db, &s),
            [("A", "3"), ("B", "1"), ("C", "1"), ("D", "1")].map(|(a, b)| (a.to_owned(), b.to_owned()))
        );
        let a = by_name(&db, &s, "A");
        assert_eq!((a.rank, a.rank_to, a.games, a.wins), (1, 1, 3, 3));
        // Opponents 2100, 2200, 2300: average 2200, 100% → +800.
        assert_eq!((a.avg_opponent, a.performance), (Some(2200), Some(3000)));
        assert_eq!(a.sonneborn_berger, 4 * 3, "beat three players with 1 point each: 3");
        let b = by_name(&db, &s, "B");
        assert_eq!((b.rank, b.rank_to), (2, 4));
        // Average (2200 + 2100 + 2400) / 3 = 2233, 33% → −125.
        assert_eq!((b.avg_opponent, b.performance), (Some(2233), Some(2233 - 125)));
        // Results in round order, opponents as ranks.
        let rounds: Vec<(u16, Option<usize>, bool, Outcome)> =
            a.results.iter().map(|r| (r.slot, r.opponent, r.white, r.outcome)).collect();
        assert_eq!(
            rounds,
            [
                (1, Some(3), true, Outcome::Win),
                (2, Some(2), false, Outcome::Win),
                (3, Some(1), true, Outcome::Win)
            ]
        );
        assert!(s.by_round && !s.unslotted);
        assert_eq!(s.slots, [1, 2, 3]);
        // Four rows of four columns (the grid has more than the three rounds)
        // and the standings, and 12 results.
        assert_eq!((s.cells, s.per_game, s.duplicates, s.level_matches), (4 * (4 + 10) + 12, true, 0, 0));
    }

    #[test]
    fn sonneborn_berger_breaks_a_tie() {
        // A and B both score 2 of 3; A beat C (2 points), B beat D (0).
        let e = "SB";
        let d = "2024.01.01";
        let all = [
            pgn(e, d, "1", "A", "C", "1-0"),
            pgn(e, d, "1", "B", "D", "1-0"),
            pgn(e, d, "2", "A", "B", "1/2-1/2"),
            pgn(e, d, "2", "C", "D", "1-0"),
            pgn(e, d, "3", "A", "D", "1/2-1/2"),
            pgn(e, d, "3", "B", "C", "1/2-1/2"),
        ]
        .concat();
        let (_d, db) = db_of(&all);
        let s = table(&db, e).standings.unwrap();
        // Points: A 2, B 2, C 1½, D ½. SB: A = 1½ (C) + ½·2 (B) + ½·½ (D) = 2.75,
        // B = ½ (D) + ½·2 (A) + ½·1½ (C) = 2.25.
        assert_eq!(ranking(&db, &s)[..2], [("A".into(), "2".into()), ("B".into(), "2".into())]);
        let (a, b) = (&s.players[0], &s.players[1]);
        assert_eq!((a.sonneborn_berger, b.sonneborn_berger), (11, 9));
        assert_eq!((a.rank, a.rank_to, b.rank, b.rank_to), (1, 1, 2, 2));
        // No Elos: no average or performance.
        assert_eq!((a.avg_opponent, a.performance, a.elo), (None, None, None));
    }

    #[test]
    fn double_round_robin_has_two_results_per_pairing() {
        let e = "Double";
        let d = "2023.05.05";
        let all = [
            pgn(e, d, "1", "A", "B", "1-0"),
            pgn(e, d, "2", "B", "C", "1/2-1/2"),
            pgn(e, d, "3", "C", "A", "0-1"),
            pgn(e, d, "4", "B", "A", "1/2-1/2"),
            pgn(e, d, "5", "C", "B", "0-1"),
            pgn(e, d, "6", "A", "C", "*"),
        ]
        .concat();
        let (_d, db) = db_of(&all);
        let s = table(&db, e).standings.unwrap();
        assert_eq!(s.pairing, Pairing::RoundRobin { cycles: 2 });
        assert_eq!((s.games, s.unfinished), (6, 1));
        let a = &s.players[0];
        assert_eq!(name(&db, a.player), "A");
        // The unfinished game is listed but counts for nothing.
        assert_eq!((a.half_points, a.games, a.results.len()), (5, 3, 4));
        let vs_c: Vec<(bool, Outcome)> = a
            .results
            .iter()
            .filter(|r| r.opponent.is_some_and(|o| name(&db, s.players[o].player) == "C"))
            .map(|r| (r.white, r.outcome))
            .collect();
        assert_eq!(vs_c, [(false, Outcome::Win), (true, Outcome::Unfinished)]);
    }

    #[test]
    fn swiss_is_detected_with_buchholz() {
        // Eight players, three rounds: 12 of 28 pairs met.
        let e = "Open";
        let d = "2022.08.01";
        let p = ["P1", "P2", "P3", "P4", "P5", "P6", "P7", "P8"];
        let rounds = [
            // round 1: 1-5, 2-6, 3-7, 4-8, top half wins
            [(0, 4, "1-0"), (1, 5, "1-0"), (2, 6, "1-0"), (3, 7, "1-0")],
            // round 2: winners and losers paired
            [(0, 1, "1-0"), (2, 3, "1/2-1/2"), (4, 5, "0-1"), (6, 7, "1-0")],
            [(0, 2, "1/2-1/2"), (1, 3, "1-0"), (5, 6, "1/2-1/2"), (4, 7, "1-0")],
        ];
        let mut all = String::new();
        for (r, games) in rounds.iter().enumerate() {
            for (board, &(w, b, res)) in games.iter().enumerate() {
                all += &pgn(e, d, &format!("{}.{}", r + 1, board + 1), p[w], p[b], res);
            }
        }
        let (_d, db) = db_of(&all);
        let s = table(&db, e).standings.unwrap();
        assert_eq!(s.pairing, Pairing::Swiss);
        assert_eq!(s.tie_breaks, &[TieBreak::BuchholzCut1, TieBreak::Buchholz]);
        // P1 2½, P2 2, P3 2, P4 1½, P6 1½, P7 1½, P5 1, P8 0.
        let p1 = by_name(&db, &s, "P1");
        assert_eq!(p1.half_points, 5);
        // P1 met P5 (1), P2 (2), P3 (2): Buchholz 5, cut 1 drops P5: 4.
        assert_eq!((p1.buchholz, p1.buchholz_cut1), (10, 8));
        assert_eq!(s.players[0].player, p1.player);
        // P2 and P3 have 2 each; P2 met P6 (1½), P1 (2½), P4 (1½) = 5½, cut 4;
        // P3 met P7 (1½), P4 (1½), P1 (2½) = 5½, cut 4: level, sharing rank 2–3.
        let (p2, p3) = (by_name(&db, &s, "P2"), by_name(&db, &s, "P3"));
        assert_eq!((p2.buchholz, p2.buchholz_cut1), (11, 8));
        assert_eq!((p2.rank, p2.rank_to, p3.rank, p3.rank_to), (2, 3, 2, 3));
        // Opponent ranks in the rounds row: P1 met P5, P2, P3.
        let opponents: Vec<String> =
            p1.results.iter().map(|r| name(&db, s.players[r.opponent.unwrap()].player)).collect();
        assert_eq!(opponents, ["P5", "P2", "P3"]);
        assert_eq!(s.slots, [1, 2, 3]);
    }

    #[test]
    fn byes_forfeits_unnamed_players_and_missing_rounds() {
        let e = "Weekend";
        let d = "2021.10.02";
        let all = [
            game(e, d, "1", ("A", 2000), ("B", 1800), "1-0", "1. e4 e5"),
            game(e, d, "1", ("C", 1900), ("BYE", 0), "1-0", ""),
            // A forfeit: decisive without a move.
            game(e, d, "2", ("C", 1900), ("A", 2000), "0-1", ""),
            game(e, d, "2", ("B", 1800), ("(bye)", 0), "1/2-1/2", ""),
            // A game without a round, and one with an unknown player.
            game(e, d, "?", ("B", 1800), ("C", 1900), "1/2-1/2", "1. d4 d5"),
            game(e, d, "3", ("?", 0), ("A", 2000), "0-1", "1. e4 e5"),
        ]
        .concat();
        let (_d, db) = db_of(&all);
        let s = table(&db, e).standings.unwrap();
        assert_eq!((s.games, s.byes, s.forfeits, s.skipped, s.unfinished), (5, 2, 1, 1, 0));
        assert!(s.unslotted && s.by_round);
        assert_eq!(s.slots, [1, 2]);
        let a = by_name(&db, &s, "A");
        // Two wins, one a forfeit: the forfeit counts for points, not performance.
        assert_eq!((a.half_points, a.games, a.wins, a.rated_games), (4, 2, 2, 1));
        assert_eq!((a.avg_opponent, a.performance), (Some(1800), Some(2600)));
        let c = by_name(&db, &s, "C");
        // C: bye (1), forfeit loss (0), draw with B (½).
        assert_eq!((c.half_points, c.games, c.wins, c.losses, c.draws), (3, 3, 0, 1, 1));
        let bye = &c.results[0];
        assert_eq!((bye.opponent, bye.outcome, bye.forfeit), (None, Outcome::Win, false));
        assert!(c.results[1].forfeit);
        assert_eq!(c.results[2].slot, 0, "the game without a round comes last, in no round");
        // Buchholz: the bye and the forfeit count C's own 1½, B has 1: 4.
        assert_eq!(c.buchholz, 8);
        // Rated games only: B at 1800, ½.
        assert_eq!((c.avg_opponent, c.performance, c.rated_games), (Some(1800), Some(1800), 1));
    }

    #[test]
    fn games_without_rounds_are_numbered_per_player() {
        let e = "Arena";
        let d = "2024.01.01";
        let names = ["a", "b", "c", "d", "e", "f"];
        let mut all = String::new();
        for (w, b) in [(0, 1), (2, 3), (4, 5), (0, 2), (1, 4)] {
            all += &pgn(e, d, "-", names[w], names[b], "1-0");
        }
        let (_d, db) = db_of(&all);
        let s = table(&db, e).standings.unwrap();
        assert_eq!(s.pairing, Pairing::Unrounded);
        assert!(!s.by_round);
        assert_eq!(s.slots, [1, 2]);
        let a = by_name(&db, &s, "a");
        assert_eq!(a.results.iter().map(|r| r.slot).collect::<Vec<_>>(), [1, 2]);
    }

    #[test]
    fn matches_and_knock_outs() {
        let mut all = String::new();
        for (i, res) in ["1/2-1/2", "1-0", "1/2-1/2", "0-1"].iter().enumerate() {
            let (w, b) = if i % 2 == 0 { ("Kasparov", "Karpov") } else { ("Karpov", "Kasparov") };
            all += &pgn("WCh", "1985.09.03", &(i + 1).to_string(), w, b, res);
        }
        // Knock-out: four players, two-game semis then a final with a tie-break.
        for (round, w, b, res) in [
            ("1.1", "A", "B", "1-0"),
            ("1.2", "B", "A", "1/2-1/2"),
            ("1.1", "C", "D", "1/2-1/2"),
            ("1.2", "D", "C", "0-1"),
            ("2.1", "A", "C", "1/2-1/2"),
            ("2.2", "C", "A", "1/2-1/2"),
            ("2.3", "A", "C", "1-0"),
        ] {
            all += &pgn("Cup", "2020.02.02", round, w, b, res);
        }
        let (_d, db) = db_of(&all);
        let m = table(&db, "WCh").standings.unwrap();
        assert_eq!(m.pairing, Pairing::Match);
        assert!(m.tie_breaks.is_empty());
        // Level: no Elo, so by name.
        assert_eq!(ranking(&db, &m), [("Karpov".into(), "2".into()), ("Kasparov".into(), "2".into())]);
        assert_eq!((m.players[0].rank, m.players[0].rank_to), (1, 2));
        let k = table(&db, "Cup").standings.unwrap();
        assert_eq!(k.pairing, Pairing::KnockOut);
        assert!(k.tie_breaks.is_empty());
        assert_eq!(k.slots, [1, 2]);
        assert_eq!(k.players[0].results.iter().filter(|r| r.slot == 2).count(), 3);
        // The final's winner, its loser, then the semi-finalists sharing third
        // place (B and D both scored ½; by name).
        let places: Vec<(String, u32, u32)> =
            k.players.iter().map(|p| (name(&db, p.player), p.rank, p.rank_to)).collect();
        assert_eq!(places, [("A".into(), 1, 1), ("C".into(), 2, 2), ("B".into(), 3, 4), ("D".into(), 3, 4)]);
    }

    #[test]
    fn editions_split_by_dates() {
        let mut all = String::new();
        // 2019 and 2020 editions; one 2020 game has only its year; the
        // winter edition spans New Year; one game has no date at all.
        all += &pgn("Memorial", "2019.06.01", "1", "A", "B", "1-0");
        all += &pgn("Memorial", "2019.06.02", "2", "B", "C", "1-0");
        all += &pgn("Memorial", "2020.06.01", "1", "A", "C", "1-0");
        all += &pgn("Memorial", "2020.??.??", "2", "C", "B", "1-0");
        all += &pgn("Memorial", "????.??.??", "3", "B", "A", "1-0");
        all += &pgn("Hastings", "2019.12.29", "1", "A", "B", "1-0");
        all += &pgn("Hastings", "2020.01.05", "2", "B", "A", "1-0");
        let (_d, db) = db_of(&all);
        let memorial = event_id(&db, "Memorial");
        let t = cross_table(&db, memorial, EditionPick::Latest).unwrap();
        let spans: Vec<(u32, u32, u32)> = t.editions.iter().map(|e| (e.first, e.last, e.games)).collect();
        assert_eq!(
            spans,
            [
                (pack_date("2019.06.01"), pack_date("2019.06.02"), 2),
                (pack_date("2020.??.??"), pack_date("2020.06.01"), 2),
                (0, 0, 1),
            ]
        );
        assert_eq!((t.edition, t.games), (1, 2));
        let t = cross_table(&db, memorial, EditionPick::WithGame(4)).unwrap();
        assert_eq!(t.edition, 2);
        let t = cross_table(&db, memorial, EditionPick::Index(0)).unwrap();
        assert_eq!((t.edition, t.players, t.standings.unwrap().players.len()), (0, 3, 3));
        let hastings = table(&db, "Hastings");
        assert_eq!(hastings.editions.len(), 1);
        assert_eq!(hastings.games, 2);

        // Show games (R1-5 of the review): each edition's games.
        let games = edition_games(&db, memorial).unwrap();
        let ids: Vec<Vec<GameId>> = games.iter().map(|(_, ids)| ids.clone()).collect();
        assert_eq!(ids, [vec![0, 1], vec![2, 3], vec![4]]);
        let spans: Vec<(u32, u32)> = games.iter().map(|(e, _)| (e.first, e.last)).collect();
        assert_eq!(spans[0], (pack_date("2019.06.01"), pack_date("2019.06.02")));
    }

    /// Berger tables: the pairs of each round of a round robin of `n`
    /// players (`n` even), by index.
    fn berger(n: usize) -> Vec<Vec<(usize, usize)>> {
        let mut ps: Vec<usize> = (0..n).collect();
        let mut rounds = Vec::new();
        for _ in 0..n - 1 {
            rounds.push((0..n / 2).map(|i| (ps[i], ps[n - 1 - i])).collect());
            let last = ps.pop().unwrap();
            ps.insert(1, last);
        }
        rounds
    }

    /// R1-1 of the review: one player's online games under one event name,
    /// without rounds, make a column per game of theirs for every opponent.
    /// The table can't take more than MAX_CELLS cells: past them it has the
    /// standings only, and no results at all.
    #[test]
    fn results_never_take_more_than_max_cells() {
        // The review's machine-freezing event: 1,500 opponents, 4 games each.
        let hub = |opponents: usize, per: usize| {
            let mut all = String::new();
            for o in 0..opponents {
                for j in 0..per {
                    let opp = format!("opp{o:04}");
                    let (w, b) = if j % 2 == 0 { ("Hero", opp.as_str()) } else { (opp.as_str(), "Hero") };
                    let res = ["1-0", "0-1", "1/2-1/2"][(o + j) % 3];
                    all += &pgn("Rated Blitz game", "2024.03.01", "-", w, b, res);
                }
            }
            all
        };
        let (_d, db) = db_of(&hub(1500, 4));
        let t = table(&db, "Rated Blitz game");
        assert_eq!((t.players, t.games), (1501, 6000));
        let s = t.standings.expect("within MAX_PLAYERS and MAX_GAMES: standings");
        assert_eq!(s.pairing, Pairing::Unrounded);
        // 1,501 rows of 6,000 columns and the standings, plus 12,000 results.
        assert_eq!(s.cells, 1501 * (6000 + 10) + 12_000);
        assert!(!s.per_game);
        assert!(s.players.iter().all(|p| p.results.is_empty()), "no results to draw");
        let hero = &s.players[0];
        assert_eq!((hero.games, hero.wins + hero.draws + hero.losses), (6000, 6000));
        assert_eq!(s.slots.len(), 6000, "the columns are still known, for the summary");

        // Over and under the bound: rows × (columns + 10) + results.
        let (_d, db) = db_of(&hub(100, 2));
        let s = table(&db, "Rated Blitz game").standings.unwrap();
        assert_eq!((s.cells, s.per_game), (101 * (200 + 10) + 400, false));
        let (_d, db) = db_of(&hub(60, 2));
        let s = table(&db, "Rated Blitz game").standings.unwrap();
        assert_eq!((s.cells, s.per_game), (61 * (120 + 10) + 240, true));
        assert_eq!(s.players[0].results.len(), 120);
        assert!(s.cells <= MAX_CELLS);
    }

    /// Places of a knock-out's players: (name, rank, rank_to).
    fn places(db: &Mutex<Database>, s: &Standings) -> Vec<(String, u32, u32)> {
        s.players.iter().map(|p| (name(db, p.player), p.rank, p.rank_to)).collect()
    }

    fn place_list(rows: &[(&str, u32, u32)]) -> Vec<(String, u32, u32)> {
        rows.iter().map(|&(n, a, b)| (n.to_owned(), a, b)).collect()
    }

    /// R1-2 of the review: a third-place match in the final's round. Its
    /// winner ranks third, not level with the champion.
    #[test]
    fn knock_out_third_place_match() {
        let semis = [
            ("1.1", "A", "D", "1-0"),
            ("1.2", "D", "A", "1/2-1/2"),
            ("1.1", "B", "C", "1/2-1/2"),
            ("1.2", "C", "B", "0-1"),
        ];
        let cup = |event: &str, round: &str, rest: &[(&str, &str, &str, &str)]| {
            let mut all = String::new();
            for &(r, w, b, res) in semis.iter().chain(rest) {
                let r = if r.starts_with('3') { r.replacen('3', round, 1) } else { r.to_owned() };
                all += &pgn(event, "2023.08.01", &r, w, b, res);
            }
            all
        };
        // The final (A - B) and the third-place match (C - D) both in round
        // 2, as in the FIDE World Cup; then the third-place match in a round
        // of its own after the final.
        let rest = [
            ("2.1", "A", "B", "1-0"),
            ("2.2", "B", "A", "1/2-1/2"),
            ("3.1", "C", "D", "1-0"),
            ("3.2", "D", "C", "1/2-1/2"),
        ];
        let (_d, db) = db_of(&(cup("Same round", "2", &rest) + &cup("Own round", "3", &rest)));
        for event in ["Same round", "Own round"] {
            let s = table(&db, event).standings.unwrap();
            assert_eq!(s.pairing, Pairing::KnockOut, "{event}");
            assert_eq!(places(&db, &s), place_list(&[("A", 1, 1), ("B", 2, 2), ("C", 3, 3), ("D", 4, 4)]));
            assert_eq!(s.level_matches, 0);
        }
    }

    /// R1-2 of the review: a final level on points (Black's armageddon draw)
    /// can't be settled from the games: its players share 1–2, and the table
    /// says so. A level match whose winner played on is settled.
    #[test]
    fn knock_out_level_matches() {
        let mut all = String::new();
        for (r, w, b, res) in [
            ("1.1", "A", "D", "1-0"),
            ("1.2", "D", "A", "1/2-1/2"),
            ("1.1", "B", "C", "1/2-1/2"),
            ("1.2", "C", "B", "0-1"),
            ("2.1", "A", "B", "1-0"),
            ("2.2", "B", "A", "1-0"),
            ("2.3", "A", "B", "1/2-1/2"),
        ] {
            all += &pgn("Armageddon final", "2023.08.01", r, w, b, res);
        }
        // Semi-finals level on points (tie-breaks not in the database): F and
        // G played on. Then a third-place match after level semi-finals: the
        // final's players are the ones who went through.
        for (r, w, b, res) in [
            ("1.1", "E", "H", "1/2-1/2"),
            ("1.2", "H", "E", "1/2-1/2"),
            ("1.1", "F", "G", "1-0"),
            ("1.2", "G", "F", "1-0"),
            ("2.1", "E", "G", "1-0"),
            ("2.2", "G", "E", "0-1"),
        ] {
            all += &pgn("Missing tie-breaks", "2023.09.01", r, w, b, res);
        }
        for (r, w, b, res) in [
            ("1.1", "P", "S", "1-0"),
            ("1.2", "S", "P", "0-1"),
            ("1.1", "Q", "R", "1/2-1/2"),
            ("1.2", "R", "Q", "1/2-1/2"),
            ("2.1", "P", "R", "1/2-1/2"),
            ("2.2", "R", "P", "0-1"),
            ("2.1", "Q", "S", "1-0"),
            ("2.2", "S", "Q", "1/2-1/2"),
        ] {
            all += &pgn("Level semi-final", "2023.10.01", r, w, b, res);
        }
        let (_d, db) = db_of(&all);
        let s = table(&db, "Armageddon final").standings.unwrap();
        assert_eq!(s.pairing, Pairing::KnockOut);
        assert_eq!(places(&db, &s), place_list(&[("A", 1, 2), ("B", 1, 2), ("C", 3, 4), ("D", 3, 4)]));
        assert_eq!(s.level_matches, 1);

        // E went through against H, G against F (whoever played round 2).
        let s = table(&db, "Missing tie-breaks").standings.unwrap();
        assert_eq!(s.pairing, Pairing::KnockOut);
        assert_eq!(places(&db, &s), place_list(&[("E", 1, 1), ("G", 2, 2), ("F", 3, 4), ("H", 3, 4)]));
        assert_eq!(s.level_matches, 0);

        // Q - R was level, and both played round 2: R met P, who won their
        // semi-final, so R went through to the final and Q played for third.
        let s = table(&db, "Level semi-final").standings.unwrap();
        assert_eq!(s.pairing, Pairing::KnockOut);
        assert_eq!(places(&db, &s), place_list(&[("P", 1, 1), ("R", 2, 2), ("Q", 3, 3), ("S", 4, 4)]));
        assert_eq!(s.level_matches, 0);
    }

    /// R1-3 of the review: events with mini-matches in a round that aren't
    /// knock-outs. Losers play on and pairs meet again, so they're round
    /// robins or a Swiss, ranked by points.
    #[test]
    fn mini_matches_without_a_knock_out() {
        let p = ["P1", "P2", "P3", "P4", "P5", "P6"];
        let rounds = berger(6);
        let reversed: Vec<Vec<(usize, usize)>> =
            rounds.iter().map(|r| r.iter().map(|&(a, b)| (b, a)).collect()).collect();
        // Norway Chess: a double round robin with an armageddon game in the
        // same round after each draw.
        let mut all = String::new();
        let mut k = 0;
        for (ri, pairs) in rounds.iter().chain(&reversed).enumerate() {
            for (bi, &(a, b)) in pairs.iter().enumerate() {
                k += 1;
                let round = format!("{}.{}", ri + 1, bi + 1);
                let res = ["1/2-1/2", "1-0", "1/2-1/2", "0-1"][k % 4];
                all += &pgn("Norway", "2025.05.27", &round, p[a], p[b], res);
                if res == "1/2-1/2" {
                    all += &pgn(
                        "Norway",
                        "2025.05.27",
                        &round,
                        p[a],
                        p[b],
                        if k % 3 != 0 { "1-0" } else { "0-1" },
                    );
                }
            }
        }
        // A round robin of two-game mini-matches, both games in one round.
        for (ri, pairs) in rounds.iter().enumerate() {
            for (bi, &(a, b)) in pairs.iter().enumerate() {
                let round = format!("{}.{}", ri + 1, bi + 1);
                all += &pgn("Mini-matches", "2024.02.10", &round, p[a], p[b], "1-0");
                all += &pgn("Mini-matches", "2024.02.10", &round, p[b], p[a], "1/2-1/2");
            }
        }
        let (_d, db) = db_of(&all);
        let s = table(&db, "Norway").standings.unwrap();
        assert_eq!(s.pairing, Pairing::RoundRobin { cycles: 2 }, "two cycles, the armageddons in them");
        assert_eq!(s.tie_breaks, &[TieBreak::SonnebornBerger, TieBreak::Wins]);
        assert!(s.players.windows(2).all(|w| w[0].half_points >= w[1].half_points), "by points");
        assert_eq!(by_name(&db, &s, "P1").half_points, 15);
        assert_eq!(by_name(&db, &s, "P2").half_points, 13);
        assert!(by_name(&db, &s, "P1").rank < by_name(&db, &s, "P2").rank);
        let s = table(&db, "Mini-matches").standings.unwrap();
        assert_eq!(s.pairing, Pairing::RoundRobin { cycles: 1 });
        assert_eq!(ranking(&db, &s)[0], ("P1".to_owned(), "7½".to_owned()));
    }

    /// R1-3 of the review: a Swiss imported twice stays a Swiss, with every
    /// game counted once.
    #[test]
    fn duplicate_games_count_once() {
        let p: Vec<String> = (1..=12).map(|i| format!("S{i}")).collect();
        let mut once = String::new();
        for (ri, pairs) in berger(12).iter().take(5).enumerate() {
            for (bi, &(a, b)) in pairs.iter().enumerate() {
                let res = ["1-0", "0-1", "1/2-1/2"][(ri + bi) % 3];
                once +=
                    &pgn("Weekend Swiss", "2024.03.02", &format!("{}.{}", ri + 1, bi + 1), &p[a], &p[b], res);
            }
        }
        let (_d, single) = db_of(&once);
        let (_d2, twice) = db_of(&(once.clone() + &once));
        let one = table(&single, "Weekend Swiss").standings.unwrap();
        let two = table(&twice, "Weekend Swiss").standings.unwrap();
        assert_eq!((one.pairing, two.pairing), (Pairing::Swiss, Pairing::Swiss));
        assert_eq!((one.games, one.duplicates, two.games, two.duplicates), (30, 0, 30, 30));
        assert_eq!(ranking(&single, &one), ranking(&twice, &two), "the same standings");
        // The games kept are the first copies.
        assert!(two.players.iter().flat_map(|p| &p.results).all(|r| r.game < 30));
    }

    /// R1-4 of the review: a Swiss with many rounds (14 players, 7 rounds:
    /// 49 of 91 pairs met) isn't a round robin; a round robin with games
    /// missing still is.
    #[test]
    fn round_robins_and_long_swiss_events() {
        let p: Vec<String> = (1..=14).map(|i| format!("C{i:02}")).collect();
        let mut all = String::new();
        for (ri, pairs) in berger(14).iter().enumerate() {
            for (bi, &(a, b)) in pairs.iter().enumerate() {
                let round = format!("{}.{}", ri + 1, bi + 1);
                let res = ["1-0", "1/2-1/2", "0-1"][(ri * 7 + bi) % 3];
                if ri < 7 {
                    all += &pgn("Club Swiss", "2024.05.04", &round, &p[a], &p[b], res);
                }
                // Every round, but only three boards of seven: 39 of 91 pairs.
                if bi < 3 {
                    all += &pgn("Some boards", "2024.06.04", &round, &p[a], &p[b], res);
                }
                // The last round missing: 84 of 91 pairs.
                if ri < 12 {
                    all += &pgn("Last round missing", "2024.07.04", &round, &p[a], &p[b], res);
                }
                // Two rounds missing: 77 of 91 pairs, 11 rounds of 13.
                if ri < 11 {
                    all += &pgn("Two rounds missing", "2024.08.04", &round, &p[a], &p[b], res);
                }
            }
        }
        let (_d, db) = db_of(&all);
        let s = table(&db, "Club Swiss").standings.unwrap();
        assert_eq!(s.pairing, Pairing::Swiss);
        assert_eq!(s.tie_breaks, &[TieBreak::BuchholzCut1, TieBreak::Buchholz]);
        assert_eq!(
            table(&db, "Last round missing").standings.unwrap().pairing,
            Pairing::RoundRobin { cycles: 1 }
        );
        assert_eq!(table(&db, "Two rounds missing").standings.unwrap().pairing, Pairing::Swiss);
        // 39 of 91 pairs is under half: a Swiss, rounds or not.
        assert_eq!(table(&db, "Some boards").standings.unwrap().pairing, Pairing::Swiss);
    }

    /// `testdata/pgn/crosstable.pgn`: four small tournaments made for these
    /// numbers (the smoke driver `crosstable.js` reads it too).
    #[test]
    fn fixture_tournaments() {
        let dir = tempfile::tempdir().unwrap();
        let db = Mutex::new(Database::create(&dir.path().join("Fixture")).unwrap());
        let pgn =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn/crosstable.pgn");
        assert_eq!(crate::import_pgn(&db, &pgn, &crate::NoProgress).unwrap().imported, 47);
        let pts = |rows: &[(&str, &str)]| -> Vec<(String, String)> {
            rows.iter().map(|&(n, p)| (n.to_owned(), p.to_owned())).collect()
        };

        // Six players, a forfeit (Fern - Elm) and an unfinished game (Cedar - Fern).
        let s = table(&db, "Spring Round Robin").standings.unwrap();
        assert_eq!(s.pairing, Pairing::RoundRobin { cycles: 1 });
        assert_eq!((s.games, s.unfinished, s.forfeits, s.byes), (15, 1, 1, 0));
        assert_eq!(
            ranking(&db, &s),
            pts(&[
                ("Alder, Anna", "4"),
                ("Cedar, Clara", "3"),
                ("Elm, Eva", "3"),
                ("Brook, Ben", "2½"),
                ("Dale, David", "1½"),
                ("Fern, Felix", "0"),
            ])
        );
        let perf = |p: &Standing| (p.avg_opponent, p.performance, p.rated_games, p.sonneborn_berger);
        // Alder: 0 (Fern) + 1¼ (Brook) + 1½ (Cedar) + 1½ (Dale) + 3 (Elm) = 7.25.
        assert_eq!(perf(&s.players[0]), (Some(2317), Some(2557), 5, 29));
        // Cedar and Elm: Sonneborn-Berger 7.5 against 4.25.
        assert_eq!(perf(&s.players[1]), (Some(2351), Some(2544), 4, 30));
        assert_eq!(perf(&s.players[2]), (Some(2366), Some(2366), 4, 17));
        assert_eq!((s.players[5].games, s.players[5].losses), (4, 4));

        // Seven players and a bye a round; three on 2½ split by Buchholz cut 1.
        let s = table(&db, "Autumn Open").standings.unwrap();
        assert_eq!(s.pairing, Pairing::Swiss);
        assert_eq!((s.games, s.byes), (12, 3));
        assert_eq!(
            ranking(&db, &s),
            pts(&[
                ("Heath, Hana", "2½"),
                ("Gale, Gus", "2½"),
                ("Juniper, Jon", "2½"),
                ("Iris, Ida", "1½"),
                ("Kestrel, Kim", "1"),
                ("Linden, Lea", "1"),
                ("Moss, Max", "½"),
            ])
        );
        let bh: Vec<(u32, u32)> = s.players.iter().map(|p| (p.buchholz_cut1, p.buchholz)).collect();
        assert_eq!(bh, [(8, 9), (7, 9), (5, 7), (10, 11), (10, 12), (10, 12), (8, 9)]);
        let ranks: Vec<(u32, u32)> = s.players.iter().map(|p| (p.rank, p.rank_to)).collect();
        assert_eq!(ranks, [(1, 1), (2, 2), (3, 3), (4, 4), (5, 6), (5, 6), (7, 7)]);

        // Two editions: a double round robin in 2023, four players in 2024.
        let event = event_id(&db, "Club Championship");
        let t = cross_table(&db, event, EditionPick::Latest).unwrap();
        assert_eq!((t.editions.len(), t.edition, t.games, t.players), (2, 1, 6, 4));
        let s = t.standings.unwrap();
        assert_eq!(s.pairing, Pairing::RoundRobin { cycles: 1 });
        assert_eq!(name(&db, s.players[0].player), "Spruce, Sam");
        let s = cross_table(&db, event, EditionPick::Index(0)).unwrap().standings.unwrap();
        assert_eq!(s.pairing, Pairing::RoundRobin { cycles: 2 });
        assert_eq!(ranking(&db, &s), pts(&[("Oak, Olga", "3"), ("Pine, Paul", "2"), ("Rowan, Rita", "1")]));

        // A knock-out whose third-place match (Vale - Willow) is played in the
        // final's round: Vale is third, behind the final's loser on 2 too.
        let s = table(&db, "Summer Cup").standings.unwrap();
        assert_eq!(s.pairing, Pairing::KnockOut);
        assert_eq!(
            places(&db, &s),
            place_list(&[
                ("Thorn, Tess", 1, 1),
                ("Umber, Uma", 2, 2),
                ("Vale, Victor", 3, 3),
                ("Willow, Wes", 4, 4)
            ])
        );
        assert_eq!(
            ranking(&db, &s),
            pts(&[("Thorn, Tess", "3"), ("Umber, Uma", "2"), ("Vale, Victor", "2"), ("Willow, Wes", "1")])
        );
    }

    #[test]
    fn performance_follows_the_fide_table() {
        // Half points of games: 2 of 2 = 100% → +800, 0 → −800, 2 of 4 → 0.
        assert_eq!(performance(2500, 4, 2), Some(3300));
        assert_eq!(performance(2500, 0, 2), Some(1700));
        assert_eq!(performance(2500, 4, 4), Some(2500));
        // 2 of 3 = 67% → +125; 1 of 3 = 33% → −125; 5½ of 9 = 61% → +80.
        assert_eq!(performance(2000, 4, 3), Some(2125));
        assert_eq!(performance(2000, 2, 3), Some(1875));
        assert_eq!(performance(2000, 11, 9), Some(2080));
        assert_eq!(performance(2000, 0, 0), None);
    }

    #[test]
    fn deleted_games_and_unknown_events() {
        let all =
            [pgn("X", "2020.01.01", "1", "A", "B", "1-0"), pgn("X", "2020.01.01", "2", "B", "A", "1-0")]
                .concat();
        let (_d, db) = db_of(&all);
        db.lock().unwrap().set_deleted(1, true).unwrap();
        let t = table(&db, "X");
        assert_eq!(t.games, 1);
        assert!(cross_table(&db, 0, EditionPick::Latest).is_err());
    }

    #[test]
    fn site_is_the_one_most_games_share() {
        let mut all = String::new();
        for site in ["Wijk", "Wijk", "Online"] {
            all += &format!(
                "[Event \"T\"]\n[Site \"{site}\"]\n[Date \"2020.01.01\"]\n[White \"A\"]\n[Black \"B\"]\n[Result \"1-0\"]\n\n1. e4 1-0\n\n"
            );
        }
        let (_d, db) = db_of(&all);
        let t = table(&db, "T");
        assert_eq!(db.lock().unwrap().names(NameKind::Site).get(t.site), "Wijk");
    }
}
