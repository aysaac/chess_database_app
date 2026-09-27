//! Editing session over a [`Game`]: cursor, variation-aware move input,
//! annotation edits and snapshot-based undo/redo.

use shakmaty::Move;

use super::{Game, Headers, Nag, NodeId, Shape};

const UNDO_LIMIT: usize = 200;

/// What to do when a move is played where a different continuation exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariationPolicy {
    /// Add as a new variation (last alternative).
    NewVariation,
    /// Insert as the new main continuation; the old one becomes a variation.
    NewMainLine,
    /// Replace the main continuation (and everything after it).
    Overwrite,
    /// Replace the main continuation's move but keep the moves after it,
    /// as far as they stay legal (GAME-03).
    Insert,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayResult {
    /// Cursor moved to `node`; `created` is false if the move already existed.
    /// `dropped` counts moves an insert removed because they became illegal.
    Moved { node: NodeId, created: bool, dropped: usize },
    /// A different continuation exists and no policy was given.
    NeedsChoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentSlot {
    Before,
    After,
}

#[derive(Debug, Clone)]
struct Snapshot {
    game: Game,
    cursor: NodeId,
    revision: u64,
}

#[derive(Debug, Clone)]
pub struct GameEditor {
    game: Game,
    cursor: NodeId,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Content revision; changes on every edit, restored by undo/redo.
    revision: u64,
    next_revision: u64,
    saved_revision: u64,
}

impl GameEditor {
    pub fn new(game: Game) -> GameEditor {
        GameEditor {
            game,
            cursor: NodeId::ROOT,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            next_revision: 1,
            saved_revision: 0,
        }
    }

    pub fn game(&self) -> &Game {
        &self.game
    }

    pub fn cursor(&self) -> NodeId {
        self.cursor
    }

    /// Changes whenever the game content changes (not on navigation).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    pub fn mark_saved(&mut self) {
        self.saved_revision = self.revision;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    // ---- navigation -------------------------------------------------------

    pub fn goto(&mut self, node: NodeId) -> bool {
        if self.game.is_attached(node) {
            self.cursor = node;
            true
        } else {
            false
        }
    }

    pub fn forward(&mut self) -> bool {
        match self.game.node(self.cursor).children.first() {
            Some(&next) => {
                self.cursor = next;
                true
            }
            None => false,
        }
    }

    pub fn back(&mut self) -> bool {
        match self.game.node(self.cursor).parent {
            Some(parent) => {
                self.cursor = parent;
                true
            }
            None => false,
        }
    }

    pub fn to_start(&mut self) {
        self.cursor = NodeId::ROOT;
    }

    /// Jumps to the end of the current line.
    pub fn to_end(&mut self) {
        self.cursor = self.game.line_end(self.cursor);
    }

    // ---- edits ------------------------------------------------------------

    fn checkpoint(&mut self) {
        self.undo.push(Snapshot { game: self.game.clone(), cursor: self.cursor, revision: self.revision });
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.revision = self.next_revision;
        self.next_revision += 1;
    }

    /// Plays a legal move `mv` at the cursor.
    pub fn play(&mut self, mv: Move, policy: Option<VariationPolicy>) -> PlayResult {
        let at = self.cursor;
        if let Some(existing) = self.game.find_child(at, mv) {
            self.cursor = existing;
            return PlayResult::Moved { node: existing, created: false, dropped: 0 };
        }
        let has_continuation = !self.game.node(at).children.is_empty();
        let policy = match (has_continuation, policy) {
            (false, _) => VariationPolicy::NewVariation,
            (true, Some(p)) => p,
            (true, None) => return PlayResult::NeedsChoice,
        };
        self.checkpoint();
        let node = self.game.add_child(at, mv);
        let children = &mut self.game.node_mut(at).children;
        let replaced = children[0];
        let mut dropped = 0;
        match policy {
            VariationPolicy::NewVariation => {}
            VariationPolicy::NewMainLine => {
                children.pop();
                children.insert(0, node);
            }
            VariationPolicy::Overwrite => {
                children.pop();
                children[0] = node;
            }
            VariationPolicy::Insert => {
                children.pop();
                children[0] = node;
                dropped = self.game.replay_below(replaced, node);
            }
        }
        self.cursor = node;
        PlayResult::Moved { node, created: true, dropped }
    }

    /// Makes the variation containing `node` the main continuation at its
    /// branch point. Returns false if `node` is already on the main line.
    pub fn promote_variation(&mut self, node: NodeId) -> bool {
        let Some(start) = self.game.variation_start(node) else { return false };
        self.checkpoint();
        let parent = self.game.node(start).parent.expect("variation start has a parent");
        let children = &mut self.game.node_mut(parent).children;
        children.retain(|&c| c != start);
        children.insert(0, start);
        true
    }

    /// Deletes the variation containing `node`. Returns false on the main line.
    pub fn delete_variation(&mut self, node: NodeId) -> bool {
        let Some(start) = self.game.variation_start(node) else { return false };
        self.checkpoint();
        let parent = self.game.node(start).parent.expect("variation start has a parent");
        self.game.node_mut(parent).children.retain(|&c| c != start);
        if !self.game.is_attached(self.cursor) {
            self.cursor = parent;
        }
        true
    }

    /// Deletes everything after `node` (all continuations and variations).
    pub fn delete_remaining(&mut self, node: NodeId) -> bool {
        if self.game.node(node).children.is_empty() {
            return false;
        }
        self.checkpoint();
        self.game.node_mut(node).children.clear();
        if !self.game.is_attached(self.cursor) {
            self.cursor = node;
        }
        true
    }

    /// Sets a comment. A comment *before* a move that does not start a
    /// variation is stored as the previous node's after-comment, which is
    /// how PGN represents it.
    pub fn set_comment(&mut self, node: NodeId, slot: CommentSlot, text: &str) {
        let text = text.trim().to_owned();
        let (target, before) = match slot {
            CommentSlot::After => (node, false),
            CommentSlot::Before => match self.game.node(node).parent {
                Some(parent) if self.game.node(parent).children.first() == Some(&node) => (parent, false),
                Some(_) => (node, true),
                None => (node, false),
            },
        };
        let current = {
            let n = self.game.node(target);
            if before { &n.comment_before } else { &n.comment_after }
        };
        if *current == text {
            return;
        }
        self.checkpoint();
        let n = self.game.node_mut(target);
        if before {
            n.comment_before = text;
        } else {
            n.comment_after = text;
        }
    }

    /// Toggles a NAG; move and position assessments are each exclusive.
    pub fn toggle_nag(&mut self, node: NodeId, nag: Nag) {
        if node == NodeId::ROOT {
            return;
        }
        self.checkpoint();
        let nags = &mut self.game.node_mut(node).nags;
        if let Some(i) = nags.iter().position(|&n| n == nag) {
            nags.remove(i);
        } else {
            let category = nag.category();
            if category != super::NagCategory::Other {
                nags.retain(|n| n.category() != category);
            }
            nags.push(nag);
            nags.sort_by_key(|n| (n.category() as u8, n.0));
        }
    }

    pub fn set_shapes(&mut self, node: NodeId, shapes: Vec<Shape>) {
        if self.game.node(node).shapes == shapes {
            return;
        }
        self.checkpoint();
        self.game.node_mut(node).shapes = shapes;
    }

    /// Adds `moves` (legal in sequence from `at`) as a line, following moves
    /// that already exist: an engine line pasted into the notation (ENG-05).
    /// `comment` goes after the last move. Moves the cursor to the end of
    /// the line when `goto` is set. One undo step; returns the last node.
    pub fn add_line(&mut self, at: NodeId, moves: &[Move], comment: Option<&str>, goto: bool) -> NodeId {
        if moves.is_empty() {
            return at;
        }
        let mut probe = at;
        let exists = moves.iter().all(|&m| match self.game.find_child(probe, m) {
            Some(c) => {
                probe = c;
                true
            }
            None => false,
        });
        let changes_comment =
            comment.is_some_and(|c| !exists || self.game.node(probe).comment_after != c.trim());
        if !exists || changes_comment {
            self.checkpoint();
        }
        let end = self.game.add_line(at, moves);
        if let Some(c) = comment.filter(|_| changes_comment) {
            self.game.node_mut(end).comment_after = c.trim().to_owned();
        }
        if goto {
            self.cursor = end;
        }
        end
    }

    /// Applies an arbitrary edit to the game as one undo step. `f` returns
    /// whether it changed anything; if not, no undo step is recorded.
    pub fn edit_with(&mut self, f: impl FnOnce(&mut Game) -> bool) -> bool {
        let before = self.game.clone();
        if !f(&mut self.game) {
            self.game = before;
            return false;
        }
        let after = std::mem::replace(&mut self.game, before);
        self.checkpoint();
        self.game = after;
        if !self.game.is_attached(self.cursor) {
            self.cursor = NodeId::ROOT;
        }
        true
    }

    pub fn set_headers(&mut self, headers: Headers) {
        if self.game.headers == headers {
            return;
        }
        self.checkpoint();
        self.game.headers = headers;
    }

    pub fn undo(&mut self) -> bool {
        let Some(snap) = self.undo.pop() else { return false };
        self.redo.push(Snapshot {
            game: std::mem::replace(&mut self.game, snap.game),
            cursor: self.cursor,
            revision: self.revision,
        });
        self.cursor = snap.cursor;
        self.revision = snap.revision;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(snap) = self.redo.pop() else { return false };
        self.undo.push(Snapshot {
            game: std::mem::replace(&mut self.game, snap.game),
            cursor: self.cursor,
            revision: self.revision,
        });
        self.cursor = snap.cursor;
        self.revision = snap.revision;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::parse_uci;
    use shakmaty::Position;

    fn play(ed: &mut GameEditor, uci: &str, policy: Option<VariationPolicy>) -> PlayResult {
        let pos = ed.game().position_at(ed.cursor());
        ed.play(parse_uci(&pos, uci).unwrap(), policy)
    }

    fn mainline(ed: &GameEditor) -> Vec<String> {
        let g = ed.game();
        let mut out = Vec::new();
        let mut cur = NodeId::ROOT;
        while let Some(&next) = g.node(cur).children.first() {
            out.push(crate::notation::uci(g.node(next).mv.unwrap()));
            cur = next;
        }
        out
    }

    fn editor_with_mainline() -> GameEditor {
        let mut ed = GameEditor::new(Game::default());
        for m in ["e2e4", "e7e5", "g1f3"] {
            play(&mut ed, m, None);
        }
        ed
    }

    #[test]
    fn existing_move_just_navigates() {
        let mut ed = editor_with_mainline();
        ed.to_start();
        let rev = ed.revision();
        assert!(matches!(play(&mut ed, "e2e4", None), PlayResult::Moved { created: false, .. }));
        assert_eq!(ed.revision(), rev);
    }

    #[test]
    fn variation_policies() {
        let mut ed = editor_with_mainline();
        ed.to_start();
        ed.forward();
        assert_eq!(play(&mut ed, "c7c5", None), PlayResult::NeedsChoice);

        play(&mut ed, "c7c5", Some(VariationPolicy::NewVariation));
        assert_eq!(mainline(&ed), ["e2e4", "e7e5", "g1f3"]);

        ed.undo();
        play(&mut ed, "c7c5", Some(VariationPolicy::NewMainLine));
        assert_eq!(mainline(&ed), ["e2e4", "c7c5"]);
        assert_eq!(ed.game().node(ed.game().path(ed.cursor())[0]).children.len(), 2);

        ed.undo();
        play(&mut ed, "c7c5", Some(VariationPolicy::Overwrite));
        assert_eq!(mainline(&ed), ["e2e4", "c7c5"]);
        assert_eq!(ed.game().node(ed.game().path(ed.cursor())[0]).children.len(), 1);
    }

    #[test]
    fn insert_keeps_following_moves_while_legal() {
        // 1.e4 e5 2.Nf3 Nc6 3.Bb5 (3.Bc4 Bc5) a6: insert 2.Nc3 for 2.Nf3.
        let mut ed = GameEditor::new(Game::default());
        for m in ["e2e4", "e7e5", "g1f3", "b8c6", "f1b5", "a7a6"] {
            play(&mut ed, m, None);
        }
        let path = ed.game().path(ed.cursor());
        ed.goto(path[3]);
        play(&mut ed, "f1c4", Some(VariationPolicy::NewVariation));
        play(&mut ed, "f8c5", None);
        ed.toggle_nag(path[4], Nag(1));
        ed.set_comment(path[5], CommentSlot::After, "Morphy");
        let rev = ed.revision();

        ed.goto(path[1]);
        let r = play(&mut ed, "b1c3", Some(VariationPolicy::Insert));
        assert!(matches!(r, PlayResult::Moved { created: true, dropped: 0, .. }));
        assert_eq!(mainline(&ed), ["e2e4", "e7e5", "b1c3", "b8c6", "f1b5", "a7a6"]);
        let new = ed.game().path(ed.game().line_end(ed.cursor()));
        assert_eq!(ed.game().node(new[4]).nags, vec![Nag(1)]);
        assert_eq!(ed.game().node(new[5]).comment_after, "Morphy");
        assert_eq!(ed.game().node(new[3]).children.len(), 2, "the Bc4 variation survives");

        // 1...d5 for 1...e5: the main line stays, but in the 3.Bc4 variation
        // 3...Bc5 is dropped (the e7 pawn now blocks the bishop).
        ed.goto(NodeId::ROOT);
        ed.forward();
        let r = play(&mut ed, "d7d5", Some(VariationPolicy::Insert));
        assert!(matches!(r, PlayResult::Moved { dropped: 1, .. }), "{r:?}");
        assert_eq!(mainline(&ed), ["e2e4", "d7d5", "b1c3", "b8c6", "f1b5", "a7a6"]);

        ed.undo();
        ed.undo();
        assert_eq!(ed.revision(), rev, "each insert is one undo step");
    }

    #[test]
    fn insert_stops_a_line_at_its_first_illegal_move() {
        let mut ed = editor_with_mainline();
        ed.to_start();
        // 1.d4 for 1.e4: 1...e5 is still legal, 2.Nf3 too.
        let r = play(&mut ed, "d2d4", Some(VariationPolicy::Insert));
        assert!(matches!(r, PlayResult::Moved { dropped: 0, .. }));
        assert_eq!(mainline(&ed), ["d2d4", "e7e5", "g1f3"]);
        ed.undo();
        // 1.Nf3 for 1.e4: 1...e5 is legal, but 2.Nf3 isn't (the knight left g1).
        let r = play(&mut ed, "g1f3", Some(VariationPolicy::Insert));
        assert!(matches!(r, PlayResult::Moved { dropped: 1, .. }));
        assert_eq!(mainline(&ed), ["g1f3", "e7e5"]);
    }

    #[test]
    fn promote_and_delete_variation() {
        let mut ed = editor_with_mainline();
        ed.to_start();
        ed.forward();
        play(&mut ed, "c7c5", Some(VariationPolicy::NewVariation));
        let c5 = ed.cursor();
        play(&mut ed, "g1f3", None);
        let nf3 = ed.cursor();

        assert!(ed.promote_variation(nf3));
        assert_eq!(mainline(&ed), ["e2e4", "c7c5", "g1f3"]);
        assert!(!ed.promote_variation(nf3), "already main line");

        let e5 = ed.game().node(ed.game().path(c5)[0]).children[1];
        assert!(ed.delete_variation(e5));
        assert_eq!(ed.game().node(ed.game().path(c5)[0]).children, vec![c5]);
        assert!(!ed.game().is_attached(e5));
    }

    #[test]
    fn delete_remaining_moves_cursor_back() {
        let mut ed = editor_with_mainline();
        let first = ed.game().path(ed.cursor())[0];
        assert!(ed.delete_remaining(first));
        assert_eq!(ed.cursor(), first);
        assert_eq!(mainline(&ed), ["e2e4"]);
        assert!(!ed.delete_remaining(first));
    }

    #[test]
    fn comment_before_mainline_move_goes_to_previous_node() {
        let mut ed = editor_with_mainline();
        let path = ed.game().path(ed.cursor());
        ed.set_comment(path[1], CommentSlot::Before, "  intro to e5 ");
        assert_eq!(ed.game().node(path[0]).comment_after, "intro to e5");
        assert_eq!(ed.game().node(path[1]).comment_before, "");
        ed.set_comment(path[0], CommentSlot::Before, "Intro");
        assert_eq!(ed.game().root().comment_after, "Intro", "before the first move is the intro");

        ed.to_start();
        ed.forward();
        play(&mut ed, "c7c5", Some(VariationPolicy::NewVariation));
        let c5 = ed.cursor();
        ed.set_comment(c5, CommentSlot::Before, "Sicilian");
        assert_eq!(ed.game().node(c5).comment_before, "Sicilian");
    }

    #[test]
    fn nag_toggling_is_exclusive_per_category() {
        let mut ed = editor_with_mainline();
        let n = ed.cursor();
        ed.toggle_nag(n, Nag(1));
        ed.toggle_nag(n, Nag(16));
        ed.toggle_nag(n, Nag(2));
        assert_eq!(ed.game().node(n).nags, vec![Nag(2), Nag(16)]);
        ed.toggle_nag(n, Nag(2));
        assert_eq!(ed.game().node(n).nags, vec![Nag(16)]);
    }

    #[test]
    fn undo_redo_and_dirty_tracking() {
        let mut ed = GameEditor::new(Game::default());
        assert!(!ed.is_dirty());
        play(&mut ed, "e2e4", None);
        play(&mut ed, "e7e5", None);
        assert!(ed.is_dirty());
        ed.mark_saved();
        assert!(!ed.is_dirty());

        assert!(ed.undo());
        assert!(ed.is_dirty());
        assert_eq!(mainline(&ed), ["e2e4"]);
        assert!(ed.redo());
        assert!(!ed.is_dirty());
        assert_eq!(mainline(&ed), ["e2e4", "e7e5"]);

        ed.undo();
        play(&mut ed, "c7c5", None);
        assert!(!ed.can_redo(), "new edit clears redo");
        assert!(ed.is_dirty());
    }

    #[test]
    fn add_line_follows_existing_moves_and_is_one_undo_step() {
        let mut ed = editor_with_mainline();
        let pos = Game::default().start_position();
        let e4 = parse_uci(&pos, "e2e4").unwrap();
        let mut after_e4 = pos.clone();
        after_e4.play_unchecked(e4);
        let c5 = parse_uci(&after_e4, "c7c5").unwrap();
        let rev = ed.revision();
        let end = ed.add_line(NodeId::ROOT, &[e4, c5], Some("Sicilian"), false);
        assert_eq!(mainline(&ed), ["e2e4", "e7e5", "g1f3"], "added as a variation");
        assert_eq!(ed.game().node(end).comment_after, "Sicilian");
        assert_ne!(ed.cursor(), end, "cursor stays");
        assert!(ed.undo());
        assert_eq!(ed.revision(), rev);
        assert!(!ed.game().is_attached(end));

        // An existing line only moves the cursor.
        let rev = ed.revision();
        let end = ed.add_line(NodeId::ROOT, &[e4], None, true);
        assert_eq!(ed.cursor(), end);
        assert_eq!(ed.revision(), rev);
    }

    #[test]
    fn edit_with_records_one_step_only_on_change() {
        let mut ed = editor_with_mainline();
        let rev = ed.revision();
        assert!(!ed.edit_with(|_| false));
        assert_eq!(ed.revision(), rev);
        assert!(!ed.can_undo() || ed.revision() == rev);
        let last = ed.cursor();
        assert!(ed.edit_with(|g| {
            g.node_mut(last).nags.push(Nag(1));
            g.node_mut(last).comment_after = "good".into();
            true
        }));
        assert_eq!(ed.game().node(last).nags, vec![Nag(1)]);
        ed.undo();
        assert!(ed.game().node(last).nags.is_empty() && ed.game().node(last).comment_after.is_empty());
    }

    #[test]
    fn goto_rejects_detached_nodes() {
        let mut ed = editor_with_mainline();
        let first = ed.game().path(ed.cursor())[0];
        let last = ed.cursor();
        ed.delete_remaining(first);
        assert!(!ed.goto(last));
        assert!(ed.goto(first));
    }
}
