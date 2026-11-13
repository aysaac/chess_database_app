//! The study round-trip check, which NET-13 (linked study sync) needs
//! first: what Lichess keeps, changes or drops of annotated games sent to
//! a study and fetched back, and of a chapter whose moves are replaced in
//! place.
//!
//! [`round_trip`] sends the games to one new private study through
//! [`export_to_studies`] (as Send to study… does) and fetches the study back
//! through [`fetch_study`] (as Import study… does). Then it replaces one
//! chapter's moves with an edited version of its game
//! ([`replace_chapter_moves`], `POST /api/study/{id}/{chapter}/moves`) and
//! fetches that chapter again. Each side of each step is read with
//! chess-core's PGN reader and the move trees are compared feature by
//! feature ([`compare`]), never the text: only what a sync would see as a
//! change counts. `examples/study_roundtrip.rs` runs it and prints the
//! report ([`RoundTrip`]'s `Display`).
//!
//! It makes one study (of the 30 Lichess makes a day per account) of at
//! most 64 chapters, sends one request at a time, and leaves the study on
//! the account.

use std::fmt;

use chess_core::game::{Game, Nag, NagCategory, Node, NodeId, Shape, ShapeColor};
use chess_core::notation::san;
use chess_core::pgn::{PgnReader, write_game};
use chess_core::position::to_fen;
use chess_core::shakmaty::{Chess, Move, Position, Square};
use chess_db::Progress;

use super::{
    Chapter, ExportTarget, MAX_CHAPTERS, SentStudy, StudyRef, Visibility, check_study_name,
    export_to_studies, fetch_study, parse_study_link, replace_chapter_moves, tag_value,
};
use crate::oauth::Token;
use crate::{Client, NetError, Site};

/// The tags a study gives every chapter, left out of the comparison.
pub const STUDY_TAGS: [&str; 3] = ["StudyName", "ChapterName", "ChapterURL"];

/// What the check compares, each counted on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    /// Every move, on the main line and in variations.
    Moves,
    /// Lines branching off a move (nested ones too).
    Variations,
    /// The order of a move's continuations: which one is the main line,
    /// and the order of the variations after it.
    VariationOrder,
    /// Comment text, before and after moves.
    Comments,
    Nags,
    /// `[%cal]`.
    Arrows,
    /// `[%csl]`.
    Highlights,
    /// `[%clk]`.
    Clocks,
    /// `[%eval]`.
    Evals,
    /// The other `[%…]` commands (`%emt`, …).
    OtherCommands,
    /// A custom start position (`FEN`).
    StartPosition,
    /// The `Orientation` tag.
    Orientation,
    /// Every other tag but [`STUDY_TAGS`].
    Tags,
}

impl Feature {
    pub const ALL: [Feature; 13] = [
        Feature::Moves,
        Feature::Variations,
        Feature::VariationOrder,
        Feature::Comments,
        Feature::Nags,
        Feature::Arrows,
        Feature::Highlights,
        Feature::Clocks,
        Feature::Evals,
        Feature::OtherCommands,
        Feature::StartPosition,
        Feature::Orientation,
        Feature::Tags,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Feature::Moves => "moves",
            Feature::Variations => "variations",
            Feature::VariationOrder => "variation order",
            Feature::Comments => "comments",
            Feature::Nags => "NAGs (glyphs)",
            Feature::Arrows => "arrows (%cal)",
            Feature::Highlights => "highlights (%csl)",
            Feature::Clocks => "clocks (%clk)",
            Feature::Evals => "evals (%eval)",
            Feature::OtherCommands => "other [%…] commands",
            Feature::StartPosition => "start position (FEN)",
            Feature::Orientation => "orientation",
            Feature::Tags => "tags",
        }
    }
}

/// How many of one feature stayed the same, changed, went or came, and
/// where the ones that didn't stay are (`examples`, in the games' order).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tally {
    pub same: usize,
    pub changed: usize,
    pub dropped: usize,
    pub added: usize,
    pub examples: Vec<String>,
}

impl Tally {
    pub fn differences(&self) -> usize {
        self.changed + self.dropped + self.added
    }
}

/// One game against another, feature by feature.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Comparison {
    tallies: [Tally; 13],
}

impl Comparison {
    pub fn get(&self, feature: Feature) -> &Tally {
        &self.tallies[feature as usize]
    }

    /// Everything that changed, went or came.
    pub fn differences(&self) -> usize {
        self.tallies.iter().map(Tally::differences).sum()
    }

    /// Adds `other`'s counts, its examples headed with `label` ("game 2").
    pub fn absorb(&mut self, other: &Comparison, label: &str) {
        for (mine, theirs) in self.tallies.iter_mut().zip(&other.tallies) {
            mine.same += theirs.same;
            mine.changed += theirs.changed;
            mine.dropped += theirs.dropped;
            mine.added += theirs.added;
            mine.examples.extend(theirs.examples.iter().map(|e| format!("{label}, {e}")));
        }
    }

    fn at(&mut self, feature: Feature) -> &mut Tally {
        &mut self.tallies[feature as usize]
    }

    fn same(&mut self, feature: Feature) {
        self.at(feature).same += 1;
    }

    fn changed(&mut self, feature: Feature, example: String) {
        let t = self.at(feature);
        t.changed += 1;
        t.examples.push(example);
    }

    fn dropped(&mut self, feature: Feature, n: usize, example: String) {
        let t = self.at(feature);
        t.dropped += n;
        t.examples.push(example);
    }

    fn added(&mut self, feature: Feature, n: usize, example: String) {
        let t = self.at(feature);
        t.added += n;
        t.examples.push(example);
    }
}

/// Compares two games feature by feature: `before` as it was sent, `after`
/// as it came back. Their trees are walked side by side from the start,
/// moves matched by what they play, so a variation put elsewhere among a
/// move's continuations is the same variation in another order, and the
/// annotations of moves that went (or came) aren't counted again.
pub fn compare(before: &Game, after: &Game) -> Comparison {
    let mut c = Comparison::default();
    start_position(&mut c, before, after);
    orientation(&mut c, before, after);
    tags(&mut c, before, after);
    let mut comments = Comments::default();
    let mut stack = vec![Pair {
        a: NodeId::ROOT,
        b: NodeId::ROOT,
        pos_a: before.start_position(),
        pos_b: after.start_position(),
        label: String::new(),
        main_a: true,
        main_b: true,
    }];
    while let Some(p) = stack.pop() {
        annotations(&mut c, &mut comments, before.node(p.a), after.node(p.b), &p.label);
        let next = continuations(&mut c, before, after, &p);
        stack.extend(next.into_iter().rev());
    }
    comments.settle(&mut c);
    c
}

/// A node of each game, reached by the same moves.
struct Pair {
    a: NodeId,
    b: NodeId,
    /// The position at each (each game's own moves played on its own
    /// start, so they're always legal there).
    pos_a: Chess,
    pos_b: Chess,
    /// The move, as "3...Bc5 (in a variation)" in `before`; empty at the
    /// start.
    label: String,
    /// On each game's main line.
    main_a: bool,
    main_b: bool,
}

/// "3...Bc5", with " (in a variation)" off the main line.
fn move_label(pos: &Chess, mv: Move, main: bool) -> String {
    let n = pos.fullmoves().get();
    let dots = if pos.turn().is_white() { "." } else { "..." };
    let line = if main { "" } else { " (in a variation)" };
    format!("{n}{dots}{}{line}", san(pos, mv))
}

fn played(pos: &Chess, mv: Move) -> Chess {
    let mut next = pos.clone();
    next.play_unchecked(mv);
    next
}

/// The moves from `id` on (it included), and the variations among them
/// (it too when `starts_line`).
fn subtree(g: &Game, id: NodeId, starts_line: bool) -> (usize, usize) {
    let (mut moves, mut lines) = (0, usize::from(starts_line));
    let mut stack = vec![id];
    while let Some(n) = stack.pop() {
        moves += 1;
        let children = &g.node(n).children;
        lines += children.len().saturating_sub(1);
        stack.extend(children);
    }
    (moves, lines)
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") }
}

/// " and the 7 moves after it", or nothing for one move.
fn and_after(moves: usize) -> String {
    match moves {
        0 | 1 => String::new(),
        2 => " and the move after it".into(),
        n => format!(" and the {} moves after it", n - 1),
    }
}

/// The continuations of a pair's moves: those in both (to compare next),
/// those that went and came, and their order.
fn continuations(c: &mut Comparison, before: &Game, after: &Game, p: &Pair) -> Vec<Pair> {
    let (ac, bc) = (&before.node(p.a).children, &after.node(p.b).children);
    let mut taken = vec![false; bc.len()];
    // (index in `ac`, index in `bc`) of the moves in both, in `ac`'s order.
    let mut order = Vec::new();
    let mut next = Vec::new();
    for (i, &ca) in ac.iter().enumerate() {
        let mv = before.node(ca).mv.expect("non-root node has a move");
        let main_a = p.main_a && i == 0;
        let label = move_label(&p.pos_a, mv, main_a);
        let Some(j) = (0..bc.len()).find(|&j| !taken[j] && after.node(bc[j]).mv == Some(mv)) else {
            let (moves, lines) = subtree(before, ca, i > 0);
            c.dropped(Feature::Moves, moves, format!("dropped {label}{}", and_after(moves)));
            if lines > 0 {
                c.dropped(
                    Feature::Variations,
                    lines,
                    format!("dropped {} with {label}", plural(lines, "variation")),
                );
            }
            continue;
        };
        taken[j] = true;
        order.push((i, j));
        c.same(Feature::Moves);
        if i > 0 {
            c.same(Feature::Variations);
        }
        let pos_b = played(&p.pos_b, after.node(bc[j]).mv.expect("non-root node has a move"));
        let main_b = p.main_b && j == 0;
        next.push(Pair { a: ca, b: bc[j], pos_a: played(&p.pos_a, mv), pos_b, label, main_a, main_b });
    }
    for (j, &cb) in bc.iter().enumerate().filter(|&(j, _)| !taken[j]) {
        let mv = after.node(cb).mv.expect("non-root node has a move");
        let label = move_label(&p.pos_b, mv, p.main_b && j == 0);
        let (moves, lines) = subtree(after, cb, j > 0);
        c.added(Feature::Moves, moves, format!("added {label}{}", and_after(moves)));
        if lines > 0 {
            c.added(Feature::Variations, lines, format!("added {} with {label}", plural(lines, "variation")));
        }
    }
    let in_order = order.windows(2).all(|w| w[0].1 < w[1].1);
    // The main line stays the main line: its move is still the first, and
    // no variation took its place (even with one continuation in both).
    let main_kept = order.iter().all(|&(i, j)| (i == 0) == (j == 0));
    if in_order && main_kept {
        if order.len() >= 2 {
            c.same(Feature::VariationOrder);
        }
        return next;
    }
    let san_of = |i: usize| san(&p.pos_a, before.node(ac[i]).mv.expect("non-root node has a move"));
    let what = if !in_order {
        let sans =
            |list: &[(usize, usize)]| list.iter().map(|&(i, _)| san_of(i)).collect::<Vec<_>>().join(", ");
        let mut theirs = order.clone();
        theirs.sort_by_key(|&(_, j)| j);
        format!("{} → {}", sans(&order), sans(&theirs))
    } else if let Some(&(i, _)) = order.iter().find(|&&(i, j)| i != 0 && j == 0) {
        format!("{} is the main line now", san_of(i))
    } else {
        // In order, and no variation took its place: the main line's move
        // is after another now.
        format!("{} isn't the main line any more", san_of(0))
    };
    let place = if p.label.is_empty() { "the first moves".into() } else { format!("after {}", p.label) };
    c.changed(Feature::VariationOrder, format!("{place}: {what}"));
    next
}

/// A comment's text in quotes, cut short when long.
fn quote(text: &str) -> String {
    const MAX: usize = 48;
    if text.chars().count() <= MAX {
        format!("\"{text}\"")
    } else {
        format!("\"{}…\"", text.chars().take(MAX).collect::<String>())
    }
}

/// Comment places that lost or gained a comment, settled once a game is
/// walked: a comment that went from one place and came at another moved,
/// and one that went into another comment was merged.
#[derive(Default)]
struct Comments {
    /// (text, place).
    dropped: Vec<(String, String)>,
    added: Vec<(String, String)>,
    /// (the new text, place).
    changed: Vec<(String, String)>,
}

impl Comments {
    fn slot(&mut self, c: &mut Comparison, a: &str, b: &str, place: String) {
        match (a.is_empty(), b.is_empty()) {
            (true, true) => {}
            (false, true) => self.dropped.push((a.to_owned(), place)),
            (true, false) => self.added.push((b.to_owned(), place)),
            _ if a == b => c.same(Feature::Comments),
            _ => {
                c.changed(Feature::Comments, format!("{place}: {} → {}", quote(a), quote(b)));
                self.changed.push((b.to_owned(), place));
            }
        }
    }

    fn settle(mut self, c: &mut Comparison) {
        for (text, place) in self.dropped {
            if let Some(k) = self.added.iter().position(|(t, _)| *t == text) {
                let (_, to) = self.added.remove(k);
                c.changed(Feature::Comments, format!("{} moved from {place} to {to}", quote(&text)));
            } else if let Some((_, into)) =
                self.added.iter().chain(&self.changed).find(|(t, _)| t.contains(text.as_str()))
            {
                c.changed(
                    Feature::Comments,
                    format!("{} ({place}) went into the comment {into}", quote(&text)),
                );
            } else {
                c.dropped(Feature::Comments, 1, format!("{place}: {} dropped", quote(&text)));
            }
        }
        for (text, place) in self.added {
            c.added(Feature::Comments, 1, format!("{place}: {} added", quote(&text)));
        }
    }
}

/// The annotations of one move (or of the start) in each game.
fn annotations(c: &mut Comparison, comments: &mut Comments, a: &Node, b: &Node, label: &str) {
    let start = label.is_empty();
    let at = if start { "at the start".to_owned() } else { format!("at {label}") };
    if !start {
        comments.slot(c, &a.comment_before, &b.comment_before, format!("before {label}"));
    }
    let after = if start { "before the first move".to_owned() } else { format!("after {label}") };
    comments.slot(c, &a.comment_after, &b.comment_after, after);
    nags(c, &a.nags, &b.nags, &at);
    let arrows = |n: &Node| n.shapes.iter().filter(|s| s.to.is_some()).copied().collect::<Vec<_>>();
    let squares = |n: &Node| n.shapes.iter().filter(|s| s.to.is_none()).copied().collect::<Vec<_>>();
    shapes(c, Feature::Arrows, &arrows(a), &arrows(b), &at);
    shapes(c, Feature::Highlights, &squares(a), &squares(b), &at);
    commands(c, &a.commands, &b.commands, &at);
}

/// A NAG as `$n`, with its symbol when it has a usual one.
fn nag(n: Nag) -> String {
    let symbol = match n.0 {
        1 => "!",
        2 => "?",
        3 => "!!",
        4 => "??",
        5 => "!?",
        6 => "?!",
        7 => "□",
        10 => "=",
        13 => "∞",
        14 => "⩲",
        15 => "⩱",
        16 => "±",
        17 => "∓",
        18 => "+−",
        19 => "−+",
        22 => "⨀",
        32 => "⟳",
        36 => "↑",
        40 => "→",
        44 => "=∞",
        132 => "⇆",
        138 => "⊕",
        140 => "∆",
        146 => "N",
        _ => return format!("${}", n.0),
    };
    format!("${} ({symbol})", n.0)
}

/// NAGs: one that went while another of its kind (move or position
/// assessment) came changed.
fn nags(c: &mut Comparison, a: &[Nag], b: &[Nag], at: &str) {
    let mut added: Vec<Nag> = b.iter().filter(|n| !a.contains(n)).copied().collect();
    for &n in a {
        if b.contains(&n) {
            c.same(Feature::Nags);
            continue;
        }
        let kind = n.category();
        let paired =
            if kind == NagCategory::Other { None } else { added.iter().position(|x| x.category() == kind) };
        match paired {
            Some(k) => {
                let x = added.remove(k);
                c.changed(Feature::Nags, format!("{at}: {} → {}", nag(n), nag(x)));
            }
            None => c.dropped(Feature::Nags, 1, format!("{at}: {} dropped", nag(n))),
        }
    }
    for x in added {
        c.added(Feature::Nags, 1, format!("{at}: {} added", nag(x)));
    }
}

fn shape(s: Shape) -> String {
    match s.to {
        Some(to) => format!("{}{}{to}", s.color.letter(), s.from),
        None => format!("{}{}", s.color.letter(), s.from),
    }
}

/// Arrows or highlights: one on the same squares in another color changed.
fn shapes(c: &mut Comparison, feature: Feature, a: &[Shape], b: &[Shape], at: &str) {
    let mut added = b.to_vec();
    let mut dropped = Vec::new();
    for s in a {
        match added.iter().position(|x| x == s) {
            Some(k) => {
                added.remove(k);
                c.same(feature);
            }
            None => dropped.push(*s),
        }
    }
    for s in dropped {
        match added.iter().position(|x| x.from == s.from && x.to == s.to) {
            Some(k) => {
                let x = added.remove(k);
                c.changed(feature, format!("{at}: {} → {}", shape(s), shape(x)));
            }
            None => c.dropped(feature, 1, format!("{at}: {} dropped", shape(s))),
        }
    }
    for x in added {
        c.added(feature, 1, format!("{at}: {} added", shape(x)));
    }
}

/// `[%…]` commands, by name and in order: clocks, evals and the others.
fn commands(c: &mut Comparison, a: &[(String, String)], b: &[(String, String)], at: &str) {
    let mut names: Vec<&str> = Vec::new();
    for (name, _) in a.iter().chain(b) {
        if !names.contains(&name.as_str()) {
            names.push(name);
        }
    }
    for name in names {
        let feature = match name {
            "clk" => Feature::Clocks,
            "eval" => Feature::Evals,
            _ => Feature::OtherCommands,
        };
        let values = |list: &[(String, String)]| -> Vec<String> {
            list.iter().filter(|(n, _)| n == name).map(|(_, v)| v.clone()).collect()
        };
        let (av, bv) = (values(a), values(b));
        for k in 0..av.len().max(bv.len()) {
            match (av.get(k), bv.get(k)) {
                (Some(x), Some(y)) if x == y => c.same(feature),
                (Some(x), Some(y)) => {
                    let how =
                        if same_value(name, x, y) { " (the same value, written another way)" } else { "" };
                    c.changed(feature, format!("{at}: [%{name} {x}] → [%{name} {y}]{how}"));
                }
                (Some(x), None) => c.dropped(feature, 1, format!("{at}: [%{name} {x}] dropped")),
                (None, Some(y)) => c.added(feature, 1, format!("{at}: [%{name} {y}] added")),
                (None, None) => {}
            }
        }
    }
}

/// Whether two values of a command mean the same: a time to the tenth of a
/// second, an eval without its depth.
fn same_value(name: &str, x: &str, y: &str) -> bool {
    match name {
        "clk" | "emt" => matches!((seconds(x), seconds(y)), (Some(p), Some(q)) if (p - q).abs() < 0.05),
        "eval" => eval(x).is_some() && eval(x) == eval(y),
        _ => x.trim() == y.trim(),
    }
}

/// `h:mm:ss[.f]` in seconds.
fn seconds(time: &str) -> Option<f64> {
    time.trim().split(':').try_fold(0.0, |total, part| Some(total * 60.0 + part.parse::<f64>().ok()?))
}

/// An eval without its depth (`0.25,22`): (a mate, centipawns or moves).
fn eval(value: &str) -> Option<(bool, i64)> {
    let v = value.split(',').next()?.trim();
    match v.strip_prefix('#') {
        Some(mate) => Some((true, mate.parse().ok()?)),
        None => Some((false, (v.parse::<f64>().ok()? * 100.0).round() as i64)),
    }
}

fn start_position(c: &mut Comparison, before: &Game, after: &Game) {
    let f = Feature::StartPosition;
    match (before.setup.as_ref().map(to_fen), after.setup.as_ref().map(to_fen)) {
        (None, None) => {}
        (Some(x), Some(y)) if x == y => c.same(f),
        (Some(x), Some(y)) => c.changed(f, format!("{x} → {y}")),
        (Some(x), None) => c.dropped(f, 1, format!("{x} dropped: the game starts from the usual position")),
        (None, Some(y)) => c.added(f, 1, format!("{y} added")),
    }
}

fn orientation(c: &mut Comparison, before: &Game, after: &Game) {
    let f = Feature::Orientation;
    let get = |g: &Game| g.headers.get_extra("Orientation").map(str::to_ascii_lowercase);
    match (get(before), get(after)) {
        (None, None) => {}
        (Some(x), Some(y)) if x == y => c.same(f),
        (Some(x), Some(y)) => c.changed(f, format!("{x} → {y}")),
        (Some(x), None) => c.dropped(f, 1, format!("{x} dropped")),
        (None, Some(y)) => c.added(f, 1, format!("{y} added (the game had none)")),
    }
}

/// A game's tags with a value, but `Orientation` and [`STUDY_TAGS`].
fn tag_list(g: &Game) -> Vec<(String, String)> {
    g.headers
        .to_pairs()
        .into_iter()
        .filter(|(k, v)| {
            !matches!(v.as_str(), "" | "?" | "????.??.??")
                && k != "Orientation"
                && !STUDY_TAGS.contains(&k.as_str())
        })
        .collect()
}

fn tags(c: &mut Comparison, before: &Game, after: &Game) {
    let f = Feature::Tags;
    let (a, b) = (tag_list(before), tag_list(after));
    for (k, v) in &a {
        match b.iter().find(|(n, _)| n == k) {
            Some((_, w)) if w == v => c.same(f),
            Some((_, w)) => c.changed(f, format!("{k}: \"{v}\" → \"{w}\"")),
            None => c.dropped(f, 1, format!("{k} \"{v}\" dropped")),
        }
    }
    for (k, w) in b.iter().filter(|(n, _)| !a.iter().any(|(m, _)| m == n)) {
        c.added(f, 1, format!("{k} \"{w}\" added"));
    }
}

// ---- The check ----------------------------------------------------------------

/// What [`round_trip`] found.
#[derive(Debug)]
pub struct RoundTrip {
    /// Where the games went; `None` when there were none to send.
    pub study: Option<SentStudy>,
    /// The chapters the study had when fetched back.
    pub chapters: usize,
    /// Each game sent against its chapter, in order.
    pub games: Vec<GameCheck>,
    /// All of them together.
    pub total: Comparison,
    /// One chapter's moves replaced and fetched back.
    pub push: Option<PushCheck>,
    /// Why replacing the moves failed, when it did (the rest of the report
    /// stands).
    pub push_error: Option<NetError>,
    /// Why the check stopped before the end once the study was made, when
    /// it did: the games weren't all taken, the study couldn't be fetched
    /// back, a cancel. What came before stands, the study's link with it.
    pub stopped: Option<NetError>,
    /// Anything else to know: what Lichess said, what our reader warned
    /// about, chapters that don't line up with the games.
    pub notes: Vec<String>,
    /// The PGN each way, to look at.
    pub texts: Texts,
}

/// The PGN texts of the check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Texts {
    /// The games as sent to the study.
    pub sent: String,
    /// The study's export.
    pub back: String,
    /// The edited game sent to replace a chapter's moves.
    pub pushed: String,
    /// That chapter's export afterwards.
    pub pushed_back: String,
}

/// A game sent, against its chapter.
#[derive(Debug)]
pub struct GameCheck {
    /// 1 for the first game.
    pub number: usize,
    pub name: String,
    /// Its chapter's id (from its `ChapterURL`).
    pub chapter: Option<String>,
    /// `None` when the chapter can't be read.
    pub diff: Option<Comparison>,
}

/// A chapter's moves replaced with an edited version of its game.
#[derive(Debug)]
pub struct PushCheck {
    /// The game's number.
    pub game: usize,
    pub chapter: String,
    /// The edits, in words.
    pub edits: Vec<String>,
    /// The chapter's export still names it (its `ChapterURL`).
    pub kept_id: bool,
    /// The edited game against the chapter fetched back.
    pub diff: Comparison,
}

/// Sends `games` (1 to 64) to a new private study named `name`, fetches
/// them back and compares each with its chapter; then replaces the moves
/// of the first chapter with at least four moves with an edited version of
/// its game, fetches that chapter and compares it with what was sent.
/// Errors before the study is made are errors (nothing is on Lichess then).
/// After, the report always comes back with the study in it: a failure is
/// in `stopped` (or `push_error` for the moves), with what came before.
pub fn round_trip(
    client: &Client,
    token: &Token,
    games: &[Game],
    name: &str,
    progress: &dyn Progress,
) -> Result<RoundTrip, NetError> {
    let mut report = RoundTrip {
        study: None,
        chapters: 0,
        games: Vec::new(),
        total: Comparison::default(),
        push: None,
        push_error: None,
        stopped: None,
        notes: Vec::new(),
        texts: Texts::default(),
    };
    if games.is_empty() {
        report.notes.push("There were no games to send: nothing was asked of Lichess.".into());
        return Ok(report);
    }
    let name = check_study_name(name)?;
    // One study, whatever the games: never more than one of the day's 30.
    if games.len() > MAX_CHAPTERS {
        return Err(NetError::StudyFull { name: name.to_owned(), room: MAX_CHAPTERS });
    }

    // The app's own PGN goes out, as Send to study… sends it, and what our
    // reader makes of it is what's compared.
    let sent: Vec<String> = games.iter().map(|g| write_game(g).trim().to_owned()).collect();
    let mut before = Vec::with_capacity(games.len());
    for (n, (game, text)) in games.iter().zip(&sent).enumerate() {
        let (read, _) = read_one(text);
        if read.as_ref() != Some(game) {
            report.notes.push(format!(
                "Game {}: the app's PGN writer and reader don't give it back as it was (our fault, not Lichess's)",
                n + 1
            ));
        }
        before.push(read.unwrap_or_else(|| game.clone()));
    }
    report.texts.sent = format!("{}\n", sent.join("\n\n\n"));

    let target = ExportTarget::New { name: name.to_owned(), visibility: Visibility::Private };
    let summary = export_to_studies(client, token, &target, &sent, progress)?;
    // The export is an error when it made no study, but for a cancel.
    let Some(study) = summary.studies.into_iter().next() else {
        return Err(if summary.cancelled { NetError::Cancelled } else { bad("no study was made") });
    };
    if let Some(r) = &study.refused {
        report.notes.push(format!("Lichess said about the games: {r}"));
    }
    if summary.too_long > 0 {
        report.notes.push(format!("{} were too long to send", plural(summary.too_long, "game")));
    }
    report.study = Some(study.clone());
    // From here the study is on the account: whatever goes wrong, the
    // report says where it is.
    report.stopped = match (summary.cancelled, summary.stopped) {
        (true, _) => Some(NetError::Cancelled),
        (false, Some(e)) => Some(e),
        (false, None) => check(client, token, &study, &before, progress, &mut report).err(),
    };
    Ok(report)
}

/// The study fetched back and each game compared with its chapter, then a
/// chapter's moves replaced (a failure there is in `push_error`).
fn check(
    client: &Client,
    token: &Token,
    study: &SentStudy,
    before: &[Game],
    progress: &dyn Progress,
    report: &mut RoundTrip,
) -> Result<(), NetError> {
    let stop = || progress.is_cancelled();
    let cancelled = || if progress.is_cancelled() { Err(NetError::Cancelled) } else { Ok(()) };
    cancelled()?;
    let link = StudyRef { study: study.id.clone(), chapter: None };
    let back = fetch_study(client, &link, Some(token), None, &stop)?.ok_or_else(|| bad("a 304 unasked"))?;
    report.chapters = back.chapters.len();
    let texts: Vec<&str> = back.chapters.iter().map(|c| c.pgn.as_str()).collect();
    report.texts.back = format!("{}\n", texts.join("\n\n\n"));
    let mut chapters: Vec<&Chapter> = back.chapters.iter().collect();
    if chapters.len() == before.len() + 1 && chapters.first().is_some_and(|c| is_empty_chapter(c)) {
        chapters.remove(0);
        report.notes.push("Lichess kept the empty chapter a new study starts with".into());
    }
    if chapters.len() != before.len() {
        report.notes.push(format!(
            "{} sent and the study has {}: compared in order",
            plural(before.len(), "game"),
            plural(chapters.len(), "chapter")
        ));
    }
    let mut current = Vec::new();
    for (i, (game, chapter)) in before.iter().zip(&chapters).enumerate() {
        let (read, warnings) = read_one(&chapter.pgn);
        for w in warnings {
            report.notes.push(format!("Chapter {}: our PGN reader says: {w}", i + 1));
        }
        let diff = read.as_ref().map(|after| compare(game, after));
        match &diff {
            Some(d) => report.total.absorb(d, &format!("game {}", i + 1)),
            None => report.notes.push(format!("Chapter {}: our PGN reader can't read it", i + 1)),
        }
        report.games.push(GameCheck {
            number: i + 1,
            name: game_name(game),
            chapter: chapter_id(chapter),
            diff,
        });
        current.push(read);
    }

    // Replacing the moves of a chapter, as a sync would push a game.
    let pick = (0..current.len()).find(|&i| {
        report.games[i].chapter.is_some() && current[i].is_some() && before[i].mainline_len() >= 4
    });
    let Some(i) = pick else {
        report.notes.push("No chapter with four moves or more had an id: its moves weren't replaced".into());
        return Ok(());
    };
    cancelled()?;
    let chapter = report.games[i].chapter.clone().expect("picked for its id");
    let link = StudyRef { study: study.id.clone(), chapter: Some(chapter.clone()) };
    let now = current[i].as_ref().expect("picked as read");
    match push(client, token, &link, &before[i], now, &stop, report) {
        Ok((edits, kept_id, diff)) => {
            report.push = Some(PushCheck { game: i + 1, chapter, edits, kept_id, diff });
        }
        Err(e) => report.push_error = Some(e),
    }
    Ok(())
}

/// Replaces the moves of the chapter `link` names with `sent` edited, under
/// the chapter's tags as Lichess has them (`now`), and compares what comes
/// back with it: the edits, whether the chapter kept its id, and the
/// comparison.
fn push(
    client: &Client,
    token: &Token,
    link: &StudyRef,
    sent: &Game,
    now: &Game,
    stop: &dyn Fn() -> bool,
    report: &mut RoundTrip,
) -> Result<(Vec<String>, bool, Comparison), NetError> {
    let chapter = link.chapter.as_deref().ok_or_else(|| bad("no chapter"))?;
    let mut edited = sent.clone();
    edited.headers = now.headers.clone();
    let edits = edit(&mut edited);
    report.texts.pushed = write_game(&edited);
    replace_chapter_moves(client, token, &link.study, chapter, &report.texts.pushed)?;
    let back = fetch_study(client, link, Some(token), None, stop)?.ok_or_else(|| bad("a 304 unasked"))?;
    let got = back.chapters.first().ok_or_else(|| bad("the chapter's export has no game"))?;
    report.texts.pushed_back = format!("{}\n", got.pgn);
    let (read, warnings) = read_one(&got.pgn);
    for w in &warnings {
        report.notes.push(format!("The chapter after its moves were replaced: our PGN reader says: {w}"));
    }
    let after = read.ok_or_else(|| bad("the chapter's export can't be read"))?;
    let kept_id = chapter_id(got).as_deref() == Some(chapter);
    Ok((edits, kept_id, compare(&edited, &after)))
}

fn bad(reason: &str) -> NetError {
    NetError::BadData { site: Site::Lichess, reason: reason.to_owned() }
}

/// The first game of a PGN text as our reader reads it, and its warnings.
fn read_one(pgn: &str) -> (Option<Game>, Vec<String>) {
    match PgnReader::new(pgn.as_bytes()).next() {
        Some(Ok(p)) => (p.game, p.warnings.into_iter().map(|w| w.message).collect()),
        Some(Err(e)) => (None, vec![e.to_string()]),
        None => (None, vec!["there's no game in it".into()]),
    }
}

/// A chapter's id, from its `ChapterURL`.
fn chapter_id(chapter: &Chapter) -> Option<String> {
    let url = tag_value(&chapter.pgn, "ChapterURL")?;
    parse_study_link(&url).ok()?.chapter
}

fn is_empty_chapter(chapter: &Chapter) -> bool {
    read_one(&chapter.pgn).0.is_some_and(|g| g.mainline_len() == 0 && !g.root().has_annotations())
}

/// "Event, White - Black", or what there is of it.
fn game_name(g: &Game) -> String {
    let h = &g.headers;
    let players = (!h.white.is_empty() && !h.black.is_empty()).then(|| format!("{} - {}", h.white, h.black));
    match (h.event.is_empty(), players) {
        (false, Some(p)) => format!("{}, {p}", h.event),
        (false, None) => h.event.clone(),
        (true, Some(p)) => p,
        (true, None) => "a game without names".into(),
    }
}

fn main_line(g: &Game) -> Vec<NodeId> {
    let mut line = Vec::new();
    let mut cur = NodeId::ROOT;
    while let Some(&next) = g.node(cur).children.first() {
        line.push(next);
        cur = next;
    }
    line
}

/// A move of a game, as "3...Bc5 (in a variation)".
fn node_label(g: &Game, id: NodeId) -> String {
    let parent = g.node(id).parent.expect("a move has a parent");
    let mv = g.node(id).mv.expect("non-root node has a move");
    move_label(&g.position_at(parent), mv, g.variation_start(id).is_none())
}

/// Edits a game as someone might between two syncs, and says how: a
/// comment changed and one added; an arrow, a highlight and an eval added;
/// a NAG added and one changed; a clock changed; a move's variations put in
/// another order, one variation removed and one added.
fn edit(g: &mut Game) -> Vec<String> {
    let mut done = Vec::new();
    let main = main_line(g);
    let Some(&first) = main.first() else { return done };
    if let Some(id) = g.preorder().into_iter().skip(1).find(|&id| !g.node(id).comment_after.is_empty()) {
        g.node_mut(id).comment_after.push_str(" (edited)");
        done.push(format!("the comment after {} changed", node_label(g, id)));
    }
    let bare = |n: &Node| n.comment_after.is_empty() && n.shapes.is_empty() && n.commands.is_empty();
    if let Some(&id) = main.iter().find(|&&id| bare(g.node(id))) {
        let n = g.node_mut(id);
        n.comment_after = "Added by the round-trip check.".into();
        // Squares before arrows, as the PGN writer puts them.
        n.shapes.push(Shape { color: ShapeColor::Yellow, from: Square::D4, to: None });
        n.shapes.push(Shape { color: ShapeColor::Blue, from: Square::A1, to: Some(Square::H8) });
        n.commands.push(("eval".into(), "0.42".into()));
        done.push(format!(
            "a comment, the arrow Ba1h8, the highlight Yd4 and [%eval 0.42] added after {}",
            node_label(g, id)
        ));
    }
    if let Some(&id) = main.iter().find(|&&id| g.node(id).nags.is_empty()) {
        g.node_mut(id).nags.push(Nag(1));
        done.push(format!("{} added to {}", nag(Nag(1)), node_label(g, id)));
    }
    let assessed = g.preorder().into_iter().find_map(|id| {
        let k = g.node(id).nags.iter().position(|n| n.category() == NagCategory::Move && n.0 != 1)?;
        Some((id, k))
    });
    if let Some((id, k)) = assessed {
        let old = g.node(id).nags[k];
        let new = Nag(if old.0 == 6 { 2 } else { 6 });
        g.node_mut(id).nags[k] = new;
        done.push(format!("{} changed to {} at {}", nag(old), nag(new), node_label(g, id)));
    }
    let timed = g.preorder().into_iter().find(|&id| g.node(id).commands.iter().any(|(k, _)| k == "clk"));
    if let Some(id) = timed
        && let Some(slot) = g.node_mut(id).commands.iter_mut().find(|(k, _)| k == "clk")
    {
        let old = std::mem::replace(&mut slot.1, "0:01:23".into());
        done.push(format!("the clock after {} changed from {old} to 0:01:23", node_label(g, id)));
    }
    let reordered = g.preorder().into_iter().find(|&id| g.node(id).children.len() >= 3);
    if let Some(id) = reordered {
        g.node_mut(id).children[1..].reverse();
        let place =
            if id == NodeId::ROOT { "at the start".into() } else { format!("after {}", node_label(g, id)) };
        done.push(format!("the variations {place} put in another order"));
    }
    let branching =
        g.preorder().into_iter().find(|&id| Some(id) != reordered && g.node(id).children.len() >= 2);
    if let Some(id) = branching {
        let gone = g.node_mut(id).children.pop().expect("two continuations");
        done.push(format!("the variation {} removed", node_label(g, gone)));
    }
    // Another move in place of the main line's second, and a reply.
    let pos = g.position_at(first);
    let there: Vec<Move> = g.node(first).children.iter().filter_map(|&c| g.node(c).mv).collect();
    if let Some(mv) = pos.legal_moves().into_iter().find(|m| !there.contains(m)) {
        let new = g.add_child(first, mv);
        g.node_mut(new).comment_after = "A line added by the round-trip check.".into();
        if let Some(reply) = played(&pos, mv).legal_moves().into_iter().next() {
            g.add_child(new, reply);
        }
        done.push(format!("the variation {} added, with a comment", node_label(g, new)));
    }
    done
}

// ---- The report ---------------------------------------------------------------

/// Examples shown per feature.
const EXAMPLES: usize = 8;

fn table(f: &mut fmt::Formatter<'_>, c: &Comparison) -> fmt::Result {
    writeln!(f, "   {:<22}{:>7}{:>9}{:>9}{:>7}", "", "same", "changed", "dropped", "added")?;
    for feature in Feature::ALL {
        let t = c.get(feature);
        if t.same + t.differences() == 0 {
            writeln!(f, "   {:<22}{:>7}", feature.name(), "-")?;
        } else {
            writeln!(
                f,
                "   {:<22}{:>7}{:>9}{:>9}{:>7}",
                feature.name(),
                t.same,
                t.changed,
                t.dropped,
                t.added
            )?;
        }
    }
    Ok(())
}

fn examples(f: &mut fmt::Formatter<'_>, c: &Comparison) -> fmt::Result {
    if c.differences() == 0 {
        return writeln!(f, "   Nothing differs.");
    }
    writeln!(f, "   What differs:")?;
    for feature in Feature::ALL {
        let t = c.get(feature);
        if t.examples.is_empty() {
            continue;
        }
        writeln!(f, "   {}:", feature.name())?;
        for e in t.examples.iter().take(EXAMPLES) {
            writeln!(f, "     {e}")?;
        }
        if t.examples.len() > EXAMPLES {
            writeln!(f, "     … and {} more", t.examples.len() - EXAMPLES)?;
        }
    }
    Ok(())
}

impl fmt::Display for RoundTrip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(study) = &self.study else {
            for n in &self.notes {
                writeln!(f, "{n}")?;
            }
            return Ok(());
        };
        writeln!(f, "Study: {} (private, \"{}\")", study.url, study.name)?;
        writeln!(
            f,
            "{} sent, {} made, {} fetched back.",
            plural(study.sent, "game"),
            plural(study.chapters, "chapter"),
            plural(self.chapters, "chapter")
        )?;
        if let Some(e) = &self.stopped {
            writeln!(f, "The check stopped: {e}")?;
        }
        // Compared once the study came back.
        if !self.games.is_empty() {
            compared(f, self)?;
        }
        if !self.notes.is_empty() {
            writeln!(f)?;
            writeln!(f, "Notes:")?;
            for n in &self.notes {
                writeln!(f, "- {n}")?;
            }
        }
        writeln!(f)?;
        writeln!(f, "The study stays on the account: delete it on Lichess when you're done with it.")
    }
}

/// The report's two parts: the round trip, and the chapter's moves replaced.
fn compared(f: &mut fmt::Formatter<'_>, r: &RoundTrip) -> fmt::Result {
    writeln!(f, "(\"-\": nothing of the kind in the games.)")?;
    writeln!(f)?;
    writeln!(f, "1. The games sent to the study and fetched back, compared as move trees")?;
    table(f, &r.total)?;
    writeln!(f, "   Not counted: the {} tags every chapter gets.", STUDY_TAGS.join(", "))?;
    for g in &r.games {
        let what = match &g.diff {
            Some(d) if d.differences() == 0 => "nothing differs".to_owned(),
            Some(d) => plural(d.differences(), "difference"),
            None => "can't be read".to_owned(),
        };
        let chapter = g.chapter.as_deref().unwrap_or("?");
        writeln!(f, "   game {}: {} (chapter {chapter}): {what}", g.number, g.name)?;
    }
    examples(f, &r.total)?;
    writeln!(f)?;
    writeln!(
        f,
        "2. A chapter's moves replaced in place (POST /api/study/{{id}}/{{chapter}}/moves) and fetched back"
    )?;
    match (&r.push, &r.push_error, &r.stopped) {
        (Some(p), _, _) => {
            writeln!(f, "   Game {} (chapter {}), sent with these edits:", p.game, p.chapter)?;
            for e in &p.edits {
                writeln!(f, "   - {e}")?;
            }
            writeln!(f, "   The chapter kept its id: {}.", if p.kept_id { "yes" } else { "no" })?;
            table(f, &p.diff)?;
            examples(f, &p.diff)
        }
        (None, Some(e), _) => writeln!(f, "   Failed: {e}"),
        (None, None, Some(_)) => writeln!(f, "   Not tried: the check stopped first."),
        (None, None, None) => writeln!(f, "   Not tried (see the notes)."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::pgn::read_pgn_str;

    fn game(pgn: &str) -> Game {
        read_pgn_str(pgn).into_iter().next().expect("a game")
    }

    #[test]
    fn the_same_game_has_no_differences() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn/study-roundtrip.pgn");
        let text = std::fs::read_to_string(path).unwrap();
        let mut total = Comparison::default();
        for (n, p) in PgnReader::new(text.as_bytes()).enumerate() {
            let p = p.unwrap();
            assert!(p.warnings.is_empty(), "game {}: {:?}", n + 1, p.warnings);
            let g = p.game.unwrap();
            let diff = compare(&g, &g);
            assert_eq!(diff.differences(), 0, "game {}: {diff:?}", n + 1);
            total.absorb(&diff, "game");
        }
        // The fixture has some of everything the check counts.
        for f in Feature::ALL {
            assert!(total.get(f).same > 0, "the fixture has no {}", f.name());
        }
        assert!(total.get(Feature::VariationOrder).same >= 2);
    }

    #[test]
    fn annotations_that_went_came_or_changed() {
        let a = game(
            "[Orientation \"black\"]\n[Annotator \"Me\"]\n[PlyCount \"3\"]\n\n\
             { Intro } 1. e4 $1 { Good [%clk 0:05:00] [%eval 0.30,20] [%csl Ge4] [%cal Gg1f3] } \
             1... e5 $14 { Solid } 2. Nf3 { [%emt 0:00:04] [%clk 0:04:58] } *",
        );
        let b = game(
            "[Annotator \"https://lichess.org/@/me\"]\n[UTCDate \"2026.11.12\"]\n\n\
             1. e4 $3 { Good [%clk 0:05:00.0] [%eval 0.3] [%csl Re4] [%cal Gg1f3,Bd1h5] } \
             1... e5 $146 2. Nf3 { Intro Solid [%clk 0:04:59] } *",
        );
        let d = compare(&a, &b);
        let t = |f| d.get(f).clone();
        assert_eq!((t(Feature::Moves).same, t(Feature::Moves).differences()), (3, 0));
        // "Intro" and "Solid" went into a comment that came; "Good" stayed.
        let c = t(Feature::Comments);
        assert_eq!((c.same, c.changed, c.dropped, c.added), (1, 2, 0, 1), "{c:?}");
        assert!(c.examples.iter().any(|e| e.contains("\"Intro\"") && e.contains("went into")), "{c:?}");
        // $1 → $3 (both move assessments), $14 dropped, $146 added.
        let n = t(Feature::Nags);
        assert_eq!((n.same, n.changed, n.dropped, n.added), (0, 1, 1, 1), "{n:?}");
        assert!(n.examples[0].contains("$1 (!) → $3 (!!)"), "{n:?}");
        let h = t(Feature::Highlights);
        assert_eq!((h.changed, h.examples[0].as_str()), (1, "at 1.e4: Ge4 → Re4"));
        let ar = t(Feature::Arrows);
        assert_eq!((ar.same, ar.added), (1, 1));
        let clk = t(Feature::Clocks);
        assert_eq!((clk.same, clk.changed), (0, 2), "{clk:?}");
        assert!(clk.examples[0].ends_with("(the same value, written another way)"), "{clk:?}");
        assert!(!clk.examples[1].contains("written another way"), "0:04:58 isn't 0:04:59");
        let ev = t(Feature::Evals);
        assert_eq!(ev.changed, 1);
        assert!(ev.examples[0].contains("written another way"), "the depth went: {ev:?}");
        assert_eq!(t(Feature::OtherCommands).dropped, 1);
        assert_eq!(t(Feature::Orientation).dropped, 1);
        let tags = t(Feature::Tags);
        // Result stays; Annotator changed; PlyCount went; UTCDate came.
        assert_eq!((tags.same, tags.changed, tags.dropped, tags.added), (1, 1, 1, 1), "{tags:?}");
    }

    #[test]
    fn moves_variations_and_their_order() {
        let a = game(
            "1. e4 e5 (1... c5 2. Nf3 (2. c3 d5) 2... d6) (1... e6 2. d4) (1... c6) 2. Nf3 Nc6 3. Bb5 a6 *",
        );
        // The variations in another order, 1...e6 now the main line, 2.c3's
        // line gone, a move added at the end and the start position kept.
        let b = game("1. e4 e6 (1... c6) (1... c5 2. Nf3 d6) (1... e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4) 2. d4 *");
        let d = compare(&a, &b);
        let moves = d.get(Feature::Moves);
        assert_eq!((moves.dropped, moves.added), (2, 1), "{moves:?}");
        assert!(moves.examples.contains(&"dropped 2.c3 (in a variation) and the move after it".to_owned()));
        assert!(moves.examples.contains(&"added 4.Ba4 (in a variation)".to_owned()), "{moves:?}");
        let v = d.get(Feature::Variations);
        assert_eq!((v.same, v.dropped), (3, 1), "{v:?}");
        let o = d.get(Feature::VariationOrder);
        assert_eq!((o.same, o.changed), (0, 1), "{o:?}");
        assert_eq!(o.examples[0], "after 1.e4: e5, c5, e6, c6 → e6, c6, c5, e5");
        assert!(d.get(Feature::StartPosition).same + d.get(Feature::StartPosition).differences() == 0);

        let from = |fen: &str| game(&format!("[SetUp \"1\"]\n[FEN \"{fen}\"]\n\n *"));
        let kp = "8/8/4k3/8/4PK2/8/8/8 b - - 0 41";
        let d = compare(&from(kp), &from(&kp.replace(" 41", " 1")));
        assert_eq!(d.get(Feature::StartPosition).changed, 1);
        let d = compare(&from(kp), &game("*"));
        assert_eq!(d.get(Feature::StartPosition).dropped, 1);
    }

    /// A main line that became a variation, or a variation the main line,
    /// changed the order even where the moves left are in their order.
    #[test]
    fn a_new_main_line_is_a_change_of_order() {
        let order = |a: &str, b: &str| d_order(&compare(&game(a), &game(b)));
        fn d_order(d: &Comparison) -> (usize, usize, Vec<String>) {
            let t = d.get(Feature::VariationOrder);
            (t.same, t.changed, t.examples.clone())
        }
        // 1...d5 came first: 1...e5 is a variation now, 1...c5 still after it.
        let (same, changed, ex) = order("1. e4 e5 (1... c5) *", "1. e4 d5 (1... e5) (1... c5) *");
        assert_eq!((same, changed), (0, 1), "{ex:?}");
        assert_eq!(ex, ["after 1.e4: e5 isn't the main line any more"]);
        // The main line went and its variation took its place.
        let (same, changed, ex) = order("1. e4 e5 (1... c5) *", "1. e4 c5 *");
        assert_eq!((same, changed), (0, 1), "{ex:?}");
        assert_eq!(ex, ["after 1.e4: c5 is the main line now"]);
        // Only a variation went: the order of the rest is the same.
        assert_eq!(order("1. e4 e5 (1... c5) (1... e6) *", "1. e4 e5 (1... e6) *"), (1, 0, vec![]));
        // One continuation each, the same: nothing to order.
        assert_eq!(order("1. e4 e5 *", "1. e4 e5 *"), (0, 0, vec![]));
    }

    /// A comment that went from one move to another, as it was, moved: one
    /// change, not one dropped and one added.
    #[test]
    fn a_comment_moved_as_it_was() {
        let d =
            compare(&game("1. e4 { The centre. } e5 2. Nf3 *"), &game("1. e4 e5 { The centre. } 2. Nf3 *"));
        let c = d.get(Feature::Comments);
        assert_eq!((c.same, c.changed, c.dropped, c.added), (0, 1, 0, 0), "{c:?}");
        assert_eq!(c.examples, ["\"The centre.\" moved from after 1.e4 to after 1...e5"]);
    }

    #[test]
    fn the_edits_touch_every_feature_they_say() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn/study-roundtrip.pgn");
        let text = std::fs::read_to_string(path).unwrap();
        let original = read_pgn_str(&text).into_iter().next().unwrap();
        let mut edited = original.clone();
        let edits = edit(&mut edited);
        assert_eq!(edits.len(), 8, "{edits:#?}");
        let d = compare(&original, &edited);
        let t = |f| d.get(f).clone();
        // The added line's comment counts with the line, not here.
        assert_eq!(
            (t(Feature::Comments).changed, t(Feature::Comments).added),
            (1, 1),
            "{:?}",
            t(Feature::Comments)
        );
        assert_eq!((t(Feature::Nags).changed, t(Feature::Nags).added), (1, 1));
        assert_eq!((t(Feature::Arrows).added, t(Feature::Highlights).added), (1, 1));
        assert_eq!((t(Feature::Clocks).changed, t(Feature::Evals).added), (1, 1));
        assert_eq!(t(Feature::VariationOrder).changed, 1);
        // 4...Bc5 went with the variation inside it (5.Bxf7+).
        assert_eq!((t(Feature::Variations).dropped, t(Feature::Variations).added), (2, 1));
        assert_eq!(t(Feature::Tags).differences() + t(Feature::StartPosition).differences(), 0);
        // What's sent reads back as edited.
        assert_eq!(read_pgn_str(&write_game(&edited))[0], edited);
    }
}
