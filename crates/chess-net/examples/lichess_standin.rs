//! A server standing in for Lichess's OAuth and study API, for smoke runs
//! of the app (NET-11, IO-10, NET-12): the same one the tests use
//! (`tests/standin/mod.rs`). Point the app at it with
//! `CHESSDB_LICHESS_URL` (debug builds) and let its login through with
//! `CHESSDB_BROWSER=approve`:
//!
//! ```sh
//! cargo run -p chess-net --example lichess_standin -- 18431 &
//! CHESSDB_LICHESS_URL=http://127.0.0.1:18431 CHESSDB_BROWSER=approve scripts/smoke/run.sh scripts/smoke/studies.js
//! ```
//!
//! The browser logs in as `SmokeUser`, who has a public study of the
//! fixture `testdata/pgn/lichess-study.pgn` (`FxStudy1`) and a private one
//! (`Private1`). Lichess's daily limit on new studies is 3 here, so a run
//! can reach it. Every request is printed.

#[path = "../tests/standin/mod.rs"]
mod standin;

use std::path::PathBuf;
use std::time::Duration;

fn main() {
    let port: u16 = std::env::args().nth(1).and_then(|p| p.parse().ok()).unwrap_or(0);
    let server = standin::Lichess::start(port);
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn/lichess-study.pgn");
    let pgn = std::fs::read_to_string(fixture).expect("the fixture");
    let chapters: Vec<&str> = chess_net::study::split_games(&pgn).into_iter().map(str::trim).collect();
    server.add_study("FxStudy1", "Fixture study", "SmokeUser", "public", &chapters);
    server.add_study("Private1", "My private lines", "SmokeUser", "private", &chapters[..2]);
    {
        let mut st = server.state.lock().expect("state");
        st.approve_as = "SmokeUser".into();
        st.daily_limit = 3;
    }
    println!("ready {}", server.url);
    let mut seen = 0;
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let requests = server.requests();
        for r in &requests[seen..] {
            println!("{r}");
        }
        seen = requests.len();
    }
}
