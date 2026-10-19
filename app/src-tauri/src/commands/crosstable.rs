//! Tournament cross-tables (DB-16): the standings of one event, reached
//! from the Events list (DB-04) or from the game on screen.

use std::sync::Mutex;

use chess_db::crosstable::{
    self, CrossTable, Edition, EditionPick, MAX_GAMES, MAX_PLAYERS, MAX_ROUND_ROBIN, Outcome, Pairing,
    TieBreak,
};
use chess_db::record::{unpack_date, unpack_round};
use chess_db::{Database, NameKind, NoProgress, games_with_name};
use tauri::State;

use crate::commands::db::thousands;
use crate::dto::{
    CrossOutcomeDto, CrossResultDto, CrossRowDto, CrossTableDto, DbInfoDto, EditionDto, NameKindDto,
    PairingDto, TieBreakDto,
};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::settings::FilterStep;
use crate::state::{ActiveFilter, AppState};

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

/// Show games from a cross-table: limits the game list to the games of
/// edition `edition` of `event` (all of the event's when it has one).
#[tauri::command]
pub async fn show_edition_games(
    state: State<'_, AppState>,
    handle: u32,
    event: u32,
    edition: u32,
) -> AppResult<DbInfoDto> {
    let db = state.db(handle)?;
    let filter = tauri::async_runtime::spawn_blocking(move || edition_filter(&db, event, edition))
        .await
        .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))??;
    state.set_filter(handle, filter);
    crate::commands::db::db_info(state, handle).await
}

/// The filter Show games makes: the event's games when it has one edition,
/// saved as usual; else the edition's, kept for this session only (no
/// steps, so it isn't made again on reopening). A saved filter step only
/// the newer builds know would make older ones drop all the settings.
fn edition_filter(db: &Mutex<Database>, event: u32, edition: u32) -> AppResult<ActiveFilter> {
    let name = db.lock().expect("db lock").names(NameKind::Event).get(event).to_owned();
    let mut editions = crosstable::edition_games(db, event)?;
    if editions.len() == 1 {
        let ids = games_with_name(db, NameKind::Event, event, &NoProgress)?;
        let step = FilterStep::Name { table: NameKindDto::Event, name: name.clone() };
        return Ok(ActiveFilter::new(ids, format!("Event: {name}"), vec![step]));
    }
    let labels = edition_labels(&editions.iter().map(|(e, _)| *e).collect::<Vec<_>>());
    let i = (edition as usize).min(editions.len() - 1);
    let ids = editions.swap_remove(i).1;
    Ok(ActiveFilter::new(ids, format!("Event: {name} ({})", labels[i]), Vec::new()))
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

/// Why a table has no results, only the standings (over `MAX_CELLS`).
fn standings_only(s: &crosstable::Standings) -> String {
    let players = thousands(s.players.len() as u64);
    let columns = thousands((s.slots.len() + usize::from(s.unslotted)) as u64);
    let what = if s.by_round {
        format!("{players} players over {columns} rounds are too many to show round by round")
    } else {
        format!("{players} players with up to {columns} games each are too many to show game by game")
    };
    format!("Standings only: {what}. Show games lists the games in the game list.")
}

/// The editions' names for the picker: their years, or their dates where
/// two editions share years.
fn edition_labels(editions: &[Edition]) -> Vec<String> {
    let years: Vec<String> = editions.iter().map(|e| edition_label(e.first, e.last)).collect();
    editions
        .iter()
        .zip(&years)
        .map(|(e, label)| {
            if years.iter().filter(|l| *l == label).count() > 1 {
                date_range(e.first, e.last)
            } else {
                label.clone()
            }
        })
        .collect()
}

fn to_dto(db: &Database, t: CrossTable) -> CrossTableDto {
    let players = db.names(NameKind::Player);
    let editions = t
        .editions
        .iter()
        .zip(edition_labels(&t.editions))
        .map(|(e, label)| EditionDto { label, games: e.games })
        .collect();
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
        duplicates: 0,
        level_matches: 0,
        grid: false,
        per_game: false,
        standings_only: None,
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
    dto.duplicates = s.duplicates;
    dto.level_matches = s.level_matches;
    dto.per_game = s.per_game;
    dto.grid = s.per_game && s.players.len() <= MAX_ROUND_ROBIN;
    dto.standings_only = (!s.per_game).then(|| standings_only(&s));
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

    /// Show games (R1-5 of the review) of one edition of an event that has
    /// several keeps its filter for the session only: settings.json written
    /// with it stays readable by builds that know only the header, name and
    /// position steps (main at 9d846bb), which drop every setting when one
    /// isn't, and has no filter to restore (R2-3). An event with one edition
    /// is saved as its event's filter.
    #[test]
    fn an_edition_filter_keeps_settings_readable_by_older_builds() {
        /// `FilterStep` as main at 9d846bb has it.
        #[derive(Debug, serde::Deserialize, PartialEq)]
        #[serde(tag = "kind", rename_all = "camelCase")]
        enum MainStep {
            #[serde(rename_all = "camelCase")]
            Header {
                query: serde_json::Value,
                #[serde(default)]
                quick: bool,
            },
            #[serde(rename_all = "camelCase")]
            Name { table: NameKindDto, name: String },
            #[serde(rename_all = "camelCase")]
            Position {
                fen: String,
                #[serde(default)]
                full: bool,
            },
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct MainFilter {
            steps: Vec<MainStep>,
            description: String,
        }
        #[derive(serde::Deserialize)]
        struct MainView {
            filter: Option<MainFilter>,
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct MainSettings {
            database_views: Vec<MainView>,
        }

        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let state = AppState::default();
        state.init_dirs(config.clone(), dir.path().join("data"));
        let handle =
            state.add_db(Database::create(&dir.path().join("Editions")).unwrap(), crate::state::DbUse::List);
        let db = state.db(handle).unwrap();
        let pgn = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn/crosstable.pgn");
        chess_db::import_pgn(&db, &pgn, &NoProgress).unwrap();
        let event = |name: &str| db.lock().unwrap().names(NameKind::Event).id_of(name).unwrap();

        let club = edition_filter(&db, event("Club Championship"), 0).unwrap();
        let next = edition_filter(&db, event("Club Championship"), 1).unwrap();
        assert_eq!((club.ids.len(), next.ids.len()), (6, 6));
        assert!(club.ids.iter().all(|id| !next.ids.contains(id)), "two editions, no game in both");
        assert_eq!(club.description, "Event: Club Championship (2023)");
        assert!(club.steps.is_empty(), "not made again on reopening");
        let spring = edition_filter(&db, event("Spring Round Robin"), 0).unwrap();
        assert_eq!(spring.ids.len(), 15);
        let name = FilterStep::Name { table: NameKindDto::Event, name: "Spring Round Robin".into() };
        assert_eq!(spring.steps, [name]);

        // As the app saves them, read with main's steps.
        let read_by_main = || -> MainSettings {
            let json = std::fs::read(config.join("settings.json")).unwrap();
            serde_json::from_slice(&json).expect("main reads the settings")
        };
        state.set_filter(handle, spring);
        let main = read_by_main();
        let saved = main.database_views[0].filter.as_ref().expect("the event's filter saved");
        let name = MainStep::Name { table: NameKindDto::Event, name: "Spring Round Robin".into() };
        assert_eq!(
            (saved.description.as_str(), saved.steps.as_slice()),
            ("Event: Spring Round Robin", [name].as_slice())
        );
        state.set_filter(handle, club);
        assert!(read_by_main().database_views[0].filter.is_none(), "the edition's filter isn't saved");
        assert_eq!(crate::settings::Settings::load(&config).database_views.len(), 1);
    }

    fn edition(first: &str, last: &str) -> Edition {
        Edition { first: pack_date(first), last: pack_date(last), games: 3 }
    }

    /// R1-7 (review of DB-16): every edition sharing a year with another is
    /// named by its dates, the last one too.
    #[test]
    fn editions_in_one_year_are_named_by_their_dates() {
        let twice = [edition("2020.01.05", "2020.01.07"), edition("2020.11.05", "2020.11.07")];
        assert_eq!(edition_labels(&twice), ["2020.01.05 – 2020.01.07", "2020.11.05 – 2020.11.07"]);
        let three = [
            edition("2019.06.01", "2019.06.09"),
            edition("2020.01.05", "2020.01.07"),
            edition("2020.11.05", "2020.11.07"),
            Edition { first: 0, last: 0, games: 1 },
        ];
        assert_eq!(
            edition_labels(&three),
            ["2019", "2020.01.05 – 2020.01.07", "2020.11.05 – 2020.11.07", "Year unknown"]
        );
    }
}
