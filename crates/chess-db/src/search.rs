//! Header search (SRCH-01) and the quick filter (SRCH-02): scans the index
//! records, never the game blobs. Name criteria are resolved against the
//! name tables first, so each record check is a few integer comparisons.
//!
//! Text matching ignores case and common accents ("reti" finds "Réti,
//! Richard"), and every word of the pattern must occur somewhere in the
//! name ("magnus carlsen" finds "Carlsen, Magnus").

use std::sync::Mutex;

use crate::database::NameKind;
use crate::names::NameTable;
use crate::record::{IndexRecord, pack_eco};
use crate::{Database, DbError, GameId, Progress};

/// A set of game ids (bitset).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameSet {
    bits: Vec<u64>,
    len: u32,
}

impl GameSet {
    pub fn from_ids(ids: &[GameId]) -> GameSet {
        let mut set = GameSet::default();
        for &id in ids {
            set.insert(id);
        }
        set
    }

    pub fn insert(&mut self, id: GameId) {
        let (word, bit) = (id as usize / 64, id % 64);
        if word >= self.bits.len() {
            self.bits.resize(word + 1, 0);
        }
        if self.bits[word] & (1 << bit) == 0 {
            self.bits[word] |= 1 << bit;
            self.len += 1;
        }
    }

    pub fn contains(&self, id: GameId) -> bool {
        self.bits.get(id as usize / 64).is_some_and(|w| w & (1 << (id % 64)) != 0)
    }

    pub fn len(&self) -> u32 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultFilter {
    WhiteWins,
    BlackWins,
    Draw,
    /// Either side won.
    Decisive,
}

/// How an Elo range applies to the two players.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EloMode {
    #[default]
    Both,
    Either,
    Average,
}

/// Header criteria; empty fields match everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderQuery {
    pub white: String,
    pub black: String,
    /// White and Black may be the other way round.
    pub ignore_colors: bool,
    /// Matches either player, the event, the site or the annotator (the
    /// quick filter).
    pub text: String,
    pub event: String,
    pub site: String,
    pub annotator: String,
    pub year_from: Option<u16>,
    pub year_to: Option<u16>,
    /// ECO range such as `B20`–`B99`.
    pub eco_from: String,
    pub eco_to: String,
    pub result: Option<ResultFilter>,
    pub elo_min: Option<u16>,
    pub elo_max: Option<u16>,
    pub elo_mode: EloMode,
    pub moves_min: Option<u16>,
    pub moves_max: Option<u16>,
    /// Only games with comments, variations, symbols or arrows.
    pub annotated_only: bool,
    pub skip_deleted: bool,
}

/// Lower-cases and strips accents from Latin letters.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        let plain = match c {
            'à'..='å' | 'ā' | 'ă' | 'ą' => "a",
            'æ' => "ae",
            'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
            'ď' | 'đ' | 'ð' => "d",
            'è'..='ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
            'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
            'ĥ' | 'ħ' => "h",
            'ì'..='ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
            'ĵ' => "j",
            'ķ' => "k",
            'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
            'ñ' | 'ń' | 'ņ' | 'ň' => "n",
            'ò'..='ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
            'œ' => "oe",
            'ŕ' | 'ŗ' | 'ř' => "r",
            'ś' | 'ŝ' | 'ş' | 'š' | 'ș' => "s",
            'ß' => "ss",
            'ţ' | 'ť' | 'ŧ' | 'ț' => "t",
            'ù'..='ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
            'ŵ' => "w",
            'ý' | 'ÿ' | 'ŷ' => "y",
            'ź' | 'ż' | 'ž' => "z",
            'þ' => "th",
            _ => {
                out.push(c);
                continue;
            }
        };
        out.push_str(plain);
    }
    out
}

/// Which names of `table` match `pattern` (every word occurs), or `None`
/// for an empty pattern (matches all).
fn matching_names(table: &NameTable, pattern: &str) -> Option<Vec<bool>> {
    let words: Vec<String> = fold(pattern)
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect();
    if words.is_empty() {
        return None;
    }
    Some(table.matches(|name| !name.is_empty() && words.iter().all(|w| name.contains(w.as_str()))))
}

fn hit(m: &Option<Vec<bool>>, id: u32) -> bool {
    m.as_ref().is_none_or(|v| v.get(id as usize).copied().unwrap_or(false))
}

/// A query compiled against one database's name tables.
struct Compiled<'q> {
    q: &'q HeaderQuery,
    white: Option<Vec<bool>>,
    black: Option<Vec<bool>>,
    event: Option<Vec<bool>>,
    site: Option<Vec<bool>>,
    annotator: Option<Vec<bool>>,
    text: Option<[Vec<bool>; 4]>,
    eco: (u16, u16),
}

impl Compiled<'_> {
    fn new<'q>(db: &Database, q: &'q HeaderQuery) -> Compiled<'q> {
        let players = db.names(NameKind::Player);
        let text = matching_names(players, &q.text).map(|p| {
            let all = |kind| matching_names(db.names(kind), &q.text).expect("non-empty pattern");
            [p, all(NameKind::Event), all(NameKind::Site), all(NameKind::Annotator)]
        });
        let eco_from = pack_eco(&q.eco_from.trim().to_uppercase());
        let eco_to = pack_eco(&q.eco_to.trim().to_uppercase());
        Compiled {
            q,
            white: matching_names(players, &q.white),
            black: matching_names(players, &q.black),
            event: matching_names(db.names(NameKind::Event), &q.event),
            site: matching_names(db.names(NameKind::Site), &q.site),
            annotator: matching_names(db.names(NameKind::Annotator), &q.annotator),
            text,
            eco: match (eco_from, eco_to) {
                (0, 0) => (0, 0),
                (from, 0) => (from, from),
                (0, to) => (pack_eco("A00"), to),
                (from, to) => (from.min(to), from.max(to)),
            },
        }
    }

    fn matches(&self, r: &IndexRecord) -> bool {
        let q = self.q;
        if q.skip_deleted && r.is_deleted() {
            return false;
        }
        let players = (hit(&self.white, r.white) && hit(&self.black, r.black))
            || (q.ignore_colors && hit(&self.white, r.black) && hit(&self.black, r.white));
        if !players
            || !hit(&self.event, r.event)
            || !hit(&self.site, r.site)
            || !hit(&self.annotator, r.annotator)
        {
            return false;
        }
        if let Some([p, e, s, a]) = &self.text {
            let any = |v: &Vec<bool>, id: u32| v.get(id as usize).copied().unwrap_or(false);
            if !(any(p, r.white)
                || any(p, r.black)
                || any(e, r.event)
                || any(s, r.site)
                || any(a, r.annotator))
            {
                return false;
            }
        }
        let year = (r.date >> 9) as u16;
        if (q.year_from.is_some() || q.year_to.is_some())
            && (year == 0 || q.year_from.is_some_and(|y| year < y) || q.year_to.is_some_and(|y| year > y))
        {
            return false;
        }
        if self.eco.0 != 0 && (r.eco == 0 || r.eco < self.eco.0 || r.eco > self.eco.1) {
            return false;
        }
        let result_ok = match q.result {
            None => true,
            Some(ResultFilter::WhiteWins) => r.result == 1,
            Some(ResultFilter::BlackWins) => r.result == 2,
            Some(ResultFilter::Draw) => r.result == 3,
            Some(ResultFilter::Decisive) => r.result == 1 || r.result == 2,
        };
        if !result_ok {
            return false;
        }
        if (q.elo_min.is_some() || q.elo_max.is_some()) && !self.elo_ok(r) {
            return false;
        }
        let moves = r.plies.div_ceil(2);
        if q.moves_min.is_some_and(|m| moves < m) || q.moves_max.is_some_and(|m| moves > m) {
            return false;
        }
        if q.annotated_only
            && r.flags
                & (crate::record::FLAG_ANNOTATIONS
                    | crate::record::FLAG_COMMENTS
                    | crate::record::FLAG_VARIATIONS)
                == 0
        {
            return false;
        }
        true
    }

    fn elo_ok(&self, r: &IndexRecord) -> bool {
        let q = self.q;
        let fits = |e: u16| e != 0 && q.elo_min.is_none_or(|m| e >= m) && q.elo_max.is_none_or(|m| e <= m);
        match q.elo_mode {
            EloMode::Both => fits(r.white_elo) && fits(r.black_elo),
            EloMode::Either => fits(r.white_elo) || fits(r.black_elo),
            EloMode::Average => {
                r.white_elo != 0
                    && r.black_elo != 0
                    && fits(((u32::from(r.white_elo) + u32::from(r.black_elo)) / 2) as u16)
            }
        }
    }
}

/// Ids of the games matching `query`, ascending. With `within`, only those
/// games are considered (search within results, SRCH-14).
pub fn search_headers(
    db: &Mutex<Database>,
    query: &HeaderQuery,
    within: Option<&GameSet>,
    progress: &dyn Progress,
) -> Result<Vec<GameId>, DbError> {
    const CHUNK: u32 = 64 * 1024;
    let count = db.lock().expect("db lock").count();
    let mut out = Vec::new();
    let mut start = 0;
    // Compiled once; name tables only grow, and new names can't match
    // records that were scanned before them.
    let guard = db.lock().expect("db lock");
    let compiled = Compiled::new(&guard, query);
    drop(guard);
    while start < count {
        progress.check_cancelled()?;
        let records = db.lock().expect("db lock").records(start, CHUNK)?;
        for (r, id) in records.iter().zip(start..) {
            if within.is_none_or(|w| w.contains(id)) && compiled.matches(r) {
                out.push(id);
            }
        }
        start = start.saturating_add(CHUNK);
        progress.report(u64::from(start.min(count)), u64::from(count));
    }
    Ok(out)
}

/// Ids of the games that use name `id` of `kind` (for players: with either
/// colour), ascending: the games of a player or tournament (DB-04).
pub fn games_with_name(
    db: &Mutex<Database>,
    kind: NameKind,
    id: u32,
    progress: &dyn Progress,
) -> Result<Vec<GameId>, DbError> {
    const CHUNK: u32 = 64 * 1024;
    let count = db.lock().expect("db lock").count();
    let mut out = Vec::new();
    let mut start = 0;
    while start < count {
        progress.check_cancelled()?;
        let records = db.lock().expect("db lock").records(start, CHUNK)?;
        for (r, gid) in records.iter().zip(start..) {
            let hit = match kind {
                NameKind::Player => r.white == id || r.black == id,
                NameKind::Event => r.event == id,
                NameKind::Site => r.site == id,
                NameKind::Annotator => r.annotator == id,
            };
            if hit {
                out.push(gid);
            }
        }
        start = start.saturating_add(CHUNK);
        progress.report(u64::from(start.min(count)), u64::from(count));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoProgress;
    use chess_core::pgn::read_pgn_str;

    const PGN: &str = r#"[White "Réti, Richard"][Black "Carlsen, Magnus"][Event "Casual"][WhiteElo "2600"][BlackElo "2850"][Date "1925.01.01"][ECO "A04"][Result "1-0"]
1. Nf3 d5 1-0

[White "Carlsen, Magnus"][Black "Anand, Viswanathan"][Event "World Championship"][Site "Chennai"][WhiteElo "2870"][BlackElo "2775"][Date "2013.11.09"][ECO "D37"][Result "1/2-1/2"]
1. d4 {a comment} d5 1/2-1/2

[White "Anand, Viswanathan"][Black "Tal, Mikhail"][Date "1990.??.??"][ECO "B99"][Result "0-1"]
1. e4 c5 2. Nf3 d6 3. d4 cxd4 0-1

[White "Tal, Mikhail"][Black "Réti, Richard"][Result "*"]
1. e4 *
"#;

    fn db() -> (tempfile::TempDir, Mutex<Database>) {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Database::create(&dir.path().join("Search")).unwrap();
        for g in read_pgn_str(PGN) {
            db.append(&g).unwrap();
        }
        db.set_deleted(3, true).unwrap();
        (dir, Mutex::new(db))
    }

    fn find(db: &Mutex<Database>, q: HeaderQuery) -> Vec<GameId> {
        search_headers(db, &q, None, &NoProgress).unwrap()
    }

    #[test]
    fn folds_accents_and_case() {
        assert_eq!(fold("Réti, Ŝtraße ŁÓDŹ"), "reti, strasse lodz");
    }

    #[test]
    fn player_names() {
        let (_d, db) = db();
        let q = |w: &str, b: &str, ignore| HeaderQuery {
            white: w.into(),
            black: b.into(),
            ignore_colors: ignore,
            ..HeaderQuery::default()
        };
        assert_eq!(find(&db, q("reti", "", false)), [0]);
        assert_eq!(find(&db, q("magnus carlsen", "", false)), [1]);
        assert_eq!(find(&db, q("", "carlsen", false)), [0]);
        assert_eq!(find(&db, q("carlsen", "", true)), [0, 1]);
        assert_eq!(find(&db, q("tal", "reti", true)), [3]);
        assert_eq!(find(&db, q("reti", "tal", true)), [3]);
        assert_eq!(find(&db, q("nobody", "", true)), Vec::<GameId>::new());
    }

    #[test]
    fn quick_filter_text_matches_any_name_field() {
        let (_d, db) = db();
        let text = |t: &str| HeaderQuery { text: t.into(), ..HeaderQuery::default() };
        assert_eq!(find(&db, text("anand")), [1, 2]);
        assert_eq!(find(&db, text("chennai")), [1]);
        assert_eq!(find(&db, text("world champ")), [1]);
    }

    #[test]
    fn numbers_ranges_and_flags() {
        let (_d, db) = db();
        let base = HeaderQuery::default();
        assert_eq!(find(&db, HeaderQuery { year_from: Some(1950), ..base.clone() }), [1, 2]);
        assert_eq!(find(&db, HeaderQuery { year_to: Some(1950), ..base.clone() }), [0]);
        // An incomplete ECO code is ignored; a lone upper bound starts at A00.
        assert_eq!(find(&db, HeaderQuery { eco_from: "b".into(), ..base.clone() }), [0, 1, 2, 3]);
        assert_eq!(find(&db, HeaderQuery { eco_to: "B50".into(), ..base.clone() }), [0]);
        assert_eq!(
            find(&db, HeaderQuery { eco_from: "A00".into(), eco_to: "C99".into(), ..base.clone() }),
            [0, 2]
        );
        assert_eq!(find(&db, HeaderQuery { eco_from: "d37".into(), ..base.clone() }), [1]);
        assert_eq!(find(&db, HeaderQuery { result: Some(ResultFilter::Decisive), ..base.clone() }), [0, 2]);
        assert_eq!(find(&db, HeaderQuery { result: Some(ResultFilter::Draw), ..base.clone() }), [1]);
        assert_eq!(find(&db, HeaderQuery { elo_min: Some(2700), ..base.clone() }), [1]);
        assert_eq!(
            find(&db, HeaderQuery { elo_min: Some(2700), elo_mode: EloMode::Either, ..base.clone() }),
            [0, 1]
        );
        assert_eq!(
            find(&db, HeaderQuery { elo_max: Some(2750), elo_mode: EloMode::Average, ..base.clone() }),
            [0]
        );
        assert_eq!(find(&db, HeaderQuery { moves_min: Some(2), ..base.clone() }), [2]);
        assert_eq!(find(&db, HeaderQuery { moves_max: Some(1), ..base.clone() }), [0, 1, 3]);
        assert_eq!(find(&db, HeaderQuery { annotated_only: true, ..base.clone() }), [1]);
        assert_eq!(find(&db, HeaderQuery { skip_deleted: true, ..base.clone() }), [0, 1, 2]);
        assert_eq!(find(&db, base), [0, 1, 2, 3]);
    }

    #[test]
    fn games_of_a_name() {
        let (_d, db) = db();
        let anand = db.lock().unwrap().names(NameKind::Player).id_of("Anand, Viswanathan").unwrap();
        assert_eq!(games_with_name(&db, NameKind::Player, anand, &NoProgress).unwrap(), [1, 2]);
        let chennai = db.lock().unwrap().names(NameKind::Site).id_of("Chennai").unwrap();
        assert_eq!(games_with_name(&db, NameKind::Site, chennai, &NoProgress).unwrap(), [1]);
    }

    #[test]
    fn search_within_results() {
        let (_d, db) = db();
        let within = GameSet::from_ids(&[1, 2, 3]);
        let q = HeaderQuery { text: "anand".into(), ..HeaderQuery::default() };
        assert_eq!(search_headers(&db, &q, Some(&within), &NoProgress).unwrap(), [1, 2]);
        let within = GameSet::from_ids(&[0, 3]);
        assert!(search_headers(&db, &q, Some(&within), &NoProgress).unwrap().is_empty());
        assert_eq!(within.len(), 2);
        assert!(within.contains(3) && !within.contains(1) && !within.contains(100_000));
    }
}
