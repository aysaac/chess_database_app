//! Lichess (<https://lichess.org/api>): a player's profile, and their games
//! as NDJSON, oldest first, each line one game with its PGN (clock times,
//! evals and the opening's name included). Lichess sends about 20 games a
//! second to a client that isn't logged in.

use std::io::{self, BufRead, BufReader, Read};
use std::sync::Arc;
use std::sync::atomic::AtomicI64;

use serde::Deserialize;

use crate::http::{Alive, Client, io_reason, read_json, status_error};
use crate::keys::lichess_key;
use crate::{Fetch, Fetched, NetError, Player, Site, Speed};

/// Times in a row the stream is asked for again, from the last game, after
/// the connection broke off without a new game since the last time.
const RESUMES: u32 = 3;
/// The longest line (one game) taken; a game's PGN is a few kB.
const MAX_LINE: u64 = 16 << 20;
/// Lichess's lowest `since` (2013-01-01).
const FIRST_GAME_MS: i64 = 1_356_998_400_070;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct User {
    username: String,
    created_at: Option<i64>,
    count: Option<Count>,
}

#[derive(Deserialize)]
struct Count {
    all: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Game {
    id: String,
    created_at: i64,
    pgn: Option<String>,
}

pub(crate) fn player(client: &Client, name: &str, stop: &dyn Fn() -> bool) -> Result<Player, NetError> {
    let url = format!("{}/api/user/{name}", client.lichess);
    let mut resp = client.get(Site::Lichess, &url, &[], "application/json", stop)?;
    match resp.status().as_u16() {
        200 => {}
        404 => return Err(NetError::NoSuchPlayer { site: Site::Lichess, name: name.to_owned() }),
        _ => return Err(status_error(Site::Lichess, &resp)),
    }
    let user: User = read_json(Site::Lichess, &mut resp, 1 << 20, &Arc::new(AtomicI64::new(0)))?;
    Ok(Player {
        site: Site::Lichess,
        username: user.username,
        joined: user.created_at.unwrap_or(FIRST_GAME_MS),
        games: user.count.map(|c| c.all),
    })
}

/// The perf types of `speeds` (all of them when empty), which also leaves
/// out the variants (Chess960, Crazyhouse…) the database can't hold.
fn perf_types(speeds: &[Speed]) -> String {
    let all = [Speed::Bullet, Speed::Blitz, Speed::Rapid, Speed::Classical, Speed::Correspondence];
    let speeds = if speeds.is_empty() { &all[..] } else { speeds };
    let names: Vec<&str> = speeds
        .iter()
        .flat_map(|s| match s {
            Speed::Bullet => &["ultraBullet", "bullet"][..],
            Speed::Blitz => &["blitz"],
            Speed::Rapid => &["rapid"],
            Speed::Classical => &["classical"],
            Speed::Correspondence => &["correspondence"],
        })
        .copied()
        .collect();
    names.join(",")
}

/// One line of the stream, without its end; `None` at the end of it.
fn read_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<Option<()>> {
    line.clear();
    let n = reader.by_ref().take(MAX_LINE).read_until(b'\n', line)?;
    if n == 0 {
        return Ok(None);
    }
    if line.last() == Some(&b'\n') {
        line.pop();
    } else if n as u64 == MAX_LINE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a game over 16 MB"));
    }
    Ok(Some(()))
}

/// Sends the player's games to `send`, oldest first, until it returns false.
pub(crate) fn fetch(
    client: &Client,
    what: &Fetch,
    send: &mut dyn FnMut(Fetched) -> bool,
    stop: &dyn Fn() -> bool,
) -> Result<(), NetError> {
    let url = format!("{}/api/games/user/{}", client.lichess, what.username);
    let bad = |reason: String| NetError::BadData { site: Site::Lichess, reason };
    let mut since = what.since.filter(|&s| s >= FIRST_GAME_MS);
    let mut resumes = 0;
    loop {
        let mut query = vec![
            ("sort", "dateAsc".to_owned()),
            ("pgnInJson", "true".to_owned()),
            ("clocks", "true".to_owned()),
            ("evals", "true".to_owned()),
            ("opening", "true".to_owned()),
            ("perfType", perf_types(&what.speeds)),
        ];
        if what.rated_only {
            query.push(("rated", "true".to_owned()));
        }
        if let Some(s) = since {
            query.push(("since", s.to_string()));
        }
        let resp = client.get(Site::Lichess, &url, &query, "application/x-ndjson", stop)?;
        match resp.status().as_u16() {
            200 => {}
            404 => return Err(NetError::NoSuchPlayer { site: Site::Lichess, name: what.username.clone() }),
            _ => return Err(status_error(Site::Lichess, &resp)),
        }
        let body = Alive { inner: resp.into_body().into_reader(), at: what.alive.clone() };
        let mut reader = BufReader::with_capacity(1 << 16, body);
        let mut line = Vec::new();
        let from = since;
        let broke_off = loop {
            match read_line(&mut reader, &mut line) {
                Ok(None) => return Ok(()),
                Ok(Some(())) => {}
                Err(e) if e.kind() == io::ErrorKind::InvalidData => return Err(bad(io_reason(&e))),
                Err(e) => break e,
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let game: Game = serde_json::from_slice(&line).map_err(|e| bad(e.to_string()))?;
            since = Some(game.created_at);
            let (Some(pgn), Some(key)) = (game.pgn, lichess_key(&game.id)) else { continue };
            if !send(Fetched::Game { key, at: game.created_at, pgn }) {
                return Ok(());
            }
        };
        // A break after new games starts the count again.
        if since != from {
            resumes = 0;
        }
        if resumes == RESUMES || stop() {
            return Err(NetError::Unreachable { site: Site::Lichess, reason: io_reason(&broke_off) });
        }
        // From the last game again: it's skipped as already downloaded.
        resumes += 1;
        log::warn!("Lichess: the games of {} broke off ({broke_off}); asking again", what.username);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perf_types_leave_out_variants() {
        assert_eq!(perf_types(&[]), "ultraBullet,bullet,blitz,rapid,classical,correspondence");
        assert_eq!(perf_types(&[Speed::Bullet, Speed::Rapid]), "ultraBullet,bullet,rapid");
    }
}
