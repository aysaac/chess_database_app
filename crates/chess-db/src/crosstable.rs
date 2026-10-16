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
//!   (`skipped`). Deleted games are ignored.
//! - **Results.** 1-0, 0-1 and ½-½ count; unfinished games (`*`) are shown
//!   but count for nothing. A decisive game without a move (and not from a
//!   set-up position) is a forfeit: it counts for the points and the
//!   tie-breaks but not for the performance or the average opponent rating,
//!   as in FIDE rating rules.
//! - **Pairing.** Two players: a match. Every player meeting one opponent
//!   per round, with mini-matches of several games: knock-out. At least half
//!   of all pairs met: round robin, as many cycles as games per pair. Else
//!   Swiss when the games have rounds, or no rounds (online arenas).
//! - **Ranking.** Points, then the tie-breaks: Sonneborn-Berger and wins in a
//!   round robin; Buchholz cut 1 and Buchholz in a Swiss or without rounds;
//!   none in a match. Byes and forfeits count for Buchholz as an opponent
//!   with the player's own final score (FIDE's dummy opponent, without its
//!   special cases); rounds missing from the database count for nothing.
//!   Players level on all of them share a rank. A knock-out ranks by the
//!   last round played, then whether the player won that round's match, so
//!   players knocked out in the same round share places (points order them
//!   within).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Mutex;

use crate::database::NameKind;
use crate::record::{FLAG_SETUP, IndexRecord};
use crate::{Database, DbError, GameId};

/// Most players an edition gets a table for. Bigger ones (a month of online
/// games under one name) only get their counts.
pub const MAX_PLAYERS: u32 = 2000;
/// Most games an edition gets a table for.
pub const MAX_GAMES: u32 = 20_000;
/// Most players a round robin has; more are paired some other way.
pub const MAX_ROUND_ROBIN: usize = 64;
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
    let editions: Vec<Edition> = groups
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
    // Games between two players: (lower id, higher id, round).
    let mut pair_games: Vec<(u32, u32, u16)> = Vec::new();

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
            pair_games.push((r.white.min(r.black), r.white.max(r.black), r.round));
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

    // A knock-out's stage: the last round played, and whether the player won
    // that round's match (more points in it than the opponent).
    let stage = |p: u32| -> Vec<u32> {
        let played = || raw[&p].iter().filter(|g| g.opponent.is_some());
        let last = played().map(|g| g.round).max().unwrap_or(0);
        let (own, theirs) = played()
            .filter(|g| g.round == last)
            .filter_map(|g| g.outcome.half_points())
            .fold((0, 0), |(own, theirs), h| (own + h, theirs + 2 - h));
        vec![u32::from(last), u32::from(own > theirs)]
    };
    // Rank: points, then the tie-breaks (a knock-out: its stage); level
    // players by points, Elo, then name.
    let key = |s: &Standing| -> Vec<u32> {
        if pairing == Pairing::KnockOut {
            return stage(s.player);
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
    }
}

/// How `n` players were paired, from the games between two of them
/// (`(lower id, higher id, round)`); see the module docs.
fn detect_pairing(n: usize, has_rounds: bool, pair_games: &[(u32, u32, u16)]) -> Pairing {
    if n <= 2 {
        return Pairing::Match;
    }
    if has_rounds && is_knock_out(pair_games) {
        return Pairing::KnockOut;
    }
    let pairs: HashSet<(u32, u32)> = pair_games.iter().map(|&(a, b, _)| (a, b)).collect();
    let all_pairs = n * (n - 1) / 2;
    if n <= MAX_ROUND_ROBIN && !pairs.is_empty() && pairs.len() * 2 >= all_pairs {
        let cycles = rounded_mean(pair_games.len() as u64, pairs.len() as u64).max(1);
        return Pairing::RoundRobin { cycles: cycles.min(u64::from(u16::MAX)) as u16 };
    }
    if has_rounds { Pairing::Swiss } else { Pairing::Unrounded }
}

/// Every player meets one opponent per round, and at least a quarter of
/// those meetings are mini-matches of several games.
fn is_knock_out(pair_games: &[(u32, u32, u16)]) -> bool {
    let mut opponents: HashMap<(u32, u16), HashSet<u32>> = HashMap::new();
    let mut meetings: HashMap<(u32, u32, u16), u32> = HashMap::new();
    for &(a, b, round) in pair_games.iter().filter(|g| g.2 != 0) {
        opponents.entry((a, round)).or_default().insert(b);
        opponents.entry((b, round)).or_default().insert(a);
        *meetings.entry((a, b, round)).or_default() += 1;
    }
    let several = meetings.values().filter(|&&g| g >= 2).count();
    !meetings.is_empty()
        && opponents.values().all(|o| o.len() == 1)
        && several > 0
        && several * 4 >= meetings.len()
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
    }

    /// `testdata/pgn/crosstable.pgn`: three small tournaments made for these
    /// numbers (the smoke driver `crosstable.js` reads it too).
    #[test]
    fn fixture_tournaments() {
        let dir = tempfile::tempdir().unwrap();
        let db = Mutex::new(Database::create(&dir.path().join("Fixture")).unwrap());
        let pgn =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn/crosstable.pgn");
        assert_eq!(crate::import_pgn(&db, &pgn, &crate::NoProgress).unwrap().imported, 39);
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
