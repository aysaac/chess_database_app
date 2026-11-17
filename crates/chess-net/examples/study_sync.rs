//! NET-13's sync from the command line, as the Sync button does it
//! (`chess_net::study::sync`): what a sync of a database with a Lichess
//! study would do, the sync itself, the games it left for the user and
//! their answers.
//!
//! ```sh
//! # What a sync would do, and then doing it (two-way unless --follow)
//! LICHESS_TOKEN=lio_… cargo run -p chess-net --example study_sync -- plan games.chessdb STUDY_LINK
//! LICHESS_TOKEN=lio_… cargo run -p chess-net --example study_sync -- apply games.chessdb STUDY_LINK
//! # Linking a database to a new private study
//! LICHESS_TOKEN=lio_… cargo run -p chess-net --example study_sync -- apply games.chessdb new "My games"
//! # Its links, and a game waiting for the user settled
//! cargo run -p chess-net --example study_sync -- status games.chessdb
//! LICHESS_TOKEN=lio_… cargo run -p chess-net --example study_sync -- resolve games.chessdb UID mine|theirs|both
//! # Games into a database (made if there's none), and a comment set on a game's first move
//! cargo run -p chess-net --example study_sync -- import games.chessdb games.pgn
//! cargo run -p chess-net --example study_sync -- comment games.chessdb SLOT "text"
//! cargo run -p chess-net --example study_sync -- show games.chessdb SLOT
//! ```
//!
//! `--follow` follows the study only (nothing goes to Lichess);
//! `--delete-chapters` removes on Lichess the chapters of games deleted
//! here. The token comes from `LICHESS_TOKEN` only, `CHESSDB_LICHESS_URL`
//! points it at another server (the stand-in, `examples/lichess_standin.rs`).

use std::path::Path;
use std::process::ExitCode;
use std::sync::Mutex;

use chess_core::pgn::read_pgn_str;
use chess_db::{Database, GameId, NoProgress, SyncMode};
use chess_net::study::sync::{self, Choice, Options, Target};
use chess_net::study::{Visibility, parse_study_link};
use chess_net::{Client, Token};

const USAGE: &str = "usage: study_sync plan|apply DB (STUDY_LINK | new NAME) [--follow] [--delete-chapters]\n       \
                     study_sync status DB | resolve DB UID mine|theirs|both | import DB FILE.pgn | comment DB SLOT TEXT | show DB SLOT";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn open(path: &str) -> Result<Mutex<Database>, Box<dyn std::error::Error>> {
    let p = Path::new(path);
    let db = if p.exists() { Database::open(p)? } else { Database::create(p)? };
    Ok(Mutex::new(db))
}

fn client() -> Client {
    match std::env::var("CHESSDB_LICHESS_URL").ok().filter(|u| !u.trim().is_empty()) {
        Some(url) => Client::with_servers(&url, "https://api.chess.com"),
        None => Client::new(),
    }
}

fn token() -> Option<Token> {
    std::env::var("LICHESS_TOKEN").ok().filter(|t| !t.trim().is_empty()).map(|t| Token::new(&t))
}

fn no_open() -> Vec<GameId> {
    Vec::new()
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let words: Vec<&str> = args.iter().map(String::as_str).filter(|a| !a.starts_with("--")).collect();
    match words.as_slice() {
        [cmd @ ("plan" | "apply"), db, rest @ ..] => {
            let mode = if flag("--follow") { SyncMode::Follow } else { SyncMode::TwoWay };
            let target = match rest {
                ["new", name] => {
                    Target::New { name: (*name).to_owned(), visibility: Visibility::Private, mode }
                }
                [link] => Target::Study { study: parse_study_link(link)?.study, mode },
                _ => return Err(USAGE.into()),
            };
            let db = open(db)?;
            let (client, token) = (client(), token());
            let plan = sync::plan(&client, token.as_ref(), &db, &target, &no_open, &NoProgress)?;
            println!("Study: {} ({})", plan.name, plan.study.as_deref().unwrap_or("new"));
            for i in plan.items.iter().filter(|i| i.step != sync::Step::Unchanged || i.note.is_some()) {
                let note = i.note.as_deref().map(|n| format!(": {n}")).unwrap_or_default();
                println!("  {:?}  {}{note}", i.step, i.name);
            }
            println!(
                "Unchanged: {}; room for {} more chapters",
                plan.count(sync::Step::Unchanged),
                plan.room
            );
            if *cmd == "apply" {
                let options = Options { delete_chapters: flag("--delete-chapters") };
                let o = sync::apply(&client, token.as_ref(), &db, &plan, options, &no_open, &NoProgress)?;
                println!("Applied to {} ({}){}:", o.name, o.study, if o.made { ", made now" } else { "" });
                let mut done: Vec<_> = o.done.iter().collect();
                done.sort_by_key(|(s, _)| format!("{s:?}"));
                for (step, n) in done {
                    println!("  {step:?}: {n}");
                }
                for (name, why) in &o.skipped {
                    println!("  left: {name}: {why}");
                }
                if let Some(e) = &o.stopped {
                    return Err(format!("the sync stopped: {e}").into());
                }
            }
        }
        ["status", db] => {
            let db = open(db)?;
            let mut db = db.lock().expect("db");
            for s in sync::linked_studies(&mut db)? {
                println!(
                    "{} ({}), {:?}: {} games",
                    s.name.as_deref().unwrap_or("?"),
                    s.study,
                    s.mode,
                    s.games
                );
                for (uid, slot, name, kind) in &s.pending {
                    println!("  waits: {uid} (game {slot}) {name}: {kind:?}");
                    if let Some(w) = sync::waiting(&mut db, *uid)? {
                        for q in &w.questions {
                            println!("    {}: here {:?}, Lichess {:?}", q.describe(), q.mine, q.theirs);
                        }
                    }
                }
            }
        }
        ["resolve", db, uid, choice] => {
            let choice = match *choice {
                "mine" => Choice::Mine,
                "theirs" => Choice::Theirs,
                "both" => Choice::Both,
                _ => return Err(USAGE.into()),
            };
            let db = open(db)?;
            let uid = uid.parse().map_err(|_| "not a game id")?;
            let said = sync::resolve(&client(), token().as_ref(), &db, uid, choice, &no_open, &NoProgress)?;
            println!("{said}");
        }
        ["import", db, file] => {
            let db = open(db)?;
            let games = read_pgn_str(&std::fs::read_to_string(file)?);
            let mut db = db.lock().expect("db");
            for g in &games {
                db.append(g)?;
            }
            println!("{} games in, {} in all", games.len(), db.count());
        }
        ["comment", db, slot, text] => {
            let db = open(db)?;
            let mut db = db.lock().expect("db");
            let slot: GameId = slot.parse()?;
            let mut g = db.read_game(slot)?;
            let first = *g.root().children.first().ok_or("the game has no moves")?;
            g.node_mut(first).comment_after = (*text).to_owned();
            db.replace(slot, &g)?;
            println!("set");
        }
        ["show", db, slot] => {
            let db = open(db)?;
            let db = db.lock().expect("db");
            print!("{}", chess_core::pgn::write_game(&db.read_game(slot.parse()?)?));
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}
