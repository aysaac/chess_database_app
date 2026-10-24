//! Downloads a player's games into a database, as the app does (NET-01):
//!
//! ```sh
//! cargo run -p chess-net --example download -- lichess DrNykterstein out.chessdb [SINCE]
//! cargo run -p chess-net --example download -- chess.com hikaru out.chessdb 2026-10-01
//! ```
//!
//! SINCE is a day (`YYYY-MM-DD`, from midnight UTC). The database is made
//! if it doesn't exist; games it has are skipped.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use chess_db::{Database, Progress};
use chess_net::{Client, Period, Request, Site, download_into, time};

struct Print {
    last: Cell<u64>,
}

impl Progress for Print {
    fn report(&self, done: u64, total: u64) {
        let pct = done * 100 / total.max(1);
        if pct != self.last.replace(pct) {
            eprint!("\r{pct:3}%");
        }
    }
    fn is_cancelled(&self) -> bool {
        false
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [site, user, path, rest @ ..] = &args[..] else {
        return Err("usage: download lichess|chess.com USERNAME DB [YYYY-MM-DD]".into());
    };
    let site = match site.as_str() {
        "lichess" => Site::Lichess,
        "chess.com" | "chesscom" => Site::ChessCom,
        other => return Err(format!("unknown site {other}").into()),
    };
    let period = match rest.first() {
        None => Period::All,
        Some(date) => Period::Since(time::date_ms(date).ok_or("SINCE is YYYY-MM-DD")?),
    };
    let path = PathBuf::from(path);
    let db =
        if Database::dir_for(&path).exists() { Database::open(&path)? } else { Database::create(&path)? };
    let db = Mutex::new(db);
    let request = Request { site, username: user.clone(), speeds: Vec::new(), rated_only: false, period };
    let t = Instant::now();
    let s = download_into(&Client::new(), &db, &request, &Print { last: Cell::new(u64::MAX) })?;
    eprintln!();
    println!(
        "{}: {} games added, {} already there, {} skipped, {} warnings, since {}, up to {} in {:.1} s{}",
        s.player.username,
        s.import.imported,
        s.already,
        s.import.skipped,
        s.import.warning_count,
        s.since.map_or("the start".into(), time::ymd),
        s.covered.map_or("nothing".into(), time::ymd),
        t.elapsed().as_secs_f64(),
        s.stopped.map_or(String::new(), |e| format!("; stopped: {e}")),
    );
    for w in s.import.warnings.iter().take(5) {
        println!("  game {}: {}", w.game_index + 1, w.message);
    }
    Ok(())
}
