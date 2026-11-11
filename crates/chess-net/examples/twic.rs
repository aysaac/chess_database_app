//! The TWIC client (NET-06) against the real site: finds the newest issue
//! and downloads one (the newest, or the one given), saying how many games
//! it holds. One page and one zip: be polite.
//!
//!     cargo run -p chess-net --example twic -- [ISSUE]

use chess_net::twic::Twic;

fn main() {
    let twic = Twic::new();
    let latest = match twic.latest_issue() {
        Ok(n) => n,
        Err(e) => {
            eprintln!("the newest issue: {e}");
            std::process::exit(1);
        }
    };
    println!("newest issue: {latest}");
    let n = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(latest);
    match twic.issue_pgn(n) {
        Ok(pgn) => {
            let text = String::from_utf8_lossy(&pgn);
            let games = text.lines().filter(|l| l.starts_with("[Event ")).count();
            println!("issue {n}: {} bytes of PGN, {games} games", pgn.len());
        }
        Err(e) => {
            eprintln!("issue {n}: {e}");
            std::process::exit(1);
        }
    }
}
