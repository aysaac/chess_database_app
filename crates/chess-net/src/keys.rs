//! What tells online games apart: the game's id on its site, read from its
//! page's URL (`Site` on Lichess, `Link` on Chess.com) or Lichess's
//! `GameId` tag. Two copies of one game have the same key however the URL
//! is written. A chapter of a Lichess study (IO-10) has a key of its own,
//! from its `ChapterURL` (`https://lichess.org/study/{study}/{chapter}`).

use crate::Site;

/// A game's id on its site, as `lichess:<8 chars>` or
/// `chess.com:<live|daily>/<number>`; a study chapter's as
/// `lichess-study:<study>/<chapter>`.
pub type GameKey = String;

/// A Lichess study or chapter id: 8 letters and digits.
pub fn is_lichess_id(id: &str) -> bool {
    id.len() == 8 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The key of a study's chapter.
pub fn study_chapter_key(study: &str, chapter: &str) -> Option<GameKey> {
    (is_lichess_id(study) && is_lichess_id(chapter)).then(|| format!("lichess-study:{study}/{chapter}"))
}

/// Lichess pages whose names have a game id's length (8 letters).
const LICHESS_PAGES: [&str; 10] = [
    "training", "analysis", "practice", "streamer", "tutorial", "features", "insights", "download",
    "settings", "timeline",
];

pub fn lichess_key(id: &str) -> Option<GameKey> {
    // Game ids have 8 characters; a player's view of a game adds 4
    // (`/abcdefghWXYZ`). Most other pages (`/tournament`) have other lengths.
    let ok = matches!(id.len(), 8 | 12)
        && id.bytes().all(|b| b.is_ascii_alphanumeric())
        && !LICHESS_PAGES.contains(&id);
    ok.then(|| format!("lichess:{}", &id[..8]))
}

/// The key of a game page's URL, if it's a Lichess or Chess.com game:
/// `https://lichess.org/abcdefgh`, `…/game/abcdefgh`, `…/abcdefgh/black`;
/// `https://www.chess.com/game/live/123`, `…/game/daily/123`,
/// `…/live/game/123` (older links). A study chapter's URL gives the
/// chapter's key: `https://lichess.org/study/{study}/{chapter}`.
pub fn key_of_url(url: &str) -> Option<GameKey> {
    let rest = url.trim().split_once("://").map_or(url.trim(), |(_, r)| r);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let path = path.split(['?', '#']).next().unwrap_or("");
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    match host {
        "lichess.org" => match parts.as_slice() {
            ["study", study, chapter, ..] => study_chapter_key(study, chapter),
            ["study", ..] => None,
            ["game", id, ..] | [id, ..] => lichess_key(id),
            [] => None,
        },
        "chess.com" => {
            let (kind, n) = match parts.as_slice() {
                ["game", kind @ ("live" | "daily"), n, ..] | [kind @ ("live" | "daily"), "game", n, ..] => {
                    (*kind, *n)
                }
                _ => return None,
            };
            n.bytes().all(|b| b.is_ascii_digit()).then(|| format!("chess.com:{kind}/{n}"))
        }
        _ => None,
    }
}

/// The site a key belongs to.
pub fn site_of(key: &str) -> Option<Site> {
    match key.split_once(':')?.0 {
        "lichess" | "lichess-study" => Some(Site::Lichess),
        "chess.com" => Some(Site::ChessCom),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_of_game_pages() {
        let k = Some("lichess:HqjDTU4A".to_owned());
        for url in [
            "https://lichess.org/HqjDTU4A",
            "https://lichess.org/game/HqjDTU4A",
            "https://lichess.org/HqjDTU4Axyz1/black",
            "http://www.lichess.org/HqjDTU4A#12",
            "lichess.org/HqjDTU4A",
        ] {
            assert_eq!(key_of_url(url), k, "{url}");
        }
        let k = Some("chess.com:live/185817877524".to_owned());
        for url in [
            "https://www.chess.com/game/live/185817877524",
            "https://chess.com/live/game/185817877524?move=3",
        ] {
            assert_eq!(key_of_url(url), k, "{url}");
        }
        assert_eq!(key_of_url("https://www.chess.com/game/daily/77"), Some("chess.com:daily/77".into()));
        for url in [
            "Chess.com",
            "https://lichess.org/",
            "https://lichess.org/abc",
            "https://lichess.org/tournament",
            "https://lichess.org/training/mix",
            "https://lichess.org/analysis",
            "https://lichess.org/@/DrNykterstein",
            "https://www.chess.com/member/hikaru",
            "https://www.chess.com/game/live/12a",
            "https://example.org/HqjDTU4A",
            "",
        ] {
            assert_eq!(key_of_url(url), None, "{url}");
        }
        assert_eq!(lichess_key("HqjDTU4A"), Some("lichess:HqjDTU4A".into()));
        assert_eq!(lichess_key("Hqj-TU4A"), None);
        assert_eq!(site_of("chess.com:live/1"), Some(Site::ChessCom));
        assert_eq!(site_of("lichess:HqjDTU4A"), Some(Site::Lichess));
    }

    #[test]
    fn keys_of_study_chapters() {
        let k = Some("lichess-study:keq5vaIF/dLpkgwla".to_owned());
        for url in [
            "https://lichess.org/study/keq5vaIF/dLpkgwla",
            "https://lichess.org/study/keq5vaIF/dLpkgwla#12",
            "lichess.org/study/keq5vaIF/dLpkgwla?x=1",
        ] {
            assert_eq!(key_of_url(url), k, "{url}");
        }
        // Ids are case-sensitive: another chapter.
        assert_ne!(key_of_url("https://lichess.org/study/keq5vaIF/DLPKGWLA"), k);
        for url in [
            "https://lichess.org/study/keq5vaIF",
            "https://lichess.org/study",
            "https://lichess.org/study/keq5vaIF/short",
            "https://lichess.org/study/by/thibault",
        ] {
            assert_eq!(key_of_url(url), None, "{url}");
        }
        assert_eq!(study_chapter_key("keq5vaIF", "dLpkgwla"), k);
        assert_eq!(study_chapter_key("keq5va-F", "dLpkgwla"), None);
        assert_eq!(site_of("lichess-study:keq5vaIF/dLpkgwla"), Some(Site::Lichess));
        assert!(is_lichess_id("keq5vaIF") && !is_lichess_id("keq5vaI"));
    }
}
