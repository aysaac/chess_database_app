//! Game model: headers plus a move tree with nested variations and
//! annotations.
//!
//! The tree is an arena (`Vec<Node>`) indexed by [`NodeId`]; node 0 is the
//! root (the starting position, no move). `children[0]` of a node is the main
//! continuation, further children are alternatives (variations) to it.
//! Removing a subtree detaches it from its parent but leaves the nodes in the
//! arena, so ids handed out stay stable; serialization only visits attached
//! nodes.

mod editor;
mod headers;

pub use editor::{CommentSlot, GameEditor, PlayResult, VariationPolicy};
pub use headers::{GameResult, Headers};

use shakmaty::{Chess, Move, Position, Square};

use crate::position;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl NodeId {
    pub const ROOT: NodeId = NodeId(0);

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Numeric Annotation Glyph (`$1` = `!`, `$14` = `+=`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Nag(pub u8);

impl Nag {
    /// Category for mutually-exclusive toggling: move assessment (1–6),
    /// position assessment (10–19), or other.
    pub fn category(self) -> NagCategory {
        match self.0 {
            1..=6 => NagCategory::Move,
            10..=19 => NagCategory::Position,
            _ => NagCategory::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NagCategory {
    Move,
    Position,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShapeColor {
    Green,
    Red,
    Yellow,
    Blue,
}

impl ShapeColor {
    pub fn letter(self) -> char {
        match self {
            ShapeColor::Green => 'G',
            ShapeColor::Red => 'R',
            ShapeColor::Yellow => 'Y',
            ShapeColor::Blue => 'B',
        }
    }

    pub fn from_letter(c: char) -> Option<ShapeColor> {
        match c {
            'G' => Some(ShapeColor::Green),
            'R' => Some(ShapeColor::Red),
            'Y' => Some(ShapeColor::Yellow),
            'B' => Some(ShapeColor::Blue),
            _ => None,
        }
    }
}

/// Graphical annotation: an arrow (`to` set) or a highlighted square.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Shape {
    pub color: ShapeColor,
    pub from: Square,
    pub to: Option<Square>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Node {
    /// `None` only for the root.
    pub mv: Option<Move>,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    /// Only meaningful for the first move of a variation; elsewhere a
    /// comment before a move is the previous node's `comment_after`.
    pub comment_before: String,
    pub comment_after: String,
    pub nags: Vec<Nag>,
    pub shapes: Vec<Shape>,
    /// Other `[%cmd value]` comment commands (`%clk`, `%eval`, …), preserved.
    pub commands: Vec<(String, String)>,
}

impl Node {
    fn root() -> Node {
        Node::default()
    }

    pub fn has_annotations(&self) -> bool {
        !self.comment_before.is_empty()
            || !self.comment_after.is_empty()
            || !self.nags.is_empty()
            || !self.shapes.is_empty()
            || !self.commands.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct Game {
    pub headers: Headers,
    /// Starting position when not the standard one.
    pub setup: Option<Chess>,
    nodes: Vec<Node>,
}

impl Default for Game {
    fn default() -> Self {
        Game::new(Headers::default(), None)
    }
}

impl Game {
    pub fn new(headers: Headers, setup: Option<Chess>) -> Game {
        Game { headers, setup, nodes: vec![Node::root()] }
    }

    pub fn start_position(&self) -> Chess {
        self.setup.clone().unwrap_or_default()
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id.index()]
    }

    pub fn root(&self) -> &Node {
        &self.nodes[0]
    }

    pub fn contains(&self, id: NodeId) -> bool {
        id.index() < self.nodes.len()
    }

    /// True if `id` is still connected to the root (not deleted).
    pub fn is_attached(&self, id: NodeId) -> bool {
        if !self.contains(id) {
            return false;
        }
        let mut cur = id;
        while let Some(parent) = self.node(cur).parent {
            if !self.node(parent).children.contains(&cur) {
                return false;
            }
            cur = parent;
        }
        cur == NodeId::ROOT
    }

    /// Appends `mv` as the last child of `parent`. The move must be legal in
    /// the position at `parent`; callers are responsible for that.
    pub fn add_child(&mut self, parent: NodeId, mv: Move) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(Node { mv: Some(mv), parent: Some(parent), ..Node::default() });
        self.nodes[parent.index()].children.push(id);
        id
    }

    /// Child of `parent` playing `mv`, if any.
    pub fn find_child(&self, parent: NodeId, mv: Move) -> Option<NodeId> {
        self.node(parent).children.iter().copied().find(|&c| self.node(c).mv == Some(mv))
    }

    /// Plays `moves` from `from`, following moves that already exist and
    /// adding the rest (as a new variation where a continuation exists).
    /// The moves must be legal in sequence. Returns the last node, or `from`
    /// if `moves` is empty.
    pub fn add_line(&mut self, from: NodeId, moves: &[Move]) -> NodeId {
        let mut cur = from;
        for &mv in moves {
            cur = match self.find_child(cur, mv) {
                Some(existing) => existing,
                None => self.add_child(cur, mv),
            };
        }
        cur
    }

    /// Copies the tree below `src` under `dst`, replaying every move by its
    /// piece, squares and promotion in the position at `dst` (GAME-03
    /// "insert"). A line stops at its first move that is no longer legal.
    /// Annotations are copied along. Returns how many moves were dropped.
    pub fn replay_below(&mut self, src: NodeId, dst: NodeId) -> usize {
        let mut dropped = 0;
        let mut stack = vec![(src, dst, self.position_at(dst))];
        while let Some((from, to, pos)) = stack.pop() {
            for child in self.node(from).children.clone() {
                let old = self.node(child).mv.expect("non-root node has a move");
                let replayed = pos.legal_moves().into_iter().find(|m| {
                    m.role() == old.role()
                        && m.from() == old.from()
                        && m.to() == old.to()
                        && m.promotion() == old.promotion()
                });
                let Some(mv) = replayed else {
                    dropped += self.subtree_len(child);
                    continue;
                };
                let copy = self.add_child(to, mv);
                let source = self.node(child).clone();
                let target = self.node_mut(copy);
                target.comment_before = source.comment_before;
                target.comment_after = source.comment_after;
                target.nags = source.nags;
                target.shapes = source.shapes;
                target.commands = source.commands;
                let mut next = pos.clone();
                next.play_unchecked(mv);
                stack.push((child, copy, next));
            }
        }
        dropped
    }

    /// Number of moves in the subtree starting at `id` (`id` included).
    fn subtree_len(&self, id: NodeId) -> usize {
        let mut n = 0;
        let mut stack = vec![id];
        while let Some(cur) = stack.pop() {
            n += 1;
            stack.extend(&self.node(cur).children);
        }
        n
    }

    /// Nodes from the first move to `id` (root excluded).
    pub fn path(&self, id: NodeId) -> Vec<NodeId> {
        let mut path = Vec::new();
        let mut cur = id;
        while let Some(parent) = self.node(cur).parent {
            path.push(cur);
            cur = parent;
        }
        path.reverse();
        path
    }

    pub fn position_at(&self, id: NodeId) -> Chess {
        let mut pos = self.start_position();
        for n in self.path(id) {
            pos.play_unchecked(self.node(n).mv.expect("non-root node has a move"));
        }
        pos
    }

    /// Position at `id` plus hashes of all earlier positions on the path,
    /// as needed by [`position::status`].
    pub fn position_with_history(&self, id: NodeId) -> (Chess, Vec<u64>) {
        let mut pos = self.start_position();
        let path = self.path(id);
        let mut history = Vec::with_capacity(path.len());
        for n in path {
            history.push(position::hash(&pos));
            pos.play_unchecked(self.node(n).mv.expect("non-root node has a move"));
        }
        (pos, history)
    }

    /// Number of plies in the main line.
    pub fn mainline_len(&self) -> usize {
        let mut n = 0;
        let mut cur = NodeId::ROOT;
        while let Some(&next) = self.node(cur).children.first() {
            n += 1;
            cur = next;
        }
        n
    }

    /// Last node of the main line continuation starting at `id`.
    pub fn line_end(&self, id: NodeId) -> NodeId {
        let mut cur = id;
        while let Some(&next) = self.node(cur).children.first() {
            cur = next;
        }
        cur
    }

    /// First node of the variation containing `id`: the nearest
    /// ancestor-or-self that is not its parent's main continuation.
    /// `None` if `id` is on the main line.
    pub fn variation_start(&self, id: NodeId) -> Option<NodeId> {
        let mut cur = id;
        while let Some(parent) = self.node(cur).parent {
            if self.node(parent).children.first() != Some(&cur) {
                return Some(cur);
            }
            cur = parent;
        }
        None
    }

    /// The first node whose position is `target`, ignoring the move
    /// counters (QOL-10 "go to position"): on the main line if it's there,
    /// else in a variation.
    pub fn find_position(&self, target: &Chess) -> Option<NodeId> {
        let want = position::hash(target);
        // Pre-order, so the whole main line comes before any variation.
        let mut stack = vec![(NodeId::ROOT, self.start_position())];
        while let Some((id, pos)) = stack.pop() {
            if position::hash(&pos) == want {
                return Some(id);
            }
            for &child in self.node(id).children.iter().rev() {
                let mut next = pos.clone();
                next.play_unchecked(self.node(child).mv.expect("non-root node has a move"));
                stack.push((child, next));
            }
        }
        None
    }

    /// Attached nodes in pre-order (root first, children in order).
    pub fn preorder(&self) -> Vec<NodeId> {
        let mut out = Vec::with_capacity(self.nodes.len());
        let mut stack = vec![NodeId::ROOT];
        while let Some(id) = stack.pop() {
            out.push(id);
            stack.extend(self.node(id).children.iter().rev());
        }
        out
    }

    pub fn has_variations(&self) -> bool {
        self.preorder().iter().any(|&id| self.node(id).children.len() > 1)
    }

    pub fn has_comments(&self) -> bool {
        self.preorder().iter().any(|&id| {
            let n = self.node(id);
            !n.comment_before.is_empty() || !n.comment_after.is_empty()
        })
    }

    fn subtree_eq(&self, a: NodeId, other: &Game, b: NodeId) -> bool {
        let (x, y) = (self.node(a), other.node(b));
        x.mv == y.mv
            && x.comment_before == y.comment_before
            && x.comment_after == y.comment_after
            && x.nags == y.nags
            && x.shapes == y.shapes
            && x.commands == y.commands
            && x.children.len() == y.children.len()
            && x.children.iter().zip(&y.children).all(|(&ca, &cb)| self.subtree_eq(ca, other, cb))
    }
}

/// Structural equality: same headers, start position and attached tree.
/// Arena layout and detached nodes are ignored.
impl PartialEq for Game {
    fn eq(&self, other: &Game) -> bool {
        self.headers == other.headers
            && self.setup == other.setup
            && self.subtree_eq(NodeId::ROOT, other, NodeId::ROOT)
    }
}

impl Eq for Game {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::parse_uci;

    fn line(game: &mut Game, from: NodeId, ucis: &[&str]) -> NodeId {
        let mut cur = from;
        for u in ucis {
            let m = parse_uci(&game.position_at(cur), u).unwrap();
            cur = game.add_child(cur, m);
        }
        cur
    }

    #[test]
    fn tree_navigation() {
        let mut g = Game::default();
        let e5 = line(&mut g, NodeId::ROOT, &["e2e4", "e7e5", "g1f3"]);
        let e4 = g.path(e5)[0];
        let c5 = line(&mut g, e4, &["c7c5", "g1f3"]);
        assert_eq!(g.mainline_len(), 3);
        assert_eq!(g.variation_start(e5), None);
        assert_eq!(g.variation_start(c5), Some(g.path(c5)[1]));
        assert_eq!(g.preorder().len(), 6);
        assert!(g.has_variations());
        assert_eq!(
            crate::position::to_fen(&g.position_at(c5)),
            "rnbqkbnr/pp1ppppp/8/2p5/4P3/5N2/PPPP1PPP/RNBQKB1R b KQkq - 1 2"
        );
    }

    #[test]
    fn finds_a_position_main_line_first() {
        use crate::position::{from_fen, hash, to_fen};
        let mut g = Game::default();
        // 1. Nf3 e5 2. e4 (main line) and 1. e4 e5 2. Nf3 (a variation) reach
        // the same position; 1. Nf3 Nf6 2. Ng1 Ng8 goes back to the start.
        let main = line(&mut g, NodeId::ROOT, &["g1f3", "e7e5", "e2e4"]);
        let var = line(&mut g, NodeId::ROOT, &["e2e4", "e7e5", "g1f3"]);
        let nf3 = g.path(main)[0];
        let back = line(&mut g, nf3, &["g8f6", "f3g1", "f6g8"]);
        assert_eq!(hash(&g.position_at(back)), hash(&Chess::default()));
        assert_eq!(g.find_position(&Chess::default()), Some(NodeId::ROOT));
        let pos = g.position_at(var);
        assert_eq!(g.find_position(&pos), Some(main), "the main line comes first");
        let e4 = g.path(var)[0];
        assert_eq!(g.find_position(&g.position_at(e4)), Some(e4), "found in a variation");
        // The move counters don't matter; castling rights do.
        let fen = to_fen(&pos);
        assert_eq!(g.find_position(&from_fen(&fen.replace(" 1 2", " 0 9")).unwrap()), Some(main));
        assert_eq!(g.find_position(&from_fen(&fen.replace("KQkq", "Qkq")).unwrap()), None);
        g.node_mut(NodeId::ROOT).children.clear();
        assert_eq!(g.find_position(&pos), None, "detached lines aren't searched");
    }

    #[test]
    fn detached_nodes_are_ignored_by_eq() {
        let mut a = Game::default();
        line(&mut a, NodeId::ROOT, &["e2e4"]);
        let mut b = a.clone();
        let d4 = line(&mut b, NodeId::ROOT, &["d2d4"]);
        assert_ne!(a, b);
        b.node_mut(NodeId::ROOT).children.retain(|&c| c != d4);
        assert!(!b.is_attached(d4));
        assert_eq!(a, b);
    }
}
