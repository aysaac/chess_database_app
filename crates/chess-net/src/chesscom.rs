//! Chess.com's published data API (<https://www.chess.com/news/view/published-data-api>):
//! a player's profile, the months they played in, and each month's games
//! as JSON (each with its PGN). Usernames go in lowercase; requests one at
//! a time.

use serde::Deserialize;

use crate::http::{Client, read_json, status_error};
use crate::keys::key_of_url;
use crate::time::{month_of, month_start_ms};
use crate::{Fetch, Fetched, NetError, Player, Site, Speed};

/// A month's JSON can be big for a bullet player (thousands of games, each
/// PGN with clock times).
const MONTH_LIMIT: u64 = 512 << 20;

#[derive(Deserialize)]
struct Profile {
    username: String,
    /// `https://www.chess.com/member/Hikaru`: the name as the player writes it.
    url: Option<String>,
    joined: Option<i64>,
}

#[derive(Deserialize)]
struct Archives {
    archives: Vec<String>,
}

#[derive(Deserialize)]
struct Month {
    games: Vec<Game>,
}

#[derive(Deserialize)]
struct Game {
    url: String,
    pgn: Option<String>,
    end_time: i64,
    #[serde(default)]
    rated: bool,
    #[serde(default)]
    rules: String,
    #[serde(default)]
    time_class: String,
}

fn no_such_player(name: &str) -> NetError {
    NetError::NoSuchPlayer { site: Site::ChessCom, name: name.to_owned() }
}

pub(crate) fn player(client: &Client, name: &str, stop: &dyn Fn() -> bool) -> Result<Player, NetError> {
    let lower = name.to_ascii_lowercase();
    let url = format!("{}/pub/player/{lower}", client.chess_com);
    let mut resp = client.get(Site::ChessCom, &url, &[], "application/json", stop)?;
    match resp.status().as_u16() {
        200 => {}
        404 | 410 => return Err(no_such_player(name)),
        _ => return Err(status_error(Site::ChessCom, &resp)),
    }
    let p: Profile = read_json(Site::ChessCom, &mut resp, 1 << 20)?;
    let shown =
        p.url.as_deref().and_then(|u| u.rsplit('/').next()).filter(|n| n.eq_ignore_ascii_case(&p.username));
    Ok(Player {
        site: Site::ChessCom,
        username: shown.unwrap_or(&p.username).to_owned(),
        joined: p.joined.map_or(month_start_ms(2007, 1), |s| s * 1000),
        games: None,
    })
}

/// `…/games/2026/10` → (2026, 10).
fn month_of_url(url: &str) -> Option<(i64, u32)> {
    let mut parts = url.trim_end_matches('/').rsplit('/');
    let m: u32 = parts.next()?.parse().ok()?;
    let y: i64 = parts.next()?.parse().ok()?;
    (parts.next()? == "games" && (1..=12).contains(&m)).then_some((y, m))
}

fn speed_of(time_class: &str) -> Option<Speed> {
    match time_class {
        "bullet" => Some(Speed::Bullet),
        "blitz" => Some(Speed::Blitz),
        "rapid" => Some(Speed::Rapid),
        "daily" => Some(Speed::Correspondence),
        _ => None,
    }
}

/// Sends the player's games to `send`, oldest first, until it returns false.
/// With `since`, the months before it are left out, and the games that
/// ended before it.
pub(crate) fn fetch(
    client: &Client,
    what: &Fetch,
    send: &mut dyn FnMut(Fetched) -> bool,
    stop: &dyn Fn() -> bool,
) -> Result<(), NetError> {
    let lower = what.username.to_ascii_lowercase();
    let base = format!("{}/pub/player/{lower}/games", client.chess_com);
    let mut resp = client.get(Site::ChessCom, &format!("{base}/archives"), &[], "application/json", stop)?;
    match resp.status().as_u16() {
        200 => {}
        404 | 410 => return Err(no_such_player(&what.username)),
        _ => return Err(status_error(Site::ChessCom, &resp)),
    }
    let archives: Archives = read_json(Site::ChessCom, &mut resp, 4 << 20)?;
    let mut months: Vec<(i64, u32)> = archives.archives.iter().filter_map(|u| month_of_url(u)).collect();
    months.sort_unstable();
    months.dedup();
    let first = what.since.map(month_of);
    for (y, m) in months.into_iter().filter(|&ym| first.is_none_or(|f| ym >= f)) {
        // The archive's own URL is the same one; built here so that only
        // the configured server is ever asked.
        let mut resp =
            client.get(Site::ChessCom, &format!("{base}/{y}/{m:02}"), &[], "application/json", stop)?;
        let next = if m == 12 { month_start_ms(y + 1, 1) } else { month_start_ms(y, m + 1) };
        match resp.status().as_u16() {
            200 => {}
            // A month that's gone: nothing to fetch.
            404 | 410 => {
                if !send(Fetched::Reached(next)) {
                    return Ok(());
                }
                continue;
            }
            _ => return Err(status_error(Site::ChessCom, &resp)),
        }
        let mut month: Month = read_json(Site::ChessCom, &mut resp, MONTH_LIMIT)?;
        month.games.sort_by_key(|g| g.end_time);
        for g in month.games {
            let at = g.end_time.saturating_mul(1000);
            let wanted = g.rules == "chess"
                && (!what.rated_only || g.rated)
                && speed_of(&g.time_class)
                    .is_some_and(|s| what.speeds.is_empty() || what.speeds.contains(&s))
                && what.since.is_none_or(|s| at >= s);
            let (true, Some(pgn), Some(key)) = (wanted, g.pgn, key_of_url(&g.url)) else { continue };
            if !send(Fetched::Game { key, at, pgn }) {
                return Ok(());
            }
        }
        if !send(Fetched::Reached(next)) {
            return Ok(());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months_of_archive_urls() {
        assert_eq!(month_of_url("https://api.chess.com/pub/player/hikaru/games/2026/10"), Some((2026, 10)));
        assert_eq!(month_of_url("https://api.chess.com/pub/player/hikaru/games/2014/01/"), Some((2014, 1)));
        assert_eq!(month_of_url("https://api.chess.com/pub/player/hikaru/games/2014/13"), None);
        assert_eq!(month_of_url("https://api.chess.com/pub/player/hikaru"), None);
    }
}
