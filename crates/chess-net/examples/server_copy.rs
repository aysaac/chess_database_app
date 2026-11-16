//! A copy of a database on the self-hosted server (NET-03), as the app's
//! offline copy makes and updates it (`chess_net::server::copy_into`):
//!
//! ```sh
//! CHESS_SERVER_PASSWORD=… cargo run -p chess-net --example server_copy -- https://chess.example.com alice DB_ID copy.chessdb
//! ```
//!
//! Signs in (a token for this run, revoked at the end), then copies the
//! database into `copy.chessdb` (made if it doesn't exist). Run again on
//! the same copy, it takes only what changed and was added since; where it
//! got to is kept next to it, in `copy.chessdb.mark`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chess_db::{Database, NoProgress};
use chess_net::Client;
use chess_net::server::{self, CopyMark, CopySource};

fn read_mark(path: &Path) -> Option<CopyMark> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut parts = text.split_whitespace().map(str::parse::<u64>);
    let (next, epoch) = (parts.next()?.ok()?, parts.next()?.ok()?);
    let seq = parts.next().and_then(Result::ok);
    Some(CopyMark { next: u32::try_from(next).ok()?, epoch, seq })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [url, user, id, path] = &args[..] else {
        return Err(
            "usage: server_copy URL USERNAME DATABASE_ID COPY.chessdb (password in CHESS_SERVER_PASSWORD)"
                .into(),
        );
    };
    let password = std::env::var("CHESS_SERVER_PASSWORD").map_err(|_| "set CHESS_SERVER_PASSWORD")?;
    let url = server::server_url(url)?;
    let client = Client::new();
    let login = server::sign_in(&client, &url, user, &password, "server_copy example")?;
    let path = PathBuf::from(path);
    let db = if path.exists() { Database::open(&path)? } else { Database::create(&path)? };
    let db = Mutex::new(db);
    let mut mark_file = path.clone().into_os_string();
    mark_file.push(".mark");
    let mark_file = PathBuf::from(mark_file);
    let source = CopySource { url: &url, token: &login.token, id };
    let result =
        server::copy_into(&client, source, read_mark(&mark_file), &db, &NoProgress, &|_| false, &mut |m| {
            let seq = m.seq.map_or(String::new(), |s| s.to_string());
            let _ = std::fs::write(&mark_file, format!("{} {} {seq}\n", m.next, m.epoch));
        });
    server::sign_out(&client, &url, &login.token, login.token_id)?;
    let copy = result?;
    println!(
        "{}: {} new games, {} changed, {} games in the copy",
        copy.db.name,
        copy.import.imported,
        copy.changed,
        db.lock().map_err(|_| "lock")?.count()
    );
    if let Some(e) = copy.stopped {
        return Err(e.into());
    }
    Ok(())
}
