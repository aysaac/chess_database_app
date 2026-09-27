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
