//! Tournament cross-tables (DB-16): the standings of one event, reached
//! from the Events list (DB-04) or from the game on screen.

use chess_db::crosstable::{
    self, CrossTable, EditionPick, MAX_GAMES, MAX_PLAYERS, MAX_ROUND_ROBIN, Outcome, Pairing, TieBreak,
};
use chess_db::record::{unpack_date, unpack_round};
use chess_db::{Database, NameKind};
use tauri::State;

use crate::commands::db::thousands;
use crate::dto::{
    CrossOutcomeDto, CrossResultDto, CrossRowDto, CrossTableDto, EditionDto, PairingDto, TieBreakDto,
};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::state::AppState;

/// The cross-table of an event: `event` (a name id of the Events list), or
/// the event of `game`. `edition` picks one of the event's editions; without
/// it, the one with `game`, else the newest.
#[tauri::command]
pub async fn cross_table(
    state: State<'_, AppState>,
    handle: u32,
    event: Option<u32>,
    game: Option<u32>,
    edition: Option<u32>,
) -> AppResult<CrossTableDto> {
    let db = state.db(handle)?;
    tauri::async_runtime::spawn_blocking(move || {
        let event = match (event, game) {
            (Some(event), _) => event,
            (None, Some(game)) => db.lock().expect("db lock").record(game)?.event,
            (None, None) => return Err(AppError::invalid("No event or game was given for the cross-table.")),
        };
        if event == 0 {
            return Err(AppError::invalid(
                "This game has no event, so there is no cross-table. Add one with Edit details and save the game.",
            ));
        }
        let pick = match (edition, game) {
            (Some(i), _) => EditionPick::Index(i as usize),
            (None, Some(game)) => EditionPick::WithGame(game),
            (None, None) => EditionPick::Latest,
        };
        let table = crosstable::cross_table(&db, event, pick)?;
        let db = db.lock().expect("db lock");
        Ok(to_dto(&db, table))
    })
    .await
    .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))?
}

fn year(date: u32) -> u32 {
    date >> 9
}

fn edition_label(first: u32, last: u32) -> String {
    match (year(first), year(last)) {
        (0, _) => "Year unknown".to_owned(),
        (a, b) if a == b => a.to_string(),
        (a, b) => format!("{a}–{b}"),
    }
}

fn date_range(first: u32, last: u32) -> String {
    match (unpack_date(first), unpack_date(last)) {
        (a, _) if a.is_empty() => String::new(),
        (a, b) if a == b => a,
        (a, b) => format!("{a} – {b}"),
    }
}

fn kind_label(pairing: Pairing) -> String {
    match pairing {
        Pairing::RoundRobin { cycles: 1 } => "Round robin".to_owned(),
        Pairing::RoundRobin { cycles: 2 } => "Double round robin".to_owned(),
        Pairing::RoundRobin { cycles } => format!("Round robin, {cycles} cycles"),
        Pairing::Swiss => "Swiss".to_owned(),
        Pairing::KnockOut => "Knock-out".to_owned(),
        Pairing::Match => "Match".to_owned(),
        Pairing::Unrounded => "No rounds".to_owned(),
    }
}

fn tie_break_dto(t: TieBreak) -> TieBreakDto {
    let (short, title) = match t {
        TieBreak::SonnebornBerger => (
            "SB",
            "Sonneborn-Berger: the scores of the opponents beaten, plus half the scores of those drawn",
        ),
        TieBreak::Wins => ("Wins", "Games won, forfeits included"),
        TieBreak::BuchholzCut1 => ("BH-C1", "Buchholz cut 1: Buchholz without the lowest-scoring opponent"),
        TieBreak::Buchholz => (
            "BH",
            "Buchholz: the opponents' scores added up (a bye or a forfeit counts the player's own score)",
        ),
    };
    TieBreakDto { short: short.to_owned(), title: title.to_owned() }
}

/// A tie-break as the table shows it, in points.
fn tie_break_value(s: &crosstable::Standing, t: TieBreak) -> f64 {
    match t {
        TieBreak::Wins => f64::from(s.wins),
        _ => f64::from(s.tie_break(t)) / 4.0,
    }
}

fn to_dto(db: &Database, t: CrossTable) -> CrossTableDto {
    let players = db.names(NameKind::Player);
    let mut labels: Vec<String> = t.editions.iter().map(|e| edition_label(e.first, e.last)).collect();
    // Two editions in one year: tell them apart by their dates.
    for (i, e) in t.editions.iter().enumerate() {
        if labels.iter().filter(|l| **l == labels[i]).count() > 1 {
            labels[i] = date_range(e.first, e.last);
        }
    }
    let editions =
        t.editions.iter().zip(labels).map(|(e, label)| EditionDto { label, games: e.games }).collect();
    let chosen = t.editions[t.edition];
    let too_big = t.standings.is_none().then(|| {
        format!(
            "This edition has {} players and {} games: too many for a cross-table, which takes up to {} players and {} games. Show games lists them in the game list.",
            thousands(u64::from(t.players)),
            thousands(u64::from(t.games)),
            thousands(u64::from(MAX_PLAYERS)),
            thousands(u64::from(MAX_GAMES)),
        )
    });
    let mut dto = CrossTableDto {
        event_id: t.event,
        event: db.names(NameKind::Event).get(t.event).to_owned(),
        site: db.names(NameKind::Site).get(t.site).to_owned(),
        dates: date_range(chosen.first, chosen.last),
        editions,
        edition: t.edition as u32,
        players: t.players,
        games: t.games,
        too_big,
        pairing: PairingDto::Unrounded,
        cycles: 0,
        kind: String::new(),
        by_round: false,
        slots: Vec::new(),
        unslotted: false,
        tie_breaks: Vec::new(),
        rows: Vec::new(),
        unfinished: 0,
        forfeits: 0,
        byes: 0,
        skipped: 0,
        grid: false,
    };
    let Some(s) = t.standings else { return dto };
    dto.pairing = match s.pairing {
        Pairing::RoundRobin { cycles } => {
            dto.cycles = cycles;
            PairingDto::RoundRobin
        }
        Pairing::Swiss => PairingDto::Swiss,
        Pairing::KnockOut => PairingDto::KnockOut,
        Pairing::Match => PairingDto::Match,
        Pairing::Unrounded => PairingDto::Unrounded,
    };
    dto.kind = kind_label(s.pairing);
    dto.by_round = s.by_round;
    dto.unslotted = s.unslotted;
    dto.tie_breaks = s.tie_breaks.iter().map(|&t| tie_break_dto(t)).collect();
    dto.unfinished = s.unfinished;
    dto.forfeits = s.forfeits;
    dto.byes = s.byes;
    dto.skipped = s.skipped;
    dto.grid = s.players.len() <= MAX_ROUND_ROBIN;
    dto.rows = s
        .players
        .iter()
        .map(|p| CrossRowDto {
            name: players.get(p.player).to_owned(),
            elo: p.elo,
            rank: p.rank,
            rank_to: p.rank_to,
            points: f64::from(p.half_points) / 2.0,
            games: p.games,
            wins: p.wins,
            draws: p.draws,
            losses: p.losses,
            percent: (p.games > 0).then(|| f64::from(p.half_points) * 50.0 / f64::from(p.games)),
            avg_opponent: p.avg_opponent,
            performance: p.performance,
            rated_games: p.rated_games,
            tie_breaks: s.tie_breaks.iter().map(|&t| tie_break_value(p, t)).collect(),
            results: p
                .results
                .iter()
                .map(|r| CrossResultDto {
                    game: r.game,
                    slot: r.slot,
                    round: unpack_round(r.round, r.subround),
                    opponent: r.opponent.map(|o| o as u32),
                    white: r.white,
                    outcome: match r.outcome {
                        Outcome::Win => CrossOutcomeDto::Win,
                        Outcome::Draw => CrossOutcomeDto::Draw,
                        Outcome::Loss => CrossOutcomeDto::Loss,
                        Outcome::Unfinished => CrossOutcomeDto::Unfinished,
                    },
                    forfeit: r.forfeit,
                })
                .collect(),
        })
        .collect();
    dto.slots = s.slots;
    dto
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_db::record::pack_date;

    #[test]
    fn labels() {
        assert_eq!(edition_label(pack_date("2019.06.01"), pack_date("2019.06.09")), "2019");
        assert_eq!(edition_label(pack_date("2019.12.28"), pack_date("2020.01.05")), "2019–2020");
        assert_eq!(edition_label(0, 0), "Year unknown");
        assert_eq!(date_range(pack_date("2019.06.01"), pack_date("2019.06.09")), "2019.06.01 – 2019.06.09");
        assert_eq!(date_range(pack_date("2019.06.01"), pack_date("2019.06.01")), "2019.06.01");
        assert_eq!(date_range(0, 0), "");
        assert_eq!(kind_label(Pairing::RoundRobin { cycles: 2 }), "Double round robin");
        assert_eq!(kind_label(Pairing::RoundRobin { cycles: 4 }), "Round robin, 4 cycles");
    }
}
