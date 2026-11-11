//! Lichess studies from the command line, as the app does them (IO-10,
//! NET-12), for checks against the real server:
//!
//! ```sh
//! # A study's chapters into a database (made if it doesn't exist); a
//! # private or unlisted one needs a token with study:read
//! cargo run -p chess-net --example study -- import https://lichess.org/study/abcd1234 out.chessdb
//! # The games of a PGN file to a new private study (a token with study:write)
//! LICHESS_TOKEN=lio_… cargo run -p chess-net --example study -- send games.pgn "Study name"
//! # The user's studies
//! LICHESS_TOKEN=lio_… cargo run -p chess-net --example study -- list USERNAME
//! ```
//!
//! The token comes from `LICHESS_TOKEN` (make one at
//! <https://lichess.org/account/oauth/token/create?scopes[]=study:read&scopes[]=study:write>).
//! A round trip: `send` a few annotated games, `import` the study it made
//! into a new database, and compare (`chess_db`'s export, or the app).

use std::path::PathBuf;
use std::sync::Mutex;

use chess_db::{Database, NoProgress};
use chess_net::Client;
use chess_net::oauth::{Token, account};
use chess_net::study::{
    ExportTarget, Visibility, download_study_into, export_to_studies, parse_study_link, split_games,
    studies_of,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let token = std::env::var("LICHESS_TOKEN").ok().map(|t| Token::new(&t));
    let client = Client::new();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["import", link, path] => {
            let link = parse_study_link(link)?;
            let path = PathBuf::from(path);
            let db = if Database::dir_for(&path).exists() {
                Database::open(&path)?
            } else {
                Database::create(&path)?
            };
            let db = Mutex::new(db);
            let s = download_study_into(&client, &db, &link, token.as_ref(), None, &NoProgress)?;
            println!(
                "{}: {} chapters, {} imported, {} already there, {} skipped, {} warnings",
                s.name, s.chapters, s.import.imported, s.already, s.import.skipped, s.import.warning_count
            );
            for w in s.import.warnings.iter().take(10) {
                println!("  chapter {}: {}", w.game_index + 1, w.message);
            }
        }
        ["send", file, name] => {
            let token = token.ok_or("set LICHESS_TOKEN to a token with study:write")?;
            println!("logged in as {}", account(&client, &token, &|| false)?);
            let pgn = std::fs::read_to_string(file)?;
            let games: Vec<String> = split_games(&pgn).into_iter().map(|g| g.trim().to_owned()).collect();
            let target = ExportTarget::New { name: name.to_owned(), visibility: Visibility::Private };
            let s = export_to_studies(&client, &token, &target, &games, &NoProgress)?;
            for st in &s.studies {
                println!("{}: {} games sent, {} chapters made: {}", st.name, st.sent, st.chapters, st.url);
                if let Some(r) = &st.refused {
                    println!("  Lichess said: {r}");
                }
            }
            if let Some(e) = &s.stopped {
                println!("stopped: {e} ({} games not sent)", s.not_sent());
            }
        }
        ["list", user] => {
            for s in studies_of(&client, user, token.as_ref(), &|| false)? {
                println!("{}  {}  (updated {})", s.id, s.name, chess_net::time::ymd(s.updated_at));
            }
        }
        _ => return Err("usage: study import LINK DB | send FILE.pgn NAME | list USERNAME".into()),
    }
    Ok(())
}
