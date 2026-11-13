//! What a linked game (NET-13) holds on each side. The user's decision: a
//! linked game keeps in this database everything Lichess can't hold,
//! attached to the same moves, and only what Lichess can hold goes to it and
//! comes from it. What Lichess holds is what the study round-trip check
//! found on lichess.org (`docs/feature-catalog.md` › NET-13 round-trip
//! check › On lichess.org); the rules are in `docs/net-13-sync.md`.
//!
//! - [`lichess_projection`]: the game cut down to what Lichess holds of it,
//!   as our PGN writer and reader carry it. It's what a push sends, and a
//!   fixed point of our PGN path: written and read back, it's itself.
//! - [`local_only_layer`]: what the projection took away, keyed by move
//!   path (the moves from the start), so it survives lines reordered or
//!   added on Lichess; stored with the game's link
//!   (`chess_db::StudyLink::layer`) as [`Layer::to_bytes`] writes it.
//! - [`apply_layer`]: a game pulled from Lichess with the layer put back
//!   where its moves still are; what's left over is returned as orphaned,
//!   never dropped.
//! - [`normalize_for_compare`] and [`is_lichess_addition`]: what the later
//!   three-way merge compares, without what Lichess adds or rewrites by
//!   itself (its ECO, Opening, Annotator, Orientation and result comment).
//!
//! No network: pure functions of games.

use std::collections::{HashMap, HashSet};

use chess_core::game::{Game, Headers, Nag, Node, NodeId, Shape, ShapeColor};
use chess_core::notation::san;
use chess_core::position::{from_fen, to_fen};
use chess_core::shakmaty::{Chess, Move, Position, Square};
use chess_db::DbError;
use chess_db::blob::{move_at, move_index};
use chess_db::bytes::{ByteReader, put_bytes, put_str, put_varint};

use crate::NetError;

/// The NAGs (glyphs) Lichess keeps: those it kept on lichess.org, sent in
/// the fixture (`testdata/pgn/study-roundtrip.pgn`) and fetched back. Any
/// other is held here only: `$142`, which it dropped, and every NAG the
/// fixture didn't have (`$3` among them), until a run shows Lichess keeps
/// it. One list, so one more run's findings go here.
pub const LICHESS_NAGS: [u8; 16] = [1, 2, 4, 5, 6, 7, 13, 14, 18, 22, 36, 40, 44, 132, 140, 146];

/// The tags beside the roster that Lichess keeps as sent (on lichess.org:
/// with Event, Site, Date, Round, White, Black, Result, WhiteElo and
/// BlackElo, which a game always has; and SetUp and FEN, which are its start
/// position). A projection has them in this order. Any other is held here
/// only: ECO, Opening and Annotator (Lichess puts its own), EventDate (it
/// drops it), its study's StudyName, ChapterName and ChapterURL, and tags it
/// wasn't sent. `Orientation` is Lichess's `white` when a game has none.
pub const LICHESS_TAGS: [&str; 5] = ["WhiteTitle", "BlackTitle", "TimeControl", "Termination", "Orientation"];

/// The comments Lichess puts on the main line's last move of a finished
/// game that has none there: the first and last seen on lichess.org, the
/// second by analogy.
pub const RESULT_COMMENTS: [&str; 3] = ["1-0 White wins.", "0-1 Black wins.", "1/2-1/2 The game is a draw."];

const ORIENTATION: &str = "Orientation";
const CLOCK: &str = "clk";

/// The moves from a game's start position to one of its nodes, in order;
/// none for the start itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct MovePath(pub Vec<Move>);

/// What a node (a move, or the start) has that Lichess doesn't hold, or
/// holds otherwise: all of it is kept here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeExtras {
    /// Its comment before the move (a variation's first move): Lichess drops
    /// them.
    pub comment_before: String,
    /// Its comment after the move as here, when what Lichess holds of it is
    /// written otherwise (spaces, line breaks, a `}`, which our PGN writer
    /// can't put in a comment).
    pub comment_after: Option<String>,
    /// All its NAGs, in their order, when Lichess doesn't hold some (or
    /// has them once, where this has one twice).
    pub nags: Option<Vec<Nag>>,
    /// All its arrows and highlights, in their order, when that isn't
    /// highlights first, as our PGN writer puts them.
    pub shapes: Option<Vec<Shape>>,
    /// All its `[%…]` commands, in their order, when Lichess doesn't hold
    /// some (evals, `%emt`, any but clocks) or holds a clock otherwise (to
    /// the second, as `h:mm:ss`).
    pub commands: Option<Vec<(String, String)>>,
}

impl NodeExtras {
    pub fn is_empty(&self) -> bool {
        *self == NodeExtras::default()
    }

    /// Only what Lichess can't hold at all: the comment before, NAGs and
    /// commands it doesn't have. Not how this writes what it does hold.
    pub fn local_only(&self, root: bool) -> NodeExtras {
        NodeExtras {
            comment_before: self.comment_before.clone(),
            comment_after: None,
            nags: self.nags.as_ref().map(|n| n.iter().copied().filter(|&g| !held_nag(g, root)).collect()),
            shapes: None,
            commands: self
                .commands
                .as_ref()
                .map(|c| c.iter().filter(|c| !held_command(c)).cloned().collect()),
        }
        .without_empty()
    }

    fn without_empty(mut self) -> NodeExtras {
        self.nags = self.nags.filter(|n| !n.is_empty());
        self.commands = self.commands.filter(|c| !c.is_empty());
        self
    }
}

/// The tags a game has as here: ECO and Annotator (empty: none), and the
/// others with their values, in their order (those Lichess holds too, for
/// their order and how they're written).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagExtras {
    pub eco: String,
    pub annotator: String,
    pub extra: Vec<(String, String)>,
}

/// What only this database holds of a linked game ([`local_only_layer`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layer {
    /// The start position (FEN) when it isn't the usual one: the paths
    /// start there.
    pub setup: Option<String>,
    pub tags: TagExtras,
    /// By path, for the nodes that have any, in pre-order (a move before
    /// the moves after it, its main continuation's line before the
    /// variations), so each path shares its first moves with the one
    /// before.
    pub nodes: Vec<(MovePath, NodeExtras)>,
    /// The main line's last move has a comment like Lichess's result
    /// comment ([`is_lichess_addition`]) here: it's this game's own, kept.
    pub own_result_comment: bool,
}

/// Why a layer's entry has no node in the pulled game to go to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrphanReason {
    /// Its move, or one before it, isn't in the game now.
    MoveGone,
    /// The game starts from another position now: no path means the same.
    StartChanged,
    /// The move has a comment before it there now (which Lichess doesn't
    /// keep: it came some other way), so this one wasn't put in its place.
    CommentBeforeTaken,
}

/// What only this database held at a place the pulled game doesn't have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Orphan {
    pub path: MovePath,
    /// The path in SAN, numbered from the layer's start ("1.e4 e5 2.Nf3",
    /// "41...Kf6"); empty for the start.
    pub line: String,
    /// What Lichess can't hold of it ([`NodeExtras::local_only`]).
    pub extras: NodeExtras,
    pub reason: OrphanReason,
}

/// [`apply_layer`]'s game, and what couldn't go back into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub game: Game,
    pub orphaned: Vec<Orphan>,
}

// ---- What Lichess holds ------------------------------------------------------

/// A comment's text as our PGN writer sends it and our reader reads it
/// back: `}` as `)`, every run of spaces and line breaks one space, none
/// at the ends.
fn lichess_text(text: &str) -> String {
    text.replace('}', ")").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn held_nag(n: Nag, root: bool) -> bool {
    // Our writer puts no NAG before the first move.
    !root && LICHESS_NAGS.contains(&n.0)
}

fn held_tag(name: &str) -> bool {
    LICHESS_TAGS.contains(&name)
}

/// A clock Lichess reads (`h:mm:ss`, the seconds with a fraction or not), in
/// milliseconds.
fn clock_ms(value: &str) -> Option<u64> {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let [h, m, s] = <[&str; 3]>::try_from(value.trim().split(':').collect::<Vec<_>>()).ok()?;
    let (s, frac) = match s.split_once('.') {
        Some((s, frac)) if digits(frac) => (s, frac),
        Some(_) => return None,
        None => (s, ""),
    };
    if !digits(h) || !digits(m) || !digits(s) {
        return None;
    }
    let (h, m, s): (u64, u64, u64) = (h.parse().ok()?, m.parse().ok()?, s.parse().ok()?);
    if m >= 60 || s >= 60 {
        return None;
    }
    let ms: u64 = format!("{frac:0<3}")[..3].parse().ok()?;
    h.checked_mul(3600)?.checked_add(m * 60 + s)?.checked_mul(1000)?.checked_add(ms)
}

/// A clock as Lichess writes it: to the nearest second (half up:
/// `0:04:57.5` came back `0:04:58`), `h:mm:ss`.
fn render_clock(ms: u64) -> String {
    let s = ms.saturating_add(500) / 1000;
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

fn held_command((name, value): &(String, String)) -> bool {
    name == CLOCK && clock_ms(value).is_some()
}

/// Highlights first, then arrows, each in its order: as our PGN writer
/// writes them (`[%csl …][%cal …]`) and our reader reads them back.
fn written_order(shapes: &[Shape]) -> Vec<Shape> {
    let squares = shapes.iter().filter(|s| s.to.is_none());
    squares.chain(shapes.iter().filter(|s| s.to.is_some())).copied().collect()
}

/// Cuts a node down to what Lichess holds of it.
fn project_node(n: &mut Node, root: bool) {
    n.comment_before.clear();
    n.comment_after = lichess_text(&n.comment_after);
    let mut seen = Vec::new();
    n.nags.retain(|&g| {
        held_nag(g, root) && !seen.contains(&g) && {
            seen.push(g);
            true
        }
    });
    n.shapes = written_order(&n.shapes);
    n.commands = n
        .commands
        .iter()
        .filter(|c| held_command(c))
        .map(|(_, v)| (CLOCK.to_owned(), render_clock(clock_ms(v).expect("held"))))
        .collect();
}

fn project_headers(h: &mut Headers) {
    h.eco.clear();
    h.annotator.clear();
    let extra = std::mem::take(&mut h.extra);
    for name in LICHESS_TAGS {
        if let Some((_, v)) = extra.iter().find(|(k, _)| k == name) {
            h.extra.push((name.to_owned(), v.clone()));
        }
    }
}

/// The game cut down to what Lichess holds of it, as our PGN writer sends it
/// and our reader reads it back (`docs/net-13-sync.md` has the rules):
///
/// - Moves, variations and their order, and the start position, as they
///   are.
/// - Comments after a move (and the game's first, before its first move),
///   their spacing as our PGN path makes it: `}` as `)`, every run of
///   spaces and line breaks one space. No comment before a move (a
///   variation's first): Lichess drops them.
/// - NAGs on [`LICHESS_NAGS`], once each, in their order; none at the start.
/// - Arrows and highlights, all four colours, highlights first.
/// - Clocks (`%clk`) to the nearest second, as `h:mm:ss`; no other `[%…]`
///   command (evals, `%emt`, …), and no clock Lichess can't read.
/// - The roster, ratings and result, and the tags on [`LICHESS_TAGS`] in
///   its order; no ECO, Opening, Annotator, EventDate or study tag
///   (StudyName, ChapterName, ChapterURL), nor any other. A game without
///   an `Orientation` has none (Lichess will say `white`).
///
/// Projecting twice is projecting once.
pub fn lichess_projection(game: &Game) -> Game {
    let mut g = game.clone();
    project_headers(&mut g.headers);
    for id in g.preorder() {
        project_node(g.node_mut(id), id == NodeId::ROOT);
    }
    g
}

// ---- The local-only layer -----------------------------------------------------

/// What [`lichess_projection`] takes away from a game, by move path: what to
/// keep here, for [`apply_layer`] to put back on the game as Lichess has it.
/// A node's entry keeps what it had in full (its NAGs, commands…) where the
/// projection changed it, so that putting it back is exact.
pub fn local_only_layer(game: &Game) -> Layer {
    let mut nodes = Vec::new();
    let mut stack = vec![(NodeId::ROOT, Vec::new())];
    while let Some((id, path)) = stack.pop() {
        let n = game.node(id);
        let mut p = n.clone();
        project_node(&mut p, id == NodeId::ROOT);
        let x = NodeExtras {
            comment_before: n.comment_before.clone(),
            comment_after: (p.comment_after != n.comment_after).then(|| n.comment_after.clone()),
            nags: (p.nags != n.nags).then(|| n.nags.clone()),
            shapes: (p.shapes != n.shapes).then(|| n.shapes.clone()),
            commands: (p.commands != n.commands).then(|| n.commands.clone()),
        };
        for &child in n.children.iter().rev() {
            let mut next = path.clone();
            next.push(game.node(child).mv.expect("non-root node has a move"));
            stack.push((child, next));
        }
        if !x.is_empty() {
            nodes.push((MovePath(path), x));
        }
    }
    let h = &game.headers;
    Layer {
        setup: game.setup.as_ref().map(to_fen),
        tags: TagExtras { eco: h.eco.clone(), annotator: h.annotator.clone(), extra: h.extra.clone() },
        nodes,
        own_result_comment: is_lichess_addition(&game.node(game.line_end(NodeId::ROOT)).comment_after),
    }
}

/// Whether a comment is the one Lichess puts on the main line's last move
/// of a finished game that has none there (`{ 1-0 White wins. }`; see
/// [`RESULT_COMMENTS`]), whatever its spacing. It isn't an edit made on
/// Lichess; [`apply_layer`] and [`normalize_for_compare`] leave it out
/// there.
pub fn is_lichess_addition(comment: &str) -> bool {
    let text = lichess_text(comment);
    RESULT_COMMENTS.contains(&text.as_str())
}

/// As sets: the same things, whatever their order or how often.
fn same_set<T, K: Ord>(a: &[T], b: &[T], key: impl Fn(&T) -> K) -> bool {
    let sorted = |v: &[T]| {
        let mut k: Vec<K> = v.iter().map(&key).collect();
        k.sort_unstable();
        k.dedup();
        k
    };
    sorted(a) == sorted(b)
}

fn shape_key(s: &Shape) -> (bool, char, u8, u8) {
    (s.to.is_some(), s.color.letter(), s.from as u8, s.to.map_or(64, |t| t as u8))
}

/// Whether the clocks of a pulled node are those of `ours` as Lichess holds
/// them: as many, each within a second (Lichess rounds them to it).
fn clocks_match(theirs: &[(String, String)], ours: &[(String, String)]) -> bool {
    let theirs: Vec<Option<u64>> =
        theirs.iter().filter(|(k, _)| k == CLOCK).map(|(_, v)| clock_ms(v)).collect();
    let ours: Vec<u64> = ours.iter().filter(|c| held_command(c)).filter_map(|(_, v)| clock_ms(v)).collect();
    theirs.len() == ours.len()
        && theirs.iter().zip(&ours).all(|(t, &o)| t.is_some_and(|t| t.abs_diff(o) < 1000))
}

/// Puts a layer's entry back on a pulled node. Where what Lichess holds of
/// the node is as this had it, the node gets this one's exactly; where it
/// changed there, Lichess's stays and only what it can't hold is added.
/// Returns this one's comment before the move if it couldn't go back.
fn apply_node(n: &mut Node, x: &NodeExtras, root: bool) -> Option<NodeExtras> {
    let mut orphan = None;
    if !x.comment_before.is_empty() {
        if n.comment_before.is_empty() || lichess_text(&n.comment_before) == lichess_text(&x.comment_before) {
            n.comment_before = x.comment_before.clone();
        } else {
            orphan = Some(NodeExtras { comment_before: x.comment_before.clone(), ..NodeExtras::default() });
        }
    }
    if let Some(ours) = &x.comment_after
        && lichess_text(&n.comment_after) == lichess_text(ours)
    {
        n.comment_after = ours.clone();
    }
    if let Some(ours) = &x.nags {
        let held: Vec<Nag> = ours.iter().copied().filter(|&g| held_nag(g, root)).collect();
        if same_set(&n.nags, &held, |g| g.0) {
            n.nags = ours.clone();
        } else {
            for &g in ours.iter().filter(|&&g| !held_nag(g, root)) {
                if !n.nags.contains(&g) {
                    n.nags.push(g);
                }
            }
        }
    }
    if let Some(ours) = &x.shapes
        && same_set(&n.shapes, ours, shape_key)
    {
        n.shapes = ours.clone();
    }
    if let Some(ours) = &x.commands {
        if clocks_match(&n.commands, ours) {
            n.commands = ours.clone();
        } else {
            for c in ours.iter().filter(|c| !held_command(c)) {
                if !n.commands.contains(c) {
                    n.commands.push(c.clone());
                }
            }
        }
    }
    orphan
}

/// The tags as here: ECO, Annotator and the tags Lichess doesn't hold from
/// the layer (Lichess's own ECO, Opening, Annotator and study tags left
/// out); those it holds from the pulled game, written as here where they
/// mean the same, in this game's order, then those added there (but the
/// `white` Lichess gives a game without an `Orientation`).
fn apply_tags(h: &mut Headers, tags: &TagExtras) {
    h.eco = tags.eco.clone();
    h.annotator = tags.annotator.clone();
    let theirs = std::mem::take(&mut h.extra);
    let same = |name: &str, a: &str, b: &str| {
        if name == ORIENTATION { a.trim().eq_ignore_ascii_case(b.trim()) } else { a.trim() == b.trim() }
    };
    for (name, ours) in &tags.extra {
        if !held_tag(name) {
            h.extra.push((name.clone(), ours.clone()));
            continue;
        }
        match theirs.iter().find(|(k, _)| k == name) {
            Some((_, v)) if same(name, v, ours) => h.extra.push((name.clone(), ours.clone())),
            Some((_, v)) => h.extra.push((name.clone(), v.clone())),
            // Gone there.
            None => {}
        }
    }
    for (name, v) in &theirs {
        let ours = tags.extra.iter().any(|(k, _)| k == name);
        let default = name == ORIENTATION && v.trim().eq_ignore_ascii_case("white");
        if held_tag(name) && !ours && !default {
            h.extra.push((name.clone(), v.clone()));
        }
    }
}

fn start_of(setup: Option<&str>) -> Option<Chess> {
    match setup {
        Some(fen) => from_fen(fen).ok(),
        None => Some(Chess::default()),
    }
}

/// A path in SAN from `start`, numbered: "1.e4 e5 2.Nf3", "41...Kf6". Stops
/// at a move that isn't legal (none is, in a layer read back).
fn line_of(start: Option<Chess>, path: &MovePath) -> String {
    let Some(mut pos) = start else { return String::new() };
    let mut out = Vec::new();
    for (i, &mv) in path.0.iter().enumerate() {
        if !pos.is_legal(mv) {
            break;
        }
        let n = pos.fullmoves().get();
        let text = san(&pos, mv);
        out.push(match (pos.turn().is_white(), i) {
            (true, _) => format!("{n}.{text}"),
            (false, 0) => format!("{n}...{text}"),
            (false, _) => text,
        });
        pos.play_unchecked(mv);
    }
    out.join(" ")
}

/// Puts a layer back on a game pulled from Lichess (its chapter as read with
/// our PGN reader): each entry on the node its path leads to, if that move
/// is there; see [`apply_node`] for how one goes back. Lichess's own
/// additions are left out: its result comment on the last move (unless this
/// game had one there itself), its ECO, Opening and Annotator, its study
/// tags and the `white` it gives a game without an orientation.
///
/// Entries whose move isn't in the pulled game (removed on Lichess), or all
/// of them if it starts elsewhere now, come back in [`Applied::orphaned`],
/// with what Lichess can't hold of them; the caller decides what to do with
/// them. With the game's own projection, `apply_layer(&lichess_projection(g),
/// &local_only_layer(g))` is `g` exactly.
pub fn apply_layer(pulled: &Game, layer: &Layer) -> Applied {
    let mut g = pulled.clone();
    let mut orphaned = Vec::new();
    let last = g.line_end(NodeId::ROOT);
    if !layer.own_result_comment && is_lichess_addition(&g.node(last).comment_after) {
        g.node_mut(last).comment_after.clear();
    }
    let start = start_of(layer.setup.as_deref());
    let mut orphan = |path: &MovePath, extras: NodeExtras, reason| {
        let extras = extras.local_only(path.0.is_empty());
        if !extras.is_empty() {
            orphaned.push(Orphan { path: path.clone(), line: line_of(start.clone(), path), extras, reason });
        }
    };
    if g.setup.as_ref().map(to_fen) != layer.setup {
        for (path, x) in &layer.nodes {
            orphan(path, x.clone(), OrphanReason::StartChanged);
        }
    } else {
        let mut todo: HashMap<&[Move], &NodeExtras> =
            layer.nodes.iter().map(|(p, x)| (p.0.as_slice(), x)).collect();
        let mut stack = vec![(NodeId::ROOT, Vec::new())];
        while let Some((id, path)) = stack.pop() {
            if let Some(x) = todo.remove(path.as_slice())
                && let Some(left) = apply_node(g.node_mut(id), x, id == NodeId::ROOT)
            {
                orphan(&MovePath(path.clone()), left, OrphanReason::CommentBeforeTaken);
            }
            for &child in g.node(id).children.iter().rev() {
                let mut next = path.clone();
                next.push(g.node(child).mv.expect("non-root node has a move"));
                stack.push((child, next));
            }
        }
        for (path, x) in &layer.nodes {
            if todo.contains_key(path.0.as_slice()) {
                orphan(path, x.clone(), OrphanReason::MoveGone);
            }
        }
    }
    apply_tags(&mut g.headers, &layer.tags);
    Applied { game: g, orphaned }
}

/// A game as the later three-way merge compares it: its projection
/// ([`lichess_projection`]), without what Lichess adds or rewrites by
/// itself, whatever the order it writes things in. On top of the
/// projection (which leaves out ECO, Opening, Annotator, the study tags and
/// any other tag Lichess doesn't hold, and the tags' order): Lichess's result
/// comment on the main line's last move is left out; a game without an
/// `Orientation` has Lichess's `white`, and it's in lower case; each node's
/// NAGs, and its arrows and highlights, are sorted, each once.
///
/// So a game sent as its projection compares equal to Lichess's export of
/// it, and two exports of an unchanged chapter compare equal. Not covered:
/// a clock Lichess makes from an `%emt` (only when a game is sent whole, with
/// its `%emt`; a projection has none), and how Lichess orders two NAGs of
/// one kind (not tried).
pub fn normalize_for_compare(game: &Game) -> Game {
    let mut g = lichess_projection(game);
    let last = g.line_end(NodeId::ROOT);
    if is_lichess_addition(&g.node(last).comment_after) {
        g.node_mut(last).comment_after.clear();
    }
    let orientation = g
        .headers
        .get_extra(ORIENTATION)
        .map_or_else(|| "white".to_owned(), |o| o.trim().to_ascii_lowercase());
    let extra = std::mem::take(&mut g.headers.extra);
    for name in LICHESS_TAGS {
        if name == ORIENTATION {
            g.headers.extra.push((name.to_owned(), orientation.clone()));
        } else if let Some(tag) = extra.iter().find(|(k, _)| k == name) {
            g.headers.extra.push(tag.clone());
        }
    }
    for id in g.preorder() {
        let n = g.node_mut(id);
        n.nags.sort_unstable();
        n.shapes.sort_by_key(shape_key);
        n.shapes.dedup();
    }
    g
}

// ---- Stored ---------------------------------------------------------------------

/// The layer's own version, first in its bytes.
const LAYER_VERSION: u8 = 1;
const HAS_SETUP: u8 = 1;
const OWN_RESULT_COMMENT: u8 = 2;

const F_BEFORE: u8 = 1;
const F_AFTER: u8 = 2;
const F_NAGS: u8 = 4;
const F_SHAPES: u8 = 8;
const F_COMMANDS: u8 = 16;

const NO_SQUARE: u8 = 0xFF;

fn bad(what: &str) -> NetError {
    NetError::Db(DbError::Corrupt(format!("a linked game's local-only layer: {what}")))
}

/// Positions along the last path written or read, so that paths sharing
/// their first moves (all of a line's, in a layer) are played once.
struct Walk {
    /// `positions[i]`: before the path's move `i`.
    positions: Vec<Chess>,
    /// The last path, as move indices.
    indices: Vec<u8>,
}

impl Walk {
    fn new(start: Chess) -> Walk {
        Walk { positions: vec![start], indices: Vec::new() }
    }

    /// Keeps the last path's first `n` moves.
    fn truncate(&mut self, n: usize) {
        self.indices.truncate(n);
        self.positions.truncate(n + 1);
    }

    /// The position after the moves kept.
    fn here(&self) -> &Chess {
        self.positions.last().expect("the start at least")
    }

    fn push(&mut self, index: u8, mv: Move) {
        let mut next = self.here().clone();
        next.play_unchecked(mv);
        self.positions.push(next);
        self.indices.push(index);
    }
}

impl Layer {
    /// The layer as stored with the game's link (`chess_db::StudyLink`'s
    /// `layer`), in the binary form of the database's own files (little
    /// endian, LEB128 varints, strings as their length then UTF-8; moves as
    /// a game blob names them). `docs/net-13-sync.md` › Stored has it. The
    /// paths must be legal from the start (those [`local_only_layer`] makes
    /// are).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = vec![LAYER_VERSION];
        let mut flags = 0;
        if self.setup.is_some() {
            flags |= HAS_SETUP;
        }
        if self.own_result_comment {
            flags |= OWN_RESULT_COMMENT;
        }
        b.push(flags);
        if let Some(fen) = &self.setup {
            put_str(&mut b, fen);
        }
        put_str(&mut b, &self.tags.eco);
        put_str(&mut b, &self.tags.annotator);
        put_varint(&mut b, self.tags.extra.len() as u64);
        for (name, value) in &self.tags.extra {
            put_str(&mut b, name);
            put_str(&mut b, value);
        }
        put_varint(&mut b, self.nodes.len() as u64);
        let mut walk = Walk::new(start_of(self.setup.as_deref()).unwrap_or_default());
        let mut last: &[Move] = &[];
        for (path, x) in &self.nodes {
            walk.truncate(last.iter().zip(&path.0).take_while(|(a, b)| a == b).count());
            for &mv in &path.0[walk.indices.len()..] {
                debug_assert!(walk.here().is_legal(mv), "a layer's path isn't legal");
                walk.push(move_index(walk.here(), mv), mv);
            }
            last = &path.0;
            put_bytes(&mut b, &walk.indices);
            let mut fields = 0;
            for (on, bit) in [
                (!x.comment_before.is_empty(), F_BEFORE),
                (x.comment_after.is_some(), F_AFTER),
                (x.nags.is_some(), F_NAGS),
                (x.shapes.is_some(), F_SHAPES),
                (x.commands.is_some(), F_COMMANDS),
            ] {
                if on {
                    fields |= bit;
                }
            }
            b.push(fields);
            if !x.comment_before.is_empty() {
                put_str(&mut b, &x.comment_before);
            }
            if let Some(c) = &x.comment_after {
                put_str(&mut b, c);
            }
            if let Some(nags) = &x.nags {
                put_bytes(&mut b, &nags.iter().map(|n| n.0).collect::<Vec<_>>());
            }
            if let Some(shapes) = &x.shapes {
                put_varint(&mut b, shapes.len() as u64);
                for s in shapes {
                    b.push(s.color.letter() as u8);
                    b.push(s.from as u8);
                    b.push(s.to.map_or(NO_SQUARE, |t| t as u8));
                }
            }
            if let Some(commands) = &x.commands {
                put_varint(&mut b, commands.len() as u64);
                for (name, value) in commands {
                    put_str(&mut b, name);
                    put_str(&mut b, value);
                }
            }
        }
        b
    }

    /// Reads back what [`to_bytes`](Self::to_bytes) wrote. A layer of a
    /// newer version is refused as such; one that can't be read is
    /// `Corrupt`.
    pub fn from_bytes(bytes: &[u8]) -> Result<Layer, NetError> {
        let mut r = ByteReader::new(bytes);
        let version = r.u8()?;
        if version != LAYER_VERSION {
            return Err(NetError::Db(DbError::UnsupportedVersion(u32::from(version))));
        }
        let flags = r.u8()?;
        if flags & !(HAS_SETUP | OWN_RESULT_COMMENT) != 0 {
            return Err(bad("unknown flags"));
        }
        let setup = if flags & HAS_SETUP != 0 { Some(r.str()?) } else { None };
        let start = start_of(setup.as_deref()).ok_or_else(|| bad("its start position can't be read"))?;
        let eco = r.str()?;
        let annotator = r.str()?;
        let n = r.varint()?;
        let mut extra = Vec::new();
        for _ in 0..n {
            extra.push((r.str()?, r.str()?));
        }
        let n = r.varint()?;
        let mut nodes = Vec::new();
        let mut seen = HashSet::new();
        let mut walk = Walk::new(start);
        let mut moves: Vec<Move> = Vec::new();
        for _ in 0..n {
            let indices = r.len_bytes()?;
            walk.truncate(walk.indices.iter().zip(indices).take_while(|(a, b)| a == b).count());
            moves.truncate(walk.indices.len());
            for &i in &indices[walk.indices.len()..] {
                let mv = move_at(walk.here(), i).ok_or_else(|| bad("a path isn't legal"))?;
                walk.push(i, mv);
                moves.push(mv);
            }
            if !seen.insert(indices.to_vec()) {
                return Err(bad("a path twice"));
            }
            let fields = r.u8()?;
            if fields & !(F_BEFORE | F_AFTER | F_NAGS | F_SHAPES | F_COMMANDS) != 0 {
                return Err(bad("unknown fields"));
            }
            let mut x = NodeExtras::default();
            if fields & F_BEFORE != 0 {
                x.comment_before = r.str()?;
            }
            if fields & F_AFTER != 0 {
                x.comment_after = Some(r.str()?);
            }
            if fields & F_NAGS != 0 {
                x.nags = Some(r.len_bytes()?.iter().map(|&n| Nag(n)).collect());
            }
            if fields & F_SHAPES != 0 {
                let n = r.varint()?;
                let mut shapes = Vec::new();
                for _ in 0..n {
                    let color = ShapeColor::from_letter(char::from(r.u8()?))
                        .ok_or_else(|| bad("a shape's colour"))?;
                    let square = |b: u8| (b < 64).then(|| Square::new(u32::from(b)));
                    let from = square(r.u8()?).ok_or_else(|| bad("a shape's square"))?;
                    let to = match r.u8()? {
                        NO_SQUARE => None,
                        b => Some(square(b).ok_or_else(|| bad("a shape's square"))?),
                    };
                    shapes.push(Shape { color, from, to });
                }
                x.shapes = Some(shapes);
            }
            if fields & F_COMMANDS != 0 {
                let n = r.varint()?;
                let mut commands = Vec::new();
                for _ in 0..n {
                    commands.push((r.str()?, r.str()?));
                }
                x.commands = Some(commands);
            }
            nodes.push((MovePath(moves.clone()), x));
        }
        if !r.is_empty() {
            return Err(bad("bytes after its end"));
        }
        let own_result_comment = flags & OWN_RESULT_COMMENT != 0;
        Ok(Layer { setup, tags: TagExtras { eco, annotator, extra }, nodes, own_result_comment })
    }
}

#[cfg(test)]
mod tests {
    use chess_core::game::GameResult;
    use chess_core::pgn::{read_pgn_str, write_game};
    use proptest::prelude::*;

    use super::*;

    fn game(pgn: &str) -> Game {
        read_pgn_str(pgn).into_iter().next().expect("a game")
    }

    /// The real round-trip check's games (`testdata/pgn/study-roundtrip.pgn`).
    fn fixture() -> Vec<Game> {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn/study-roundtrip.pgn");
        read_pgn_str(&std::fs::read_to_string(path).unwrap())
    }

    fn node_at(g: &Game, sans: &[&str]) -> NodeId {
        let mut id = NodeId::ROOT;
        for s in sans {
            let pos = g.position_at(id);
            id = *g
                .node(id)
                .children
                .iter()
                .find(|&&c| san(&pos, g.node(c).mv.unwrap()) == *s)
                .unwrap_or_else(|| panic!("no {s}"));
        }
        id
    }

    fn commands(g: &Game, id: NodeId) -> Vec<(String, String)> {
        g.node(id).commands.clone()
    }

    fn cmd(name: &str, value: &str) -> (String, String) {
        (name.to_owned(), value.to_owned())
    }

    /// What Lichess made of a game sent to a study, as the round-trip check
    /// saw it on lichess.org (2026-11-12), read back with our PGN reader:
    /// evals, `%emt`, comments before a variation's first move and NAGs
    /// not on its list dropped; clocks to the second; with `whole` (the game
    /// sent as it is, not projected) a clock made from an `%emt` where a move
    /// had no clock (the player's clock before, less the time, plus the
    /// increment); its own ECO and Opening; its Annotator where there was
    /// none; `white` where there was no Orientation; the study's tags; tags
    /// in its own order and the others dropped; its result comment on the
    /// last move of a finished game that had no comment there.
    fn lichess_like(sent: &Game, whole: bool) -> Game {
        let mut g = sent.clone();
        let increment = g
            .headers
            .get_extra("TimeControl")
            .and_then(|t| t.split_once('+'))
            .and_then(|(_, i)| i.parse::<u64>().ok());
        for id in g.preorder() {
            let emt = g.node(id).commands.iter().find(|(k, _)| k == "emt").and_then(|(_, v)| clock_ms(v));
            let had_clock = g.node(id).commands.iter().any(|(k, _)| k == "clk");
            let before = g.node(id).parent.and_then(|p| g.node(p).parent).and_then(|p| {
                g.node(p).commands.iter().find(|(k, _)| k == "clk").and_then(|(_, v)| clock_ms(v))
            });
            let n = g.node_mut(id);
            if whole
                && !had_clock
                && let (Some(emt), Some(inc), Some(before)) = (emt, increment, before)
            {
                n.commands.push(cmd("clk", &render_clock(before - emt + inc * 1000)));
            }
            n.comment_before.clear();
            n.nags.retain(|&g| held_nag(g, id == NodeId::ROOT));
            n.commands = n
                .commands
                .iter()
                .filter(|c| held_command(c))
                .map(|(_, v)| cmd("clk", &render_clock(clock_ms(v).unwrap())))
                .collect();
        }
        let last = g.line_end(NodeId::ROOT);
        let result = match g.headers.result {
            GameResult::WhiteWins => Some(RESULT_COMMENTS[0]),
            GameResult::BlackWins => Some(RESULT_COMMENTS[1]),
            GameResult::Draw => Some(RESULT_COMMENTS[2]),
            GameResult::Unknown => None,
        };
        if let Some(text) = result
            && g.node(last).comment_after.is_empty()
        {
            g.node_mut(last).comment_after = text.to_owned();
        }
        let from_position = g.setup.is_some();
        let h = &mut g.headers;
        h.eco = if from_position { String::new() } else { "A00".into() };
        if h.annotator.is_empty() {
            h.annotator = "https://lichess.org/@/owner".into();
        }
        let mut kept: Vec<(String, String)> = h.extra.iter().filter(|(k, _)| held_tag(k)).cloned().collect();
        kept.reverse();
        match kept.iter_mut().find(|(k, _)| k == ORIENTATION) {
            Some((_, v)) => *v = v.to_ascii_lowercase(),
            None => kept.push(cmd(ORIENTATION, "white")),
        }
        let opening = if from_position { "?" } else { "Lichess's own" };
        h.extra = [
            cmd("StudyName", "Round-trip check"),
            cmd("ChapterName", "A chapter"),
            cmd("ChapterURL", "https://lichess.org/study/abcdEFGH/ijklMNOP"),
            cmd("Opening", opening),
        ]
        .into_iter()
        .chain(kept)
        .collect();
        read_pgn_str(&write_game(&g)).remove(0)
    }

    // ---- The rules, from what lichess.org did -----------------------------------

    #[test]
    fn the_projection_holds_what_lichess_kept() {
        let games = fixture();
        let g = &games[0];
        let p = lichess_projection(g);
        // Moves, variations, their order, the start: as they are.
        assert_eq!(p.preorder().len(), g.preorder().len());
        assert_eq!(p.setup, g.setup);
        // Tags: no ECO or Annotator; then those it keeps, in the list's order.
        assert_eq!((p.headers.eco.as_str(), p.headers.annotator.as_str()), ("", ""));
        assert_eq!(p.headers.extra, [cmd("TimeControl", "300+3"), cmd(ORIENTATION, "white")]);
        assert_eq!((p.headers.white.as_str(), p.headers.result), ("Student", GameResult::Unknown));
        // The comment before 3...Be7 (a variation's first move) goes.
        let line = ["e4", "e5", "Nf3", "Nc6", "Bc4", "Be7"];
        assert_eq!(g.node(node_at(g, &line)).comment_before, "The Hungarian, quiet:");
        assert_eq!(p.node(node_at(&p, &line)).comment_before, "");
        // Evals go; a clock to the tenth is rounded; %emt goes.
        assert_eq!(commands(&p, node_at(&p, &["e4"])), [cmd("clk", "0:05:00")]);
        assert_eq!(commands(&p, node_at(&p, &["e4", "e5", "Nf3", "Nc6"])), [cmd("clk", "0:04:58")]);
        let bc4 = node_at(&p, &["e4", "e5", "Nf3", "Nc6", "Bc4"]);
        assert_eq!(commands(&p, bc4), [cmd("clk", "0:04:55")]);
        assert_eq!(p.node(bc4).shapes.len(), 4, "arrows and highlights stay");
        let ng5 = node_at(&p, &["e4", "e5", "Nf3", "Nc6", "Bc4", "Nf6", "Ng5"]);
        assert_eq!((commands(&p, ng5), p.node(ng5).nags.clone()), (vec![], vec![Nag(6), Nag(146)]));
        // $142 goes, $132 and $36 stay.
        let main: Vec<&str> =
            "e4 e5 Nf3 Nc6 Bc4 Nf6 Ng5 d5 exd5 Na5 Bb5+ c6 dxc6 bxc6 Be2 h6 Nf3 e4 Ne5 Bd6 d4"
                .split(' ')
                .collect();
        let d4 = node_at(&p, &main);
        assert_eq!(g.node(node_at(g, &main)).nags, [Nag(142)]);
        assert_eq!(p.node(d4).nags, []);
        assert_eq!(p.node(p.node(d4).children[0]).nags, [Nag(132), Nag(36)]);
        // The game's first comment stays, with its highlights.
        assert_eq!(p.root().comment_after, "An opening lesson with every kind of annotation.");
        assert_eq!(p.root().shapes.len(), 2);

        // Game 3: ECO, Opening, EventDate and Annotator go; titles,
        // TimeControl and Termination stay, in the list's order.
        let p = lichess_projection(&games[2]);
        assert_eq!(
            p.headers.extra,
            [
                cmd("WhiteTitle", "FM"),
                cmd("BlackTitle", "WIM"),
                cmd("TimeControl", "5400+30"),
                cmd("Termination", "Time forfeit")
            ]
        );
        assert_eq!((p.headers.white_elo, p.headers.eco.as_str()), (Some(2234), ""));
        assert!(p.preorder().iter().all(|&id| p.node(id).commands.iter().all(|(k, _)| k == "clk")));
    }

    #[test]
    fn the_nags_lichess_keeps() {
        let all: String = (1..=255).map(|n| format!(" ${n}")).collect();
        let g = game(&format!("1. e4{all} *"));
        let p = lichess_projection(&g);
        let kept: Vec<u8> = p.node(node_at(&p, &["e4"])).nags.iter().map(|n| n.0).collect();
        assert_eq!(kept, LICHESS_NAGS);
        // Every NAG the fixture has but $142 is on the list.
        let mut seen = HashSet::new();
        for g in fixture() {
            for id in g.preorder() {
                seen.extend(g.node(id).nags.iter().map(|n| n.0));
            }
        }
        let mut not_held: Vec<u8> = seen.into_iter().filter(|n| !LICHESS_NAGS.contains(n)).collect();
        not_held.sort_unstable();
        assert_eq!(not_held, [142]);
    }

    #[test]
    fn clocks_as_lichess_holds_them() {
        for (text, ms, lichess) in [
            ("0:04:57.5", 297_500, "0:04:58"),
            ("0:04:57.49", 297_490, "0:04:57"),
            ("0:05:00", 300_000, "0:05:00"),
            ("1:30:00", 5_400_000, "1:30:00"),
            ("0:4:5", 245_000, "0:04:05"),
            ("12:00:00.999", 43_200_999, "12:00:01"),
            (" 0:00:00 ", 0, "0:00:00"),
        ] {
            assert_eq!(clock_ms(text), Some(ms), "{text}");
            assert_eq!(render_clock(ms), lichess, "{text}");
        }
        for bad in ["", "4:57", "0:60:00", "0:00:60", "abc", "0:04:5x", "0:04:57.", "-0:00:01", "0:04:57.5.1"]
        {
            assert_eq!(clock_ms(bad), None, "{bad:?}");
        }
        // A clock Lichess can't read is held here only.
        let g = game("1. e4 { [%clk 4:57] [%clk 0:01:00] } *");
        assert_eq!(commands(&lichess_projection(&g), node_at(&g, &["e4"])), [cmd("clk", "0:01:00")]);
    }

    #[test]
    fn lichess_s_result_comment() {
        for text in RESULT_COMMENTS {
            assert!(is_lichess_addition(text));
            assert!(is_lichess_addition(&format!("  {}\n", text.replace(' ', "  "))));
        }
        for text in
            ["", "1-0", "White wins.", "1-0 White wins!", "1-0 White wins. Well played", "0-1 White wins"]
        {
            assert!(!is_lichess_addition(text), "{text}");
        }
        // Left out on the last move, where Lichess puts it; not elsewhere.
        let ours = game("[Result \"1-0\"]\n\n1. e4 { 1-0 White wins. } e5 2. Nf3 1-0");
        let pulled = game("[Result \"1-0\"]\n\n1. e4 { 1-0 White wins. } e5 2. Nf3 { 1-0 White wins. } 1-0");
        let back = apply_layer(&pulled, &local_only_layer(&ours)).game;
        assert_eq!(back, ours);
        assert_eq!(normalize_for_compare(&pulled), normalize_for_compare(&ours));
        // This game's own, on its last move, is kept.
        let own = game("[Result \"1-0\"]\n\n1. e4 e5 2. Nf3 { 1-0 White wins. } 1-0");
        assert!(local_only_layer(&own).own_result_comment);
        assert_eq!(apply_layer(&lichess_projection(&own), &local_only_layer(&own)).game, own);
    }

    #[test]
    fn tags_as_here_whatever_lichess_puts() {
        let ours = game(
            "[Event \"E\"]\n[ECO \"C58\"]\n[Annotator \"Coach\"]\n[EventDate \"2025.03.12\"]\n[TimeControl \"300+3\"]\n\
             [PlyCount \"2\"]\n[Opening \"Ours\"]\n\n1. e4 e5 *",
        );
        let layer = local_only_layer(&ours);
        let mut pulled = lichess_like(&lichess_projection(&ours), false);
        assert_eq!(pulled.headers.eco, "A00");
        assert_eq!(pulled.headers.get_extra(ORIENTATION), Some("white"));
        assert_eq!(
            apply_layer(&pulled, &layer).game,
            ours,
            "Lichess's ECO, Opening, Annotator, Orientation go"
        );
        assert_eq!(normalize_for_compare(&pulled), normalize_for_compare(&ours));
        // What changed there comes: an Orientation, a tag added; a tag it
        // holds removed there (TimeControl) goes.
        pulled.headers.extra =
            vec![cmd(ORIENTATION, "black"), cmd("WhiteTitle", "GM"), cmd("StudyName", "S")];
        pulled.headers.event = "Renamed".into();
        let back = apply_layer(&pulled, &layer).game;
        assert_eq!(back.headers.event, "Renamed");
        assert_eq!((back.headers.eco.as_str(), back.headers.annotator.as_str()), ("C58", "Coach"));
        assert_eq!(
            back.headers.extra,
            [
                cmd("EventDate", "2025.03.12"),
                cmd("PlyCount", "2"),
                cmd("Opening", "Ours"),
                cmd(ORIENTATION, "black"),
                cmd("WhiteTitle", "GM")
            ]
        );
        // An Orientation written otherwise is the same.
        let upper = game("[Orientation \"Black\"]\n\n1. e4 *");
        let pulled = lichess_like(&lichess_projection(&upper), false);
        assert_eq!(pulled.headers.get_extra(ORIENTATION), Some("black"));
        assert_eq!(apply_layer(&pulled, &local_only_layer(&upper)).game, upper);
    }

    // ---- The fixture through Lichess ---------------------------------------------

    /// Each fixture game sent as its projection and fetched back as
    /// lichess.org did, then its layer put back: the game exactly, nothing
    /// orphaned; and the merge's view of each side is the same.
    #[test]
    fn the_fixture_comes_back_whole() {
        for (n, g) in fixture().iter().enumerate() {
            let layer = local_only_layer(g);
            let pulled = lichess_like(&lichess_projection(g), false);
            let applied = apply_layer(&pulled, &layer);
            assert_eq!(applied.orphaned, [], "game {}", n + 1);
            assert_eq!(applied.game, *g, "game {}", n + 1);
            assert_eq!(normalize_for_compare(&pulled), normalize_for_compare(g), "game {}", n + 1);
            assert_eq!(Layer::from_bytes(&layer.to_bytes()).unwrap(), layer, "game {}", n + 1);
        }
    }

    /// The round-trip check sent the games whole, `%emt` and all, and Lichess
    /// made a clock of it: that clock is the only difference left, which is
    /// why a push sends the projection.
    #[test]
    fn a_game_sent_whole_gets_lichess_s_clock_from_an_emt() {
        let g = &fixture()[0];
        let pulled = lichess_like(g, true);
        let ng5 = &["e4", "e5", "Nf3", "Nc6", "Bc4", "Nf6", "Ng5"];
        assert_eq!(commands(&pulled, node_at(&pulled, ng5)), [cmd("clk", "0:04:54")], "as on lichess.org");
        let back = apply_layer(&pulled, &local_only_layer(g)).game;
        assert_eq!(commands(&back, node_at(&back, ng5)), [cmd("clk", "0:04:54"), cmd("emt", "0:00:04")]);
        let mut expected = g.clone();
        let id = node_at(&expected, ng5);
        expected.node_mut(id).commands.insert(0, cmd("clk", "0:04:54"));
        assert_eq!(back, expected);
    }

    // ---- Lichess's edits ---------------------------------------------------------

    /// Edits made on Lichess come over; what only we hold goes back where its
    /// moves still are; the rest comes back as orphaned.
    #[test]
    fn edits_on_lichess_and_what_is_orphaned() {
        let ours = game(
            "1. e4 $1 $142 { Our  comment [%clk 0:04:57.5] [%eval 0.3] } e5 \
             ( { Before } 1... c5 $13 { [%eval 0.2] } 2. Nf3 $146 $150 ) \
             ( 1... e6 { [%eval 0.1] [%emt 0:00:02] } ) 2. Nf3 *",
        );
        let layer = local_only_layer(&ours);
        let mut pulled = lichess_projection(&ours);
        let e4 = node_at(&pulled, &["e4"]);
        // On Lichess: the comment rewritten, $1 made $2, the clock changed,
        // 1...e6 deleted, 1...c5 made the main line, a line added.
        pulled.node_mut(e4).comment_after = "Their comment".into();
        pulled.node_mut(e4).nags = vec![Nag(2)];
        pulled.node_mut(e4).commands = vec![cmd("clk", "0:01:23")];
        let e6 = node_at(&pulled, &["e4", "e6"]);
        pulled.node_mut(e4).children.retain(|&c| c != e6);
        pulled.node_mut(e4).children.swap(0, 1);
        let d5 = chess_core::position::parse_uci(&pulled.position_at(e4), "d7d5").unwrap();
        let d5 = pulled.add_child(e4, d5);
        pulled.node_mut(d5).comment_after = "Added there".into();

        let Applied { game: back, orphaned } = apply_layer(&pulled, &layer);
        let n = back.node(node_at(&back, &["e4"]));
        assert_eq!(n.comment_after, "Their comment");
        assert_eq!(n.nags, [Nag(2), Nag(142)], "theirs, and ours Lichess can't hold");
        assert_eq!(n.commands, [cmd("clk", "0:01:23"), cmd("eval", "0.3")]);
        let c5 = node_at(&back, &["e4", "c5"]);
        assert_eq!(back.node(e4).children[0], c5, "c5 is the main line there");
        assert_eq!(back.node(c5).comment_before, "Before");
        assert_eq!(back.node(c5).commands, [cmd("eval", "0.2")]);
        assert_eq!(back.node(node_at(&back, &["e4", "c5", "Nf3"])).nags, [Nag(146), Nag(150)]);
        assert_eq!(back.node(node_at(&back, &["e4", "d5"])).comment_after, "Added there");
        assert_eq!(orphaned.len(), 1, "{orphaned:#?}");
        let o = &orphaned[0];
        assert_eq!((o.reason, o.line.as_str()), (OrphanReason::MoveGone, "1.e4 e6"));
        assert_eq!(o.extras.commands, Some(vec![cmd("eval", "0.1"), cmd("emt", "0:00:02")]));
        assert_eq!(o.extras.comment_after, None);

        // A comment before a move there (it can't come from Lichess, but if
        // it did): ours is orphaned, not merged in.
        let mut odd = lichess_projection(&ours);
        let c5 = node_at(&odd, &["e4", "c5"]);
        odd.node_mut(c5).comment_before = "Theirs".into();
        let applied = apply_layer(&odd, &layer);
        assert_eq!(applied.game.node(c5).comment_before, "Theirs");
        let o = &applied.orphaned[0];
        assert_eq!(
            (o.reason, o.extras.comment_before.as_str()),
            (OrphanReason::CommentBeforeTaken, "Before")
        );

        // Another start position: every path means something else.
        let mut moved = lichess_projection(&ours);
        moved.setup = Some(from_fen("4k3/8/8/8/8/8/4P3/4K3 w - - 0 1").unwrap());
        let applied = apply_layer(&moved, &layer);
        assert!(applied.orphaned.iter().all(|o| o.reason == OrphanReason::StartChanged));
        let lines: Vec<&str> = applied.orphaned.iter().map(|o| o.line.as_str()).collect();
        assert_eq!(lines, ["1.e4", "1.e4 c5", "1.e4 c5 2.Nf3", "1.e4 e6"]);
    }

    #[test]
    fn a_layer_that_cant_be_read_is_an_error() {
        let g = &fixture()[1];
        let bytes = local_only_layer(g).to_bytes();
        for cut in 0..bytes.len() {
            assert!(Layer::from_bytes(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        let mut newer = bytes.clone();
        newer[0] = 2;
        assert!(matches!(Layer::from_bytes(&newer), Err(NetError::Db(DbError::UnsupportedVersion(2)))));
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(Layer::from_bytes(&longer).is_err());
        // A path whose move isn't one of the position's: its length, then
        // its moves (none here), the fields, the comment.
        let layer = Layer {
            nodes: vec![(
                MovePath(vec![]),
                NodeExtras { comment_before: "x".into(), ..NodeExtras::default() },
            )],
            ..Layer::default()
        };
        let mut bad = layer.to_bytes();
        let at = bad.len() - 4;
        assert_eq!(bad[at..], [0, F_BEFORE, 1, b'x']);
        bad.splice(at..at + 1, [1, 200]);
        assert!(Layer::from_bytes(&bad).is_err());
    }

    // ---- Properties -----------------------------------------------------------------

    /// A small generator of numbers, from a seed.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }

        fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
            &items[self.below(items.len())]
        }
    }

    const TEXTS: [&str; 12] = [
        "Good.",
        "A  double  space",
        "Line\nbreak",
        " padded ",
        "with } brace",
        "{ open",
        "Très bien — ½ ♘ 🙂",
        "1-0 White wins.",
        "1/2-1/2   The game is a draw.",
        "( brackets ) and ; semicolons",
        "Black to move: the opposition.",
        "   ",
    ];
    const NAGS: [u8; 14] = [1, 2, 3, 4, 6, 7, 10, 14, 36, 132, 138, 142, 146, 220];
    const CLOCKS: [&str; 7] = ["0:05:00", "0:04:57.5", "0:4:5", "1:30:00.25", "0:00:00.4", "4:57", "x"];
    const COMMANDS: [(&str, &str); 6] = [
        ("eval", "0.30"),
        ("eval", "#-7"),
        ("eval", "0.25,22"),
        ("emt", "0:00:04"),
        ("foo", "bar baz"),
        ("foo", ""),
    ];
    const SETUPS: [&str; 2] = ["8/8/4k3/8/4PK2/8/8/8 b - - 0 41", "4k3/1P4P1/8/8/8/8/1p4p1/4K3 w - - 0 50"];

    /// A game with a random tree of moves and random annotations of every
    /// kind (comments before moves, spacing and `}`s, NAGs held or not,
    /// shapes in any order, clocks in any form, evals and other commands),
    /// random tags in a random order, from the usual start or not. Its
    /// texts are ones our PGN reader could give (no `[%` in comments, tag
    /// values trimmed).
    fn random_game(seed: u64) -> Game {
        let mut rng = Rng(seed | 1);
        let setup = (rng.below(4) == 0).then(|| from_fen(rng.pick(&SETUPS)).unwrap());
        let mut g = Game::new(Headers::default(), setup);
        let moves = rng.below(40);
        let mut last = NodeId::ROOT;
        for _ in 0..moves {
            let at = if rng.below(10) < 7 {
                last
            } else {
                let all = g.preorder();
                *rng.pick(&all)
            };
            let there: Vec<Move> = g.node(at).children.iter().filter_map(|&c| g.node(c).mv).collect();
            let free: Vec<Move> =
                g.position_at(at).legal_moves().into_iter().filter(|m| !there.contains(m)).collect();
            if free.is_empty() {
                continue;
            }
            last = g.add_child(at, *rng.pick(&free));
        }
        for id in g.preorder() {
            if rng.below(3) > 0 {
                continue;
            }
            let root = id == NodeId::ROOT;
            let mut x = Node::default();
            if rng.below(2) == 0 {
                x.comment_after = rng.pick(&TEXTS).to_string();
            }
            if !root && rng.below(4) == 0 {
                x.comment_before = rng.pick(&TEXTS).to_string();
            }
            for _ in 0..rng.below(4) {
                x.nags.push(Nag(*rng.pick(&NAGS)));
            }
            for _ in 0..rng.below(4) {
                let color =
                    *rng.pick(&[ShapeColor::Green, ShapeColor::Red, ShapeColor::Yellow, ShapeColor::Blue]);
                let from = Square::new(rng.below(64) as u32);
                let to = (rng.below(2) == 0).then(|| Square::new(rng.below(64) as u32));
                x.shapes.push(Shape { color, from, to });
            }
            for _ in 0..rng.below(4) {
                if rng.below(2) == 0 {
                    x.commands.push(cmd("clk", rng.pick(&CLOCKS)));
                } else {
                    let (k, v) = *rng.pick(&COMMANDS);
                    x.commands.push(cmd(k, v));
                }
            }
            let n = g.node_mut(id);
            n.comment_after = x.comment_after;
            n.comment_before = x.comment_before;
            n.nags = x.nags;
            n.shapes = x.shapes;
            n.commands = x.commands;
        }
        let h = &mut g.headers;
        h.white = rng.pick(&["", "Student", "Ñúñez, José"]).to_string();
        h.event = rng.pick(&["", "Club"]).to_string();
        h.result =
            *rng.pick(&[GameResult::Unknown, GameResult::WhiteWins, GameResult::BlackWins, GameResult::Draw]);
        h.white_elo = (rng.below(2) == 0).then_some(2234);
        h.eco = rng.pick(&["", "C58"]).to_string();
        h.annotator = rng.pick(&["", "Coach"]).to_string();
        let orientation = *rng.pick(&["white", "black", "White"]);
        let mut tags = vec![
            ("Opening", "Two Knights"),
            ("EventDate", "2025.03.12"),
            ("TimeControl", "300+3"),
            (ORIENTATION, orientation),
            ("StudyName", "S"),
            ("ChapterName", "C"),
            ("ChapterURL", "https://lichess.org/study/abcdEFGH/ijklMNOP"),
            ("WhiteTitle", "FM"),
            ("Termination", "Normal"),
            ("PlyCount", "40"),
            ("Custom", ""),
        ];
        tags.retain(|_| rng.below(2) == 0);
        for i in (1..tags.len()).rev() {
            let j = rng.below(i + 1);
            tags.swap(i, j);
        }
        h.extra = tags.into_iter().map(|(k, v)| cmd(k, v)).collect();
        g
    }

    /// The same changes to two games of the same shape (one a projection of
    /// the other: their arenas are alike): some moves' variations put in
    /// another order, and a line added.
    fn reorder_and_add(rng: &mut Rng, a: &mut Game, b: &mut Game) {
        for id in a.preorder() {
            let n = a.node(id).children.len();
            if n >= 3 || (n == 2 && rng.below(2) == 0) {
                let mut alts: Vec<NodeId> = a.node(id).children[1..].to_vec();
                alts.rotate_left(1);
                alts.reverse();
                for g in [&mut *a, &mut *b] {
                    g.node_mut(id).children.truncate(1);
                    g.node_mut(id).children.extend(&alts);
                }
            }
        }
        let all = a.preorder();
        let at = *rng.pick(&all);
        let there: Vec<Move> = a.node(at).children.iter().filter_map(|&c| a.node(c).mv).collect();
        if let Some(mv) = a.position_at(at).legal_moves().into_iter().find(|m| !there.contains(m)) {
            for g in [a, b] {
                let id = g.add_child(at, mv);
                g.node_mut(id).comment_after = "Added on Lichess".into();
            }
        }
    }

    /// The properties below aren't empty: the random games have what they
    /// should, often enough.
    #[test]
    fn the_random_games_have_everything() {
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for seed in 0..200u64 {
            let g = random_game(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let layer = local_only_layer(&g);
            let nodes: Vec<&Node> = g.preorder().into_iter().map(|id| g.node(id)).collect();
            for (what, yes) in [
                ("variations", g.has_variations()),
                ("three continuations", nodes.iter().any(|n| n.children.len() >= 3)),
                ("a comment before a move", nodes.iter().any(|n| !n.comment_before.is_empty())),
                ("a comment's spacing", layer.nodes.iter().any(|(_, x)| x.comment_after.is_some())),
                (
                    "a NAG Lichess lacks",
                    nodes.iter().any(|n| n.nags.iter().any(|g| !LICHESS_NAGS.contains(&g.0))),
                ),
                ("shapes out of order", layer.nodes.iter().any(|(_, x)| x.shapes.is_some())),
                ("a clock to the tenth", nodes.iter().any(|n| n.commands.contains(&cmd("clk", "0:04:57.5")))),
                ("an eval", nodes.iter().any(|n| n.commands.iter().any(|(k, _)| k == "eval"))),
                ("a start position", g.setup.is_some()),
                ("a result comment", is_lichess_addition(&g.node(g.line_end(NodeId::ROOT)).comment_after)),
                ("ECO", !g.headers.eco.is_empty()),
                ("a study tag", g.headers.get_extra("ChapterURL").is_some()),
                ("no Orientation", g.headers.get_extra(ORIENTATION).is_none()),
            ] {
                *seen.entry(what).or_default() += usize::from(yes);
            }
        }
        for (what, n) in &seen {
            assert!(*n >= 3, "{what}: {n} of 200 games");
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(400))]

        /// The projection and the layer give the game back exactly: no
        /// clock rounding or spacing allowance (the layer keeps clocks to
        /// the tenth and comments as written).
        #[test]
        fn projection_and_layer_give_the_game_back(seed in any::<u64>()) {
            let g = random_game(seed);
            let applied = apply_layer(&lichess_projection(&g), &local_only_layer(&g));
            prop_assert_eq!(applied.orphaned, vec![]);
            prop_assert_eq!(applied.game, g);
        }

        /// Variations reordered and a line added on Lichess: what only we
        /// hold follows its moves.
        #[test]
        fn what_we_hold_follows_its_moves(seed in any::<u64>()) {
            let g = random_game(seed);
            let layer = local_only_layer(&g);
            let mut pulled = lichess_projection(&g);
            let mut expected = g.clone();
            reorder_and_add(&mut Rng(seed.rotate_left(17) | 1), &mut pulled, &mut expected);
            let applied = apply_layer(&pulled, &layer);
            prop_assert_eq!(applied.orphaned, vec![]);
            prop_assert_eq!(applied.game, expected);
        }

        /// Sent as its projection and fetched back as lichess.org did
        /// (through our PGN writer and reader), the layer gives the game
        /// back, and both sides compare equal.
        #[test]
        fn what_lichess_sends_back_gives_the_game_back(seed in any::<u64>()) {
            let g = random_game(seed);
            let pulled = lichess_like(&lichess_projection(&g), false);
            let applied = apply_layer(&pulled, &local_only_layer(&g));
            prop_assert_eq!(applied.orphaned, vec![]);
            prop_assert_eq!(&applied.game, &g);
            prop_assert_eq!(normalize_for_compare(&pulled), normalize_for_compare(&g));
        }

        #[test]
        fn projecting_twice_is_projecting_once(seed in any::<u64>()) {
            let g = random_game(seed);
            let p = lichess_projection(&g);
            prop_assert_eq!(lichess_projection(&p), p.clone());
            let n = normalize_for_compare(&g);
            prop_assert_eq!(normalize_for_compare(&n), n.clone());
            prop_assert_eq!(normalize_for_compare(&p), n);
            // A projection's layer has nothing for its nodes.
            prop_assert_eq!(local_only_layer(&p).nodes, vec![]);
        }

        /// A projection is what our PGN writer and reader carry: written and
        /// read back, it's itself.
        #[test]
        fn a_projection_survives_our_pgn_path(seed in any::<u64>()) {
            let p = lichess_projection(&random_game(seed));
            prop_assert_eq!(read_pgn_str(&write_game(&p)).remove(0), p);
        }

        #[test]
        fn the_layer_reads_back(seed in any::<u64>()) {
            let layer = local_only_layer(&random_game(seed));
            prop_assert_eq!(Layer::from_bytes(&layer.to_bytes()).unwrap(), layer);
        }
    }
}
