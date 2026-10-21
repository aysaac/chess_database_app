//! The Reference tab (OPEN-14) for the position on screen: the moves with
//! Hot and Best players and the top games (`reference_view`, fast), the
//! main line and lines (`reference_lines`, a job for common positions), and
//! a game's main line for the mini board (`game_line`).

use chess_core::notation::{san, uci};
use chess_core::openings;
use chess_core::position;
use chess_core::shakmaty::{Chess, Move, Position};
use chess_db::reference::LINES_JOB_GAMES;
use chess_db::{
    Line, NoProgress, ReferenceLines, ReferenceOverview, find_position, game_mainline,
    reference_lines as count_lines, reference_overview,
};
use tauri::{AppHandle, Manager, State};

use crate::commands::db::{thousands, to_job_error};
use crate::commands::tree::{beyond_index, position_and_setup, position_of, side_score, tree_row};
use crate::dto::{
    BestPlayerDto, GameLineDto, HotDto, LinePositionDto, LinesDto, OpeningDto, ReferenceDto,
    ReferenceLineDto, ReferenceLinesDto, ReferenceMoveDto, TopGameDto,
};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::state::{AppState, LinesKey};

/// What the Best players column shows of a name: the surname of a
/// "Surname, First name", else the whole name (an online handle).
fn surname(name: &str) -> String {
    name.split(',').next().unwrap_or(name).trim().to_owned()
}

pub(crate) fn year_of(date: &str) -> Option<u16> {
    date.get(..4).and_then(|y| y.parse().ok()).filter(|&y| y > 0)
}

fn internal(e: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Internal, e.to_string())
}

fn view_dto(pos: &Chess, o: ReferenceOverview, indexed: bool, beyond_index: bool) -> ReferenceDto {
    let white = pos.turn().is_white();
    let moves = o
        .moves
        .iter()
        .map(|m| ReferenceMoveDto {
            row: tree_row(&m.stats, white, m.mv.map(|mv| (uci(mv), san(pos, mv)))),
            hot: m.hot.as_ref().map(|h| HotDto {
                recent: h.recent,
                share: h.share,
                all_share: h.all_share,
                trend: h.trend.map(Into::into),
            }),
            best: m
                .best
                .iter()
                .map(|p| BestPlayerDto { name: p.name.clone(), surname: surname(&p.name), elo: p.elo })
                .collect(),
        })
        .collect();
    let top_games = o
        .top_games
        .into_iter()
        .map(|g| TopGameDto {
            id: g.row.id,
            year: year_of(&g.row.date),
            white: g.row.white,
            white_elo: g.row.white_elo,
            black: g.row.black,
            black_elo: g.row.black_elo,
            result: g.row.result.as_str().to_owned(),
            event: g.row.event,
            ply: g.ply,
        })
        .collect();
    ReferenceDto {
        fen: position::to_fen(pos),
        opening: openings::at_position(pos).map(OpeningDto::from),
        total: tree_row(&o.total, white, None),
        moves,
        recent_from: o.recent_years.map(|(from, _)| from),
        recent_to: o.recent_years.map(|(_, to)| to),
        top_games,
        indexed,
        partial: o.partial,
        beyond_index,
        max_ply: o.max_ply,
    }
}

/// The Reference tab's moves (with Hot and Best players) and top games for
/// the position on screen in `session`, from database `handle`. Fast at any
/// position: it reads the position index and the per-game stats.
#[tauri::command]
pub async fn reference_view(
    state: State<'_, AppState>,
    handle: u32,
    session: u32,
) -> AppResult<ReferenceDto> {
    let (pos, set_up) = position_and_setup(&state, session)?;
    let db = state.db(handle)?;
    let overview = tauri::async_runtime::spawn_blocking({
        let (db, pos) = (db.clone(), pos.clone());
        move || reference_overview(&db, &pos)
    })
    .await
    .map_err(internal)??;
    let indexed = db.lock().expect("db lock").positions().is_built();
    let beyond = beyond_index(&pos, set_up, overview.max_ply);
    Ok(view_dto(&pos, overview, indexed, beyond))
}

/// The positions of `moves` from `start`: `start` itself, then one per move.
pub(crate) fn positions(start: &Chess, moves: &[Move]) -> Vec<LinePositionDto> {
    let mut pos = start.clone();
    let mut out = Vec::with_capacity(moves.len() + 1);
    out.push(LinePositionDto {
        fen: position::to_fen(&pos),
        san: None,
        uci: None,
        ply: 0,
        move_number: 0,
        white: false,
    });
    for (i, &m) in moves.iter().enumerate() {
        let (move_number, white) = (pos.fullmoves().get(), pos.turn().is_white());
        let san = san(&pos, m);
        pos.play_unchecked(m);
        out.push(LinePositionDto {
            fen: position::to_fen(&pos),
            san: Some(san),
            uci: Some(uci(m)),
            ply: i as u32 + 1,
            move_number,
            white,
        });
    }
    out
}

fn line_dto(pos: &Chess, line: &Line) -> ReferenceLineDto {
    let s = &line.stats;
    ReferenceLineDto {
        positions: positions(pos, &line.moves),
        games: s.games,
        white_wins: s.white_wins,
        draws: s.draws,
        black_wins: s.black_wins,
        score: side_score(s.white_wins, s.draws, s.black_wins, pos.turn().is_white()),
        last_year: (s.last_year > 0).then_some(s.last_year),
        avg_elo: (s.elo_count > 0).then(|| (s.elo_sum / u64::from(s.elo_count)) as u32),
    }
}

fn lines_dto(pos: &Chess, lines: &ReferenceLines) -> LinesDto {
    LinesDto {
        fen: position::to_fen(pos),
        games: lines.games,
        main_line: lines.main_line.as_ref().map(|l| line_dto(pos, l)),
        lines: lines.lines.iter().map(|l| line_dto(pos, l)).collect(),
    }
}

/// The main line and lines (OPEN-14) for the position on screen in
/// `session`, from database `handle`. They're counted from every game
/// reaching the position, so from about 50,000 games on this starts a job
/// (with progress and Cancel) and returns it: ask again when it's done.
/// Results are cached until the database changes.
#[tauri::command]
pub async fn reference_lines(
    app: AppHandle,
    state: State<'_, AppState>,
    handle: u32,
    session: u32,
) -> AppResult<ReferenceLinesDto> {
    let pos = position_of(&state, session)?;
    let db = state.db(handle)?;
    let key = LinesKey::new(handle, position::hash(&pos), &db.lock().expect("db lock"));
    {
        let mut cache = state.lines.lock().expect("lines lock");
        if let Some(lines) = cache.get(&key) {
            return Ok(ReferenceLinesDto::Ready { lines: Box::new(lines) });
        }
        if let Some((job, games)) = cache.running(&key, &state.jobs) {
            return Ok(ReferenceLinesDto::Job { job, games });
        }
    }
    let hits = tauri::async_runtime::spawn_blocking({
        let (db, pos) = (db.clone(), pos.clone());
        move || find_position(&db, &pos)
    })
    .await
    .map_err(internal)??;
    let games = hits.entries.len();
    if games < LINES_JOB_GAMES {
        let lines = tauri::async_runtime::spawn_blocking({
            let pos = pos.clone();
            move || count_lines(&db, &pos, &hits, &NoProgress)
        })
        .await
        .map_err(internal)??;
        let dto = lines_dto(&pos, &lines);
        state.lines.lock().expect("lines lock").put(key, dto.clone());
        return Ok(ReferenceLinesDto::Ready { lines: Box::new(dto) });
    }
    // Checked again and registered under one lock, so two calls for the
    // same position (the board went there and back) share one job.
    let mut cache = state.lines.lock().expect("lines lock");
    if let Some(lines) = cache.get(&key) {
        return Ok(ReferenceLinesDto::Ready { lines: Box::new(lines) });
    }
    if let Some((job, games)) = cache.running(&key, &state.jobs) {
        return Ok(ReferenceLinesDto::Job { job, games });
    }
    let label = format!("Counting the lines of {} games", thousands(games as u64));
    let job = state.jobs.spawn("reference", label, move |ctx| {
        let lines = count_lines(&db, &pos, &hits, ctx).map_err(to_job_error)?;
        let dto = lines_dto(&pos, &lines);
        app.state::<AppState>().lines.lock().expect("lines lock").put(key, dto);
        Ok(format!("Counted the lines of {} games", thousands(u64::from(lines.games))))
    });
    cache.add_job(key, job, games as u32);
    Ok(ReferenceLinesDto::Job { job, games: games as u32 })
}

/// A game's main line as positions, for the Reference tab's mini board,
/// with where it reaches the position on screen in `session`.
#[tauri::command]
pub async fn game_line(
    state: State<'_, AppState>,
    handle: u32,
    game: u32,
    session: u32,
) -> AppResult<GameLineDto> {
    line_of_game(&state, handle, game, session)
}

fn line_of_game(state: &AppState, handle: u32, game: u32, session: u32) -> AppResult<GameLineDto> {
    let target = position::hash(&position_of(state, session)?);
    let db = state.db(handle)?;
    let db = db.lock().expect("db lock");
    let row = db.row(game)?;
    let (start, moves) = game_mainline(&db, game)?;
    drop(db);
    let positions = positions(&start, &moves);
    // The same position as the board's, as the index compares them.
    let mut pos = start;
    let mut at = (position::hash(&pos) == target).then_some(0);
    for (i, &m) in moves.iter().enumerate() {
        if at.is_some() {
            break;
        }
        pos.play_unchecked(m);
        if position::hash(&pos) == target {
            at = Some(i as u32 + 1);
        }
    }
    Ok(GameLineDto {
        id: row.id,
        year: year_of(&row.date),
        white: row.white,
        white_elo: row.white_elo,
        black: row.black,
        black_elo: row.black_elo,
        result: row.result.as_str().to_owned(),
        event: row.event,
        positions,
        start: at.unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{DbUse, Session};
    use chess_core::game::{Game, GameEditor};
    use chess_core::pgn::read_pgn_str;
    use chess_db::{Database, update_position_index};

    #[test]
    fn surnames() {
        assert_eq!(surname("Carlsen, Magnus"), "Carlsen");
        assert_eq!(surname("Van Wely, Loek"), "Van Wely");
        assert_eq!(surname("DrNykterstein"), "DrNykterstein");
        assert_eq!(surname(" Kasparov,G "), "Kasparov");
    }

    #[test]
    fn line_positions_number_the_moves() {
        let mut pos = Chess::default();
        let e4 = position::parse_uci(&pos, "e2e4").unwrap();
        pos.play_unchecked(e4);
        let moves: Vec<Move> = ["e7e5", "g1f3"]
            .iter()
            .scan(pos.clone(), |p, u| {
                let m = position::parse_uci(p, u).unwrap();
                p.play_unchecked(m);
                Some(m)
            })
            .collect();
        let out = positions(&pos, &moves);
        assert_eq!(out.len(), 3);
        assert_eq!((out[0].san.as_deref(), out[0].ply), (None, 0));
        assert_eq!(out[0].fen, position::to_fen(&pos));
        let labels: Vec<(u32, bool, &str, &str)> = out[1..]
            .iter()
            .map(|p| (p.move_number, p.white, p.san.as_deref().unwrap(), p.uci.as_deref().unwrap()))
            .collect();
        assert_eq!(labels, [(1, false, "e5", "e7e5"), (2, true, "Nf3", "g1f3")]);
        assert_eq!(out[2].ply, 2);
    }

    /// Two games in a fresh, indexed database and a session at 1.e4 e5.
    fn setup() -> (tempfile::TempDir, AppState, u32, u32) {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::default();
        let mut db = Database::create(&dir.path().join("Ref")).unwrap();
        let pgn = "[White \"Carlsen, Magnus\"][Black \"B\"][WhiteElo \"2850\"][Date \"2019.01.01\"][Result \"1-0\"]\n\
                   1. e4 e5 2. Nf3 Nc6 1-0\n\n[White \"C\"][Black \"D\"][Result \"0-1\"]\n1. Nf3 Nc6 2. e4 e5 3. Bc4 0-1\n";
        for g in read_pgn_str(pgn) {
            db.append(&g).unwrap();
        }
        let handle = state.add_db(db, DbUse::Reference);
        update_position_index(&state.db(handle).unwrap(), &NoProgress).unwrap();
        let mut editor = GameEditor::new(Game::new(Default::default(), None));
        for u in ["e2e4", "e7e5"] {
            let pos = editor.game().position_at(editor.cursor());
            editor.play(position::parse_uci(&pos, u).unwrap(), None);
        }
        let session = state.add_session(Session::new(editor, None)).unwrap();
        (dir, state, handle, session)
    }

    #[test]
    fn a_games_line_starts_where_it_reaches_the_board() {
        let (_d, state, handle, session) = setup();
        let line = line_of_game(&state, handle, 0, session).unwrap();
        assert_eq!((line.start, line.white.as_str(), line.year), (2, "Carlsen, Magnus", Some(2019)));
        assert_eq!(line.positions.len(), 5);
        assert_eq!(line.positions[2].san.as_deref(), Some("e5"));
        // 1.Nf3 Nc6 2.e4 e5 never has the board's position after 1.e4 e5
        // (it transposes only after 2.Nf3 Nc6): it starts at the start.
        let line = line_of_game(&state, handle, 1, session).unwrap();
        assert_eq!((line.start, line.positions.len(), line.year), (0, 6, None));
        assert!(line_of_game(&state, handle, 2, session).is_err(), "no such game");
    }

    #[test]
    fn lines_are_cached_until_the_database_changes() {
        let (_d, state, handle, _session) = setup();
        let db = state.db(handle).unwrap();
        let pos = Chess::default();
        let key = LinesKey::new(handle, position::hash(&pos), &db.lock().unwrap());
        let hits = find_position(&db, &pos).unwrap();
        let lines = lines_dto(&pos, &count_lines(&db, &pos, &hits, &NoProgress).unwrap());
        // One first move each: 1.e4 and 1.Nf3, one game each.
        assert_eq!(lines.games, 2);
        let firsts: Vec<_> = lines.lines.iter().map(|l| l.positions[1].san.clone().unwrap()).collect();
        assert_eq!(firsts, ["e4", "Nf3"]);
        assert_eq!(lines.lines[0].score, 1.0, "White won the 1.e4 game");
        state.lines.lock().unwrap().put(key, lines);
        assert!(state.lines.lock().unwrap().get(&key).is_some());

        // A new game changes the key.
        db.lock().unwrap().append(&read_pgn_str("1. d4 *").remove(0)).unwrap();
        let changed = LinesKey::new(handle, position::hash(&pos), &db.lock().unwrap());
        assert_ne!(changed, key);
        assert!(state.lines.lock().unwrap().get(&changed).is_none());

        // The oldest go once there are too many.
        let mut cache = state.lines.lock().unwrap();
        for i in 0..40 {
            let k = LinesKey { position: i, ..key };
            cache.put(k, LinesDto { fen: String::new(), games: 0, main_line: None, lines: Vec::new() });
        }
        assert!(cache.get(&key).is_none(), "evicted");
        assert!(cache.get(&LinesKey { position: 39, ..key }).is_some());
    }
}
