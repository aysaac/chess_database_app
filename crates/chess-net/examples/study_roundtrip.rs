//! The study round-trip check (NET-13), as `chess_net::study::roundtrip`
//! runs it: what Lichess keeps, changes or drops of annotated games sent to
//! a study and fetched back, and of a chapter's moves replaced in place
//! (`POST /api/study/{id}/{chapter}/moves`). The games and chapters are
//! compared as move trees, feature by feature.
//!
//! ```sh
//! # Against lichess.org, with a token that can read and write studies
//! LICHESS_TOKEN=lio_… cargo run -p chess-net --example study_roundtrip
//! # Another PGN file, and the PGN of each step and the report kept in a folder
//! LICHESS_TOKEN=lio_… cargo run -p chess-net --example study_roundtrip -- games.pgn --out target/roundtrip
//! # Against the stand-in Lichess (examples/lichess_standin.rs), which keeps everything
//! cargo run -p chess-net --example lichess_standin -- 18431 &
//! CHESSDB_LICHESS_URL=http://127.0.0.1:18431 LICHESS_TOKEN=lio_standin_roundtrip \
//!     cargo run -p chess-net --example study_roundtrip
//! ```
//!
//! The games are `testdata/pgn/study-roundtrip.pgn` unless a file is given
//! (64 at most, each read without a warning). The token comes only from
//! `LICHESS_TOKEN` (make one at
//! <https://lichess.org/account/oauth/token/create?scopes[]=study:read&scopes[]=study:write>);
//! nothing else is read for it. `CHESSDB_LICHESS_URL` points it at another
//! server. It makes one new private study (Lichess makes 30 a day per
//! account), sends one request at a time, and leaves the study on the
//! account. It fails (exit code 1) only on errors, not on differences:
//! those are what the report is for. Once the study is made, an error still
//! prints (and saves) the report so far, with the study's link.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chess_core::pgn::PgnReader;
use chess_db::NoProgress;
use chess_net::Client;
use chess_net::oauth::{Token, account};
use chess_net::study::MAX_CHAPTERS;
use chess_net::study::roundtrip::{RoundTrip, round_trip};
use chess_net::time::{now_ms, ymd};

const USAGE: &str = "usage: study_roundtrip [FILE.pgn] [--out FOLDER]   (LICHESS_TOKEN must be set)";

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let (mut file, mut out) = (None, None);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(args.next().ok_or("--out needs a folder")?)),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            _ if file.is_none() && !arg.starts_with('-') => file = Some(PathBuf::from(arg)),
            _ => return Err(USAGE.into()),
        }
    }
    let file = file.unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn/study-roundtrip.pgn")
    });
    let token = std::env::var("LICHESS_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty())
        .map(|t| Token::new(&t))
        .ok_or("set LICHESS_TOKEN to a Lichess token with study:read and study:write")?;
    let client = match std::env::var("CHESSDB_LICHESS_URL").ok().filter(|u| !u.trim().is_empty()) {
        Some(url) => Client::with_servers(&url, "https://api.chess.com"),
        None => Client::new(),
    };

    // Games our own reader warns about would blur what Lichess did.
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut games = Vec::new();
    for parsed in PgnReader::new(text.as_bytes()) {
        let parsed = parsed?;
        if let Some(w) = parsed.warnings.first() {
            return Err(format!("{}, game {}: {}", file.display(), parsed.index + 1, w.message).into());
        }
        games.extend(parsed.game);
    }
    if games.is_empty() {
        return Err(format!("{} has no games", file.display()).into());
    }
    if games.len() > MAX_CHAPTERS {
        return Err(
            format!("{} has {} games: a study holds {MAX_CHAPTERS}", file.display(), games.len()).into()
        );
    }

    println!("Lichess: {}", client.lichess_url());
    println!("Games: {} from {}", games.len(), file.display());
    println!("Logged in as {}", account(&client, &token, &|| false)?);
    let name = format!("Round-trip check {}", ymd(now_ms()));
    let report = round_trip(&client, &token, &games, &name, &NoProgress)?;
    println!();
    print!("{report}");
    if let Some(dir) = &out {
        save(dir, &report)?;
        println!("The PGN of each step and this report are in {}", dir.display());
    }
    if let Some(e) = &report.stopped {
        eprintln!("error: the check stopped: {e}");
        return Ok(ExitCode::FAILURE);
    }
    if let Some(e) = &report.push_error {
        eprintln!("error: replacing the chapter's moves failed: {e}");
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}

/// The report and the PGN of each step, in `dir`.
fn save(dir: &Path, report: &RoundTrip) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let t = &report.texts;
    let report = report.to_string();
    for (name, text) in [
        ("report.txt", &report),
        ("1-sent.pgn", &t.sent),
        ("2-back.pgn", &t.back),
        ("3-pushed.pgn", &t.pushed),
        ("4-pushed-back.pgn", &t.pushed_back),
    ] {
        if text.is_empty() {
            continue;
        }
        // A temp file renamed over the old one, closed before the rename.
        let path = dir.join(name);
        let temp = dir.join(format!("{name}.tmp"));
        {
            let mut f = std::fs::File::create(&temp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        std::fs::rename(&temp, &path)?;
    }
    Ok(())
}
