//! The three-way merge of a linked game (NET-13): this database's game, the
//! chapter as Lichess has it now, and the base snapshot (Lichess's export at
//! the last sync), compared as what Lichess holds of them
//! ([`lichess_projection`]), move by move.
//!
//! Nodes are matched by their moves from the start, so a line reordered or
//! added on one side is the same line. Field by field, a change on one side
//! goes in; the same change on both is one change; different changes to the
//! same field are a [`Conflict`]:
//!
//! - **Comments, clocks**: one value each.
//! - **NAGs**: a move's assessment (`!`, `?`…) and a position's (`+=`, `=`…)
//!   are one value each; the others merge as a set.
//! - **Arrows and highlights**: a set: what either side added is in, what
//!   either removed is out. Never a conflict.
//! - **Lines**: a line added on either side is in. A line removed on one side
//!   goes if the other side didn't change it; changed there, it's a conflict.
//!   The order of a move's lines (which is the main one) comes from the side
//!   that changed it; both changing it to different main lines is a conflict.
//! - **Tags** Lichess holds (the roster, ratings, result, titles, time
//!   control, termination, orientation): one value each. An Event, Date or
//!   Orientation this game doesn't have is Lichess's own (it fills them in:
//!   a study's name and the chapter's number, the day, the side it picked)
//!   and isn't a change here.
//! - **The start position**: changed on both sides to different ones, a
//!   conflict.
//!
//! [`Merged`] is the game as Lichess will hold it, this game's values
//! wherever they won, and what was left for the user. The sync puts the
//! local-only layer back on it ([`super::layer::apply_layer`]). No network:
//! pure functions of games.

use chess_core::game::{Game, GameResult, Headers, Nag, NagCategory, NodeId, Shape};
use chess_core::notation::san;
use chess_core::position::to_fen;
use chess_core::shakmaty::{Chess, Move, Position};

use super::layer::{LICHESS_TAGS, is_lichess_addition, lichess_projection};

/// Tags Lichess fills in when a chapter has none: the study's name and the
/// chapter's number as its Event, the day as its Date, the side it picked as
/// its Orientation (seen on lichess.org, 2026-11-16).
const FILLED: [&str; 3] = ["Event", "Date", "Orientation"];

/// A merge that would take more than this many of this game's moves away
/// (lines removed on Lichess) is left for the user.
pub const LARGE_REMOVAL: usize = 10;

/// What the merge couldn't decide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictKind {
    Comment,
    Nags,
    Clock,
    /// A tag, by name.
    Tag(String),
    /// A line removed on Lichess that was changed here.
    RemovedThere,
    /// A line removed here that was changed on Lichess.
    RemovedHere,
    /// Both sides made another move the main line.
    MainLine,
    /// Both sides changed the start position.
    Start,
    /// A clean merge that would take this many of this game's moves away
    /// (lines removed on Lichess), some of its main line perhaps: too much
    /// to do without asking ([`Merged::is_large`]).
    Large(usize),
}

/// One place where both sides changed the same thing differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// Where: a move ("12...Nf6"), "the start" or "the tags".
    pub at: String,
    pub kind: ConflictKind,
    /// What each side has there, in words ("" for nothing).
    pub mine: String,
    pub theirs: String,
}

impl Conflict {
    /// In a few words: "12...Nf6: comment".
    pub fn describe(&self) -> String {
        let what = match &self.kind {
            ConflictKind::Comment => "comment".to_owned(),
            ConflictKind::Nags => "move or position symbols".to_owned(),
            ConflictKind::Clock => "clock".to_owned(),
            ConflictKind::Tag(name) => format!("the {name} tag"),
            ConflictKind::RemovedThere => "line removed on Lichess, changed here".to_owned(),
            ConflictKind::RemovedHere => "line removed here, changed on Lichess".to_owned(),
            ConflictKind::MainLine => "another main line on each side".to_owned(),
            ConflictKind::Start => "another start position on each side".to_owned(),
            ConflictKind::Large(n) => {
                let moves = if *n == 1 { "1 move".to_owned() } else { format!("{n} moves") };
                format!("lines removed on Lichess would take {moves} of this game away")
            }
        };
        format!("{}: {what}", self.at)
    }
}

impl Merged {
    /// What's left for the user: the conflicts, or for a large merge, that.
    pub fn questions(&self) -> Vec<Conflict> {
        let mut out = self.conflicts.clone();
        if out.is_empty() && self.is_large() {
            out.push(Conflict {
                at: "the moves".into(),
                kind: ConflictKind::Large(self.removed_here),
                mine: String::new(),
                theirs: String::new(),
            });
        }
        out
    }
}

/// What [`merge`] made.
#[derive(Debug, Clone)]
pub struct Merged {
    /// The game as Lichess will hold it: tags and moves merged, with this
    /// game's values (their order and spelling) wherever they won. Where
    /// there was a conflict, this game's.
    pub game: Game,
    pub conflicts: Vec<Conflict>,
    /// This game's moves the merge takes away (lines removed on Lichess).
    pub removed_here: usize,
    /// Some of them are on this game's main line.
    pub main_line_removed: bool,
}

impl Merged {
    /// Nothing for the user to decide.
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }

    /// It would take much of this game away: more than [`LARGE_REMOVAL`]
    /// moves, or any of its main line.
    pub fn is_large(&self) -> bool {
        self.removed_here > LARGE_REMOVAL || self.main_line_removed
    }
}

// ---- What the merge sees ----------------------------------------------------------

/// A node and the tree below it, as Lichess holds it.
#[derive(Debug, Clone)]
struct Tree {
    mv: Option<Move>,
    comment: String,
    nags: Vec<Nag>,
    shapes: Vec<Shape>,
    clocks: Vec<(String, String)>,
    children: Vec<Tree>,
}

fn shape_key(s: &Shape) -> (bool, char, u8, u8) {
    (s.to.is_some(), s.color.letter(), s.from as u8, s.to.map_or(64, |t| t as u8))
}

fn sorted_nags(nags: &[Nag]) -> Vec<Nag> {
    let mut v = nags.to_vec();
    v.sort_unstable();
    v.dedup();
    v
}

fn sorted_shapes(shapes: &[Shape]) -> Vec<Shape> {
    let mut v = shapes.to_vec();
    v.sort_by_key(shape_key);
    v.dedup();
    v
}

impl Tree {
    fn of(g: &Game, id: NodeId) -> Tree {
        let n = g.node(id);
        Tree {
            mv: n.mv,
            comment: n.comment_after.clone(),
            nags: n.nags.clone(),
            shapes: n.shapes.clone(),
            clocks: n.commands.clone(),
            children: n.children.iter().map(|&c| Tree::of(g, c)).collect(),
        }
    }

    fn same_fields(&self, o: &Tree) -> bool {
        self.mv == o.mv
            && self.comment == o.comment
            && sorted_nags(&self.nags) == sorted_nags(&o.nags)
            && sorted_shapes(&self.shapes) == sorted_shapes(&o.shapes)
            && self.clocks == o.clocks
    }

    /// The same, all the way down, lines in the same order.
    fn same(&self, o: &Tree) -> bool {
        self.same_fields(o)
            && self.children.len() == o.children.len()
            && self.children.iter().zip(&o.children).all(|(a, b)| a.same(b))
    }

    fn size(&self) -> usize {
        1 + self.children.iter().map(Tree::size).sum::<usize>()
    }

    fn child(&self, mv: Move) -> Option<&Tree> {
        self.children.iter().find(|c| c.mv == Some(mv))
    }

    fn moves(&self) -> Vec<Move> {
        self.children.iter().filter_map(|c| c.mv).collect()
    }
}

/// A game as Lichess holds it: its projection, without Lichess's result
/// comment on the main line's last move, the orientation in lower case.
fn view(game: &Game) -> Game {
    let mut g = lichess_projection(game);
    let last = g.line_end(NodeId::ROOT);
    if is_lichess_addition(&g.node(last).comment_after) {
        g.node_mut(last).comment_after.clear();
    }
    for (k, v) in &mut g.headers.extra {
        if k == "Orientation" {
            *v = v.trim().to_ascii_lowercase();
        }
    }
    g
}

/// The tags Lichess holds, by name, with their values (`None`: none).
fn tags(h: &Headers) -> Vec<(&'static str, Option<String>)> {
    let text = |s: &str| Some(s.trim().to_owned()).filter(|s| !s.is_empty());
    let mut out = vec![
        ("Event", text(&h.event)),
        ("Site", text(&h.site)),
        ("Date", text(&h.date)),
        ("Round", text(&h.round)),
        ("White", text(&h.white)),
        ("Black", text(&h.black)),
        ("Result", Some(h.result.as_str().to_owned())),
        ("WhiteElo", h.white_elo.map(|e| e.to_string())),
        ("BlackElo", h.black_elo.map(|e| e.to_string())),
    ];
    for name in LICHESS_TAGS {
        out.push((name, h.get_extra(name).and_then(text)));
    }
    out
}

/// Whether this game's tag is the base's: the same, or one Lichess filled in
/// that this game doesn't have.
fn same_tag_here(name: &str, ours: &Option<String>, base: &Option<String>) -> bool {
    ours == base || (ours.is_none() && FILLED.contains(&name))
}

fn fen(g: &Game) -> Option<String> {
    g.setup.as_ref().map(to_fen)
}

/// Whether two games are the same as Lichess holds them: a chapter that
/// didn't change since its base, or one that has what was sent to it.
pub fn same_on_lichess(a: &Game, b: &Game) -> bool {
    let (a, b) = (view(a), view(b));
    fen(&a) == fen(&b)
        && tags(&a.headers) == tags(&b.headers)
        && Tree::of(&a, NodeId::ROOT).same(&Tree::of(&b, NodeId::ROOT))
}

/// Whether this database's game changed since the base, in what Lichess
/// holds of it: what would go to Lichess. Tags Lichess filled in that the
/// game doesn't have aren't changes; what only this database holds
/// (evals, comments before a variation's first move…) isn't either.
pub fn changed_here(ours: &Game, base: &Game) -> bool {
    let (o, b) = (view(ours), view(base));
    let tags_same =
        tags(&o.headers).iter().zip(tags(&b.headers)).all(|((name, x), (_, y))| same_tag_here(name, x, &y));
    !(tags_same && fen(&o) == fen(&b) && Tree::of(&o, NodeId::ROOT).same(&Tree::of(&b, NodeId::ROOT)))
}

/// The tags to send to a chapter (`theirs`, as Lichess has it) so that it
/// has those of `ours`: each Lichess holds whose value differs, with an
/// empty value for one to take away. Never one Lichess filled in that `ours`
/// doesn't have.
pub fn tag_changes(ours: &Game, theirs: &Game) -> Vec<(String, String)> {
    let (o, t) = (view(ours), view(theirs));
    tags(&o.headers)
        .into_iter()
        .zip(tags(&t.headers))
        .filter(|((name, ov), (_, tv))| !same_tag_here(name, ov, tv))
        .map(|((name, ov), _)| (name.to_owned(), ov.unwrap_or_default()))
        .collect()
}

// ---- The merge ----------------------------------------------------------------------

/// One value from three: a change on one side wins, the same on both is
/// that; `None` for a conflict. With no base (a node both sides added), an
/// empty value gives way to the other.
fn pick<T: PartialEq + Clone>(b: Option<&T>, o: &T, t: &T, empty: impl Fn(&T) -> bool) -> Option<T> {
    if o == t {
        return Some(o.clone());
    }
    match b {
        Some(b) if b == o => Some(t.clone()),
        Some(b) if b == t => Some(o.clone()),
        Some(_) => None,
        None if empty(o) => Some(t.clone()),
        None if empty(t) => Some(o.clone()),
        None => None,
    }
}

/// Three sets merged: in if both have it or one added it, out if one
/// removed it.
fn set_merge<T: PartialEq + Clone>(b: &[T], o: &[T], t: &[T]) -> Vec<T> {
    let mut out: Vec<T> = Vec::new();
    for x in o.iter().chain(t) {
        if out.contains(x) {
            continue;
        }
        let (ib, io, it) = (b.contains(x), o.contains(x), t.contains(x));
        if (io && it) || (!ib && (io || it)) {
            out.push(x.clone());
        }
    }
    out
}

struct Merger {
    conflicts: Vec<Conflict>,
    removed_here: usize,
    main_line_removed: bool,
}

fn nag_text(nags: &[Nag]) -> String {
    nags.iter().map(|n| format!("${}", n.0)).collect::<Vec<_>>().join(" ")
}

fn clock_text(clocks: &[(String, String)]) -> String {
    clocks.iter().map(|(_, v)| v.as_str()).collect::<Vec<_>>().join(" ")
}

/// A node's name: "12...Nf6", "the start".
fn label(pos: &Chess, mv: Option<Move>) -> String {
    match mv {
        None => "the start".to_owned(),
        Some(mv) => {
            let n = pos.fullmoves().get();
            let dots = if pos.turn().is_white() { "." } else { "..." };
            format!("{n}{dots}{}", san(pos, mv))
        }
    }
}

impl Merger {
    fn conflict(&mut self, at: &str, kind: ConflictKind, mine: String, theirs: String) {
        self.conflicts.push(Conflict { at: at.to_owned(), kind, mine, theirs });
    }

    /// A node both sides have (and the base, if `b`), merged. `pos` is the
    /// position before its move; `main`: it's on this game's main line.
    fn node(&mut self, b: Option<&Tree>, o: &Tree, t: &Tree, pos: &Chess, main: bool) -> Tree {
        let at = label(pos, o.mv);
        let mut out = Tree {
            mv: o.mv,
            comment: String::new(),
            nags: Vec::new(),
            shapes: Vec::new(),
            clocks: Vec::new(),
            children: Vec::new(),
        };
        out.comment =
            pick(b.map(|b| &b.comment), &o.comment, &t.comment, String::is_empty).unwrap_or_else(|| {
                self.conflict(&at, ConflictKind::Comment, o.comment.clone(), t.comment.clone());
                o.comment.clone()
            });

        // NAGs: one assessment of the move and one of the position, the rest a set.
        let of = |nags: &[Nag], cat: NagCategory| -> Vec<Nag> {
            sorted_nags(&nags.iter().copied().filter(|n| n.category() == cat).collect::<Vec<_>>())
        };
        let no_nags: &[Nag] = &[];
        let bn = b.map_or(no_nags, |b| &b.nags);
        let mut nags = Vec::new();
        let mut nag_conflict = false;
        for cat in [NagCategory::Move, NagCategory::Position] {
            let (bv, ov, tv) = (of(bn, cat), of(&o.nags, cat), of(&t.nags, cat));
            match pick(b.map(|_| &bv), &ov, &tv, Vec::is_empty) {
                Some(v) => nags.extend(v),
                None => {
                    nag_conflict = true;
                    nags.extend(ov);
                }
            }
        }
        let other = |nags: &[Nag]| of(nags, NagCategory::Other);
        nags.extend(set_merge(&other(bn), &other(&o.nags), &other(&t.nags)));
        if nag_conflict {
            self.conflict(&at, ConflictKind::Nags, nag_text(&o.nags), nag_text(&t.nags));
        }
        // This game's order where it's this game's set, else Lichess's.
        out.nags = if sorted_nags(&nags) == sorted_nags(&o.nags) {
            o.nags.clone()
        } else if sorted_nags(&nags) == sorted_nags(&t.nags) {
            t.nags.clone()
        } else {
            nags
        };

        let no_shapes: &[Shape] = &[];
        let shapes = set_merge(b.map_or(no_shapes, |b| &b.shapes), &o.shapes, &t.shapes);
        out.shapes = if sorted_shapes(&shapes) == sorted_shapes(&o.shapes) {
            o.shapes.clone()
        } else if sorted_shapes(&shapes) == sorted_shapes(&t.shapes) {
            t.shapes.clone()
        } else {
            shapes
        };

        out.clocks = pick(b.map(|b| &b.clocks), &o.clocks, &t.clocks, Vec::is_empty).unwrap_or_else(|| {
            self.conflict(&at, ConflictKind::Clock, clock_text(&o.clocks), clock_text(&t.clocks));
            o.clocks.clone()
        });

        let mut next = pos.clone();
        if let Some(mv) = o.mv {
            next.play_unchecked(mv);
        }
        out.children = self.children(b, o, t, &next, main);
        out
    }

    /// The lines after a node, merged, in their merged order.
    fn children(&mut self, b: Option<&Tree>, o: &Tree, t: &Tree, pos: &Chess, main: bool) -> Vec<Tree> {
        let no_kids: &[Tree] = &[];
        let bk = b.map_or(no_kids, |b| &b.children);
        let mut moves: Vec<Move> = Vec::new();
        for c in o.children.iter().chain(&t.children).chain(bk) {
            if let Some(mv) = c.mv
                && !moves.contains(&mv)
            {
                moves.push(mv);
            }
        }
        let first_here = o.children.first().and_then(|c| c.mv);
        let mut kids: Vec<Tree> = Vec::new();
        for mv in moves {
            let bc = bk.iter().find(|c| c.mv == Some(mv));
            let on_main = main && first_here == Some(mv);
            match (bc, o.child(mv), t.child(mv)) {
                (_, Some(oc), Some(tc)) => kids.push(self.node(bc, oc, tc, pos, on_main)),
                (Some(bc), Some(oc), None) => {
                    if bc.same(oc) {
                        self.removed_here += oc.size();
                        self.main_line_removed |= on_main;
                    } else {
                        let at = label(pos, Some(mv));
                        self.conflict(&at, ConflictKind::RemovedThere, String::new(), String::new());
                        kids.push(oc.clone());
                    }
                }
                (Some(bc), None, Some(tc)) => {
                    if !bc.same(tc) {
                        let at = label(pos, Some(mv));
                        self.conflict(&at, ConflictKind::RemovedHere, String::new(), String::new());
                        kids.push(tc.clone());
                    }
                }
                (None, Some(oc), None) => kids.push(oc.clone()),
                (None, None, Some(tc)) => kids.push(tc.clone()),
                (Some(_), None, None) | (None, None, None) => {}
            }
        }
        // The order: from the side that changed it.
        let base_moves: Vec<Move> = bk.iter().filter_map(|c| c.mv).collect();
        let (om, tm) = (o.moves(), t.moves());
        let (o_changed, t_changed) = (order_changed(&base_moves, &om), order_changed(&base_moves, &tm));
        if o_changed && t_changed && om.first() != tm.first() {
            let name = |m: Option<&Move>| m.map(|&m| san(pos, m)).unwrap_or_default();
            let at = om.first().map_or_else(|| label(pos, tm.first().copied()), |&m| label(pos, Some(m)));
            self.conflict(&at, ConflictKind::MainLine, name(om.first()), name(tm.first()));
        }
        let (first, second) = if t_changed && !o_changed { (&tm, &om) } else { (&om, &tm) };
        let mut order: Vec<Move> = Vec::with_capacity(kids.len());
        for &m in first.iter().chain(second.iter()).chain(base_moves.iter()) {
            if !order.contains(&m) && kids.iter().any(|k| k.mv == Some(m)) {
                order.push(m);
            }
        }
        order
            .into_iter()
            .filter_map(|m| kids.iter().position(|k| k.mv == Some(m)).map(|i| kids[i].clone()))
            .collect()
    }
}

/// Whether a side put a node's lines in another order than the base's: not
/// the base's lines it kept, in the base's order, then those it added.
fn order_changed(base: &[Move], side: &[Move]) -> bool {
    let kept = base.iter().filter(|m| side.contains(m));
    let added = side.iter().filter(|m| !base.contains(m));
    let expected: Vec<&Move> = kept.chain(added).collect();
    expected.len() != side.len() || expected.into_iter().zip(side).any(|(a, b)| a != b)
}

/// The tags merged into `out` (this game's projected tags to start with).
fn merge_tags(b: &Headers, o: &Headers, t: &Headers, out: &mut Headers, conflicts: &mut Vec<Conflict>) {
    for (((name, bv), (_, ov)), (_, tv)) in tags(b).into_iter().zip(tags(o)).zip(tags(t)) {
        let value = if tv == bv {
            ov
        } else if same_tag_here(name, &ov, &bv) || ov == tv {
            tv
        } else {
            conflicts.push(Conflict {
                at: "the tags".into(),
                kind: ConflictKind::Tag(name.to_owned()),
                mine: ov.clone().unwrap_or_default(),
                theirs: tv.unwrap_or_default(),
            });
            ov
        };
        set_tag(out, name, value.as_deref());
    }
}

fn set_tag(h: &mut Headers, name: &str, value: Option<&str>) {
    match name {
        "Result" => h.result = GameResult::parse(value.unwrap_or("*")),
        "Event" | "Site" | "Date" | "Round" | "White" | "Black" | "WhiteElo" | "BlackElo" => {
            h.set(name, value.unwrap_or(""));
        }
        _ => match value {
            Some(v) => match h.extra.iter_mut().find(|(k, _)| k == name) {
                Some(slot) => slot.1 = v.to_owned(),
                None => h.extra.push((name.to_owned(), v.to_owned())),
            },
            None => h.extra.retain(|(k, _)| k != name),
        },
    }
}

/// The merged tree as a game.
fn to_game(headers: Headers, setup: Option<Chess>, tree: &Tree) -> Game {
    let mut g = Game::new(headers, setup);
    let fill = |g: &mut Game, id: NodeId, t: &Tree| {
        let n = g.node_mut(id);
        n.comment_after = t.comment.clone();
        n.nags = t.nags.clone();
        n.shapes = t.shapes.clone();
        n.commands = t.clocks.clone();
    };
    fill(&mut g, NodeId::ROOT, tree);
    let mut stack: Vec<(NodeId, &Tree)> = vec![(NodeId::ROOT, tree)];
    while let Some((id, t)) = stack.pop() {
        for c in &t.children {
            let child = g.add_child(id, c.mv.expect("non-root node has a move"));
            fill(&mut g, child, c);
            stack.push((child, c));
        }
    }
    g
}

/// Merges this database's game (`ours`) and the chapter as Lichess has it
/// now (`theirs`) from the base snapshot, in what Lichess holds of them (see
/// the module's comment). The game it gives is a projection: put the
/// local-only layer of `ours` back on it for this database.
pub fn merge(base: &Game, ours: &Game, theirs: &Game) -> Merged {
    let (b, o, t) = (view(base), view(ours), view(theirs));
    let mut m = Merger { conflicts: Vec::new(), removed_here: 0, main_line_removed: false };
    let mut headers = o.headers.clone();
    merge_tags(&b.headers, &o.headers, &t.headers, &mut headers, &mut m.conflicts);
    let (bf, of, tf) = (fen(&b), fen(&o), fen(&t));
    let (bt, ot, tt) = (Tree::of(&b, NodeId::ROOT), Tree::of(&o, NodeId::ROOT), Tree::of(&t, NodeId::ROOT));
    let (setup, tree) = if of == tf {
        // A base from another start says nothing about these moves.
        let base = (bf == of).then_some(&bt);
        let tree = m.node(base, &ot, &tt, &o.start_position(), true);
        (o.setup.clone(), tree)
    } else if of == bf && ot.same(&bt) {
        (t.setup.clone(), tt)
    } else if tf == bf && tt.same(&bt) {
        (o.setup.clone(), ot)
    } else {
        m.conflict("the start", ConflictKind::Start, of.unwrap_or_default(), tf.unwrap_or_default());
        (o.setup.clone(), ot)
    };
    Merged {
        game: to_game(headers, setup, &tree),
        conflicts: m.conflicts,
        removed_here: m.removed_here,
        main_line_removed: m.main_line_removed,
    }
}

#[cfg(test)]
mod tests {
    use chess_core::game::ShapeColor;
    use chess_core::pgn::read_pgn_str;
    use chess_core::shakmaty::Square;
    use proptest::prelude::*;

    use super::*;

    fn game(pgn: &str) -> Game {
        read_pgn_str(pgn).into_iter().next().expect("a game")
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

    const BASE: &str = "[Event \"E\"]\n[White \"A\"]\n[Black \"B\"]\n[Result \"*\"]\n\n\
                        1. e4 e5 2. Nf3 { develops } Nc6 (2... d6 3. d4) 3. Bb5 a6 *";

    #[test]
    fn one_side_changed_takes_that_side() {
        let b = game(BASE);
        let t = game(&BASE.replace("{ develops }", "{ develops a knight } $1"));
        let m = merge(&b, &b, &t);
        assert!(m.is_clean() && !m.is_large());
        assert!(same_on_lichess(&m.game, &t));
        let m = merge(&b, &t, &b);
        assert!(m.is_clean() && same_on_lichess(&m.game, &t));
        assert!(!changed_here(&b, &b) && changed_here(&t, &b));
    }

    #[test]
    fn edits_in_different_places_both_go_in() {
        let b = game(BASE);
        // Here: a comment on 1.e4 and a new line at move 3. There: a NAG on
        // 3.Bb5, the 2...d6 line made the main one, an arrow at the start.
        let o = game(
            &BASE.replace("1. e4 e5", "1. e4 { king pawn } e5").replace("3. Bb5 a6", "3. Bb5 (3. Bc4) a6"),
        );
        let t = game(
            "[Event \"E\"]\n[White \"A\"]\n[Black \"B\"]\n[Result \"*\"]\n\n\
             { [%cal Ge2e4] } 1. e4 e5 2. Nf3 { develops } d6 (2... Nc6 3. Bb5 $1 a6) 3. d4 *",
        );
        let m = merge(&b, &o, &t);
        assert!(m.is_clean(), "{:?}", m.conflicts);
        let g = &m.game;
        assert_eq!(g.node(node_at(g, &["e4"])).comment_after, "king pawn");
        assert_eq!(g.root().shapes.len(), 1);
        let nf3 = node_at(g, &["e4", "e5", "Nf3"]);
        let main = g.node(nf3).children[0];
        assert_eq!(san(&g.position_at(nf3), g.node(main).mv.unwrap()), "d6", "Lichess's main line");
        let bb5 = node_at(g, &["e4", "e5", "Nf3", "Nc6", "Bb5"]);
        assert_eq!(g.node(bb5).nags, [Nag(1)]);
        let nc6 = node_at(g, &["e4", "e5", "Nf3", "Nc6"]);
        assert_eq!(g.node(nc6).children.len(), 2, "3.Bb5 and the line added here, 3.Bc4");
    }

    #[test]
    fn the_same_field_changed_both_ways_is_a_conflict() {
        let b = game(BASE);
        let o = game(&BASE.replace("{ develops }", "{ mine }"));
        let t = game(&BASE.replace("{ develops }", "{ theirs }"));
        let m = merge(&b, &o, &t);
        assert_eq!(m.conflicts.len(), 1);
        let c = &m.conflicts[0];
        assert_eq!(
            (c.at.as_str(), &c.kind, c.mine.as_str(), c.theirs.as_str()),
            ("2.Nf3", &ConflictKind::Comment, "mine", "theirs")
        );
        assert_eq!(c.describe(), "2.Nf3: comment");
        assert_eq!(m.game.node(node_at(&m.game, &["e4", "e5", "Nf3"])).comment_after, "mine", "ours kept");
        // Both the same: one change.
        assert!(merge(&b, &o, &o).is_clean());
        // `!` here, `?` there.
        let o = game(&BASE.replace("3. Bb5", "3. Bb5 $1"));
        let t = game(&BASE.replace("3. Bb5", "3. Bb5 $2"));
        assert_eq!(merge(&b, &o, &t).conflicts[0].kind, ConflictKind::Nags);
        // A tag.
        let o = game(&BASE.replace("[White \"A\"]", "[White \"Ann\"]"));
        let t = game(&BASE.replace("[White \"A\"]", "[White \"Al\"]"));
        assert_eq!(merge(&b, &o, &t).conflicts[0].kind, ConflictKind::Tag("White".into()));
    }

    #[test]
    fn lines_removed_on_one_side() {
        let b = game(BASE);
        let no_d6 = BASE.replace(" (2... d6 3. d4)", "");
        // Removed there, unchanged here: it goes.
        let m = merge(&b, &game(&BASE.replace("1. e4 e5", "1. e4 { x } e5")), &game(&no_d6));
        assert!(m.is_clean() && !m.is_large());
        assert_eq!(m.removed_here, 2);
        assert_eq!(m.game.node(node_at(&m.game, &["e4", "e5", "Nf3"])).children.len(), 1);
        // Removed there, changed here: a conflict, ours kept.
        let o = game(&BASE.replace("3. d4)", "3. d4 { keep this })"));
        let m = merge(&b, &o, &game(&no_d6));
        assert_eq!(m.conflicts[0].kind, ConflictKind::RemovedThere);
        assert_eq!(m.conflicts[0].at, "2...d6");
        assert_eq!(m.game.node(node_at(&m.game, &["e4", "e5", "Nf3"])).children.len(), 2);
        // Removed here, changed there.
        let m = merge(&b, &game(&no_d6), &o);
        assert_eq!(m.conflicts[0].kind, ConflictKind::RemovedHere);
        // The main line cut on Lichess, a comment added here: large.
        let short = "[Event \"E\"]\n[White \"A\"]\n[Black \"B\"]\n[Result \"*\"]\n\n\
                     1. e4 e5 2. Nf3 { develops } Nc6 (2... d6 3. d4) *";
        let m = merge(&b, &game(&BASE.replace("1. e4 e5", "1. e4 { x } e5")), &game(short));
        assert!(m.is_clean() && m.is_large() && m.main_line_removed);
    }

    #[test]
    fn another_main_line_on_each_side_is_a_conflict() {
        let b = game("1. e4 e5 (1... c5) (1... e6) *");
        let o = game("1. e4 c5 (1... e5) (1... e6) *");
        let t = game("1. e4 e6 (1... e5) (1... c5) *");
        let m = merge(&b, &o, &t);
        assert_eq!(m.conflicts.len(), 1);
        assert_eq!(
            (m.conflicts[0].kind.clone(), m.conflicts[0].mine.as_str(), m.conflicts[0].theirs.as_str()),
            (ConflictKind::MainLine, "c5", "e6")
        );
        // One side reordering: that side's order.
        let m = merge(&b, &b, &t);
        assert!(m.is_clean() && same_on_lichess(&m.game, &t));
        // Lines added on both sides at one move: both, ours first.
        let m = merge(&game("1. e4 *"), &game("1. e4 e5 *"), &game("1. e4 c5 *"));
        assert!(m.is_clean());
        assert_eq!(m.game.node(node_at(&m.game, &["e4"])).children.len(), 2);
    }

    #[test]
    fn tags_lichess_fills_in_are_not_changes() {
        let sent = game("[White \"A\"]\n[Black \"B\"]\n\n1. c4 e5 *");
        let back = game(
            "[Event \"Study: Chapter 2\"]\n[Date \"2026.11.17\"]\n[White \"A\"]\n[Black \"B\"]\n\
             [Orientation \"black\"]\n[ECO \"A21\"]\n[Opening \"English\"]\n[Annotator \"https://lichess.org/@/me\"]\n\n1. c4 e5 *",
        );
        assert!(!changed_here(&sent, &back));
        assert!(!same_on_lichess(&sent, &back), "but Lichess has them");
        // Merged with an edit there: the tags stay as here.
        let edited = game(&write(&back).replace("1. c4 e5", "1. c4 { English } e5"));
        let m = merge(&back, &sent, &edited);
        assert!(m.is_clean());
        assert_eq!((m.game.headers.event.as_str(), m.game.headers.date.as_str()), ("", ""));
        assert_eq!(m.game.headers.get_extra("Orientation"), None);
        // Changed there, though: theirs.
        let renamed = game(&write(&back).replace("Study: Chapter 2", "Lesson 2"));
        assert_eq!(merge(&back, &sent, &renamed).game.headers.event, "Lesson 2");
    }

    fn write(g: &Game) -> String {
        chess_core::pgn::write_game(g)
    }

    #[test]
    fn the_start_position() {
        let b = game("1. e4 *");
        let fen = "[SetUp \"1\"]\n[FEN \"8/8/8/4k3/8/8/4P3/4K3 w - - 0 1\"]\n\n1. e3 *";
        let t = game(fen);
        assert!(same_on_lichess(&merge(&b, &b, &t).game, &t));
        let o = game("[SetUp \"1\"]\n[FEN \"8/8/8/4k3/8/8/3P4/4K3 w - - 0 1\"]\n\n1. d3 *");
        assert_eq!(merge(&b, &o, &t).conflicts[0].kind, ConflictKind::Start);
    }

    #[test]
    fn a_game_s_own_values_where_they_won() {
        // Lichess's export has NAGs and arrows in its order, a result
        // comment and its rounded clocks: the merge keeps ours where they
        // mean the same.
        let b = game("1. e4 $14 $1 { [%csl Ge4][%cal Ge2e4] } { a } e5 1-0");
        let t = game("1. e4 $1 $14 { [%csl Ge4][%cal Ge2e4] a } e5 { 1-0 White wins. } 1-0");
        let o = game("1. e4 $14 $1 { [%csl Ge4][%cal Ge2e4] } { a } e5 2. Nf3 1-0");
        let m = merge(&b, &o, &t);
        assert!(m.is_clean());
        let e4 = node_at(&m.game, &["e4"]);
        assert_eq!(m.game.node(e4).nags, [Nag(14), Nag(1)]);
        let last = m.game.line_end(NodeId::ROOT);
        assert_eq!(m.game.node(last).comment_after, "");
        assert_eq!(m.game.mainline_len(), 3);
    }

    // ---- Random trees ------------------------------------------------------------------

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 33
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }
    }

    /// A random game: a main line of 6 to 20 moves, variations here and
    /// there, a few annotations.
    fn random_game(rng: &mut Rng) -> Game {
        let mut g = Game::default();
        let mut id = NodeId::ROOT;
        let len = 6 + rng.below(15);
        for _ in 0..len {
            let pos = g.position_at(id);
            let moves = pos.legal_moves();
            if moves.is_empty() {
                break;
            }
            let mv = moves[rng.below(moves.len())];
            // Sometimes a variation first.
            if rng.below(4) == 0 && moves.len() > 1 {
                let alt = moves[rng.below(moves.len())];
                if alt != mv {
                    let v = g.add_child(id, alt);
                    let vpos = g.position_at(v);
                    let vm = vpos.legal_moves();
                    if !vm.is_empty() {
                        g.add_child(v, vm[rng.below(vm.len())]);
                    }
                }
            }
            let next = g.add_child(id, mv);
            // The main line first.
            let kids = &mut g.node_mut(id).children;
            let at = kids.iter().position(|&c| c == next).unwrap();
            let k = kids.remove(at);
            kids.insert(0, k);
            if rng.below(3) == 0 {
                g.node_mut(next).comment_after = format!("c{}", rng.below(100));
            }
            if rng.below(4) == 0 {
                g.node_mut(next).nags.push(Nag([1, 2, 14, 18, 146][rng.below(5)]));
            }
            id = next;
        }
        g
    }

    /// 1 to `most` random edits to `g`, in nodes whose ply parity is `parity`
    /// (so two sides editing different parities never touch the same node),
    /// or anywhere with `None`.
    fn edit(rng: &mut Rng, g: &mut Game, parity: Option<usize>, most: usize) {
        let n = 1 + rng.below(most);
        for _ in 0..n {
            let ids: Vec<NodeId> = g
                .preorder()
                .into_iter()
                .filter(|&id| id != NodeId::ROOT && parity.is_none_or(|p| g.path(id).len() % 2 == p))
                .collect();
            if ids.is_empty() {
                return;
            }
            let id = ids[rng.below(ids.len())];
            match rng.below(4) {
                0 => g.node_mut(id).comment_after = format!("edit {}", rng.below(1000)),
                1 => {
                    let n = g.node_mut(id);
                    n.nags.retain(|x| x.category() != NagCategory::Move);
                    n.nags.push(Nag(1 + rng.below(6) as u8));
                }
                2 => {
                    let sq = Square::new(rng.below(64) as u32);
                    g.node_mut(id).shapes.push(Shape { color: ShapeColor::Red, from: sq, to: None });
                }
                _ => g.node_mut(id).commands = vec![("clk".into(), format!("0:0{}:00", rng.below(10)))],
            }
        }
    }

    /// A line added after a node of parity `parity`.
    fn add_line(rng: &mut Rng, g: &mut Game, parity: usize) {
        let ids: Vec<NodeId> =
            g.preorder().into_iter().filter(|&id| g.path(id).len() % 2 == parity).collect();
        let id = ids[rng.below(ids.len())];
        let pos = g.position_at(id);
        let moves = pos.legal_moves();
        if let Some(&mv) = moves.iter().find(|m| g.find_child(id, **m).is_none()) {
            g.add_child(id, mv);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        /// A side that didn't change gives way to the other, whatever it did.
        #[test]
        fn an_unchanged_side_gives_way(seed in any::<u64>()) {
            let mut rng = Rng(seed);
            let b = random_game(&mut rng);
            let mut x = b.clone();
            edit(&mut rng, &mut x, None, 5);
            if rng.below(2) == 0 {
                let p = rng.below(2);
                add_line(&mut rng, &mut x, p);
            }
            let m = merge(&b, &b, &x);
            prop_assert!(m.is_clean(), "{:?}", m.conflicts);
            prop_assert!(same_on_lichess(&m.game, &x));
            let m = merge(&b, &x, &b);
            prop_assert!(m.is_clean());
            prop_assert!(same_on_lichess(&m.game, &x));
            // The same change on both sides is one change.
            let m = merge(&b, &x, &x);
            prop_assert!(m.is_clean());
            prop_assert!(same_on_lichess(&m.game, &x));
        }

        /// Edits to different nodes and lines added on both sides all go
        /// in, with no conflict.
        #[test]
        fn edits_to_different_nodes_merge(seed in any::<u64>()) {
            let mut rng = Rng(seed);
            let b = random_game(&mut rng);
            let (mut o, mut t) = (b.clone(), b.clone());
            edit(&mut rng, &mut o, Some(0), 4);
            edit(&mut rng, &mut t, Some(1), 4);
            add_line(&mut rng, &mut o, 1);
            add_line(&mut rng, &mut t, 0);
            let m = merge(&b, &o, &t);
            prop_assert!(m.is_clean(), "{:?}", m.conflicts);
            prop_assert_eq!(m.removed_here, 0);
            // Every node of either side is in, with that side's edits.
            let (vo, vt, vm) = (view(&o), view(&t), view(&m.game));
            for (side, parity) in [(&vo, 0), (&vt, 1)] {
                for id in side.preorder().into_iter().skip(1) {
                    let path: Vec<Move> = side.path(id).iter().map(|&n| side.node(n).mv.unwrap()).collect();
                    let mut at = NodeId::ROOT;
                    for mv in &path {
                        let found = vm.find_child(at, *mv);
                        prop_assert!(found.is_some(), "a line is missing");
                        at = found.unwrap();
                    }
                    if path.len() % 2 == parity {
                        prop_assert_eq!(&vm.node(at).comment_after, &side.node(id).comment_after);
                        prop_assert_eq!(sorted_nags(&vm.node(at).nags), sorted_nags(&side.node(id).nags));
                        prop_assert_eq!(&vm.node(at).commands, &side.node(id).commands);
                    }
                }
            }
        }

        /// Whether there's a conflict doesn't depend on which side is ours.
        #[test]
        fn conflicts_are_symmetric(seed in any::<u64>()) {
            let mut rng = Rng(seed);
            let b = random_game(&mut rng);
            let (mut o, mut t) = (b.clone(), b.clone());
            edit(&mut rng, &mut o, None, 4);
            edit(&mut rng, &mut t, None, 4);
            let (a, z) = (merge(&b, &o, &t), merge(&b, &t, &o));
            prop_assert_eq!(a.conflicts.len(), z.conflicts.len());
            if a.is_clean() {
                // No line was added or reordered: both merges are the same.
                prop_assert!(same_on_lichess(&a.game, &z.game));
                prop_assert!(!changed_here(&a.game, &z.game));
            }
        }

        /// A merge changes nothing of what only the database holds: the
        /// game it gives is a projection, and merging it with itself is it.
        #[test]
        fn a_merge_is_a_projection(seed in any::<u64>()) {
            let mut rng = Rng(seed);
            let b = random_game(&mut rng);
            let mut o = b.clone();
            edit(&mut rng, &mut o, None, 3);
            let m = merge(&b, &o, &b);
            prop_assert!(same_on_lichess(&m.game, &lichess_projection(&m.game)));
            prop_assert!(!changed_here(&m.game, &o));
        }
    }
}
