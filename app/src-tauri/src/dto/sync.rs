//! Databases kept up to date from where their games come from (NET-01,
//! IO-10, NET-03): a player's games on Lichess or Chess.com, a Lichess
//! study, a database on the self-hosted server. And the sign-in to that
//! server.

use serde::{Deserialize, Deserializer, Serialize};
use ts_rs::TS;

use super::{OnlineSiteDto, SpeedDto};

/// Which side of the board: only the games the player had it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ColorDto {
    White,
    Black,
}

impl From<ColorDto> for chess_net::Color {
    fn from(c: ColorDto) -> chess_net::Color {
        match c {
            ColorDto::White => chess_net::Color::White,
            ColorDto::Black => chess_net::Color::Black,
        }
    }
}

/// What a database does when its games reach its limit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LimitPolicyDto {
    /// Updates stop bringing games in: the limit is a ceiling.
    #[default]
    Stop,
    /// Updates go on, and the oldest games over the limit are marked as
    /// deleted (until the database is compacted).
    KeepLatest,
}

/// Where a database kept up to date gets its games, with how far its last
/// update got (the app fills that in: a new source leaves it out).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum SyncSourceDto {
    /// A player's games (NET-01): each update downloads the new ones.
    #[serde(rename_all = "camelCase")]
    Player {
        site: OnlineSiteDto,
        username: String,
        /// Empty: all of the site's time controls.
        #[serde(default)]
        speeds: Vec<SpeedDto>,
        #[serde(default)]
        rated_only: bool,
        /// Only the games the player had this color; none: both.
        #[serde(default)]
        color: Option<ColorDto>,
        /// Only games from this time on and up to this one (ms since the
        /// epoch, the first day's start and the last day's end where the
        /// user is).
        #[serde(default)]
        #[ts(type = "number | null")]
        since: Option<i64>,
        #[serde(default)]
        #[ts(type = "number | null")]
        until: Option<i64>,
        /// When the newest game the last update brought started (Lichess)
        /// or ended (Chess.com), ms since the epoch: the next one starts
        /// there.
        #[serde(default)]
        #[ts(type = "number | null")]
        reached: Option<i64>,
    },
    /// A Lichess study, or one chapter of it (IO-10): each update imports
    /// the chapters the database lacks, once Lichess says it changed.
    #[serde(rename_all = "camelCase")]
    Study {
        /// The study's id.
        study: String,
        #[serde(default)]
        chapter: Option<String>,
        name: String,
        /// The study's `Last-Modified` at the last update.
        #[serde(default)]
        last_modified: Option<String>,
    },
    /// A database on the self-hosted server (NET-03): each update takes the
    /// games added there since.
    #[serde(rename_all = "camelCase")]
    Server {
        /// The server's address, as `server_url` writes it.
        url: String,
        /// The account it's read with.
        username: String,
        /// Its id there.
        database: String,
        name: String,
        /// The first game number the last update didn't take, and the
        /// server's numbering then.
        #[serde(default)]
        next: Option<u32>,
        #[serde(default)]
        #[ts(type = "number | null")]
        epoch: Option<u64>,
    },
}

impl SyncSourceDto {
    /// The same source: the same games, whatever its updates noted.
    pub fn same(&self, other: &SyncSourceDto) -> bool {
        use SyncSourceDto::*;
        match (self, other) {
            (
                Player { site, username, speeds, rated_only, color, since, until, .. },
                Player {
                    site: s2,
                    username: u2,
                    speeds: sp2,
                    rated_only: r2,
                    color: c2,
                    since: f2,
                    until: t2,
                    ..
                },
            ) => {
                let sorted = |v: &[SpeedDto]| {
                    let mut v = v.to_vec();
                    v.sort_unstable();
                    v.dedup();
                    v
                };
                site == s2
                    && username.eq_ignore_ascii_case(u2)
                    && sorted(speeds) == sorted(sp2)
                    && rated_only == r2
                    && color == c2
                    && since == f2
                    && until == t2
            }
            (Study { study, chapter, .. }, Study { study: s2, chapter: c2, .. }) => {
                study == s2 && chapter == c2
            }
            (Server { url, database, .. }, Server { url: u2, database: d2, .. }) => {
                url == u2 && database == d2
            }
            _ => false,
        }
    }

    /// The source as a new one: nothing noted yet.
    pub fn fresh(mut self) -> SyncSourceDto {
        match &mut self {
            SyncSourceDto::Player { reached, .. } => *reached = None,
            SyncSourceDto::Study { last_modified, .. } => *last_modified = None,
            SyncSourceDto::Server { next, epoch, .. } => (*next, *epoch) = (None, None),
        }
        self
    }

    /// What it is, in a few words: "DrNykterstein's Lichess games".
    pub fn describe(&self) -> String {
        match self {
            SyncSourceDto::Player { site, username, .. } => {
                format!("{username}'s {} games", chess_net::Site::from(*site).name())
            }
            SyncSourceDto::Study { name, .. } => format!("the study {name}"),
            SyncSourceDto::Server { name, url, .. } => format!("{name} on {}", host_of(url)),
        }
    }
}

/// A server's address without its scheme, as people say it.
pub fn host_of(url: &str) -> &str {
    url.split_once("://").map_or(url, |(_, rest)| rest)
}

/// How a database keeps up to date. Kept in the settings too, so fields a
/// newer version adds must be optional for this one to read them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, rename_all = "camelCase")]
#[ts(export)]
pub struct DbSyncDto {
    #[serde(deserialize_with = "known_sources")]
    pub sources: Vec<SyncSourceDto>,
    /// Updated when the app starts and when the database is opened.
    pub auto: bool,
    /// The most games the database holds, over all its sources (live ones:
    /// those marked as deleted don't count); none: no limit. Only for
    /// players' games.
    pub limit: Option<u32>,
    pub limit_policy: LimitPolicyDto,
    /// When its last update ended (ms since the epoch); `None`: never.
    #[ts(type = "number | null")]
    pub last_update: Option<i64>,
    /// What the last update said.
    pub last_result: Option<String>,
    /// The last update failed for at least one of the sources.
    pub last_failed: bool,
}

/// The sources this version knows: one a newer version wrote is left out
/// rather than the settings with it.
fn known_sources<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<SyncSourceDto>, D::Error> {
    let raw = Vec::<serde_json::Value>::deserialize(d)?;
    Ok(raw.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
}

/// The sign-in to the self-hosted server (NET-03).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ServerAccountDto {
    /// `None`: not signed in.
    pub url: Option<String>,
    pub username: Option<String>,
    /// The token is in the OS's credential store, so the sign-in lasts
    /// after the app closes; otherwise it lasts until then.
    pub kept: bool,
    /// Not signed in: the address to offer (the last one this session).
    pub suggested_url: Option<String>,
}

/// A database one may read on the server.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ServerDbDto {
    pub id: String,
    pub name: String,
    pub owner: String,
    /// `read`, `write` or `owner`.
    pub access: String,
    pub games: u32,
}

impl From<chess_net::server::ServerDb> for ServerDbDto {
    fn from(d: chess_net::server::ServerDb) -> ServerDbDto {
        ServerDbDto { id: d.id, name: d.name, owner: d.owner, access: d.access, games: d.games }
    }
}
